//! Transform section of the Design panel (alignment, anchor, X/Y, W/H,
//! rotation) and the artboard settings.

use egui::{RichText, Ui};
use egui_phosphor::regular as icon;
use kurbo::{Affine, Point};

use crate::app::App;
use crate::commands::Cmd;
use crate::document::ShapeKind;
use crate::geom;
use crate::units::Unit;
use crate::ui::menubar::ALIGN;
use crate::ui::theme;
use crate::ui::toolbar::cmd_button;
use crate::ui::widgets::{self, Num};

/// Artboard presets: (label, width, height, unit). Physical sizes follow the DPI.
pub const PRESETS: [(&str, f64, f64, Unit); 8] = [
    ("1080p", 1920.0, 1080.0, Unit::Px),
    ("4K", 3840.0, 2160.0, Unit::Px),
    ("Square", 1080.0, 1080.0, Unit::Px),
    ("Icon", 512.0, 512.0, Unit::Px),
    ("A4", 210.0, 297.0, Unit::Mm),
    ("A5", 148.0, 210.0, Unit::Mm),
    ("A3", 297.0, 420.0, Unit::Mm),
    ("Letter", 8.5, 11.0, Unit::In),
];

/// Settings of the active artboard (name, position, size, presets) and the
/// document-wide unit, DPI and grid.
pub fn artboard_ui(app: &mut App, ui: &mut Ui) {
    let before = app.active().clone();
    let mut a = before.clone();
    widgets::text_field(ui, &mut a.name, "Artboard name", f32::INFINITY);
    let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
    ui.horizontal(|ui| {
        widgets::num_field(ui, "X", &mut a.x, Num::px(), fw);
        widgets::num_field(ui, "Y", &mut a.y, Num::px(), fw);
    });
    ui.horizontal(|ui| {
        widgets::num_field(ui, "W", &mut a.w, Num::px().range(1.0, 100000.0), fw);
        widgets::num_field(ui, "H", &mut a.h, Num::px().range(1.0, 100000.0), fw);
    });
    widgets::tip(ui.checkbox(&mut a.clip, "Clip content"), "Hide artwork outside the artboard's edges", None);
    if a != before {
        app.checkpoint("artboard");
        a.w = a.w.max(1.0);
        a.h = a.h.max(1.0);
        *app.active_mut() = a;
    }
    ui.horizontal(|ui| {
        let unit = app.doc.unit;
        let mut pick = None;
        widgets::dropdown(ui, "doc_unit", format!("{}  {}", icon::RULER, unit.label()), 118.0, |ui| {
            for u in Unit::ALL {
                if ui.selectable_label(u == unit, format!("{} ({})", u.label(), u.suffix())).clicked() {
                    pick = Some(u);
                }
            }
        });
        if let Some(u) = pick {
            app.set_unit(u);
        }
        let mut dpi = app.doc.dpi;
        if widgets::tip(widgets::num_field(ui, "DPI", &mut dpi, Num::default().range(1.0, 2400.0).decimals(0), ui.available_width().min(120.0)), "Pixels per inch: converts px to physical units and sets the PDF page size", None).changed() {
            app.doc.dpi = dpi.max(1.0);
            app.dirty = true;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        let dpi = app.doc.dpi;
        for (label, pw, ph, unit) in PRESETS {
            let (w, h) = (unit.to_px(pw, dpi), unit.to_px(ph, dpi));
            let sel = (app.active().w - w).abs() < 0.01 && (app.active().h - h).abs() < 0.01;
            let b = egui::Button::new(RichText::new(label).size(theme::FONT_SM)).selected(sel).min_size(egui::vec2(0.0, 20.0));
            if ui.add(b).on_hover_text(format!("{} × {} {}", pw, ph, unit.suffix())).clicked() {
                app.checkpoint("");
                let ab = app.active_mut();
                ab.w = w;
                ab.h = h;
                if unit != Unit::Px && app.doc.unit == Unit::Px {
                    app.set_unit(unit);
                }
                let id = app.active().id;
                app.zoom_to_artboard(id);
            }
        }
    });
    if ui.add(egui::Button::new(RichText::new(format!("{}  Swap orientation", icon::ARROWS_CLOCKWISE)).size(theme::FONT_SM))).clicked() {
        app.checkpoint("");
        let ab = app.active_mut();
        std::mem::swap(&mut ab.w, &mut ab.h);
        let id = app.active().id;
        app.zoom_to_artboard(id);
    }
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let mut g = app.doc.grid;
        if widgets::tip(widgets::num_field(ui, "Grid", &mut g, Num::px().range(1.0, 10000.0), 118.0), "Grid spacing (shown and snapped to when the grid is on)", None).changed() {
            app.doc.grid = g.max(1.0);
            app.dirty = true;
        }
        let r = widgets::icon_toggle(ui, &mut app.ui.show_grid, icon::GRID_FOUR);
        widgets::tip(r, "Show grid", crate::commands::Cmd::ToggleGrid.shortcut_text().as_deref());
        let r = widgets::icon_toggle(ui, &mut app.ui.show_guides, icon::CROSSHAIR_SIMPLE);
        widgets::tip(r, "Show guides — drag from a ruler to add one", crate::commands::Cmd::ToggleGuides.shortcut_text().as_deref());
    });
}

/// Position, size, rotation and alignment of the selection.
pub fn transform_section(app: &mut App, ui: &mut Ui) {
    let Some(b) = app.selection_bounds() else { return };
    let multi = app.selection.len() > 1;
    let first = app.doc.get(app.selection[0]).cloned();
    widgets::section_icon(ui, "transform", icon::ARROWS_OUT_CARDINAL, "Transform", |_| {}, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for c in ALIGN {
                cmd_button(ui, app, c);
            }
            ui.add_space(4.0);
            if multi {
                cmd_button(ui, app, Cmd::DistributeH);
                cmd_button(ui, app, Cmd::DistributeV);
            } else {
                cmd_button(ui, app, Cmd::FlipH);
                cmd_button(ui, app, Cmd::FlipV);
            }
        });
        ui.add_space(2.0);
        let (ax, ay) = app.ui.anchor;
        let fx = ax as f64 / 2.0;
        let fy = ay as f64 / 2.0;
        let anchor = Point::new(b.x0 + b.width() * fx, b.y0 + b.height() * fy);
        // X/Y are relative to the active artboard (it follows the selection),
        // matching the rulers.
        let (ox, oy) = (app.active().x, app.active().y);
        let (mut x, mut y, mut w, mut h) = (anchor.x - ox, anchor.y - oy, b.width(), b.height());
        let mut changed_pos = false;
        let mut changed_size = false;
        let mut changed_rot = false;
        let rot = first.as_ref().filter(|_| !multi).map(|p| geom::decompose(p.transform).1.to_degrees()).unwrap_or(0.0);
        let mut r = rot;
        ui.horizontal(|ui| {
            let mut a = app.ui.anchor;
            widgets::anchor_selector(ui, &mut a);
            app.ui.anchor = a;
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
                ui.horizontal(|ui| {
                    changed_pos |= widgets::num_field(ui, "X", &mut x, Num::px(), fw).changed();
                    changed_pos |= widgets::num_field(ui, "Y", &mut y, Num::px(), fw).changed();
                });
                ui.horizontal(|ui| {
                    let fw = fw - 11.0;
                    let lock = app.ui.lock_aspect;
                    let ratio = b.height() / b.width().max(1e-9);
                    if widgets::num_field(ui, "W", &mut w, Num::px().range(0.01, 1e6), fw).changed() {
                        changed_size = true;
                        if lock {
                            h = w * ratio;
                        }
                    }
                    let r = widgets::icon_button_sized(ui, if lock { icon::LINK_SIMPLE } else { icon::LINK_BREAK }, true, lock, egui::vec2(18.0, theme::ROW_H), 12.0);
                    if widgets::tip(r, if lock { "Unlock aspect ratio" } else { "Lock aspect ratio (also for the resize handles)" }, None).clicked() {
                        app.ui.lock_aspect = !lock;
                    }
                    if widgets::num_field(ui, "H", &mut h, Num::px().range(0.01, 1e6), fw).changed() {
                        changed_size = true;
                        if lock {
                            w = h / ratio.max(1e-9);
                        }
                    }
                });
                let can_rotate = !multi && first.as_ref().map(|p| !matches!(p.kind, ShapeKind::Path(_))).unwrap_or(false);
                if can_rotate {
                    ui.horizontal(|ui| {
                        changed_rot = widgets::num_field(ui, icon::ARROW_CLOCKWISE, &mut r, Num::default().suffix("°").speed(0.5).decimals(1), fw).changed();
                    });
                }
            });
        });
        let a = if changed_pos {
            Some(Affine::translate((x + ox - anchor.x, y + oy - anchor.y)))
        } else if changed_size && w > 0.01 && h > 0.01 {
            Some(Affine::translate(anchor.to_vec2()) * Affine::scale_non_uniform(w / b.width().max(1e-9), h / b.height().max(1e-9)) * Affine::translate(-anchor.to_vec2()))
        } else if changed_rot {
            Some(Affine::rotate_about((r - rot).to_radians(), anchor))
        } else {
            None
        };
        if let Some(a) = a {
            app.checkpoint("transform");
            for id in app.selection.clone() {
                if let Some(mut s) = app.doc.get(id).cloned() {
                    if changed_size {
                        app.resize_shape(&mut s, a);
                    } else {
                        s.apply_transform(a);
                    }
                    app.doc.replace(s);
                }
            }
        }
    });
}
