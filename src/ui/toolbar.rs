//! Main toolbar with frequent operations.

use egui::{Color32, RichText, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;

use super::menubar::{self, ARRANGE};
use super::theme::{self, TEXT_DIM};
use super::widgets;
use crate::app::App;
use crate::commands::Cmd;

/// Icon button bound to a command (enabled state + tooltip with shortcut).
pub fn cmd_button(ui: &mut Ui, app: &mut App, cmd: Cmd) -> egui::Response {
    let enabled = cmd.enabled(app);
    let on = matches!(cmd.checked(app), Some(true)) && !matches!(cmd, Cmd::Tool(_));
    let r = widgets::icon_button_sized(ui, cmd.icon(), enabled, on, vec2(26.0, 26.0), theme::ICON);
    let r = widgets::tip(r, cmd.label(), cmd.shortcut_text().as_deref());
    if r.clicked() {
        let ctx = ui.ctx().clone();
        app.run_cmd(cmd, &ctx);
    }
    r
}

/// Toolbar dropdown: icon + caret opening a menu.
fn dropdown_button(ui: &mut Ui, glyph: &str, tooltip: &str, add: impl FnOnce(&mut Ui)) {
    let text = RichText::new(format!("{glyph} {}", icon::CARET_DOWN)).size(theme::ICON);
    let r = ui.menu_button(text, |ui| {
        ui.set_min_width(240.0);
        add(ui)
    });
    widgets::tip(r.response, tooltip, None);
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let frame = egui::Frame::new().fill(theme::BG_PANEL).inner_margin(egui::Margin::symmetric(8, 0));
    egui::Panel::top("toolbar").exact_size(theme::TOOLBAR_H).frame(frame).show(ui, |ui| {
        let full = ui.max_rect();
        ui.painter().hline(full.x_range(), full.bottom() - 0.5, Stroke::new(1.0, theme::SEPARATOR));
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            {
                let v = &mut ui.style_mut().visuals.widgets;
                v.inactive.weak_bg_fill = Color32::TRANSPARENT;
                v.inactive.bg_stroke = Stroke::NONE;
                v.hovered.bg_stroke = Stroke::NONE;
                v.open.bg_stroke = Stroke::NONE;
            }
            cmd_button(ui, app, Cmd::Undo);
            cmd_button(ui, app, Cmd::Redo);
            widgets::vsep(ui);
            for c in menubar::BOOLEAN {
                cmd_button(ui, app, c);
            }
            widgets::vsep(ui);
            dropdown_button(ui, icon::ALIGN_LEFT, "Align & distribute", |ui| menubar::align_menu(ui, app));
            dropdown_button(ui, icon::STACK, "Arrange", |ui| menubar::cmd_items(ui, app, &ARRANGE));
            widgets::vsep(ui);
            cmd_button(ui, app, Cmd::FlipH);
            cmd_button(ui, app, Cmd::FlipV);
            widgets::vsep(ui);
            cmd_button(ui, app, Cmd::ConvertToCurves);
            cmd_button(ui, app, Cmd::ExpandStroke);
            widgets::vsep(ui);
            cmd_button(ui, app, Cmd::ToggleSnap);
            cmd_button(ui, app, Cmd::ToggleRulers);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let r = widgets::primary_button(ui, &format!("{}  Export", icon::EXPORT));
                if widgets::tip(r, "Export…", Cmd::Export.shortcut_text().as_deref()).clicked() {
                    let ctx = ui.ctx().clone();
                    app.run_cmd(Cmd::Export, &ctx);
                }
                ui.add_space(6.0);
                // Quick actions launcher.
                let label = RichText::new(format!("{}  Quick actions", icon::LIGHTNING)).size(theme::FONT_SM).color(TEXT_DIM);
                let b = egui::Button::new(label)
                    .fill(theme::BG_INPUT)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .shortcut_text(RichText::new("Ctrl+K").size(theme::FONT_SM).color(theme::TEXT_FAINT))
                    .min_size(vec2(180.0, 24.0));
                if ui.add(b).clicked() {
                    app.ui.open_palette();
                }
            });
        });
    });
}
