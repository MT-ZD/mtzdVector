//! Components tab: every main component in the document with a preview and
//! its instance count. Click selects the main, double-click shows it, "+"
//! places an instance; drag a row onto the canvas to drop one there.

use egui::{Color32, CornerRadius, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::app::{App, Tool};
use crate::ui::theme::{self, TEXT, TEXT_DIM, TEXT_FAINT};
use crate::ui::widgets;

fn drag_key() -> egui::Id {
    egui::Id::new("component_drag")
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let mains = app.mains();
    if mains.is_empty() && app.settings.libraries.is_empty() {
        widgets::empty_state(ui, icon::DIAMONDS_FOUR, "No components yet", "Select artwork and press Ctrl+Alt+K\nto make a reusable component.");
        ui.vertical_centered(|ui| {
            let ctx = ui.ctx().clone();
            let cmd = crate::commands::Cmd::CreateComponent;
            if ui.add_enabled(cmd.enabled(app), egui::Button::new(RichText::new(format!("{}  Create component", icon::DIAMONDS_FOUR)).size(theme::FONT_MD))).clicked() {
                app.run_cmd(cmd, &ctx);
            }
        });
        ui.add_space(16.0);
        libraries_ui(app, ui);
        return;
    }
    let canvas_zoom = app.view.zoom;
    let row_h = 44.0;
    let mut dragging: Option<u64> = ui.data(|d| d.get_temp(drag_key()));
    egui::ScrollArea::vertical().id_salt("components_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for id in mains {
            let Some(s) = app.doc.get(id).cloned() else { continue };
            let n = app.instances_of(id).len();
            let selected = app.selection == [id];
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
            let bg = if selected { theme::ACCENT_DIM } else if resp.hovered() { theme::BG_HOVER } else { Color32::TRANSPARENT };
            ui.painter().rect_filled(rect, CornerRadius::same(4), bg);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            child.spacing_mut().item_spacing.x = 8.0;
            let (tr, _) = child.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            child.painter().rect_filled(tr, CornerRadius::same(4), Color32::from_gray(0x44));
            let painter = child.painter().clone();
            app.cache.paint_thumbnail(&painter, &s, &app.doc, &mut app.fonts, tr, canvas_zoom);
            child.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let from_lib = matches!(&s.component, Some(crate::document::Component::Main { library: Some(_), .. }));
                    ui.label(RichText::new(if from_lib { icon::BOOKS } else { icon::DIAMONDS_FOUR }).size(12.0).color(theme::COMPONENT));
                    ui.add(egui::Label::new(RichText::new(&s.name).size(theme::FONT_MD).color(if selected { Color32::WHITE } else { TEXT })).truncate().selectable(false));
                });
                let count = match n {
                    0 => "No instances".to_string(),
                    1 => "1 instance".to_string(),
                    n => format!("{n} instances"),
                };
                ui.label(RichText::new(count).size(theme::FONT_SM).color(TEXT_FAINT));
            });
            let mut add = false;
            child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.rect_contains_pointer(rect) || selected {
                    add = widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), "Place an instance (or drag the row onto the canvas)", None).clicked();
                }
            });
            if add {
                let at = if app.artboard_selected { app.active().rect().center() } else { app.view_center_doc() };
                app.insert_instance(id, at);
                app.set_tool(Tool::Select);
            } else if resp.double_clicked() {
                app.selection = vec![id];
                if let Some(b) = app.selection_bounds() {
                    let m = b.width().max(b.height()) * 0.5;
                    app.zoom_to_rect(b.inflate(m, m), 96.0);
                }
            } else if resp.clicked() {
                app.selection = vec![id];
                app.show_design();
            }
            if resp.drag_started() {
                dragging = Some(id);
            }
        }
        ui.add_space(10.0);
        libraries_ui(app, ui);
    });
    // Drag a component onto the canvas.
    if let Some(id) = dragging {
        let (pos, released) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_released()));
        if let Some(pos) = pos {
            let over = app.canvas_rect.contains(pos);
            ui.ctx().set_cursor_icon(if over { egui::CursorIcon::Copy } else { egui::CursorIcon::Grabbing });
            let name = app.doc.get(id).map(|s| s.name.clone()).unwrap_or_default();
            let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("component_drag_ghost"));
            let p = ui.ctx().layer_painter(layer);
            let text = format!("{}  {name}", icon::DIAMONDS_FOUR);
            let g = p.layout_no_wrap(text, egui::FontId::proportional(theme::FONT_MD), Color32::WHITE);
            let r = egui::Rect::from_min_size(pos + vec2(14.0, 10.0), g.size() + vec2(16.0, 10.0));
            p.rect_filled(r, CornerRadius::same(4), if over { theme::ACCENT } else { theme::BG_HOVER });
            p.galley(r.min + vec2(8.0, 5.0), g, Color32::WHITE);
            if released {
                if over {
                    let at = app.view.to_doc(pos);
                    if let Some(inst) = app.insert_instance(id, at) {
                        // Dropped on an auto layout: it joins it.
                        if let Some(g) = app.layout_at(at, &[inst]) {
                            app.join_layout(&[inst], g);
                        }
                    }
                    app.set_tool(Tool::Select);
                }
                dragging = None;
            }
        }
        if !ui.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
            dragging = None;
        }
    }
    ui.data_mut(|d| match dragging {
        Some(id) => {
            d.insert_temp(drag_key(), id);
        }
        None => d.remove::<u64>(drag_key()),
    });
}

/// Shared libraries: other documents whose components can be placed here.
fn libraries_ui(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(RichText::new("LIBRARIES").size(theme::FONT_SM).color(TEXT_FAINT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            let r = widgets::tip_long(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), "Add library", None, "Use the components of another .mvec document in this one");
            if r.clicked() {
                if let Some(p) = rfd::FileDialog::new().set_title("Add Library").add_filter("Vector document", &["mvec"]).pick_file() {
                    if !app.settings.libraries.contains(&p) {
                        app.settings.libraries.push(p);
                    }
                }
            }
        });
    });
    if app.settings.libraries.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Add a document to reuse its components here.").size(theme::FONT_SM).color(TEXT_FAINT));
        });
        return;
    }
    // Pending updates, checked every couple of seconds.
    let now = ui.input(|i| i.time);
    let key = egui::Id::new("library_updates");
    let cached: Option<(f64, u64, Vec<(u64, bool)>)> = ui.data(|d| d.get_temp(key));
    let updates = match cached {
        Some((t, rev, u)) if now - t < 2.0 && rev == app.doc.revision() => u,
        _ => {
            let u = app.library_updates();
            ui.data_mut(|d| d.insert_temp(key, (now, app.doc.revision(), u.clone())));
            u
        }
    };
    let mut remove = None;
    let mut place = None;
    let mut update = None;
    for path in app.settings.libraries.clone() {
        let name = crate::library::library_name(&path);
        let pending = updates.iter().filter(|(id, ok)| *ok && app.doc.get(*id).is_some_and(|s| matches!(&s.component, Some(crate::document::Component::Main { library: Some(l), .. }) if l.path == path))).count();
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new(icon::BOOKS).size(13.0).color(TEXT_DIM));
            ui.add(egui::Label::new(RichText::new(&name).size(theme::FONT_MD).color(TEXT)).truncate().selectable(false)).on_hover_text(path.display().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if widgets::tip(widgets::mini_icon(ui, icon::X, TEXT_DIM), "Remove the library (placed components stay)", None).clicked() {
                    remove = Some(path.clone());
                }
                if pending > 0 && widgets::flat_button(ui, Some(icon::ARROWS_CLOCKWISE), &format!("Update {pending}"), true).on_hover_text("The library changed: update its components here (overrides are kept)").clicked() {
                    update = Some(path.clone());
                }
            });
        });
        match app.libraries.get(&path) {
            Err(e) => {
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    ui.label(RichText::new(format!("Can't open: {e}")).size(theme::FONT_SM).color(TEXT_FAINT));
                });
            }
            Ok(lib) => {
                let comps = lib.components();
                if comps.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(26.0);
                        ui.label(RichText::new("No components").size(theme::FONT_SM).color(TEXT_FAINT));
                    });
                }
                for (id, cname, set) in comps {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(4), theme::BG_HOVER);
                    }
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                    child.spacing_mut().item_spacing.x = 6.0;
                    child.add_space(20.0);
                    child.label(RichText::new(icon::DIAMONDS_FOUR).size(11.0).color(theme::COMPONENT));
                    let label = match set {
                        Some(set) => format!("{set} · {cname}"),
                        None => cname,
                    };
                    child.add(egui::Label::new(RichText::new(label).size(theme::FONT_SM).color(TEXT)).truncate().selectable(false));
                    if resp.on_hover_text("Click to place an instance").clicked() {
                        place = Some((path.clone(), id));
                    }
                }
            }
        }
        ui.add_space(4.0);
    }
    if let Some(p) = remove {
        app.settings.libraries.retain(|x| *x != p);
        app.libraries.forget(&p);
    }
    if let Some(p) = update {
        let n = app.update_library_components(Some(&p));
        app.notify(format!("Updated {n} component{}", if n == 1 { "" } else { "s" }));
        ui.data_mut(|d| d.remove::<(f64, u64, Vec<(u64, bool)>)>(key));
    }
    if let Some((p, id)) = place {
        let at = if app.artboard_selected { app.active().rect().center() } else { app.view_center_doc() };
        if app.insert_library_component(&p, id, at).is_some() {
            app.set_tool(Tool::Select);
        }
    }
}
