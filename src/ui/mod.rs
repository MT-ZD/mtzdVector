//! Application shell (Affinity-style docked layout):
//! menu bar · toolbar · context toolbar · tools column · canvas with rulers ·
//! studio dock · status bar, plus context menus and the quick-actions palette.

pub mod context_menu;
pub mod dialogs;
pub mod paint_dialogs;
pub mod welcome;
pub mod window;
pub mod export;
mod export_preview;
pub mod menubar;
pub mod palette;
pub mod rulers;
pub mod statusbar;
pub mod studio;
pub mod theme;
pub mod tools_panel;
pub mod toolbar;
pub mod widgets;

use egui::{Event, Key};

use crate::app::{App, Tool};
use crate::commands::{self, Cmd, Press, Scope};
use crate::document::ShapeKind;
use crate::platform::{ClipKey, PlatformEvent, clipboard};


pub struct PaletteState {
    pub query: String,
    pub selected: usize,
    pub just_opened: bool,
}

/// Canvas / layer context menu state.
pub struct CtxMenu {
    pub pos: egui::Pos2,
    pub doc: kurbo::Point,
    /// Shapes under the cursor, top-most first ("Select layer ▸").
    pub hits: Vec<u64>,
    /// Ignore outside clicks (set while the opening button is still held).
    pub opened_frame: bool,
    /// Right-click on a misspelled word while editing: its range and
    /// suggestions.
    pub spell: Option<(std::ops::Range<usize>, String, Vec<String>)>,
}

pub struct UiState {
    /// Active tab per studio group: [Design], [Layers|Components|Icons|History].
    pub tab: [usize; 2],
    pub collapsed: [bool; 2],
    /// Height of the upper (Design) studio group (points).
    pub group_h: [f32; 1],
    pub show_studio: bool,
    pub show_rulers: bool,
    pub show_guides: bool,
    pub show_grid: bool,
    /// Draw shapes as hairline outlines.
    pub outline_view: bool,
    /// Keep width/height proportional (fields and resize handles).
    pub lock_aspect: bool,
    /// Shape whose gradient is being edited (shows the on-canvas handles).
    pub gradient_edit: Option<u64>,
    /// Selected gradient stop (stop bar and on-canvas handles).
    pub gradient_stop: usize,
    /// Tool group whose flyout is docked beside the Tools column.
    pub tool_flyout: Option<tools_panel::Group>,
    /// Where the floating flyout was dragged to (None: beside its button).
    pub flyout_pos: Option<egui::Pos2>,
    /// Screen rects of the grouped tool buttons (flyout anchors).
    pub flyout_anchor: Vec<(tools_panel::Group, egui::Rect)>,
    pub offset: crate::path_ops::OffsetState,
    pub warp: crate::paint_ops::WarpState,
    pub blend: crate::paint_ops::BlendState,
    pub trace: crate::paint_ops::TraceState,
    pub prefs: crate::settings::PrefsState,
    /// Inline colour editor open in the Design panel ("fill", "stroke", …).
    pub paint_open: Option<String>,
    pub palette: Option<PaletteState>,
    pub ctx_menu: Option<CtxMenu>,
    pub artboard_open: bool,
    pub shortcuts_open: bool,
    pub about_open: bool,
    /// Start screen with templates and recent documents.
    pub welcome_open: bool,
    pub welcome: welcome::WelcomeState,
    pub export: crate::ui::export::ExportState,
    /// Transform reference point (0..=2, 0..=2).
    pub anchor: (u8, u8),
    pub keys: studio::keys::KeysState,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            tab: [0, 0],
            collapsed: [false; 2],
            group_h: [540.0],
            show_studio: true,
            show_rulers: true,
            show_guides: true,
            show_grid: false,
            outline_view: false,
            lock_aspect: false,
            gradient_edit: None,
            gradient_stop: 0,
            tool_flyout: None,
            flyout_pos: None,
            flyout_anchor: Vec::new(),
            offset: Default::default(),
            warp: Default::default(),
            blend: Default::default(),
            trace: Default::default(),
            prefs: Default::default(),
            paint_open: None,
            palette: None,
            ctx_menu: None,
            artboard_open: false,
            shortcuts_open: false,
            welcome_open: false,
            welcome: Default::default(),
            export: Default::default(),
            about_open: false,
            anchor: (0, 0),
            keys: Default::default(),
        }
    }
}

impl UiState {
    pub fn open_palette(&mut self) {
        if self.palette.is_some() {
            self.palette = None;
        } else {
            self.palette = Some(PaletteState { query: String::new(), selected: 0, just_opened: true });
        }
    }
}

impl eframe::App for App {
    /// Tablet pens (read through tablet-v2) drive the pointer like a mouse.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(rx) = &self.platform.pen_rx else { return };
        let events: Vec<crate::platform::PenEvent> = rx.try_iter().collect();
        if events.is_empty() {
            return;
        }
        for e in &events {
            if let crate::platform::PenEvent::Pressure(p) = e {
                self.tablet_pressure = Some(*p);
            }
            if matches!(e, crate::platform::PenEvent::Button { button: egui::PointerButton::Primary, pressed: false } | crate::platform::PenEvent::Leave) {
                self.tablet_pressure = None;
            }
        }
        #[cfg(target_os = "linux")]
        raw.events.extend(crate::platform::tablet_wayland::to_egui(&events, ctx.zoom_factor(), ctx.input(|i| i.modifiers), &mut self.tablet_pos));
        self.tablet_seen = true;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame_ui(ui);
        let title = format!("{}{} — mtzdVector", self.doc_name(), if self.dirty { " •" } else { "" });
        if self.last_title != title {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}

impl App {
    pub fn doc_name(&self) -> String {
        self.file_path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into())
    }

    /// One UI frame, independent of the windowing backend (also used headless).
    pub fn frame_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        crate::units::set(self.doc.unit, self.doc.dpi);
        self.fonts.poll();
        if self.fonts.downloads_in_flight > 0 || self.fonts.google_state == crate::fonts::LoadState::Loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        #[cfg(unix)]
        for files in crate::platform::single::take_requests() {
            // Another launch handed its files over: open them, raise the window.
            for f in files {
                self.open_path(f);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        let platform_keys = self.handle_platform_events(&ctx);
        self.handle_winit_drops(&ctx);
        self.handle_keys(&ctx, platform_keys);
        self.artboard_tick();
        self.components_tick();
        self.layout_tick();
        self.live_tick();
        self.text_flow_tick();
        self.spell_tick(&ctx);
        self.print_tick(&ctx);

        menubar::show(self, ui);
        toolbar::show(self, ui);
        statusbar::show(self, ui);
        tools_panel::show(self, ui);
        if self.ui.show_studio {
            studio::show(self, ui);
        }
        // Panel edits of a main reach its instances before the canvas paints.
        self.components_tick();
        self.layout_tick();
        self.live_tick();
        self.text_flow_tick();
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CANVAS)).show(ui, |ui| {
            let full = ui.available_rect_before_wrap();
            let cr = rulers::canvas_rect(full, self.ui.show_rulers);
            let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(cr));
            self.canvas(&mut cui);
            if self.ui.show_rulers {
                rulers::show(self, ui, full);
            }
        });
        tools_panel::flyout(self, &ctx);
        context_menu::show(self, &ctx);
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        welcome::show(self, &ctx);
        export::show(self, &ctx);
        window::modal_overlay(self, &ctx);
        window::frame(ui);
        self.session_tick(&ctx);
        self.settings_tick(&ctx);
        if let Some(id) = self.pending_select.take() {
            self.finish_text_edit();
            self.selection = vec![id];
        }
        if let Some(c) = self.pending_cmd.take() {
            if c.enabled(self) {
                c.run(self, &ctx);
            }
        }
        // Canvas edits: instances follow on the next frame (tick asks for it).
        self.components_tick();
        self.cache.prune(&self.doc);
    }

    /// Which scopes are blocked right now.
    fn scope_allowed(&self, ctx: &egui::Context, scope: Scope) -> bool {
        let field = ctx.egui_wants_keyboard_input();
        match scope {
            Scope::Global => true,
            Scope::NoTextField => !field,
            Scope::Canvas => !field && self.text_edit.is_none() && self.ui.palette.is_none(),
        }
    }

    pub fn run_cmd(&mut self, cmd: Cmd, ctx: &egui::Context) {
        cmd.run(self, ctx);
    }

    fn handle_keys(&mut self, ctx: &egui::Context, mut presses: Vec<Press>) {
        let rich_keys = self.platform.has_keyboard();
        let mut fallback_paste: Option<String> = None;
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    Event::Key { key, physical_key, pressed: true, modifiers, .. } => presses.push(Press { key: *key, physical: *physical_key, mods: *modifiers }),
                    // egui-winit turns Ctrl+C/X/V into these events (the keys never
                    // arrive). Use them unless our own keyboard listener is active.
                    Event::Copy if !rich_keys => presses.push(Press { key: Key::C, physical: Some(Key::C), mods: i.modifiers }),
                    Event::Cut if !rich_keys => presses.push(Press { key: Key::X, physical: Some(Key::X), mods: i.modifiers }),
                    Event::Paste(t) if !rich_keys => {
                        fallback_paste = Some(t.clone());
                        presses.push(Press { key: Key::V, physical: Some(Key::V), mods: i.modifiers });
                    }
                    _ => {}
                }
            }
        });
        for p in presses {
            if let Some(cmd) = commands::lookup(&p) {
                if !self.scope_allowed(ctx, cmd.meta().scope) {
                    continue;
                }
                // Fallback clipboard: paste the text egui handed us.
                if matches!(cmd, Cmd::Paste | Cmd::PasteHere) && !self.platform.clipboard.available() {
                    if let Some(t) = fallback_paste.take() {
                        let at = (cmd == Cmd::PasteHere).then(|| self.cursor_doc.unwrap_or_else(|| self.view_center_doc()));
                        self.paste_text_event(&t, at);
                        continue;
                    }
                }
                self.menu_anchor_doc = None;
                self.run_cmd(cmd, ctx);
                continue;
            }
            if !self.scope_allowed(ctx, Scope::Canvas) {
                continue;
            }
            self.special_key(p);
        }
    }

    /// Context-dependent keys: Escape, Enter and arrow nudges.
    fn special_key(&mut self, p: Press) {
        let m = p.mods;
        if m.command || m.alt || (p.key == Key::Escape && self.ui.ctx_menu.is_some()) {
            return;
        }
        match p.key {
            Key::Escape => {
                if self.pen.is_some() {
                    self.finish_pen();
                } else if self.tool != Tool::Select {
                    self.set_tool(Tool::Select);
                } else if let Some(parent) = self.selection.first().and_then(|id| self.doc.parent_of(*id)) {
                    // Inside a boolean: step out to the boolean itself.
                    self.selection = vec![parent];
                } else {
                    self.selection.clear();
                }
            }
            Key::Enter => {
                if self.pen.is_some() {
                    self.finish_pen();
                } else if let Some(children) = self.selection.first().filter(|_| self.selection.len() == 1).and_then(|id| self.doc.get(*id)).and_then(|s| s.kind.children()) {
                    // Enter a boolean: select its children.
                    self.selection = children.iter().map(|c| c.id).collect();
                } else if self.tool == Tool::Select && self.selection.len() == 1 {
                    let is_text = matches!(self.doc.get(self.selection[0]).map(|s| &s.kind), Some(ShapeKind::Text(_)));
                    if is_text {
                        self.begin_text_edit(self.selection[0]);
                    } else {
                        self.set_tool(Tool::Node);
                    }
                }
            }
            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown => {
                let amt = if m.shift { self.settings.nudge_big } else { self.settings.nudge };
                let d = match p.key {
                    Key::ArrowLeft => (-amt, 0.0),
                    Key::ArrowRight => (amt, 0.0),
                    Key::ArrowUp => (0.0, -amt),
                    _ => (0.0, amt),
                };
                if self.tool == Tool::Node && !self.node_sel.is_empty() {
                    self.checkpoint("nudge");
                    self.nudge_nodes(kurbo::Vec2::new(d.0, d.1));
                } else if !self.selection.is_empty() {
                    self.checkpoint("nudge");
                    let ids = self.selection.clone();
                    self.translate_ids(&ids, kurbo::Vec2::new(d.0, d.1));
                }
            }
            _ => {}
        }
    }

    /// Drain platform events (Wayland DnD + keyboard). Returns clipboard key
    /// presses for the dispatcher.
    fn handle_platform_events(&mut self, ctx: &egui::Context) -> Vec<Press> {
        let zf = ctx.zoom_factor().max(0.01);
        let mut keys = Vec::new();
        for e in self.platform.poll() {
            match e {
                PlatformEvent::DragHover(p) => self.drop_hover = Some(egui::pos2(p.x / zf, p.y / zf)),
                PlatformEvent::DragLeave => self.drop_hover = None,
                PlatformEvent::DropFiles(files, p) => {
                    self.drop_hover = None;
                    let at = self.view.to_doc(egui::pos2(p.x / zf, p.y / zf));
                    self.place_files(&files, at);
                }
                PlatformEvent::DropData { mime, bytes, pos } => {
                    self.drop_hover = None;
                    let at = self.view.to_doc(egui::pos2(pos.x / zf, pos.y / zf));
                    if let Some(c) = clipboard::decode(&mime, bytes) {
                        self.paste_content(c, Some(at));
                    }
                }
                PlatformEvent::Key { key, mods } => {
                    let k = match key {
                        ClipKey::Copy => Key::C,
                        ClipKey::Cut => Key::X,
                        ClipKey::Paste => Key::V,
                    };
                    keys.push(Press { key: k, physical: Some(k), mods });
                }
            }
        }
        keys
    }

    /// winit's own file drop events (X11; also harmless elsewhere).
    fn handle_winit_drops(&mut self, ctx: &egui::Context) {
        if self.platform.wayland_dnd {
            return;
        }
        let (hovering, files, pos) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect::<Vec<_>>(),
                i.pointer.hover_pos(),
            )
        });
        if hovering {
            self.drop_hover = Some(pos.unwrap_or(self.canvas_rect.center()));
        } else if self.drop_hover.is_some() && std::env::var("MTZD_DROP").is_err() {
            self.drop_hover = None;
        }
        if !files.is_empty() {
            self.drop_hover = None;
            let at = pos.map(|p| self.view.to_doc(p)).unwrap_or_else(|| self.view_center_doc());
            self.place_files(&files, at);
        }
    }
}
