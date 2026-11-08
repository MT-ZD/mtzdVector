//! Canvas: view navigation, tool interactions and on-canvas overlays.

use egui::{Color32, CornerRadius, CursorIcon, Pos2, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use kurbo::{Affine, BezPath, Point, Rect, Shape as _, Vec2};

use crate::app::{App, BuilderMode, BuilderState, Drag, Handle, PenState, TextEditState, Tool};
use crate::boolean;
use crate::document::{FillRule, Node, Paint, PathData, Rgba, Shape, ShapeKind, Stroke as DStroke, SubPath, TextData};
use crate::geom;
use crate::render;
use crate::ui::theme;

const HANDLE_R: f32 = 7.0;
/// Pen: clicks this close (screen px) to the first node close the path.
const PEN_CLOSE_R: f32 = 12.0;
/// Distance of the rotation knob above the top edge (screen px).
const KNOB_GAP: f32 = 26.0;

struct Gizmo {
    corners: [Pos2; 4],
    handles: [(Handle, Pos2); 8],
    knob: Pos2,
    top: Pos2,
    center: Pos2,
}

/// Point inside a convex quad (with `pad` px of slack).
/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn point_in_quad(p: Pos2, q: &[Pos2; 4], pad: f32) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let e = b - a;
        let len = e.length().max(1e-3);
        let d = (e.x * (p.y - a.y) - e.y * (p.x - a.x)) / len;
        if sign == 0.0 && d.abs() > pad {
            sign = d.signum();
        }
        if sign != 0.0 && d * sign < -pad {
            return false;
        }
    }
    true
}

/// Resize cursor for a handle at `hp` on a box centered at `c` (follows rotation).
fn oriented_cursor(h: Handle, hp: Pos2, c: Pos2) -> CursorIcon {
    let v = hp - c;
    if v.length() < 1.0 {
        return handle_cursor(h);
    }
    let a = v.y.atan2(v.x).to_degrees().rem_euclid(180.0);
    match a {
        a if !(22.5..157.5).contains(&a) => CursorIcon::ResizeHorizontal,
        a if a < 67.5 => CursorIcon::ResizeNwSe,
        a if a < 112.5 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// Draw a small rotate glyph at the pointer (the OS has no rotate cursor).
fn paint_rotate_cursor(painter: &egui::Painter, at: Pos2) {
    let g = egui_phosphor::regular::ARROW_CLOCKWISE;
    let font = egui::FontId::proportional(17.0);
    for o in [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(0.0, 1.0)] {
        painter.text(at + o, egui::Align2::CENTER_CENTER, g, font.clone(), Color32::BLACK);
    }
    painter.text(at, egui::Align2::CENTER_CENTER, g, font, Color32::WHITE);
}

/// Two opposed arrows along `dir` (the dragged edge), offset across it:
/// the shear cursor.
fn paint_shear_cursor(painter: &egui::Painter, at: Pos2, dir: egui::Vec2) {
    let n = vec2(-dir.y, dir.x);
    let arrow = |o: egui::Vec2, d: egui::Vec2| {
        let a = at + o - d * 7.0;
        let b = at + o + d * 7.0;
        let head = [b, b - d * 4.0 + n * 3.0, b - d * 4.0 - n * 3.0];
        vec![[a, b], [head[0], head[1]], [head[0], head[2]]]
    };
    let segs: Vec<[Pos2; 2]> = arrow(-n * 3.5, dir).into_iter().chain(arrow(n * 3.5, -dir)).collect();
    for s in &segs {
        painter.line_segment(*s, Stroke::new(3.5, Color32::BLACK));
    }
    for s in &segs {
        painter.line_segment(*s, Stroke::new(1.6, Color32::WHITE));
    }
}

fn handle_cursor(h: Handle) -> CursorIcon {
    match h {
        Handle::N | Handle::S => CursorIcon::ResizeVertical,
        Handle::E | Handle::W => CursorIcon::ResizeHorizontal,
        Handle::NE | Handle::SW => CursorIcon::ResizeNeSw,
        Handle::NW | Handle::SE => CursorIcon::ResizeNwSe,
    }
}

struct Input {
    pos: Option<Pos2>,
    pressed: bool,
    released: bool,
    down: bool,
    double: bool,
    triple: bool,
    shift: bool,
    alt: bool,
    cmd: bool,
}

impl App {
    /// Start the stabilizer (when on) at `p`.
    fn start_stabilizer(&mut self, p: Point) {
        let len = self.stab_len as f64 / self.view.zoom.max(1e-6) as f64;
        self.stab = self.stabilizer.then(|| crate::freehand::Stabilizer::new(self.stab_mode, len, p));
    }

    /// Pointer position `p` through the stabilizer: the new drawing tip, or
    /// None while the rope is slack.
    fn stabilized(&mut self, p: Point) -> Option<Point> {
        match self.stab.as_mut() {
            Some(st) => st.feed(p),
            None => Some(p),
        }
    }

    /// The pencil / brush shape for the points drawn so far.
    fn freehand_shape(&self, pts: &[Point], times: &[f64], pressure: &[Option<f32>]) -> Option<Shape> {
        let px = 1.0 / self.view.zoom as f64;
        if self.tool == Tool::Brush {
            crate::freehand::brush_shape(pts, times, pressure, px, &self.brush)
        } else {
            crate::freehand::pencil_shape(pts, self.brush.smooth, px, self.brush.color)
        }
    }

    /// Put the stroke drawn so far in the document (one undo step for the
    /// whole stroke).
    fn freehand_live(&mut self, live: &mut Option<u64>, shape: Option<Shape>) {
        match (shape, *live) {
            (Some(mut s), Some(id)) => {
                s.id = id;
                if s.kind.children().is_some() {
                    // Media brushes: a group of strands, whose members need ids.
                    *live = self.doc.replace_with(id, vec![s]).first().copied();
                } else {
                    self.doc.replace(s);
                }
            }
            (Some(s), None) => {
                self.checkpoint("Draw");
                *live = Some(self.doc.add_shape(s));
            }
            (None, Some(id)) => {
                self.doc.remove(id);
                *live = None;
            }
            (None, None) => {}
        }
    }

    pub fn canvas(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let stable = (self.canvas_rect.size() - rect.size()).length() < 0.5;
        self.canvas_rect = rect;
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        ui.input(|i| {
            self.input_time = i.time;
            for e in &i.events {
                if let egui::Event::Touch { force, phase, .. } = e {
                    self.pen_pressure = match phase {
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => None,
                        _ => *force,
                    };
                }
            }
            if self.tablet_seen {
                self.pen_pressure = self.tablet_pressure;
            }
        });
        // Fit the artboard once the layout has settled (panel sizes change during the first frames).
        let sane = ui.ctx().content_rect().expand(1.0).contains_rect(rect) && !ui.ctx().will_discard();
        if !self.view_ready && rect.width() > 50.0 && sane {
            self.zoom_to_fit();
            self.fit_passes += 1;
            self.view_ready = stable && self.fit_passes >= 4;
        }
        let painter = ui.painter_at(rect);

        self.navigate(ui, &resp);
        self.tool_input(ui, &resp);
        // Open on press (not click): a click is cancelled by the slightest
        // pointer motion, which made the menu hard to bring up.
        let (sec_press, sec_pos) = ui.input(|i| (i.pointer.button_pressed(egui::PointerButton::Secondary), i.pointer.press_origin()));
        if sec_press && resp.hovered() && self.press.is_none() {
            if let Some(pos) = sec_pos.or(resp.hover_pos()) {
                self.open_context_menu(pos, false);
            }
        }

        // Background + artboards.
        painter.rect_filled(rect, 0.0, theme::CANVAS);
        self.paint_artboards(&painter);

        if self.ui.outline_view {
            self.cache.paint_outlines(&painter, &self.doc, &mut self.fonts, &self.view);
        } else {
            self.cache.paint_document(&painter, &self.doc, &mut self.fonts, &self.view, None);
        }
        self.paint_artboard_overlay(&painter);
        self.overlays(&painter);
        self.artboard_rename_ui(ui);
        if let Some(at) = self.drop_hover {
            paint_drop_overlay(&painter, rect, at);
        }
    }

    fn navigate(&mut self, ui: &mut Ui, resp: &egui::Response) {
        let (scroll, zoom, hover, delta, middle, primary, space) = ui.input(|i| {
            (
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.pointer.hover_pos(),
                i.pointer.delta(),
                i.pointer.middle_down(),
                i.pointer.primary_down(),
                i.key_down(egui::Key::Space),
            )
        });
        let space = space && self.text_edit.is_none() && !ui.ctx().egui_wants_keyboard_input();
        let over = resp.hovered();
        if over {
            if let Some(p) = hover {
                if zoom != 1.0 {
                    self.zoom_at(zoom, p);
                } else if scroll != egui::Vec2::ZERO {
                    self.view.origin += scroll;
                }
            }
        }
        let pan_tool = self.tool == Tool::Hand || space;
        if (middle && (over || matches!(self.drag, Drag::Pan))) || (pan_tool && primary && (over || matches!(self.drag, Drag::Pan))) {
            if !matches!(self.drag, Drag::Pan) && self.press.is_none() {
                self.drag = Drag::Pan;
            }
            if matches!(self.drag, Drag::Pan) {
                self.view.origin += delta;
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
        } else if matches!(self.drag, Drag::Pan) {
            self.drag = Drag::None;
        } else if pan_tool && over {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
    }

    // ------------------------------------------------------------------
    // Hit testing
    // ------------------------------------------------------------------

    pub fn hit_shape(&mut self, p: Point) -> Option<u64> {
        self.hit_shapes(p, true).into_iter().next()
    }

    /// Direct child of compound `id` under `p` (top-most first).
    pub fn hit_child(&mut self, id: u64, p: Point) -> Option<u64> {
        let Some(parent) = self.doc.get(id).cloned() else { return None };
        let is_group = matches!(parent.kind, ShapeKind::Group { .. });
        let children = parent.kind.children()?.clone();
        let tol = 4.0 / self.view.zoom as f64;
        for c in children.iter().rev() {
            if !c.visible || c.locked {
                continue;
            }
            if is_group {
                if self.shape_hit(c, p, tol) {
                    return Some(c.id);
                }
                continue;
            }
            // Boolean children count by their outline, whatever their style.
            let cache = self.cache.update(c, &mut self.fonts);
            let w = cache.doc_path.winding(p);
            let inside = match c.fill_rule {
                FillRule::NonZero => w != 0,
                FillRule::EvenOdd => w % 2 != 0,
            };
            if inside || geom::distance_to_path(&cache.doc_path, p) <= tol {
                return Some(c.id);
            }
        }
        None
    }

    /// Shape a click at `p` selects: the top-level shape, or, while editing
    /// inside a boolean, the sibling under the pointer. `deep` goes straight
    /// to the innermost shape (Ctrl+click).
    pub fn pick(&mut self, p: Point, deep: bool) -> Option<u64> {
        // An auto layout's background stands for the layout itself.
        let id = self.pick_any(p, deep)?;
        match self.doc.parent_of(id) {
            Some(g) if self.doc.get(g).and_then(|s| s.layout.as_ref()).and_then(|l| l.background) == Some(id) => Some(g),
            _ => Some(id),
        }
    }

    fn pick_any(&mut self, p: Point, deep: bool) -> Option<u64> {
        let top = self.hit_shape(p);
        if deep {
            let mut cur = top?;
            while let Some(c) = self.hit_child(cur, p) {
                cur = c;
            }
            return Some(cur);
        }
        // Context: the deepest compound that contains a selected shape and is
        // under the pointer. Its children count by their own outline, so a
        // subtracted cutter (a hole in the result) stays grabbable, unless a
        // shape above the compound is hit.
        for sel in self.selection.clone() {
            let mut parent = self.doc.parent_of(sel);
            while let Some(par) = parent {
                let root = self.doc.root_of(par);
                let in_front = match (top, root) {
                    (None, _) => true,
                    (Some(t), Some(r)) => r == t || self.doc.index_of(r).unwrap_or(0) > self.doc.index_of(t).unwrap_or(0),
                    _ => false,
                };
                if in_front {
                    if let Some(c) = self.hit_child(par, p) {
                        return Some(c);
                    }
                }
                parent = self.doc.parent_of(par);
            }
        }
        top
    }

    /// Every visible shape under `p`, top-most first (locked ones only when
    /// `skip_locked` is false).
    pub fn hit_shapes(&mut self, p: Point, skip_locked: bool) -> Vec<u64> {
        let tol = 4.0 / self.view.zoom as f64;
        let mut out = Vec::new();
        // Cheap reject on the flat list of hit areas (the same test shape_hit starts with).
        let boxes = self.cache.hit_boxes(&self.doc, &mut self.fonts);
        for i in (0..self.doc.shapes().len()).rev() {
            if !boxes[i].inflate(tol, tol).contains(p) {
                continue;
            }
            let s = &self.doc.shapes()[i];
            if !s.visible || (skip_locked && s.locked) {
                continue;
            }
            let s = s.clone();
            if self.shape_hit(&s, p, tol) {
                out.push(s.id);
            }
        }
        out
    }

    /// Whether `p` touches the shape's painted area (groups: any child).
    pub fn shape_hit(&mut self, s: &Shape, p: Point, tol: f64) -> bool {
        if let ShapeKind::Group { children } = &s.kind {
            if !s.outlines.is_empty() && self.cache.update(s, &mut self.fonts).ring_contains(p) {
                return true;
            }
            return children.iter().any(|c| c.visible && self.shape_hit(c, p, tol));
        }
        let c = self.cache.update(s, &mut self.fonts);
        let Some(b) = c.bounds else { return false };
        let sw = crate::render::effect_reach(s);
        if !b.inflate(tol + sw, tol + sw).contains(p) || !c.clip_contains(p) {
            return false;
        }
        if c.ring_contains(p) {
            return true;
        }
        if let ShapeKind::Text(_) = s.kind {
            // Text tools place the caret anywhere in the text box; otherwise
            // only the glyphs (with a little slack) count.
            if self.tool.is_text() || self.text_edit.as_ref().map(|t| t.id) == Some(s.id) {
                return true;
            }
            let c = self.cache.update(s, &mut self.fonts);
            return c.doc_path.winding(p) != 0 || geom::distance_to_path(&c.doc_path, p) <= tol + 2.0 / self.view.zoom as f64 + sw;
        }
        let inside = s.fill.is_some() && {
            let w = c.doc_path.winding(p);
            match s.fill_rule {
                FillRule::NonZero => w != 0,
                FillRule::EvenOdd => w % 2 != 0,
            }
        };
        inside || geom::distance_to_path(&c.doc_path, p) <= tol + sw
    }

    /// Whether a marquee rectangle touches the shape's actual geometry: an
    /// outline crossing or inside the rect, or the rect inside the fill.
    pub fn shape_in_rect(&mut self, s: &Shape, r: Rect) -> bool {
        if let ShapeKind::Group { children } = &s.kind {
            return children.iter().any(|c| c.visible && self.shape_in_rect(c, r));
        }
        let c = self.cache.update(s, &mut self.fonts);
        let Some(b) = c.bounds else { return false };
        if b.intersect(r).area() <= 0.0 && !(b.width() == 0.0 || b.height() == 0.0) {
            return false;
        }
        if r.contains(b.origin()) && r.contains(Point::new(b.x1, b.y1)) {
            return true;
        }
        let path = c.doc_path.clone();
        let fill_rule = s.fill_rule;
        let filled = s.fill.is_some() || matches!(s.kind, ShapeKind::Text(_));
        if filled {
            let w = path.winding(r.center());
            if match fill_rule {
                FillRule::NonZero => w != 0,
                FillRule::EvenOdd => w % 2 != 0,
            } {
                return true;
            }
        }
        geom::path_touches_rect(&path, r, 0.25 / self.view.zoom as f64)
    }

    /// Screen-space selection gizmo: the 4 box corners (NW, NE, SE, SW), the
    /// 8 handles, the rotation knob and the box center.
    fn gizmo(&mut self) -> Option<Gizmo> {
        let (frame, r) = self.sel_frame()?;
        let m = self.view.affine() * frame;
        let sp = |x: f64, y: f64| {
            let q = m * Point::new(x, y);
            pos2(q.x as f32, q.y as f32)
        };
        let (cx, cy) = (r.center().x, r.center().y);
        let corners = [sp(r.x0, r.y0), sp(r.x1, r.y0), sp(r.x1, r.y1), sp(r.x0, r.y1)];
        let handles = [
            (Handle::NW, corners[0]),
            (Handle::N, sp(cx, r.y0)),
            (Handle::NE, corners[1]),
            (Handle::E, sp(r.x1, cy)),
            (Handle::SE, corners[2]),
            (Handle::S, sp(cx, r.y1)),
            (Handle::SW, corners[3]),
            (Handle::W, sp(r.x0, cy)),
        ];
        let center = sp(cx, cy);
        // Local "up" in screen space (falls back to screen up for flat boxes).
        let top = sp(cx, r.y0);
        let up_v = m * Point::new(cx, r.y0 - 1.0) - m * Point::new(cx, r.y0);
        let up = if up_v.hypot() > 1e-9 { vec2(up_v.x as f32, up_v.y as f32).normalized() } else { vec2(0.0, -1.0) };
        let knob = top + up * KNOB_GAP;
        Some(Gizmo { corners, handles, knob, top, center })
    }

    fn selection_handle_at(&mut self, sp: Pos2) -> Option<(Handle, Pos2, Pos2)> {
        let g = self.gizmo()?;
        g.handles.into_iter().find(|(_, hp)| hp.distance(sp) <= HANDLE_R).map(|(h, hp)| (h, hp, g.center))
    }

    fn knob_at(&mut self, sp: Pos2) -> bool {
        self.gizmo().map(|g| g.knob.distance(sp) <= HANDLE_R + 1.0).unwrap_or(false)
    }

    fn pivot_at(&mut self, sp: Pos2) -> bool {
        self.pivot_point().map(|p| self.view.to_screen(p).distance(sp) <= 7.0).unwrap_or(false)
    }

    fn rotate_zone(&mut self, sp: Pos2) -> bool {
        let Some(g) = self.gizmo() else { return false };
        if point_in_quad(sp, &g.corners, 2.0) {
            return false;
        }
        g.corners.iter().any(|c| c.distance(sp) <= 22.0)
    }

    /// Path shapes currently editable with the node tool.
    fn node_targets(&self) -> Vec<u64> {
        self.selection
            .iter()
            .copied()
            .filter(|id| matches!(self.doc.get(*id).map(|s| &s.kind), Some(ShapeKind::Path(_))))
            .collect()
    }

    fn node_selected(&self, id: u64, sub: usize, node: usize) -> bool {
        self.node_sel.contains(&(id, sub, node))
    }

    /// Whether handles of a node should be shown (selected or neighbor of selected).
    fn handles_visible(&self, id: u64, sub: usize, node: usize, sp: &SubPath) -> bool {
        let n = sp.nodes.len();
        if self.node_selected(id, sub, node) {
            return true;
        }
        // Drawing: the node being placed and the one before it (its outgoing
        // handle shapes the new segment), like Figma.
        if let Some(pen) = &self.pen {
            if pen.id == id && pen.sub == sub && node + 2 >= n {
                return true;
            }
        }
        if let Drag::PenHandle { id: d, sub: s, node: k, .. } = &self.drag {
            if *d == id && *s == sub && (*k == node || (*k == 0 && sp.closed && node + 1 == n)) {
                return true;
            }
        }
        let prev = if node == 0 { if sp.closed { Some(n - 1) } else { None } } else { Some(node - 1) };
        let next = if node + 1 == n { if sp.closed { Some(0) } else { None } } else { Some(node + 1) };
        prev.map(|p| self.node_selected(id, sub, p)).unwrap_or(false) || next.map(|q| self.node_selected(id, sub, q)).unwrap_or(false)
    }

    // ------------------------------------------------------------------
    // Tool input dispatch
    // ------------------------------------------------------------------

    fn tool_input(&mut self, ui: &mut Ui, resp: &egui::Response) {
        let inp = ui.input(|i| Input {
            pos: i.pointer.interact_pos().or(i.pointer.hover_pos()),
            pressed: i.pointer.primary_pressed(),
            released: i.pointer.primary_released(),
            down: i.pointer.primary_down(),
            double: i.pointer.button_double_clicked(egui::PointerButton::Primary),
            triple: i.pointer.button_triple_clicked(egui::PointerButton::Primary),
            shift: i.modifiers.shift,
            alt: i.modifiers.alt,
            cmd: i.modifiers.command,
        });
        let space = ui.input(|i| i.key_down(egui::Key::Space)) && self.text_edit.is_none();
        self.cursor_doc = inp.pos.filter(|p| self.canvas_rect.contains(*p)).map(|p| self.view.to_doc(p));

        if self.text_edit.is_some() {
            let events = ui.input(|i| i.events.clone());
            let ctx = ui.ctx().clone();
            self.text_keys(&ctx, &events);
        }
        if self.tool == Tool::ShapeBuilder {
            self.refresh_builder();
        }
        if self.guide_tick(ui.ctx()) {
            return;
        }
        if self.press.is_none() && self.artboard_input(ui, resp) {
            return;
        }
        if self.tool == Tool::Hand || space || matches!(self.drag, Drag::Pan) {
            return;
        }
        let Some(pos) = inp.pos else { return };
        let p = self.view.to_doc(pos);

        // Guides: hover and drag to move (Select tool, outside the selection handles).
        self.guide_hover = None;
        if self.tool == Tool::Select && resp.hovered() && self.press.is_none() && self.selection_handle_at(pos).is_none() {
            self.guide_hover = self.guide_at(pos).filter(|_| self.pick(p, false).map(|id| !self.selection.contains(&id)).unwrap_or(true));
            if let Some(i) = self.guide_hover {
                let g = self.doc.guides[i];
                ui.ctx().set_cursor_icon(if g.vertical { CursorIcon::ResizeColumn } else { CursorIcon::ResizeRow });
                if inp.pressed {
                    self.guide_drag = Some((Some(i), g.vertical, g.pos));
                    return;
                }
            }
        }

        // Pen: preview where the next node goes (snapped).
        if self.tool == Tool::Pen && resp.hovered() && self.press.is_none() {
            self.pen_hover = Some(self.pen_target(p, &inp));
        } else if self.pen_hover.take().is_some() && self.press.is_none() {
            self.snap_guides.clear();
        }

        // Hover state & cursors.
        if resp.hovered() && self.press.is_none() {
            self.hover = if matches!(self.tool, Tool::Select | Tool::Node | Tool::Corner | Tool::Text | Tool::FrameText) { if self.tool == Tool::Select { self.pick(p, inp.cmd) } else { self.hit_shape(p) } } else { None };
            match self.tool {
                Tool::Select => {
                    self.rotate_hover = false;
                    self.shear_hover = None;
                    if self.gradient_handle_at(pos).is_some() || self.prim_handle_at(pos).is_some() {
                        ui.ctx().set_cursor_icon(CursorIcon::Grab);
                    } else if self.pivot_at(pos) {
                        ui.ctx().set_cursor_icon(CursorIcon::Move);
                    } else if let Some((h, hp, c)) = self.selection_handle_at(pos) {
                        ui.ctx().set_cursor_icon(oriented_cursor(h, hp, c));
                    } else if let Some((h, _)) = self.shear_edge_at(pos) {
                        ui.ctx().set_cursor_icon(CursorIcon::None);
                        self.shear_hover = Some(h);
                    } else if self.knob_at(pos) || self.rotate_zone(pos) {
                        ui.ctx().set_cursor_icon(CursorIcon::None);
                        self.rotate_hover = true;
                    }
                }
                Tool::Rect | Tool::Ellipse | Tool::Star | Tool::Polygon | Tool::Prim(_) => {
                    ui.ctx().set_cursor_icon(if self.prim_handle_at(pos).is_some() { CursorIcon::Grab } else { CursorIcon::Crosshair });
                }
                Tool::Fill => ui.ctx().set_cursor_icon(if self.gradient_handle_at(pos).is_some() { CursorIcon::Grab } else { CursorIcon::Crosshair }),
                Tool::Pen | Tool::Eyedropper | Tool::Knife | Tool::Pencil | Tool::Brush => ui.ctx().set_cursor_icon(CursorIcon::Crosshair),
                Tool::Zoom => ui.ctx().set_cursor_icon(if ui.input(|i| i.modifiers.alt) { CursorIcon::ZoomOut } else { CursorIcon::ZoomIn }),
                Tool::Text | Tool::FrameText => {
                    let over_text = self.hover.and_then(|h| self.doc.get(h)).map(|s| matches!(s.kind, ShapeKind::Text(_))).unwrap_or(false);
                    ui.ctx().set_cursor_icon(if over_text { CursorIcon::Text } else { CursorIcon::Crosshair });
                }
                Tool::ShapeBuilder => {
                    self.builder_hover = self.builder.as_ref().and_then(|b| boolean::region_at(&b.regions, p));
                    if self.builder_hover.is_some() {
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                }
                _ => {}
            }
        }

        // Word / paragraph selection inside the edited text.
        let editing_hit = self.text_edit.as_ref().map(|t| t.id).filter(|id| self.hit_shape(p) == Some(*id));
        if let (Some(id), true) = (editing_hit, (inp.double || inp.triple) && resp.hovered()) {
            self.text_mouse(id, p, false, if inp.triple { 3 } else { 2 });
        } else if inp.double && resp.hovered() && !(inp.alt && self.tool == Tool::Select) {
            // Quick Alt+clicks step through layers; they aren't a double-click.
            self.on_double_click(p, pos);
        }
        if inp.pressed && resp.hovered() {
            self.press = Some(pos);
            self.dragging = false;
            self.on_press(p, pos, &inp);
        }
        if let Some(start) = self.press {
            if inp.down {
                if !self.dragging && start.distance(pos) > 3.0 {
                    self.dragging = true;
                    self.on_drag_start(&inp);
                }
                if self.dragging {
                    self.on_drag(p, &inp);
                }
            }
            if inp.released || !inp.down {
                self.snap_guides.clear();
                self.gap_guides.clear();
                self.on_release(p, &inp);
                self.press = None;
                self.dragging = false;
                self.drag = Drag::None;
            }
        }
    }

    fn on_press(&mut self, p: Point, sp: Pos2, inp: &Input) {
        match self.tool {
            Tool::Select => self.select_press(p, sp, inp),
            Tool::Node => self.node_press(p, sp, inp),
            Tool::Pen => self.pen_press(p, sp, inp),
            Tool::Rect | Tool::Ellipse | Tool::Star | Tool::Polygon | Tool::Prim(_) => {
                if let Some((id, i)) = self.prim_handle_at(sp) {
                    self.drag = Drag::PrimHandle { id, i };
                } else {
                    self.create_press(p, inp.cmd);
                }
            }
            Tool::Fill => self.fill_press(p, sp, inp.cmd),
            Tool::Text | Tool::FrameText => self.text_press(p, inp.shift),
            Tool::ShapeBuilder => {
                if let Some(b) = &self.builder {
                    let r = boolean::region_at(&b.regions, p);
                    self.drag = Drag::Builder {
                        marked: r.into_iter().collect(),
                        mode: if inp.alt { BuilderMode::Subtract } else { self.builder_mode },
                    };
                } else if let Some(id) = self.hit_shape(p) {
                    if inp.shift {
                        if !self.selection.contains(&id) {
                            self.selection.push(id);
                        }
                    } else {
                        self.selection = vec![id];
                    }
                }
            }
            Tool::Corner => self.corner_press(p, sp, inp),
            Tool::Eyedropper => self.eyedropper_click(p, inp.shift),
            Tool::Knife => {
                self.start_stabilizer(p);
                self.drag = Drag::Knife { pts: vec![p] };
            }
            Tool::Pencil | Tool::Brush => {
                self.start_stabilizer(p);
                let t = self.input_time;
                self.drag = Drag::Freehand { pts: vec![p], times: vec![t], pressure: vec![self.pen_pressure], live: None };
            }
            Tool::Zoom => self.drag = Drag::ZoomRect { start: p },
            Tool::Hand | Tool::Artboard => {}
        }
    }

    fn on_drag_start(&mut self, inp: &Input) {
        if let Drag::Create { start, id: 0 } = self.drag {
            let id = self.begin_create(start);
            self.drag = Drag::Create { start, id };
            return;
        }
        match &self.drag {
            Drag::Move { .. } => {
                self.checkpoint("");
                if inp.alt {
                    // Alt-drag duplicates the selection and moves the copies.
                    let src = self.selection_bounds();
                    let mut new_sel = Vec::new();
                    for id in self.selection.clone() {
                        if let Some(orig) = self.doc.get(id).cloned() {
                            let mut s = orig.clone();
                            s.id = 0;
                            let idx = self.doc.index_of(id).map(|i| i + 1).unwrap_or(usize::MAX);
                            let nid = self.doc.insert_shape(idx, s);
                            self.instance_from_copy(&orig, nid);
                            new_sel.push(nid);
                        }
                    }
                    self.selection = new_sel;
                    // Ctrl+D afterwards repeats this move.
                    self.last_dup = src.map(|b| (self.selection.clone(), b));
                    let originals = self.selected_shapes().into_iter().cloned().collect();
                    if let Drag::Move { originals: o, .. } = &mut self.drag {
                        *o = originals;
                    }
                }
            }
            Drag::Scale { .. }
            | Drag::Rotate { .. }
            | Drag::Nodes { .. }
            | Drag::Handle { .. }
            | Drag::Bend { .. }
            | Drag::Corner { .. }
            | Drag::Gradient { .. }
            | Drag::PrimHandle { .. }
            | Drag::FillLay { .. }
            | Drag::Shear { .. } => self.checkpoint(""),
            _ => {}
        }
    }

    fn on_drag(&mut self, p: Point, inp: &Input) {
        let mut drag = std::mem::replace(&mut self.drag, Drag::None);
        // Alt at any point of a handle drag breaks the handles apart (the
        // node turns sharp) for the rest of the drag, like Figma.
        if let (true, Drag::Handle { break_smooth, .. }) = (inp.alt, &mut drag) {
            *break_smooth = true;
        }
        match &drag {
            Drag::Move { start, originals } => {
                let mut d = p - *start;
                if inp.shift {
                    if d.x.abs() > d.y.abs() {
                        d.y = 0.0;
                    } else {
                        d.x = 0.0;
                    }
                }
                self.snap_guides.clear();
                self.gap_guides.clear();
                if self.snap && !inp.cmd {
                    d = self.snap_move(originals, d);
                }
                let a = Affine::translate(d);
                for o in originals {
                    let mut s = o.clone();
                    s.apply_transform(a);
                    self.doc.replace(s);
                }
            }
            Drag::Gradient { id, part } => {
                // End points snap to geometry (Shift constrains, Ctrl skips snapping).
                use crate::gradient_handles::GradPart;
                self.snap_guides.clear();
                let ends = matches!(part, GradPart::Start | GradPart::End);
                let p = if ends && !inp.shift && !inp.cmd { self.snap_point(p, &[]) } else { p };
                let part = self.drag_gradient(*id, *part, p, inp.shift);
                self.drag = Drag::Gradient { id: *id, part };
                return;
            }
            Drag::PrimHandle { id, i } => self.drag_prim_handle(*id, *i, p),
            Drag::FillLay { start, targets } => {
                self.snap_guides.clear();
                let end = if inp.shift {
                    constrain_45(*start, p)
                } else if !inp.cmd {
                    self.snap_point(p, &[])
                } else {
                    p
                };
                self.lay_gradient(targets, *start, end);
            }
            Drag::Knife { pts } => {
                let mut pts = pts.clone();
                if inp.shift {
                    pts.truncate(1);
                    pts.push(p);
                } else if let Some(q) = self.stabilized(p) {
                    if pts.last().map(|l| (*l - q).hypot() * self.view.zoom as f64 > 2.0).unwrap_or(true) {
                        pts.push(q);
                    }
                }
                self.drag = Drag::Knife { pts };
                return;
            }
            Drag::Freehand { .. } => {
                if let Drag::Freehand { pts, times, pressure, live } = &mut drag {
                    if let Some(p) = self.stabilized(p) {
                        if pts.last().is_none_or(|q| (*q - p).hypot() * self.view.zoom as f64 > 1.5) {
                            pts.push(p);
                            times.push(self.input_time);
                            pressure.push(self.pen_pressure);
                            // Draw the real stroke as it grows.
                            let shape = self.freehand_shape(pts, times, pressure);
                            self.freehand_live(live, shape);
                        }
                    }
                }
                self.drag = drag;
                return;
            }
            Drag::TextSelect { id } => {
                if let Some(i) = self.caret_at(*id, p) {
                    if let Some(te) = self.text_edit.as_mut().filter(|t| t.id == *id) {
                        te.cursor = i;
                    }
                }
            }
            Drag::Pivot => {
                // Snap to the box's anchors and center, else to geometry
                // (nodes, outlines) and alignment lines.
                if let Some((frame, r)) = self.sel_frame() {
                    let lp = frame.inverse() * p;
                    let tol = 8.0 / self.view.zoom as f64;
                    let mut anchor = None;
                    for gx in [0.0, 0.5, 1.0] {
                        for gy in [0.0, 0.5, 1.0] {
                            let q = Point::new(r.x0 + r.width() * gx, r.y0 + r.height() * gy);
                            if self.snap && (q - lp).hypot() <= tol {
                                anchor = Some((gx, gy));
                            }
                        }
                    }
                    let f = match anchor {
                        Some(f) => {
                            self.snap_guides.clear();
                            f
                        }
                        None => {
                            let lp = frame.inverse() * self.snap_pivot(p);
                            ((lp.x - r.x0) / r.width().max(1e-9), (lp.y - r.y0) / r.height().max(1e-9))
                        }
                    };
                    self.pivot = Some(f);
                    self.pivot_for = self.selection.clone();
                }
            }
            Drag::Scale { handle, bbox, frame, originals } => {
                let ids: Vec<u64> = originals.iter().map(|o| o.id).collect();
                let p = self.snap_scale_handle(p, *handle, *bbox, *frame, inp, &ids);
                let inv = frame.inverse();
                let a = *frame * scale_affine(*handle, *bbox, inv * p, inp.shift != self.ui.lock_aspect, inp.alt) * inv;
                for o in originals {
                    let mut s = o.clone();
                    self.resize_shape(&mut s, a);
                    self.doc.replace(s);
                }
            }
            Drag::Rotate { center, start_angle, originals } => {
                let mut ang = (p - *center).atan2() - start_angle;
                if inp.shift {
                    // Snap the resulting absolute angle (relative to the artboard).
                    let base = rotation_base(originals);
                    let step = std::f64::consts::PI / 12.0;
                    ang = ((base + ang) / step).round() * step - base;
                }
                let a = Affine::rotate_about(ang, *center);
                for o in originals {
                    let mut s = o.clone();
                    s.apply_transform(a);
                    self.doc.replace(s);
                }
            }
            Drag::Create { start, id } => {
                let p = self.snap_pointer(p, inp, &[*id]);
                let r = drag_rect(*start, p, inp.shift, inp.alt);
                if let Some(s) = self.doc.get_mut(*id) {
                    s.kind.set_size(r.width().max(1.0), r.height().max(1.0));
                    s.transform = Affine::translate(r.origin().to_vec2());
                }
            }
            Drag::Nodes { start, original } => {
                let mut d = p - *start;
                // Snap the grabbed node (the selected node nearest the press).
                let grabbed = original
                    .iter()
                    .filter_map(|o| match &o.kind {
                        ShapeKind::Path(pd) => Some((o.id, pd)),
                        _ => None,
                    })
                    .flat_map(|(id, pd)| pd.subpaths.iter().enumerate().flat_map(move |(si, sp)| sp.nodes.iter().enumerate().map(move |(ni, n)| ((id, si, ni), n.p))))
                    .filter(|(k, _)| self.node_sel.contains(k))
                    .min_by(|a, b| (a.1 - *start).hypot().total_cmp(&(b.1 - *start).hypot()))
                    .map(|(_, p)| p);
                if inp.shift {
                    // Shift: straight moves (horizontal, vertical or 45°).
                    d = constrain_45(Point::ZERO, d.to_point()).to_vec2();
                } else if let (Some(n0), true) = (grabbed, self.snap) {
                    let ids: Vec<u64> = original.iter().map(|o| o.id).collect();
                    d = self.snap_pointer(n0 + d, inp, &ids) - n0;
                }
                for o in original {
                    let mut s = o.clone();
                    if let ShapeKind::Path(pd) = &mut s.kind {
                        for (si, sp) in pd.subpaths.iter_mut().enumerate() {
                            for (ni, n) in sp.nodes.iter_mut().enumerate() {
                                if self.node_sel.contains(&(o.id, si, ni)) {
                                    n.transform(Affine::translate(d));
                                }
                            }
                        }
                    }
                    self.doc.replace(s);
                }
            }
            Drag::Handle { id, sub, node, is_out, original, break_smooth } => {
                let mut s = original.clone();
                if let ShapeKind::Path(pd) = &mut s.kind {
                    if let Some(n) = pd.subpaths.get_mut(*sub).and_then(|sp| sp.nodes.get_mut(*node)) {
                        if *is_out {
                            n.hout = Some(p);
                        } else {
                            n.hin = Some(p);
                        }
                        if *break_smooth {
                            n.smooth = false;
                        } else if n.smooth {
                            geom::mirror_handle(n, *is_out);
                        }
                    }
                }
                let _ = id;
                self.doc.replace(s);
            }
            Drag::Bend { sub, seg, t, start, original, .. } => {
                let mut s = original.clone();
                if let ShapeKind::Path(pd) = &mut s.kind {
                    if let Some(sp) = pd.subpaths.get_mut(*sub) {
                        geom::bend_segment(sp, *seg, *t, p - *start);
                    }
                }
                self.doc.replace(s);
            }
            Drag::PenHandle { id, sub, node, out_only } => {
                if let Some(s) = self.doc.get_mut(*id) {
                    if let ShapeKind::Path(pd) = &mut s.kind {
                        if let Some(n) = pd.subpaths.get_mut(*sub).and_then(|sp| sp.nodes.get_mut(*node)) {
                            // Shift: handle at 45° steps.
                            let h = if inp.shift { constrain_45(n.p, p) } else { p };
                            n.hout = Some(h);
                            if *out_only || inp.alt {
                                // Broken handles: the incoming one stays put.
                                n.smooth = false;
                            } else {
                                n.hin = Some(n.p - (h - n.p));
                                n.smooth = true;
                            }
                        }
                    }
                }
            }
            Drag::Corner { id, corner, single, original } => {
                let mut s = original.clone();
                set_corner_from_drag(&mut s, *corner, *single, p, &self.node_sel);
                let _ = id;
                self.doc.replace(s);
            }
            Drag::Shear { .. } => {
                if let Drag::Shear { horizontal, edge, frame, bbox, start, originals } = &drag {
                    let d = frame.inverse() * p - *start;
                    let local = shear_xf(*bbox, *horizontal, Some(*edge), *start, d, inp.alt, inp.shift);
                    let xf = *frame * local * frame.inverse();
                    for o in originals.iter() {
                        let mut s = o.clone();
                        s.apply_transform(xf);
                        self.doc.replace(s);
                    }
                    self.shear_angle = Some(shear_angle(local, *horizontal));
                }
                self.drag = drag;
                return;
            }
            Drag::Builder { .. } => {
                let r = self.builder.as_ref().and_then(|b| boolean::region_at(&b.regions, p));
                if let Drag::Builder { mut marked, mode } = drag {
                    if let Some(r) = r {
                        if !marked.contains(&r) {
                            marked.push(r);
                        }
                    }
                    self.drag = Drag::Builder { marked, mode: if inp.alt { BuilderMode::Subtract } else { mode } };
                    return;
                }
            }
            _ => {}
        }
        self.drag = drag;
    }

    fn on_release(&mut self, p: Point, inp: &Input) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        if let Some(id) = self.press_top.take().filter(|_| !self.dragging) {
            self.selection = vec![id];
        }
        if matches!(drag, Drag::Pivot) {
            self.snap_guides.clear();
        }
        match drag {
            Drag::ZoomRect { start } => {
                let r = Rect::from_points(start, p);
                let sp = self.view.to_screen(p);
                if self.dragging && r.width() * self.view.zoom as f64 > 4.0 && r.height() * self.view.zoom as f64 > 4.0 {
                    self.zoom_to_rect(r, 0.0);
                } else {
                    self.zoom_at(if inp.alt { 0.5 } else { 2.0 }, sp);
                }
            }
            Drag::Freehand { mut pts, mut times, mut pressure, mut live } if self.dragging => {
                let end = match self.stab.as_mut() {
                    Some(st) => st.finish(p),
                    None => Some(p),
                };
                if let Some(p) = end {
                    pts.push(p);
                    times.push(self.input_time);
                    pressure.push(self.pen_pressure);
                }
                let shape = self.freehand_shape(&pts, &times, &pressure);
                self.freehand_live(&mut live, shape);
                if let Some(id) = live {
                    self.selection = vec![id];
                }
                self.stab = None;
            }
            Drag::Freehand { live: Some(id), .. } => {
                // A click: drop whatever was drawn.
                self.doc.remove(id);
                self.stab = None;
            }
            Drag::Knife { mut pts } if self.dragging => {
                if inp.shift {
                    pts.truncate(1);
                    pts.push(p);
                } else if let Some(q) = self.stab.as_mut().map_or(Some(p), |st| st.finish(p)) {
                    pts.push(q);
                }
                self.stab = None;
                let pts = geom::smooth_polyline(&pts, self.knife_smooth);
                if self.knife_cut(&pts) == 0 {
                    self.notify("The cut didn't cross a filled shape");
                }
            }
            Drag::Marquee { start, additive } if self.dragging => {
                let r = Rect::from_points(start, p);
                let mut sel: Vec<u64> = if additive { self.selection.clone() } else { Vec::new() };
                for s in self.doc.shapes().to_vec() {
                    if !s.visible || s.locked {
                        continue;
                    }
                    if self.shape_in_rect(&s, r) && !sel.contains(&s.id) {
                        sel.push(s.id);
                    }
                }
                self.selection = sel;
            }
            Drag::NodeMarquee { start, additive } if self.dragging => {
                let r = Rect::from_points(start, p);
                if !additive {
                    self.node_sel.clear();
                }
                for id in self.node_targets() {
                    if let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) {
                        for (si, sp) in pd.subpaths.iter().enumerate() {
                            for (ni, n) in sp.nodes.iter().enumerate() {
                                if r.contains(n.p) && !self.node_sel.contains(&(id, si, ni)) {
                                    self.node_sel.push((id, si, ni));
                                }
                            }
                        }
                    }
                }
            }
            Drag::FillLay { .. } if !self.dragging => {
                // A click picks the shape to edit.
                match self.pick(p, false) {
                    Some(hit) => self.selection = vec![hit],
                    None => self.selection.clear(),
                }
            }
            Drag::Create { id, .. } => {
                if !self.dragging || id == 0 {
                    // A click: select what is under the pointer (Shift adds).
                    match self.pick(p, false) {
                        Some(hit) if inp.shift => {
                            if let Some(i) = self.selection.iter().position(|x| *x == hit) {
                                self.selection.remove(i);
                            } else {
                                self.selection.push(hit);
                            }
                        }
                        Some(hit) => self.selection = vec![hit],
                        None if !inp.shift => self.selection.clear(),
                        None => {}
                    }
                    self.node_sel.clear();
                }
            }
            Drag::Move { originals, .. } if self.dragging => {
                let ids: Vec<u64> = originals.iter().map(|s| s.id).collect();
                if !self.layout_drop(&ids, p) {
                    let before: Vec<(u64, Option<u64>)> = originals.iter().map(|s| (s.id, self.member_of(s))).collect();
                    self.rehome(&before);
                }
            }
            Drag::Builder { marked, mode } => {
                if !marked.is_empty() {
                    self.commit_builder(&marked, mode);
                }
            }
            Drag::TextFrame { start } => {
                let r = drag_rect(start, p, inp.shift, inp.alt);
                let min = 8.0 / self.view.zoom as f64;
                let artistic = self.tool == Tool::Text;
                if artistic && self.dragging && (p.y - start.y).abs() > min {
                    // Artistic text: the drag height sets the font size.
                    let (size, top) = artistic_drag(start, p, self.default_text.line_height);
                    self.default_text.size = size;
                    self.create_text(Point::new(start.x, top), None);
                } else if !artistic && self.dragging && r.width() > min && r.height() > min {
                    self.create_text(r.origin(), Some([r.width(), r.height()]));
                } else if self.tool == Tool::FrameText {
                    self.create_text(start, Some([320.0, 160.0]));
                } else {
                    let lh = self.default_text.size * self.default_text.line_height;
                    self.create_text(Point::new(start.x, start.y - lh / 2.0), None);
                }
            }
            other => self.drag = other,
        }
        self.drag = Drag::None;
    }

    fn on_double_click(&mut self, p: Point, sp: Pos2) {
        if self.mesh_slice(p) {
            return;
        }
        if self.tool == Tool::Fill {
            self.gradient_add_stop(sp);
            return;
        }
        match self.tool {
            Tool::Select => {
                if let Some(id) = self.pick(p, false) {
                    if self.doc.get(id).map(|s| s.kind.is_container()).unwrap_or(false) {
                        // Enter the group / boolean: select the child under the pointer.
                        if let Some(c) = self.hit_child(id, p) {
                            self.selection = vec![c];
                        }
                        return;
                    }
                    match self.doc.get(id).map(|s| &s.kind) {
                        Some(ShapeKind::Text(_)) => self.begin_text_edit(id),
                        Some(ShapeKind::Path(_)) => {
                            self.selection = vec![id];
                            self.set_tool(Tool::Node);
                        }
                        Some(_) => {
                            // Like Figma: editing a primitive's nodes turns it into a path.
                            self.selection = vec![id];
                            let ctx = self.egui_ctx.clone();
                            crate::commands::Cmd::ConvertToCurves.run(self, &ctx);
                            if matches!(self.selection.first().and_then(|i| self.doc.get(*i)).map(|s| &s.kind), Some(ShapeKind::Path(_))) {
                                self.set_tool(Tool::Node);
                                self.notify("Converted to a path for node editing — Undo restores the shape");
                            }
                        }
                        None => {}
                    }
                }
            }
            Tool::Node => {
                // Double-click node toggles smooth / sharp; on segment inserts node.
                if let Some((id, si, ni, _)) = self.node_hit(sp) {
                    self.checkpoint("");
                    if let Some(s) = self.doc.get_mut(id) {
                        if let ShapeKind::Path(pd) = &mut s.kind {
                            let n = &mut pd.subpaths[si].nodes[ni];
                            if n.hin.is_some() || n.hout.is_some() {
                                n.hin = None;
                                n.hout = None;
                                n.smooth = false;
                            } else {
                                make_smooth(&mut pd.subpaths[si], ni);
                            }
                        }
                    }
                    return;
                }
                let tol = 6.0 / self.view.zoom as f64;
                for id in self.node_targets() {
                    let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) else { continue };
                    if let Some((si, gi, t, d)) = geom::nearest_segment(pd, p) {
                        if d <= tol {
                            self.checkpoint("");
                            if let Some(s) = self.doc.get_mut(id) {
                                if let ShapeKind::Path(pd) = &mut s.kind {
                                    geom::insert_node(&mut pd.subpaths[si], gi, t);
                                    self.node_sel = vec![(id, si, gi + 1)];
                                }
                            }
                            return;
                        }
                    }
                }
                if self.hit_shape(p).is_none() {
                    self.set_tool(Tool::Select);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Select tool
    // ------------------------------------------------------------------

    /// Whether a selected shape is under `p`, even behind other shapes.
    fn selection_under(&mut self, p: Point) -> bool {
        let hits = self.hit_shapes(p, true);
        self.selection.clone().into_iter().any(|id| match self.doc.parent_of(id) {
            Some(par) => self.hit_child(par, p) == Some(id),
            None => hits.contains(&id),
        })
    }

    /// Layers of `s` under `p`, top-most first, looking inside groups.
    fn hit_layers(&mut self, s: &Shape, p: Point, tol: f64, out: &mut Vec<u64>) {
        if !self.shape_hit(s, p, tol) {
            return;
        }
        let found = out.len();
        if let ShapeKind::Group { children } = &s.kind {
            for c in children.iter().rev().filter(|c| c.visible && !c.locked) {
                self.hit_layers(c, p, tol, out);
            }
        }
        // Not a group, or hit only through the group's own outline.
        if out.len() == found {
            out.push(s.id);
        }
    }

    /// The next layer down at `p` below the selected one, wrapping to the
    /// top. A selected group steps to its top layer here; with nothing
    /// selected under `p`, the top-most layer.
    fn layer_beneath(&mut self, p: Point) -> Option<u64> {
        let tol = 4.0 / self.view.zoom as f64;
        let mut stack = Vec::new();
        for id in self.hit_shapes(p, true) {
            if let Some(s) = self.doc.get(id).cloned() {
                self.hit_layers(&s, p, tol, &mut stack);
            }
        }
        // An auto layout's background stands for the layout itself.
        let mut layers: Vec<u64> = Vec::with_capacity(stack.len());
        for id in stack {
            let id = match self.doc.parent_of(id) {
                Some(g) if self.doc.get(g).and_then(|s| s.layout.as_ref()).and_then(|l| l.background) == Some(id) => g,
                _ => id,
            };
            if !layers.contains(&id) {
                layers.push(id);
            }
        }
        let in_selected_group = |h: u64| {
            let mut parent = self.doc.parent_of(h);
            while let Some(x) = parent {
                if self.selection.contains(&x) {
                    return true;
                }
                parent = self.doc.parent_of(x);
            }
            false
        };
        let next = match layers.iter().position(|h| self.selection.contains(h)) {
            Some(i) => (i + 1) % layers.len(),
            None => layers.iter().position(|h| in_selected_group(*h)).unwrap_or(0),
        };
        layers.get(next).copied()
    }

    fn select_press(&mut self, p: Point, sp: Pos2, inp: &Input) {
        self.press_top = None;
        if let Some((id, part)) = self.gradient_handle_at(sp) {
            self.select_grad_part(id, part);
            self.drag = Drag::Gradient { id, part };
            return;
        }
        if let Some((id, i)) = self.prim_handle_at(sp) {
            self.drag = Drag::PrimHandle { id, i };
            return;
        }
        // Clicking elsewhere ends gradient editing.
        self.ui.gradient_edit = None;
        // Alt+click: the layer beneath the selected one (a drag duplicates).
        let beneath = if inp.alt && !inp.shift && !inp.cmd { self.layer_beneath(p) } else { None };
        self.press_top = beneath;
        if !self.selection.is_empty() {
            if self.pivot_at(sp) {
                self.drag = Drag::Pivot;
                return;
            }
            if let Some((h, _, _)) = self.selection_handle_at(sp) {
                let (frame, bbox) = self.sel_frame().unwrap();
                self.drag = Drag::Scale { handle: h, bbox, frame, originals: self.selected_shapes().into_iter().cloned().collect() };
                return;
            }
            if let Some((horizontal, edge)) = self.shear_edge_at(sp) {
                let (frame, bbox) = self.sel_frame().unwrap();
                self.shear_angle = None;
                let start = frame.inverse() * p;
                self.drag = Drag::Shear { horizontal, edge, frame, bbox, start, originals: self.selected_shapes().into_iter().cloned().collect() };
                return;
            }
            if self.knob_at(sp) || self.rotate_zone(sp) {
                let c = self.pivot_point().unwrap();
                self.drag = Drag::Rotate {
                    center: c,
                    start_angle: (p - c).atan2(),
                    originals: self.selected_shapes().into_iter().cloned().collect(),
                };
                return;
            }
        }
        match self.pick(p, inp.cmd) {
            Some(id) => {
                if inp.shift {
                    if let Some(i) = self.selection.iter().position(|s| *s == id) {
                        self.selection.remove(i);
                        return;
                    }
                    self.selection.push(id);
                } else if !self.selection.contains(&id) {
                    if !inp.cmd && self.selection_under(p) {
                        // Covered by another shape: a drag moves the
                        // selection, a plain click selects what's on top.
                        self.press_top = beneath.or(Some(id));
                    } else {
                        self.selection = vec![id];
                    }
                }
                self.drag = Drag::Move { start: p, originals: self.selected_shapes().into_iter().cloned().collect() };
            }
            None => {
                if !inp.shift {
                    self.selection.clear();
                }
                self.drag = Drag::Marquee { start: p, additive: inp.shift };
            }
        }
    }

    // ------------------------------------------------------------------
    // Node tool
    // ------------------------------------------------------------------

    /// Node under the screen position: (shape, subpath, node, distance).
    fn node_hit(&self, sp: Pos2) -> Option<(u64, usize, usize, f32)> {
        let mut best = None;
        for id in self.node_targets() {
            let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) else { continue };
            for (si, sub) in pd.subpaths.iter().enumerate() {
                for (ni, n) in sub.nodes.iter().enumerate() {
                    let d = self.view.to_screen(n.p).distance(sp);
                    if d <= HANDLE_R && best.map(|b: (u64, usize, usize, f32)| d < b.3).unwrap_or(true) {
                        best = Some((id, si, ni, d));
                    }
                }
            }
        }
        best
    }

    fn handle_hit(&self, sp: Pos2) -> Option<(u64, usize, usize, bool)> {
        for id in self.node_targets() {
            let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) else { continue };
            for (si, sub) in pd.subpaths.iter().enumerate() {
                for (ni, n) in sub.nodes.iter().enumerate() {
                    if !self.handles_visible(id, si, ni, sub) {
                        continue;
                    }
                    for (h, is_out) in [(n.hout, true), (n.hin, false)] {
                        if let Some(h) = h {
                            if self.view.to_screen(h).distance(sp) <= HANDLE_R {
                                return Some((id, si, ni, is_out));
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn node_press(&mut self, p: Point, sp: Pos2, inp: &Input) {
        if let Some((id, si, ni, is_out)) = self.handle_hit(sp) {
            if let Some(orig) = self.doc.get(id).cloned() {
                self.drag = Drag::Handle { id, sub: si, node: ni, is_out, original: orig, break_smooth: inp.alt };
            }
            return;
        }
        if let Some((id, si, ni, _)) = self.node_hit(sp) {
            let key = (id, si, ni);
            if inp.cmd {
                // Bend (Ctrl): the node turns sharp; dragging pulls out new
                // mirrored handles, like Figma.
                self.checkpoint("");
                if let Some(ShapeKind::Path(pd)) = self.doc.get_mut(id).map(|s| &mut s.kind) {
                    let n = &mut pd.subpaths[si].nodes[ni];
                    n.hin = None;
                    n.hout = None;
                    n.smooth = false;
                }
                self.node_sel = vec![key];
                self.drag = Drag::PenHandle { id, sub: si, node: ni, out_only: false };
                return;
            }
            if inp.shift {
                if let Some(i) = self.node_sel.iter().position(|k| *k == key) {
                    self.node_sel.remove(i);
                    return;
                }
                self.node_sel.push(key);
            } else if !self.node_sel.contains(&key) {
                self.node_sel = vec![key];
            }
            let original = self.node_targets().iter().filter_map(|id| self.doc.get(*id).cloned()).collect();
            self.drag = Drag::Nodes { start: p, original };
            return;
        }
        let tol = 6.0 / self.view.zoom as f64;
        for id in self.node_targets() {
            let Some(s) = self.doc.get(id) else { continue };
            let ShapeKind::Path(pd) = &s.kind else { continue };
            if let Some((si, gi, t, d)) = geom::nearest_segment(pd, p) {
                if d <= tol {
                    // Like Figma: dragging a segment moves it as is (its two
                    // nodes with their handles); Ctrl+drag bends the curve.
                    if inp.cmd {
                        if !inp.shift {
                            self.node_sel.clear();
                        }
                        self.drag = Drag::Bend { sub: si, seg: gi, t, start: p, original: s.clone() };
                        return;
                    }
                    let sub = &pd.subpaths[si];
                    let ends = [(id, si, gi), (id, si, (gi + 1) % sub.nodes.len().max(1))];
                    if !inp.shift {
                        self.node_sel.clear();
                    }
                    for k in ends {
                        if !self.node_sel.contains(&k) {
                            self.node_sel.push(k);
                        }
                    }
                    let original = self.node_targets().iter().filter_map(|id| self.doc.get(*id).cloned()).collect();
                    self.drag = Drag::Nodes { start: p, original };
                    return;
                }
            }
        }
        if let Some(id) = self.hit_shape(p) {
            if !self.selection.contains(&id) {
                self.selection = vec![id];
                self.node_sel.clear();
            }
            if self.doc.get(id).map(|s| !matches!(s.kind, ShapeKind::Path(_))).unwrap_or(false) {
                self.notify("Convert to Curves (Ctrl+Enter) to edit the nodes of this shape");
            }
            return;
        }
        if !inp.shift {
            self.node_sel.clear();
        }
        self.drag = Drag::NodeMarquee { start: p, additive: inp.shift };
    }

    pub fn delete_nodes(&mut self) {
        if self.node_sel.is_empty() {
            return;
        }
        self.checkpoint("");
        let mut sel = std::mem::take(&mut self.node_sel);
        sel.sort();
        sel.reverse();
        for (id, si, ni) in sel {
            let mut remove_shape = false;
            if let Some(s) = self.doc.get_mut(id) {
                if let ShapeKind::Path(pd) = &mut s.kind {
                    if let Some(sp) = pd.subpaths.get_mut(si) {
                        if ni < sp.nodes.len() {
                            sp.nodes.remove(ni);
                        }
                        if sp.nodes.len() < 2 {
                            pd.subpaths.remove(si);
                        }
                    }
                    remove_shape = pd.subpaths.is_empty();
                }
            }
            if remove_shape {
                self.doc.remove(id);
                self.selection.retain(|x| *x != id);
            }
        }
    }

    /// Set the selected nodes to smooth (true) or sharp (false).
    pub fn set_nodes_smooth(&mut self, smooth: bool) {
        if self.node_sel.is_empty() {
            return;
        }
        self.checkpoint("");
        for (id, si, ni) in self.node_sel.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                if let ShapeKind::Path(pd) = &mut s.kind {
                    if let Some(sp) = pd.subpaths.get_mut(si) {
                        if ni >= sp.nodes.len() {
                            continue;
                        }
                        if smooth {
                            make_smooth(sp, ni);
                        } else {
                            let n = &mut sp.nodes[ni];
                            n.hin = None;
                            n.hout = None;
                            n.smooth = false;
                        }
                    }
                }
            }
        }
    }

    pub fn toggle_closed(&mut self) {
        let targets = self.node_targets();
        if targets.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in targets {
            if let Some(s) = self.doc.get_mut(id) {
                if let ShapeKind::Path(pd) = &mut s.kind {
                    for sp in &mut pd.subpaths {
                        sp.closed = !sp.closed;
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Pen tool
    // ------------------------------------------------------------------

    /// Where a pen click at `p` places its node: Shift constrains to 45°
    /// from the previous node, otherwise it snaps (leaving the snap guides
    /// set for the hover preview).
    fn pen_target(&mut self, p: Point, inp: &Input) -> Point {
        let prev = self.pen.as_ref().and_then(|pen| match self.doc.get(pen.id).map(|s| &s.kind) {
            Some(ShapeKind::Path(pd)) => pd.subpaths.get(pen.sub).and_then(|s| s.nodes.last()).map(|n| n.p),
            _ => None,
        });
        if let (Some(q), true) = (prev, inp.shift) {
            self.snap_guides.clear();
            constrain_45(q, p)
        } else if self.snap && !inp.cmd {
            let ex: Vec<u64> = self.pen.as_ref().map(|p| p.id).into_iter().collect();
            self.snap_point(p, &ex)
        } else {
            self.snap_guides.clear();
            p
        }
    }

    fn pen_press(&mut self, p: Point, sp: Pos2, inp: &Input) {
        let p = self.pen_target(p, inp);
        self.snap_guides.clear();
        self.pen_hover = None;
        if let Some(pen) = &self.pen {
            let (id, sub) = (pen.id, pen.sub);
            let info = match self.doc.get(id).map(|s| &s.kind) {
                Some(ShapeKind::Path(pd)) => pd.subpaths.get(sub).map(|s| (s.nodes.len(), s.nodes.first().map(|n| n.p), s.nodes.last().map(|n| n.p))),
                _ => None,
            };
            let Some((n, first, last)) = info else {
                self.pen = None;
                return;
            };
            self.checkpoint("pen");
            self.history.break_coalescing();
            if let Some(f) = first {
                if n >= 2 && self.view.to_screen(f).distance(sp) <= PEN_CLOSE_R {
                    if let Some(s) = self.doc.get_mut(id) {
                        if let ShapeKind::Path(pd) = &mut s.kind {
                            pd.subpaths[sub].closed = true;
                        }
                        if s.fill.is_none() {
                            s.fill = Some(Paint::Solid(Rgba::from_hex("#D9D9D9").unwrap()));
                        }
                    }
                    self.pen = None;
                    self.selection = vec![id];
                    // Dragging now curves the closing segment (first node's handles).
                    self.drag = Drag::PenHandle { id, sub, node: 0, out_only: false };
                    return;
                }
            }
            // Like Figma: clicking the last node again drops its outgoing
            // handle (the next segment starts straight); dragging re-pulls it.
            if let Some(l) = last {
                if self.view.to_screen(l).distance(sp) <= 6.0 {
                    if let Some(ShapeKind::Path(pd)) = self.doc.get_mut(id).map(|s| &mut s.kind) {
                        if let Some(node) = pd.subpaths[sub].nodes.last_mut() {
                            node.hout = None;
                            node.smooth = false;
                        }
                    }
                    self.drag = Drag::PenHandle { id, sub, node: n - 1, out_only: true };
                    return;
                }
            }
            if let Some(s) = self.doc.get_mut(id) {
                if let ShapeKind::Path(pd) = &mut s.kind {
                    pd.subpaths[sub].nodes.push(Node::corner(p));
                }
            }
            self.drag = Drag::PenHandle { id, sub, node: n, out_only: false };
        } else if let Some((id, sub, at_start)) = self.open_end_at(sp) {
            // Continue an open path from either end.
            self.checkpoint("pen");
            if let Some(ShapeKind::Path(pd)) = self.doc.get_mut(id).map(|s| &mut s.kind) {
                let spth = &mut pd.subpaths[sub];
                if at_start {
                    reverse_subpath(spth);
                }
                let last = spth.nodes.len() - 1;
                self.selection = vec![id];
                self.node_sel.clear();
                self.pen = Some(PenState { id, sub });
                self.drag = Drag::PenHandle { id, sub, node: last, out_only: true };
            }
        } else if let Some((id, si, gi, t)) = self.pen_segment_at(p) {
            // On a selected path: add a node there.
            self.checkpoint("");
            if let Some(ShapeKind::Path(pd)) = self.doc.get_mut(id).map(|s| &mut s.kind) {
                geom::insert_node(&mut pd.subpaths[si], gi, t);
                self.node_sel = vec![(id, si, gi + 1)];
            }
        } else {
            self.checkpoint("");
            let pd = PathData { subpaths: vec![SubPath { nodes: vec![Node::corner(p)], closed: false }] };
            let mut s = Shape::new(ShapeKind::Path(pd));
            s.fill = None;
            s.stroke = Some(DStroke { width: 2.0, color: Rgba::from_hex("#1E1E1E").unwrap(), ..Default::default() });
            let id = self.doc.add_shape(s);
            self.selection = vec![id];
            self.pen = Some(PenState { id, sub: 0 });
            self.drag = Drag::PenHandle { id, sub: 0, node: 0, out_only: false };
        }
    }

    /// An end node of an open path under the pointer (screen pos): (shape,
    /// subpath, is the first node). Selected paths first.
    fn open_end_at(&self, sp: Pos2) -> Option<(u64, usize, bool)> {
        let mut ids = self.node_targets();
        for s in self.doc.shapes().iter().rev() {
            if s.visible && !s.locked && !ids.contains(&s.id) {
                ids.push(s.id);
            }
        }
        for id in ids {
            let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) else { continue };
            for (si, sub) in pd.subpaths.iter().enumerate() {
                if sub.closed || sub.nodes.is_empty() {
                    continue;
                }
                let near = |n: &Node| self.view.to_screen(n.p).distance(sp) <= 8.0;
                if near(sub.nodes.last().unwrap()) {
                    return Some((id, si, false));
                }
                if near(&sub.nodes[0]) {
                    return Some((id, si, true));
                }
            }
        }
        None
    }

    /// A segment of a selected path near `p`: (shape, subpath, segment, t).
    fn pen_segment_at(&self, p: Point) -> Option<(u64, usize, usize, f64)> {
        let tol = 6.0 / self.view.zoom as f64;
        for id in self.node_targets() {
            let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) else { continue };
            if let Some((si, gi, t, d)) = geom::nearest_segment(pd, p) {
                if d <= tol {
                    return Some((id, si, gi, t));
                }
            }
        }
        None
    }

    /// Backspace while drawing: remove the last node (and the path with it
    /// when nothing is left).
    pub fn pen_backspace(&mut self) {
        let Some(pen) = self.pen.as_ref().map(|p| (p.id, p.sub)) else { return };
        let (id, sub) = pen;
        self.checkpoint("");
        let mut left = 0;
        if let Some(ShapeKind::Path(pd)) = self.doc.get_mut(id).map(|s| &mut s.kind) {
            if let Some(spth) = pd.subpaths.get_mut(sub) {
                spth.nodes.pop();
                left = spth.nodes.len();
            }
        }
        if left == 0 {
            self.pen = None;
            self.doc.remove(id);
            self.selection.retain(|i| *i != id);
        }
    }

    // ------------------------------------------------------------------
    // Shape creation
    // ------------------------------------------------------------------

    /// Shape tools: the shape appears once the pointer is dragged (a plain
    /// click selects instead).
    /// Pressing a gradient part selects its stop (the ends: first / last).
    fn select_grad_part(&mut self, id: u64, part: crate::gradient_handles::GradPart) {
        use crate::gradient_handles::GradPart;
        let n = match self.doc.get(id).and_then(|s| s.fill.as_ref()) {
            Some(crate::document::Paint::Gradient(g)) => g.stops.len(),
            _ => return,
        };
        match part {
            GradPart::Stop(i) => self.ui.gradient_stop = i,
            GradPart::Start => self.ui.gradient_stop = 0,
            GradPart::End => self.ui.gradient_stop = n.saturating_sub(1),
            GradPart::MeshPoint(i) => self.ui.gradient_stop = i,
            GradPart::Mid(_) => {}
        }
    }

    /// Fill tool press: grab a gradient handle, or start laying a gradient
    /// across the selection (or the shape under the pointer).
    fn fill_press(&mut self, p: Point, sp: Pos2, no_snap: bool) {
        if let Some((id, part)) = self.gradient_handle_at(sp) {
            self.select_grad_part(id, part);
            self.drag = Drag::Gradient { id, part };
            return;
        }
        let hit = self.pick(p, false);
        let targets = match hit {
            Some(h) if !self.selection.contains(&h) => {
                self.selection = vec![h];
                vec![h]
            }
            _ => self.selection.clone(),
        };
        let start = if no_snap { p } else { self.snap_point(p, &[]) };
        self.snap_guides.clear();
        self.drag = Drag::FillLay { start, targets };
    }

    fn create_press(&mut self, p: Point, no_snap: bool) {
        let p = if self.snap && !no_snap { self.snap_point(p, &[]) } else { p };
        self.snap_guides.clear();
        self.drag = Drag::Create { start: p, id: 0 };
    }

    fn begin_create(&mut self, start: Point) -> u64 {
        self.checkpoint("");
        let kind = match self.tool {
            Tool::Rect => ShapeKind::Rect { w: 1.0, h: 1.0, radii: [0.0; 4] },
            Tool::Ellipse => ShapeKind::Ellipse { w: 1.0, h: 1.0 },
            Tool::Star => ShapeKind::Star { w: 1.0, h: 1.0, points: self.star_points, inner: self.star_inner, corner: 0.0 },
            Tool::Prim(k) => ShapeKind::Prim { w: 1.0, h: 1.0, prim: self.prim_params.get(&k).cloned().unwrap_or_else(|| k.default_prim()) },
            _ => ShapeKind::Polygon { w: 1.0, h: 1.0, sides: self.polygon_sides, corner: 0.0 },
        };
        let open = matches!(&kind, ShapeKind::Prim { prim, .. } if prim.kind.is_open());
        let qr = matches!(&kind, ShapeKind::Prim { prim, .. } if prim.kind == crate::prims::PrimKind::Qr);
        let mut s = Shape::new(kind);
        if qr {
            // Scanners want strong contrast.
            s.fill = Some(crate::document::Paint::Solid(crate::document::Rgba::BLACK));
        }
        if open {
            // Open shapes (spiral) draw as a line.
            s.fill = None;
            s.stroke = Some(crate::document::Stroke { width: 2.0, cap: crate::document::LineCap::Round, color: crate::document::Rgba::from_hex("#1E1E1E").unwrap(), ..Default::default() });
        }
        s.transform = Affine::translate(start.to_vec2());
        let id = self.doc.add_shape(s);
        self.selection = vec![id];
        id
    }

    // ------------------------------------------------------------------
    // Text tool
    // ------------------------------------------------------------------

    fn text_press(&mut self, p: Point, shift: bool) {
        let hit = self.hit_shape(p).filter(|id| matches!(self.doc.get(*id).map(|s| &s.kind), Some(ShapeKind::Text(_))));
        if let Some(id) = hit {
            let same = self.text_edit.as_ref().map(|t| t.id) == Some(id);
            if !same {
                self.finish_text_edit();
                self.begin_text_edit(id);
            }
            // Place the caret (Shift extends) and drag to select.
            self.text_mouse(id, p, shift && same, 1);
            self.drag = Drag::TextSelect { id };
            return;
        }
        if self.text_edit.is_some() {
            self.finish_text_edit();
            self.tool = Tool::Select;
            return;
        }
        // Artistic text clicked on a shape's edge flows along it.
        if self.tool == Tool::Text {
            let tol = 6.0 / self.view.zoom as f64;
            let guide = self.hit_shapes(p, true).into_iter().find(|id| {
                self.doc.get(*id).filter(|s| !matches!(s.kind, ShapeKind::Text(_))).cloned().map(|s| geom::distance_to_path(&self.cache.update(&s, &mut self.fonts).doc_path, p) <= tol).unwrap_or(false)
            });
            if let Some(gid) = guide {
                self.create_path_text(gid, p);
                return;
            }
        }
        // Click creates artistic text (frame text for the frame tool), a drag
        // draws a text frame; both resolve on release.
        self.drag = Drag::TextFrame { start: p };
    }

    /// New text at `at` (top-left); `frame` makes it frame text.
    pub fn create_text(&mut self, at: Point, frame: Option<[f64; 2]>) {
        self.checkpoint("");
        let td = TextData { content: String::new(), frame, ..self.default_text.clone() };
        let mut s = Shape::new(ShapeKind::Text(td));
        s.name = if frame.is_some() { "Frame Text".into() } else { "Artistic Text".into() };
        s.fill = Some(Paint::Solid(Rgba::from_hex("#1E1E1E").unwrap()));
        s.transform = Affine::translate(at.to_vec2());
        let id = self.doc.add_shape(s);
        self.selection = vec![id];
        self.text_edit = Some(TextEditState { id, cursor: 0, anchor: 0, restore: None });
    }

    /// Start typing text along the outline of shape `guide` (which is consumed).
    pub fn create_path_text(&mut self, guide: u64, at: Point) {
        let Some(g) = self.doc.get(guide).cloned() else { return };
        let path = self.cache.update(&g, &mut self.fonts).doc_path.clone();
        let poly = crate::fonts::Polyline::new(&path);
        self.checkpoint("");
        let td = TextData { content: String::new(), frame: None, path: Some(geom::bez_to_pathdata(&path)), path_offset: poly.project(at), align: crate::document::TextAlign::Left, ..self.default_text.clone() };
        let mut s = Shape::new(ShapeKind::Text(td));
        s.name = "Path Text".into();
        s.fill = Some(Paint::Solid(Rgba::from_hex("#1E1E1E").unwrap()));
        let idx = self.doc.index_of(guide).unwrap_or(0);
        self.doc.remove(guide);
        let id = self.doc.insert_shape(idx, s);
        self.selection = vec![id];
        self.text_edit = Some(TextEditState { id, cursor: 0, anchor: 0, restore: Some(g) });
    }

    // ------------------------------------------------------------------
    // Corner (roundover) tool
    // ------------------------------------------------------------------

    /// Corner markers in document space: (shape id, corner index, position).
    fn corner_markers(&self) -> Vec<(u64, usize, Point)> {
        let mut out = Vec::new();
        for id in &self.selection {
            let Some(s) = self.doc.get(*id) else { continue };
            match &s.kind {
                ShapeKind::Rect { .. } | ShapeKind::Star { .. } | ShapeKind::Polygon { .. } => {
                    if let Some(pd) = geom::kind_nodes(&s.kind) {
                        for (i, n) in pd.subpaths[0].nodes.iter().enumerate() {
                            out.push((s.id, i, s.transform * n.p));
                        }
                    }
                }
                ShapeKind::Path(pd) => {
                    let mut k = 0;
                    for sp in &pd.subpaths {
                        let n = sp.nodes.len();
                        for (i, node) in sp.nodes.iter().enumerate() {
                            let interior = sp.closed || (i > 0 && i + 1 < n);
                            if interior && !node.smooth {
                                out.push((s.id, k, node.p));
                            }
                            k += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn corner_press(&mut self, p: Point, sp: Pos2, inp: &Input) {
        let hit = self
            .corner_markers()
            .into_iter()
            .map(|(id, i, q)| (id, i, self.view.to_screen(q).distance(sp)))
            .filter(|m| m.2 <= HANDLE_R + 3.0)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((id, i, _)) = hit {
            if let Some(orig) = self.doc.get(id).cloned() {
                self.drag = Drag::Corner { id, corner: i, single: inp.alt, original: orig };
            }
            return;
        }
        if let Some(id) = self.hit_shape(p) {
            if inp.shift {
                if !self.selection.contains(&id) {
                    self.selection.push(id);
                }
            } else {
                self.selection = vec![id];
            }
        } else if !inp.shift {
            self.selection.clear();
        }
    }

    /// Apply a radius to all corners of the selected shapes (or selected nodes).
    pub fn set_corner_radius_all(&mut self, r: f64) {
        self.checkpoint("corner-all");
        let node_sel = self.node_sel.clone();
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                match &mut s.kind {
                    ShapeKind::Rect { radii, .. } => *radii = [r; 4],
                    ShapeKind::Star { corner, .. } | ShapeKind::Polygon { corner, .. } => *corner = r,
                    ShapeKind::Path(pd) => {
                        let any_sel = node_sel.iter().any(|k| k.0 == id);
                        for (si, sp) in pd.subpaths.iter_mut().enumerate() {
                            for (ni, n) in sp.nodes.iter_mut().enumerate() {
                                if !any_sel || node_sel.contains(&(id, si, ni)) {
                                    n.corner = r;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Shape builder
    // ------------------------------------------------------------------

    pub(crate) fn refresh_builder(&mut self) {
        let mut ids: Vec<u64> = self
            .selection
            .iter()
            .copied()
            .filter(|id| self.doc.get(*id).map(|s| s.visible).unwrap_or(false))
            .collect();
        ids.sort_by_key(|id| self.doc.index_of(*id).unwrap_or(0));
        let sig: Vec<(u64, u64)> = ids.iter().filter_map(|id| self.doc.get(*id).map(|s| (s.id, s.rev))).collect();
        if ids.len() < 2 {
            self.builder = None;
            return;
        }
        if self.builder.as_ref().map(|b| b.sig == sig).unwrap_or(false) {
            return;
        }
        let polys: Vec<boolean::Polys> = ids
            .iter()
            .filter_map(|id| self.doc.get(*id))
            .map(|s| {
                let c = self.cache.update(s, &mut self.fonts);
                boolean::path_to_polys(&c.doc_path, s.fill_rule)
            })
            .collect();
        let regions = boolean::arrangement(&polys);
        self.builder = Some(BuilderState { sig, ids, regions });
    }

    pub(crate) fn commit_builder(&mut self, marked: &[usize], mode: BuilderMode) {
        let Some(b) = self.builder.take() else { return };
        self.checkpoint("");
        let originals: Vec<Shape> = b.ids.iter().filter_map(|id| self.doc.get(*id).cloned()).collect();
        if originals.len() != b.ids.len() {
            return;
        }
        let subtract = mode == BuilderMode::Subtract;
        if mode == BuilderMode::New {
            // New shape from the marked regions above the top-most original.
            let parts: Vec<&boolean::Polys> = marked.iter().filter_map(|i| b.regions.get(*i)).map(|r| &r.polys).collect();
            let u = boolean::union_all(&parts);
            if u.is_empty() {
                return;
            }
            let owner = marked.iter().filter_map(|i| b.regions.get(*i)).map(|r| r.owner).max().unwrap_or(0);
            let mut s = originals[owner].clone();
            s.id = 0;
            s.kind = ShapeKind::Path(geom::polys_to_pathdata_smooth(&u));
            s.transform = Affine::IDENTITY;
            s.fill_rule = FillRule::EvenOdd;
            s.name = format!("{} (new)", originals[owner].name);
            let top = b.ids.iter().filter_map(|id| self.doc.index_of(*id)).max().unwrap_or(0);
            self.doc.insert_shape(top + 1, s);
            // Keep the originals selected so more pieces can be built from them.
            self.builder = None;
            self.notify("New shape created from the marked regions");
            return;
        }
        let insert_at = b.ids.iter().filter_map(|id| self.doc.index_of(*id)).max().unwrap_or(0) + 1 - b.ids.len();
        // Group remaining regions by owner shape.
        let mut groups: Vec<Vec<&boolean::Polys>> = vec![Vec::new(); originals.len()];
        for (i, r) in b.regions.iter().enumerate() {
            if !marked.contains(&i) {
                groups[r.owner].push(&r.polys);
            }
        }
        let mut new_shapes: Vec<(usize, Shape)> = Vec::new();
        let make = |style: &Shape, polys: &boolean::Polys| -> Shape {
            let mut s = style.clone();
            s.id = 0;
            s.kind = ShapeKind::Path(geom::polys_to_pathdata_smooth(polys));
            s.transform = Affine::IDENTITY;
            s.fill_rule = FillRule::EvenOdd;
            s
        };
        for (owner, parts) in groups.iter().enumerate() {
            if parts.is_empty() {
                continue;
            }
            let u = boolean::union_all(parts);
            if !u.is_empty() {
                new_shapes.push((owner, make(&originals[owner], &u)));
            }
        }
        let mut merged_index = None;
        if !subtract {
            let parts: Vec<&boolean::Polys> = marked.iter().filter_map(|i| b.regions.get(*i)).map(|r| &r.polys).collect();
            let u = boolean::union_all(&parts);
            let first_owner = b.regions[marked[0]].owner;
            let top_owner = marked.iter().filter_map(|i| b.regions.get(*i)).map(|r| r.owner).max().unwrap_or(first_owner);
            if !u.is_empty() {
                let mut s = make(&originals[first_owner], &u);
                s.name = format!("{} (merged)", originals[first_owner].name);
                merged_index = Some(new_shapes.len());
                new_shapes.push((top_owner, s));
            }
        }
        for s in &originals {
            self.doc.remove(s.id);
        }
        // Stable sort by owner keeps layer order; merged shape goes above its owner group.
        let mut order: Vec<usize> = (0..new_shapes.len()).collect();
        order.sort_by_key(|&i| (new_shapes[i].0, Some(i) == merged_index));
        let mut sel = Vec::new();
        for (k, i) in order.into_iter().enumerate() {
            let s = new_shapes[i].1.clone();
            sel.push(self.doc.insert_shape(insert_at + k, s));
        }
        self.selection = sel;
        self.builder = None;
    }

    // ------------------------------------------------------------------
    // Overlays
    // ------------------------------------------------------------------

    /// Name tags above components, like Figma: always for top-level main
    /// components, and for any component while selected or hovered.
    fn paint_component_labels(&mut self, painter: &egui::Painter) {
        use crate::document::Component;
        let mut ids: Vec<u64> = self.doc.shapes().iter().filter(|s| matches!(s.component, Some(Component::Main { .. })) && s.visible).map(|s| s.id).collect();
        for id in self.selection.iter().copied().chain(self.hover) {
            if self.doc.get(id).map(|s| s.component.is_some()).unwrap_or(false) && !ids.contains(&id) {
                ids.push(id);
            }
        }
        let font = egui::FontId::proportional(11.0);
        for id in ids {
            let Some(s) = self.doc.get(id) else { continue };
            let glyph = if matches!(s.component, Some(Component::Main { .. })) { egui_phosphor::regular::DIAMONDS_FOUR } else { egui_phosphor::regular::DIAMOND };
            let text = format!("{glyph} {}", s.name);
            let Some(b) = self.cache.get(id).and_then(|c| c.bounds) else { continue };
            let r = self.view.rect_to_screen(b);
            if !r.intersects(self.canvas_rect) {
                continue;
            }
            let g = painter.layout(text, font.clone(), theme::COMPONENT, (r.width() + 40.0).max(60.0));
            let g = if g.rows.len() > 1 { painter.layout_no_wrap(format!("{glyph}"), font.clone(), theme::COMPONENT) } else { g };
            painter.galley(pos2(r.left(), r.top() - g.size().y - 4.0), g, theme::COMPONENT);
        }
    }

    fn overlays(&mut self, painter: &egui::Painter) {
        let accent = theme::ACCENT;
        self.paint_guides(painter, self.canvas_rect);

        // Inside a boolean: show every sibling's outline (subtracted shapes
        // are otherwise invisible).
        let parents: Vec<u64> = self.selection.iter().filter_map(|id| self.doc.parent_of(*id)).collect();
        for pid in parents {
            if let Some(ShapeKind::Compound { children, .. }) = self.doc.get(pid).map(|s| s.kind.clone()) {
                // (groups don't need this: their children are visible)
                for c in &children {
                    let path = self.cache.update(c, &mut self.fonts).doc_path.clone();
                    render::outline(painter, &path, &self.view, Stroke::new(1.0, accent.gamma_multiply(0.45)));
                }
            }
        }

        // A selected live warp: its envelope, warped.
        for id in &self.selection {
            if let Some(crate::paint_ops::LiveEffect::Warp(w)) = self.doc.get(*id).and_then(|s| s.live.as_ref()) {
                let path = w.params().outline(w.frame);
                render::outline(painter, &path, &self.view, Stroke::new(1.0, accent.gamma_multiply(0.7)));
            }
        }

        // Components (and their layers) are outlined in their own colour.
        let comp_col = |app: &App, id: u64| if app.in_component(id) { theme::COMPONENT } else { accent };
        let sel_col = if !self.selection.is_empty() && self.selection.iter().all(|id| self.in_component(*id)) { theme::COMPONENT } else { accent };
        let thin = Stroke::new(1.0, sel_col);

        // Hover outline.
        if let Some(h) = self.hover {
            if !self.selection.contains(&h) && self.press.is_none() {
                if let Some(c) = self.cache.get(h) {
                    render::outline(painter, &c.doc_path, &self.view, Stroke::new(1.5, comp_col(self, h)));
                }
            }
        }
        self.paint_component_labels(painter);

        // Selection outlines (just their boxes for very large selections),
        // skipping those out of view.
        if self.tool != Tool::ShapeBuilder {
            let boxes_only = self.selection.len() > 1000;
            for id in &self.selection {
                let Some(c) = self.cache.get(*id) else { continue };
                let Some(b) = c.bounds else { continue };
                let r = self.view.rect_to_screen(b);
                if !r.expand(2.0).intersects(self.canvas_rect) {
                    continue;
                }
                if boxes_only {
                    painter.rect_stroke(r, 0.0, thin, StrokeKind::Middle);
                } else {
                    render::outline(painter, &c.doc_path, &self.view, thin);
                }
            }
        }

        match self.tool {
            Tool::Select | Tool::Rect | Tool::Ellipse | Tool::Star | Tool::Polygon | Tool::Prim(_) => {
                if let (Some(b), Some(g)) = (self.selection_bounds(), self.gizmo()) {
                    let c = g.corners;
                    painter.add(egui::Shape::closed_line(c.to_vec(), thin));
                    let rotating = matches!(self.drag, Drag::Rotate { .. }) && self.dragging;
                    if self.tool == Tool::Select && !rotating {
                        // Rotation knob above the top edge.
                        let knob_hot = self.rotate_hover && self.cursor_doc.map(|p| self.view.to_screen(p).distance(g.knob) <= HANDLE_R + 1.0).unwrap_or(false);
                        painter.line_segment([g.top, g.knob], thin);
                        painter.circle_filled(g.knob, if knob_hot { 6.0 } else { 5.0 }, if knob_hot { sel_col } else { Color32::WHITE });
                        painter.circle_stroke(g.knob, if knob_hot { 6.0 } else { 5.0 }, thin);
                        for (_, hp) in g.handles {
                            let hr = egui::Rect::from_center_size(hp, vec2(8.0, 8.0));
                            painter.rect_filled(hr, CornerRadius::same(1), Color32::WHITE);
                            painter.rect_stroke(hr, CornerRadius::same(1), thin, StrokeKind::Middle);
                        }
                    }
                    if self.tool == Tool::Select {
                        if let Some(pv) = self.pivot_point() {
                            paint_pivot(painter, self.view.to_screen(pv), self.pivot.is_some() || rotating);
                        }
                    }
                    // Size badge below the box.
                    let label = format!("{} × {}", crate::units::fmt_value(b.width()), crate::units::fmt(b.height()));
                    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(11.0), Color32::WHITE);
                    let bottom = c.iter().map(|p| p.y).fold(f32::MIN, f32::max);
                    let cx = c.iter().map(|p| p.x).sum::<f32>() / 4.0;
                    let pos = pos2(cx - galley.size().x / 2.0, bottom + 10.0);
                    let bg = egui::Rect::from_min_size(pos - vec2(6.0, 2.0), galley.size() + vec2(12.0, 4.0));
                    if !rotating {
                        painter.rect_filled(bg, CornerRadius::same(4), sel_col);
                        painter.galley(pos, galley, Color32::WHITE);
                    }
                }
                // Live rotation: arc from the start angle, spoke to the pointer, angle badge.
                if let (Drag::Rotate { center, start_angle, .. }, true, Some(cur)) = (&self.drag, self.dragging, self.cursor_doc) {
                    let shift = painter.ctx().input(|i| i.modifiers.shift);
                    let mut ang = (cur - *center).atan2() - start_angle;
                    ang = (ang + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                    if let (true, Drag::Rotate { originals, .. }) = (shift, &self.drag) {
                        let base = rotation_base(originals);
                        let step = std::f64::consts::PI / 12.0;
                        ang = ((base + ang) / step).round() * step - base;
                    }
                    let cs = self.view.to_screen(*center);
                    let radius = 46.0f32;
                    let n = 40;
                    let mut pts = vec![cs];
                    for i in 0..=n {
                        let t = *start_angle + ang * i as f64 / n as f64;
                        pts.push(cs + vec2(t.cos() as f32, t.sin() as f32) * radius);
                    }
                    let fill = accent.gamma_multiply(0.18);
                    for w in pts[1..].windows(2) {
                        painter.add(egui::Shape::convex_polygon(vec![cs, w[0], w[1]], fill, Stroke::NONE));
                    }
                    painter.add(egui::Shape::line(pts[1..].to_vec(), Stroke::new(1.5, accent)));
                    let start_dir = vec2(start_angle.cos() as f32, start_angle.sin() as f32);
                    painter.line_segment([cs, cs + start_dir * radius], Stroke::new(1.0, accent.gamma_multiply(0.6)));
                    let cur_s = self.view.to_screen(cur);
                    let cur_ang = *start_angle + ang;
                    let reach = (cur_s - cs).length().max(radius);
                    painter.line_segment([cs, cs + vec2(cur_ang.cos() as f32, cur_ang.sin() as f32) * reach], Stroke::new(1.0, accent));
                    paint_pivot(painter, cs, true);
                    // Badge: delta (and absolute angle for a single shape).
                    let mut label = format!("{:+.1}°", ang.to_degrees());
                    if self.selection.len() == 1 {
                        if let Some(s) = self.doc.get(self.selection[0]) {
                            if !matches!(s.kind, ShapeKind::Path(_)) {
                                let [a, b, ..] = s.transform.as_coeffs();
                                label = format!("{}  ({:.1}°)", label, b.atan2(a).to_degrees());
                            }
                        }
                    }
                    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(11.5), Color32::WHITE);
                    let bg = egui::Rect::from_min_size(cur_s + vec2(16.0, 14.0), galley.size() + vec2(12.0, 5.0));
                    painter.rect_filled(bg, CornerRadius::same(4), accent);
                    painter.galley(bg.min + vec2(6.0, 2.5), galley, Color32::WHITE);
                }
                if self.tool == Tool::Select && (self.rotate_hover || (matches!(self.drag, Drag::Rotate { .. }) && self.dragging)) {
                    if let Some(cur) = self.cursor_doc {
                        painter.ctx().set_cursor_icon(CursorIcon::None);
                        paint_rotate_cursor(painter, self.view.to_screen(cur));
                    }
                }
                // Shearing from an edge: a slanted-arrows cursor along the
                // edge, and the angle while dragging.
                let shearing = match &self.drag {
                    Drag::Shear { horizontal, frame, .. } if self.press.is_some() => Some((*horizontal, *frame)),
                    _ => None,
                };
                let shear_cur = shearing.or_else(|| self.shear_hover.zip(self.sel_frame().map(|f| f.0)));
                if let (Some((h, frame)), Some(cur), Tool::Select) = (shear_cur, self.cursor_doc, self.tool) {
                    let a = frame * if h { Point::new(1.0, 0.0) } else { Point::new(0.0, 1.0) } - frame * Point::ORIGIN;
                    let at = self.view.to_screen(cur);
                    painter.ctx().set_cursor_icon(CursorIcon::None);
                    paint_shear_cursor(painter, at, vec2(a.x as f32, a.y as f32).normalized());
                    if let (Some(deg), true) = (self.shear_angle, shearing.is_some() && self.dragging) {
                        let galley = painter.layout_no_wrap(format!("{:.1}°", deg), egui::FontId::proportional(11.0), Color32::WHITE);
                        let r = egui::Rect::from_min_size(at + vec2(16.0, 14.0), galley.size() + vec2(10.0, 6.0));
                        painter.rect_filled(r, CornerRadius::same(4), accent);
                        painter.galley(r.min + vec2(5.0, 3.0), galley, Color32::WHITE);
                    }
                }
            }
            _ => {}
        }

        // Snap guides: highlight the target, draw the aligned line between
        // the two objects and mark the coincident points.
        let pink = Color32::from_rgb(0xFF, 0x4F, 0xA3);
        for g in &self.snap_guides {
            let tr = self.view.rect_to_screen(g.target);
            if g.is_shape {
                painter.rect_filled(tr, 0.0, pink.gamma_multiply(0.08));
                painter.rect_stroke(tr, 0.0, Stroke::new(1.5, pink), StrokeKind::Middle);
            }
            let mr = self.view.rect_to_screen(g.moving);
            let span = if g.vertical {
                (tr.top().min(mr.top()) - 12.0)..=(tr.bottom().max(mr.bottom()) + 12.0)
            } else {
                (tr.left().min(mr.left()) - 12.0)..=(tr.right().max(mr.right()) + 12.0)
            };
            let cross = |c: Pos2| {
                let k = 3.5;
                painter.line_segment([c - vec2(k, k), c + vec2(k, k)], Stroke::new(1.5, pink));
                painter.line_segment([c - vec2(k, -k), c + vec2(k, -k)], Stroke::new(1.5, pink));
            };
            if g.vertical {
                let x = self.view.to_screen(Point::new(g.pos, 0.0)).x;
                painter.vline(x, span, Stroke::new(1.0, pink));
                for r in [tr, mr] {
                    for y in [r.top(), r.center().y, r.bottom()] {
                        cross(pos2(x, y));
                    }
                }
            } else {
                let y = self.view.to_screen(Point::new(0.0, g.pos)).y;
                painter.hline(span, y, Stroke::new(1.0, pink));
                for r in [tr, mr] {
                    for x in [r.left(), r.center().x, r.right()] {
                        cross(pos2(x, y));
                    }
                }
            }
        }

        self.paint_gaps(painter);
        // Knife stroke.
        if let (Drag::Knife { pts }, true) = (&self.drag, self.dragging) {
            // Preview the cut as it will be made (smoothed).
            let mut dp = pts.clone();
            if let Some(c) = self.cursor_doc {
                if painter.ctx().input(|i| i.modifiers.shift) {
                    dp.truncate(1);
                }
                dp.push(c);
            }
            let sp: Vec<Pos2> = geom::smooth_polyline(&dp, self.knife_smooth).iter().map(|p| self.view.to_screen(*p)).collect();
            let red = Color32::from_rgb(0xF2, 0x48, 0x22);
            painter.add(egui::Shape::line(sp.clone(), Stroke::new(3.0, Color32::from_white_alpha(160))));
            painter.add(egui::Shape::dashed_line(&sp, Stroke::new(1.5, red), 6.0, 4.0));
        }
        // Stabilizer: the rope from the drawing tip to the pointer.
        if let (Some(st), true, Some(cur)) = (&self.stab, self.dragging, self.cursor_doc) {
            let (a, b) = (self.view.to_screen(st.tip), self.view.to_screen(cur));
            let pink = Color32::from_rgb(0xE0, 0x3C, 0xA0);
            if st.mode == crate::freehand::StabMode::Rope {
                painter.line_segment([a, b], Stroke::new(1.5, pink));
                painter.circle_stroke(b, (st.len as f32 * self.view.zoom).clamp(3.0, 6.0), Stroke::new(1.5, pink));
            }
            painter.circle_filled(a, 2.5, pink);
        }
        self.paint_gradient_handles(painter);
        if !matches!(self.drag, Drag::Create { .. }) || !self.dragging {
            self.paint_prim_handles(painter);
        }
        if self.tool == Tool::Eyedropper && self.canvas_rect.contains(self.view.to_screen(self.cursor_doc.unwrap_or_default())) {
            self.paint_eyedropper(painter);
        }
        // Alt held with a selection: distances to the hovered shape / artboard.
        if self.tool == Tool::Select && self.press.is_none() && self.text_edit.is_none() && painter.ctx().input(|i| i.modifiers.alt) {
            self.paint_measure(painter);
        }

        // Marquee.
        if let (Drag::Marquee { start, .. } | Drag::NodeMarquee { start, .. } | Drag::ZoomRect { start }, Some(cur)) = (&self.drag, self.cursor_doc) {
            if self.dragging {
                let r = self.view.rect_to_screen(Rect::from_points(*start, cur));
                painter.rect_filled(r, 0.0, accent.gamma_multiply(0.12));
                painter.rect_stroke(r, 0.0, thin, StrokeKind::Middle);
            }
        }

        // Text frame being drawn.
        if let (Drag::TextFrame { start }, Some(cur), true) = (&self.drag, self.cursor_doc, self.dragging) {
            if self.tool == Tool::Text {
                // Size preview: a text-height bar with the resulting size.
                let (size, top) = artistic_drag(*start, cur, self.default_text.line_height);
                let lh = size * self.default_text.line_height;
                let a = self.view.to_screen(Point::new(start.x, top));
                let b = self.view.to_screen(Point::new(start.x, top + lh));
                painter.line_segment([a, b], Stroke::new(2.0, accent));
                for y in [a.y, b.y] {
                    painter.line_segment([pos2(a.x - 5.0, y), pos2(a.x + 5.0, y)], Stroke::new(1.5, accent));
                }
                let g = painter.layout_no_wrap(format!("{:.1} px", size), egui::FontId::proportional(11.0), Color32::WHITE);
                let bg = egui::Rect::from_min_size(pos2(a.x + 10.0, (a.y + b.y) / 2.0 - 9.0), g.size() + vec2(12.0, 4.0));
                painter.rect_filled(bg, CornerRadius::same(4), accent);
                painter.galley(bg.min + vec2(6.0, 2.0), g, Color32::WHITE);
            } else {
                let shift = painter.ctx().input(|i| i.modifiers.shift);
                let r = self.view.rect_to_screen(drag_rect(*start, cur, shift, false));
                dashed_rect(painter, r, Stroke::new(1.0, accent));
            }
        }

        // Frames of frame text (selected, hovered or with a text tool active),
        // with a red marker when the text overflows.
        let ids: Vec<u64> = self.doc.shapes().iter().filter(|s| s.visible && matches!(&s.kind, ShapeKind::Text(t) if t.frame.is_some())).map(|s| s.id).collect();
        for id in ids {
            let Some(s) = self.doc.get(id) else { continue };
            let ShapeKind::Text(td) = &s.kind else { continue };
            let Some(f) = td.frame else { continue };
            let active = self.selection.contains(&id) || self.hover == Some(id) || self.tool.is_text();
            if !active {
                continue;
            }
            let corners = [Point::new(0.0, 0.0), Point::new(f[0], 0.0), Point::new(f[0], f[1]), Point::new(0.0, f[1])].map(|q| self.view.to_screen(s.transform * q));
            let col = if self.selection.contains(&id) { accent.gamma_multiply(0.8) } else { theme::TEXT_FAINT };
            for i in 0..4 {
                dashed_line(painter, corners[i], corners[(i + 1) % 4], Stroke::new(1.0, col));
            }
            if self.cache.get(id).and_then(|c| c.text.as_ref()).map(|t| t.overflow).unwrap_or(false) {
                let c = corners[2];
                let r = egui::Rect::from_center_size(c, vec2(11.0, 11.0));
                painter.rect_filled(r, CornerRadius::same(2), theme::DANGER);
                painter.text(r.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(11.0), Color32::WHITE);
            }
            // Thread: a link from this frame's end to the next frame's start.
            if let Some(n) = td.next.and_then(|n| self.doc.get(n)) {
                if matches!(n.kind, ShapeKind::Text(_)) {
                    let to = self.view.to_screen(n.transform * Point::ZERO);
                    let from = corners[2];
                    let link = Stroke::new(1.0, accent.gamma_multiply(0.7));
                    dashed_line(painter, from, to, link);
                    painter.circle_filled(from, 3.0, accent);
                    painter.circle_stroke(to, 3.5, Stroke::new(1.5, accent));
                }
            }
        }

        // Node editing (node tool and pen).
        if matches!(self.tool, Tool::Node | Tool::Pen) {
            for id in self.node_targets() {
                self.paint_nodes(painter, id);
            }
            // Pen hover: continue an open path from its end, or add a node
            // on a selected path.
            let mut ghost = self.pen_hover.filter(|_| self.tool == Tool::Pen && self.press.is_none());
            if let (None, Some(cur), Tool::Pen, true) = (&self.pen, self.cursor_doc, self.tool, self.press.is_none()) {
                let sp = self.view.to_screen(cur);
                if let Some((id, si, start)) = self.open_end_at(sp) {
                    ghost = None;
                    if let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) {
                        let sub = &pd.subpaths[si];
                        let n = if start { &sub.nodes[0] } else { sub.nodes.last().unwrap() };
                        let c = self.view.to_screen(n.p);
                        painter.circle_stroke(c, 8.0, Stroke::new(1.5, accent));
                        painter.text(c + vec2(12.0, -12.0), egui::Align2::LEFT_BOTTOM, "Continue path", egui::FontId::proportional(11.0), accent);
                    }
                } else if let Some((id, si, gi, t)) = self.pen_segment_at(cur) {
                    ghost = None;
                    if let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) {
                        use kurbo::ParamCurve;
                        let q = self.view.to_screen(geom::segment_at(&pd.subpaths[si], gi).eval(t));
                        painter.circle_filled(q, 4.0, Color32::WHITE);
                        painter.circle_stroke(q, 4.0, Stroke::new(1.5, accent));
                        painter.text(q + vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, "+", egui::FontId::proportional(13.0), accent);
                    }
                }
            }
            if let (Some(pen), Some(cur)) = (&self.pen, self.cursor_doc) {
                if self.press.is_none() {
                    if let Some(ShapeKind::Path(pd)) = self.doc.get(pen.id).map(|s| &s.kind) {
                        let sub = pd.subpaths.get(pen.sub);
                        let first = sub.filter(|s| s.nodes.len() >= 2).and_then(|s| s.nodes.first()).map(|n| n.p);
                        // Snap the preview to the first node when closing.
                        let closing = first.filter(|f| self.view.to_screen(*f).distance(self.view.to_screen(cur)) <= PEN_CLOSE_R);
                        let cur = closing.or(self.pen_hover).unwrap_or(cur);
                        if let Some(f) = closing {
                            ghost = None;
                            let c = self.view.to_screen(f);
                            painter.circle_stroke(c, 9.0, Stroke::new(2.0, accent));
                            painter.circle_filled(c, 4.5, accent);
                            painter.text(c + vec2(12.0, -12.0), egui::Align2::LEFT_BOTTOM, "Close path", egui::FontId::proportional(11.0), accent);
                        }
                        if let Some(last) = sub.and_then(|s| s.nodes.last()) {
                            let mut b = BezPath::new();
                            b.move_to(last.p);
                            match last.hout {
                                Some(h) => b.curve_to(h, cur, cur),
                                None => b.line_to(cur),
                            }
                            render::outline(painter, &b, &self.view, Stroke::new(1.0, accent.gamma_multiply(0.8)));
                        }
                    }
                }
            }
            // The node the next click places.
            if let Some(g) = ghost {
                let c = self.view.to_screen(g);
                let r = egui::Rect::from_center_size(c, vec2(7.0, 7.0));
                painter.rect_filled(r.expand(1.0), CornerRadius::same(1), Color32::from_black_alpha(60));
                painter.rect_filled(r, CornerRadius::same(1), Color32::WHITE.gamma_multiply(0.9));
                painter.rect_stroke(r, CornerRadius::same(1), Stroke::new(1.5, accent), egui::StrokeKind::Middle);
            }
        }

        // Corner tool markers.
        if self.tool == Tool::Corner {
            for (_, _, q) in self.corner_markers() {
                let c = self.view.to_screen(q);
                painter.circle_filled(c, 5.0, Color32::WHITE);
                painter.circle_stroke(c, 5.0, Stroke::new(1.5, accent));
            }
            if let (Drag::Corner { id, .. }, true) = (&self.drag, self.dragging) {
                if let Some(s) = self.doc.get(*id) {
                    let r = current_corner_radius(s);
                    if let Some(sp) = self.cursor_doc.map(|c| self.view.to_screen(c)) {
                        let galley = painter.layout_no_wrap(format!("Radius {}", fmt_num(r)), egui::FontId::proportional(11.0), Color32::WHITE);
                        let bg = egui::Rect::from_min_size(sp + vec2(14.0, 10.0), galley.size() + vec2(12.0, 4.0));
                        painter.rect_filled(bg, CornerRadius::same(4), accent);
                        painter.galley(bg.min + vec2(6.0, 2.0), galley, Color32::WHITE);
                    }
                }
            }
        }

        // Shape builder regions.
        if self.tool == Tool::ShapeBuilder {
            if let Some(b) = &self.builder {
                let (marked, mode) = match &self.drag {
                    Drag::Builder { marked, mode } => (marked.clone(), *mode),
                    _ => (Vec::new(), if painter.ctx().input(|i| i.modifiers.alt) { BuilderMode::Subtract } else { self.builder_mode }),
                };
                for (i, r) in b.regions.iter().enumerate() {
                    let hovered = self.builder_hover == Some(i);
                    let is_marked = marked.contains(&i);
                    if is_marked || hovered {
                        let color = match mode {
                            BuilderMode::Subtract => Color32::from_rgb(255, 80, 80),
                            BuilderMode::New => Color32::from_rgb(0x3D, 0xD6, 0x8C),
                            BuilderMode::Add => accent,
                        };
                        let (v, idx) = render::tessellate_fill_raw(&r.path, 0.1 / self.view.zoom as f64, FillRule::EvenOdd);
                        let mut mesh = egui::Mesh::default();
                        let col = color.gamma_multiply(if is_marked { 0.55 } else { 0.3 });
                        for q in v {
                            mesh.colored_vertex(self.view.to_screen(Point::new(q[0] as f64, q[1] as f64)), col);
                        }
                        mesh.indices = idx;
                        painter.add(mesh);
                    }
                    let s = if hovered { Stroke::new(2.0, accent) } else { Stroke::new(1.0, accent.gamma_multiply(0.55)) };
                    render::outline(painter, &r.path, &self.view, s);
                }
            } else {
                for id in self.selection.clone() {
                    if let Some(c) = self.cache.get(id) {
                        render::outline(painter, &c.doc_path, &self.view, thin);
                    }
                }
            }
        }

        // Text selection and caret.
        self.paint_text_selection(painter);
        self.paint_spelling(painter);
        if let Some(te) = &self.text_edit {
            if let (Some(s), Some(c)) = (self.doc.get(te.id), self.cache.get(te.id)) {
                if let Some(layout) = &c.text {
                    let ci = layout.carets.iter().enumerate().filter(|(_, k)| k.index <= te.cursor).max_by_key(|(_, k)| k.index).map(|(i, _)| i);
                    let caret = ci.map(|i| (layout.carets[i].x, layout.carets[i].line)).unwrap_or((0.0, 0));
                    let x = caret.0;
                    let (y, lh) = layout.lines.get(caret.1).map(|l| (l.y0, l.y1 - l.y0)).unwrap_or((0.0, layout.line_height));
                    let (a, b) = match (&layout.on_path, ci) {
                        (Some(op), Some(i)) => {
                            // Caret stands perpendicular to the path at the baseline.
                            let (p, ang) = op[i];
                            let up = kurbo::Vec2::new(ang.sin(), -ang.cos()) * layout.line_height * 0.8;
                            (self.view.to_screen(s.transform * (p + up * 0.05)), self.view.to_screen(s.transform * (p + up)))
                        }
                        _ => (self.view.to_screen(s.transform * Point::new(x, y + lh * 0.1)), self.view.to_screen(s.transform * Point::new(x, y + lh * 0.9))),
                    };
                    let t = painter.ctx().input(|i| i.time);
                    if (t * 1.6).fract() < 0.6 {
                        painter.line_segment([a, b], Stroke::new(2.0, accent));
                    }
                    painter.ctx().request_repaint_after(std::time::Duration::from_millis(150));
                    let bounds = s.transform.transform_rect_bbox(layout.bounds);
                    painter.rect_stroke(self.view.rect_to_screen(bounds).expand(2.0), 0.0, Stroke::new(1.0, accent.gamma_multiply(0.6)), StrokeKind::Middle);
                }
            }
        }
    }

    fn paint_nodes(&self, painter: &egui::Painter, id: u64) {
        let Some(ShapeKind::Path(pd)) = self.doc.get(id).map(|s| &s.kind) else { return };
        let accent = theme::ACCENT;
        for (si, sp) in pd.subpaths.iter().enumerate() {
            for (ni, n) in sp.nodes.iter().enumerate() {
                if self.handles_visible(id, si, ni, sp) {
                    let c = self.view.to_screen(n.p);
                    for h in [n.hin, n.hout].into_iter().flatten() {
                        let hp = self.view.to_screen(h);
                        painter.line_segment([c, hp], Stroke::new(1.0, accent));
                        painter.circle_filled(hp, 3.5, Color32::WHITE);
                        painter.circle_stroke(hp, 3.5, Stroke::new(1.0, accent));
                    }
                }
            }
            for (ni, n) in sp.nodes.iter().enumerate() {
                let c = self.view.to_screen(n.p);
                let sel = self.node_selected(id, si, ni);
                let fill = if sel { accent } else { Color32::WHITE };
                if n.smooth {
                    painter.circle_filled(c, 4.0, fill);
                    painter.circle_stroke(c, 4.0, Stroke::new(1.5, accent));
                } else {
                    let r = egui::Rect::from_center_size(c, vec2(7.0, 7.0));
                    painter.rect_filled(r, 0.0, fill);
                    painter.rect_stroke(r, 0.0, Stroke::new(1.5, accent), StrokeKind::Middle);
                }
            }
        }
    }
}

impl App {
    /// Snap a move delta so the moved bounds' edges/centers align with the
    /// artboard or other shapes (6 screen px tolerance).
    fn snap_move(&mut self, originals: &[Shape], d: Vec2) -> Vec2 {
        let mut moving: Option<Rect> = None;
        for o in originals {
            let (p, _) = render::compute_outline(o, &mut self.fonts);
            if let Some(b) = geom::path_bounds(&p) {
                moving = geom::union_rect(moving, b);
            }
        }
        let Some(m0) = moving else { return d };
        let ids: Vec<u64> = originals.iter().map(|o| o.id).collect();
        self.snap_box(m0, d, &ids)
    }

    /// Snap a scale handle. Corners follow the pointer. On a rotated or
    /// sheared box an edge handle can only slide along its local axis, so it
    /// goes as far along that axis as reaches what the handle snapped to.
    fn snap_scale_handle(&mut self, p: Point, handle: Handle, bbox: Rect, frame: Affine, inp: &Input, exclude: &[u64]) -> Point {
        let [a, b, c, d, ..] = frame.as_coeffs();
        let axis_aligned = b.abs() < 1e-9 && c.abs() < 1e-9 && a > 0.0 && d > 0.0;
        let along_x = matches!(handle, Handle::E | Handle::W);
        if axis_aligned || !(along_x || matches!(handle, Handle::N | Handle::S)) {
            return self.snap_pointer(p, inp, exclude);
        }
        let l = frame.inverse() * p;
        let (at, axis) = if along_x { (Point::new(l.x, bbox.center().y), Vec2::new(a, b)) } else { (Point::new(bbox.center().x, l.y), Vec2::new(c, d)) };
        let axis = axis.normalize();
        let h = frame * at;
        let off = self.snap_pointer(h, inp, exclude) - h;
        let reach = 3.0 * self.tol();
        let travel = |off: f64, axis: f64| (off != 0.0 && (off / axis).abs() <= reach).then(|| off / axis);
        let (tx, ty) = (travel(off.x, axis.x), travel(off.y, axis.y));
        let Some(t) = [tx, ty].into_iter().flatten().min_by(|p, q| p.abs().total_cmp(&q.abs())) else {
            self.snap_guides.clear();
            return p;
        };
        // Only the lines the handle actually reaches keep a guide.
        let q = h + axis * t;
        let r = Rect::from_points(q, q);
        let hits = |v: Option<f64>| v.is_some_and(|v| (v - t).abs() < 1e-6);
        let (hit_x, hit_y) = (hits(tx), hits(ty));
        self.snap_guides.retain(|g| if g.vertical { hit_x } else { hit_y });
        for g in &mut self.snap_guides {
            g.moving = r;
        }
        q
    }

    /// Snap a pointer position while drawing or resizing (Ctrl disables).
    fn snap_pointer(&mut self, p: Point, inp: &Input, exclude: &[u64]) -> Point {
        if inp.cmd {
            self.snap_guides.clear();
            return p;
        }
        self.snap_point(p, exclude)
    }
}

/// `p` constrained to the nearest 45° direction from `from`.
/// Reverse a subpath's direction (handles swap sides).
pub fn reverse_subpath(sp: &mut SubPath) {
    sp.nodes.reverse();
    for n in &mut sp.nodes {
        std::mem::swap(&mut n.hin, &mut n.hout);
    }
}

pub fn constrain_45(from: Point, p: Point) -> Point {
    let d = p - from;
    let step = std::f64::consts::FRAC_PI_4;
    let a = (d.atan2() / step).round() * step;
    let dir = Vec2::new(a.cos(), a.sin());
    from + dir * d.dot(dir)
}

/// Current absolute rotation of what is being rotated: the shape's own angle
/// for a single non-path shape, else 0.
pub fn rotation_base(originals: &[Shape]) -> f64 {
    match originals {
        [s] if !matches!(s.kind, ShapeKind::Path(_)) => {
            let [a, b, ..] = s.transform.as_coeffs();
            b.atan2(a)
        }
        _ => 0.0,
    }
}

/// Artistic text drag: font size from the drag height and the top of the line.
fn artistic_drag(start: Point, cur: Point, line_height: f64) -> (f64, f64) {
    let h = (cur.y - start.y).abs().max(1.0);
    let size = (h / line_height.max(0.1)).clamp(1.0, 2000.0);
    (size, start.y.min(cur.y))
}

/// Rotation pivot marker: ring with a crosshair (filled when customized/active).
fn paint_pivot(painter: &egui::Painter, c: Pos2, strong: bool) {
    let accent = theme::ACCENT;
    let s = Stroke::new(1.5, if strong { accent } else { accent.gamma_multiply(0.75) });
    painter.circle_filled(c, 5.0, if strong { Color32::WHITE } else { Color32::from_white_alpha(170) });
    painter.circle_stroke(c, 5.0, s);
    painter.line_segment([c - vec2(9.0, 0.0), c - vec2(3.0, 0.0)], s);
    painter.line_segment([c + vec2(3.0, 0.0), c + vec2(9.0, 0.0)], s);
    painter.line_segment([c - vec2(0.0, 9.0), c - vec2(0.0, 3.0)], s);
    painter.line_segment([c + vec2(0.0, 3.0), c + vec2(0.0, 9.0)], s);
}

fn dashed_line(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 4.0, 3.0));
}

fn dashed_rect(painter: &egui::Painter, r: egui::Rect, stroke: Stroke) {
    let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(egui::Shape::dashed_line(&c, stroke, 4.0, 3.0));
}

/// "Drop to import" highlight while a drag hovers the canvas.
fn paint_drop_overlay(painter: &egui::Painter, rect: egui::Rect, at: Pos2) {
    let accent = theme::ACCENT;
    painter.rect_filled(rect, 0.0, accent.gamma_multiply(0.08));
    painter.rect_stroke(rect.shrink(3.0), CornerRadius::same(6), Stroke::new(2.0, accent), StrokeKind::Inside);
    // Drop point marker.
    painter.circle_stroke(at, 9.0, Stroke::new(1.5, accent));
    painter.line_segment([at - vec2(14.0, 0.0), at + vec2(14.0, 0.0)], Stroke::new(1.0, accent));
    painter.line_segment([at - vec2(0.0, 14.0), at + vec2(0.0, 14.0)], Stroke::new(1.0, accent));
    // Label pill.
    let text = format!("{}  Drop to import", egui_phosphor::regular::DOWNLOAD_SIMPLE);
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
    let c = pos2(rect.center().x, rect.top() + 40.0);
    let bg = egui::Rect::from_center_size(c, galley.size() + vec2(28.0, 14.0));
    painter.rect_filled(bg, CornerRadius::same(255), accent);
    painter.galley(bg.center() - galley.size() / 2.0, galley, Color32::WHITE);
    let sub = painter.layout_no_wrap("SVG · PDF · EPS · PNG · JPEG · .mvec".into(), egui::FontId::proportional(11.0), theme::TEXT_DIM);
    painter.galley(pos2(c.x - sub.size().x / 2.0, bg.bottom() + 6.0), sub, theme::TEXT_DIM);
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{:.0}", v) } else { format!("{:.1}", v) }
}

fn current_corner_radius(s: &Shape) -> f64 {
    match &s.kind {
        ShapeKind::Rect { radii, .. } => radii.iter().cloned().fold(0.0, f64::max),
        ShapeKind::Star { corner, .. } | ShapeKind::Polygon { corner, .. } => *corner,
        ShapeKind::Path(pd) => pd.subpaths.iter().flat_map(|s| s.nodes.iter()).map(|n| n.corner).fold(0.0, f64::max),
        _ => 0.0,
    }
}

/// Convert a sharp node into a smooth one with handles along the neighbor direction.
fn make_smooth(sp: &mut SubPath, ni: usize) {
    let n = sp.nodes.len();
    if n < 2 {
        return;
    }
    let prev = if ni == 0 { if sp.closed { n - 1 } else { ni } } else { ni - 1 };
    let next = if ni + 1 == n { if sp.closed { 0 } else { ni } } else { ni + 1 };
    let p = sp.nodes[ni].p;
    let a = sp.nodes[prev].p;
    let b = sp.nodes[next].p;
    let mut dir = b - a;
    if dir.hypot() < 1e-9 {
        dir = Vec2::new(1.0, 0.0);
    }
    let dir = dir.normalize();
    let l_in = (p - a).hypot() / 3.0;
    let l_out = (b - p).hypot() / 3.0;
    let node = &mut sp.nodes[ni];
    node.hin = Some(p - dir * l_in.max(1.0));
    node.hout = Some(p + dir * l_out.max(1.0));
    node.smooth = true;
    node.corner = 0.0;
}

fn drag_rect(start: Point, cur: Point, square: bool, center: bool) -> Rect {
    let mut d = cur - start;
    if square {
        let m = d.x.abs().max(d.y.abs());
        let sg = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
        d = Vec2::new(m * sg(d.x), m * sg(d.y));
    }
    if center {
        Rect::from_points(start - d, start + d)
    } else {
        Rect::from_points(start, start + d)
    }
}

fn scale_affine(h: Handle, b: Rect, p: Point, keep_ratio: bool, from_center: bool) -> Affine {
    let (mx, my) = match h {
        Handle::N => (0, -1),
        Handle::S => (0, 1),
        Handle::E => (1, 0),
        Handle::W => (-1, 0),
        Handle::NE => (1, -1),
        Handle::NW => (-1, -1),
        Handle::SE => (1, 1),
        Handle::SW => (-1, 1),
    };
    let c = b.center();
    let ax = if from_center { c.x } else if mx > 0 { b.x0 } else { b.x1 };
    let ay = if from_center { c.y } else if my > 0 { b.y0 } else { b.y1 };
    let ox = if mx > 0 { b.x1 } else { b.x0 };
    let oy = if my > 0 { b.y1 } else { b.y0 };
    let safe = |v: f64| if v.abs() < 1e-6 { 1e-6_f64.copysign(v) } else { v };
    let mut sx = if mx != 0 { (p.x - ax) / safe(ox - ax) } else { 1.0 };
    let mut sy = if my != 0 { (p.y - ay) / safe(oy - ay) } else { 1.0 };
    if keep_ratio {
        if mx != 0 && my != 0 {
            let s = sx.abs().max(sy.abs());
            sx = s * sx.signum();
            sy = s * sy.signum();
        } else if mx != 0 {
            sy = sx.abs();
        } else {
            sx = sy.abs();
        }
    }
    let clamp = |v: f64| if v.abs() < 1e-3 { 1e-3_f64.copysign(if v == 0.0 { 1.0 } else { v }) } else { v };
    Affine::translate((ax, ay)) * Affine::scale_non_uniform(clamp(sx), clamp(sy)) * Affine::translate((-ax, -ay))
}

/// Inward bisector and position of a corner in local space.
fn corner_geometry(nodes: &[Node], closed: bool, i: usize) -> Option<(Point, Vec2)> {
    let n = nodes.len();
    if n < 3 && !closed {
        return None;
    }
    let prev = if i == 0 { if closed { n - 1 } else { return None } } else { i - 1 };
    let next = if i + 1 == n { if closed { 0 } else { return None } } else { i + 1 };
    let p = nodes[i].p;
    let a = nodes[i].hin.unwrap_or(nodes[prev].hout.unwrap_or(nodes[prev].p));
    let b = nodes[i].hout.unwrap_or(nodes[next].hin.unwrap_or(nodes[next].p));
    let v1 = a - p;
    let v2 = b - p;
    if v1.hypot() < 1e-9 || v2.hypot() < 1e-9 {
        return None;
    }
    let bis = v1.normalize() + v2.normalize();
    if bis.hypot() < 1e-9 {
        return None;
    }
    Some((p, bis.normalize()))
}

fn set_corner_from_drag(s: &mut Shape, corner: usize, single: bool, p_doc: Point, node_sel: &[(u64, usize, usize)]) {
    let inv = s.transform.inverse();
    let lp = inv * p_doc;
    let id = s.id;
    match &mut s.kind {
        ShapeKind::Rect { w, h, radii } => {
            let pd = geom::rect_nodes(*w, *h, *radii);
            if let Some((c, bis)) = corner_geometry(&pd.subpaths[0].nodes, true, corner) {
                // The handle sits at (r, r) from the corner, i.e. r·√2 along the bisector.
                let d = (lp - c).dot(bis).max(0.0);
                let r = (d / std::f64::consts::SQRT_2).min(w.min(*h) / 2.0);
                if single {
                    radii[corner] = r;
                } else {
                    *radii = [r; 4];
                }
            }
        }
        ShapeKind::Star { .. } | ShapeKind::Polygon { .. } => {
            if let Some(pd) = geom::kind_nodes(&s.kind) {
                if let Some((c, bis)) = corner_geometry(&pd.subpaths[0].nodes, true, corner) {
                    let r = (lp - c).dot(bis).max(0.0);
                    if let ShapeKind::Star { corner: cr, .. } | ShapeKind::Polygon { corner: cr, .. } = &mut s.kind {
                        *cr = r;
                    }
                }
            }
        }
        ShapeKind::Path(pd) => {
            // Map flat corner index to (subpath, node).
            let mut k = 0;
            let mut target = None;
            for (si, sp) in pd.subpaths.iter().enumerate() {
                if corner < k + sp.nodes.len() {
                    target = Some((si, corner - k));
                    break;
                }
                k += sp.nodes.len();
            }
            let Some((si, ni)) = target else { return };
            let sp = &pd.subpaths[si];
            let Some((c, bis)) = corner_geometry(&sp.nodes, sp.closed, ni) else { return };
            let r = (lp - c).dot(bis).max(0.0);
            let apply_all = node_sel.contains(&(id, si, ni)) && node_sel.len() > 1;
            if apply_all && !single {
                for (sid, a, b) in node_sel {
                    if *sid == id {
                        if let Some(n) = pd.subpaths.get_mut(*a).and_then(|sp| sp.nodes.get_mut(*b)) {
                            n.corner = r;
                        }
                    }
                }
            } else {
                pd.subpaths[si].nodes[ni].corner = r;
            }
        }
        _ => {}
    }
}

/// The shear for dragging by `d` from `start`. A grabbed edge moves with
/// the pointer while the opposite edge (or, with `alt` or inside the box,
/// the centre line) stays; `snap` rounds the angle to 15°.
pub(crate) fn shear_xf(b: Rect, horizontal: bool, edge: Option<f64>, start: Point, d: kurbo::Vec2, alt: bool, snap: bool) -> Affine {
    let c = b.center();
    let (lo, hi, mid) = if horizontal { (b.y0, b.y1, c.y) } else { (b.x0, b.x1, c.x) };
    let grab = edge.unwrap_or(if horizontal { start.y } else { start.x });
    let anchor = if alt || edge.is_none() { mid } else if (grab - lo).abs() < (grab - hi).abs() { hi } else { lo };
    let lever = grab - anchor;
    let lever = if lever.abs() < 1e-6 { (hi - lo) / 2.0 } else { lever };
    let mut k = if horizontal { d.x } else { d.y } / lever;
    if snap {
        let step = 15f64.to_radians();
        k = ((k.atan() / step).round() * step).clamp(-1.4, 1.4).tan();
    }
    let k = k.clamp(-20.0, 20.0);
    if horizontal { Affine::new([1.0, 0.0, k, 1.0, -k * anchor, 0.0]) } else { Affine::new([1.0, k, 0.0, 1.0, 0.0, -k * anchor]) }
}

/// Shear angle (degrees) of a shear transform: positive leans the top to
/// the right (horizontal) or the right side up (vertical).
fn shear_angle(xf: Affine, horizontal: bool) -> f64 {
    let c = xf.as_coeffs();
    -(if horizontal { c[2] } else { c[1] }).atan().to_degrees()
}

impl App {
    /// The shear zone under screen point `sp`: just beside a middle
    /// handle, along its edge (like Affinity). Returns (top or bottom edge,
    /// the edge's coordinate in the frame's space).
    fn shear_edge_at(&mut self, sp: Pos2) -> Option<(bool, f64)> {
        let g = self.gizmo()?;
        if g.handles.iter().any(|(_, hp)| hp.distance(sp) <= HANDLE_R + 1.0) {
            return None;
        }
        let (_, b) = self.sel_frame()?;
        let c = g.corners;
        // Corners: top-left, top-right, bottom-right, bottom-left; each edge
        // with its middle handle.
        let edges = [(c[0], c[1], true, b.y0), (c[1], c[2], false, b.x1), (c[2], c[3], true, b.y1), (c[3], c[0], false, b.x0)];
        edges
            .into_iter()
            .find(|(a, e, ..)| {
                let mid = *a + (*e - *a) * 0.5;
                // The box must be big enough to leave room beside the handle.
                a.distance(*e) > 6.0 * HANDLE_R && mid.distance(sp) <= HANDLE_R + 12.0 && seg_dist(sp, *a, *e) <= 6.0
            })
            .map(|(_, _, h, edge)| (h, edge))
    }

    #[cfg(test)]
    /// Shear the selection by `degrees` about its centre.
    pub fn shear_selection(&mut self, horizontal: bool, degrees: f64) {
        let Some(b) = self.selection_bounds() else { return };
        if degrees.abs() < 1e-9 {
            return;
        }
        self.checkpoint("Shear");
        // Positive angles lean the top right / the right side up.
        let k = -degrees.clamp(-85.0, 85.0).to_radians().tan();
        let c = b.center();
        let xf = if horizontal { Affine::new([1.0, 0.0, k, 1.0, -k * c.y, 0.0]) } else { Affine::new([1.0, k, 0.0, 1.0, 0.0, -k * c.x]) };
        for id in self.selection.clone() {
            if let Some(mut s) = self.doc.get(id).cloned() {
                s.apply_transform(xf);
                self.doc.replace(s);
            }
        }
    }
}
