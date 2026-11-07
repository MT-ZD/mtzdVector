//! Auto layout (groups that stack their members in a row or column with a
//! gap and padding, like Figma's) and resize constraints (how a member
//! follows when its group is resized).

use std::collections::HashMap;

use kurbo::{Affine, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::document::{Shape, ShapeKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    #[default]
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// A group's auto layout. Members flow in layer order (bottom layer first);
/// `background` (a rectangle member) is sized to the content plus padding
/// and marks the layout's top-left corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoLayout {
    pub dir: Direction,
    pub gap: f64,
    /// Top, right, bottom, left.
    pub padding: [f64; 4],
    /// Cross-axis alignment.
    pub align: Align,
    /// Id of the background rectangle member.
    #[serde(default)]
    pub background: Option<u64>,
}

impl Default for AutoLayout {
    fn default() -> Self {
        AutoLayout { dir: Direction::Row, gap: 12.0, padding: [12.0; 4], align: Align::Center, background: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pin {
    /// Keeps its distance to the left (top) edge.
    Start,
    /// Keeps its distance to the right (bottom) edge.
    End,
    /// Keeps both distances: stretches.
    Both,
    /// Keeps its offset from the centre.
    Center,
    /// Scales with the group (the default).
    #[default]
    Scale,
}

impl Pin {
    pub const ALL: [Pin; 5] = [Pin::Start, Pin::End, Pin::Both, Pin::Center, Pin::Scale];

    pub fn label(self, horizontal: bool) -> &'static str {
        match (self, horizontal) {
            (Pin::Start, true) => "Left",
            (Pin::End, true) => "Right",
            (Pin::Both, true) => "Left & Right",
            (Pin::Start, false) => "Top",
            (Pin::End, false) => "Bottom",
            (Pin::Both, false) => "Top & Bottom",
            (Pin::Center, _) => "Center",
            (Pin::Scale, _) => "Scale",
        }
    }
}

/// How a group member follows when the group is resized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraints {
    pub h: Pin,
    pub v: Pin,
}

/// One interval after a resize, by its pin. `a0..a1` is the member's span
/// inside the group's `b0..b1`, which became `n0..n1`.
fn pin_span(pin: Pin, (a0, a1): (f64, f64), (b0, b1): (f64, f64), (n0, n1): (f64, f64)) -> (f64, f64) {
    let w = a1 - a0;
    match pin {
        Pin::Start => (n0 + (a0 - b0), n0 + (a0 - b0) + w),
        Pin::End => (n1 - (b1 - a1) - w, n1 - (b1 - a1)),
        Pin::Both => (n0 + (a0 - b0), (n1 - (b1 - a1)).max(n0 + (a0 - b0) + 1e-3)),
        Pin::Center => {
            let c = (n0 + n1) / 2.0 + ((a0 + a1) / 2.0 - (b0 + b1) / 2.0);
            (c - w / 2.0, c + w / 2.0)
        }
        Pin::Scale => {
            let k = if (b1 - b0).abs() > 1e-9 { (n1 - n0) / (b1 - b0) } else { 1.0 };
            (n0 + (a0 - b0) * k, n0 + (a1 - b0) * k)
        }
    }
}

/// Whether `t` only scales and translates (no rotation or skew).
fn axis_aligned(t: Affine) -> bool {
    let c = t.as_coeffs();
    c[1].abs() < 1e-9 && c[2].abs() < 1e-9 && c[0] > 0.0 && c[3] > 0.0
}

/// Resize `s` by `t` (a scale and translation). A group whose members have
/// constraints moves and sizes each member by them instead of scaling it;
/// `bounds_of` gives a shape's geometric bounds. Returns false when `t`
/// isn't axis-aligned or nothing is constrained (apply `t` as usual then).
pub fn resize_constrained(s: &mut Shape, t: Affine, bounds_of: &mut dyn FnMut(&Shape) -> Option<Rect>) -> bool {
    if !axis_aligned(t) || s.layout.is_some() {
        return false;
    }
    let Some(children) = s.kind.children() else { return false };
    if !children.iter().any(|c| c.constraints.is_some_and(|k| k != Constraints::default())) {
        return false;
    }
    let Some(b) = bounds_of(s) else { return false };
    let nb = t.transform_rect_bbox(b);
    let ShapeKind::Group { children } = &mut s.kind else { return false };
    for c in children.iter_mut() {
        let k = c.constraints.unwrap_or_default();
        let Some(cb) = bounds_of(c) else {
            c.apply_transform(t);
            continue;
        };
        let (x0, x1) = pin_span(k.h, (cb.x0, cb.x1), (b.x0, b.x1), (nb.x0, nb.x1));
        let (y0, y1) = pin_span(k.v, (cb.y0, cb.y1), (b.y0, b.y1), (nb.y0, nb.y1));
        let sx = if cb.width() > 1e-9 { (x1 - x0) / cb.width() } else { 1.0 };
        let sy = if cb.height() > 1e-9 { (y1 - y0) / cb.height() } else { 1.0 };
        let m = Affine::translate((x0, y0)) * Affine::scale_non_uniform(sx, sy) * Affine::translate((-cb.x0, -cb.y0));
        c.apply_transform(m);
    }
    true
}

/// Where each member goes: (member id, translation), and the background's
/// new rect. `members` are (id, bounds) in flow order.
pub fn arrange(l: &AutoLayout, origin: kurbo::Point, members: &[(u64, Rect)]) -> (Vec<(u64, Vec2)>, Rect) {
    let [pt, pr, pb, pl] = l.padding;
    let cross = members.iter().map(|(_, r)| if l.dir == Direction::Row { r.height() } else { r.width() }).fold(0.0, f64::max);
    let mut cursor = if l.dir == Direction::Row { origin.x + pl } else { origin.y + pt };
    let mut moves = Vec::new();
    for (id, r) in members {
        let (main_len, cross_len) = if l.dir == Direction::Row { (r.width(), r.height()) } else { (r.height(), r.width()) };
        let off = match l.align {
            Align::Start => 0.0,
            Align::Center => (cross - cross_len) / 2.0,
            Align::End => cross - cross_len,
        };
        let target = if l.dir == Direction::Row { kurbo::Point::new(cursor, origin.y + pt + off) } else { kurbo::Point::new(origin.x + pl + off, cursor) };
        moves.push((*id, target - r.origin()));
        cursor += main_len + l.gap;
    }
    let content = if members.is_empty() { 0.0 } else { cursor - l.gap - if l.dir == Direction::Row { origin.x + pl } else { origin.y + pt } };
    let (w, h) = if l.dir == Direction::Row { (pl + content + pr, pt + cross + pb) } else { (pl + cross + pr, pt + content + pb) };
    (moves, Rect::new(origin.x, origin.y, origin.x + w, origin.y + h))
}

/// Geometric bounds of `s` (groups: their members').
pub fn geo_bounds(s: &Shape, fonts: &mut crate::fonts::FontManager) -> Option<Rect> {
    match s.kind.children() {
        Some(c) if matches!(s.kind, ShapeKind::Group { .. }) => c.iter().filter_map(|m| geo_bounds(m, fonts)).reduce(|a, b| a.union(b)),
        _ => crate::geom::path_bounds(&crate::render::compute_outline(s, fonts).0),
    }
}

impl App {
    /// Apply a resize `a` to `s`, by its members' constraints when it is a
    /// group that has them.
    pub fn resize_shape(&mut self, s: &mut Shape, a: Affine) {
        let fonts = &mut self.fonts;
        if !resize_constrained(s, a, &mut |sh| geo_bounds(sh, fonts)) {
            s.apply_transform(a);
        }
    }

    /// Per frame: lay out every auto-layout group whose content changed.
    /// A member dragged out of place is re-ordered by where it was dropped.
    pub fn layout_tick(&mut self) {
        let key = (self.doc.revision(), self.fonts.generation);
        // While dragging, members follow the pointer; the drop re-orders them.
        if self.layout_key == Some(key) || (self.press.is_some() && self.dragging) {
            return;
        }
        let groups: Vec<u64> = self.doc.all_ids().into_iter().filter(|id| self.doc.get(*id).is_some_and(|s| s.layout.is_some())).collect();
        let mut changed = false;
        for g in groups {
            changed |= self.relayout(g);
        }
        self.layout_key = if changed { None } else { Some((self.doc.revision(), self.fonts.generation)) };
        if changed {
            self.egui_ctx.request_repaint();
        }
    }

    /// Lay out group `id`; true when anything moved.
    pub fn relayout(&mut self, id: u64) -> bool {
        let Some(g) = self.doc.get(id).cloned() else { return false };
        let Some(l) = g.layout.clone() else { return false };
        let Some(children) = g.kind.children() else { return false };
        let bg = l.background.and_then(|b| children.iter().find(|c| c.id == b)).cloned();
        let mut members: Vec<(u64, Rect)> = Vec::new();
        for c in children {
            if Some(c.id) == l.background || !c.visible {
                continue;
            }
            if let Some(b) = self.cache.update(c, &mut self.fonts).bounds {
                members.push((c.id, b));
            }
        }
        let origin = match &bg {
            Some(b) => self.cache.update(b, &mut self.fonts).bounds.map(|r| r.origin()),
            None => None,
        }
        .or_else(|| members.iter().map(|m| m.1).reduce(|a, b| a.union(b)).map(|r| kurbo::Point::new(r.x0 - l.padding[3], r.y0 - l.padding[0])))
        .unwrap_or_default();
        // Dragged members: re-order by position along the axis.
        let placed = self.layout_placed.get(&id);
        let moved = placed.is_some_and(|p: &HashMap<u64, kurbo::Point>| members.iter().any(|(mid, r)| p.get(mid).is_some_and(|q| (*q - r.origin()).hypot() > 0.5)));
        if moved {
            let axis = |r: &Rect| if l.dir == Direction::Row { r.center().x } else { r.center().y };
            members.sort_by(|a, b| axis(&a.1).total_cmp(&axis(&b.1)));
        }
        let (moves, rect) = arrange(&l, origin, &members);
        let mut out = g.clone();
        let mut any = false;
        if let ShapeKind::Group { children } = &mut out.kind {
            // The background stays at the bottom, under every member.
            if let Some(i) = children.iter().position(|c| Some(c.id) == l.background).filter(|i| *i > 0) {
                let b = children.remove(i);
                children.insert(0, b);
                any = true;
            }
            if moved {
                // Paint order follows the new flow order (background first).
                let order: Vec<u64> = members.iter().map(|m| m.0).collect();
                children.sort_by_key(|c| if Some(c.id) == l.background { 0 } else { order.iter().position(|x| *x == c.id).map(|p| p + 1).unwrap_or(usize::MAX) });
                any = true;
            }
            for (mid, d) in &moves {
                if d.hypot() > 1e-6 {
                    if let Some(c) = children.iter_mut().find(|c| c.id == *mid) {
                        c.apply_transform(Affine::translate(*d));
                        any = true;
                    }
                }
            }
            if let Some(b) = l.background.and_then(|b| children.iter_mut().find(|c| c.id == b)) {
                if let ShapeKind::Rect { w, h, .. } = &mut b.kind {
                    let t = b.transform.translation();
                    if (*w - rect.width()).abs() > 1e-6 || (*h - rect.height()).abs() > 1e-6 || (t.x - rect.x0).abs() > 1e-6 || (t.y - rect.y0).abs() > 1e-6 {
                        *w = rect.width().max(0.0);
                        *h = rect.height().max(0.0);
                        b.transform = Affine::translate((rect.x0, rect.y0));
                        any = true;
                    }
                }
            }
        }
        if any {
            self.doc.replace(out);
        }
        let placed: HashMap<u64, kurbo::Point> = members.iter().map(|(mid, r)| (*mid, r.origin() + moves.iter().find(|m| m.0 == *mid).map(|m| m.1).unwrap_or_default())).collect();
        self.layout_placed.insert(id, placed);
        any
    }

    /// Give the selected group (or the selection, grouped) an auto layout.
    pub fn add_auto_layout(&mut self) {
        let shapes = self.selected_bottom_up();
        if shapes.is_empty() {
            return;
        }
        let single_group = shapes.len() == 1 && matches!(shapes[0].kind, ShapeKind::Group { .. }) && shapes[0].component.is_none();
        self.checkpoint("");
        let gid = if single_group {
            shapes[0].id
        } else {
            let mut g = Shape::new(ShapeKind::Group { children: shapes.clone() });
            g.fill = None;
            g.name = "Auto layout".into();
            self.combine_into(&shapes, g)
        };
        let Some(b) = self.doc.get(gid).cloned().and_then(|g| self.cache.update(&g, &mut self.fonts).bounds) else { return };
        let mut l = AutoLayout::default();
        // Row or column, whichever the members already resemble.
        if let Some(g) = self.doc.get(gid).cloned() {
            let kids: Vec<Shape> = g.kind.children().cloned().unwrap_or_default();
            let rects: Vec<Rect> = kids.iter().filter_map(|c| self.cache.update(c, &mut self.fonts).bounds).collect();
            let spread_x = rects.iter().map(|r| r.center().x).fold((f64::MAX, f64::MIN), |a, v| (a.0.min(v), a.1.max(v)));
            let spread_y = rects.iter().map(|r| r.center().y).fold((f64::MAX, f64::MIN), |a, v| (a.0.min(v), a.1.max(v)));
            if spread_y.1 - spread_y.0 > spread_x.1 - spread_x.0 {
                l.dir = Direction::Column;
            }
        }
        let mut bg = Shape::new(ShapeKind::Rect { w: b.width() + 24.0, h: b.height() + 24.0, radii: [0.0; 4] });
        bg.transform = Affine::translate((b.x0 - 12.0, b.y0 - 12.0));
        bg.fill = None;
        bg.stroke = None;
        bg.name = "Background".into();
        bg.id = self.doc.fresh_id();
        l.background = Some(bg.id);
        if let Some(mut g) = self.doc.get(gid).cloned() {
            if let ShapeKind::Group { children } = &mut g.kind {
                children.insert(0, bg);
            }
            g.layout = Some(l);
            self.doc.replace(g);
        }
        self.selection = vec![gid];
        self.layout_key = None;
        self.relayout(gid);
        self.dirty = true;
    }

    pub fn remove_auto_layout(&mut self) {
        let ids: Vec<u64> = self.selection.iter().copied().filter(|id| self.doc.get(*id).is_some_and(|s| s.layout.is_some())).collect();
        if ids.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in ids {
            if let Some(mut g) = self.doc.get(id).cloned() {
                g.layout = None;
                self.doc.replace(g);
            }
        }
    }

    /// The auto-layout group directly containing `id`, if any.
    pub fn layout_parent(&self, id: u64) -> Option<u64> {
        self.doc.parent_of(id).filter(|p| self.doc.get(*p).is_some_and(|s| s.layout.is_some()))
    }

    /// The top-most auto-layout group under `p`, ignoring `exclude` (and
    /// anything inside it).
    pub fn layout_at(&mut self, p: kurbo::Point, exclude: &[u64]) -> Option<u64> {
        let mut best: Option<(Vec<usize>, u64)> = None;
        for id in self.doc.all_ids() {
            let Some(s) = self.doc.get(id) else { continue };
            if s.layout.is_none() || !s.visible || s.locked || exclude.iter().any(|e| self.doc.is_within(id, *e)) {
                continue;
            }
            let s = s.clone();
            if !geo_bounds(&s, &mut self.fonts).is_some_and(|b| b.contains(p)) {
                continue;
            }
            let key = self.doc.order_key(id);
            // Deeper or later in paint order wins.
            if best.as_ref().is_none_or(|(k, _)| key > *k) {
                best = Some((key, id));
            }
        }
        best.map(|b| b.1)
    }

    /// Move `ids` into auto layout `g`, each in the slot its centre falls in.
    pub fn join_layout(&mut self, ids: &[u64], g: u64) -> bool {
        let Some(l) = self.doc.get(g).and_then(|s| s.layout.clone()) else { return false };
        let axis = |r: Rect| if l.dir == Direction::Row { r.center().x } else { r.center().y };
        let mut moved = false;
        for &id in ids {
            if self.doc.parent_of(id) == Some(g) || self.doc.is_within(g, id) {
                continue;
            }
            let Some(s) = self.doc.get(id).cloned() else { continue };
            let Some(c) = geo_bounds(&s, &mut self.fonts).map(axis) else { continue };
            let children: Vec<Shape> = self.doc.get(g).and_then(|s| s.kind.children().cloned()).unwrap_or_default();
            let mut index = children.len();
            for (i, ch) in children.iter().enumerate() {
                if Some(ch.id) == l.background {
                    continue;
                }
                if geo_bounds(ch, &mut self.fonts).is_some_and(|b| axis(b) > c) {
                    index = i;
                    break;
                }
            }
            moved |= self.doc.move_into(id, Some(g), index);
        }
        if moved {
            self.layout_key = None;
            self.dirty = true;
            let name = self.doc.get(g).map(|s| s.name.clone()).unwrap_or_default();
            self.notify(format!("Added to {name}"));
        }
        moved
    }

    /// After a move on the canvas: shapes dropped on an auto layout join it;
    /// members dragged out of theirs leave it. Returns true when the layer
    /// structure changed.
    pub fn layout_drop(&mut self, ids: &[u64], p: kurbo::Point) -> bool {
        let target = self.layout_at(p, ids);
        let mut changed = false;
        // Leaving: a member released outside its group's box.
        for &id in ids {
            let Some(g) = self.layout_parent(id) else { continue };
            if target == Some(g) {
                continue;
            }
            let outside = self.doc.get(g).cloned().and_then(|s| geo_bounds(&s, &mut self.fonts)).is_some_and(|b| !b.contains(p));
            if outside {
                let parent = self.doc.parent_of(g);
                changed |= match parent {
                    Some(pp) => self.doc.move_into(id, Some(pp), usize::MAX),
                    None => self.doc.move_beside(id, g, true),
                };
            }
        }
        if let Some(g) = target {
            changed |= self.join_layout(ids, g);
        }
        if changed {
            self.layout_key = None;
            self.dirty = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_follow_the_resize() {
        // Member 10..30 in a 0..100 group resized to 0..200.
        let (b, n) = ((0.0, 100.0), (0.0, 200.0));
        assert_eq!(pin_span(Pin::Start, (10.0, 30.0), b, n), (10.0, 30.0));
        assert_eq!(pin_span(Pin::End, (10.0, 30.0), b, n), (110.0, 130.0));
        assert_eq!(pin_span(Pin::Both, (10.0, 30.0), b, n), (10.0, 130.0));
        assert_eq!(pin_span(Pin::Center, (10.0, 30.0), b, n), (60.0, 80.0));
        assert_eq!(pin_span(Pin::Scale, (10.0, 30.0), b, n), (20.0, 60.0));
    }

    #[test]
    fn arrange_row_and_column() {
        let l = AutoLayout { dir: Direction::Row, gap: 10.0, padding: [5.0, 6.0, 7.0, 8.0], align: Align::End, background: None };
        let m = [(1, Rect::new(100.0, 100.0, 120.0, 110.0)), (2, Rect::new(0.0, 0.0, 30.0, 40.0))];
        let (moves, r) = arrange(&l, kurbo::Point::new(0.0, 0.0), &m);
        // First at (8, 5 + 40 - 10), second at (8 + 20 + 10, 5).
        assert_eq!(moves[0].1, Vec2::new(8.0 - 100.0, 35.0 - 100.0));
        assert_eq!(moves[1].1, Vec2::new(38.0, 5.0));
        assert_eq!(r, Rect::new(0.0, 0.0, 8.0 + 60.0 + 6.0, 5.0 + 40.0 + 7.0));
        let l = AutoLayout { dir: Direction::Column, align: Align::Center, ..l };
        let (moves, r) = arrange(&l, kurbo::Point::new(0.0, 0.0), &m);
        assert_eq!(moves[0].1, Vec2::new(8.0 + 5.0 - 100.0, 5.0 - 100.0));
        assert_eq!(r.height(), 5.0 + 10.0 + 10.0 + 40.0 + 7.0);
    }
}
