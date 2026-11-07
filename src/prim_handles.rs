//! On-canvas parameter handles (orange dots, like Affinity's) for
//! primitives, stars and polygons: drag to change a star's points, inner
//! radius and corners, a cog's teeth, a spiral's turns…

use egui::{Color32, Pos2, Stroke};
use kurbo::Point;

use crate::app::{App, Tool};
use crate::document::{Shape, ShapeKind};
use crate::prims;

pub const PRIM_HANDLE: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x1F);
/// Screen distance that keeps inset handles off the box's edge handles.
const INSET_PX: f64 = 14.0;

fn has_handles(k: &ShapeKind) -> bool {
    matches!(k, ShapeKind::Prim { .. } | ShapeKind::Star { .. } | ShapeKind::Polygon { .. })
}

/// Local positions of a shape's parameter handles.
pub fn local_handles(kind: &ShapeKind, m: f64) -> Vec<Point> {
    match kind {
        ShapeKind::Prim { w, h, prim } => prims::handles(prim, *w, *h, m),
        ShapeKind::Star { w, h, points, inner, corner } => prims::radial_handles(*points, Some(*inner), *corner, *w, *h, m),
        ShapeKind::Polygon { w, h, sides, corner } => prims::radial_handles(*sides, None, *corner, *w, *h, m),
        _ => Vec::new(),
    }
}

/// Move handle `i` of a shape kind to local point `q`.
pub fn drag_local(kind: &mut ShapeKind, i: usize, q: Point, m: f64) {
    match kind {
        ShapeKind::Prim { w, h, prim } => prims::drag_handle(prim, i, q, *w, *h, m),
        ShapeKind::Star { w, h, points, inner, corner } => {
            let (n, inr, c) = prims::radial_drag(i, q, *points, Some(*inner), *corner, *w, *h, m);
            (*points, *inner, *corner) = (n, inr.unwrap_or(*inner), c);
        }
        ShapeKind::Polygon { w, h, sides, corner } => {
            let (n, _, c) = prims::radial_drag(i, q, *sides, None, *corner, *w, *h, m);
            (*sides, *corner) = (n, c);
        }
        _ => {}
    }
}

impl App {
    /// The single selected shape whose handles should show.
    fn prim_target(&self) -> Option<u64> {
        if self.selection.len() != 1 || !(self.tool == Tool::Select || self.tool.is_shape()) {
            return None;
        }
        let id = self.selection[0];
        let s = self.doc.get(id)?;
        (has_handles(&s.kind) && !s.locked && s.component.is_none()).then_some(id)
    }

    /// Inset in the shape's local units.
    fn handle_inset(&self, s: &Shape) -> f64 {
        let c = s.transform.as_coeffs();
        let scale = (c[2] * c[2] + c[3] * c[3]).sqrt().max(1e-9);
        INSET_PX / (self.view.zoom as f64 * scale)
    }

    /// Handle positions in screen space.
    fn prim_handle_points(&self) -> Option<(u64, Vec<Pos2>)> {
        let id = self.prim_target()?;
        let s = self.doc.get(id)?;
        let m = self.handle_inset(s);
        let pts = local_handles(&s.kind, m).into_iter().map(|q| self.view.to_screen(s.transform * q)).collect();
        Some((id, pts))
    }

    pub fn prim_handle_at(&self, sp: Pos2) -> Option<(u64, usize)> {
        let (id, pts) = self.prim_handle_points()?;
        // Nearest within reach (handles can sit close together).
        pts.iter()
            .enumerate()
            .map(|(i, p)| (i, p.distance(sp)))
            .filter(|(_, d)| *d <= 7.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| (id, i))
    }

    /// Move handle `i` of shape `id` to document point `p`.
    pub fn drag_prim_handle(&mut self, id: u64, i: usize, p: Point) {
        let Some(s) = self.doc.get(id) else { return };
        let m = self.handle_inset(s);
        let q = s.transform.inverse() * p;
        let mut s = s.clone();
        drag_local(&mut s.kind, i, q, m);
        // New shapes of this kind start from the edited values.
        match &s.kind {
            ShapeKind::Prim { prim, .. } => {
                self.prim_params.insert(prim.kind, prim.clone());
            }
            ShapeKind::Star { points, inner, .. } => {
                self.star_points = *points;
                self.star_inner = *inner;
            }
            ShapeKind::Polygon { sides, .. } => self.polygon_sides = *sides,
            _ => {}
        }
        self.doc.replace(s);
    }

    pub fn paint_prim_handles(&self, painter: &egui::Painter) {
        let Some((_, pts)) = self.prim_handle_points() else { return };
        let hot = self.cursor_doc.map(|c| self.view.to_screen(c)).and_then(|c| self.prim_handle_at(c)).map(|(_, i)| i);
        for (i, p) in pts.into_iter().enumerate() {
            let r = if hot == Some(i) { 5.5 } else { 4.5 };
            painter.circle_filled(p, r + 1.5, Color32::from_black_alpha(80));
            painter.circle_filled(p, r, PRIM_HANDLE);
            painter.circle_stroke(p, r, Stroke::new(1.5, Color32::WHITE));
        }
    }
}
