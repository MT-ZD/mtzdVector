//! Plugins › Manage Plugins…: the compiled-in plugins, each with a switch.

use egui::{RichText, Stroke};
use egui_phosphor::regular as icon;

use crate::app::App;
use crate::ui::dialogs::{footer, modal, modal_text, modal_title};
use crate::ui::theme::{self, TEXT, TEXT_DIM, TEXT_FAINT};

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.ui.plugins_open {
        return;
    }
    let mut close = false;
    let r = modal("plugins_dialog").show(ctx, |ui| {
        let w = 440.0;
        ui.set_width(w);
        modal_title(ui, icon::PUZZLE_PIECE, "Plugins");
        modal_text(ui, "Plugins add tools, panels and commands. Switched-off plugins keep their settings.");
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        if app.plugins.plugins.is_empty() {
            ui.label(RichText::new("No plugins in this build.").color(TEXT_FAINT));
        }
        for i in 0..app.plugins.plugins.len() {
            let (name, desc, glyph) = {
                let p = &app.plugins.plugins[i];
                (p.name(), p.description(), p.icon())
            };
            let mut on = app.plugins.is_on(i);
            egui::Frame::new().fill(theme::BG_INPUT).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(w - 26.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(glyph).size(22.0).color(if on { theme::ACCENT } else { TEXT_FAINT }));
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.set_max_width(w - 140.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(name).size(theme::FONT_LG).strong().color(TEXT));
                        ui.label(RichText::new(desc).size(theme::FONT_SM).color(TEXT_DIM));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let r = ui.checkbox(&mut on, "On");
                        crate::ui::widgets::describe(&r, egui::accesskit::Role::CheckBox, &format!("{name} plugin"), None);
                    });
                });
            });
            if on != app.plugins.is_on(i) {
                app.with_plugins(|h, app| h.set_enabled(i, on, app));
            }
        }
        if footer(ui, &[], &[], "Done").is_some() {
            close = true;
        }
    });
    if close || r.should_close() {
        app.ui.plugins_open = false;
    }
}
