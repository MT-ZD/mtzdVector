//! Drawing operations on the selection: opacity masks, patterns from
//! artwork, blends between shapes, envelope warps and image tracing.

use kurbo::{Affine, BezPath, CubicBez, ParamCurve, PathEl, Point, Rect, Vec2};

use crate::app::App;
use crate::document::{FillRule, Mask, MaskMode, Paint, PathData, Rgba, Shape, ShapeKind, Stroke};
use crate::fonts::FontManager;
use crate::geom;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Dialog state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarpStyle {
    Arch,
    ArcUpper,
    ArcLower,
    Bulge,
    Squeeze,
    Flag,
    Wave,
    Rise,
    Inflate,
    Twist,
}

impl WarpStyle {
    pub const ALL: [WarpStyle; 10] = [
        WarpStyle::Arch,
        WarpStyle::ArcUpper,
        WarpStyle::ArcLower,
        WarpStyle::Bulge,
        WarpStyle::Squeeze,
        WarpStyle::Flag,
        WarpStyle::Wave,
        WarpStyle::Rise,
        WarpStyle::Inflate,
        WarpStyle::Twist,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WarpStyle::Arch => "Arch",
            WarpStyle::ArcUpper => "Arc Upper",
            WarpStyle::ArcLower => "Arc Lower",
            WarpStyle::Bulge => "Bulge",
            WarpStyle::Squeeze => "Squeeze",
            WarpStyle::Flag => "Flag",
            WarpStyle::Wave => "Wave",
            WarpStyle::Rise => "Rise",
            WarpStyle::Inflate => "Inflate",
            WarpStyle::Twist => "Twist",
        }
    }
}

/// Envelope warp dialog: the parameters and the live warp group they edit.
pub struct WarpState {
    pub open: bool,
    pub style: WarpStyle,
    /// -1..1.
    pub bend: f64,
    /// Perspective-like distortion, -1..1.
    pub hdist: f64,
    pub vdist: f64,
    /// The live warp group being edited.
    pub target: Option<u64>,
    /// Selection before the dialog opened (restored on Cancel).
    pub selection: Vec<u64>,
}

impl WarpState {
    pub fn params(&self) -> WarpParams {
        WarpParams { style: self.style, bend: self.bend, hdist: self.hdist, vdist: self.vdist }
    }
}

impl Default for WarpState {
    fn default() -> Self {
        WarpState { open: false, style: WarpStyle::Arch, bend: 0.5, hdist: 0.0, vdist: 0.0, target: None, selection: Vec::new() }
    }
}

/// A non-destructive effect on a group: its children are the result,
/// rebuilt when the parameters (or a blend's end shapes) change, until the
/// group is expanded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LiveEffect {
    Warp(WarpEffect),
    /// The first and last children are the ends; the ones between are
    /// generated.
    Blend { steps: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarpEffect {
    pub style: WarpStyle,
    pub bend: f64,
    pub hdist: f64,
    pub vdist: f64,
    /// The envelope at rest: unit square → document.
    pub frame: Affine,
    /// The shapes as they were before warping.
    pub sources: Vec<Shape>,
}

impl WarpEffect {
    pub fn params(&self) -> WarpParams {
        WarpParams { style: self.style, bend: self.bend, hdist: self.hdist, vdist: self.vdist }
    }

    pub fn set_params(&mut self, w: WarpParams) {
        (self.style, self.bend, self.hdist, self.vdist) = (w.style, w.bend, w.hdist, w.vdist);
    }
}

/// Give `new` the ids of `old` by position (nested too); the rest get fresh
/// ids, so regenerating keeps selections and keeps ids unique.
fn adopt_ids(new: &mut [Shape], old: &[Shape], doc: &mut crate::document::Document) {
    for (k, n) in new.iter_mut().enumerate() {
        let o = old.get(k);
        n.id = match o {
            Some(o) => o.id,
            None => doc.fresh_id(),
        };
        let empty = Vec::new();
        let oc = o.and_then(|o| o.kind.children()).unwrap_or(&empty).clone();
        if let Some(c) = n.kind.children_mut() {
            adopt_ids(c, &oc, doc);
        }
    }
}

pub struct BlendState {
    pub open: bool,
    pub steps: u32,
    /// The blend being previewed while the dialog is open, and the
    /// selection to restore on Cancel.
    pub target: Option<u64>,
    pub selection: Vec<u64>,
}

impl Default for BlendState {
    fn default() -> Self {
        BlendState { open: false, steps: 5, target: None, selection: Vec::new() }
    }
}

pub struct TraceState {
    pub open: bool,
    pub colours: u32,
    /// 0..1: raster resolution of the trace.
    pub detail: f32,
    /// Regions smaller than this many traced pixels are dropped.
    pub speckle: f64,
    pub smooth: f32,
}

impl Default for TraceState {
    fn default() -> Self {
        TraceState { open: false, colours: 6, detail: 0.5, speckle: 4.0, smooth: 0.5 }
    }
}

// ---------------------------------------------------------------------------
// Shapes as document-space paths
// ---------------------------------------------------------------------------

/// `s` as a plain path in document space (text and primitives become their
/// outlines). Groups and booleans recurse; None for an empty outline.
pub fn as_doc_path(s: &Shape, fonts: &mut FontManager) -> Option<Shape> {
    match &s.kind {
        ShapeKind::Group { children } => {
            let mut g = s.clone();
            g.kind = ShapeKind::Group { children: children.iter().filter_map(|c| as_doc_path(c, fonts)).collect() };
            Some(g)
        }
        _ => {
            let (path, _) = crate::render::compute_outline(s, fonts);
            if path.elements().is_empty() {
                return None;
            }
            let mut n = s.clone();
            n.kind = ShapeKind::Path(geom::bez_to_pathdata(&path));
            n.transform = Affine::IDENTITY;
            crate::render::keep_gradient(s, &mut n, fonts);
            if let Some(c) = &s.clip {
                // Clips are document-space already.
                n.clip = Some(c.clone());
            }
            Some(n)
        }
    }
}

/// Where the warp moves `p` (in `bounds`).
#[cfg(test)]
pub fn warp_point(p: Point, bounds: Rect, style: WarpStyle, bend: f64, hdist: f64, vdist: f64) -> Point {
    let frame = Affine::new([bounds.width().max(1e-9), 0.0, 0.0, bounds.height().max(1e-9), bounds.x0, bounds.y0]);
    WarpParams { style, bend, hdist, vdist }.map(frame, p)
}

/// Envelope warp parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpParams {
    pub style: WarpStyle,
    /// -1..1.
    pub bend: f64,
    pub hdist: f64,
    pub vdist: f64,
}

impl WarpParams {
    /// Warp `p` by the envelope whose rest shape is `frame` (unit square →
    /// document).
    pub fn map(&self, frame: Affine, p: Point) -> Point {
        let q = frame.inverse() * p;
        let (x, y) = (q.x * 2.0 - 1.0, q.y * 2.0 - 1.0);
        let (nx, ny) = self.unit(x, y);
        frame * Point::new((nx + 1.0) / 2.0, (ny + 1.0) / 2.0)
    }

    /// The warp on -1..1 coordinates (y down).
    fn unit(&self, x: f64, y: f64) -> (f64, f64) {
        let b = self.bend;
        let pi = std::f64::consts::PI;
        let mut nx = x;
        let ny;
        match self.style {
            WarpStyle::Arch => ny = y - b * (1.0 - x * x),
            WarpStyle::ArcUpper => ny = y - b * (1.0 - x * x) * (1.0 - y) / 2.0,
            WarpStyle::ArcLower => ny = y + b * (1.0 - x * x) * (1.0 + y) / 2.0,
            WarpStyle::Bulge => ny = y * (1.0 + b * (1.0 - x * x)),
            WarpStyle::Squeeze => ny = y * (1.0 - b * 0.6 * (1.0 - x * x)),
            WarpStyle::Flag => ny = y + b * 0.5 * (pi * x).sin(),
            WarpStyle::Wave => {
                ny = y + b * 0.4 * (pi * x).sin();
                nx = x + b * 0.15 * (pi * y).sin();
            }
            WarpStyle::Rise => ny = y - b * 0.5 * (x + 1.0) * (1.0 - 0.25 * (x + 1.0)),
            WarpStyle::Inflate => {
                let r = (x * x + y * y).sqrt().min(1.5);
                let k = 1.0 + b * 0.5 * (1.0 - (r / 1.5).powi(2));
                nx = x * k;
                ny = y * k;
            }
            WarpStyle::Twist => {
                let r = (x * x + y * y).sqrt();
                let a = b * pi * 0.5 * (1.0 - (r / 1.42).min(1.0));
                let (s, c) = a.sin_cos();
                nx = x * c - y * s;
                ny = x * s + y * c;
            }
        }
        // Perspective-like distortion: one side grows, the other shrinks.
        // Both read the same (pre-distortion) coordinates, so the envelope's
        // edges stay the edges of the result.
        let (ox, oy) = (nx, ny);
        (ox * (1.0 + self.vdist * 0.5 * oy), oy * (1.0 + self.hdist * 0.5 * ox))
    }

    /// The warped envelope's outline (document space), for drawing.
    pub fn outline(&self, frame: Affine) -> BezPath {
        let mut path = BezPath::new();
        let r = BezPath::from_vec(vec![PathEl::MoveTo(Point::new(0.0, 0.0)), PathEl::LineTo(Point::new(1.0, 0.0)), PathEl::LineTo(Point::new(1.0, 1.0)), PathEl::LineTo(Point::new(0.0, 1.0)), PathEl::ClosePath]);
        let tol = (frame.determinant().abs().sqrt() * 1e-3).max(0.05);
        path.extend(warp_bez(&(frame * r), &|p| self.map(frame, p), tol));
        path
    }
}

/// `s` warped by the envelope `frame` (unit square → document).
pub fn warp_shape_in(s: &Shape, frame: Affine, w: WarpParams, fonts: &mut FontManager) -> Option<Shape> {
    let mut n = as_doc_path(s, fonts)?;
    // Within a hundredth of a pixel at most sizes.
    let tol = (frame.determinant().abs().sqrt() * 2e-4).clamp(0.005, 0.05);
    warp_in_place(&mut n, &|p| w.map(frame, p), tol);
    Some(n)
}

fn warp_in_place(s: &mut Shape, f: &dyn Fn(Point) -> Point, tol: f64) {
    match &mut s.kind {
        ShapeKind::Group { children } => children.iter_mut().for_each(|c| warp_in_place(c, f, tol)),
        ShapeKind::Path(pd) => {
            let bez = geom::pathdata_to_bez(pd);
            *pd = geom::bez_to_pathdata(&warp_bez(&bez, f, tol));
        }
        _ => {}
    }
}

/// `path` with every point moved by `f`. Each segment becomes cubics that
/// follow the mapped curve within `tol` (split where it bends), so straight
/// edges bend with the envelope and nothing cuts its corners.
pub fn warp_bez(path: &BezPath, f: &dyn Fn(Point) -> Point, tol: f64) -> BezPath {
    fn fit(out: &mut BezPath, c: CubicBez, f: &dyn Fn(Point) -> Point, tol: f64, depth: u32) {
        // Mapped end points, with tangents from the mapped curve's
        // derivative (finite differences), give the candidate cubic.
        let h = 1e-4;
        let (p0, p3) = (f(c.p0), f(c.p3));
        let d0 = (f(c.eval(h)) - p0) / h;
        let d1 = (p3 - f(c.eval(1.0 - h))) / h;
        let m = CubicBez::new(p0, p0 + d0 / 3.0, p3 - d1 / 3.0, p3);
        let ok = depth >= 10 || [0.2, 0.4, 0.5, 0.6, 0.8].iter().all(|&t| (f(c.eval(t)) - m.eval(t)).hypot() <= tol);
        if ok {
            out.curve_to(m.p1, m.p2, m.p3);
        } else {
            let (a, b) = c.subdivide();
            fit(out, a, f, tol, depth + 1);
            fit(out, b, f, tol, depth + 1);
        }
    }
    let mut out = BezPath::new();
    let mut cur = Point::ZERO;
    let mut start = Point::ZERO;
    let line = |a: Point, b: Point| CubicBez::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b);
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                out.move_to(f(p));
                cur = p;
                start = p;
            }
            PathEl::LineTo(p) => {
                if (p - cur).hypot() > 1e-12 {
                    fit(&mut out, line(cur, p), f, tol, 0);
                }
                cur = p;
            }
            PathEl::QuadTo(a, p) => {
                fit(&mut out, kurbo::QuadBez::new(cur, a, p).raise(), f, tol, 0);
                cur = p;
            }
            PathEl::CurveTo(a, b, p) => {
                fit(&mut out, CubicBez::new(cur, a, b, p), f, tol, 0);
                cur = p;
            }
            PathEl::ClosePath => {
                if (cur - start).hypot() > 1e-9 {
                    fit(&mut out, line(cur, start), f, tol, 0);
                }
                out.close_path();
                cur = start;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Blend between shapes
// ---------------------------------------------------------------------------

const BLEND_SAMPLES: usize = 96;

/// Subpaths of a path as closed flags and points evenly spaced by arc length.
fn resample(path: &BezPath, n: usize) -> Vec<(bool, Vec<Point>)> {
    let mut subs: Vec<(bool, Vec<Point>)> = Vec::new();
    kurbo::flatten(path.iter(), 0.05, |el| match el {
        PathEl::MoveTo(p) => subs.push((false, vec![p])),
        PathEl::LineTo(p) => {
            if let Some(s) = subs.last_mut() {
                s.1.push(p);
            }
        }
        PathEl::ClosePath => {
            if let Some(s) = subs.last_mut() {
                s.0 = true;
                let first = s.1[0];
                s.1.push(first);
            }
        }
        _ => {}
    });
    subs.retain(|s| s.1.len() >= 2);
    subs.into_iter()
        .map(|(closed, pts)| {
            let mut acc = vec![0.0];
            for w in pts.windows(2) {
                acc.push(acc.last().unwrap() + (w[1] - w[0]).hypot());
            }
            let total = acc.last().copied().unwrap_or(0.0).max(1e-9);
            let count = if closed { n } else { n + 1 };
            let div = n as f64;
            let mut out = Vec::with_capacity(count);
            let mut j = 0;
            for k in 0..count {
                let at = k as f64 / div * total;
                while j + 2 < acc.len() && acc[j + 1] < at {
                    j += 1;
                }
                let seg = (acc[j + 1] - acc[j]).max(1e-12);
                out.push(pts[j].lerp(pts[j + 1], ((at - acc[j]) / seg).clamp(0.0, 1.0)));
            }
            (closed, out)
        })
        .collect()
}

fn signed_area(p: &[Point]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].to_vec2().cross(p[(i + 1) % n].to_vec2())).sum::<f64>() / 2.0
}

/// Rotate (and reverse) the closed ring `b` to line up with `a`.
fn align_ring(a: &[Point], b: &mut Vec<Point>) {
    if signed_area(a).signum() != signed_area(b).signum() {
        b.reverse();
    }
    let n = b.len();
    let ca = a.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / a.len() as f64;
    let cb = b.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / n as f64;
    let best = (0..n)
        .min_by(|&r1, &r2| {
            let cost = |r: usize| (0..n).step_by(4).map(|i| ((a[i].to_vec2() - ca) - (b[(i + r) % n].to_vec2() - cb)).hypot2()).sum::<f64>();
            cost(r1).total_cmp(&cost(r2))
        })
        .unwrap_or(0);
    b.rotate_left(best);
}

fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

fn lerp_paint(a: Option<&Paint>, b: Option<&Paint>, t: f32) -> Option<Paint> {
    match (a, b) {
        (Some(Paint::Solid(x)), Some(Paint::Solid(y))) => Some(Paint::Solid(lerp_rgba(*x, *y, t))),
        (Some(Paint::Solid(x)), None) => Some(Paint::Solid(x.with_alpha(x.a * (1.0 - t)))),
        (None, Some(Paint::Solid(y))) => Some(Paint::Solid(y.with_alpha(y.a * t))),
        (None, None) => None,
        (Some(x), Some(y)) => Some(if t < 0.5 { x.clone() } else { y.clone() }),
        (Some(x), None) => (t < 0.5).then(|| x.clone()),
        (None, Some(y)) => (t >= 0.5).then(|| y.clone()),
    }
}

fn lerp_stroke(a: Option<&Stroke>, b: Option<&Stroke>, t: f32) -> Option<Stroke> {
    let tf = t as f64;
    match (a, b) {
        (Some(x), Some(y)) => {
            let mut s = if t < 0.5 { x.clone() } else { y.clone() };
            s.width = x.width + (y.width - x.width) * tf;
            s.color = lerp_rgba(x.color, y.color, t);
            Some(s)
        }
        (Some(x), None) => (t < 0.5).then(|| Stroke { width: x.width * (1.0 - tf), ..x.clone() }),
        (None, Some(y)) => (t >= 0.5).then(|| Stroke { width: y.width * tf, ..y.clone() }),
        (None, None) => None,
    }
}

/// `steps` shapes morphing from `a` to `b` (both document-space outlines
/// with their styles), in paint order.
pub fn blend_steps(a: &Shape, pa: &BezPath, b: &Shape, pb: &BezPath, steps: u32) -> Vec<Shape> {
    let ra = resample(pa, BLEND_SAMPLES);
    let mut rb = resample(pb, BLEND_SAMPLES);
    if ra.is_empty() || rb.is_empty() {
        return Vec::new();
    }
    // Pair subpaths in order; the shorter list repeats its last subpath.
    let n = ra.len().max(rb.len());
    for (k, s) in rb.iter_mut().enumerate() {
        if let Some(sa) = ra.get(k) {
            if sa.0 && s.0 {
                align_ring(&sa.1, &mut s.1);
            }
        }
    }
    let mut out = Vec::new();
    for i in 1..=steps {
        let t = i as f64 / (steps + 1) as f64;
        let mut path = BezPath::new();
        for k in 0..n {
            let (ca, xa) = &ra[k.min(ra.len() - 1)];
            let (cb, xb) = &rb[k.min(rb.len() - 1)];
            let closed = *ca && *cb;
            let len = xa.len().min(xb.len());
            let pts: Vec<Point> = (0..len).map(|j| xa[j].lerp(xb[j], t)).collect();
            let sp = crate::document::SubPath { nodes: pts.iter().map(|p| crate::document::Node::corner(*p)).collect(), closed };
            let sp = geom::simplify_subpath(&sp, 0.1);
            path.extend(geom::pathdata_to_bez(&PathData { subpaths: vec![sp] }));
        }
        let mut s = Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&path)));
        let tf = t as f32;
        s.name = format!("Step {i}");
        s.fill = lerp_paint(a.fill.as_ref(), b.fill.as_ref(), tf);
        s.stroke = lerp_stroke(a.stroke.as_ref(), b.stroke.as_ref(), tf);
        s.opacity = a.opacity + (b.opacity - a.opacity) * tf;
        s.fill_rule = if t < 0.5 { a.fill_rule } else { b.fill_rule };
        out.push(s);
    }
    out
}

// ---------------------------------------------------------------------------
// Image trace
// ---------------------------------------------------------------------------

/// Colour layers from straight-alpha RGBA: (colour, mask) per layer, lightest
/// first. Each mask covers its colour and every darker one, so stacked
/// layers leave no gaps.
pub fn posterize(rgba: &[u8], w: usize, h: usize, k: usize) -> Vec<(Rgba, Vec<bool>)> {
    let opaque: Vec<usize> = (0..w * h).filter(|i| rgba[i * 4 + 3] >= 128).collect();
    if opaque.is_empty() {
        return Vec::new();
    }
    let px = |i: usize| [rgba[i * 4] as f32, rgba[i * 4 + 1] as f32, rgba[i * 4 + 2] as f32];
    let step = (opaque.len() / 20000).max(1);
    let sample: Vec<[f32; 3]> = opaque.iter().step_by(step).map(|&i| px(i)).collect();
    let d2 = |a: &[f32; 3], b: &[f32; 3]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
    // Farthest-point start (deterministic), then k-means.
    let mut centres = vec![sample[0]];
    while centres.len() < k.max(1) {
        let far = sample.iter().max_by(|a, b| {
            let da = centres.iter().map(|c| d2(a, c)).fold(f32::MAX, f32::min);
            let db = centres.iter().map(|c| d2(b, c)).fold(f32::MAX, f32::min);
            da.total_cmp(&db)
        });
        match far {
            Some(f) if centres.iter().all(|c| d2(f, c) > 1.0) => centres.push(*f),
            _ => break,
        }
    }
    let nearest = |p: &[f32; 3], cs: &[[f32; 3]]| (0..cs.len()).min_by(|&a, &b| d2(p, &cs[a]).total_cmp(&d2(p, &cs[b]))).unwrap_or(0);
    for _ in 0..12 {
        let mut sum = vec![[0f64; 4]; centres.len()];
        for p in &sample {
            let c = nearest(p, &centres);
            sum[c][0] += p[0] as f64;
            sum[c][1] += p[1] as f64;
            sum[c][2] += p[2] as f64;
            sum[c][3] += 1.0;
        }
        for (c, s) in centres.iter_mut().zip(&sum) {
            if s[3] > 0.0 {
                *c = [(s[0] / s[3]) as f32, (s[1] / s[3]) as f32, (s[2] / s[3]) as f32];
            }
        }
    }
    let lum = |c: &[f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let mut order: Vec<usize> = (0..centres.len()).collect();
    order.sort_by(|a, b| lum(&centres[*b]).total_cmp(&lum(&centres[*a])));
    let mut rank = vec![0usize; centres.len()];
    for (r, c) in order.iter().enumerate() {
        rank[*c] = r;
    }
    let mut level = vec![usize::MAX; w * h];
    for &i in &opaque {
        level[i] = rank[nearest(&px(i), &centres)];
    }
    order
        .iter()
        .enumerate()
        .map(|(r, &c)| {
            let col = Rgba { r: centres[c][0] / 255.0, g: centres[c][1] / 255.0, b: centres[c][2] / 255.0, a: 1.0 };
            (col, level.iter().map(|l| *l != usize::MAX && *l >= r).collect())
        })
        .collect()
}

/// Trace a straight-alpha raster into stacked colour shapes (lightest at
/// the bottom), placed over `rect` in document space.
/// Outline the set pixels of `mask` with smooth curves and sharp corners
/// (visioncortex, the engine of VTracer): clusters smaller than `speckle`
/// pixels are dropped; `smooth` 0..1 trades corners for curves.
fn trace_mask(mask: &[bool], w: usize, h: usize, smooth: f32, speckle: f64, to_doc: Affine) -> BezPath {
    use visioncortex::{BinaryImage, CompoundPathElement, PathSimplifyMode};
    let mut img = BinaryImage::new_w_h(w, h);
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                img.set_pixel(x, y, true);
            }
        }
    }
    let corner = (35.0 + smooth as f64 * 85.0).to_radians();
    let length = 3.0 + smooth as f64 * 3.0;
    let splice = 45f64.to_radians();
    let mut out = BezPath::new();
    let pt = |x: f64, y: f64| to_doc * Point::new(x, y);
    for c in img.to_clusters(false).iter() {
        if (c.size() as f64) < speckle.max(1.0) {
            continue;
        }
        for el in c.to_compound_path(PathSimplifyMode::Spline, corner, length, 10, splice).iter() {
            match el {
                CompoundPathElement::Spline(sp) => {
                    let p = &sp.points;
                    if p.len() < 4 {
                        continue;
                    }
                    out.move_to(pt(p[0].x, p[0].y));
                    for k in (1..p.len() - 2).step_by(3) {
                        out.curve_to(pt(p[k].x, p[k].y), pt(p[k + 1].x, p[k + 1].y), pt(p[k + 2].x, p[k + 2].y));
                    }
                    out.close_path();
                }
                CompoundPathElement::PathF64(pf) if pf.path.len() >= 3 => {
                    out.move_to(pt(pf.path[0].x, pf.path[0].y));
                    for q in &pf.path[1..] {
                        out.line_to(pt(q.x, q.y));
                    }
                    out.close_path();
                }
                CompoundPathElement::PathI32(pi) if pi.path.len() >= 3 => {
                    out.move_to(pt(pi.path[0].x as f64, pi.path[0].y as f64));
                    for q in &pi.path[1..] {
                        out.line_to(pt(q.x as f64, q.y as f64));
                    }
                    out.close_path();
                }
                _ => {}
            }
        }
    }
    out
}

pub fn trace_raster(rgba: &[u8], w: usize, h: usize, rect: Rect, st: &TraceState) -> Vec<Shape> {
    let to_doc = Affine::new([rect.width() / w as f64, 0.0, 0.0, rect.height() / h as f64, rect.x0, rect.y0]);
    let mut out = Vec::new();
    for (i, (col, mask)) in posterize(rgba, w, h, st.colours.clamp(2, 32) as usize).into_iter().enumerate() {
        let path = trace_mask(&mask, w, h, st.smooth, st.speckle, to_doc);
        if path.elements().is_empty() {
            continue;
        }
        let mut s = Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&path)));
        s.name = format!("Colour {}", i + 1);
        s.fill = Some(Paint::Solid(col));
        s.stroke = None;
        s.fill_rule = FillRule::EvenOdd;
        out.push(s);
    }
    out
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

impl App {
    /// Top shape masks the rest: its luminance (or alpha) sets their opacity.
    /// Several masked shapes are grouped first.
    pub fn make_mask(&mut self, mode: MaskMode) {
        let shapes = self.selected_bottom_up();
        if shapes.len() < 2 {
            self.notify("Select the artwork and the mask shape on top");
            return;
        }
        self.checkpoint("");
        let mask = shapes.last().unwrap().clone();
        self.doc.remove(mask.id);
        let rest: Vec<u64> = shapes[..shapes.len() - 1].iter().map(|s| s.id).collect();
        let target = if rest.len() == 1 {
            rest[0]
        } else {
            let parts = &shapes[..shapes.len() - 1];
            let mut g = Shape::new(ShapeKind::Group { children: parts.to_vec() });
            g.fill = None;
            g.name = "Masked".into();
            self.combine_into(parts, g)
        };
        if let Some(t) = self.doc.get_mut(target) {
            let mut m = mask;
            m.clip = None;
            t.mask = Some(Box::new(Mask { shape: m, mode, invert: false }));
        }
        self.selection = vec![target];
        self.notify(match mode {
            MaskMode::Luminance => "Masked: light areas of the mask show, dark areas hide",
            MaskMode::Alpha => "Masked: the mask's opacity shows the artwork",
        });
    }

    /// Put the mask back as a shape above.
    pub fn release_mask(&mut self) {
        let targets: Vec<u64> = self.selected_shapes().iter().filter(|s| s.mask.is_some()).map(|s| s.id).collect();
        if targets.is_empty() {
            return;
        }
        self.checkpoint("");
        let mut sel = Vec::new();
        for id in targets {
            let Some(mask) = self.doc.get_mut(id).and_then(|s| s.mask.take()) else { continue };
            let idx = self.doc.index_of(id).unwrap_or(0);
            sel.push(id);
            let mut m = mask.shape;
            m.id = 0;
            sel.push(self.doc.insert_shape(idx + 1, m));
        }
        self.selection = sel;
    }

    /// A live blend between the two selected shapes, grouped with them.
    pub fn blend_selection(&mut self, steps: u32) {
        let shapes = self.selected_bottom_up();
        if shapes.len() != 2 {
            self.notify("Select two shapes to blend between");
            return;
        }
        let (a, b) = (&shapes[0], &shapes[1]);
        let pa = crate::render::compute_outline(a, &mut self.fonts).0;
        let pb = crate::render::compute_outline(b, &mut self.fonts).0;
        let mid = blend_steps(a, &pa, b, &pb, steps.clamp(1, 500));
        if mid.is_empty() {
            self.notify("These shapes have no outline to blend");
            return;
        }
        self.checkpoint("");
        let mut children = vec![a.clone()];
        children.extend(mid);
        children.push(b.clone());
        let mut g = Shape::new(ShapeKind::Group { children });
        g.fill = None;
        g.name = "Blend".into();
        g.live = Some(LiveEffect::Blend { steps: steps.clamp(1, 500) });
        let id = self.combine_into(&shapes, g);
        self.selection = vec![id];
    }

    /// Rebuild the steps of live blend `id` (between its first and last
    /// children); true when they changed.
    pub fn regen_blend(&mut self, id: u64) -> bool {
        let Some(g) = self.doc.get(id).cloned() else { return false };
        let Some(LiveEffect::Blend { steps }) = g.live else { return false };
        let Some(children) = g.kind.children() else { return false };
        if children.len() < 2 {
            return false;
        }
        let (a, b) = (&children[0], &children[children.len() - 1]);
        let pa = crate::render::compute_outline(a, &mut self.fonts).0;
        let pb = crate::render::compute_outline(b, &mut self.fonts).0;
        let mut mid = blend_steps(a, &pa, b, &pb, steps);
        adopt_ids(&mut mid, &children[1..children.len() - 1], &mut self.doc);
        let mut out = g.clone();
        let mut list = vec![a.clone()];
        list.extend(mid);
        list.push(b.clone());
        if list == *children {
            return false;
        }
        out.kind = ShapeKind::Group { children: list };
        self.doc.replace(out);
        self.doc.fresh_rev();
        true
    }

    /// Change a live blend's number of steps.
    pub fn set_blend_steps(&mut self, id: u64, steps: u32) {
        if matches!(self.doc.get(id).and_then(|s| s.live.as_ref()), Some(LiveEffect::Blend { steps: s }) if *s != steps) {
            self.checkpoint("blend-steps");
            self.apply_blend_steps(id, steps);
        }
    }

    /// Open the blend dialog: the blend appears at once (or, with a live
    /// blend selected, is edited) and follows the dialog until Cancel.
    pub fn begin_blend(&mut self) {
        let before = self.selection.clone();
        if let [id] = self.selection[..] {
            if let Some(LiveEffect::Blend { steps }) = self.doc.get(id).and_then(|s| s.live.clone()) {
                self.checkpoint("");
                self.ui.blend = BlendState { open: true, steps, target: Some(id), selection: before };
                return;
            }
        }
        self.blend_selection(self.ui.blend.steps);
        let made = self.selection.first().copied().filter(|id| matches!(self.doc.get(*id).and_then(|s| s.live.as_ref()), Some(LiveEffect::Blend { .. })));
        if let Some(id) = made {
            self.ui.blend.open = true;
            self.ui.blend.target = Some(id);
            self.ui.blend.selection = before;
        }
    }

    /// Show the dialog's step count on the blend being previewed.
    pub fn blend_preview(&mut self) {
        if let Some(id) = self.ui.blend.target {
            if matches!(self.doc.get(id).and_then(|s| s.live.as_ref()), Some(LiveEffect::Blend { steps }) if *steps != self.ui.blend.steps) {
                self.apply_blend_steps(id, self.ui.blend.steps);
            }
        }
    }

    /// Close the blend dialog, keeping the blend or putting things back.
    pub fn end_blend(&mut self, keep: bool) {
        if keep {
            self.blend_preview();
        } else if self.ui.blend.target.is_some() {
            self.history.cancel(&mut self.doc);
            self.selection = std::mem::take(&mut self.ui.blend.selection);
            self.selection.retain(|id| self.doc.get(*id).is_some());
        }
        self.ui.blend.selection.clear();
        self.ui.blend.target = None;
        self.ui.blend.open = false;
    }

    fn apply_blend_steps(&mut self, id: u64, steps: u32) {
        let Some(mut g) = self.doc.get(id).cloned() else { return };
        if !matches!(g.live, Some(LiveEffect::Blend { .. })) {
            return;
        }
        g.live = Some(LiveEffect::Blend { steps: steps.clamp(1, 500) });
        self.doc.replace(g);
        self.regen_blend(id);
        self.dirty = true;
    }

    /// Per frame: rebuild live blends whose end shapes changed.
    pub fn live_tick(&mut self) {
        let rev = self.doc.revision();
        if self.live_key == Some(rev) {
            return;
        }
        let blends: Vec<u64> = self.doc.all_ids().into_iter().filter(|id| matches!(self.doc.get(*id).and_then(|s| s.live.as_ref()), Some(LiveEffect::Blend { .. }))).collect();
        for id in blends {
            let Some(g) = self.doc.get(id) else { continue };
            let Some(c) = g.kind.children() else { continue };
            let (Some(a), Some(b)) = (c.first(), c.last()) else { continue };
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                serde_json::to_string(&(a, b, &g.live)).unwrap_or_default().hash(&mut h);
                h.finish()
            };
            if self.blend_keys.get(&id) != Some(&key) {
                self.blend_keys.insert(id, key);
                self.regen_blend(id);
            }
        }
        self.live_key = Some(self.doc.revision());
    }

    /// Rebuild the result of live warp `id` from its sources.
    pub fn regen_warp(&mut self, id: u64) {
        let Some(mut g) = self.doc.get(id).cloned() else { return };
        let Some(LiveEffect::Warp(w)) = &g.live else { return };
        let (frame, params) = (w.frame, w.params());
        let sources = w.sources.clone();
        let mut out: Vec<Shape> = sources.iter().filter_map(|s| warp_shape_in(s, frame, params, &mut self.fonts)).collect();
        let old = g.kind.children().cloned().unwrap_or_default();
        adopt_ids(&mut out, if old.is_empty() { &sources } else { &old }, &mut self.doc);
        g.kind = ShapeKind::Group { children: out };
        self.doc.replace(g);
        self.doc.fresh_rev();
        self.dirty = true;
    }

    /// Open the envelope warp dialog: edits the selected live warp, or
    /// wraps the selection in a new one.
    pub fn begin_warp(&mut self) {
        if let [id] = self.selection[..] {
            if let Some(LiveEffect::Warp(w)) = self.doc.get(id).and_then(|s| s.live.as_ref()) {
                let p = w.params();
                self.checkpoint("Warp");
                let st = &mut self.ui.warp;
                (st.style, st.bend, st.hdist, st.vdist) = (p.style, p.bend, p.hdist, p.vdist);
                st.target = Some(id);
                st.selection = vec![id];
                st.open = true;
                return;
            }
        }
        let shapes = self.selected_bottom_up();
        let Some(b) = self.selection_bounds() else { return };
        if shapes.is_empty() {
            return;
        }
        self.checkpoint("Warp");
        let before = self.selection.clone();
        let p = self.ui.warp.params();
        let frame = Affine::new([b.width().max(1e-6), 0.0, 0.0, b.height().max(1e-6), b.x0, b.y0]);
        let mut g = Shape::new(ShapeKind::Group { children: Vec::new() });
        g.fill = None;
        g.name = "Warp".into();
        g.live = Some(LiveEffect::Warp(WarpEffect { style: p.style, bend: p.bend, hdist: p.hdist, vdist: p.vdist, frame, sources: shapes.clone() }));
        // Results take the sources' ids (they leave the document).
        let mut out: Vec<Shape> = shapes.iter().filter_map(|s| warp_shape_in(s, frame, p, &mut self.fonts)).collect();
        for (o, s) in out.iter_mut().zip(&shapes) {
            o.id = s.id;
        }
        g.kind = ShapeKind::Group { children: out };
        let id = self.combine_into(&shapes, g);
        self.selection = vec![id];
        let st = &mut self.ui.warp;
        st.target = Some(id);
        st.selection = before;
        st.open = true;
    }

    /// Set the parameters of live warp `id` (no checkpoint).
    pub fn set_warp_params(&mut self, id: u64, p: WarpParams) {
        let Some(mut g) = self.doc.get(id).cloned() else { return };
        let Some(LiveEffect::Warp(w)) = &mut g.live else { return };
        if w.params() == p {
            return;
        }
        w.set_params(p);
        self.doc.replace(g);
        self.regen_warp(id);
    }

    /// Show the dialog's parameters on the warp group.
    pub fn warp_preview(&mut self) {
        if let Some(id) = self.ui.warp.target {
            let p = self.ui.warp.params();
            self.set_warp_params(id, p);
        }
    }

    /// Close the warp dialog, keeping the warp or putting things back.
    pub fn end_warp(&mut self, keep: bool) {
        if keep {
            self.warp_preview();
        } else {
            self.history.cancel(&mut self.doc);
            self.selection = std::mem::take(&mut self.ui.warp.selection);
            self.selection.retain(|id| self.doc.get(*id).is_some());
        }
        self.ui.warp.selection.clear();
        self.ui.warp.target = None;
        self.ui.warp.open = false;
    }

    /// Make a live warp or blend permanent: its current result becomes
    /// plain shapes (a group of one is replaced by that shape).
    pub fn expand_live(&mut self, id: u64) {
        let Some(mut g) = self.doc.get(id).cloned() else { return };
        if g.live.is_none() {
            return;
        }
        self.checkpoint("");
        g.live = None;
        let single = matches!(&g.kind, ShapeKind::Group { children } if children.len() == 1) && g.clip.is_none() && g.mask.is_none() && g.shadow.is_none() && g.blur == 0.0 && g.opacity == 1.0;
        if single {
            let child = g.kind.children().unwrap()[0].clone();
            let ids = self.doc.replace_with(id, vec![child]);
            self.selection = ids;
        } else {
            self.doc.replace(g);
            self.doc.fresh_rev();
            self.selection = vec![id];
        }
        self.blend_keys.remove(&id);
    }

    /// Undo a live warp or blend: a warp gives back its sources, a blend
    /// its two end shapes.
    pub fn release_live(&mut self, id: u64) {
        let Some(g) = self.doc.get(id).cloned() else { return };
        let shapes = match &g.live {
            Some(LiveEffect::Warp(w)) => w.sources.clone(),
            Some(LiveEffect::Blend { .. }) => {
                let c = g.kind.children().cloned().unwrap_or_default();
                match (c.first(), c.last()) {
                    (Some(a), Some(b)) if c.len() >= 2 => vec![a.clone(), b.clone()],
                    _ => c,
                }
            }
            None => return,
        };
        self.checkpoint("");
        self.selection = self.doc.replace_with(id, shapes);
        self.blend_keys.remove(&id);
    }

    /// Trace the selected shape (usually an image) into colour shapes above it.
    pub fn trace_selection(&mut self) {
        let Some(s) = self.selected_bottom_up().last().cloned() else { return };
        let st = &self.ui.trace;
        let b = self.cache.update(&s, &mut self.fonts).bounds;
        let Some(b) = b else { return };
        let longest = b.width().max(b.height()).max(1e-6);
        let target_px = 200.0 + st.detail as f64 * 1000.0;
        let Some(r) = crate::render::svg_paint::rasterize_shape_svg(&s, &self.doc, &mut self.fonts, target_px / longest, target_px) else {
            self.notify("Nothing to trace");
            return;
        };
        let shapes = trace_raster(&r.rgba, r.w, r.h, r.rect, &self.ui.trace);
        if shapes.is_empty() {
            self.notify("Nothing to trace");
            return;
        }
        self.checkpoint("");
        let n = shapes.len();
        let mut g = Shape::new(ShapeKind::Group { children: shapes });
        g.fill = None;
        g.name = format!("{} (traced)", if s.name.is_empty() { "Image" } else { &s.name });
        let idx = self.doc.index_of(s.id).map(|i| i + 1).unwrap_or(self.doc.shapes().len());
        let id = self.doc.insert_shape(idx, g);
        self.selection = vec![id];
        self.notify(&format!("Traced into {n} colour layers"));
    }
}
