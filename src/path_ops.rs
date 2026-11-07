//! Path operations: node nudge, join, reverse, simplify, offset and the
//! knife tool.

use kurbo::{Affine, BezPath, Point, Vec2};

use crate::app::App;
use crate::boolean::{self, BoolOp};
use crate::document::{FillRule, LineCap, LineJoin, PathData, Shape, ShapeKind, Stroke, SubPath};
use crate::geom;

/// Offset Path dialog state.
#[derive(Clone, Debug)]
pub struct OffsetState {
    pub open: bool,
    /// Positive grows, negative insets (px).
    pub distance: f64,
    pub round: bool,
}

impl Default for OffsetState {
    fn default() -> Self {
        OffsetState { open: false, distance: 10.0, round: true }
    }
}

impl App {
    /// Arrow keys in the node tool: move the selected nodes with their handles.
    pub fn nudge_nodes(&mut self, d: Vec2) {
        let sel = self.node_sel.clone();
        let a = Affine::translate(d);
        for id in self.node_targets_of(&sel) {
            if let Some(s) = self.doc.get_mut(id) {
                if let ShapeKind::Path(pd) = &mut s.kind {
                    for (si, sp) in pd.subpaths.iter_mut().enumerate() {
                        for (ni, n) in sp.nodes.iter_mut().enumerate() {
                            if sel.contains(&(id, si, ni)) {
                                n.transform(a);
                            }
                        }
                    }
                }
            }
        }
    }

    fn node_targets_of(&self, sel: &[(u64, usize, usize)]) -> Vec<u64> {
        let mut ids: Vec<u64> = sel.iter().map(|k| k.0).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    fn selected_paths(&self) -> Vec<u64> {
        self.selection.iter().copied().filter(|id| matches!(self.doc.get(*id).map(|s| &s.kind), Some(ShapeKind::Path(_)))).collect()
    }

    /// Reverse the direction of the selected paths (affects arrowheads and path text).
    pub fn reverse_paths(&mut self) {
        let ids = self.selected_paths();
        if ids.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in ids {
            if let Some(ShapeKind::Path(pd)) = self.doc.get_mut(id).map(|s| &mut s.kind) {
                for sp in &mut pd.subpaths {
                    reverse_subpath(sp);
                }
            }
        }
        self.node_sel.clear();
    }

    /// Two selected end nodes: connect them (closing the subpath, or merging
    /// two subpaths — of the same or different paths). Coincident ends merge
    /// into one node.
    pub fn join_nodes(&mut self) -> Result<(), String> {
        let sel = self.node_sel.clone();
        let ends: Vec<(u64, usize, bool)> = sel
            .iter()
            .filter_map(|&(id, si, ni)| {
                let ShapeKind::Path(pd) = &self.doc.get(id)?.kind else { return None };
                let sp = pd.subpaths.get(si)?;
                if sp.closed || sp.nodes.is_empty() {
                    return None;
                }
                if ni == 0 {
                    Some((id, si, false))
                } else if ni + 1 == sp.nodes.len() {
                    Some((id, si, true))
                } else {
                    None
                }
            })
            .collect();
        let [a, b] = ends[..] else { return Err("Select two end nodes of open paths to join".into()) };
        self.checkpoint("");
        // Bring the second path's subpath into the first shape.
        let (sa, sb) = if a.0 == b.0 {
            (a.1, b.1)
        } else {
            let other = self.doc.get(b.0).cloned().ok_or("missing")?;
            let ShapeKind::Path(opd) = &other.kind else { return Err("not a path".into()) };
            let mut sub = opd.subpaths[b.1].clone();
            let to_a = self.doc.get(a.0).map(|s| s.transform.inverse()).unwrap_or_default() * other.transform;
            for n in &mut sub.nodes {
                n.transform(to_a);
            }
            let Some(ShapeKind::Path(pd)) = self.doc.get_mut(a.0).map(|s| &mut s.kind) else { return Err("not a path".into()) };
            pd.subpaths.push(sub);
            let idx = pd.subpaths.len() - 1;
            // The rest of the other path stays (or the shape goes when emptied).
            let rest: Vec<SubPath> = opd.subpaths.iter().enumerate().filter(|(i, _)| *i != b.1).map(|(_, s)| s.clone()).collect();
            if rest.is_empty() {
                self.doc.remove(b.0);
                self.selection.retain(|x| *x != b.0);
            } else if let Some(ShapeKind::Path(p)) = self.doc.get_mut(b.0).map(|s| &mut s.kind) {
                p.subpaths = rest;
            }
            (a.1, idx)
        };
        let id = a.0;
        let Some(ShapeKind::Path(pd)) = self.doc.get_mut(id).map(|s| &mut s.kind) else { return Err("not a path".into()) };
        let tol = 1e-6;
        if sa == sb {
            let sp = &mut pd.subpaths[sa];
            let (f, l) = (sp.nodes[0].p, sp.nodes[sp.nodes.len() - 1].p);
            if (f - l).hypot() < tol && sp.nodes.len() > 1 {
                let last = sp.nodes.pop().unwrap();
                sp.nodes[0].hin = last.hin;
            }
            sp.closed = true;
        } else {
            // Orient so that A's chosen end is its last node and B's is its first.
            let (a_end, b_end) = (a.2, b.2);
            let mut first = pd.subpaths[sa].clone();
            let mut second = pd.subpaths[sb].clone();
            if !a_end {
                reverse_subpath(&mut first);
            }
            if b_end {
                reverse_subpath(&mut second);
            }
            let (fl, sf) = (first.nodes.last().unwrap().p, second.nodes[0].p);
            let mut nodes = first.nodes;
            if (fl - sf).hypot() < tol {
                let n0 = second.nodes.remove(0);
                nodes.last_mut().unwrap().hout = n0.hout;
            }
            nodes.extend(second.nodes);
            let (lo, hi) = (sa.min(sb), sa.max(sb));
            pd.subpaths.remove(hi);
            pd.subpaths[lo] = SubPath { nodes, closed: false };
        }
        self.node_sel.clear();
        self.selection = vec![id];
        Ok(())
    }

    /// Fewer nodes, same shape (within a fraction of the size).
    pub fn simplify_paths(&mut self) {
        let ids = self.selected_paths();
        if ids.is_empty() {
            return;
        }
        self.checkpoint("");
        let mut before = 0;
        let mut after = 0;
        for id in ids {
            let Some(s) = self.doc.get_mut(id) else { continue };
            let ShapeKind::Path(pd) = &mut s.kind else { continue };
            let mut subs = Vec::new();
            for sp in &pd.subpaths {
                before += sp.nodes.len();
                let bez = geom::pathdata_to_bez(&PathData { subpaths: vec![sp.clone()] });
                let diag = geom::path_bounds(&bez).map(|b| (b.width().powi(2) + b.height().powi(2)).sqrt()).unwrap_or(1.0);
                let fit = geom::simplify_subpath(sp, (diag * 0.003).clamp(0.1, 4.0));
                let ok = if fit.nodes.len() < sp.nodes.len() { fit } else { sp.clone() };
                after += ok.nodes.len();
                subs.push(ok);
            }
            pd.subpaths = subs;
        }
        self.node_sel.clear();
        self.notify(format!("Simplified: {before} → {after} nodes"));
    }

    /// New shapes outset (or inset, negative distance) from the selection,
    /// placed above each original.
    pub fn offset_selection(&mut self, distance: f64, round: bool) {
        let shapes = self.selected_bottom_up();
        if shapes.is_empty() || distance.abs() < 1e-9 {
            return;
        }
        self.checkpoint("");
        let mut new_sel = Vec::new();
        for s in shapes {
            let id = s.id;
            let (path, _) = crate::render::compute_outline(&s, &mut self.fonts);
            let Some(pd) = offset_path(&path, s.fill_rule, distance, round) else { continue };
            let mut ns = Shape::new(ShapeKind::Path(pd));
            ns.fill = s.fill.clone();
            ns.stroke = s.stroke.clone();
            ns.fill_rule = FillRule::NonZero;
            ns.name = format!("{} offset", s.name);
            crate::render::keep_gradient(&s, &mut ns, &mut self.fonts);
            let idx = self.doc.root_of(id).and_then(|r| self.doc.index_of(r)).map(|i| i + 1).unwrap_or(usize::MAX);
            new_sel.push(self.doc.insert_shape(idx, ns));
        }
        if new_sel.is_empty() {
            self.notify("Nothing to offset (inset too large?)");
        } else {
            self.selection = new_sel;
        }
    }

    /// Cut the filled shapes crossed by the knife stroke `pts` into pieces.
    /// Applies to the selection, or to every shape the cut crosses.
    pub fn knife_cut(&mut self, pts: &[Point]) -> usize {
        if pts.len() < 2 {
            return 0;
        }
        let mut line = BezPath::new();
        line.move_to(pts[0]);
        for p in &pts[1..] {
            line.line_to(*p);
        }
        let blade = Stroke { width: 0.02, join: LineJoin::Round, cap: LineCap::Butt, ..Default::default() };
        let sliver = crate::strokes::stroke_area(&line, &blade, FillRule::NonZero);
        let lb = geom::path_bounds(&line);
        let candidates: Vec<u64> = if self.selection.is_empty() { self.doc.shapes().iter().map(|s| s.id).collect() } else { self.selection.clone() };
        let mut cut = 0;
        let mut new_sel = Vec::new();
        let mut checkpointed = false;
        for id in candidates {
            let Some(s) = self.doc.get(id).cloned() else { continue };
            if !s.visible || s.locked || s.fill.is_none() || s.kind.is_container() || matches!(s.kind, ShapeKind::Text(_)) {
                continue;
            }
            let (path, _) = crate::render::compute_outline(&s, &mut self.fonts);
            if let (Some(a), Some(b)) = (geom::path_bounds(&path), lb) {
                if a.intersect(b).area() <= 0.0 && !(a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1) {
                    continue;
                }
            }
            let polys = boolean::path_to_polys(&path, s.fill_rule);
            let rest = boolean::op(&polys, &sliver, BoolOp::Subtract);
            let pieces = boolean::split_pieces(&rest);
            if pieces.len() < 2 {
                continue;
            }
            if !checkpointed {
                self.checkpoint("");
                checkpointed = true;
            }
            let mut shapes = Vec::new();
            for (i, piece) in pieces.iter().enumerate() {
                let mut ns = s.clone();
                ns.id = 0;
                ns.kind = ShapeKind::Path(geom::polys_to_pathdata_smooth(piece));
                ns.transform = Affine::IDENTITY;
                ns.fill_rule = FillRule::NonZero;
                ns.name = format!("{} {}", s.name, i + 1);
                crate::render::keep_gradient(&s, &mut ns, &mut self.fonts);
                shapes.push(ns);
            }
            new_sel.extend(self.doc.replace_with(id, shapes));
            cut += 1;
        }
        if cut > 0 {
            self.selection = new_sel;
        }
        cut
    }
}

pub fn reverse_subpath(sp: &mut SubPath) {
    sp.nodes.reverse();
    for n in &mut sp.nodes {
        std::mem::swap(&mut n.hin, &mut n.hout);
    }
    // Closed: keep the same start node.
    if sp.closed && !sp.nodes.is_empty() {
        sp.nodes.rotate_right(1);
    }
}

/// Grow (or shrink) a filled outline by `d`, with round or miter joins.
pub fn offset_path(path: &BezPath, rule: FillRule, d: f64, round: bool) -> Option<PathData> {
    let base = boolean::path_to_polys(path, rule);
    let join = if round { LineJoin::Round } else { LineJoin::Miter };
    let st = Stroke { join, cap: LineCap::Round, miter_limit: 10.0, ..Default::default() };
    let band = boolean::path_to_polys(&crate::strokes::outline(path, &st, 2.0 * d.abs()), FillRule::NonZero);
    let out = boolean::op(&base, &band, if d > 0.0 { BoolOp::Union } else { BoolOp::Subtract });
    if boolean::area(&out) < 1e-6 {
        return None;
    }
    let pd = geom::polys_to_pathdata_smooth(&out);
    (!pd.subpaths.is_empty()).then_some(pd)
}
