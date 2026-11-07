//! Icons tab: search Iconify and insert icons as editable shapes.

use egui::{RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::app::App;
use crate::icons::COLLECTIONS;
use crate::ui::theme::{self, TEXT_DIM};
use crate::ui::widgets;

const CELL: f32 = 44.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    app.icons.poll();
    if let Some((name, at)) = app.icons.pending_insert.clone() {
        if app.icons.svg(&name).is_some() {
            app.icons.pending_insert = None;
            app.insert_icon_at(&name, at);
        }
    }
    let mut go = false;
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut app.icons.query).hint_text(format!("{}  Search icons", icon::MAGNIFYING_GLASS)).desired_width(ui.available_width() - 34.0));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            go = true;
        }
        if widgets::tip(widgets::mini_icon(ui, icon::MAGNIFYING_GLASS, TEXT_DIM), "Search", Some("Enter")).clicked() {
            go = true;
        }
    });
    ui.add_space(4.0);
    widgets::prop_row(ui, "Set", |ui| {
        let cur = COLLECTIONS[app.icons.collection.min(COLLECTIONS.len() - 1)].1;
        widgets::dropdown(ui, "icon_set", cur, 150.0, |ui| {
            for (i, (_, label)) in COLLECTIONS.iter().enumerate() {
                if ui.selectable_value(&mut app.icons.collection, i, *label).clicked() {
                    go = true;
                }
            }
        });
    });
    if go {
        app.icons.search();
    }
    ui.add_space(4.0);
    if app.icons.searching {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Searching Iconify…").size(theme::FONT_SM).color(TEXT_DIM));
        });
    }
    if let Some(e) = &app.icons.error {
        ui.label(RichText::new(e).size(theme::FONT_SM).color(TEXT_DIM));
    }
    if app.icons.results.is_empty() {
        if !app.icons.searching && app.icons.error.is_none() {
            widgets::empty_state(ui, icon::SMILEY, "Iconify icons", "Search 200,000+ open-source icons.\nClick an icon to insert it.");
        }
        return;
    }
    let cols = ((ui.available_width() + 4.0) / (CELL + 4.0)).floor().max(1.0) as usize;
    let names = app.icons.results.clone();
    let drag_key = egui::Id::new("icon_drag");
    let mut dragging: Option<String> = ui.data(|d| d.get_temp(drag_key));
    egui::ScrollArea::vertical().id_salt("icon_grid").auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("icons").spacing(vec2(4.0, 4.0)).show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(CELL, CELL), Sense::click_and_drag());
                if ui.is_rect_visible(rect) {
                    let bg = if resp.hovered() { theme::BG_HOVER } else { theme::BG_INPUT };
                    ui.painter().rect_filled(rect, 4.0, bg);
                    let inner = rect.shrink(9.0);
                    match app.icons.preview(name, &mut app.fonts) {
                        Some(m) => crate::render::paint_raw_mesh(ui.painter(), &m.verts, &m.indices, inner.min, inner.width(), theme::TEXT),
                        None => {
                            ui.painter().circle_filled(rect.center(), 2.0, TEXT_DIM);
                        }
                    }
                }
                let resp = widgets::tip(resp, name, None);
                if resp.clicked() {
                    if app.icons.svg(name).is_some() {
                        app.insert_icon(name);
                    } else {
                        app.icons.request(name);
                        app.icons.pending_insert = Some((name.clone(), None));
                    }
                }
                if resp.drag_started() {
                    if app.icons.svg(name).is_none() {
                        app.icons.request(name);
                    }
                    dragging = Some(name.clone());
                }
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
    });
    // Drag an icon onto the canvas to place it there.
    if let Some(name) = dragging.clone() {
        let (pos, released) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_released()));
        if let Some(pos) = pos {
            let over = app.canvas_rect.contains(pos);
            ui.ctx().set_cursor_icon(if over { egui::CursorIcon::Copy } else { egui::CursorIcon::Grabbing });
            let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("icon_drag_ghost"));
            let p = ui.ctx().layer_painter(layer);
            let r = egui::Rect::from_center_size(pos, vec2(CELL, CELL));
            p.rect_filled(r, 4.0, if over { theme::ACCENT } else { theme::BG_HOVER });
            if let Some(m) = app.icons.preview(&name, &mut app.fonts) {
                let inner = r.shrink(9.0);
                crate::render::paint_raw_mesh(&p, &m.verts, &m.indices, inner.min, inner.width(), egui::Color32::WHITE);
            }
            if released {
                if over {
                    let at = app.view.to_doc(pos);
                    if app.icons.svg(&name).is_some() {
                        app.insert_icon_at(&name, Some(at));
                    } else {
                        app.icons.pending_insert = Some((name.clone(), Some(at)));
                    }
                }
                dragging = None;
            }
        }
        if !ui.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
            dragging = None;
        }
    }
    ui.data_mut(|d| match dragging {
        Some(n) => {
            d.insert_temp(drag_key, n);
        }
        None => d.remove::<String>(drag_key),
    });
}
