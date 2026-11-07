//! On-canvas gradient handles, Affinity style: the gradient line with its end
//! handles, a dot per colour stop (drag along the line) and a midpoint tick
//! between neighbouring stops (drag to bias the blend). They show for the
//! Fill tool, or for the Move tool while the fill editor is open. The Fill
//! tool also lays a new gradient by dragging across shapes.

use egui::{Color32, Pos2, Stroke};
use kurbo::{Affine, Point, Shape as _};

use crate::app::{App, Tool};
use crate::document::{Gradient, GradientKind, GradientStop, Paint, Rgba};

/// Part of the gradient overlay under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradPart {
    Start,
    End,
    /// Colour stop by index.
    Stop(usize),
    /// Midpoint between stop `i` and `i + 1`.
    Mid(usize),
    /// Mesh gradient point by index.
    MeshPoint(usize),
}

const HIT: f32 = 8.0;

impl App {
    /// Unit box → document for a shape (the space gradients live in).
    pub fn gradient_box(&mut self, id: u64) -> Option<Affine> {
        let s = self.doc.get(id)?.clone();
        let c = self.cache.update(&s, &mut self.fonts);
        let lb = crate::render::shape_local_bounds(&s, &c.doc_path);
        Some(s.transform * Affine::new([lb.width().max(1e-9), 0.0, 0.0, lb.height().max(1e-9), lb.x0, lb.y0]))
    }

    /// (shape id, unit box → document, gradient) when handles should show.
    pub fn gradient_frame(&mut self) -> Option<(u64, Affine, Gradient)> {
        if self.selection.len() != 1 {
            return None;
        }
        let id = self.selection[0];
        let on = match self.tool {
            Tool::Fill => true,
            Tool::Select => self.ui.show_studio && self.ui.gradient_edit == Some(id),
            _ => false,
        };
        if !on {
            return None;
        }
        let Some(Paint::Gradient(g)) = self.doc.get(id)?.fill.clone() else { return None };
        let m = self.gradient_box(id)?;
        Some((id, m, g))
    }

    /// Screen positions of the line ends.
    fn grad_ends(&self, m: Affine, g: &Gradient) -> (Pos2, Pos2) {
        let at = |u: [f64; 2]| self.view.to_screen(m * Point::new(u[0], u[1]));
        (at(g.start), at(g.end))
    }

    /// Screen position of each midpoint tick (between stops i and i+1).
    fn grad_mids(a: Pos2, b: Pos2, g: &Gradient) -> Vec<Pos2> {
        g.stops
            .windows(2)
            .map(|w| {
                let t = w[0].offset + (w[1].offset - w[0].offset) * w[0].mid;
                a + (b - a) * t.clamp(0.0, 1.0)
            })
            .collect()
    }

    /// Gradient overlay part under a screen position.
    pub fn gradient_handle_at(&mut self, sp: Pos2) -> Option<(u64, GradPart)> {
        let (id, m, g) = self.gradient_frame()?;
        if let Some(mesh) = &g.mesh.as_ref().filter(|_| g.kind == GradientKind::Mesh) {
            let at = |u: [f64; 2]| self.view.to_screen(m * Point::new(u[0], u[1]));
            return mesh.points.iter().enumerate().map(|(i, u)| (i, at(*u).distance(sp))).filter(|(_, d)| *d <= HIT).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| (id, GradPart::MeshPoint(i)));
        }
        let (a, b) = self.grad_ends(m, &g);
        if b.distance(sp) <= HIT {
            return Some((id, GradPart::End));
        }
        if a.distance(sp) <= HIT {
            return Some((id, GradPart::Start));
        }
        // Interior stops (the first and last sit under the end handles
        // unless moved inwards).
        for (i, st) in g.stops.iter().enumerate() {
            let p = a + (b - a) * st.offset.clamp(0.0, 1.0);
            if p.distance(sp) <= HIT - 2.0 && p.distance(a) > HIT && p.distance(b) > HIT {
                return Some((id, GradPart::Stop(i)));
            }
        }
        for (i, p) in Self::grad_mids(a, b, &g).into_iter().enumerate() {
            if p.distance(sp) <= HIT - 3.0 {
                return Some((id, GradPart::Mid(i)));
            }
        }
        None
    }

    /// Line parameter (0..1) of the gradient line closest to a screen point,
    /// and the distance from it in pixels.
    pub fn gradient_line_t(&mut self, sp: Pos2) -> Option<(u64, f32, f32)> {
        let (id, m, g) = self.gradient_frame()?;
        let (a, b) = self.grad_ends(m, &g);
        let d = b - a;
        let len2 = d.length_sq().max(1e-6);
        let t = ((sp - a).dot(d) / len2).clamp(0.0, 1.0);
        Some((id, t, (a + d * t).distance(sp)))
    }

    /// Drag a part of the gradient; returns the part's (possibly re-sorted) identity.
    pub fn drag_gradient(&mut self, id: u64, part: GradPart, p: Point, shift: bool) -> GradPart {
        let Some((_, m, g)) = self.gradient_frame() else { return part };
        let doc = |u: [f64; 2]| m * Point::new(u[0], u[1]);
        match part {
            GradPart::MeshPoint(i) => {
                let u = m.inverse() * p;
                self.edit_gradient(id, |g| {
                    if let Some(pt) = g.mesh.as_mut().and_then(|mg| mg.points.get_mut(i)) {
                        *pt = [u.x, u.y];
                    }
                });
                part
            }
            GradPart::Start | GradPart::End => {
                let end = part == GradPart::End;
                let p = if shift { crate::canvas::constrain_45(doc(if end { g.start } else { g.end }), p) } else { p };
                let u = m.inverse() * p;
                self.edit_gradient(id, |g| *(if end { &mut g.end } else { &mut g.start }) = [u.x, u.y]);
                part
            }
            GradPart::Stop(i) | GradPart::Mid(i) => {
                let (a, b) = (doc(g.start), doc(g.end));
                let d = b - a;
                let t = ((p - a).dot(d) / d.hypot2().max(1e-12)).clamp(0.0, 1.0) as f32;
                let mut out = part;
                let mut sel = None;
                self.edit_gradient(id, |g| {
                    if let GradPart::Stop(_) = part {
                        let Some(st) = g.stops.get_mut(i) else { return };
                        st.offset = t;
                        let key = *st;
                        g.sort();
                        let j = g.stops.iter().position(|s| *s == key).unwrap_or(i);
                        out = GradPart::Stop(j);
                        sel = Some(j);
                    } else if i + 1 < g.stops.len() {
                        let (o0, o1) = (g.stops[i].offset, g.stops[i + 1].offset);
                        if o1 - o0 > 1e-4 {
                            g.stops[i].mid = ((t - o0) / (o1 - o0)).clamp(0.05, 0.95);
                        }
                    }
                });
                if let Some(j) = sel {
                    self.ui.gradient_stop = j;
                }
                out
            }
        }
    }

    pub fn edit_gradient(&mut self, id: u64, f: impl FnOnce(&mut Gradient)) {
        if let Some(s) = self.doc.get_mut(id) {
            if let Some(Paint::Gradient(g)) = &mut s.fill {
                f(g);
            }
        }
    }

    /// Double-click on a mesh gradient: slice it there. On a row line a
    /// column is added through the point, on a column line a row, inside a
    /// cell both. True if handled.
    pub fn mesh_slice(&mut self, p: Point) -> bool {
        let Some((id, m, g)) = self.gradient_frame() else { return false };
        let (GradientKind::Mesh, Some(mesh)) = (g.kind, g.mesh.as_ref()) else { return false };
        let u = m.inverse() * p;
        let Some((gs, gt)) = mesh.grid_at([u.x, u.y]) else { return false };
        // Distance (screen px) from the nearest row and column lines.
        let to_screen = |q: [f64; 2]| self.view.to_screen(m * Point::new(q[0], q[1]));
        let (r0, c0) = (gt.floor(), gs.floor());
        let cell_h = to_screen(mesh.pos_at(gs, r0)).distance(to_screen(mesh.pos_at(gs, (r0 + 1.0).min((mesh.rows - 1) as f64))));
        let cell_w = to_screen(mesh.pos_at(c0, gt)).distance(to_screen(mesh.pos_at((c0 + 1.0).min((mesh.cols - 1) as f64), gt)));
        let on_row = (gt - gt.round()).abs() * cell_h as f64 <= HIT as f64;
        let on_col = (gs - gs.round()).abs() * cell_w as f64 <= HIT as f64;
        if on_row && on_col {
            return false;
        }
        self.checkpoint("Slice mesh");
        let mut point = None;
        self.edit_gradient(id, |g| {
            let Some(mesh) = g.mesh.as_mut() else { return };
            let (mut ci, mut ri) = (gs.round() as usize, gt.round() as usize);
            if !on_row && mesh.insert_row(gt) {
                ri = gt.floor() as usize + 1;
            }
            if !on_col && mesh.insert_column(gs) {
                ci = gs.floor() as usize + 1;
            }
            point = Some(ri * mesh.cols + ci);
        });
        if let Some(i) = point {
            self.ui.gradient_stop = i;
        }
        true
    }

    /// Fill tool, double-click on the line: add a stop there.
    pub fn gradient_add_stop(&mut self, sp: Pos2) -> bool {
        let Some((id, t, dist)) = self.gradient_line_t(sp) else { return false };
        if dist > HIT {
            return false;
        }
        self.checkpoint("Add gradient stop");
        let mut sel = 0;
        self.edit_gradient(id, |g| {
            let st = GradientStop::new(t, g.color_at(t));
            g.stops.push(st);
            g.sort();
            sel = g.stops.iter().position(|s| *s == st).unwrap_or(0);
        });
        self.ui.gradient_stop = sel;
        true
    }

    /// Fill tool, Delete: remove the selected stop (keeping two). True if handled.
    pub fn gradient_delete_stop(&mut self) -> bool {
        if self.tool != Tool::Fill {
            return false;
        }
        let Some((id, _, g)) = self.gradient_frame() else { return false };
        if g.stops.len() <= 2 {
            return true;
        }
        let i = self.ui.gradient_stop.min(g.stops.len() - 1);
        self.checkpoint("Remove gradient stop");
        self.edit_gradient(id, |g| {
            g.stops.remove(i);
        });
        self.ui.gradient_stop = i.saturating_sub(1);
        true
    }

    /// Fill tool drag: lay a gradient from `a` to `b` (document space) on
    /// every target. Existing gradients keep their stops and type; a solid
    /// fill fades from its colour to white; no fill becomes black to white.
    pub fn lay_gradient(&mut self, targets: &[u64], a: Point, b: Point) {
        for &id in targets {
            let Some(m) = self.gradient_box(id) else { continue };
            let inv = m.inverse();
            let (ua, ub) = (inv * a, inv * b);
            let Some(s) = self.doc.get_mut(id) else { continue };
            let mut g = match &s.fill {
                Some(Paint::Gradient(g)) => g.clone(),
                Some(Paint::Solid(c)) => Gradient::new(GradientKind::Linear, *c, Rgba::WHITE),
                _ => Gradient::new(GradientKind::Linear, Rgba::BLACK, Rgba::WHITE),
            };
            g.start = [ua.x, ua.y];
            g.end = [ub.x, ub.y];
            s.fill = Some(Paint::Gradient(g));
        }
    }

    pub fn paint_gradient_handles(&mut self, painter: &egui::Painter) {
        let Some((_, m, g)) = self.gradient_frame() else { return };
        let v = self.view;
        if let (GradientKind::Mesh, Some(mesh)) = (g.kind, &g.mesh) {
            // Grid lines and a dot per point, filled with its colour.
            let at = |u: [f64; 2]| v.to_screen(m * Point::new(u[0], u[1]));
            let shadow = Stroke::new(3.0, Color32::from_black_alpha(90));
            let white = Stroke::new(1.0, Color32::WHITE);
            for r in 0..mesh.rows {
                for c in 0..mesh.cols {
                    let p = at(mesh.points[r * mesh.cols + c]);
                    for q in [(c + 1 < mesh.cols).then(|| r * mesh.cols + c + 1), (r + 1 < mesh.rows).then(|| (r + 1) * mesh.cols + c)].into_iter().flatten() {
                        let q = at(mesh.points[q]);
                        painter.line_segment([p, q], shadow);
                        painter.line_segment([p, q], white);
                    }
                }
            }
            let sel = self.ui.gradient_stop;
            for (i, (u, col)) in mesh.points.iter().zip(&mesh.colors).enumerate() {
                let p = at(*u);
                painter.circle_filled(p, 7.0, Color32::from_black_alpha(90));
                painter.circle_filled(p, 6.0, if i == sel { crate::ui::theme::ACCENT } else { Color32::WHITE });
                painter.circle_filled(p, 4.0, col.to_color32(1.0));
            }
            return;
        }
        let (a, b) = self.grad_ends(m, &g);
        let shadow = Stroke::new(3.0, Color32::from_black_alpha(90));
        let white = Stroke::new(1.5, Color32::WHITE);
        if matches!(g.kind, GradientKind::Radial | GradientKind::Conic) {
            // The t = 1 ellipse (conic: the circle the sweep starts on).
            let r = ((g.end[0] - g.start[0]).powi(2) + (g.end[1] - g.start[1]).powi(2)).sqrt();
            let circle = kurbo::Circle::new(Point::new(g.start[0], g.start[1]), r).to_path(1e-3);
            let path = m * circle;
            crate::render::outline(painter, &path, &v, shadow);
            crate::render::outline(painter, &path, &v, white);
        }
        painter.line_segment([a, b], shadow);
        painter.line_segment([a, b], white);
        let sel = self.ui.gradient_stop;
        let accent = crate::ui::theme::ACCENT;
        // Midpoint ticks: small diamonds across the line.
        let dir = (b - a).normalized();
        let n = egui::vec2(-dir.y, dir.x);
        for p in Self::grad_mids(a, b, &g) {
            let pts = vec![p + n * 5.5, p + dir * 3.5, p - n * 5.5, p - dir * 3.5];
            painter.add(egui::Shape::convex_polygon(pts, Color32::WHITE, Stroke::new(1.5, Color32::from_black_alpha(150))));
        }
        // Stops along the line.
        let last = g.stops.len().saturating_sub(1);
        for (i, st) in g.stops.iter().enumerate() {
            let p = a + (b - a) * st.offset.clamp(0.0, 1.0);
            let r = if i == 0 || i == last { 4.5 } else { 5.0 };
            painter.circle_filled(p, r + 1.5, Color32::from_black_alpha(90));
            painter.circle_filled(p, r + 0.5, if i == sel { accent } else { Color32::WHITE });
            painter.circle_filled(p, r - 1.5, st.color.to_color32(1.0));
        }
        // End handles: hollow rings so the stop colours under them show.
        for p in [a, b] {
            painter.circle_stroke(p, 7.5, Stroke::new(3.5, Color32::from_black_alpha(90)));
            painter.circle_stroke(p, 7.5, Stroke::new(2.0, Color32::WHITE));
        }
    }
}
