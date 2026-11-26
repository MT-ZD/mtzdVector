//! The Studio's Laser tab, laid out like LightBurn's dock: the Cuts /
//! Layers table and the selected layer's settings on top, then the Laser
//! (device and job), Move (jog), Library (saved cut settings) and Console
//! (wire log) tabs. Machine settings are a dialog.

use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;

use super::plan::PlanLayer;
use super::settings::{Area, Corner, Dither, DocSettings, FillStyle, LayerSettings, Mode, Placement, Preset};
use super::{Busy, Conn, EMULATOR, LaserPlugin, Sub, doc_settings, set_doc_settings};
use crate::app::App;
use crate::ui::theme::{self, ACCENT, DANGER, SUCCESS, TEXT, TEXT_DIM, TEXT_FAINT};
use crate::ui::widgets::{self, Num, num_field, segmented_width};

fn port_label(p: &str) -> String {
    if p == EMULATOR {
        "Emulator (no machine)".into()
    } else if p.is_empty() {
        "Choose a port…".into()
    } else {
        p.into()
    }
}

fn dim(ui: &mut Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text.into()).size(theme::FONT_SM).color(TEXT_DIM)).wrap());
}

/// A button of the job grid; filled when `fill` is set.
fn job_button(ui: &mut Ui, glyph: &str, label: &str, enabled: bool, fill: Option<Color32>, width: f32) -> egui::Response {
    let text = RichText::new(format!("{glyph}  {label}")).size(theme::FONT_MD);
    let b = match fill {
        Some(f) => egui::Button::new(text.color(Color32::WHITE).strong()).fill(f).stroke(Stroke::NONE),
        None => egui::Button::new(text),
    };
    ui.add_enabled(enabled, b.min_size(vec2(width, 30.0)).corner_radius(theme::radius()))
}

fn spd_pwr(s: &LayerSettings) -> String {
    format!("{:.0} mm/s · {:.0}%", s.speed, s.max_power)
}

/// A small caption above a group of controls.
fn caption(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text.to_uppercase()).size(10.0).strong().color(TEXT_FAINT));
    ui.add_space(2.0);
}

/// A row with an icon and a label on the left, the control on the right.
fn row<R>(ui: &mut Ui, glyph: &str, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (r, resp) = ui.allocate_exact_size(vec2(92.0, 26.0), Sense::hover());
        widgets::describe(&resp, egui::accesskit::Role::Label, label, None);
        let p = ui.painter();
        p.text(r.left_center(), Align2::LEFT_CENTER, glyph, FontId::proportional(14.0), TEXT_DIM);
        p.text(r.left_center() + vec2(21.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(theme::FONT_MD), TEXT_DIM);
        add(ui)
    })
    .inner
}

fn mode_glyph(m: Mode) -> &'static str {
    match m {
        Mode::Line => icon::PATH,
        Mode::Fill => icon::PAINT_BUCKET,
        Mode::Image => icon::IMAGE,
    }
}

pub fn show(p: &mut LaserPlugin, app: &mut App, ui: &mut Ui) {
    let w = ui.available_width();
    p.refresh_ports(app.now);
    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
    let plan = p.plan(app);
    if !plan.layers.iter().any(|l| Some(&l.key) == p.selected.as_ref()) {
        p.selected = plan.layers.first().map(|l| l.key.clone());
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::STACK).size(15.0).color(ACCENT));
        ui.label(RichText::new("Cuts / Layers").size(theme::FONT_LG).strong().color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = widgets::tip(widgets::icon_button_sized(ui, icon::GEAR, true, false, vec2(26.0, 26.0), 16.0), "Machine settings (bed, home corner, power limit)", None);
            widgets::describe(&r, egui::accesskit::Role::Button, "Machine settings", None);
            if r.clicked() {
                p.machine_open = true;
            }
        });
    });
    let mut doc = doc_settings(app);
    layer_table(p, ui, &plan.layers, &mut doc, w);
    set_doc_settings(app, &doc);
    if !plan.layers.is_empty() {
        dim(ui, "Double-click a layer (or its pencil) to edit its cut settings.");
    }
    for msg in &plan.warnings {
        dim(ui, format!("{}  {msg}", icon::INFO));
    }

    ui.add_space(12.0);
    let labels = [format!("{} Laser", icon::LIGHTNING), format!("{} Move", icon::ARROWS_OUT_CARDINAL), format!("{} Library", icon::BOOKS), format!("{} Log", icon::TERMINAL)];
    let mut sub = p.sub;
    segmented_width(ui, &mut sub, &[(Sub::Laser, labels[0].as_str()), (Sub::Move, labels[1].as_str()), (Sub::Library, labels[2].as_str()), (Sub::Console, labels[3].as_str())], w);
    p.sub = sub;
    ui.add_space(4.0);
    match sub {
        Sub::Laser => laser_tab(p, app, ui, w),
        Sub::Move => move_tab(p, ui, w),
        Sub::Library => library_tab(p, app, ui, w),
        Sub::Console => console_tab(p, ui),
    }
    ui.add_space(12.0);
}

/// LightBurn's Cuts / Layers list: #, colour, mode, speed / power, edit,
/// output. Double-click opens the Cut Settings dialog.
fn layer_table(p: &mut LaserPlugin, ui: &mut Ui, layers: &[PlanLayer], doc: &mut DocSettings, w: f32) {
    if layers.is_empty() {
        egui::Frame::new().fill(theme::BG_INPUT).corner_radius(6).inner_margin(egui::Margin::symmetric(12, 14)).show(ui, |ui| {
            ui.set_width(w - 24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::STACK).size(22.0).color(TEXT_FAINT));
                dim(ui, "No shapes to cut yet. Strokes become Line (cut) layers and fills Fill (engrave) layers, one per colour.");
            });
        });
        return;
    }
    let row_h = 32.0;
    let cols = [28.0, 26.0, 58.0];
    let out_w = 30.0;
    let edit_w = 28.0;
    let font = FontId::proportional(theme::FONT_SM);
    let (hr, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    let painter = ui.painter();
    let mut x = hr.left() + 8.0;
    for (t, cw) in ["#", "", "Mode"].iter().zip(cols) {
        painter.text(egui::pos2(x, hr.center().y), Align2::LEFT_CENTER, *t, font.clone(), TEXT_FAINT);
        x += cw;
    }
    painter.text(egui::pos2(x, hr.center().y), Align2::LEFT_CENTER, "mm/s · Power", font.clone(), TEXT_FAINT);
    painter.text(egui::pos2(hr.right() - out_w / 2.0 - 4.0, hr.center().y), Align2::CENTER_CENTER, "Out", font.clone(), TEXT_FAINT);
    egui::Frame::new().fill(theme::BG_INPUT).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(6).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, l) in layers.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(w - 2.0, row_h), Sense::click());
            let selected = p.selected.as_ref() == Some(&l.key);
            let painter = ui.painter();
            if selected {
                painter.rect_filled(r.shrink(2.0), 4, theme::ACCENT_DIM);
            } else if resp.hovered() {
                painter.rect_filled(r.shrink(2.0), 4, theme::BG_HOVER);
            }
            if i > 0 {
                painter.hline(r.x_range().shrink(6.0), r.top(), Stroke::new(1.0, theme::BORDER.gamma_multiply(0.6)));
            }
            let s = &l.settings;
            let ink = if s.output { TEXT } else { TEXT_FAINT };
            let cy = r.center().y;
            let mut x = r.left() + 8.0;
            painter.text(egui::pos2(x, cy), Align2::LEFT_CENTER, format!("{i:02}"), FontId::monospace(10.5), if s.output { TEXT_DIM } else { TEXT_FAINT });
            x += cols[0];
            let sw = egui::Rect::from_center_size(egui::pos2(x + 8.0, cy), vec2(16.0, 16.0));
            painter.rect(sw, 3, Color32::from_rgb(l.rgb[0], l.rgb[1], l.rgb[2]), Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
            x += cols[1];
            painter.text(egui::pos2(x, cy), Align2::LEFT_CENTER, mode_glyph(s.mode), FontId::proportional(13.0), ink);
            let kind = match (s.mode, s.fill_style) {
                (Mode::Line, _) => "Line",
                (Mode::Image, _) => "Image",
                (Mode::Fill, FillStyle::Scan) => "Fill",
                (Mode::Fill, FillStyle::CrossHatch) => "Cross",
                (Mode::Fill, FillStyle::Offset) => "Offset",
            };
            painter.text(egui::pos2(x + 18.0, cy), Align2::LEFT_CENTER, kind, font.clone(), ink);
            x += cols[2];
            painter.text(egui::pos2(x, cy), Align2::LEFT_CENTER, format!("{:.0} · {:.0}%", s.speed, s.max_power), font.clone(), if s.output { TEXT_DIM } else { TEXT_FAINT });
            // Edit and output buttons.
            let er = egui::Rect::from_center_size(egui::pos2(r.right() - out_w - edit_w / 2.0 - 4.0, cy), vec2(24.0, 24.0));
            let e = widgets::tip(ui.interact(er, ui.id().with(("laser_edit", &l.key)), Sense::click()), "Cut settings", None);
            if e.hovered() || resp.hovered() || selected {
                ui.painter().text(er.center(), Align2::CENTER_CENTER, icon::PENCIL_SIMPLE, FontId::proportional(14.0), if e.hovered() { TEXT } else { TEXT_DIM });
            }
            widgets::describe(&e, egui::accesskit::Role::Button, &format!("Edit layer {i:02}"), None);
            let or = egui::Rect::from_center_size(egui::pos2(r.right() - out_w / 2.0 - 4.0, cy), vec2(24.0, 24.0));
            let o = ui.interact(or, ui.id().with(("laser_out", &l.key)), Sense::click());
            let glyph = if s.output { icon::CHECK_SQUARE } else { icon::SQUARE };
            ui.painter().text(or.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(16.0), if s.output { ACCENT } else { TEXT_FAINT });
            let o = widgets::tip(o, if s.output { "Output: this layer is sent to the laser" } else { "Output off: this layer is skipped" }, None);
            widgets::describe(&o, egui::accesskit::Role::CheckBox, &format!("Output {}", l.key), None);
            if o.clicked() {
                doc.layers.insert(l.key.clone(), LayerSettings { output: !s.output, ..s.clone() });
            } else if e.clicked() || resp.double_clicked() {
                p.selected = Some(l.key.clone());
                p.editing = Some(l.key.clone());
            } else if resp.clicked() {
                p.selected = Some(l.key.clone());
            }
            widgets::describe(&resp, egui::accesskit::Role::Row, &format!("Layer {i:02} {} {}", l.key, s.mode.label()), None);
        }
    });
}

/// LightBurn's Cut Settings editor: one layer's settings and its presets.
pub fn cut_dialog(p: &mut LaserPlugin, app: &mut App, ctx: &egui::Context) {
    use crate::ui::dialogs::{footer, modal, modal_title};
    let Some(key) = p.editing.clone() else { return };
    let plan = p.plan(app);
    let Some(l) = plan.layers.iter().find(|l| l.key == key) else {
        p.editing = None;
        return;
    };
    let mut doc = doc_settings(app);
    let mut s = l.settings.clone();
    let cap = p.machine.power_cap;
    let mut close = false;
    let r = modal("laser_cut_settings").show(ctx, |ui| {
        let w = 420.0;
        ui.set_width(w);
        ui.horizontal(|ui| {
            modal_title(ui, icon::SLIDERS_HORIZONTAL, "Cut Settings");
        });
        ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            ui.painter().rect(r, 3, Color32::from_rgb(l.rgb[0], l.rgb[1], l.rgb[2]), Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
            ui.label(RichText::new(&l.key).monospace().size(theme::FONT_MD).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if s.mode == Mode::Image {
                    ui.label(RichText::new(format!("{}  Image", icon::IMAGE)).color(TEXT_DIM));
                } else {
                    let line = format!("{}  Line", icon::PATH);
                    let fill = format!("{}  Fill", icon::PAINT_BUCKET);
                    widgets::segmented_sized(ui, &mut s.mode, &[(Mode::Line, line.as_str()), (Mode::Fill, fill.as_str())], 80.0);
                }
            });
        });
        let fw = (w - 10.0) / 2.0;
        caption(ui, "Speed and power");
        ui.horizontal(|ui| {
            num_field(ui, "Speed", &mut s.speed, Num::default().range(0.5, 1000.0).suffix(" mm/s").decimals(1), fw);
            let mut passes = s.passes as f64;
            num_field(ui, "Passes", &mut passes, Num::default().range(1.0, 50.0).decimals(0), fw);
            s.passes = passes.round() as u32;
        });
        ui.horizontal(|ui| {
            num_field(ui, "Max power", &mut s.max_power, Num::default().range(0.0, 100.0).suffix(" %").decimals(1), fw);
            num_field(ui, "Min power", &mut s.min_power, Num::default().range(0.0, 100.0).suffix(" %").decimals(1), fw);
        });
        if s.max_power > cap || s.min_power > cap {
            dim(ui, format!("{}  Capped at {cap:.0} % by Machine settings", icon::INFO));
        }
        if s.mode == Mode::Fill {
            caption(ui, "Fill");
            let names = [format!("{}  Scan", icon::ROWS), format!("{}  Cross-hatch", icon::HASH), format!("{}  Offset", icon::TARGET)];
            segmented_width(ui, &mut s.fill_style, &[(FillStyle::Scan, names[0].as_str()), (FillStyle::CrossHatch, names[1].as_str()), (FillStyle::Offset, names[2].as_str())], w);
            dim(
                ui,
                match s.fill_style {
                    FillStyle::Scan => "Parallel lines at the angle. The usual engrave.",
                    FillStyle::CrossHatch => "Lines at the angle, then across them: a more even burn (wood, anodised metal), twice the time.",
                    FillStyle::Offset => "Rings that follow the outline inwards: smooth edges on curves, good for thin shapes and outlines.",
                },
            );
            ui.horizontal(|ui| {
                num_field(ui, "Interval", &mut s.interval, Num::default().range(0.02, 5.0).suffix(" mm").decimals(2).speed(0.01), fw);
                if s.fill_style != FillStyle::Offset {
                    num_field(ui, "Angle", &mut s.angle, Num::default().range(-180.0, 180.0).suffix("°").decimals(0), fw);
                }
            });
            dim(ui, format!("{:.0} lines per inch", 25.4 / s.interval.max(0.01)));
            if s.fill_style != FillStyle::Offset {
                ui.horizontal(|ui| {
                    num_field(ui, "Angle step", &mut s.angle_step, Num::default().range(-180.0, 180.0).suffix("° / pass").decimals(0), fw);
                    dim(ui, if s.passes > 1 { "Turns the lines each pass" } else { "Needs 2 or more passes" });
                });
                ui.checkbox(&mut s.bidirectional, "Engrave both ways (faster)");
            }
            ui.checkbox(&mut s.one_by_one, "Fill shapes one by one (less travel between far-apart shapes)");
        }
        if s.mode == Mode::Image {
            caption(ui, "Image");
            let names: Vec<String> = [Dither::FloydSteinberg, Dither::Jarvis, Dither::Ordered, Dither::Threshold].iter().map(|d| d.label().to_string()).collect();
            segmented_width(ui, &mut s.dither, &[(Dither::FloydSteinberg, names[0].as_str()), (Dither::Jarvis, names[1].as_str()), (Dither::Ordered, names[2].as_str()), (Dither::Threshold, names[3].as_str())], w);
            dim(
                ui,
                match s.dither {
                    Dither::FloydSteinberg => "Photos: greys become scattered dots.",
                    Dither::Jarvis => "Photos, smoother: the error spreads wider, finer grain.",
                    Dither::Ordered => "A regular dot pattern: even tones, a screen-print look.",
                    Dither::Threshold => "Logos and line art: darker than the threshold burns, the rest doesn't.",
                },
            );
            ui.horizontal(|ui| {
                num_field(ui, "Interval", &mut s.interval, Num::default().range(0.05, 2.0).suffix(" mm").decimals(2).speed(0.01), fw);
                if s.dither == Dither::Threshold {
                    num_field(ui, "Threshold", &mut s.threshold, Num::default().range(1.0, 99.0).suffix(" %").decimals(0), fw);
                }
            });
            dim(ui, format!("{:.0} DPI (lines and dots per inch)", 25.4 / s.interval.max(0.01)));
            ui.horizontal(|ui| {
                num_field(ui, "Brightness", &mut s.brightness, Num::default().range(-100.0, 100.0).decimals(0), fw);
                num_field(ui, "Contrast", &mut s.contrast, Num::default().range(-100.0, 100.0).decimals(0), fw);
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.invert, "Negative image");
                ui.add_space(16.0);
                ui.checkbox(&mut s.bidirectional, "Engrave both ways");
            });
            dim(ui, format!("{}  Engraved line by line as short cuts, which works on any Ruida. RDWorks' scan mode is faster; it isn't supported yet.", icon::INFO));
        }
        caption(ui, "Options");
        ui.horizontal(|ui| {
            ui.checkbox(&mut s.air, "Air assist");
            ui.add_space(16.0);
            ui.checkbox(&mut s.output, "Output (send to the laser)");
        });
        caption(ui, "Presets");
        ui.horizontal(|ui| {
            let mut apply = None;
            widgets::dropdown(ui, "laser_apply_preset", format!("{}  Apply a preset…", icon::BOOKS), fw, |ui| {
                if p.machine.presets.is_empty() {
                    ui.label(RichText::new("No presets yet").color(TEXT_FAINT));
                }
                for (i, pr) in p.machine.presets.iter().enumerate() {
                    if ui.selectable_label(false, format!("{} — {} {}", pr.name, pr.settings.mode.label(), spd_pwr(&pr.settings))).clicked() {
                        apply = Some(i);
                    }
                }
            });
            if let Some(i) = apply {
                s = LayerSettings { output: s.output, ..p.machine.presets[i].settings.clone() };
            }
            if p.preset_name.is_none() && widgets::flat_button(ui, Some(icon::FLOPPY_DISK), "Save as preset…", true).clicked() {
                p.preset_name = Some(String::new());
            }
        });
        if let Some(name) = &mut p.preset_name {
            let mut done = None;
            ui.horizontal(|ui| {
                let r = widgets::text_field(ui, name, "Name, e.g. 4 mm plywood – cut", w - 140.0);
                // Focus once, when the field opens.
                let opened = ui.id().with("laser_preset_focus");
                if !ui.data(|d| d.get_temp::<bool>(opened).unwrap_or(false)) {
                    r.request_focus();
                    ui.data_mut(|d| d.insert_temp(opened, true));
                }
                let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Save").clicked() || enter) && !name.trim().is_empty() {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
            if done.is_some() {
                let opened = ui.id().with("laser_preset_focus");
                ui.data_mut(|d| d.remove::<bool>(opened));
            }
            match done {
                Some(true) => {
                    let name = name.trim().to_string();
                    save_preset(p, name, &s);
                    p.preset_name = None;
                }
                Some(false) => p.preset_name = None,
                None => {}
            }
        }
        if footer(ui, &[], &[], "Done").is_some() {
            close = true;
        }
    });
    if s != l.settings {
        // A mode switch starts from that mode's defaults (cutting and
        // engraving speeds and powers differ a lot).
        if s.mode != l.settings.mode {
            let d = LayerSettings::defaults_for(s.mode);
            s = LayerSettings { output: s.output, ..d };
        }
        doc.layers.insert(key, s);
        set_doc_settings(app, &doc);
    }
    if close || (r.should_close() && p.preset_name.is_none()) {
        p.editing = None;
        p.preset_name = None;
    }
}

/// Add a preset (or replace the one with this name).
pub fn save_preset(p: &mut LaserPlugin, name: String, s: &LayerSettings) {
    let settings = LayerSettings { output: true, ..s.clone() };
    match p.machine.presets.iter_mut().find(|q| q.name == name) {
        Some(q) => q.settings = settings,
        None => p.machine.presets.push(Preset { name, settings }),
    }
}

fn laser_tab(p: &mut LaserPlugin, app: &mut App, ui: &mut Ui, w: f32) {
    caption(ui, "Device");
    let locked = p.session_open();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!locked, |ui| {
            let mut port = p.machine.port.clone();
            widgets::dropdown(ui, "laser_port", format!("{}  {}", icon::USB, port_label(&port)), w - 112.0, |ui| {
                for q in &p.ports {
                    ui.selectable_value(&mut port, q.clone(), q.as_str());
                }
                if p.ports.is_empty() {
                    ui.label(RichText::new("No serial ports found").color(TEXT_FAINT));
                }
                ui.separator();
                ui.selectable_value(&mut port, EMULATOR.to_string(), port_label(EMULATOR));
            });
            p.machine.port = port;
        });
        let r = if locked { ui.add(egui::Button::new("Disconnect").min_size(vec2(98.0, 26.0))) } else { ui.add_sized(vec2(98.0, 26.0), egui::Button::new(RichText::new("Connect").color(Color32::WHITE)).fill(ACCENT).stroke(Stroke::NONE)) };
        if r.clicked() {
            p.toggle_connection(ui.ctx());
        }
    });
    let (dot, text) = match &p.conn {
        Conn::Off => (TEXT_FAINT, "Disconnected".to_string()),
        Conn::Connecting(port) => (ACCENT, format!("Looking for the controller on {}…", port_label(port))),
        Conn::On { link, .. } => {
            let state = match p.busy {
                Busy::Idle => "Ready".to_string(),
                Busy::Uploading { sent, total } => format!("Sending job… {}%", sent * 100 / total.max(1)),
                Busy::Running if p.paused => "Paused".into(),
                Busy::Running => "Busy".into(),
                Busy::Framing => "Framing".into(),
                Busy::Moving => "Moving".into(),
            };
            (SUCCESS, format!("{state} · {link}"))
        }
    };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, dot);
        ui.add(egui::Label::new(RichText::new(text).size(theme::FONT_SM).color(TEXT)).wrap());
    });
    if let Busy::Uploading { sent, total } = p.busy {
        ui.add(egui::ProgressBar::new(sent as f32 / total.max(1) as f32).desired_width(w).desired_height(6.0));
    }
    if let Some(e) = p.error.clone() {
        ui.add(egui::Label::new(RichText::new(format!("{}  {e}", icon::WARNING_CIRCLE)).size(theme::FONT_SM).color(DANGER)).wrap());
        if ui.small_button("Dismiss").clicked() {
            p.error = None;
        }
    }

    // The job buttons (LightBurn: Home, Frame, Start / Pause, Stop).
    caption(ui, "Job");
    let job = p.job(app);
    let ok = job.is_ok();
    let on = p.connected();
    let running = p.running();
    let idle = on && !running && p.busy == Busy::Idle;
    let third = (w - 16.0) / 3.0;
    let half = (w - 8.0) / 2.0;
    ui.horizontal(|ui| {
        if widgets::tip(job_button(ui, icon::HOUSE, "Home", idle, None, third), "The machine finds its origin", None).clicked() {
            p.home();
        }
        if widgets::tip(job_button(ui, icon::BOUNDING_BOX, "Frame", idle && ok, None, third), "Trace the job's outline with the laser off", None).clicked() {
            p.frame(app);
        }
        if job_button(ui, icon::PLAY, "Start", idle && ok, Some(ACCENT), third).clicked() {
            p.ask_start(app);
        }
    });
    ui.horizontal(|ui| {
        let (g, l) = if p.paused { (icon::PLAY, "Resume") } else { (icon::PAUSE, "Pause") };
        if job_button(ui, g, l, on && running, None, half).clicked() {
            p.toggle_pause();
        }
        if widgets::tip(job_button(ui, icon::STOP, "Stop", on, Some(DANGER), half), "Stop the machine now", None).clicked() {
            p.stop();
        }
    });
    match &job {
        Ok(j) => {
            let ((x0, y0), (x1, y1)) = j.bounds().unwrap_or(((0, 0), (0, 0)));
            let e = j.estimate(p.status.map(|s| s.pos).unwrap_or((0, 0)), p.machine.travel_speed);
            let secs = e.seconds.round() as u64;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(icon::RULER).color(TEXT_DIM));
                dim(ui, format!("{:.1} × {:.1} mm", (x1 - x0) as f64 / 1000.0, (y1 - y0) as f64 / 1000.0));
                ui.add_space(12.0);
                ui.label(RichText::new(icon::TIMER).color(TEXT_DIM));
                dim(ui, format!("about {}:{:02}", secs / 60, secs % 60));
            });
        }
        Err(e) => dim(ui, format!("{}  {e}", icon::INFO)),
    }

    caption(ui, "Placement");
    let cw = w - 98.0;
    row(ui, icon::CROSSHAIR, "Start from", |ui| {
        let mut place = p.machine.placement;
        widgets::dropdown(ui, "laser_start_from", place.label(), cw - 8.0, |ui| {
            for pl in [Placement::Absolute, Placement::CurrentPosition, Placement::UserOrigin] {
                ui.selectable_value(&mut place, pl, pl.label());
            }
        });
        p.machine.placement = place;
    });
    let anchor_name = |a: (u8, u8)| {
        const NAMES: [[&str; 3]; 3] = [["top-left corner", "top-middle", "top-right corner"], ["left-middle", "centre", "right-middle"], ["bottom-left corner", "bottom-middle", "bottom-right corner"]];
        NAMES[a.1 as usize % 3][a.0 as usize % 3]
    };
    match p.machine.placement {
        Placement::Absolute => {}
        _ => {
            row(ui, icon::DOTS_NINE, "Job origin", |ui| {
                widgets::anchor_selector(ui, &mut p.machine.anchor);
            });
        }
    }
    let corner = match p.machine.origin {
        Corner::TopLeft => "top-left",
        Corner::TopRight => "top-right",
        Corner::BottomLeft => "bottom-left",
        Corner::BottomRight => "bottom-right",
    };
    let where_ = match p.machine.placement {
        Placement::Absolute => format!("The artboard is the bed: its {corner} corner is the machine's home."),
        Placement::CurrentPosition => format!("The job's {} goes where the head is now (green mark on the canvas).", anchor_name(p.machine.anchor)),
        Placement::UserOrigin => match p.machine.user_origin {
            Some((x, y)) => format!("The job's {} goes to the user origin at X {x:.1}, Y {y:.1} mm (green mark).", anchor_name(p.machine.anchor)),
            None => "No user origin yet: move the head there and use Move › Set origin here.".into(),
        },
    };
    dim(ui, format!("{}  {where_}", icon::MAP_PIN));
    row(ui, icon::SELECTION, "Cut", |ui| {
        let mut area = p.machine.area;
        segmented_width(ui, &mut area, &[(Area::Artboard, "Artboard"), (Area::Selection, "Selection")], cw);
        p.machine.area = area;
    });
    if p.machine.area == Area::Selection && app.selection.is_empty() {
        dim(ui, "Nothing selected: the whole artboard is cut");
    }
    row(ui, icon::EYE, "Preview", |ui| {
        ui.checkbox(&mut p.preview, "Toolpaths on canvas");
    });

    caption(ui, "More");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if widgets::tip(widgets::flat_button(ui, Some(icon::FRAME_CORNERS), "Artboard to bed", true), &format!("Make the artboard {:.0} × {:.0} mm, the size of the bed", p.machine.bed_w, p.machine.bed_h), None).clicked() {
            p.fit_artboard(app);
        }
        if widgets::tip(widgets::flat_button(ui, Some(icon::FLOPPY_DISK), "Save .rd…", ok), "Save the job for RDWorks or a USB stick", None).clicked() {
            p.save_rd(app);
        }
    });
}

fn move_tab(p: &mut LaserPlugin, ui: &mut Ui, w: f32) {
    if !p.connected() {
        dim(ui, "Connect to the laser (Laser tab) to move the head.");
        return;
    }
    let pos = p.status.map(|s| super::plan::to_machine(&p.machine, (s.pos.0 as f64 / 1000.0, s.pos.1 as f64 / 1000.0)));
    dim(ui, pos.map(|(x, y)| format!("Head at X {x:.1}  Y {y:.1} mm")).unwrap_or("Position unknown".into()));
    widgets::prop_row(ui, "Distance", |ui| {
        let mut step = p.machine.jog;
        segmented_width(ui, &mut step, &[(0.1, "0.1"), (1.0, "1"), (10.0, "10"), (50.0, "50 mm")], w - 74.0);
        p.machine.jog = step;
    });
    let free = p.busy == Busy::Idle;
    let b = vec2(44.0, 34.0);
    let pad = ((w - b.x * 3.0 - 12.0) / 2.0).max(0.0);
    let press = |ui: &mut Ui, g: &str, tip: &str| widgets::tip(ui.add_enabled(free, egui::Button::new(RichText::new(g).size(17.0)).min_size(b)), tip, None).clicked();
    let s = p.machine.jog;
    let mut go: Option<(f64, f64)> = None;
    let mut home = false;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(pad + b.x + 6.0);
        if press(ui, icon::ARROW_UP, "Move up") {
            go = Some((0.0, -s));
        }
    });
    ui.horizontal(|ui| {
        ui.add_space(pad);
        if press(ui, icon::ARROW_LEFT, "Move left") {
            go = Some((-s, 0.0));
        }
        if press(ui, icon::HOUSE, "Home (the machine finds its origin)") {
            home = true;
        }
        if press(ui, icon::ARROW_RIGHT, "Move right") {
            go = Some((s, 0.0));
        }
    });
    ui.horizontal(|ui| {
        ui.add_space(pad + b.x + 6.0);
        if press(ui, icon::ARROW_DOWN, "Move down") {
            go = Some((0.0, s));
        }
    });
    if let Some((dx, dy)) = go {
        p.jog(dx, dy);
    }
    if home {
        p.home();
    }
    // User origin (LightBurn's "Set Origin" / "Go to Origin").
    caption(ui, "User origin");
    dim(ui, match p.machine.user_origin {
        Some((x, y)) => format!("At X {x:.1}  Y {y:.1} mm. Jobs set to “Start from: User origin” are placed there."),
        None => "Not set. Move the head to where jobs should start, then set it.".into(),
    });
    ui.horizontal(|ui| {
        if widgets::tip(widgets::flat_button(ui, Some(icon::MAP_PIN), "Set origin here", free && pos.is_some()), "Use the head's position as the user origin", None).clicked()
            && let Some(at) = pos
        {
            p.machine.user_origin = Some(at);
            p.machine.placement = super::settings::Placement::UserOrigin;
        }
        if widgets::tip(widgets::flat_button(ui, Some(icon::NAVIGATION_ARROW), "Go to origin", free && p.machine.user_origin.is_some()), "Move the head to the user origin (laser off)", None).clicked() {
            p.go_to_origin();
        }
    });
}

fn library_tab(p: &mut LaserPlugin, app: &mut App, ui: &mut Ui, w: f32) {
    dim(ui, "Saved cut settings for your materials. Assign one to the selected layer, or save a layer's settings with “Save as preset”.");
    if p.machine.presets.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("No presets yet").color(TEXT_FAINT));
        return;
    }
    let selected = p.selected.clone();
    let mut assign = None;
    let mut delete = None;
    for (i, pr) in p.machine.presets.iter().enumerate() {
        egui::Frame::new().fill(theme::BG_INPUT).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(4).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
            ui.set_width(w - 18.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(&pr.name).size(theme::FONT_MD).color(TEXT));
                    let s = &pr.settings;
                    let extra = if s.mode == Mode::Fill { format!(" · {} {:.2} mm", s.fill_style.label(), s.interval) } else { String::new() };
                    ui.label(RichText::new(format!("{} · {} · {}×{extra}", s.mode.label(), spd_pwr(s), s.passes)).size(theme::FONT_SM).color(TEXT_DIM));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::tip(widgets::mini_icon(ui, icon::TRASH, TEXT_DIM), "Delete preset", None).clicked() {
                        delete = Some(i);
                    }
                    if ui.add_enabled(selected.is_some(), egui::Button::new("Assign")).on_hover_text("Use for the selected layer").clicked() {
                        assign = Some(i);
                    }
                });
            });
        });
    }
    if let (Some(i), Some(key)) = (assign, selected) {
        let mut doc = doc_settings(app);
        let output = doc.layers.get(&key).map(|s| s.output).unwrap_or(true);
        doc.layers.insert(key, LayerSettings { output, ..p.machine.presets[i].settings.clone() });
        set_doc_settings(app, &doc);
    }
    if let Some(i) = delete {
        p.machine.presets.remove(i);
    }
}

fn console_tab(p: &mut LaserPlugin, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if ui.small_button("Copy").clicked() {
            let text: Vec<String> = p.log.iter().map(|(tx, t)| format!("{} {t}", if *tx { "→" } else { "←" })).collect();
            ui.ctx().copy_text(text.join("\n"));
        }
        if ui.small_button("Clear").clicked() {
            p.log.clear();
        }
    });
    egui::Frame::new().fill(theme::BG_INPUT).corner_radius(4).inner_margin(6).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("laser_log_scroll").max_height(260.0).auto_shrink([false, true]).stick_to_bottom(true).show(ui, |ui| {
            for (tx, t) in &p.log {
                let (arrow, col) = if *tx { ("→", ACCENT) } else { ("←", SUCCESS) };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(arrow).monospace().size(10.5).color(col));
                    ui.label(RichText::new(t).monospace().size(10.5).color(TEXT_DIM));
                });
            }
            if p.log.is_empty() {
                ui.label(RichText::new("Nothing sent or received yet").size(theme::FONT_SM).color(TEXT_FAINT));
            }
        });
    });
}

/// Laser › Machine Settings: bed, home corner, power cap and link details.
pub fn machine_dialog(p: &mut LaserPlugin, app: &mut App, ctx: &egui::Context) {
    use crate::ui::dialogs::{footer, modal, modal_text, modal_title};
    if !p.machine_open {
        return;
    }
    let mut close = false;
    let mut fit = false;
    let r = modal("laser_machine").show(ctx, |ui| {
        let w = 380.0;
        ui.set_width(w);
        modal_title(ui, icon::GEAR, "Laser Machine Settings");
        modal_text(ui, "How your machine is built. These are kept with the app, not the document.");
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        let fw = (w - 8.0) / 2.0;
        let m = &mut p.machine;
        ui.horizontal(|ui| {
            num_field(ui, "Bed W", &mut m.bed_w, Num::default().range(50.0, 3000.0).suffix(" mm").decimals(0), fw);
            num_field(ui, "Bed H", &mut m.bed_h, Num::default().range(50.0, 3000.0).suffix(" mm").decimals(0), fw);
        });
        dim(ui, "Home (origin) corner: where the head goes when it homes");
        let mut c = m.origin;
        segmented_width(ui, &mut c, &[(Corner::TopLeft, "Top left"), (Corner::TopRight, "Top right"), (Corner::BottomLeft, "Bottom left"), (Corner::BottomRight, "Bottom right")], w);
        m.origin = c;
        ui.horizontal(|ui| {
            num_field(ui, "Max power", &mut m.power_cap, Num::default().range(1.0, 100.0).suffix(" %").decimals(0), fw);
            num_field(ui, "Rapid", &mut m.travel_speed, Num::default().range(10.0, 2000.0).suffix(" mm/s").decimals(0), fw);
        });
        dim(ui, "No layer fires above Max power. Rapid is used for time estimates.");
        ui.checkbox(&mut m.frame_light, "Red pointer on while framing");
        let mut magic = m.magic as f64;
        ui.horizontal(|ui| {
            num_field(ui, "Magic", &mut magic, Num::default().range(0.0, 255.0).decimals(0), fw);
            dim(ui, "136 (0x88) for RDC644x");
        });
        m.magic = magic.round() as u8;
        ui.add_space(4.0);
        if widgets::flat_button(ui, Some(icon::FRAME_CORNERS), "Make the artboard the bed size", true).clicked() {
            fit = true;
        }
        if footer(ui, &[], &[], "Done").is_some() {
            close = true;
        }
    });
    if fit {
        p.fit_artboard(app);
    }
    if close || r.should_close() {
        p.machine_open = false;
    }
}
