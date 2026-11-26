//! The Laser plugin: cut, score and engrave on a Ruida (RDC644x) laser over
//! its USB cable. Colour layers carry speed / power / passes; the Laser tab
//! connects, jogs, frames and starts jobs (after a confirmation), and
//! previews the toolpaths on the canvas.

pub mod panel;
pub mod plan;
pub mod settings;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use egui::Stroke;
use egui_phosphor::regular as icon;
use mtzd_ruida::emulator::{EmulatedTransport, Emulator};
use mtzd_ruida::session::{Config, Event, Opener, Request, Session, Status};
use mtzd_ruida::transport::{Handshake, SerialTransport, Transport};
use serde_json::Value;

use self::plan::Plan;
use self::settings::{Area, DocSettings, MachineSettings};
use super::{PanelSpec, Plugin, PluginCommand};
use crate::app::App;

pub const ID: &str = "laser";
pub const EMULATOR: &str = "emulator";
const LOG_LINES: usize = 500;

#[derive(Clone, Debug, PartialEq)]
pub enum Conn {
    Off,
    Connecting(String),
    On { link: String, baud: u32, card_id: u64 },
}

/// The tabs under the layer table (as in LightBurn's dock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sub {
    Laser,
    Move,
    Library,
    Console,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Busy {
    Idle,
    Uploading { sent: usize, total: usize },
    Running,
    Framing,
    Moving,
}

/// A job waiting for the user's go-ahead.
pub struct Confirm {
    pub job: mtzd_ruida::Job,
    pub estimate: mtzd_ruida::Estimate,
    pub name: String,
}

pub struct LaserPlugin {
    pub machine: MachineSettings,
    session: Option<Session>,
    /// The emulator behind an `emulator` connection.
    pub emulator: Option<Arc<Mutex<Emulator>>>,
    pub conn: Conn,
    pub status: Option<Status>,
    pub busy: Busy,
    pub paused: bool,
    /// Wire log: (sent by us, text).
    pub log: VecDeque<(bool, String)>,
    pub error: Option<String>,
    pub preview: bool,
    /// The layer being edited (its colour key).
    pub selected: Option<String>,
    pub sub: Sub,
    /// The Machine Settings dialog is open.
    pub machine_open: bool,
    /// When the Laser tab was last drawn (the canvas shows the job's start
    /// and the bed while it's open).
    panel_seen: f64,
    /// The layer in the Cut Settings dialog.
    pub editing: Option<String>,
    /// Name typed for a new preset (Some while the save field shows).
    pub preset_name: Option<String>,
    /// The plan and what it was made from.
    plan: Option<(PlanKey, Arc<Plan>)>,
    pub confirm: Option<Confirm>,
    pub confirm_disconnect: bool,
    pub ports: Vec<String>,
    ports_at: f64,
}

#[derive(Clone, PartialEq)]
struct PlanKey {
    doc: u64,
    rev: u64,
    fonts: u64,
    settings: Value,
    area: Area,
    artboard: u64,
    selection: Vec<u64>,
}

impl Default for LaserPlugin {
    fn default() -> Self {
        LaserPlugin {
            machine: MachineSettings::default(),
            session: None,
            emulator: None,
            conn: Conn::Off,
            status: None,
            busy: Busy::Idle,
            paused: false,
            log: VecDeque::new(),
            error: None,
            preview: false,
            selected: None,
            sub: Sub::Laser,
            machine_open: false,
            editing: None,
            panel_seen: f64::NEG_INFINITY,
            preset_name: None,
            plan: None,
            confirm: None,
            confirm_disconnect: false,
            ports: Vec::new(),
            ports_at: f64::NEG_INFINITY,
        }
    }
}

/// The laser settings of a document.
pub fn doc_settings(app: &App) -> DocSettings {
    app.doc.plugin_data.get(ID).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// Save the document's laser settings (one undo step per edit gesture).
pub fn set_doc_settings(app: &mut App, s: &DocSettings) {
    if doc_settings(app) == *s {
        return;
    }
    app.checkpoint("laser-layers");
    app.doc.plugin_data.insert(ID.into(), serde_json::to_value(s).unwrap_or(Value::Null));
}

impl LaserPlugin {
    pub fn connected(&self) -> bool {
        matches!(self.conn, Conn::On { .. })
    }

    /// A session exists (connecting or connected).
    pub fn session_open(&self) -> bool {
        self.session.is_some()
    }

    pub fn running(&self) -> bool {
        matches!(self.busy, Busy::Uploading { .. } | Busy::Running)
    }

    fn logs(&mut self, tx: bool, text: impl Into<String>) {
        self.log.push_back((tx, text.into()));
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    pub fn refresh_ports(&mut self, now: f64) {
        if now - self.ports_at > 2.0 {
            self.ports_at = now;
            self.ports = mtzd_ruida::transport::ports();
        }
    }

    /// The artboard the job is placed from.
    fn artboard(app: &mut App) -> u64 {
        let sel = app.selection_bounds().and_then(|b| app.doc.artboard_for(b).map(|a| a.id));
        sel.unwrap_or(app.doc.primary().id)
    }

    /// The current plan (made again when the document or settings changed).
    pub fn plan(&mut self, app: &mut App) -> Arc<Plan> {
        let settings = doc_settings(app);
        let area = if self.machine.area == Area::Selection && app.selection.is_empty() { Area::Artboard } else { self.machine.area };
        let key = PlanKey {
            doc: app.doc_uid,
            rev: app.doc.revision(),
            fonts: app.fonts.generation,
            settings: serde_json::to_value(&settings).unwrap_or(Value::Null),
            area,
            artboard: Self::artboard(app),
            selection: if area == Area::Selection { app.selection.clone() } else { Vec::new() },
        };
        if let Some((k, p)) = &self.plan
            && *k == key
        {
            return p.clone();
        }
        let p = Arc::new(plan::plan(&app.doc, &mut app.fonts, &settings, key.area, key.artboard, &key.selection));
        self.plan = Some((key, p.clone()));
        p
    }

    fn head(&self) -> Option<mtzd_ruida::Pt> {
        self.status.map(|s| s.pos)
    }

    /// The job as it would run now.
    pub fn job(&mut self, app: &mut App) -> Result<mtzd_ruida::Job, String> {
        let plan = self.plan(app);
        let job = plan::to_job(&plan, &self.machine, self.head())?;
        if job.is_empty() {
            return Err("Nothing to cut: no layer is on, or the area is empty".into());
        }
        Ok(job)
    }

    pub fn connect(&mut self, ctx: &egui::Context) {
        if self.session.is_some() {
            return;
        }
        let port = self.machine.port.clone();
        if port.is_empty() {
            self.error = Some("Pick the laser's port first".into());
            return;
        }
        let opener: Opener = if port == EMULATOR {
            let mut e = Emulator::default();
            e.run_polls = 12;
            let emu = e.shared();
            self.emulator = Some(emu.clone());
            Box::new(move |baud| Ok(Box::new(EmulatedTransport { emu: emu.clone(), baud }) as Box<dyn Transport>))
        } else {
            self.emulator = None;
            let path = port.clone();
            Box::new(move |baud| SerialTransport::open(&path, baud, Handshake::Hardware).map(|t| Box::new(t) as Box<dyn Transport>))
        };
        let cfg = Config { magic: self.machine.magic, ..Default::default() };
        let c = ctx.clone();
        self.session = Some(Session::start(opener, cfg, Arc::new(move || c.request_repaint())));
        self.conn = Conn::Connecting(port);
        self.error = None;
        self.status = None;
        self.busy = Busy::Idle;
    }

    pub fn disconnect(&mut self) {
        // Dropping the session asks its worker to close the port.
        self.session = None;
        self.conn = Conn::Off;
        self.status = None;
        self.busy = Busy::Idle;
        self.paused = false;
    }

    pub fn stop(&mut self) {
        if let Some(s) = &self.session {
            s.stop();
            self.logs(true, "Stop requested");
        }
    }

    fn send(&mut self, r: Request) {
        if let Some(s) = &self.session {
            s.send(r);
        }
    }

    /// Move the head by (dx, dy) mm as seen on the bed (x right, y down).
    pub fn jog(&mut self, dx: f64, dy: f64) {
        let Some(head) = self.head() else { return };
        let m = &self.machine;
        let user = plan::to_machine(m, (head.0 as f64 / 1000.0, head.1 as f64 / 1000.0));
        let to = ((user.0 + dx).clamp(0.0, m.bed_w), (user.1 + dy).clamp(0.0, m.bed_h));
        let mm = plan::to_machine(m, to);
        self.busy = Busy::Moving;
        self.send(Request::Rapid { to: ((mm.0 * 1000.0).round() as i64, (mm.1 * 1000.0).round() as i64), light: false });
    }

    /// Rapid move to the user origin.
    pub fn go_to_origin(&mut self) {
        let Some(o) = self.machine.user_origin else { return };
        let mm = plan::to_machine(&self.machine, o);
        self.busy = Busy::Moving;
        self.send(Request::Rapid { to: ((mm.0 * 1000.0).round() as i64, (mm.1 * 1000.0).round() as i64), light: false });
    }

    pub fn home(&mut self) {
        self.busy = Busy::Moving;
        self.send(Request::Home);
    }

    pub fn frame(&mut self, app: &mut App) {
        match self.job(app) {
            Ok(job) => {
                if let Some(points) = plan::frame(&job) {
                    self.busy = Busy::Framing;
                    let light = self.machine.frame_light;
                    self.send(Request::Frame { points, light });
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Ask before starting (the modal sends it).
    pub fn ask_start(&mut self, app: &mut App) {
        match self.job(app) {
            Ok(job) => {
                let start = self.head().unwrap_or((0, 0));
                let estimate = job.estimate(start, self.machine.travel_speed);
                self.confirm = Some(Confirm { job, estimate, name: app.doc_name() });
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub(crate) fn start_confirmed(&mut self) {
        if let Some(c) = self.confirm.take() {
            self.paused = false;
            let stream = c.job.encode();
            self.busy = Busy::Uploading { sent: 0, total: stream.len() };
            self.logs(true, format!("Job “{}”: {} bytes", c.name, stream.len()));
            self.send(Request::Job { stream, name: c.name });
        }
    }

    pub fn toggle_pause(&mut self) {
        if self.paused {
            self.send(Request::Resume);
        } else {
            self.send(Request::Pause);
        }
        self.paused = !self.paused;
    }

    pub fn save_rd(&mut self, app: &mut App) {
        match self.job(app) {
            Ok(job) => {
                let name = app.doc_name();
                let stem = std::path::Path::new(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("job".into());
                if let Some(p) = rfd::FileDialog::new().add_filter("Ruida job", &["rd"]).set_file_name(format!("{stem}.rd")).save_file() {
                    match std::fs::write(&p, job.rd_file(self.machine.magic)) {
                        Ok(()) => app.notify(format!("Saved {}", p.display())),
                        Err(e) => self.error = Some(format!("Couldn't save: {e}")),
                    }
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Handle the session's events.
    pub fn poll(&mut self, app: &mut App) {
        let mut events = Vec::new();
        if let Some(s) = &self.session {
            while let Ok(e) = s.events.try_recv() {
                events.push(e);
            }
        }
        for e in events {
            match e {
                Event::Connected { link, baud, card_id } => {
                    self.logs(false, format!("Connected: {link}, card {card_id:#x}"));
                    self.conn = Conn::On { link, baud, card_id };
                }
                Event::Status(s) => {
                    let was = self.status.map(|s| s.running);
                    self.status = Some(s);
                    if self.busy == Busy::Running && was == Some(true) && !s.running {
                        self.busy = Busy::Idle;
                        self.paused = false;
                        app.notify("Laser job finished");
                    }
                    if self.busy == Busy::Moving {
                        self.busy = Busy::Idle;
                    }
                }
                Event::Progress { sent, total } => self.busy = Busy::Uploading { sent, total },
                Event::JobSent { name } => {
                    self.logs(false, format!("Job “{name}” sent"));
                    self.busy = Busy::Running;
                }
                Event::Framed => self.busy = Busy::Idle,
                Event::Stopped => {
                    self.busy = Busy::Idle;
                    self.paused = false;
                    self.logs(false, "Stopped");
                }
                Event::Log { tx, text } => self.logs(tx, text),
                Event::Error(e) => {
                    self.logs(false, format!("Error: {e}"));
                    self.error = Some(e);
                    if self.busy != Busy::Running {
                        self.busy = Busy::Idle;
                    }
                }
                Event::Disconnected => {
                    self.session = None;
                    self.conn = Conn::Off;
                    self.busy = Busy::Idle;
                    self.status = None;
                }
            }
        }
        if self.session.as_ref().is_some_and(|s| !s.is_alive()) && self.conn != Conn::Off {
            self.disconnect();
        }
    }

    fn confirm_ui(&mut self, ctx: &egui::Context) {
        use crate::ui::dialogs::{footer, modal, modal_text, modal_title};
        let Some(c) = &self.confirm else { return };
        let mut answer = None;
        let ((x0, y0), (x1, y1)) = c.job.bounds().unwrap_or(((0, 0), (0, 0)));
        let mm = |v: i64| v as f64 / 1000.0;
        let secs = c.estimate.seconds.round() as u64;
        let r = modal("laser_confirm").show(ctx, |ui| {
            ui.set_width(400.0);
            modal_title(ui, icon::WARNING, "Start the laser job?");
            modal_text(ui, "The laser will fire. Check that the lid is closed, air assist is on, and someone stays with the machine.");
            ui.add_space(10.0);
            egui::Grid::new("laser_confirm_grid").num_columns(2).spacing([18.0, 4.0]).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    ui.label(egui::RichText::new(k).color(crate::ui::theme::TEXT_DIM));
                    ui.label(v);
                    ui.end_row();
                };
                row(ui, "Layers", c.job.layers.len().to_string());
                row(ui, "Area", format!("{:.1} × {:.1} mm", mm(x1 - x0), mm(y1 - y0)));
                row(ui, "Machine position", format!("({:.1}, {:.1}) – ({:.1}, {:.1}) mm", mm(x0), mm(y0), mm(x1), mm(y1)));
                row(ui, "Max power", format!("{:.0} %", c.job.max_power()));
                row(ui, "Cut length", format!("{:.0} mm", c.estimate.cut_mm));
                row(ui, "Time (estimate)", format!("{}:{:02}", secs / 60, secs % 60));
            });
            match footer(ui, &[], &["Cancel"], "Start").as_deref() {
                Some("Start") => answer = Some(true),
                Some(_) => answer = Some(false),
                None => {}
            }
        });
        if r.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => self.start_confirmed(),
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    fn disconnect_ui(&mut self, ctx: &egui::Context) {
        use crate::ui::dialogs::{footer, modal, modal_text, modal_title};
        if !self.confirm_disconnect {
            return;
        }
        let mut answer = None;
        let r = modal("laser_disconnect").show(ctx, |ui| {
            ui.set_width(380.0);
            modal_title(ui, icon::WARNING, "Disconnect during the job?");
            modal_text(ui, "The machine keeps running the job it has. Stop stops it first.");
            match footer(ui, &["Stop and Disconnect"], &["Cancel"], "Disconnect").as_deref() {
                Some("Disconnect") => answer = Some(1),
                Some("Stop and Disconnect") => answer = Some(2),
                Some(_) => answer = Some(0),
                None => {}
            }
        });
        if r.should_close() && answer.is_none() {
            answer = Some(0);
        }
        if let Some(a) = answer {
            self.confirm_disconnect = false;
            if a == 2 {
                self.stop();
            }
            if a >= 1 {
                self.disconnect();
            }
        }
    }

    /// Make the job's artboard the size of the bed (keeping its top-left).
    pub fn fit_artboard(&mut self, app: &mut App) {
        let id = Self::artboard(app);
        let k = 25.4 / app.doc.dpi.max(1.0);
        let (w, h) = (self.machine.bed_w / k, self.machine.bed_h / k);
        let Some(a) = app.doc.artboard(id) else { return };
        if (a.w - w).abs() < 1e-6 && (a.h - h).abs() < 1e-6 {
            app.notify("The artboard already matches the bed");
            return;
        }
        app.checkpoint("");
        if let Some(a) = app.doc.artboard_mut(id) {
            a.w = w;
            a.h = h;
        }
        app.dirty = true;
        app.notify(format!("Artboard set to the {:.0} × {:.0} mm bed", self.machine.bed_w, self.machine.bed_h));
    }

    /// Disconnect, asking first while a job runs.
    pub fn toggle_connection(&mut self, ctx: &egui::Context) {
        if self.session.is_none() {
            self.connect(ctx);
        } else if self.running() {
            self.confirm_disconnect = true;
        } else {
            self.disconnect();
        }
    }
}

impl LaserPlugin {
    /// Where the job starts, the bed and the head, on the canvas (as
    /// LightBurn shows its job origin): a green square at the start point,
    /// the bed outline from the home corner, a red cross at the head.
    fn paint_marks(&self, plan: &Plan, app: &App, painter: &egui::Painter) {
        let view = &app.view;
        let to_screen = |p: kurbo::Point| {
            let q = view.affine() * p;
            egui::pos2(q.x as f32, q.y as f32)
        };
        let m = &self.machine;
        let green = egui::Color32::from_rgb(40, 190, 90);
        let font = egui::FontId::proportional(11.0);
        let label = |at: egui::Pos2, text: &str, col: egui::Color32| {
            let g = painter.layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE);
            let r = egui::Rect::from_min_size(at + egui::vec2(9.0, -g.size().y - 6.0), g.size() + egui::vec2(10.0, 4.0));
            painter.rect_filled(r, 3, col);
            painter.galley(r.min + egui::vec2(5.0, 2.0), g, egui::Color32::WHITE);
        };
        let k = plan.mm_per_px;
        // The bed (absolute placement: the artboard's top-left is the bed's).
        if m.placement == settings::Placement::Absolute
            && let Some(f) = plan.frame
        {
            let bed = egui::Rect::from_two_pos(to_screen(f.origin()), to_screen(kurbo::Point::new(f.x0 + m.bed_w / k, f.y0 + m.bed_h / k)));
            let st = Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(40, 190, 90, 160));
            let c = [bed.left_top(), bed.right_top(), bed.right_bottom(), bed.left_bottom(), bed.left_top()];
            for w in c.windows(2) {
                painter.extend(egui::Shape::dashed_line(&[w[0], w[1]], st, 6.0, 4.0));
            }
        }
        if let Some(p) = plan::start_point(plan, m) {
            let c = to_screen(p);
            let r = egui::Rect::from_center_size(c, egui::vec2(10.0, 10.0));
            painter.rect_filled(r, 1, green);
            painter.rect_stroke(r, 1, Stroke::new(1.5, egui::Color32::WHITE), egui::StrokeKind::Outside);
            let text = match m.placement {
                settings::Placement::Absolute => "Home (machine 0, 0)".to_string(),
                settings::Placement::CurrentPosition => "Start · at the head".to_string(),
                settings::Placement::UserOrigin if m.user_origin.is_some() => "Start · user origin".to_string(),
                settings::Placement::UserOrigin => "Start · user origin not set".to_string(),
            };
            label(c, &text, green);
        }
        // The head, where absolute coordinates put it on the artboard.
        if m.placement == settings::Placement::Absolute
            && let (Some(s), Some(f)) = (self.status, plan.frame)
        {
            let u = plan::to_machine(m, (s.pos.0 as f64 / 1000.0, s.pos.1 as f64 / 1000.0));
            let c = to_screen(kurbo::Point::new(f.x0 + u.0 / k, f.y0 + u.1 / k));
            let red = egui::Color32::from_rgb(230, 60, 50);
            painter.line_segment([c - egui::vec2(9.0, 0.0), c + egui::vec2(9.0, 0.0)], Stroke::new(1.5, red));
            painter.line_segment([c - egui::vec2(0.0, 9.0), c + egui::vec2(0.0, 9.0)], Stroke::new(1.5, red));
            painter.circle_stroke(c, 5.0, Stroke::new(1.5, red));
        }
    }
}

const CMDS: [PluginCommand; 9] = [
    PluginCommand { id: "panel", label: "Laser Panel", icon: icon::LIGHTNING, keys: &[], aliases: "laser, cnc, cutter, ruida, engrave" },
    PluginCommand { id: "connect", label: "Connect / Disconnect Laser", icon: icon::PLUGS_CONNECTED, keys: &[], aliases: "laser, usb, machine" },
    PluginCommand { id: "frame", label: "Frame Laser Job", icon: icon::BOUNDING_BOX, keys: &[], aliases: "laser, trace outline, preview on machine" },
    PluginCommand { id: "start", label: "Start Laser Job…", icon: icon::PLAY, keys: &[], aliases: "laser, cut, engrave, run" },
    PluginCommand { id: "pause", label: "Pause / Resume Laser", icon: icon::PAUSE, keys: &[], aliases: "laser" },
    PluginCommand { id: "stop", label: "Stop Laser", icon: icon::STOP, keys: &[], aliases: "laser, abort, emergency" },
    PluginCommand { id: "fit_artboard", label: "Artboard to Laser Bed Size", icon: icon::FRAME_CORNERS, keys: &[], aliases: "laser, bed, machine size, work area, resize artboard" },
    PluginCommand { id: "machine", label: "Laser Machine Settings…", icon: icon::GEAR, keys: &[], aliases: "laser, device settings, bed size, origin" },
    PluginCommand { id: "save_rd", label: "Save Laser Job as .rd…", icon: icon::FLOPPY_DISK, keys: &[], aliases: "laser, ruida, rdworks, export" },
];

impl Plugin for LaserPlugin {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        "Laser (Ruida)"
    }

    fn description(&self) -> &'static str {
        "Cut, score and engrave on a CO₂ laser with a Ruida RDC644x controller over USB. Colour layers, framing, toolpath preview."
    }

    fn icon(&self) -> &'static str {
        icon::LIGHTNING
    }

    fn commands(&self) -> Vec<PluginCommand> {
        CMDS.to_vec()
    }

    fn panels(&self) -> Vec<PanelSpec> {
        vec![PanelSpec { id: "laser", title: "Laser" }]
    }

    fn enabled_cmd(&self, cmd: &str, _app: &App) -> bool {
        let on = self.connected();
        match cmd {
            "frame" | "start" => on && !self.running() && self.busy != Busy::Framing,
            "pause" => on && self.running(),
            "stop" => on,
            _ => true,
        }
    }

    fn checked_cmd(&self, cmd: &str, _app: &App) -> Option<bool> {
        (cmd == "connect").then_some(self.session.is_some())
    }

    fn run(&mut self, cmd: &str, app: &mut App, ctx: &egui::Context) {
        match cmd {
            "panel" => app.show_plugin_panel("laser"),
            "connect" => self.toggle_connection(ctx),
            "frame" => self.frame(app),
            "start" => self.ask_start(app),
            "pause" => self.toggle_pause(),
            "stop" => self.stop(),
            "save_rd" => self.save_rd(app),
            "fit_artboard" => self.fit_artboard(app),
            "machine" => self.machine_open = true,
            _ => {}
        }
    }

    fn panel_ui(&mut self, _panel: &str, app: &mut App, ui: &mut egui::Ui) {
        self.panel_seen = app.now;
        panel::show(self, app, ui);
    }

    fn tick(&mut self, app: &mut App, _ctx: &egui::Context) {
        self.poll(app);
        if self.preview {
            self.plan(app);
        }
    }

    fn windows(&mut self, app: &mut App, ctx: &egui::Context) {
        panel::machine_dialog(self, app, ctx);
        panel::cut_dialog(self, app, ctx);
        self.confirm_ui(ctx);
        self.disconnect_ui(ctx);
    }

    fn canvas_overlay(&mut self, app: &App, painter: &egui::Painter) {
        let panel_open = app.now - self.panel_seen < 0.75;
        if !self.preview && !panel_open {
            return;
        }
        let Some((_, plan)) = &self.plan else { return };
        let plan = plan.clone();
        let view = &app.view;
        let to_screen = |p: kurbo::Point| {
            let q = view.affine() * p;
            egui::pos2(q.x as f32, q.y as f32)
        };
        if self.preview {
            // Fade the artwork so the toolpaths stand out.
            if let Some(f) = plan.frame {
                let r = egui::Rect::from_two_pos(to_screen(f.origin()), to_screen(kurbo::Point::new(f.x1, f.y1)));
                painter.rect_filled(r, 0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 215));
            }
            let travel = Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(230, 80, 60, 170));
            let mut at: Option<kurbo::Point> = None;
            for l in &plan.layers {
                // Light colours would vanish on the white artboard.
                let col = if l.rgb.iter().map(|v| *v as u32).sum::<u32>() > 600 { egui::Color32::from_rgb(120, 120, 120) } else { egui::Color32::from_rgb(l.rgb[0], l.rgb[1], l.rgb[2]) };
                let area = l.settings.mode != settings::Mode::Line;
                // Pictures can be hundreds of thousands of dots: thin out.
                let step = (l.paths.len() / 60_000).max(1);
                for (k, p) in l.paths.iter().enumerate() {
                    // Travel: between cut paths, and into each layer (a fill's
                    // hops between its lines would bury it).
                    if let Some(a) = at
                        && (!area || k == 0)
                    {
                        painter.extend(egui::Shape::dashed_line(&[to_screen(a), to_screen(p[0])], travel, 4.0, 3.0));
                    }
                    at = p.last().copied();
                    if k % step != 0 {
                        continue;
                    }
                    if area && p.len() == 2 {
                        painter.line_segment([to_screen(p[0]), to_screen(p[1])], Stroke::new(1.0, col.gamma_multiply(0.8)));
                    } else if area {
                        crate::render::outline(painter, &plan::polyline(p), view, Stroke::new(1.0, col.gamma_multiply(0.8)));
                    } else {
                        crate::render::outline(painter, &plan::polyline(p), view, Stroke::new(1.4, col));
                    }
                }
            }
        }
        self.paint_marks(&plan, app, painter);
    }

    fn load_settings(&mut self, data: &Value) {
        if let Ok(m) = serde_json::from_value(data.clone()) {
            self.machine = m;
        }
    }

    fn save_settings(&self) -> Value {
        serde_json::to_value(&self.machine).unwrap_or(Value::Null)
    }

    fn on_disable(&mut self, _app: &mut App) {
        if self.running() {
            self.stop();
        }
        self.disconnect();
        self.preview = false;
    }
}
