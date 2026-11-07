//! Artboards on the canvas: drawing (backgrounds under the artwork, name
//! labels above), the Artboard tool (F: drag to create, drag one to move it
//! with its contents), label clicks/drags with any tool, and resize handles
//! on the selected artboard. Commands to add, duplicate, delete and step
//! between artboards.

use egui::{Color32, CursorIcon, Pos2, Stroke, Ui, vec2};
use kurbo::{Affine, Point, Rect, Vec2};

use crate::app::{App, Tool};
use crate::document::{Artboard, Shape};
use crate::ui::theme;

/// Gap between artboards placed automatically.
const GAP: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AbMode {
    Create,
    Move,
    /// Edges being dragged: x (-1 left, 1 right, 0 none), y likewise.
    Resize(i8, i8),
}

pub struct ArtboardDrag {
    pub id: u64,
    pub mode: AbMode,
    pub start: Point,
    pub orig: Rect,
    /// Top-level shapes inside the artboard (moved along), as they were.
    pub contents: Vec<Shape>,
    pub moved: bool,
    /// Alt: the first motion clones the artboard (and its artwork) and the
    /// copy is what moves.
    pub clone: bool,
}

impl App {
    /// The artboard the panel edits (falls back to the first).
    pub fn active(&self) -> &Artboard {
        self.doc.artboard(self.active_artboard).unwrap_or(self.doc.primary())
    }

    pub fn active_mut(&mut self) -> &mut Artboard {
        let id = self.active().id;
        self.doc.artboard_mut(id).unwrap()
    }

    pub fn activate_artboard(&mut self, id: u64, select: bool) {
        self.active_artboard = id;
        if select {
            self.selection.clear();
            self.node_sel.clear();
        }
        self.artboard_selected = select;
    }

    /// The artboard a top-level shape belongs to (see `Document::member_of`).
    pub fn member_of(&mut self, s: &Shape) -> Option<u64> {
        let b = self.cache.update(s, &mut self.fonts).bounds?;
        self.doc.member_of(b).map(|a| a.id)
    }

    /// After shapes moved: those whose artboard changed go to the top of their
    /// new artboard's stack (or of the canvas when they left every
    /// artboard), like dropping a layer into a Figma frame. `before` holds
    /// each moved shape's id and its artboard before the move.
    pub fn rehome(&mut self, before: &[(u64, Option<u64>)]) {
        let mut changed: Vec<(u64, Option<u64>)> = Vec::new();
        for &(id, was) in before {
            let Some(s) = self.doc.get(id).cloned() else { continue };
            if self.doc.index_of(id).is_none() {
                continue; // not top-level
            }
            let now = self.member_of(&s);
            if now != was {
                changed.push((id, now));
            }
        }
        if changed.is_empty() {
            return;
        }
        let moving: Vec<u64> = changed.iter().map(|c| c.0).collect();
        for &(id, now) in &changed {
            let top = match now {
                Some(ab) => {
                    let list: Vec<Shape> = self.doc.shapes().iter().map(|s| Shape::clone(s)).collect();
                    let mut top = None;
                    for (i, s) in list.iter().enumerate() {
                        if !moving.contains(&s.id) && self.member_of(s) == Some(ab) {
                            top = Some(i);
                        }
                    }
                    top
                }
                None => Some(self.doc.shapes().len()),
            };
            if let (Some(t), Some(i)) = (top, self.doc.index_of(id)) {
                // Removing the shape first shifts everything above it down.
                let to = if i <= t { t } else { t + 1 };
                self.doc.move_to_index(id, to);
            }
        }
        if let [(_, now)] = changed.as_slice() {
            let name = now.and_then(|a| self.doc.artboard(a)).map(|a| a.name.clone());
            self.notify(match name {
                Some(n) => format!("Moved into {n}"),
                None => "Moved out of the artboard".to_string(),
            });
        }
        self.dirty = true;
    }

    /// Move a top-level shape from artboard `from` into artboard `to` (None:
    /// the canvas outside every artboard), keeping its place relative to the
    /// artboard when it can; returns whether it moved.
    pub fn move_to_artboard(&mut self, id: u64, from: Option<u64>, to: Option<u64>) -> bool {
        if from == to {
            return false;
        }
        let Some(s) = self.doc.get(id).cloned() else { return false };
        let Some(b) = self.cache.update(&s, &mut self.fonts).bounds else { return false };
        let (f, t) = (from.and_then(|a| self.doc.artboard(a)).cloned(), to.and_then(|a| self.doc.artboard(a)).cloned());
        let delta = match (&f, &t) {
            (Some(f), Some(t)) => Vec2::new(t.x - f.x, t.y - f.y),
            (None, Some(t)) => t.rect().center() - b.center(),
            // Past the right edge of every artboard.
            (_, None) => Vec2::new(self.doc.bounds().x1 + GAP - b.x0, 0.0),
        };
        self.translate_ids(&[id], delta);
        if let Some(t) = &t {
            let s = self.doc.get(id).cloned().unwrap();
            if self.member_of(&s) != Some(t.id) {
                // Didn't land inside (a smaller artboard): centre it.
                self.center_ids_at(&[id], t.rect().center());
            }
        }
        self.dirty = true;
        true
    }

    /// Top-level shapes belonging to artboard `id`, bottom-most first.
    pub fn shapes_in(&mut self, id: u64) -> Vec<Shape> {
        let list: Vec<Shape> = self.doc.shapes().iter().map(|s| Shape::clone(s)).collect();
        list.into_iter().filter(|s| self.member_of(s) == Some(id)).collect()
    }

    /// Room to the right of every artboard, aligned with the top of `like`.
    fn free_spot(&self, like: &Artboard) -> Point {
        Point::new(self.doc.bounds().x1 + GAP, like.y)
    }

    /// Add an artboard the size of the active one, to the right of the others.
    pub fn new_artboard(&mut self) {
        let like = self.active().clone();
        let at = self.free_spot(&like);
        self.checkpoint("");
        let id = self.doc.add_artboard(Rect::from_origin_size(at, (like.w, like.h)), None);
        if let Some(a) = self.doc.artboard_mut(id) {
            a.background = like.background;
        }
        self.activate_artboard(id, true);
        self.zoom_to_artboard(id);
        self.dirty = true;
    }

    /// Copy the active artboard and everything on it.
    pub fn duplicate_artboard(&mut self) {
        let src = self.active().clone();
        let at = self.free_spot(&src);
        let contents = self.shapes_in(src.id);
        self.checkpoint("");
        let id = self.doc.add_artboard(Rect::from_origin_size(at, (src.w, src.h)), Some(format!("{} copy", src.name)));
        if let Some(a) = self.doc.artboard_mut(id) {
            a.background = src.background;
            a.clip = src.clip;
        }
        let shift = Affine::translate((at.x - src.x, at.y - src.y));
        for mut s in contents {
            s.id = 0;
            s.apply_transform(shift);
            self.doc.add_shape(s);
        }
        self.activate_artboard(id, true);
        self.zoom_to_artboard(id);
        self.dirty = true;
    }

    /// Remove the active artboard (its artwork stays). Keeps at least one.
    pub fn delete_artboard(&mut self) {
        if self.doc.artboards.len() < 2 {
            self.notify("A document needs at least one artboard");
            return;
        }
        let id = self.active().id;
        self.checkpoint("");
        self.doc.artboards.retain(|a| a.id != id);
        self.active_artboard = self.doc.primary().id;
        self.artboard_selected = false;
        self.dirty = true;
    }

    pub fn zoom_to_artboard(&mut self, id: u64) {
        if let Some(r) = self.doc.artboard(id).map(|a| a.rect()) {
            self.zoom_to_rect(r, 96.0);
        }
    }

    /// Activate and show the next (`1`) or previous (`-1`) artboard.
    pub fn step_artboard(&mut self, dir: i32) {
        let n = self.doc.artboards.len() as i32;
        let i = self.doc.artboards.iter().position(|a| a.id == self.active().id).unwrap_or(0) as i32;
        let id = self.doc.artboards[((i + dir) % n + n) as usize % n as usize].id;
        self.activate_artboard(id, true);
        self.zoom_to_artboard(id);
    }

    /// Per frame: the active artboard follows the selection.
    pub fn artboard_tick(&mut self) {
        if !self.selection.is_empty() {
            self.artboard_selected = false;
            if let Some(b) = self.selection_bounds() {
                if let Some(a) = self.doc.artboard_for(b) {
                    self.active_artboard = a.id;
                }
            }
        }
        if self.doc.artboard(self.active_artboard).is_none() {
            self.active_artboard = self.doc.primary().id;
        }
    }

    /// Backgrounds (with a soft shadow) of every artboard, under the artwork.
    pub fn paint_artboards(&mut self, painter: &egui::Painter) {
        let clip = painter.clip_rect();
        for a in &self.doc.artboards {
            let ab = self.view.rect_to_screen(a.rect());
            if !ab.intersects(clip.expand(30.0)) {
                continue;
            }
            painter.add(egui::epaint::RectShape::filled(ab, 0.0, Color32::BLACK).with_blur_width(24.0));
            if a.background.a < 1.0 {
                crate::color::checkerboard(&painter.with_clip_rect(ab.intersect(clip)), ab, 10.0);
            }
            painter.rect_filled(ab, 0.0, a.background.to_color32(1.0));
        }
    }

    /// Name labels above the artboards (remembered for hit testing), and the
    /// selected artboard's outline and handles.
    pub fn paint_artboard_overlay(&mut self, painter: &egui::Painter) {
        self.artboard_labels.clear();
        let active = self.active().id;
        let show_all = self.doc.artboards.len() > 1;
        let hover_label = self.artboard_label_hover;
        for a in &self.doc.artboards {
            let ab = self.view.rect_to_screen(a.rect());
            let strong = a.id == active && (self.artboard_selected || show_all);
            let text = if a.id == active || !show_all { format!("{}  {} × {}", a.name, crate::units::fmt_value(a.w), crate::units::fmt(a.h)) } else { a.name.clone() };
            let color = if strong && self.artboard_selected { theme::ACCENT } else if strong || hover_label == Some(a.id) { theme::TEXT } else { theme::TEXT_DIM };
            let font = egui::FontId::proportional(11.5);
            let renaming = self.rename_on_canvas && self.renaming.as_ref().map(|r| r.0) == Some(a.id);
            let galley = painter.layout_no_wrap(if renaming { String::new() } else { text }, font, color);
            let max_w = ab.width().max(40.0);
            let size = vec2(galley.size().x.min(max_w), galley.size().y);
            let r = egui::Rect::from_min_size(ab.left_top() + vec2(0.0, -6.0 - size.y), size);
            painter.with_clip_rect(r.intersect(painter.clip_rect())).galley(r.min, galley, color);
            self.artboard_labels.push((a.id, r.expand2(vec2(2.0, 3.0))));
        }
        // Outline + handles of the selected artboard (or the one being dragged).
        let shown = self.artboard_drag.as_ref().map(|d| d.id).or((self.artboard_selected || self.tool == Tool::Artboard).then_some(active));
        if let Some(a) = shown.and_then(|id| self.doc.artboard(id)) {
            let ab = self.view.rect_to_screen(a.rect());
            painter.rect_stroke(ab, 0.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Outside);
            if self.artboard_selected || self.tool == Tool::Artboard {
                for (p, _) in handle_points(ab) {
                    let h = egui::Rect::from_center_size(p, vec2(7.0, 7.0));
                    painter.rect_filled(h, 1.0, Color32::WHITE);
                    painter.rect_stroke(h, 1.0, Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
                }
            }
            if let Some(d) = &self.artboard_drag {
                if d.moved {
                    let badge = format!("{} × {}", crate::units::fmt_value(a.w), crate::units::fmt(a.h));
                    let pos = ab.center_bottom() + vec2(0.0, 10.0);
                    let g = painter.layout_no_wrap(badge, egui::FontId::proportional(11.0), Color32::WHITE);
                    let br = egui::Rect::from_center_size(pos + vec2(0.0, g.size().y / 2.0), g.size() + vec2(10.0, 4.0));
                    painter.rect_filled(br, 3.0, theme::ACCENT);
                    painter.galley(br.min + vec2(5.0, 2.0), g, Color32::WHITE);
                }
            }
        }
    }

    /// The inline name field over an artboard label being renamed.
    pub fn artboard_rename_ui(&mut self, ui: &mut Ui) {
        if !self.rename_on_canvas {
            return;
        }
        let Some(id) = self.renaming.as_ref().map(|r| r.0) else {
            self.rename_on_canvas = false;
            return;
        };
        let Some(a) = self.doc.artboard(id) else { return };
        let ab = self.view.rect_to_screen(a.rect());
        let r = egui::Rect::from_min_size(ab.left_top() + vec2(-4.0, -26.0), vec2(ab.width().clamp(120.0, 240.0), 22.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
        crate::ui::studio::layers::rename_field(self, &mut child, r.width());
        if self.renaming.is_none() {
            self.rename_on_canvas = false;
        }
    }

    fn artboard_handle_at(&self, sp: Pos2) -> Option<(i8, i8)> {
        if !(self.artboard_selected || self.tool == Tool::Artboard) {
            return None;
        }
        let ab = self.view.rect_to_screen(self.active().rect());
        handle_points(ab).into_iter().find(|(p, _)| p.distance(sp) <= 6.0).map(|(_, d)| d)
    }

    fn artboard_label_at(&self, sp: Pos2) -> Option<u64> {
        self.artboard_labels.iter().rev().find(|(_, r)| r.contains(sp)).map(|(id, _)| *id)
    }

    /// Artboard interaction; returns true when it handled the pointer this
    /// frame (the regular tool input is skipped).
    pub fn artboard_input(&mut self, ui: &mut Ui, resp: &egui::Response) -> bool {
        let (pos, pressed, down, double, alt) = ui.input(|i| (i.pointer.interact_pos().or(i.pointer.hover_pos()), i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.button_double_clicked(egui::PointerButton::Primary), i.modifiers.alt));
        let Some(sp) = pos else { return false };
        let p = self.view.to_doc(sp);
        // Double-click a label (reported on the second release, while the
        // press already started a label drag): rename it in place.
        if double && self.artboard_drag.as_ref().map(|d| !d.moved).unwrap_or(true) {
            if let Some(id) = self.artboard_label_at(sp) {
                self.artboard_drag = None;
                self.activate_artboard(id, true);
                self.start_rename(id);
                self.rename_on_canvas = true;
                return true;
            }
        }
        if let Some(mut d) = self.artboard_drag.take() {
            if !down {
                self.dirty |= d.moved;
                self.snap_guides.clear();
                self.gap_guides.clear();
                return true;
            }
            let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command || i.modifiers.ctrl));
            self.drag_artboard(&mut d, p, shift, self.snap && !ctrl);
            ui.ctx().set_cursor_icon(match d.mode {
                AbMode::Move => CursorIcon::Grabbing,
                AbMode::Resize(x, y) => resize_cursor(x, y),
                AbMode::Create => CursorIcon::Crosshair,
            });
            self.artboard_drag = Some(d);
            return true;
        }
        if !resp.hovered() || self.press.is_some() || self.tool == Tool::Hand || ui.input(|i| i.key_down(egui::Key::Space)) {
            self.artboard_label_hover = None;
            return false;
        }
        let label = self.artboard_label_at(sp);
        self.artboard_label_hover = label;
        let handle = self.artboard_handle_at(sp);
        if let Some((x, y)) = handle {
            ui.ctx().set_cursor_icon(resize_cursor(x, y));
        } else if label.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Default);
        } else if self.tool == Tool::Artboard {
            ui.ctx().set_cursor_icon(if self.doc.artboard_at(p).is_some() { CursorIcon::Move } else { CursorIcon::Crosshair });
        }
        if !pressed {
            return false;
        }
        if let Some(dir) = handle {
            let a = self.active().clone();
            self.begin_artboard_drag(a.id, AbMode::Resize(dir.0, dir.1), p, false);
            return true;
        }
        if let Some(id) = label {
            self.activate_artboard(id, true);
            self.begin_artboard_drag(id, AbMode::Move, p, alt);
            return true;
        }
        if self.tool == Tool::Artboard {
            match self.doc.artboard_at(p).map(|a| a.id) {
                Some(id) => {
                    self.activate_artboard(id, true);
                    self.begin_artboard_drag(id, AbMode::Move, p, alt);
                }
                None => {
                    // The artboard is added once the pointer moves.
                    self.artboard_drag = Some(ArtboardDrag { id: 0, mode: AbMode::Create, start: p, orig: Rect::from_points(p, p), contents: Vec::new(), moved: false, clone: false });
                }
            }
            return true;
        }
        // Any other press deselects the artboard (the tool handles it).
        self.artboard_selected = false;
        false
    }

    /// `alt`: clone the artboard and move the copy.
    fn begin_artboard_drag(&mut self, id: u64, mode: AbMode, p: Point, alt: bool) {
        let Some(orig) = self.doc.artboard(id).map(|a| a.rect()) else { return };
        let contents = if mode == AbMode::Move { self.shapes_in(id) } else { Vec::new() };
        self.artboard_drag = Some(ArtboardDrag { id, mode, start: p, orig, contents, moved: false, clone: alt && mode == AbMode::Move });
    }

    /// Copy artboard `id` with its artwork in place (for Alt+drag); returns
    /// the copy's id and its top-level shapes.
    fn clone_artboard_in_place(&mut self, id: u64, contents: &[Shape]) -> Option<(u64, Vec<Shape>)> {
        let src = self.doc.artboard(id)?.clone();
        let new = self.doc.add_artboard(src.rect(), Some(format!("{} copy", src.name)));
        if let Some(a) = self.doc.artboard_mut(new) {
            a.background = src.background;
            a.clip = src.clip;
        }
        let mut copies = Vec::new();
        for orig in contents {
            let mut s = orig.clone();
            s.id = 0;
            let nid = self.doc.add_shape(s);
            self.instance_from_copy(orig, nid);
            copies.extend(self.doc.get(nid).cloned());
        }
        Some((new, copies))
    }

    /// Snap a point while dragging artboard `id` (its own box doesn't count).
    fn snap_ab_point(&mut self, id: u64, p: Point, snap: bool) -> Point {
        if !snap {
            self.snap_guides.clear();
            return p;
        }
        self.snap_artboard = Some(id);
        let q = self.snap_point(p, &[]);
        self.snap_artboard = None;
        q
    }

    fn drag_artboard(&mut self, d: &mut ArtboardDrag, p: Point, shift: bool, snap: bool) {
        let mut delta = p - d.start;
        if !d.moved {
            if delta.hypot() * (self.view.zoom as f64) < 3.0 {
                return;
            }
            // First real motion: one undo step for the whole drag.
            d.moved = true;
            self.checkpoint("");
            if d.mode == AbMode::Create {
                d.id = self.doc.add_artboard(Rect::from_origin_size(d.start, (1.0, 1.0)), None);
                self.activate_artboard(d.id, true);
            }
            if d.clone {
                if let Some((id, copies)) = self.clone_artboard_in_place(d.id, &d.contents) {
                    d.id = id;
                    d.contents = copies;
                    self.activate_artboard(id, true);
                }
            }
        }
        let o = d.orig;
        let r = match d.mode {
            AbMode::Move => {
                if shift {
                    if delta.x.abs() > delta.y.abs() { delta.y = 0.0 } else { delta.x = 0.0 }
                }
                let mut delta = Vec2::new(delta.x.round(), delta.y.round());
                if snap {
                    let ids: Vec<u64> = d.contents.iter().map(|s| s.id).collect();
                    self.snap_artboard = Some(d.id);
                    let s = self.snap_box(o, delta, &ids);
                    self.snap_artboard = None;
                    // Keep a Shift constraint on its axis.
                    delta = Vec2::new(if shift && delta.x == 0.0 { 0.0 } else { s.x }, if shift && delta.y == 0.0 { 0.0 } else { s.y });
                    if shift {
                        let (vx, vy) = (delta.x == 0.0, delta.y == 0.0);
                        self.snap_guides.retain(|g| !(g.vertical && vx || !g.vertical && vy));
                    }
                } else {
                    self.snap_guides.clear();
                    self.gap_guides.clear();
                }
                let shift_a = Affine::translate(delta);
                for s in &d.contents {
                    let mut s2 = s.clone();
                    s2.apply_transform(shift_a);
                    self.doc.replace(s2);
                }
                o + delta
            }
            AbMode::Create => {
                let p = if shift { p } else { self.snap_ab_point(d.id, p, snap) };
                let (mut x1, mut y1) = (p.x.round(), p.y.round());
                if shift {
                    let side = (x1 - d.start.x).abs().max((y1 - d.start.y).abs());
                    x1 = d.start.x + side * (x1 - d.start.x).signum();
                    y1 = d.start.y + side * (y1 - d.start.y).signum();
                }
                Rect::from_points(Point::new(d.start.x.round(), d.start.y.round()), Point::new(x1, y1))
            }
            AbMode::Resize(hx, hy) => {
                // Snap the dragged edges (the corner or edge midpoint being moved).
                let ex = match hx { -1 => o.x0, 1 => o.x1, _ => o.center().x };
                let ey = match hy { -1 => o.y0, 1 => o.y1, _ => o.center().y };
                let e = Point::new(ex + if hx != 0 { delta.x } else { 0.0 }, ey + if hy != 0 { delta.y } else { 0.0 });
                let q = self.snap_ab_point(d.id, e, snap);
                if hx != 0 {
                    delta.x += q.x - e.x;
                }
                if hy != 0 {
                    delta.y += q.y - e.y;
                }
                if hx == 0 || hy == 0 {
                    let v = hx != 0;
                    self.snap_guides.retain(|g| g.vertical == v);
                }
                let (mut x0, mut y0, mut x1, mut y1) = (o.x0, o.y0, o.x1, o.y1);
                match hx {
                    -1 => x0 = (o.x0 + delta.x).round().min(o.x1 - 1.0),
                    1 => x1 = (o.x1 + delta.x).round().max(o.x0 + 1.0),
                    _ => {}
                }
                match hy {
                    -1 => y0 = (o.y0 + delta.y).round().min(o.y1 - 1.0),
                    1 => y1 = (o.y1 + delta.y).round().max(o.y0 + 1.0),
                    _ => {}
                }
                Rect::new(x0, y0, x1, y1)
            }
        };
        if let Some(a) = self.doc.artboard_mut(d.id) {
            a.set_rect(r);
        }
    }
}

fn handle_points(ab: egui::Rect) -> [(Pos2, (i8, i8)); 8] {
    let (l, r, t, b, cx, cy) = (ab.left(), ab.right(), ab.top(), ab.bottom(), ab.center().x, ab.center().y);
    [
        (Pos2::new(l, t), (-1, -1)),
        (Pos2::new(cx, t), (0, -1)),
        (Pos2::new(r, t), (1, -1)),
        (Pos2::new(r, cy), (1, 0)),
        (Pos2::new(r, b), (1, 1)),
        (Pos2::new(cx, b), (0, 1)),
        (Pos2::new(l, b), (-1, 1)),
        (Pos2::new(l, cy), (-1, 0)),
    ]
}

fn resize_cursor(x: i8, y: i8) -> CursorIcon {
    match (x, y) {
        (0, _) => CursorIcon::ResizeVertical,
        (_, 0) => CursorIcon::ResizeHorizontal,
        (a, b) if a == b => CursorIcon::ResizeNwSe,
        _ => CursorIcon::ResizeNeSw,
    }
}
