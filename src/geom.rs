//! Geometry helpers: primitive generation, live corner rounding, conversions,
//! hit testing and flattening.

use std::f64::consts::{PI, TAU};

use kurbo::{
    Affine, BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, PathEl,
    PathSeg, Point, Rect, Shape as _, Vec2,
};

use crate::document::{Node, PathData, ShapeKind, SubPath};

const KAPPA: f64 = 0.552_284_749_830_793_4;

// ---------------------------------------------------------------------------
// Primitive → PathData
// ---------------------------------------------------------------------------

pub fn rect_nodes(w: f64, h: f64, radii: [f64; 4]) -> PathData {
    let pts = [Point::new(0.0, 0.0), Point::new(w, 0.0), Point::new(w, h), Point::new(0.0, h)];
    let nodes = pts
        .iter()
        .zip(radii)
        .map(|(p, r)| Node { corner: r.max(0.0), ..Node::corner(*p) })
        .collect();
    PathData { subpaths: vec![SubPath { nodes, closed: true }] }
}

pub fn ellipse_nodes(w: f64, h: f64) -> PathData {
    let (rx, ry) = (w / 2.0, h / 2.0);
    let c = Point::new(rx, ry);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let mk = |p: Point, hin: Vec2, hout: Vec2| Node {
        p,
        hin: Some(p + hin),
        hout: Some(p + hout),
        smooth: true,
        corner: 0.0,
    };
    let nodes = vec![
        mk(Point::new(c.x, 0.0), Vec2::new(-kx, 0.0), Vec2::new(kx, 0.0)),
        mk(Point::new(w, c.y), Vec2::new(0.0, -ky), Vec2::new(0.0, ky)),
        mk(Point::new(c.x, h), Vec2::new(kx, 0.0), Vec2::new(-kx, 0.0)),
        mk(Point::new(0.0, c.y), Vec2::new(0.0, ky), Vec2::new(0.0, -ky)),
    ];
    PathData { subpaths: vec![SubPath { nodes, closed: true }] }
}

fn radial_nodes(w: f64, h: f64, count: usize, radius_at: impl Fn(usize) -> f64, corner: f64) -> PathData {
    let (rx, ry) = (w / 2.0, h / 2.0);
    let nodes = (0..count)
        .map(|i| {
            let a = -PI / 2.0 + TAU * i as f64 / count as f64;
            let r = radius_at(i);
            Node { corner, ..Node::corner(Point::new(rx + rx * r * a.cos(), ry + ry * r * a.sin())) }
        })
        .collect();
    PathData { subpaths: vec![SubPath { nodes, closed: true }] }
}

pub fn star_nodes(w: f64, h: f64, points: u32, inner: f64, corner: f64) -> PathData {
    let n = points.max(3) as usize;
    radial_nodes(w, h, n * 2, |i| if i % 2 == 0 { 1.0 } else { inner }, corner)
}

pub fn polygon_nodes(w: f64, h: f64, sides: u32, corner: f64) -> PathData {
    radial_nodes(w, h, sides.max(3) as usize, |_| 1.0, corner)
}

/// Node representation of a shape in local coordinates (None for text).
pub fn kind_nodes(kind: &ShapeKind) -> Option<PathData> {
    Some(match kind {
        ShapeKind::Rect { w, h, radii } => rect_nodes(*w, *h, *radii),
        ShapeKind::Ellipse { w, h } => ellipse_nodes(*w, *h),
        ShapeKind::Star { w, h, points, inner, corner } => star_nodes(*w, *h, *points, *inner, *corner),
        ShapeKind::Polygon { w, h, sides, corner } => polygon_nodes(*w, *h, *sides, *corner),
        ShapeKind::Prim { w, h, prim } => bez_to_pathdata(&crate::prims::path(prim, *w, *h)),
        ShapeKind::Path(pd) => pd.clone(),
        ShapeKind::Text(_) | ShapeKind::Compound { .. } | ShapeKind::Group { .. } => return None,
    })
}

// ---------------------------------------------------------------------------
// PathData → BezPath with live corner rounding
// ---------------------------------------------------------------------------

fn segment(a: &Node, b: &Node) -> PathSeg {
    if a.hout.is_none() && b.hin.is_none() {
        PathSeg::Line(Line::new(a.p, b.p))
    } else {
        PathSeg::Cubic(CubicBez::new(a.p, a.hout.unwrap_or(a.p), b.hin.unwrap_or(b.p), b.p))
    }
}

pub fn subpath_segments(sp: &SubPath) -> Vec<PathSeg> {
    let n = sp.nodes.len();
    let mut segs = Vec::with_capacity(n);
    for i in 0..n.saturating_sub(1) {
        segs.push(segment(&sp.nodes[i], &sp.nodes[i + 1]));
    }
    if sp.closed && n > 1 {
        segs.push(segment(&sp.nodes[n - 1], &sp.nodes[0]));
    }
    segs
}

fn start_tangent(seg: &PathSeg) -> Vec2 {
    match seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.p1 - q.p0,
        PathSeg::Cubic(c) => {
            for v in [c.p1 - c.p0, c.p2 - c.p0, c.p3 - c.p0] {
                if v.hypot2() > 1e-12 {
                    return v;
                }
            }
            Vec2::ZERO
        }
    }
}

fn end_tangent(seg: &PathSeg) -> Vec2 {
    match seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.p2 - q.p1,
        PathSeg::Cubic(c) => {
            for v in [c.p3 - c.p2, c.p3 - c.p1, c.p3 - c.p0] {
                if v.hypot2() > 1e-12 {
                    return v;
                }
            }
            Vec2::ZERO
        }
    }
}

fn push_seg(path: &mut BezPath, seg: &PathSeg) {
    match seg {
        PathSeg::Line(l) => path.line_to(l.p1),
        PathSeg::Quad(q) => path.quad_to(q.p1, q.p2),
        PathSeg::Cubic(c) => path.curve_to(c.p1, c.p2, c.p3),
    }
}

/// Arc approximation data for a rounded corner at a node.
struct CornerCut {
    t: f64,
    /// Handle length factor relative to `t`.
    k: f64,
}

pub fn subpath_to_bez(sp: &SubPath, out: &mut BezPath) {
    let n = sp.nodes.len();
    if n == 0 {
        return;
    }
    let segs = subpath_segments(sp);
    if segs.is_empty() {
        out.move_to(sp.nodes[0].p);
        return;
    }
    let lens: Vec<f64> = segs.iter().map(|s| s.arclen(1e-3)).collect();
    let nseg = segs.len();

    // Compute cut distance at every node.
    let mut cuts: Vec<Option<CornerCut>> = (0..n).map(|_| None).collect();
    for i in 0..n {
        let node = &sp.nodes[i];
        if node.corner <= 1e-9 || node.smooth {
            continue;
        }
        let (si, so) = if sp.closed {
            ((i + nseg - 1) % nseg, i % nseg)
        } else {
            if i == 0 || i == n - 1 {
                continue;
            }
            (i - 1, i)
        };
        let d1 = end_tangent(&segs[si]);
        let d2 = start_tangent(&segs[so]);
        if d1.hypot2() < 1e-12 || d2.hypot2() < 1e-12 {
            continue;
        }
        let d1 = d1.normalize();
        let d2 = d2.normalize();
        let phi = d1.dot(d2).clamp(-1.0, 1.0).acos(); // turning angle
        if phi < 1e-3 || phi > PI - 1e-3 {
            continue;
        }
        let max_t = 0.5 * lens[si].min(lens[so]);
        let t = (node.corner * (phi / 2.0).tan()).min(max_t);
        let k = 4.0 / 3.0 * (phi / 4.0).tan() / (phi / 2.0).tan();
        cuts[i] = Some(CornerCut { t, k });
    }

    let trimmed = |si: usize| -> PathSeg {
        let a = si;
        let b = (si + 1) % n;
        let seg = segs[si];
        let len = lens[si];
        if len < 1e-9 {
            return seg;
        }
        let t0 = cuts[a].as_ref().map(|c| c.t).unwrap_or(0.0);
        let t1 = cuts[b].as_ref().map(|c| c.t).unwrap_or(0.0);
        if t0 == 0.0 && t1 == 0.0 {
            return seg;
        }
        let s0 = if t0 > 0.0 { seg.inv_arclen(t0, 1e-4) } else { 0.0 };
        let s1 = if t1 > 0.0 { seg.inv_arclen(len - t1, 1e-4) } else { 1.0 };
        seg.subsegment(s0..s1.max(s0))
    };

    let first = trimmed(0);
    out.move_to(first.start());
    let mut prev_end_dir;
    let mut cur = first;
    for si in 0..nseg {
        if si > 0 {
            cur = trimmed(si);
        }
        push_seg(out, &cur);
        prev_end_dir = end_tangent(&segs[si]);
        let j = (si + 1) % n;
        let is_last_open = !sp.closed && si == nseg - 1;
        if is_last_open {
            break;
        }
        if let Some(cut) = &cuts[j] {
            let next_si = (si + 1) % nseg;
            let next = if next_si == 0 { first } else { trimmed(next_si) };
            let p1 = cur.end();
            let p2 = next.start();
            let d1 = prev_end_dir.normalize();
            let d2 = start_tangent(&segs[next_si]).normalize();
            let h = cut.t * cut.k;
            out.curve_to(p1 + d1 * h, p2 - d2 * h, p2);
        }
    }
    if sp.closed {
        out.close_path();
    }
}

pub fn pathdata_to_bez(pd: &PathData) -> BezPath {
    let mut out = BezPath::new();
    for sp in &pd.subpaths {
        subpath_to_bez(sp, &mut out);
    }
    out
}

/// Local-space outline of a non-text shape.
pub fn local_path(kind: &ShapeKind) -> BezPath {
    match kind {
        ShapeKind::Rect { radii, w, h } if radii.iter().all(|r| *r <= 0.0) => {
            Rect::new(0.0, 0.0, *w, *h).to_path(0.1)
        }
        ShapeKind::Prim { w, h, prim } => crate::prims::path(prim, *w, *h),
        ShapeKind::Text(_) | ShapeKind::Compound { .. } | ShapeKind::Group { .. } => BezPath::new(),
        other => kind_nodes(other).map(|pd| pathdata_to_bez(&pd)).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// BezPath → PathData (for "convert to curves" and importers)
// ---------------------------------------------------------------------------

pub fn bez_to_pathdata(path: &BezPath) -> PathData {
    let mut pd = PathData::default();
    let mut cur: Option<SubPath> = None;
    let finish = |sp: SubPath, pd: &mut PathData| {
        if !sp.nodes.is_empty() {
            pd.subpaths.push(sp);
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if let Some(sp) = cur.take() {
                    finish(sp, &mut pd);
                }
                cur = Some(SubPath { nodes: vec![Node::corner(p)], closed: false });
            }
            PathEl::LineTo(p) => {
                let sp = cur.get_or_insert_with(|| SubPath { nodes: vec![Node::corner(p)], closed: false });
                sp.nodes.push(Node::corner(p));
            }
            PathEl::QuadTo(q, p) => {
                let sp = cur.get_or_insert_with(|| SubPath { nodes: vec![Node::corner(q)], closed: false });
                let p0 = sp.nodes.last().unwrap().p;
                let c1 = p0 + (q - p0) * (2.0 / 3.0);
                let c2 = p + (q - p) * (2.0 / 3.0);
                sp.nodes.last_mut().unwrap().hout = Some(c1);
                sp.nodes.push(Node { hin: Some(c2), ..Node::corner(p) });
            }
            PathEl::CurveTo(c1, c2, p) => {
                let sp = cur.get_or_insert_with(|| SubPath { nodes: vec![Node::corner(c1)], closed: false });
                sp.nodes.last_mut().unwrap().hout = Some(c1);
                sp.nodes.push(Node { hin: Some(c2), ..Node::corner(p) });
            }
            PathEl::ClosePath => {
                if let Some(mut sp) = cur.take() {
                    sp.closed = true;
                    if sp.nodes.len() > 1 {
                        let first = sp.nodes[0].p;
                        let last = sp.nodes.last().unwrap().p;
                        if (first - last).hypot() < 1e-6 {
                            let last = sp.nodes.pop().unwrap();
                            sp.nodes[0].hin = last.hin;
                        }
                    }
                    let start = sp.nodes.first().map(|n| n.p);
                    finish(sp, &mut pd);
                    // A following segment without MoveTo starts at the same point.
                    let _ = start;
                }
            }
        }
    }
    if let Some(sp) = cur.take() {
        finish(sp, &mut pd);
    }
    // Detect smooth nodes (collinear handles).
    for sp in &mut pd.subpaths {
        for n in &mut sp.nodes {
            if let (Some(a), Some(b)) = (n.hin, n.hout) {
                let v1 = n.p - a;
                let v2 = b - n.p;
                if v1.hypot() > 1e-6 && v2.hypot() > 1e-6 {
                    let c = v1.normalize().dot(v2.normalize());
                    n.smooth = c > 0.999;
                }
            }
        }
    }
    pd
}

// ---------------------------------------------------------------------------
// Hit testing & utilities
// ---------------------------------------------------------------------------

pub fn distance_to_path(path: &BezPath, p: Point) -> f64 {
    let mut best = f64::INFINITY;
    for seg in path.segments() {
        let n = seg.nearest(p, 1e-3);
        best = best.min(n.distance_sq);
    }
    best.sqrt()
}

pub fn path_bounds(path: &BezPath) -> Option<Rect> {
    if path.elements().is_empty() {
        None
    } else {
        Some(path.bounding_box())
    }
}

pub fn union_rect(a: Option<Rect>, b: Rect) -> Option<Rect> {
    Some(match a {
        Some(a) => a.union(b),
        None => b,
    })
}

/// Flatten a path to closed polygons (for boolean ops). Open subpaths are closed.
pub fn flatten_polys(path: &BezPath, tolerance: f64) -> Vec<Vec<[f64; 2]>> {
    let mut polys: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut cur: Vec<[f64; 2]> = Vec::new();
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) => {
            if cur.len() > 2 {
                polys.push(std::mem::take(&mut cur));
            }
            cur.clear();
            cur.push([p.x, p.y]);
        }
        PathEl::LineTo(p) => cur.push([p.x, p.y]),
        PathEl::ClosePath => {
            if cur.len() > 2 {
                polys.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
        _ => {}
    });
    if cur.len() > 2 {
        polys.push(cur);
    }
    // Drop duplicate closing points.
    for p in &mut polys {
        if p.len() > 1 && p[0] == *p.last().unwrap() {
            p.pop();
        }
    }
    polys
}

pub fn polys_to_bez(polys: &[Vec<[f64; 2]>]) -> BezPath {
    let mut b = BezPath::new();
    for poly in polys {
        if poly.len() < 3 {
            continue;
        }
        b.move_to((poly[0][0], poly[0][1]));
        for p in &poly[1..] {
            b.line_to((p[0], p[1]));
        }
        b.close_path();
    }
    b
}

/// Signed area of a polygon.
pub fn poly_area(poly: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// Even-odd point-in-polygon test over several contours.
pub fn point_in_polys(polys: &[Vec<[f64; 2]>], p: Point) -> bool {
    let mut inside = false;
    for poly in polys {
        let n = poly.len();
        let mut j = n.wrapping_sub(1);
        for i in 0..n {
            let (xi, yi) = (poly[i][0], poly[i][1]);
            let (xj, yj) = (poly[j][0], poly[j][1]);
            if (yi > p.y) != (yj > p.y) && p.x < (xj - xi) * (p.y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

/// Fit a set of flattened polygons back into smooth-ish nodes: we keep them as
/// straight segments (accurate, editable). Collinear points are merged.
pub fn polys_to_pathdata(polys: &[Vec<[f64; 2]>]) -> PathData {
    let mut pd = PathData::default();
    for poly in polys {
        let pts: Vec<Point> = poly.iter().map(|p| Point::new(p[0], p[1])).collect();
        let n = pts.len();
        if n < 3 {
            continue;
        }
        let mut nodes = Vec::new();
        for i in 0..n {
            let a = pts[(i + n - 1) % n];
            let b = pts[i];
            let c = pts[(i + 1) % n];
            let cross = (b - a).cross(c - b).abs();
            let len = (b - a).hypot() * (c - b).hypot();
            if len > 0.0 && cross / len < 1e-4 && (b - a).dot(c - b) > 0.0 {
                continue; // collinear
            }
            nodes.push(Node::corner(b));
        }
        if nodes.len() >= 3 {
            pd.subpaths.push(SubPath { nodes, closed: true });
        }
    }
    pd
}

/// Split cubic/line segment `seg` of a subpath at parameter t, inserting a node.
pub fn insert_node(sp: &mut SubPath, seg_index: usize, t: f64) {
    let n = sp.nodes.len();
    let a = seg_index;
    let b = (seg_index + 1) % n;
    let seg = segment(&sp.nodes[a], &sp.nodes[b]);
    match seg {
        PathSeg::Cubic(c) => {
            let (l, r) = c.subdivide_at(t);
            sp.nodes[a].hout = Some(l.p1);
            sp.nodes[b].hin = Some(r.p2);
            let node = Node { p: l.p3, hin: Some(l.p2), hout: Some(r.p1), smooth: true, corner: 0.0 };
            sp.nodes.insert(a + 1, node);
        }
        _ => {
            let p = seg.eval(t);
            sp.nodes.insert(a + 1, Node::corner(p));
        }
    }
}

trait Subdivide {
    fn subdivide_at(&self, t: f64) -> (CubicBez, CubicBez);
}

impl Subdivide for CubicBez {
    fn subdivide_at(&self, t: f64) -> (CubicBez, CubicBez) {
        (self.subsegment(0.0..t), self.subsegment(t..1.0))
    }
}

/// Bend a segment so that its point at parameter `t` moves by `delta`
/// (cubic "drag the curve" behavior). Lines become curves.
pub fn bend_segment(sp: &mut SubPath, seg_index: usize, t: f64, delta: Vec2) {
    let n = sp.nodes.len();
    let a = seg_index;
    let b = (seg_index + 1) % n;
    let pa = sp.nodes[a].p;
    let pb = sp.nodes[b].p;
    let c1 = sp.nodes[a].hout.unwrap_or(pa + (pb - pa) / 3.0);
    let c2 = sp.nodes[b].hin.unwrap_or(pb + (pa - pb) / 3.0);
    let t = t.clamp(0.02, 0.98);
    let w = if t <= 1.0 / 6.0 {
        0.0
    } else if t <= 0.5 {
        (6.0 * t - 1.0).powi(3) / 16.0
    } else if t <= 5.0 / 6.0 {
        1.0 - (6.0 * (1.0 - t) - 1.0).powi(3) / 16.0
    } else {
        1.0
    };
    let mt = 1.0 - t;
    let off1 = delta * ((1.0 - w) / (3.0 * mt * mt * t));
    let off2 = delta * (w / (3.0 * mt * t * t));
    sp.nodes[a].hout = Some(c1 + off1);
    sp.nodes[b].hin = Some(c2 + off2);
    // Keep smooth neighbors smooth.
    for (idx, is_out) in [(a, true), (b, false)] {
        let node = &mut sp.nodes[idx];
        if node.smooth {
            mirror_handle(node, is_out);
        }
    }
}

/// After `hout` (if `moved_out`) or `hin` changed, rotate the opposite handle to stay collinear.
pub fn mirror_handle(node: &mut Node, moved_out: bool) {
    let (moved, other) = if moved_out { (node.hout, node.hin) } else { (node.hin, node.hout) };
    if let (Some(m), Some(o)) = (moved, other) {
        let dir = node.p - m;
        if dir.hypot() < 1e-9 {
            return;
        }
        let len = (o - node.p).hypot();
        let newo = node.p + dir.normalize() * len;
        if moved_out {
            node.hin = Some(newo);
        } else {
            node.hout = Some(newo);
        }
    }
}

/// Decompose an affine into (translation, rotation radians, scale x, scale y).
pub fn decompose(a: Affine) -> (Vec2, f64, f64, f64) {
    let [m0, m1, m2, m3, tx, ty] = a.as_coeffs();
    let sx = (m0 * m0 + m1 * m1).sqrt();
    let rot = m1.atan2(m0);
    let det = m0 * m3 - m1 * m2;
    let sy = if sx > 1e-12 { det / sx } else { (m2 * m2 + m3 * m3).sqrt() };
    (Vec2::new(tx, ty), rot, sx, sy)
}

/// Nearest point on any segment of the subpaths; returns (subpath, segment, t, distance).
pub fn nearest_segment(pd: &PathData, p: Point) -> Option<(usize, usize, f64, f64)> {
    let mut best: Option<(usize, usize, f64, f64)> = None;
    for (si, sp) in pd.subpaths.iter().enumerate() {
        for (gi, seg) in subpath_segments(sp).iter().enumerate() {
            let n = seg.nearest(p, 1e-4);
            let d = n.distance_sq.sqrt();
            if best.map(|b| d < b.3).unwrap_or(true) {
                best = Some((si, gi, n.t, d));
            }
        }
    }
    best
}

pub fn segment_at(sp: &SubPath, index: usize) -> PathSeg {
    let n = sp.nodes.len();
    segment(&sp.nodes[index], &sp.nodes[(index + 1) % n])
}

/// Convert boolean-op polygons back into smooth editable Bézier nodes by
/// fitting cubic curves through the flattened outline (Schneider's algorithm).
/// Sharp corners are detected and preserved; straight runs stay lines.
pub fn polys_to_pathdata_smooth(polys: &[Vec<[f64; 2]>]) -> PathData {
    let mut out = BezPath::new();
    for poly in polys {
        fit_contour(poly, &mut out, FIT_TOL);
    }
    let pd = bez_to_pathdata(&out);
    if pd.subpaths.is_empty() { polys_to_pathdata(polys) } else { pd }
}

const FIT_TOL: f64 = 0.15;

/// Refit a subpath with fewer nodes: flatten, then fit cubics within `tol`
/// (corners sharper than ~35° are kept).
pub fn simplify_subpath(sp: &SubPath, tol: f64) -> SubPath {
    let bez = pathdata_to_bez(&PathData { subpaths: vec![sp.clone()] });
    let mut pts: Vec<[f64; 2]> = Vec::new();
    kurbo::flatten(&bez, (tol * 0.2).max(1e-3), |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => pts.push([p.x, p.y]),
        _ => {}
    });
    let mut out = BezPath::new();
    if sp.closed {
        fit_contour(&pts, &mut out, tol);
    } else {
        let p: Vec<Point> = pts.iter().map(|q| Point::new(q[0], q[1])).collect();
        let mut dedup: Vec<Point> = Vec::with_capacity(p.len());
        for q in p {
            if dedup.last().map(|l: &Point| (*l - q).hypot() > 1e-6).unwrap_or(true) {
                dedup.push(q);
            }
        }
        if dedup.len() < 3 {
            return sp.clone();
        }
        let n = dedup.len();
        out.move_to(dedup[0]);
        let mut run_start = 0;
        for k in 1..n {
            let corner = k == n - 1 || {
                let (d1, d2) = (dedup[k] - dedup[k - 1], dedup[k + 1] - dedup[k]);
                let l = d1.hypot() * d2.hypot();
                l < 1e-12 || d1.dot(d2) / l < CORNER_COS
            };
            if corner {
                let run = &dedup[run_start..=k];
                let t1 = (run[1] - run[0]).normalize();
                let t2 = (run[run.len() - 2] - run[run.len() - 1]).normalize();
                fit_run(run, t1, t2, &mut out, tol);
                run_start = k;
            }
        }
    }
    let pd = bez_to_pathdata(&out);
    match pd.subpaths.into_iter().next() {
        Some(mut s) if s.nodes.len() >= 2 => {
            s.closed = sp.closed;
            s
        }
        _ => sp.clone(),
    }
}
const CORNER_COS: f64 = 0.82; // ~35 degrees

fn fit_contour(poly: &[[f64; 2]], out: &mut BezPath, tol: f64) {
    // Deduplicate points.
    let mut pts: Vec<Point> = Vec::with_capacity(poly.len());
    for q in poly {
        let p = Point::new(q[0], q[1]);
        if pts.last().map(|l: &Point| (*l - p).hypot() > 1e-6).unwrap_or(true) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && (pts[0] - *pts.last().unwrap()).hypot() < 1e-6 {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return;
    }
    // Corner detection.
    let is_corner: Vec<bool> = (0..n)
        .map(|i| {
            let a = pts[(i + n - 1) % n];
            let b = pts[i];
            let c = pts[(i + 1) % n];
            let d1 = b - a;
            let d2 = c - b;
            let l = d1.hypot() * d2.hypot();
            l < 1e-12 || d1.dot(d2) / l < CORNER_COS
        })
        .collect();
    let corners: Vec<usize> = (0..n).filter(|i| is_corner[*i]).collect();
    if corners.is_empty() {
        // Smooth closed loop: split into two halves with continuous tangents.
        let mut loop_pts = pts.clone();
        loop_pts.push(pts[0]);
        let t0 = (pts[1] - pts[n - 1]).normalize();
        let half = n / 2;
        let th = (pts[(half + 1) % n] - pts[half - 1]).normalize();
        out.move_to(pts[0]);
        fit_run(&loop_pts[..=half], t0, -th, out, tol);
        fit_run(&loop_pts[half..], th, -t0, out, tol);
        out.close_path();
        return;
    }
    // Rotate so that we start at a corner.
    let start = corners[0];
    let rot: Vec<Point> = (0..=n).map(|k| pts[(start + k) % n]).collect();
    let corner_rot: Vec<bool> = (0..=n).map(|k| is_corner[(start + k) % n]).collect();
    out.move_to(rot[0]);
    let mut run_start = 0;
    for k in 1..=n {
        if corner_rot[k] || k == n {
            let run = &rot[run_start..=k];
            let t1 = (run[1] - run[0]).normalize();
            let t2 = (run[run.len() - 2] - run[run.len() - 1]).normalize();
            fit_run(run, t1, t2, out, tol);
            run_start = k;
        }
    }
    out.close_path();
}

/// Fit a run of points (endpoints fixed) appending segments to `out`.
fn fit_run(pts: &[Point], t1: Vec2, t2: Vec2, out: &mut BezPath, tol: f64) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    // Straight run?
    let a = pts[0];
    let b = pts[n - 1];
    let chord = b - a;
    let len = chord.hypot();
    if n == 2 || (len > 1e-9 && pts.iter().all(|p| ((*p - a).cross(chord) / len).abs() < tol)) {
        out.line_to(b);
        return;
    }
    fit_cubic(pts, t1, t2, tol, out, 0);
}

fn chord_params(pts: &[Point]) -> Vec<f64> {
    let mut u = vec![0.0; pts.len()];
    for i in 1..pts.len() {
        u[i] = u[i - 1] + (pts[i] - pts[i - 1]).hypot();
    }
    let total = *u.last().unwrap();
    if total > 0.0 {
        for v in &mut u {
            *v /= total;
        }
    }
    u
}

fn bez_pt(c: &CubicBez, t: f64) -> Point {
    c.eval(t)
}

fn generate_bezier(pts: &[Point], u: &[f64], t1: Vec2, t2: Vec2) -> CubicBez {
    let first = pts[0];
    let last = pts[pts.len() - 1];
    let mut c = [[0.0f64; 2]; 2];
    let mut x = [0.0f64; 2];
    for (i, &t) in u.iter().enumerate() {
        let mt = 1.0 - t;
        let b0 = mt * mt * mt;
        let b1 = 3.0 * t * mt * mt;
        let b2 = 3.0 * t * t * mt;
        let b3 = t * t * t;
        let a1 = t1 * b1;
        let a2 = t2 * b2;
        c[0][0] += a1.dot(a1);
        c[0][1] += a1.dot(a2);
        c[1][1] += a2.dot(a2);
        let tmp = pts[i].to_vec2() - (first.to_vec2() * (b0 + b1) + last.to_vec2() * (b2 + b3));
        x[0] += a1.dot(tmp);
        x[1] += a2.dot(tmp);
    }
    c[1][0] = c[0][1];
    let det = c[0][0] * c[1][1] - c[1][0] * c[0][1];
    let (mut al, mut ar) = if det.abs() > 1e-12 {
        ((x[0] * c[1][1] - c[0][1] * x[1]) / det, (c[0][0] * x[1] - c[1][0] * x[0]) / det)
    } else {
        (0.0, 0.0)
    };
    let seg = (last - first).hypot();
    let eps = 1e-6 * seg;
    if al < eps || ar < eps {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    CubicBez::new(first, first + t1 * al, last + t2 * ar, last)
}

fn newton_reparam(c: &CubicBez, p: Point, u: f64) -> f64 {
    let q = c.eval(u);
    let d1 = c.deriv().eval(u).to_vec2();
    let d2 = c.deriv().deriv().eval(u).to_vec2();
    let diff = q - p;
    let num = diff.dot(d1);
    let den = d1.dot(d1) + diff.dot(d2);
    if den.abs() < 1e-12 { u } else { (u - num / den).clamp(0.0, 1.0) }
}

fn max_error(pts: &[Point], c: &CubicBez, u: &[f64]) -> (f64, usize) {
    let mut max = 0.0;
    let mut split = pts.len() / 2;
    for i in 1..pts.len() - 1 {
        let d = (bez_pt(c, u[i]) - pts[i]).hypot2();
        if d >= max {
            max = d;
            split = i;
        }
    }
    (max.sqrt(), split)
}

fn fit_cubic(pts: &[Point], t1: Vec2, t2: Vec2, tol: f64, out: &mut BezPath, depth: u32) {
    let n = pts.len();
    if n == 2 || depth > 12 {
        let d = (pts[n - 1] - pts[0]).hypot() / 3.0;
        out.curve_to(pts[0] + t1 * d, pts[n - 1] + t2 * d, pts[n - 1]);
        return;
    }
    let mut u = chord_params(pts);
    let mut c = generate_bezier(pts, &u, t1, t2);
    let (mut err, mut split) = max_error(pts, &c, &u);
    if err < tol {
        out.curve_to(c.p1, c.p2, c.p3);
        return;
    }
    if err < tol * 6.0 {
        for _ in 0..6 {
            u = u.iter().enumerate().map(|(i, &t)| newton_reparam(&c, pts[i], t)).collect();
            c = generate_bezier(pts, &u, t1, t2);
            let (e, s) = max_error(pts, &c, &u);
            err = e;
            split = s;
            if err < tol {
                out.curve_to(c.p1, c.p2, c.p3);
                return;
            }
        }
    }
    let split = split.clamp(1, n - 2);
    let mut tc = pts[split - 1] - pts[split + 1];
    if tc.hypot() < 1e-12 {
        tc = pts[split - 1] - pts[split];
    }
    let tc = tc.normalize();
    fit_cubic(&pts[..=split], t1, tc, tol, out, depth + 1);
    fit_cubic(&pts[split..], -tc, t2, tol, out, depth + 1);
}

/// Smooth a freehand polyline (knife strokes): `strength` 0 keeps it as
/// drawn, 1 rounds it off heavily. Laplacian smoothing (ends pinned), then
/// corner cutting so the result has no visible kinks.
pub fn smooth_polyline(pts: &[Point], strength: f32) -> Vec<Point> {
    if strength <= 0.0 || pts.len() < 3 {
        return pts.to_vec();
    }
    let mut cur = pts.to_vec();
    let iters = (strength * 24.0).round() as usize;
    for _ in 0..iters {
        let prev = cur.clone();
        for i in 1..cur.len() - 1 {
            let avg = prev[i - 1].midpoint(prev[i + 1]);
            cur[i] = prev[i].lerp(avg, 0.5);
        }
    }
    // Chaikin corner cutting, two rounds, keeping both ends.
    for _ in 0..2 {
        let mut out = Vec::with_capacity(cur.len() * 2);
        out.push(cur[0]);
        for w in cur.windows(2) {
            out.push(w[0].lerp(w[1], 0.25));
            out.push(w[0].lerp(w[1], 0.75));
        }
        out.push(*cur.last().unwrap());
        cur = out;
    }
    cur
}

/// Whether any part of a path's outline lies inside or crosses `r`.
pub fn path_touches_rect(path: &BezPath, r: Rect, tol: f64) -> bool {
    let seg_hits = |a: Point, b: Point| {
        if r.contains(a) || r.contains(b) {
            return true;
        }
        // Liang–Barsky clip of the segment against the rect.
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (p, q) in [(-dx, a.x - r.x0), (dx, r.x1 - a.x), (-dy, a.y - r.y0), (dy, r.y1 - a.y)] {
            if p.abs() < 1e-12 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
                if t0 > t1 {
                    return false;
                }
            }
        }
        true
    };
    let mut hit = false;
    let (mut start, mut last) = (Point::ZERO, Point::ZERO);
    kurbo::flatten(path, tol.max(1e-3), |el| {
        if hit {
            return;
        }
        match el {
            PathEl::MoveTo(p) => {
                start = p;
                last = p;
                hit = r.contains(p);
            }
            PathEl::LineTo(p) => {
                hit = seg_hits(last, p);
                last = p;
            }
            PathEl::ClosePath => {
                hit = seg_hits(last, start);
                last = start;
            }
            _ => {}
        }
    });
    hit
}

