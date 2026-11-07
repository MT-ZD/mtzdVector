//! Gradient editor for the Color tab: type, stop bar (click to add, drag to
//! move, drag off to remove), and the selected stop's position and colour.

use egui::{Color32, CornerRadius, Id, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::color;
use crate::document::{Gradient, GradientKind, GradientStop};
use crate::ui::theme::{self, ACCENT, TEXT_DIM};
use crate::ui::widgets::{self, Num};

/// Sort stops keeping track of the selected one.
fn sort_keep(g: &mut Gradient, sel: usize) -> usize {
    let key = g.stops.get(sel).copied();
    g.sort();
    key.and_then(|k| g.stops.iter().position(|s| *s == k)).unwrap_or(0)
}

pub fn editor(app: &mut crate::app::App, ui: &mut Ui, g: &mut Gradient) {
    let mut sel = app.ui.gradient_stop.min(g.stops.len().saturating_sub(1));

    let mut kind = g.kind;
    widgets::segmented_width(ui, &mut kind, &[(GradientKind::Linear, "Linear"), (GradientKind::Radial, "Radial"), (GradientKind::Conic, "Conic"), (GradientKind::Mesh, "Mesh")], ui.available_width());
    if kind != g.kind {
        g.set_kind(kind);
        app.ui.gradient_stop = 0;
    }
    ui.add_space(4.0);
    if g.kind == GradientKind::Mesh {
        mesh_editor(app, ui, g);
        return;
    }
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = widgets::icon_button_sized(ui, icon::ARROW_CLOCKWISE, true, false, vec2(24.0, 22.0), 14.0);
            if widgets::tip(r, "Rotate 90°", None).clicked() {
                // About the box centre.
                let rot = |p: [f64; 2]| [0.5 - (p[1] - 0.5), 0.5 + (p[0] - 0.5)];
                g.start = rot(g.start);
                g.end = rot(g.end);
            }
            let r = widgets::icon_button_sized(ui, icon::ARROWS_LEFT_RIGHT, true, false, vec2(24.0, 22.0), 14.0);
            if widgets::tip(r, "Reverse", None).clicked() {
                for s in &mut g.stops {
                    s.offset = 1.0 - s.offset;
                }
                sel = sort_keep(g, sel);
            }
        });
    });
    ui.add_space(6.0);

    // Stop bar.
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
    let bar_rect = egui::Rect::from_min_size(rect.min + vec2(7.0, 0.0), vec2(w - 14.0, 18.0));
    let resp = ui.interact(bar_rect.with_max_y(bar_rect.bottom() - 7.0), ui.id().with("grad_bar_add"), Sense::click());
    let bar = egui::Rect::from_min_size(rect.min + vec2(7.0, 0.0), vec2(w - 14.0, 18.0));
    let painter = ui.painter();
    widgets::ramp_strip(painter, bar, g);
    painter.rect_stroke(bar, CornerRadius::same(2), Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Outside);
    let x_of = |t: f32| bar.left() + bar.width() * t.clamp(0.0, 1.0);
    let t_of = |x: f32| ((x - bar.left()) / bar.width()).clamp(0.0, 1.0);
    // Midpoint ticks between stops (drag to bias the blend).
    let base_id = ui.id().with("grad_bar");
    for i in 0..g.stops.len().saturating_sub(1) {
        let (a, b) = (g.stops[i], g.stops[i + 1]);
        if b.offset - a.offset < 0.02 {
            continue;
        }
        let c = pos2(x_of(a.offset + (b.offset - a.offset) * a.mid), bar.bottom());
        let hr = egui::Rect::from_min_max(c - vec2(5.0, 7.0), c + vec2(5.0, 2.0));
        let r = ui.interact(hr, base_id.with(("mid", i)), Sense::drag());
        if r.dragged() {
            if let Some(p) = r.interact_pointer_pos() {
                g.stops[i].mid = ((t_of(p.x) - a.offset) / (b.offset - a.offset)).clamp(0.05, 0.95);
            }
        }
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let k = if r.hovered() || r.dragged() { 5.0 } else { 4.0 };
        let pts = vec![c + vec2(0.0, -k), c + vec2(k * 0.7, 0.0), c + vec2(0.0, k), c + vec2(-k * 0.7, 0.0)];
        ui.painter().add(egui::Shape::convex_polygon(pts, Color32::WHITE, Stroke::new(1.0, Color32::from_black_alpha(160))));
    }
    // Stop markers: one strip picks the nearest marker, so overlapping
    // stops (and the ends) stay selectable and drags survive re-sorting.
    let mut remove = None;
    let strip = egui::Rect::from_min_max(pos2(rect.left(), bar.bottom() + 2.0), rect.max);
    let sresp = ui.interact(strip, base_id.with("markers"), Sense::click_and_drag());
    let nearest = |x: f32, g: &Gradient| {
        g.stops
            .iter()
            .enumerate()
            .map(|(i, st)| (i, (x_of(st.offset) - x).abs()))
            .filter(|(_, d)| *d <= 9.0)
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
    };
    let hover_i = sresp.hover_pos().and_then(|p| nearest(p.x, g));
    let grab_key = base_id.with("grab");
    if sresp.drag_started() || sresp.clicked() {
        if let Some(i) = sresp.interact_pointer_pos().and_then(|p| nearest(p.x, g)) {
            sel = i;
            ui.data_mut(|d| d.insert_temp(grab_key, true));
        } else {
            ui.data_mut(|d| d.insert_temp(grab_key, false));
        }
    }
    let grabbing = sresp.dragged() && ui.data(|d| d.get_temp::<bool>(grab_key).unwrap_or(false));
    if grabbing && sresp.drag_delta() != egui::Vec2::ZERO {
        if let (Some(p), true) = (sresp.interact_pointer_pos(), sel < g.stops.len()) {
            g.stops[sel].offset = t_of(p.x);
            // Pulled well below the bar: remove (keeping at least two stops).
            if p.y > bar.bottom() + 40.0 && g.stops.len() > 2 {
                remove = Some(sel);
            }
        }
    }
    if hover_i.is_some() || grabbing {
        ui.ctx().set_cursor_icon(if grabbing { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    }
    let mut order: Vec<usize> = (0..g.stops.len()).filter(|i| *i != sel).collect();
    if sel < g.stops.len() {
        order.push(sel); // selected on top
    }
    for i in order {
        let st = g.stops[i];
        let c = pos2(x_of(st.offset), bar.bottom() + 8.0);
        let mr = egui::Rect::from_center_size(c, vec2(12.0, 12.0));
        let painter = ui.painter();
        // Pointer notch + swatch.
        painter.line_segment([pos2(c.x, bar.bottom()), pos2(c.x, mr.top())], Stroke::new(1.0, Color32::WHITE));
        painter.rect_filled(mr, CornerRadius::same(3), Color32::WHITE);
        let inner = mr.shrink(2.0);
        if st.color.a < 1.0 {
            color::checkerboard(&painter.with_clip_rect(inner), inner, 3.0);
        }
        painter.rect_filled(inner, CornerRadius::same(2), st.color.to_color32(1.0));
        let ring = if i == sel {
            Stroke::new(2.0, ACCENT)
        } else if hover_i == Some(i) {
            Stroke::new(1.5, Color32::WHITE)
        } else {
            Stroke::new(1.0, Color32::from_black_alpha(160))
        };
        painter.rect_stroke(mr, CornerRadius::same(3), ring, StrokeKind::Outside);
    }
    if let Some(i) = remove {
        g.stops.remove(i);
        sel = sel.min(g.stops.len() - 1);
    } else if g.stops.windows(2).any(|w| w[0].offset > w[1].offset) {
        sel = sort_keep(g, sel);
    }
    // Click on the bar: add a stop with the colour already there.
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = t_of(p.x);
            g.stops.push(GradientStop::new(t, g.color_at(t)));
            sel = g.stops.len() - 1;
            sel = sort_keep(g, sel);
        }
    }
    widgets::tip(resp, "Click to add a stop · drag a stop down to remove it", None);

    // Selected stop.
    let n = g.stops.len();
    if let Some(st) = g.stops.get_mut(sel) {
        ui.horizontal(|ui| {
            let mut pos = st.offset as f64 * 100.0;
            if widgets::num_field(ui, "Stop", &mut pos, Num::default().range(0.0, 100.0).suffix("%").decimals(0), 96.0).changed() {
                st.offset = (pos / 100.0) as f32;
            }
            if sel + 1 < n {
                let mut mid = st.mid as f64 * 100.0;
                let r = widgets::num_field(ui, "Mid", &mut mid, Num::default().range(5.0, 95.0).suffix("%").decimals(0), 84.0);
                if widgets::tip(r, "Blend midpoint towards the next stop", None).changed() {
                    st.mid = (mid / 100.0) as f32;
                }
            } else {
                ui.label(RichText::new(format!("{} of {}", sel + 1, n)).size(theme::FONT_SM).color(TEXT_DIM));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = n > 2;
                let r = widgets::icon_button_sized(ui, icon::TRASH, can, false, vec2(24.0, 22.0), 14.0);
                if widgets::tip(r, "Remove stop", None).clicked() && can {
                    remove = Some(sel);
                }
            });
        });
    }
    if let Some(i) = remove.take() {
        if g.stops.len() > 2 {
            g.stops.remove(i);
            sel = sel.min(g.stops.len() - 1);
        }
    }
    sel = sort_keep(g, sel);
    ui.add_space(4.0);
    if let Some(st) = g.stops.get_mut(sel) {
        super::design::picker(app, ui, &mut st.color, Id::new(("grad_stop_picker", sel)));
    }
    app.ui.gradient_stop = sel;
}

/// Mesh gradient: grid size and the selected point's colour (points are
/// dragged on the canvas).
fn mesh_editor(app: &mut crate::app::App, ui: &mut Ui, g: &mut Gradient) {
    let Some(mesh) = g.mesh.as_mut() else { return };
    let (mut cols, mut rows) = (mesh.cols as f64, mesh.rows as f64);
    ui.horizontal(|ui| {
        let a = widgets::num_field(ui, "Columns", &mut cols, Num::default().range(2.0, 8.0).decimals(0), 100.0).changed();
        let b = widgets::num_field(ui, "Rows", &mut rows, Num::default().range(2.0, 8.0).decimals(0), 84.0).changed();
        if a || b {
            // Resample the colours onto the new grid.
            let old = mesh.clone();
            *mesh = crate::document::MeshGrid::new(cols as usize, rows as usize, |u, v| old.color_at([u, v]));
            app.ui.gradient_stop = 0;
        }
    });
    let n = mesh.points.len();
    let sel = app.ui.gradient_stop.min(n.saturating_sub(1));
    ui.label(RichText::new(format!("Point {} of {n} · drag points on the canvas, click one to colour it, double-click to slice", sel + 1)).size(theme::FONT_SM).color(TEXT_DIM));
    ui.add_space(4.0);
    if let Some(c) = mesh.colors.get_mut(sel) {
        super::design::picker(app, ui, c, Id::new(("mesh_point_picker", sel)));
    }
    app.ui.gradient_stop = sel;
}

