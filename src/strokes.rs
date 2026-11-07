//! Stroke geometry beyond what a plain path stroke can express: dashes,
//! inside/outside alignment and arrowheads. Rendering, export and Expand
//! Stroke all go through [`stroke_parts`].

use kurbo::{BezPath, ParamCurve, ParamCurveDeriv, PathEl, PathSeg, Point, Shape as _, Vec2};

use crate::boolean::{self, BoolOp};
use crate::document::{Arrowhead, FillRule, LineCap, LineJoin, Stroke, StrokeAlign};
use crate::geom;

/// One piece of a stroke, painted in the stroke color.
#[derive(Clone, Debug)]
pub enum StrokePart {
    /// Stroke this path with the stroke's width/join/cap (dashes already applied).
    Line(BezPath),
    /// Fill this outline with the given rule.
    Fill(BezPath, FillRule),
}

/// A stroke that SVG/PDF can express natively (dashes aside).
pub fn is_plain(st: &Stroke, path: &BezPath) -> bool {
    !st.has_arrows() && st.profile.is_empty() && (st.align == StrokeAlign::Center || !all_closed(path))
}

/// Outline of a variable-width stroke (centred; `st.profile` gives the
/// width along each subpath). Dashes don't apply to profiled strokes.
pub fn variable_outline(path: &BezPath, st: &Stroke) -> BezPath {
    let mut out = BezPath::new();
    let tol = (st.width * 0.02).clamp(0.02, 0.5);
    // Flatten into subpaths of points.
    let mut subs: Vec<(Vec<Point>, bool)> = Vec::new();
    kurbo::flatten(path.iter(), tol, |el| match el {
        PathEl::MoveTo(p) => subs.push((vec![p], false)),
        PathEl::LineTo(p) => {
            if let Some(s) = subs.last_mut() {
                if s.0.last().is_none_or(|q| (*q - p).hypot() > 1e-9) {
                    s.0.push(p);
                }
            }
        }
        PathEl::ClosePath => {
            if let Some(s) = subs.last_mut() {
                s.1 = true;
            }
        }
        _ => {}
    });
    for (mut pts, closed) in subs {
        if closed && pts.len() > 2 && (pts[0] - *pts.last().unwrap()).hypot() < 1e-9 {
            pts.pop();
        }
        if pts.len() < 2 {
            continue;
        }
        // Straight runs flatten to their two ends only: subdivide them so
        // the profile is sampled along their length, not just at the tips.
        if st.profile.len() > 1 {
            let mut total = pts.windows(2).map(|w| (w[1] - w[0]).hypot()).sum::<f64>();
            if closed {
                total += (pts[0] - pts[pts.len() - 1]).hypot();
            }
            let step = (total / 128.0).max(st.width * 0.1).max(tol);
            let mut fine = Vec::with_capacity(pts.len());
            for i in 0..pts.len() {
                fine.push(pts[i]);
                let next = if i + 1 < pts.len() { Some(pts[i + 1]) } else if closed { Some(pts[0]) } else { None };
                if let Some(q) = next {
                    let k = ((q - pts[i]).hypot() / step).floor() as usize;
                    for j in 1..k {
                        fine.push(pts[i].lerp(q, j as f64 / k as f64));
                    }
                }
            }
            pts = fine;
        }
        let n = pts.len();
        let mut acc = vec![0.0; n];
        for i in 1..n {
            acc[i] = acc[i - 1] + (pts[i] - pts[i - 1]).hypot();
        }
        let total = acc[n - 1] + if closed { (pts[0] - pts[n - 1]).hypot() } else { 0.0 };
        if total <= 1e-9 {
            continue;
        }
        let half = |i: usize| crate::document::profile_at(&st.profile, acc[i] / total).max(0.0) * st.width / 2.0;
        let seg_dir = |i: usize, j: usize| {
            let d = pts[j] - pts[i];
            let l = d.hypot();
            if l > 1e-12 { d / l } else { Vec2::new(1.0, 0.0) }
        };
        // Vertex normals (mitred, limited).
        let mut normals = Vec::with_capacity(n);
        for i in 0..n {
            let prev = if i > 0 { Some(seg_dir(i - 1, i)) } else if closed { Some(seg_dir(n - 1, 0)) } else { None };
            let next = if i + 1 < n { Some(seg_dir(i, i + 1)) } else if closed { Some(seg_dir(n - 1, 0)) } else { None };
            let (a, b) = (prev.or(next).unwrap(), next.or(prev).unwrap());
            let na = Vec2::new(-a.y, a.x);
            let nb = Vec2::new(-b.y, b.x);
            let m = na + nb;
            let ml = m.hypot();
            let nrm = if ml < 1e-6 { nb } else { m / ml };
            let cos = nrm.dot(nb).max(0.5);
            normals.push(nrm / cos);
        }
        let left: Vec<Point> = (0..n).map(|i| pts[i] + normals[i] * half(i)).collect();
        let right: Vec<Point> = (0..n).map(|i| pts[i] - normals[i] * half(i)).collect();
        if closed {
            out.move_to(left[0]);
            for p in &left[1..] {
                out.line_to(*p);
            }
            out.close_path();
            out.move_to(right[n - 1]);
            for p in right[..n - 1].iter().rev() {
                out.line_to(*p);
            }
            out.close_path();
            continue;
        }
        let cap = |out: &mut BezPath, c: Point, from: Point, to: Point, dir: Vec2| {
            let r = (from - c).hypot();
            match st.cap {
                LineCap::Round if r > 1e-9 => {
                    let a0 = (from - c).atan2();
                    let steps = 10;
                    for k in 1..steps {
                        let a = a0 - std::f64::consts::PI * k as f64 / steps as f64;
                        out.line_to(c + Vec2::from_angle(a) * r);
                    }
                    out.line_to(to);
                }
                LineCap::Square if r > 1e-9 => {
                    out.line_to(from + dir * r);
                    out.line_to(to + dir * r);
                    out.line_to(to);
                }
                _ => out.line_to(to),
            }
        };
        out.move_to(left[0]);
        for p in &left[1..] {
            out.line_to(*p);
        }
        cap(&mut out, pts[n - 1], left[n - 1], right[n - 1], seg_dir(n - 2, n - 1));
        for p in right[..n - 1].iter().rev() {
            out.line_to(*p);
        }
        cap(&mut out, pts[0], right[0], left[0], -seg_dir(0, 1));
        out.close_path();
    }
    out
}

fn all_closed(path: &BezPath) -> bool {
    let mut open = false;
    let mut any = false;
    let mut closed = true;
    for el in path.elements() {
        match el {
            PathEl::MoveTo(_) => {
                if open {
                    closed = false;
                }
                open = true;
                any = true;
            }
            PathEl::ClosePath => open = false,
            _ => {}
        }
    }
    any && closed && !open
}

pub fn dashed(path: &BezPath, st: &Stroke) -> BezPath {
    if !st.is_dashed() || st.width <= 0.0 {
        return path.clone();
    }
    let d = st.dash_px();
    BezPath::from_vec(kurbo::dash(path.elements().iter().copied(), st.dash_offset * st.width, &d).collect())
}

/// Outline of a stroke of the given width as a fillable path.
pub fn outline(path: &BezPath, st: &Stroke, width: f64) -> BezPath {
    let join = match st.join {
        LineJoin::Miter => kurbo::Join::Miter,
        LineJoin::Round => kurbo::Join::Round,
        LineJoin::Bevel => kurbo::Join::Bevel,
    };
    let cap = match st.cap {
        LineCap::Butt => kurbo::Cap::Butt,
        LineCap::Round => kurbo::Cap::Round,
        LineCap::Square => kurbo::Cap::Square,
    };
    let style = kurbo::Stroke::new(width).with_join(join).with_caps(cap).with_miter_limit(st.miter_limit.max(1.0));
    kurbo::stroke(path.iter(), &style, &kurbo::StrokeOpts::default(), 0.05)
}

pub fn stroke_parts(path: &BezPath, st: &Stroke, fill_rule: FillRule) -> Vec<StrokePart> {
    let mut parts = Vec::new();
    if st.width <= 0.0 {
        return parts;
    }
    let line = dashed(path, st);
    if !st.profile.is_empty() {
        parts.push(StrokePart::Fill(variable_outline(path, st), FillRule::NonZero));
    } else if st.align == StrokeAlign::Center || !all_closed(path) {
        parts.push(StrokePart::Line(line));
    } else {
        // Double-width stroke clipped to the inside or outside of the shape.
        let wide = boolean::path_to_polys(&outline(&line, st, st.width * 2.0), FillRule::NonZero);
        let area = boolean::path_to_polys(path, fill_rule);
        let op = if st.align == StrokeAlign::Inside { BoolOp::Intersect } else { BoolOp::Subtract };
        let res = boolean::op(&wide, &area, op);
        if !res.is_empty() {
            parts.push(StrokePart::Fill(geom::polys_to_bez(&res), FillRule::EvenOdd));
        }
    }
    if st.has_arrows() {
        let mut heads = BezPath::new();
        for (start, end) in open_ends(path) {
            if let Some((p, dir)) = start {
                arrowhead(&mut heads, st.start_arrow, p, dir, st);
            }
            if let Some((p, dir)) = end {
                arrowhead(&mut heads, st.end_arrow, p, dir, st);
            }
        }
        if !heads.is_empty() {
            parts.push(StrokePart::Fill(heads, FillRule::NonZero));
        }
    }
    parts
}

/// Union of all stroke parts as clean polygons (for Expand Stroke).
pub fn stroke_area(path: &BezPath, st: &Stroke, fill_rule: FillRule) -> boolean::Polys {
    let mut out: boolean::Polys = Vec::new();
    for part in stroke_parts(path, st, fill_rule) {
        let (p, rule) = match part {
            StrokePart::Line(l) => (outline(&l, st, st.width), FillRule::NonZero),
            StrokePart::Fill(f, r) => (f, r),
        };
        let polys = boolean::path_to_polys(&p, rule);
        out = boolean::op(&out, &polys, BoolOp::Union);
    }
    out
}

type End = Option<(Point, Vec2)>;

/// End points of open subpaths with their outward directions.
fn open_ends(path: &BezPath) -> Vec<(End, End)> {
    let mut out = Vec::new();
    let mut segs: Vec<PathSeg> = Vec::new();
    let flush = |segs: &mut Vec<PathSeg>, closed: bool, out: &mut Vec<(End, End)>| {
        if !closed && !segs.is_empty() {
            let first = segs.iter().find_map(|s| tangent(s, 0.0).map(|d| (s.start(), -d)));
            let last = segs.iter().rev().find_map(|s| tangent(s, 1.0).map(|d| (s.end(), d)));
            out.push((first, last));
        }
        segs.clear();
    };
    let mut last = Point::ZERO;
    let mut start = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush(&mut segs, false, &mut out);
                last = p;
                start = p;
            }
            PathEl::LineTo(p) => {
                segs.push(PathSeg::Line(kurbo::Line::new(last, p)));
                last = p;
            }
            PathEl::QuadTo(a, p) => {
                segs.push(PathSeg::Quad(kurbo::QuadBez::new(last, a, p)));
                last = p;
            }
            PathEl::CurveTo(a, b, p) => {
                segs.push(PathSeg::Cubic(kurbo::CubicBez::new(last, a, b, p)));
                last = p;
            }
            PathEl::ClosePath => {
                flush(&mut segs, true, &mut out);
                last = start;
            }
        }
    }
    flush(&mut segs, false, &mut out);
    out
}

fn tangent(s: &PathSeg, t: f64) -> Option<Vec2> {
    let d = match s {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.deriv().eval(t).to_vec2(),
        PathSeg::Cubic(c) => {
            let d = c.deriv().eval(t).to_vec2();
            if d.hypot() > 1e-9 {
                d
            } else if t < 0.5 {
                // Coincident handle: use a short chord instead.
                c.eval(0.05) - c.eval(0.0)
            } else {
                c.eval(1.0) - c.eval(0.95)
            }
        }
    };
    (d.hypot() > 1e-9).then(|| d.normalize())
}

fn arrowhead(out: &mut BezPath, kind: Arrowhead, p: Point, dir: Vec2, st: &Stroke) {
    let w = st.width.max(0.5) * st.arrow_scale.max(0.1);
    let n = Vec2::new(-dir.y, dir.x);
    match kind {
        Arrowhead::None => {}
        Arrowhead::Triangle => {
            let tip = p + dir * (w * 3.0);
            let back = p - dir * (w * 0.5);
            out.move_to(tip);
            out.line_to(back + n * (w * 2.0));
            out.line_to(back - n * (w * 2.0));
            out.close_path();
        }
        Arrowhead::Arrow => {
            let tip = p + dir * (w * 0.5);
            let mut chevron = BezPath::new();
            chevron.move_to(tip - dir * (w * 3.0) + n * (w * 2.5));
            chevron.line_to(tip);
            chevron.line_to(tip - dir * (w * 3.0) - n * (w * 2.5));
            let mut s = st.clone();
            s.join = LineJoin::Miter;
            s.cap = LineCap::Round;
            s.miter_limit = 10.0;
            out.extend(outline(&chevron, &s, st.width.max(0.5)));
        }
        Arrowhead::Circle => {
            out.extend(kurbo::Circle::new(p, w * 1.75).path_elements(0.05));
        }
        Arrowhead::Square => {
            let h = w * 1.5;
            out.move_to(p + dir * h + n * h);
            out.line_to(p + dir * h - n * h);
            out.line_to(p - dir * h - n * h);
            out.line_to(p - dir * h + n * h);
            out.close_path();
        }
        Arrowhead::Bar => {
            let t = st.width.max(0.5) / 2.0;
            let h = w * 2.5;
            out.move_to(p + dir * t + n * h);
            out.line_to(p + dir * t - n * h);
            out.line_to(p - dir * t - n * h);
            out.line_to(p - dir * t + n * h);
            out.close_path();
        }
    }
}
