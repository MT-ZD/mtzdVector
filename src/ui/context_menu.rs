//! Right-click menu for the canvas and layer rows, built from commands.

use egui::{Id, RichText, Ui};
use egui_phosphor::regular as icon;

use super::menubar::{self, ARRANGE, cmd_item, cmd_items, submenu};
use super::studio::kind_icon;
use super::theme::{self, TEXT_DIM};
use super::CtxMenu;
use crate::app::{App, Tool};
use crate::commands::Cmd;

impl App {
    /// Open the context menu at a screen position. Right-clicking an
    /// unselected shape selects it first; empty canvas clears the selection.
    pub fn open_context_menu(&mut self, pos: egui::Pos2, from_layers: bool) {
        let doc = self.view.to_doc(pos);
        // A misspelled word under the cursor while editing: offer corrections.
        if let Some(r) = self.misspelled_at(doc) {
            let id = self.text_edit.as_ref().map(|t| t.id).unwrap_or(0);
            let head = self.thread_head(id);
            let word = match self.doc.get(head).map(|s| &s.kind) {
                Some(crate::document::ShapeKind::Text(t)) => t.content.get(r.clone()).unwrap_or("").to_string(),
                _ => String::new(),
            };
            let lang = self.text_lang(id);
            let sugg = self.spell.suggest(&word, &lang);
            self.ui.ctx_menu = Some(CtxMenu { pos, doc, hits: Vec::new(), opened_frame: true, spell: Some((r, word, sugg)) });
            return;
        }
        let hits = if from_layers { Vec::new() } else { self.hit_shapes(doc, true) };
        if !from_layers && self.tool != Tool::Node {
            match hits.first() {
                Some(top) if !self.selection.contains(top) => {
                    self.selection = vec![*top];
                    self.node_sel.clear();
                }
                Some(_) => {}
                None => {
                    self.selection.clear();
                    self.node_sel.clear();
                }
            }
            if self.tool.is_text() {
                self.finish_text_edit();
                self.tool = Tool::Select;
            }
        }
        self.menu_anchor_doc = Some(doc);
        self.ui.ctx_menu = Some(CtxMenu { pos, doc, hits, opened_frame: true, spell: None });
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(menu) = &mut app.ui.ctx_menu else { return };
    let pos = menu.pos;
    // Ignore clicks until the button that opened the menu is released, so the
    // release doesn't count as a click outside.
    let first = menu.opened_frame;
    menu.opened_frame = menu.opened_frame && ctx.input(|i| i.pointer.any_down());
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut open = !escape;
    let layer = egui::LayerId::new(egui::Order::Foreground, Id::new("ctx_menu_layer"));
    egui::Popup::new(Id::new("ctx_menu"), ctx.clone(), egui::PopupAnchor::Position(pos), layer)
        .kind(egui::PopupKind::Menu)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .style(egui::containers::menu::menu_style)
        .open_bool(&mut open)
        .close_behavior(if first { egui::PopupCloseBehavior::IgnoreClicks } else { egui::PopupCloseBehavior::CloseOnClick })
        .show(|ui| {
            ui.set_min_width(230.0);
            contents(app, ui);
        });
    if !open {
        app.ui.ctx_menu = None;
    }
}

fn contents(app: &mut App, ui: &mut Ui) {
    if let Some((range, word, sugg)) = app.ui.ctx_menu.as_ref().and_then(|m| m.spell.clone()) {
        spelling(app, ui, range, &word, &sugg);
        return;
    }
    let has = !app.selection.is_empty();
    if app.tool == Tool::Node && (!app.node_sel.is_empty() || has) {
        header(ui, "Nodes");
        cmd_items(ui, app, &menubar::PATH);
        ui.separator();
    }
    if !has {
        cmd_items(ui, app, &[Cmd::Paste, Cmd::PasteHere]);
        ui.separator();
        cmd_item(ui, app, Cmd::SelectAll);
        ui.separator();
        cmd_items(ui, app, &[Cmd::ZoomFit, Cmd::Zoom100]);
        ui.separator();
        cmd_item(ui, app, Cmd::ArtboardSettings);
        return;
    }
    cmd_items(ui, app, &[Cmd::Cut, Cmd::Copy, Cmd::CopySvg, Cmd::Paste, Cmd::PasteHere]);
    ui.separator();
    cmd_items(ui, app, &[Cmd::CopyStyle, Cmd::PasteStyle]);
    ui.separator();
    cmd_items(ui, app, &[Cmd::Duplicate, Cmd::Delete]);
    ui.separator();
    cmd_items(ui, app, &[Cmd::Group, Cmd::Ungroup]);
    if app.has_instance_selected() {
        cmd_items(ui, app, &[Cmd::GoToMain, Cmd::ResetInstance, Cmd::DetachInstance]);
    } else {
        cmd_item(ui, app, Cmd::CreateComponent);
    }
    ui.separator();
    submenu(ui, icon::STACK, "Arrange", |ui| cmd_items(ui, app, &ARRANGE));
    submenu(ui, icon::ALIGN_LEFT, "Align", |ui| menubar::align_menu(ui, app));
    submenu(ui, icon::UNITE, "Boolean", |ui| menubar::boolean_menu(ui, app));
    ui.separator();
    cmd_items(ui, app, &[Cmd::FlipH, Cmd::FlipV]);
    ui.separator();
    let bitmap = app.selection.len() == 1 && app.selected_shapes().iter().any(|s| matches!(s.fill, Some(crate::document::Paint::Image(_))));
    if bitmap {
        cmd_item(ui, app, Cmd::TraceImage);
    }
    cmd_items(ui, app, &[Cmd::ConvertToCurves, Cmd::ExpandStroke, Cmd::ExpandOutlines]);
    let extra: Vec<Cmd> = [Cmd::ExpandLive, Cmd::ReleaseLive, Cmd::Flatten, Cmd::MakeClip, Cmd::ReleaseClip, Cmd::ReleaseMask, Cmd::TextOnPath, Cmd::ReleaseTextPath, Cmd::LinkTextFrames, Cmd::UnlinkTextFrames].into_iter().filter(|c| c.enabled(app)).collect();
    if !extra.is_empty() {
        cmd_items(ui, app, &extra);
    }
    let path_ops: Vec<Cmd> = [Cmd::OffsetPath, Cmd::SimplifyPath, Cmd::ReversePath].into_iter().filter(|c| c.enabled(app)).collect();
    if !path_ops.is_empty() {
        submenu(ui, icon::PATH, "Path", |ui| {
            ui.set_min_width(180.0);
            cmd_items(ui, app, &path_ops);
        });
    }
    ui.separator();
    let hidden = app.selected_shapes().iter().all(|s| !s.visible);
    menubar::cmd_item_labeled(ui, app, Cmd::ToggleHidden, Some(if hidden { "Show" } else { "Hide" }));
    menubar::cmd_item_labeled(ui, app, Cmd::ToggleLocked, Some("Lock"));
    cmd_item(ui, app, Cmd::Rename);
    let hits = app.ui.ctx_menu.as_ref().map(|m| m.hits.clone()).unwrap_or_default();
    if !hits.is_empty() {
        ui.separator();
        submenu(ui, icon::STACK_SIMPLE, "Select Layer", |ui| {
            ui.set_min_width(200.0);
            for id in hits {
                let Some(s) = app.doc.get(id) else { continue };
                let selected = app.selection.contains(&id);
                let text = RichText::new(format!("{}   {}", kind_icon(&s.kind), s.name)).size(theme::FONT_MD);
                let r = ui.add(egui::Button::new(text).selected(selected).min_size(egui::vec2(0.0, 22.0)));
                if r.hovered() {
                    app.hover = Some(id);
                }
                if r.clicked() {
                    app.selection = vec![id];
                    ui.close();
                }
            }
        });
    }
}

fn header(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(10.0).color(TEXT_DIM));
}

/// Corrections for a misspelled word in the edited text.
fn spelling(app: &mut App, ui: &mut Ui, range: std::ops::Range<usize>, word: &str, sugg: &[String]) {
    header(ui, &format!("“{word}”"));
    if sugg.is_empty() {
        ui.label(RichText::new("No suggestions").size(theme::FONT_SM).color(TEXT_DIM));
    }
    for s in sugg {
        if ui.button(RichText::new(s).strong()).clicked() {
            app.replace_text_range(range.clone(), s);
        }
    }
    ui.separator();
    let lang = app.text_edit.as_ref().map(|t| app.text_lang(t.id)).unwrap_or_default();
    if ui.button(format!("{}  Add to Dictionary", icon::BOOK_OPEN)).clicked() {
        app.spell.learn(word, &lang);
        app.spell_marks = None;
    }
    if ui.button(format!("{}  Ignore", icon::EYE_SLASH)).clicked() {
        app.spell.ignore(word);
        app.spell_marks = None;
    }
}
