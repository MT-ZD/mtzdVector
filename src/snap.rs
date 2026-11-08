//! Snapping (artboard, shapes, path nodes, guides, grid), smart spacing,
//! Alt-hover measurements and their on-canvas indicators.

use std::collections::HashSet;

use egui::{Color32, Pos2, Stroke, pos2, vec2};
use kurbo::{Point, Rect, Vec2};

use crate::app::{App, SnapGuide};
use crate::document::ShapeKind;

pub const PINK: Color32 = Color32::from_rgb(0xFF, 0x4F, 0xA3);
pub const GUIDE: Color32 = Color32::from_rgb(0x2E, 0xC4, 0xE8);

/// A horizontal (`along_x`) or vertical gap between two boxes, for the
/// smart-spacing indicator.
#[derive(Clone, Debug)]
pub struct Gap {
    pub a: Rect,
    pub b: Rect,
    pub along_x: bool,
    /// Which side is the moving box (0 neither, 1 `a`, 2 `b`).
    pub moving: u8,
}

/// Everything a moving object can snap to.
pub struct Targets {
    /// Vertical lines (x) and horizontal lines (y), with the index of the
    /// box they belong to (None for guides).
    xs: Vec<(f64, Option<usize>)>,
    ys: Vec<(f64, Option<usize>)>,
    /// (bounds, is a shape)
    boxes: Vec<(Rect, bool)>,
    /// Shape boxes for smart spacing.
    shapes: Vec<Rect>,
    /// Key points: corners, centers, path nodes.
    points: Vec<Point>,
    grid: Option<f64>,
}

impl App {
    pub(crate) fn tol(&self) -> f64 {
        6.0 / self.view.zoom as f64
    }

    /// Ids that must not act as targets: `exclude`, their descendants and ancestors.
    fn excluded(&self, exclude: &[u64]) -> HashSet<u64> {
        let mut set = HashSet::new();
        for id in exclude {
            set.insert(*id);
            let mut p = self.doc.parent_of(*id);
            while let Some(x) = p {
                set.insert(x);
                p = self.doc.parent_of(x);
            }
            if let Some(s) = self.doc.get(*id) {
                fn walk(s: &crate::document::Shape, set: &mut HashSet<u64>) {
                    if let Some(ch) = s.kind.children() {
                        for c in ch {
                            set.insert(c.id);
                            walk(c, set);
                        }
                    }
                }
                walk(s, &mut set);
            }
        }
        set
    }

    pub fn snap_targets(&mut self, exclude: &[u64]) -> Targets {
        let mut t = Targets { xs: Vec::new(), ys: Vec::new(), boxes: Vec::new(), shapes: Vec::new(), points: Vec::new(), grid: None };
        let skip_ab = self.snap_artboard;
        let arts: Vec<Rect> = self.doc.artboards.iter().filter(|a| Some(a.id) != skip_ab).map(|a| a.rect()).collect();
        let add_box = |t: &mut Targets, r: Rect, is_shape: bool| {
            let i = t.boxes.len();
            t.boxes.push((r, is_shape));
            for x in [r.x0, r.center().x, r.x1] {
                t.xs.push((x, Some(i)));
            }
            for y in [r.y0, r.center().y, r.y1] {
                t.ys.push((y, Some(i)));
            }
            for p in [r.origin(), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1), r.center()] {
                t.points.push(p);
            }
        };
        for art in &arts {
            add_box(&mut t, *art, false);
        }
        let skip = self.excluded(exclude);
        // Walk the tree directly (no per-id lookups: this runs on every drag step).
        fn walk<S: std::borrow::Borrow<crate::document::Shape>>(list: &[S], top: bool, skip: &HashSet<u64>, cache: &mut crate::render::GeoCache, fonts: &mut crate::fonts::FontManager, t: &mut Targets, add_box: &dyn Fn(&mut Targets, Rect, bool)) {
            for s in list {
                let s = s.borrow();
                if !s.visible {
                    continue;
                }
                // Excluded: an ancestor of a moving shape still offers its other children.
                let skipped = skip.contains(&s.id);
                if !skipped {
                    if let Some(b) = cache.update(s, fonts).bounds {
                        add_box(t, b, true);
                        if top {
                            t.shapes.push(b);
                        }
                    }
                }
                match &s.kind {
                    _ if skipped && s.kind.children().is_none() => {}
                    ShapeKind::Path(pd) if !skipped => {
                        for sp in &pd.subpaths {
                            t.points.extend(sp.nodes.iter().take(500).map(|n| n.p));
                        }
                    }
                    // Children of booleans are construction geometry, not visible shapes.
                    ShapeKind::Compound { .. } => {}
                    k => {
                        if let Some(children) = k.children() {
                            walk(children, false, skip, cache, fonts, t, add_box);
                        }
                    }
                }
            }
        }
        walk(self.doc.shapes(), true, &skip, &mut self.cache, &mut self.fonts, &mut t, &add_box);
        if skip_ab.is_some() {
            // Moving an artboard: equal spacing between artboards.
            t.shapes = arts;
        }
        if self.ui.show_guides {
            for g in &self.doc.guides {
                if g.vertical {
                    t.xs.push((g.pos, None));
                } else {
                    t.ys.push((g.pos, None));
                }
            }
        }
        if self.ui.show_grid && self.doc.grid > 0.0 {
            t.grid = Some(self.doc.grid);
        }
        t
    }

    /// Snap the values (e.g. left/center/right of a box) along one axis.
    /// Returns (offset, line, target box). Shapes win ties with the artboard.
    fn best_axis(t: &Targets, vals: &[f64], lines: &[(f64, Option<usize>)], tol: f64) -> Option<(f64, f64, Option<usize>)> {
        let mut best: Option<(f64, f64, Option<usize>)> = None;
        let shape_rank = |i: Option<usize>| match i {
            None => 2,
            Some(i) if t.boxes[i].1 => 1,
            Some(_) => 0,
        };
        for &(line, ti) in lines {
            for &v in vals {
                let off = line - v;
                if off.abs() > tol {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((bo, _, bi)) => off.abs() < bo.abs() - 1e-9 || ((off.abs() - bo.abs()).abs() <= 1e-9 && shape_rank(ti) > shape_rank(bi)),
                };
                if better {
                    best = Some((off, line, ti));
                }
            }
        }
        if best.is_none() {
            if let Some(g) = t.grid {
                for &v in vals {
                    let line = (v / g).round() * g;
                    let off = line - v;
                    if off.abs() <= tol && best.map(|(bo, _, _)| off.abs() < bo.abs()).unwrap_or(true) {
                        best = Some((off, line, None));
                    }
                }
            }
        }
        best
    }

    fn push_guide(&mut self, t: &Targets, vertical: bool, line: f64, ti: Option<usize>, moving: Rect) {
        let (target, is_shape) = match ti {
            Some(i) => t.boxes[i],
            None => (moving, false),
        };
        self.snap_guides.push(SnapGuide { vertical, pos: line, target, moving, is_shape });
    }

    /// Snap a point: key points first (both axes), then lines per axis.
    pub fn snap_point(&mut self, p: Point, exclude: &[u64]) -> Point {
        self.snap_guides.clear();
        self.gap_guides.clear();
        if !self.snap {
            return p;
        }
        let t = self.snap_targets(exclude);
        let tol = self.tol();
        if let Some(q) = t.points.iter().filter(|q| (**q - p).hypot() <= tol).min_by(|a, b| (**a - p).hypot().total_cmp(&(**b - p).hypot())) {
            let r = Rect::from_points(*q, *q);
            self.snap_guides.push(SnapGuide { vertical: true, pos: q.x, target: r, moving: r, is_shape: false });
            self.snap_guides.push(SnapGuide { vertical: false, pos: q.y, target: r, moving: r, is_shape: false });
            return *q;
        }
        let bx = Self::best_axis(&t, &[p.x], &t.xs, tol);
        let by = Self::best_axis(&t, &[p.y], &t.ys, tol);
        let q = Point::new(p.x + bx.map(|b| b.0).unwrap_or(0.0), p.y + by.map(|b| b.0).unwrap_or(0.0));
        let r = Rect::from_points(q, q);
        if let Some((_, line, ti)) = bx {
            self.push_guide(&t, true, line, ti, r);
        }
        if let Some((_, line, ti)) = by {
            self.push_guide(&t, false, line, ti, r);
        }
        q
    }

    /// Snap the rotation pivot to geometry: path nodes and shape centres
    /// first (the selection's own too), then the nearest point on an
    /// outline, then the usual alignment lines.
    pub fn snap_pivot(&mut self, p: Point) -> Point {
        self.snap_guides.clear();
        self.gap_guides.clear();
        if !self.snap {
            return p;
        }
        let tol = self.tol();
        let mut nodes: Vec<Point> = Vec::new();
        let mut best_edge: Option<(f64, Point)> = None;
        for id in self.doc.all_ids() {
            let Some(s) = self.doc.get(id).cloned() else { continue };
            if !s.visible || s.kind.is_container() {
                continue;
            }
            let c = self.cache.update(&s, &mut self.fonts);
            let Some(b) = c.bounds else { continue };
            if !b.inflate(tol, tol).contains(p) {
                continue;
            }
            nodes.push(b.center());
            let path = c.doc_path.clone();
            for el in path.elements().iter().take(2000) {
                match el {
                    kurbo::PathEl::MoveTo(q) | kurbo::PathEl::LineTo(q) => nodes.push(*q),
                    kurbo::PathEl::QuadTo(_, q) => nodes.push(*q),
                    kurbo::PathEl::CurveTo(_, _, q) => nodes.push(*q),
                    kurbo::PathEl::ClosePath => {}
                }
            }
            for seg in path.segments().take(2000) {
                use kurbo::{ParamCurve, ParamCurveNearest};
                let n = seg.nearest(p, 1e-6);
                let d = n.distance_sq.sqrt();
                if d <= tol && best_edge.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best_edge = Some((d, seg.eval(n.t)));
                }
            }
        }
        let hit = nodes
            .iter()
            .map(|q| (*q, (*q - p).hypot()))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(q, _)| q)
            .or(best_edge.map(|(_, q)| q));
        if let Some(q) = hit {
            let r = Rect::from_points(q, q);
            self.snap_guides.push(SnapGuide { vertical: true, pos: q.x, target: r, moving: r, is_shape: false });
            self.snap_guides.push(SnapGuide { vertical: false, pos: q.y, target: r, moving: r, is_shape: false });
            return q;
        }
        self.snap_point(p, &[])
    }

    /// Snap a moved box (offset `d` from `m0`): edges and centers to lines,
    /// then smart spacing. Returns the adjusted offset.
    pub fn snap_box(&mut self, m0: Rect, d: Vec2, exclude: &[u64]) -> Vec2 {
        self.snap_guides.clear();
        self.gap_guides.clear();
        let t = self.snap_targets(exclude);
        let tol = self.tol();
        let m = m0 + d;
        let mut bx = Self::best_axis(&t, &[m.x0, m.center().x, m.x1], &t.xs, tol);
        let mut by = Self::best_axis(&t, &[m.y0, m.center().y, m.y1], &t.ys, tol);
        let mut out = d;
        // Smart spacing competes with line snaps; the closer one wins.
        // Smart spacing considers the shapes in view (as on screen).
        let view = Rect::new(self.view.to_doc(self.canvas_rect.min).x, self.view.to_doc(self.canvas_rect.min).y, self.view.to_doc(self.canvas_rect.max).x, self.view.to_doc(self.canvas_rect.max).y);
        let near: Vec<Rect> = if self.canvas_rect.is_positive() { t.shapes.iter().copied().filter(|r| r.overlaps(view) || r.contains(view.origin())).collect() } else { t.shapes.clone() };
        let sx = spacing(m, &near, true, tol);
        let sy = spacing(m, &near, false, tol);
        let mut gaps = Vec::new();
        match (sx, bx) {
            (Some((off, g)), b) if b.map(|b| off.abs() < b.0.abs() - 1e-9).unwrap_or(true) => {
                out.x += off;
                gaps.extend(g);
                bx = None;
            }
            (_, Some((off, _, _))) => out.x += off,
            _ => {}
        }
        match (sy, by) {
            (Some((off, g)), b) if b.map(|b| off.abs() < b.0.abs() - 1e-9).unwrap_or(true) => {
                out.y += off;
                gaps.extend(g);
                by = None;
            }
            (_, Some((off, _, _))) => out.y += off,
            _ => {}
        }
        let snapped = m0 + out;
        if let Some((_, line, ti)) = bx {
            self.push_guide(&t, true, line, ti, snapped);
        }
        if let Some((_, line, ti)) = by {
            self.push_guide(&t, false, line, ti, snapped);
        }
        // Gaps involving the moving box follow its final position.
        for g in &mut gaps {
            match g.moving {
                1 => g.a = snapped,
                2 => g.b = snapped,
                _ => {}
            }
        }
        self.gap_guides = gaps;
        out
    }
}

fn swap(r: Rect) -> Rect {
    Rect::new(r.y0, r.x0, r.y1, r.x1)
}

/// Smart spacing along x (`along_x`) or y: equal gaps to both neighbours, or
/// a gap that already exists between two other shapes.
pub fn spacing(m: Rect, others: &[Rect], along_x: bool, tol: f64) -> Option<(f64, Vec<Gap>)> {
    let tr = |r: Rect| if along_x { r } else { swap(r) };
    let m = tr(m);
    let others: Vec<Rect> = others.iter().map(|r| tr(*r)).collect();
    let overlap = |a: &Rect, b: &Rect| a.y0 < b.y1 && b.y0 < a.y1;
    let row: Vec<Rect> = others.iter().copied().filter(|r| overlap(r, &m)).collect();
    let w = m.width();
    let left = row.iter().copied().filter(|r| r.x1 <= m.x0 + tol).max_by(|a, b| a.x1.total_cmp(&b.x1));
    let right = row.iter().copied().filter(|r| r.x0 >= m.x1 - tol).min_by(|a, b| a.x0.total_cmp(&b.x0));
    // Existing gaps between neighbouring shapes in any row: each box's
    // nearest overlapping box to the right (a scan over boxes sorted by x0).
    let mut by_x = others.clone();
    by_x.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    let mut gaps: Vec<(f64, Rect, Rect)> = Vec::new();
    for a in &others {
        let start = by_x.partition_point(|b| b.x0 < a.x1);
        if let Some(b) = by_x[start..].iter().find(|b| overlap(a, b)) {
            let g = b.x0 - a.x1;
            if g > 0.5 {
                gaps.push((g, *a, *b));
            }
        }
    }
    let mut best: Option<(f64, Vec<(Rect, Rect, u8)>)> = None;
    let mut consider = |x0: f64, pairs: Vec<(Rect, Rect, u8)>| {
        let off = x0 - m.x0;
        if off.abs() <= tol && best.as_ref().map(|b| off.abs() < b.0.abs()).unwrap_or(true) {
            best = Some((off, pairs));
        }
    };
    let placed = |x0: f64| Rect::new(x0, m.y0, x0 + w, m.y1);
    if let (Some(l), Some(r)) = (left, right) {
        let x0 = (l.x1 + r.x0 - w) / 2.0;
        if x0 > l.x1 {
            consider(x0, vec![(l, placed(x0), 2), (placed(x0), r, 1)]);
        }
    }
    for (g, a, b) in &gaps {
        if let Some(l) = left {
            let x0 = l.x1 + g;
            consider(x0, vec![(l, placed(x0), 2), (*a, *b, 0)]);
        }
        if let Some(r) = right {
            let x0 = r.x0 - g - w;
            consider(x0, vec![(placed(x0), r, 1), (*a, *b, 0)]);
        }
    }
    let back = |r: Rect| if along_x { r } else { swap(r) };
    best.map(|(off, pairs)| (off, pairs.into_iter().map(|(a, b, moving)| Gap { a: back(a), b: back(b), along_x, moving }).collect()))
}

/// Pill label at a screen position.
pub fn label(painter: &egui::Painter, at: Pos2, text: &str, color: Color32) {
    let g = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(10.5), Color32::WHITE);
    let r = egui::Rect::from_center_size(at, g.size() + vec2(8.0, 3.0));
    painter.rect_filled(r, 3.0, color);
    painter.galley(r.center() - g.size() / 2.0, g, Color32::WHITE);
}

impl App {
    /// Smart-spacing indicators: the equal gaps with their size.
    pub fn paint_gaps(&self, painter: &egui::Painter) {
        let s = Stroke::new(1.0, PINK);
        for g in &self.gap_guides {
            let (a, b) = (self.view.rect_to_screen(g.a), self.view.rect_to_screen(g.b));
            if g.along_x {
                let y = (a.top().max(b.top()) + a.bottom().min(b.bottom())) / 2.0;
                let (x0, x1) = (a.right(), b.left());
                painter.hline(x0..=x1, y, s);
                painter.vline(x0, (y - 4.0)..=(y + 4.0), s);
                painter.vline(x1, (y - 4.0)..=(y + 4.0), s);
                label(painter, pos2((x0 + x1) / 2.0, y + 11.0), &crate::units::fmt(g.b.x0 - g.a.x1), PINK);
            } else {
                let x = (a.left().max(b.left()) + a.right().min(b.right())) / 2.0;
                let (y0, y1) = (a.bottom(), b.top());
                painter.vline(x, y0..=y1, s);
                painter.hline((x - 4.0)..=(x + 4.0), y0, s);
                painter.hline((x - 4.0)..=(x + 4.0), y1, s);
                label(painter, pos2(x + 22.0, (y0 + y1) / 2.0), &crate::units::fmt(g.b.y0 - g.a.y1), PINK);
            }
        }
    }

    /// Alt held: distances from the selection to the hovered shape or the artboard.
    pub fn paint_measure(&mut self, painter: &egui::Painter) {
        let Some(s) = self.selection_bounds() else { return };
        let hovered = self.hover.filter(|h| !self.selection.contains(h)).and_then(|h| self.cache.get(h)).and_then(|c| c.bounds);
        let t = hovered.unwrap_or(self.doc.artboard_for(s).unwrap_or(self.active()).rect());
        let red = Color32::from_rgb(0xF2, 0x48, 0x22);
        let st = Stroke::new(1.0, red);
        let v = self.view;
        painter.rect_stroke(v.rect_to_screen(t), 0.0, st, egui::StrokeKind::Middle);
        painter.rect_stroke(v.rect_to_screen(s), 0.0, Stroke::new(1.0, crate::ui::theme::ACCENT), egui::StrokeKind::Middle);
        let hline = |x0: f64, x1: f64, y: f64| {
            if (x1 - x0).abs() < 1e-6 {
                return;
            }
            let (a, b) = (v.to_screen(Point::new(x0, y)), v.to_screen(Point::new(x1, y)));
            painter.line_segment([a, b], st);
            label(painter, pos2((a.x + b.x) / 2.0, a.y - 10.0), &crate::units::fmt((x1 - x0).abs()), red);
        };
        let vline = |y0: f64, y1: f64, x: f64| {
            if (y1 - y0).abs() < 1e-6 {
                return;
            }
            let (a, b) = (v.to_screen(Point::new(x, y0)), v.to_screen(Point::new(x, y1)));
            painter.line_segment([a, b], st);
            label(painter, pos2(a.x + 4.0, (a.y + b.y) / 2.0), &crate::units::fmt((y1 - y0).abs()), red);
        };
        let (cx, cy) = (s.center().x, s.center().y);
        let contains = t.x0 <= s.x0 && t.x1 >= s.x1 && t.y0 <= s.y0 && t.y1 >= s.y1;
        if contains {
            hline(t.x0, s.x0, cy);
            hline(s.x1, t.x1, cy);
            vline(t.y0, s.y0, cx);
            vline(s.y1, t.y1, cx);
            return;
        }
        // Separate along an axis: the gap; overlapping: offsets of the near edges.
        let y = (s.y0.max(t.y0) + s.y1.min(t.y1)) / 2.0;
        let y = if s.y0.max(t.y0) < s.y1.min(t.y1) { y } else { cy };
        if s.x1 <= t.x0 {
            hline(s.x1, t.x0, y);
        } else if t.x1 <= s.x0 {
            hline(t.x1, s.x0, y);
        }
        let x = if s.x0.max(t.x0) < s.x1.min(t.x1) { (s.x0.max(t.x0) + s.x1.min(t.x1)) / 2.0 } else { cx };
        if s.y1 <= t.y0 {
            vline(s.y1, t.y0, x);
        } else if t.y1 <= s.y0 {
            vline(t.y1, s.y0, x);
        }
    }

    /// Guides across the canvas (and the grid underneath).
    pub fn paint_guides(&self, painter: &egui::Painter, area: egui::Rect) {
        if self.ui.show_grid && self.doc.grid > 0.0 {
            let mut g = self.doc.grid;
            while g * (self.view.zoom as f64) < 8.0 {
                g *= 2.0;
            }
            let c = Color32::from_rgba_unmultiplied(0x80, 0x80, 0x80, 40);
            let a = self.view.to_doc(area.min);
            let b = self.view.to_doc(area.max);
            // Inside each artboard (grid lines are document-wide multiples).
            for ab in &self.doc.artboards {
                let r = ab.rect();
                let (x0, x1) = (a.x.max(r.x0), b.x.min(r.x1));
                let (y0, y1) = (a.y.max(r.y0), b.y.min(r.y1));
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                let top = self.view.to_screen(Point::new(0.0, y0)).y..=self.view.to_screen(Point::new(0.0, y1)).y;
                let left = self.view.to_screen(Point::new(x0, 0.0)).x..=self.view.to_screen(Point::new(x1, 0.0)).x;
                let mut x = (x0 / g).ceil() * g;
                while x <= x1 {
                    painter.vline(self.view.to_screen(Point::new(x, 0.0)).x, top.clone(), Stroke::new(1.0, c));
                    x += g;
                }
                let mut y = (y0 / g).ceil() * g;
                while y <= y1 {
                    painter.hline(left.clone(), self.view.to_screen(Point::new(0.0, y)).y, Stroke::new(1.0, c));
                    y += g;
                }
            }
        }
        if !self.ui.show_guides {
            return;
        }
        let line = |vertical: bool, pos: f64, s: Stroke| {
            if vertical {
                painter.vline(self.view.to_screen(Point::new(pos, 0.0)).x, area.y_range(), s);
            } else {
                painter.hline(area.x_range(), self.view.to_screen(Point::new(0.0, pos)).y, s);
            }
        };
        for (i, g) in self.doc.guides.iter().enumerate() {
            if self.guide_drag.map(|d| d.0 == Some(i)).unwrap_or(false) {
                continue;
            }
            line(g.vertical, g.pos, Stroke::new(if self.guide_hover == Some(i) { 2.0 } else { 1.0 }, GUIDE));
        }
        if let Some((_, vertical, pos)) = self.guide_drag {
            line(vertical, pos, Stroke::new(1.5, GUIDE));
            let sp = self.view.to_screen(Point::new(pos, pos));
            let at = if vertical { pos2(sp.x + 4.0, area.top() + 14.0) } else { pos2(area.left() + 30.0, sp.y - 10.0) };
            label(painter, at, &crate::units::fmt(pos), GUIDE);
        }
    }

    /// Guide under a screen position.
    pub fn guide_at(&self, sp: Pos2) -> Option<usize> {
        if !self.ui.show_guides {
            return None;
        }
        self.doc.guides.iter().position(|g| {
            let s = self.view.to_screen(Point::new(g.pos, g.pos));
            if g.vertical { (s.x - sp.x).abs() <= 4.0 } else { (s.y - sp.y).abs() <= 4.0 }
        })
    }
}

impl App {
    /// Guide position for a pointer: snapped to shape edges/centers, else
    /// rounded to whole pixels.
    fn guide_pos(&mut self, vertical: bool, sp: Pos2) -> f64 {
        let d = self.view.to_doc(sp);
        let v = if vertical { d.x } else { d.y };
        if self.snap {
            let t = self.snap_targets(&[]);
            let lines: Vec<(f64, Option<usize>)> = (if vertical { &t.xs } else { &t.ys }).iter().copied().filter(|l| l.1.is_some()).collect();
            if let Some((off, _, _)) = Self::best_axis(&t, &[v], &lines, self.tol()) {
                return v + off;
            }
        }
        if self.view.zoom >= 4.0 { (v * 10.0).round() / 10.0 } else { v.round() }
    }

    /// Start dragging a new guide out of a ruler.
    pub fn begin_new_guide(&mut self, vertical: bool, sp: Pos2) {
        let pos = self.guide_pos(vertical, sp);
        self.ui.show_guides = true;
        self.guide_drag = Some((None, vertical, pos));
    }

    /// Drive an active guide drag. Returns true while it owns the pointer.
    /// Releasing outside the canvas (e.g. back on the ruler) removes the guide.
    pub fn guide_tick(&mut self, ctx: &egui::Context) -> bool {
        let Some((idx, vertical, _)) = self.guide_drag else { return false };
        let (down, pos) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
        ctx.set_cursor_icon(if vertical { egui::CursorIcon::ResizeColumn } else { egui::CursorIcon::ResizeRow });
        let Some(sp) = pos else { return true };
        if down {
            let p = self.guide_pos(vertical, sp);
            self.guide_drag = Some((idx, vertical, p));
            return true;
        }
        let (_, _, p) = self.guide_drag.take().unwrap();
        let inside = self.canvas_rect.contains(sp);
        match (idx, inside) {
            (Some(i), true) => {
                if self.doc.guides.get(i).map(|g| g.pos != p).unwrap_or(false) {
                    self.checkpoint("");
                    self.doc.guides[i].pos = p;
                }
            }
            (Some(i), false) => {
                if i < self.doc.guides.len() {
                    self.checkpoint("");
                    self.doc.guides.remove(i);
                }
            }
            (None, true) => {
                self.checkpoint("");
                self.ui.show_guides = true;
                self.doc.guides.push(crate::document::Guide { vertical, pos: p });
            }
            (None, false) => {}
        }
        true
    }
}
