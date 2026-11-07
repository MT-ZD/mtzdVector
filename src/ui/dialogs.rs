//! Small windows: artboard settings, keyboard shortcuts, about.

use egui::{RichText, Ui};
use egui_phosphor::regular as icon;

use super::studio::transform::artboard_ui;
use super::theme::{self, ACCENT, TEXT_DIM};
use crate::app::App;
use super::widgets;
use crate::color;
use crate::commands::{Category, Cmd};

/// A modal with the app's dialog frame: roomy margins, rounded corners and
/// a soft shadow.
pub fn modal(id: &str) -> egui::Modal {
    let frame = egui::Frame::new()
        .fill(theme::BG_PANEL)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(10)
        .inner_margin(egui::Margin { left: 22, right: 22, top: 20, bottom: 18 })
        .shadow(egui::Shadow { offset: [0, 10], blur: 32, spread: 0, color: egui::Color32::from_black_alpha(130) });
    egui::Modal::new(egui::Id::new(id)).frame(frame)
}

fn footer_button(ui: &mut Ui, label: &str, primary: bool) -> egui::Response {
    let text = RichText::new(label).size(theme::FONT_MD);
    let b = if primary {
        egui::Button::new(text.color(egui::Color32::WHITE)).fill(ACCENT).stroke(egui::Stroke::NONE)
    } else {
        egui::Button::new(text)
    };
    ui.add(b.corner_radius(6).min_size(egui::vec2(76.0, 30.0)))
}

/// Standard dialog footer: secondary actions left, primary action right.
pub fn footer(ui: &mut Ui, left: &[&str], right: &[&str], primary: &str) -> Option<String> {
    let mut out = None;
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for l in left {
            if footer_button(ui, l, false).clicked() {
                out = Some(l.to_string());
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if footer_button(ui, primary, true).clicked() {
                out = Some(primary.to_string());
            }
            for r in right {
                if footer_button(ui, r, false).clicked() {
                    out = Some(r.to_string());
                }
            }
        });
    });
    out
}

pub fn modal_title(ui: &mut Ui, glyph: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new(glyph).size(20.0).color(ACCENT));
        ui.label(RichText::new(title).size(15.0).strong());
    });
    ui.add_space(10.0);
}

/// Body text of a modal: dimmed, wrapped, comfortable line spacing.
pub fn modal_text(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(theme::FONT_MD).color(TEXT_DIM).line_height(Some(19.0)));
}

/// Unsaved-changes prompt and crash-recovery offer.
fn prompts(app: &mut App, ctx: &egui::Context) {
    if let Some(action) = app.session.prompt.clone() {
        let name = app.file_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
        let mut answer: Option<Option<bool>> = None;
        let r = modal("unsaved_prompt").show(ctx, |ui| {
            ui.set_width(380.0);
            modal_title(ui, icon::WARNING_CIRCLE, &format!("Save changes to “{name}”?"));
            modal_text(ui, &format!("Your changes will be lost if you don't save them before {}.", action.verb()));
            match footer(ui, &["Don't Save"], &["Cancel"], "Save").as_deref() {
                Some("Save") => answer = Some(Some(true)),
                Some("Don't Save") => answer = Some(Some(false)),
                Some(_) => answer = Some(None),
                None => {}
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                answer = Some(Some(true));
            }
        });
        if r.should_close() && answer.is_none() {
            answer = Some(None);
        }
        if let Some(a) = answer {
            app.answer_prompt(a, ctx);
        }
        return;
    }
    if let Some(rec) = app.session.recovery.clone() {
        let mut choice = None;
        modal("recovery_prompt").show(ctx, |ui| {
            ui.set_width(380.0);
            modal_title(ui, icon::LIFEBUOY, "Recover unsaved work?");
            let what = rec.original.as_ref().and_then(|p| p.file_name()).map(|n| format!("“{}”", n.to_string_lossy())).unwrap_or_else(|| "an untitled document".into());
            let ago = rec.modified.elapsed().map(|d| fmt_ago(d.as_secs())).unwrap_or_default();
            modal_text(ui, &format!("mtzdVector didn't close properly. An autosave of {what} from {ago} is available."));
            choice = footer(ui, &["Discard"], &["Later"], "Recover");
        });
        match choice.as_deref() {
            Some("Recover") => {
                app.session.recovery = None;
                app.recover(rec);
            }
            Some("Discard") => {
                app.session.recovery = None;
                crate::session::discard(&rec);
            }
            Some(_) => app.session.recovery = None,
            None => {}
        }
    }
}

fn fmt_ago(s: u64) -> String {
    match s {
        0..=59 => "moments ago".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    prompts(app, ctx);
    offset_dialog(app, ctx);
    super::paint_dialogs::show(app, ctx);
    preferences(app, ctx);
    if app.ui.artboard_open {
        let mut open = true;
        egui::Window::new(format!("{}  Artboard", icon::FRAME_CORNERS))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(300.0)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 120.0])
            .show(ctx, |ui| {
                artboard_ui(app, ui);
                ui.add_space(6.0);
                ui.label(RichText::new("Background").size(theme::FONT_SM).color(TEXT_DIM));
                let mut c = app.active().background;
                if color::color_picker(ui, &mut c, egui::Id::new("bg_picker_dialog")) {
                    app.active_mut().background = c;
                    app.dirty = true;
                }
            });
        app.ui.artboard_open = open;
    }
    if app.ui.shortcuts_open {
        let mut open = true;
        egui::Window::new(format!("{}  Keyboard Shortcuts", icon::KEYBOARD))
            .open(&mut open)
            .collapsible(false)
            .default_size([520.0, 560.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| shortcuts(ui));
        app.ui.shortcuts_open = open;
    }
    if app.ui.about_open {
        let mut open = true;
        egui::Window::new("About").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                crate::logo::show(ui, 44.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("mtzdVector").size(16.0).strong());
                    ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM));
                });
            });
            ui.add_space(6.0);
            ui.label(RichText::new("A compact vector editor: shapes, paths, text, booleans,\nshape builder, SVG / PDF / EPS import & export.").color(TEXT_DIM));
        });
        app.ui.about_open = open;
    }
}

fn shortcuts(ui: &mut Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let cats = [
            Category::File,
            Category::Edit,
            Category::Object,
            Category::Arrange,
            Category::Align,
            Category::Boolean,
            Category::Text,
            Category::View,
            Category::Tools,
            Category::Help,
        ];
        for cat in cats {
            let cmds: Vec<Cmd> = Cmd::all().into_iter().filter(|c| c.meta().category == cat && !c.meta().keys.is_empty()).collect();
            if cmds.is_empty() {
                continue;
            }
            ui.add_space(6.0);
            ui.label(RichText::new(cat.label().to_uppercase()).size(10.5).color(TEXT_DIM));
            egui::Grid::new(("sc", cat.label())).num_columns(2).spacing([24.0, 4.0]).min_col_width(220.0).show(ui, |ui| {
                for c in cmds {
                    ui.label(format!("{}   {}", c.icon(), c.label()));
                    let keys = c.meta().keys.iter().map(crate::commands::format_shortcut).collect::<Vec<_>>().join("  /  ");
                    ui.label(RichText::new(keys).monospace().color(TEXT_DIM));
                    ui.end_row();
                }
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Also: arrows nudge (Shift ×10) · Space+drag pans · Esc cancels · Enter edits").size(theme::FONT_SM).color(TEXT_DIM));
    });
}

/// Offset Path: grow or inset the selection into new shapes.
pub fn offset_dialog(app: &mut App, ctx: &egui::Context) {
    if !app.ui.offset.open {
        return;
    }
    let mut action = None;
    let r = modal("offset_dialog").show(ctx, |ui| {
        ui.set_width(340.0);
        modal_title(ui, icon::ARROWS_OUT_SIMPLE, "Offset Path");
        let st = &mut app.ui.offset;
        widgets::prop_row(ui, "Distance", |ui| {
            widgets::num_field(ui, "", &mut st.distance, widgets::Num::px().range(-10000.0, 10000.0).speed(0.5), 120.0);
        });
        widgets::prop_row(ui, "Joins", |ui| {
            widgets::segmented(ui, &mut st.round, &[(true, "Round"), (false, "Miter")]);
        });
        ui.label(RichText::new("Creates new shapes above the selection. Use a negative distance to inset.").size(theme::FONT_SM).color(theme::TEXT_FAINT));
        action = footer(ui, &[], &["Cancel"], "Offset");
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            action = Some("Offset".into());
        }
    });
    if r.should_close() && action.is_none() {
        action = Some("Cancel".into());
    }
    match action.as_deref() {
        Some("Offset") => {
            app.ui.offset.open = false;
            let (d, round) = (app.ui.offset.distance, app.ui.offset.round);
            app.offset_selection(d, round);
        }
        Some(_) => app.ui.offset.open = false,
        None => {}
    }
}

/// Preferences: interface size, nudge, autosave, new-document unit, layout reset.
fn preferences(app: &mut App, ctx: &egui::Context) {
    if !app.ui.prefs.open {
        return;
    }
    let mut action = None;
    let r = modal("prefs_dialog").show(ctx, |ui| {
        ui.set_width(400.0);
        modal_title(ui, icon::GEAR_SIX, "Preferences");
        let s = &mut app.settings;
        widgets::prop_row(ui, "Interface", |ui| {
            widgets::segmented(ui, &mut s.ui_scale, &[(0.9, "90%"), (1.0, "100%"), (1.15, "115%"), (1.3, "130%"), (1.5, "150%")]);
        });
        widgets::prop_row(ui, "Nudge", |ui| {
            widgets::num_field(ui, "", &mut s.nudge, widgets::Num::px().range(0.01, 1000.0).speed(0.1), 90.0);
            ui.label(RichText::new("Shift").size(theme::FONT_SM).color(TEXT_DIM));
            widgets::num_field(ui, "", &mut s.nudge_big, widgets::Num::px().range(0.01, 1000.0).speed(0.1), 90.0);
        });
        widgets::prop_row(ui, "Autosave", |ui| {
            widgets::segmented(ui, &mut s.autosave_secs, &[(0.0, "Off"), (30.0, "30 s"), (60.0, "1 min"), (300.0, "5 min")]);
        });
        widgets::prop_row(ui, "Backup", |ui| {
            widgets::tip(ui.checkbox(&mut s.keep_backup, "Keep the previous version"), "Saving keeps the file it replaces as NAME.mvec~ next to it", None);
        });
        widgets::prop_row(ui, "Text", |ui| {
            widgets::tip(ui.checkbox(&mut s.spell_check, "Check spelling"), "Underline misspelled words while editing text. Dictionaries come from the system, or are downloaded once (wooorm/dictionaries)", None);
            let code = s.language.clone();
            widgets::dropdown(ui, "pref_lang", crate::ui::studio::typography::language_name(&code), 130.0, |ui| {
                for (c, name) in crate::ui::studio::typography::LANGUAGES {
                    if ui.selectable_label(*c == code, *name).clicked() {
                        s.language = c.to_string();
                    }
                }
            });
        });
        widgets::prop_row(ui, "Tablet", |ui| {
            widgets::tip_long(ui.checkbox(&mut s.tablet, "Pen pressure"), "Pen pressure", None, "Read drawing tablets directly for pen pressure (Wayland). Turn off if your pen misbehaves; it then works as a mouse. Applies after a restart");
        });
        widgets::prop_row(ui, "Units", |ui| {
            let unit = s.default_unit;
            widgets::dropdown(ui, "pref_unit", format!("{}  {}", icon::RULER, unit.label()), 140.0, |ui| {
                for u in crate::units::Unit::ALL {
                    if ui.selectable_label(u == unit, format!("{} ({})", u.label(), u.suffix())).clicked() {
                        s.default_unit = u;
                    }
                }
            });
            ui.label(RichText::new("for new documents").size(theme::FONT_SM).color(TEXT_DIM));
        });
        ui.add_space(4.0);
        ui.label(RichText::new("Panel sizes, tabs and view toggles are remembered between sessions.").size(theme::FONT_SM).color(theme::TEXT_FAINT));
        action = footer(ui, &["Reset Layout"], &[], "Done");
    });
    if r.should_close() && action.is_none() {
        action = Some("Done".into());
    }
    match action.as_deref() {
        Some("Reset Layout") => {
            // Workspace back to defaults; preferences stay.
            let d = crate::settings::Settings::default();
            let keep = app.settings.clone();
            let s = crate::settings::Settings { ui_scale: keep.ui_scale, nudge: keep.nudge, nudge_big: keep.nudge_big, autosave_secs: keep.autosave_secs, default_unit: keep.default_unit, keep_backup: keep.keep_backup, ..d };
            app.apply_settings(s, ctx);
            // The studio dock remembers its width in egui's panel state.
            ctx.data_mut(|d| d.remove::<egui::containers::panel::PanelState>(egui::Id::new("studio")));
            app.notify("Panel layout reset");
        }
        Some(_) => app.ui.prefs.open = false,
        None => {}
    }
}
