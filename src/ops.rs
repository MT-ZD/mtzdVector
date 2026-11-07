//! Document utilities used by commands and the context menu: distribute,
//! expand stroke, style copy/paste, visibility, path breaking.

use kurbo::Affine;

use crate::app::{App, StyleClip};
use crate::document::{FillRule, Paint, Shape, ShapeKind, Stroke, SubPath, TextAlign};
use crate::geom;

/// Split a subpath at the given node indices.
pub fn break_subpath(sp: &SubPath, idxs: &[usize]) -> Vec<SubPath> {
    let n = sp.nodes.len();
    let mut idxs: Vec<usize> = idxs.iter().copied().filter(|i| *i < n).collect();
    idxs.sort_unstable();
    idxs.dedup();
    if idxs.is_empty() || n < 2 {
        return vec![sp.clone()];
    }
    let mut nodes = sp.nodes.clone();
    if sp.closed {
        // Open the loop at the first break point: it becomes both ends.
        let k0 = idxs[0];
        nodes.rotate_left(k0);
        let mut first = nodes[0].clone();
        first.hin = None;
        let mut last = nodes[0].clone();
        last.hout = None;
        nodes[0] = first;
        nodes.push(last);
        idxs = idxs[1..].iter().map(|j| j - k0).collect();
    }
    // Split the open run at each remaining interior index.
    let mut out = Vec::new();
    let mut start = 0;
    for &i in &idxs {
        if i == 0 || i + 1 >= nodes.len() || i <= start {
            continue;
        }
        let mut part: Vec<_> = nodes[start..=i].to_vec();
        if let Some(l) = part.last_mut() {
            l.hout = None;
        }
        out.push(part);
        start = i;
    }
    let mut rest: Vec<_> = nodes[start..].to_vec();
    if start > 0 {
        if let Some(f) = rest.first_mut() {
            f.hin = None;
        }
    }
    out.push(rest);
    out.into_iter().filter(|p| p.len() >= 2).map(|nodes| SubPath { nodes, closed: false }).collect()
}

impl App {
    /// Distribute the selection with equal gaps (needs 3+ objects).
    pub fn distribute(&mut self, horizontal: bool) {
        let mut items: Vec<(u64, kurbo::Rect)> = Vec::new();
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get(id).cloned() {
                if let Some(b) = self.cache.update(&s, &mut self.fonts).bounds {
                    items.push((id, b));
                }
            }
        }
        if items.len() < 3 {
            return;
        }
        let key = |r: &kurbo::Rect| if horizontal { r.center().x } else { r.center().y };
        items.sort_by(|a, b| key(&a.1).total_cmp(&key(&b.1)));
        let (lo, hi) = if horizontal {
            (items.first().unwrap().1.x0, items.iter().map(|i| i.1.x1).fold(f64::MIN, f64::max))
        } else {
            (items.first().unwrap().1.y0, items.iter().map(|i| i.1.y1).fold(f64::MIN, f64::max))
        };
        let total: f64 = items.iter().map(|i| if horizontal { i.1.width() } else { i.1.height() }).sum();
        let gap = (hi - lo - total) / (items.len() - 1) as f64;
        self.checkpoint("");
        let mut cur = lo;
        for (id, b) in items {
            let d = if horizontal { (cur - b.x0, 0.0) } else { (0.0, cur - b.y0) };
            if let Some(s) = self.doc.get_mut(id) {
                s.apply_transform(Affine::translate(d));
            }
            cur += (if horizontal { b.width() } else { b.height() }) + gap;
        }
    }

    /// Replace each stroke by a filled outline. Shapes that also have a fill
    /// keep it (without stroke) underneath the new outline.
    pub fn expand_stroke(&mut self) {
        let targets: Vec<Shape> = self.selected_shapes().into_iter().filter(|s| s.stroke.as_ref().map(|st| st.width > 0.0).unwrap_or(false)).cloned().collect();
        if targets.is_empty() {
            self.notify("Select a shape with a stroke");
            return;
        }
        self.checkpoint("");
        let mut sel: Vec<u64> = self.selection.iter().copied().filter(|id| !targets.iter().any(|t| t.id == *id)).collect();
        for s in targets {
            let st = s.stroke.clone().unwrap();
            let path = self.cache.update(&s, &mut self.fonts).doc_path.clone();
            let area = crate::strokes::stroke_area(&path, &st, s.fill_rule);
            let outline = geom::polys_to_pathdata_smooth(&area);
            let mut out = s.clone();
            out.id = 0;
            out.kind = ShapeKind::Path(outline);
            out.transform = Affine::IDENTITY;
            out.fill = Some(Paint::Solid(st.color));
            out.stroke = None;
            out.fill_rule = FillRule::EvenOdd;
            out.name = format!("{} (stroke)", s.name);
            let idx = self.doc.index_of(s.id).unwrap_or(0);
            if s.fill.is_some() {
                let mut base = s.clone();
                base.stroke = None;
                self.doc.replace(base);
                sel.push(s.id);
                sel.push(self.doc.insert_shape(idx + 1, out));
            } else {
                self.doc.remove(s.id);
                sel.push(self.doc.insert_shape(idx, out));
            }
        }
        self.selection = sel;
        self.notify("Expanded stroke");
    }

    /// Bake the outline effect into real shapes: one filled ring per step
    /// (with its colour or gradient) under the shape, which loses the effect.
    pub fn expand_outlines(&mut self) {
        let targets: Vec<Shape> = self.selected_shapes().into_iter().filter(|s| !s.outlines.is_empty()).cloned().collect();
        if targets.is_empty() {
            self.notify("Select a shape with an outline effect");
            return;
        }
        self.checkpoint("");
        let mut sel: Vec<u64> = self.selection.clone();
        for s in targets {
            let path = self.cache.update(&s, &mut self.fonts).doc_path.clone();
            let rings = crate::render::outline_rings(&s, &path, &mut self.fonts);
            let mut idx = self.doc.index_of(s.id).unwrap_or(0);
            let n = rings.len();
            for (k, (rp, paint)) in rings.into_iter().enumerate() {
                let mut out = Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&rp)));
                out.name = if n == 1 { format!("{} (outline)", s.name) } else { format!("{} (outline {})", s.name, k + 1) };
                out.stroke = None;
                out.fill_rule = FillRule::EvenOdd;
                out.opacity = s.opacity;
                out.clip = s.clip.clone();
                out.fill = Some(match paint {
                    Paint::Gradient(mut g) => {
                        // The ring's gradient lives in its own box under the
                        // shape's transform: move it to the new path's box.
                        let lb = crate::render::ring_bounds(&s, &rp);
                        let from = s.transform * Affine::new([lb.width().max(1e-9), 0.0, 0.0, lb.height().max(1e-9), lb.x0, lb.y0]);
                        let nb = geom::path_bounds(&rp).unwrap_or(lb);
                        let to = Affine::new([nb.width().max(1e-9), 0.0, 0.0, nb.height().max(1e-9), nb.x0, nb.y0]);
                        let m = to.inverse() * from;
                        for p in [&mut g.start, &mut g.end] {
                            let q = m * kurbo::Point::new(p[0], p[1]);
                            *p = [q.x, q.y];
                        }
                        Paint::Gradient(g)
                    }
                    other => other,
                });
                sel.push(self.doc.insert_shape(idx, out));
                idx += 1;
            }
            if let Some(o) = self.doc.get_mut(s.id) {
                o.outlines.clear();
            }
        }
        self.selection = sel;
        self.notify("Expanded outline effect");
    }

    /// Toggle the selected texts between artistic and frame text. Artistic
    /// text becomes a frame around its current layout.
    pub fn convert_text_type(&mut self) {
        let ids: Vec<u64> = self.selection.iter().copied().filter(|id| matches!(self.doc.get(*id).map(|s| &s.kind), Some(ShapeKind::Text(_)))).collect();
        if ids.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in ids {
            let Some(s) = self.doc.get(id).cloned() else { continue };
            let size = self.cache.update(&s, &mut self.fonts).text.as_ref().map(|t| (t.bounds.width(), t.bounds.height()));
            if let Some(s) = self.doc.get_mut(id) {
                if let ShapeKind::Text(td) = &mut s.kind {
                    td.frame = match td.frame {
                        Some(_) => None,
                        None => size.map(|(w, h)| [w.ceil() + 2.0, h.ceil()]),
                    };
                }
            }
        }
    }

    pub fn set_opacity(&mut self, o: f32) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("opacity-key");
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                s.opacity = o;
            }
        }
        self.notify(format!("Opacity {:.0}%", o * 100.0));
    }

    pub fn copy_style(&mut self) {
        if let Some(s) = self.selected_shapes().first() {
            self.style_clip = Some(StyleClip { fill: s.fill.clone(), stroke: s.stroke.clone(), opacity: s.opacity, shadow: s.shadow.clone(), inner_shadow: s.inner_shadow.clone(), blur: s.blur, noise: s.noise, outlines: s.outlines.clone(), fill_key: s.fill_key, stroke_key: s.stroke_key });
            self.notify("Style copied");
        }
    }

    pub fn paste_style(&mut self) {
        let Some(st) = self.style_clip.clone() else { return };
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                s.fill = st.fill.clone();
                s.stroke = st.stroke.clone();
                s.opacity = st.opacity;
                s.shadow = st.shadow.clone();
                s.inner_shadow = st.inner_shadow.clone();
                s.blur = st.blur;
                s.noise = st.noise;
                s.outlines = st.outlines.clone();
                s.fill_key = st.fill_key;
                s.stroke_key = st.stroke_key;
            }
        }
    }

    pub fn toggle_hidden(&mut self) {
        let any_visible = self.selected_shapes().iter().any(|s| s.visible);
        self.checkpoint("");
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                s.visible = !any_visible;
            }
        }
    }

    pub fn toggle_locked(&mut self) {
        let any_unlocked = self.selected_shapes().iter().any(|s| !s.locked);
        self.checkpoint("");
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                s.locked = any_unlocked;
            }
        }
        if any_unlocked {
            self.selection.clear();
            self.node_sel.clear();
        }
    }

    /// Split paths at the selected nodes.
    pub fn break_at_nodes(&mut self) {
        if self.node_sel.is_empty() {
            return;
        }
        self.checkpoint("");
        let sel = std::mem::take(&mut self.node_sel);
        let mut ids: Vec<u64> = sel.iter().map(|k| k.0).collect();
        ids.dedup();
        for id in ids {
            let Some(s) = self.doc.get_mut(id) else { continue };
            let ShapeKind::Path(pd) = &mut s.kind else { continue };
            let mut subs = Vec::new();
            for (si, sp) in pd.subpaths.iter().enumerate() {
                let idxs: Vec<usize> = sel.iter().filter(|k| k.0 == id && k.1 == si).map(|k| k.2).collect();
                subs.extend(break_subpath(sp, &idxs));
            }
            pd.subpaths = subs;
            if s.fill.is_some() && s.stroke.is_none() {
                s.stroke = Some(Stroke { width: 2.0, ..Default::default() });
            }
        }
    }

    pub fn set_text_align(&mut self, align: TextAlign) {
        self.checkpoint("");
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                if let ShapeKind::Text(t) = &mut s.kind {
                    t.align = align;
                }
            }
        }
    }
}

pub fn op_name(op: crate::boolean::BoolOp) -> &'static str {
    match op {
        crate::boolean::BoolOp::Union => "Union",
        crate::boolean::BoolOp::Subtract => "Subtract",
        crate::boolean::BoolOp::Intersect => "Intersect",
        crate::boolean::BoolOp::Exclude => "Exclude",
    }
}

impl App {
    /// Selected shapes ordered bottom-most first.
    pub fn selected_bottom_up(&self) -> Vec<Shape> {
        let mut shapes: Vec<Shape> = self.selected_shapes().into_iter().cloned().collect();
        shapes.sort_by_key(|s| self.doc.order_key(s.id));
        // Drop shapes whose container is also selected.
        let ids: Vec<u64> = shapes.iter().map(|s| s.id).collect();
        shapes.retain(|s| {
            let mut p = self.doc.parent_of(s.id);
            while let Some(x) = p {
                if ids.contains(&x) {
                    return false;
                }
                p = self.doc.parent_of(x);
            }
            true
        });
        shapes
    }

    /// Replace `shapes` (bottom-up) by `new`, placed where the top-most one was.
    pub(crate) fn combine_into(&mut self, shapes: &[Shape], new: Shape) -> u64 {
        let top = shapes.last().unwrap().id;
        for s in &shapes[..shapes.len() - 1] {
            self.doc.remove(s.id);
        }
        self.doc.replace_with(top, vec![new])[0]
    }

    /// Group the selection (Ctrl+G).
    pub fn group_selection(&mut self) {
        let shapes = self.selected_bottom_up();
        if shapes.is_empty() {
            return;
        }
        self.checkpoint("");
        let mut g = Shape::new(ShapeKind::Group { children: shapes.clone() });
        g.fill = None;
        g.name = "Group".into();
        let id = self.combine_into(&shapes, g);
        self.selection = vec![id];
    }

    /// Dissolve selected groups and booleans into their children (Ctrl+Shift+G).
    pub fn ungroup(&mut self) {
        let targets: Vec<Shape> = self.selected_shapes().into_iter().filter(|s| s.kind.is_container()).cloned().collect();
        if targets.is_empty() {
            return;
        }
        self.checkpoint("");
        let mut sel: Vec<u64> = self.selection.iter().copied().filter(|id| !targets.iter().any(|t| t.id == *id)).collect();
        for s in targets {
            let Some(children) = s.kind.children().cloned() else { continue };
            // Group opacity carries over to the children.
            let children: Vec<Shape> = children
                .into_iter()
                .map(|mut c| {
                    if matches!(s.kind, ShapeKind::Group { .. }) {
                        c.opacity *= s.opacity;
                    }
                    c
                })
                .collect();
            sel.extend(self.doc.replace_with(s.id, children));
        }
        self.selection = sel;
    }

    /// Group the selection into a live (non-destructive) boolean.
    pub fn make_compound(&mut self, op: crate::boolean::BoolOp) {
        let shapes = self.selected_bottom_up();
        if shapes.len() < 2 {
            self.notify("Select at least two shapes");
            return;
        }
        self.checkpoint("");
        // Like Figma: subtract keeps the bottom shape's style, the others the top's.
        let style = if op == crate::boolean::BoolOp::Subtract { &shapes[0] } else { shapes.last().unwrap() };
        let mut ns = Shape::new(ShapeKind::Compound { op, children: shapes.clone() });
        ns.fill = style.fill.clone().or_else(|| Some(Paint::Solid(crate::document::Rgba::from_hex("#D9D9D9").unwrap())));
        ns.stroke = style.stroke.clone();
        ns.opacity = style.opacity;
        ns.shadow = style.shadow.clone();
        ns.inner_shadow = style.inner_shadow.clone();
        ns.fill_rule = FillRule::EvenOdd;
        ns.name = op_name(op).into();
        let id = self.combine_into(&shapes, ns);
        self.selection = vec![id];
    }

    /// Bake the selection into plain paths: compounds and primitives become
    /// curves; several shapes merge into one path with the top shape's style.
    pub fn flatten(&mut self) {
        let shapes = self.selected_bottom_up();
        if shapes.is_empty() {
            return;
        }
        // A group (auto layout too) flattens its visible contents.
        let group = shapes.len() == 1 && matches!(shapes[0].kind, ShapeKind::Group { .. });
        if shapes.len() == 1 && !group {
            self.convert_to_curves();
            return;
        }
        fn leaves(s: &crate::document::Shape, out: &mut Vec<crate::document::Shape>) {
            if !s.visible {
                return;
            }
            match &s.kind {
                ShapeKind::Group { children } => children.iter().for_each(|c| leaves(c, out)),
                _ => out.push(s.clone()),
            }
        }
        let mut parts = Vec::new();
        shapes.iter().for_each(|s| leaves(s, &mut parts));
        let Some(top) = parts.last().cloned() else { return };
        self.checkpoint("");
        let mut merged = kurbo::BezPath::new();
        for s in &parts {
            let p = self.cache.update(s, &mut self.fonts).doc_path.clone();
            merged.extend(p.iter());
        }
        let mut ns = top;
        ns.id = 0;
        ns.kind = ShapeKind::Path(geom::bez_to_pathdata(&merged));
        ns.transform = Affine::IDENTITY;
        ns.clip = None;
        ns.layout = None;
        ns.constraints = None;
        ns.component = None;
        ns.name = "Vector".into();
        let id = self.combine_into(&shapes, ns);
        self.selection = vec![id];
        self.notify("Flattened");
    }

    /// The top-most selected shape becomes the clip mask of the others.
    pub fn make_clip(&mut self) {
        let shapes = self.selected_bottom_up();
        if shapes.len() < 2 {
            self.notify("Select the shapes to clip and the clipping shape on top");
            return;
        }
        self.checkpoint("");
        let mask = shapes.last().unwrap().clone();
        self.doc.remove(mask.id);
        let mut sel = Vec::new();
        let mut m = mask.clone();
        m.clip = None;
        for s in &shapes[..shapes.len() - 1] {
            // Groups don't paint: the mask goes onto every shape inside.
            let saved = std::mem::replace(&mut self.selection, vec![s.id]);
            let leaves = self.style_targets();
            self.selection = saved;
            for id in leaves {
                if let Some(t) = self.doc.get_mut(id) {
                    t.clip = Some(Box::new(m.clone()));
                }
            }
            sel.push(s.id);
        }
        self.selection = sel;
        self.notify("Clipped");
    }

    /// Remove the clip mask, restoring it as a shape above.
    pub fn release_clip(&mut self) {
        let targets: Vec<u64> = self.selected_shapes().iter().filter(|s| s.clip.is_some()).map(|s| s.id).collect();
        if targets.is_empty() {
            return;
        }
        self.checkpoint("");
        let mut sel = Vec::new();
        for id in targets {
            let Some(mask) = self.doc.get_mut(id).and_then(|s| s.clip.take()) else { continue };
            let idx = self.doc.index_of(id).unwrap_or(0);
            sel.push(id);
            let mut m = *mask;
            m.id = 0;
            sel.push(self.doc.insert_shape(idx + 1, m));
        }
        self.selection = sel;
    }

    /// Flow the selected text along the selected path (the path is consumed).
    pub fn text_on_path(&mut self) {
        let shapes = self.selected_bottom_up();
        let Some(text) = shapes.iter().find(|s| matches!(s.kind, ShapeKind::Text(_))).cloned() else { return };
        let Some(guide) = shapes.iter().find(|s| !matches!(s.kind, ShapeKind::Text(_))).cloned() else { return };
        let path = self.cache.update(&guide, &mut self.fonts).doc_path.clone();
        let local = text.transform.inverse() * (*path).clone();
        self.checkpoint("");
        if let Some(t) = self.doc.get_mut(text.id) {
            if let ShapeKind::Text(td) = &mut t.kind {
                td.path = Some(geom::bez_to_pathdata(&local));
                td.path_offset = 0.0;
                td.frame = None;
            }
        }
        self.doc.remove(guide.id);
        self.selection = vec![text.id];
        self.notify("Text on path");
    }

    /// Turn path text back into a straight artistic text plus its guide path.
    pub fn release_text_path(&mut self) {
        let targets: Vec<Shape> = self.selected_shapes().into_iter().filter(|s| matches!(&s.kind, ShapeKind::Text(t) if t.path.is_some())).cloned().collect();
        if targets.is_empty() {
            return;
        }
        self.checkpoint("");
        let mut sel = Vec::new();
        for s in targets {
            let ShapeKind::Text(td) = &s.kind else { continue };
            let Some(pd) = td.path.clone() else { continue };
            let idx = self.doc.index_of(s.id).unwrap_or(0);
            let mut guide_pd = pd;
            guide_pd.transform(s.transform);
            let mut guide = Shape::new(ShapeKind::Path(guide_pd));
            guide.fill = None;
            guide.stroke = Some(Stroke { width: 1.0, ..Default::default() });
            guide.name = "Text path".into();
            if let Some(t) = self.doc.get_mut(s.id) {
                if let ShapeKind::Text(td) = &mut t.kind {
                    td.path = None;
                }
            }
            sel.push(s.id);
            sel.push(self.doc.insert_shape(idx, guide));
        }
        self.selection = sel;
    }
}

impl App {
    /// Insert a downloaded Iconify icon at the view center, selected.
    pub fn insert_icon(&mut self, name: &str) {
        self.insert_icon_at(name, None);
    }

    /// Insert icon `name` centred on `at` (else the view centre).
    pub fn insert_icon_at(&mut self, name: &str, at: Option<kurbo::Point>) {
        let Some(svg) = self.icons.svg(name).map(String::from) else { return };
        self.checkpoint("");
        let ids = match crate::io::svg::import_into(svg.as_bytes(), &mut self.fonts, &mut self.doc) {
            Ok(ids) if !ids.is_empty() => ids,
            _ => {
                self.notify(format!("Could not import {name}"));
                return;
            }
        };
        let mut bounds: Option<kurbo::Rect> = None;
        for id in &ids {
            if let Some(s) = self.doc.get(*id).cloned() {
                if let Some(b) = self.cache.update(&s, &mut self.fonts).bounds {
                    bounds = geom::union_rect(bounds, b);
                }
            }
        }
        let center = if let Some(p) = at {
            p
        } else if self.canvas_rect.is_positive() { self.view.to_doc(self.canvas_rect.center()) } else { self.active().rect().center() };
        if let Some(b) = bounds {
            let d = center - b.center();
            for id in &ids {
                if let Some(s) = self.doc.get_mut(*id) {
                    s.apply_transform(Affine::translate(d));
                }
            }
        }
        for id in &ids {
            if let Some(s) = self.doc.get_mut(*id) {
                s.name = name.to_string();
            }
        }
        // Dropped on an auto layout: the icon joins it.
        if let Some(g) = at.and_then(|p| self.layout_at(p, &ids)) {
            self.join_layout(&ids, g);
        }
        self.selection = ids;
        self.tool = crate::app::Tool::Select;
        self.notify(format!("Inserted {name}"));
    }
}

impl App {
    /// Shapes that style edits apply to: the selection with groups replaced by
    /// their (styleable) descendants, like Figma.
    pub fn style_targets(&self) -> Vec<u64> {
        fn walk(s: &crate::document::Shape, seen: &mut std::collections::HashSet<u64>, out: &mut Vec<u64>) {
            // An auto layout's fill and stroke are its background's.
            if let Some(bg) = s.layout.as_ref().and_then(|l| l.background).and_then(|b| s.kind.children()?.iter().find(|c| c.id == b)) {
                if seen.insert(bg.id) {
                    out.push(bg.id);
                }
                return;
            }
            match &s.kind {
                ShapeKind::Group { children } => {
                    for c in children {
                        walk(c, seen, out);
                    }
                }
                _ => {
                    if seen.insert(s.id) {
                        out.push(s.id);
                    }
                }
            }
        }
        let (mut out, mut seen) = (Vec::new(), std::collections::HashSet::new());
        for id in &self.selection {
            if let Some(s) = self.doc.get(*id) {
                walk(s, &mut seen, &mut out);
            }
        }
        out
    }
}
