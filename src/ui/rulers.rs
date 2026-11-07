//! Top and left rulers with cursor marker and selection extent.

use egui::{Align2, Pos2, Rect, Stroke, Ui, pos2, vec2};

use super::theme::{self, ACCENT, RULER, TEXT_DIM};
use crate::app::App;

/// Canvas area left after reserving the rulers.
pub fn canvas_rect(full: Rect, rulers: bool) -> Rect {
    if rulers { Rect::from_min_max(full.min + vec2(RULER, RULER), full.max) } else { full }
}

/// Pick a "nice" major tick step (1/2/5 × 10^k document units) so labels are
/// at least `min_px` apart.
pub fn tick_step(zoom: f32, min_px: f32) -> f64 {
    let raw = (min_px / zoom.max(1e-6)) as f64;
    let k = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * k >= raw {
            return m * k;
        }
    }
    10.0 * k
}

fn fmt(v: f64) -> String {
    if v.abs() >= 1.0 || v == 0.0 { format!("{:.0}", v) } else { format!("{:.2}", v) }
}

/// Paint the rulers over the canvas area `full` (call after the canvas).
pub fn show(app: &mut App, ui: &mut Ui, full: Rect) {
    let view = app.view;
    let top = Rect::from_min_max(pos2(full.left() + RULER, full.top()), pos2(full.right(), full.top() + RULER));
    let left = Rect::from_min_max(pos2(full.left(), full.top() + RULER), pos2(full.left() + RULER, full.bottom()));
    let corner = Rect::from_min_size(full.min, vec2(RULER, RULER));
    // Drag out of a ruler to create a guide (top ruler: horizontal).
    for (r, vertical, id) in [(top, false, "ruler_top"), (left, true, "ruler_left")] {
        let resp = ui.interact(r, ui.id().with(id), egui::Sense::drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeColumn } else { egui::CursorIcon::ResizeRow });
        }
        if resp.drag_started() && app.guide_drag.is_none() {
            if let Some(p) = resp.interact_pointer_pos() {
                app.begin_new_guide(vertical, p);
            }
        }
    }
    let painter = ui.painter().with_clip_rect(full);
    painter.rect_filled(top, 0.0, theme::BG_HEADER);
    painter.rect_filled(left, 0.0, theme::BG_HEADER);
    painter.rect_filled(corner, 0.0, theme::BG_HEADER);
    painter.hline(full.x_range(), top.bottom() - 0.5, Stroke::new(1.0, theme::SEPARATOR));
    painter.vline(left.right() - 0.5, full.y_range(), Stroke::new(1.0, theme::SEPARATOR));
    painter.text(corner.center(), Align2::CENTER_CENTER, crate::units::current().0.suffix(), egui::FontId::proportional(8.5), theme::TEXT_FAINT);

    // Selection extent.
    if let Some(b) = app.selection_bounds() {
        let r = view.rect_to_screen(b);
        let tx = Rect::from_x_y_ranges(r.left().max(top.left())..=r.right().min(top.right()), top.y_range());
        let ly = Rect::from_x_y_ranges(left.x_range(), r.top().max(left.top())..=r.bottom().min(left.bottom()));
        if tx.width() > 0.0 {
            painter.rect_filled(tx, 0.0, ACCENT.gamma_multiply(0.22));
        }
        if ly.height() > 0.0 {
            painter.rect_filled(ly, 0.0, ACCENT.gamma_multiply(0.22));
        }
    }

    // Ticks are laid out in the document unit (`k` units per px), measured
    // from the active artboard's top-left corner.
    let k = crate::units::factor();
    let (ox, oy) = (app.active().x, app.active().y);
    let uzoom = view.zoom as f64 / k;
    let step = tick_step(uzoom as f32, 64.0);
    let minor = step / 10.0;
    let minor = if minor * uzoom >= 5.0 { minor } else { step / 2.0 };
    let font = egui::FontId::proportional(9.5);
    let tick = Stroke::new(1.0, theme::TEXT_FAINT);

    // Horizontal ruler.
    let x0 = (view.to_doc(pos2(top.left(), 0.0)).x - ox) * k;
    let x1 = (view.to_doc(pos2(top.right(), 0.0)).x - ox) * k;
    let mut v = (x0 / minor).floor() * minor;
    while v <= x1 {
        let sx = view.to_screen(kurbo::Point::new(v / k + ox, 0.0)).x;
        let major = ((v / step).round() * step - v).abs() < minor * 0.01;
        let h = if major { RULER - 2.0 } else { 4.0 };
        painter.vline(sx, (top.bottom() - h)..=top.bottom(), tick);
        if major {
            painter.text(pos2(sx + 3.0, top.top() + 1.0), Align2::LEFT_TOP, fmt(v), font.clone(), TEXT_DIM);
        }
        v += minor;
    }
    // Vertical ruler (labels drawn rotated as stacked digits would be noisy:
    // draw small horizontal labels instead).
    let y0 = (view.to_doc(pos2(0.0, left.top())).y - oy) * k;
    let y1 = (view.to_doc(pos2(0.0, left.bottom())).y - oy) * k;
    let mut v = (y0 / minor).floor() * minor;
    while v <= y1 {
        let sy = view.to_screen(kurbo::Point::new(0.0, v / k + oy)).y;
        let major = ((v / step).round() * step - v).abs() < minor * 0.01;
        let w = if major { RULER - 2.0 } else { 4.0 };
        painter.hline((left.right() - w)..=left.right(), sy, tick);
        if major {
            let g = painter.layout_no_wrap(fmt(v), font.clone(), TEXT_DIM);
            let shape = egui::epaint::TextShape::new(pos2(left.left() + 2.0, sy - 3.0), g, TEXT_DIM).with_angle(-std::f32::consts::FRAC_PI_2);
            painter.add(shape);
        }
        v += minor;
    }

    // Cursor markers.
    if let Some(p) = app.cursor_doc {
        let s: Pos2 = view.to_screen(p);
        if top.x_range().contains(s.x) {
            painter.vline(s.x, top.y_range(), Stroke::new(1.0, ACCENT));
        }
        if left.y_range().contains(s.y) {
            painter.hline(left.x_range(), s.y, Stroke::new(1.0, ACCENT));
        }
    }
}
