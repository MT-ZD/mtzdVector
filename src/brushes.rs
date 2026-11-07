//! The brush library: vector brushes in categories, like Affinity's path
//! brushes, but built from paths so the result stays editable. Simple
//! brushes are one stroke with a width profile on the drawn curve; media
//! brushes (bristles, pencils, chalk, watercolour, spray, hatching) are a
//! group of strands, dots or ticks laid along it.

use kurbo::{BezPath, Point, Vec2};

use crate::document::{LineCap, LineJoin, Node, PathData, Shape, ShapeKind, Stroke, SubPath, WidthPoint};
use crate::freehand::{BrushOpts, grain, widths_from};
use crate::geom;

/// Most points in a width profile.
const PROFILE_POINTS: usize = 24;
/// Most dots a spray or stipple stroke lays down.
const MAX_DOTS: usize = 1500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum BrushStyle {
    // Basic
    #[default]
    Round,
    Pointed,
    Marker,
    Highlighter,
    Calligraphy,
    FlatNib,
    Dots,
    // Inks
    Ink,
    DipPen,
    BrushPen,
    Splatter,
    // Pencils
    Pencil,
    Graphite,
    ColouredPencil,
    // Dry media
    Charcoal,
    CharcoalStick,
    Chalk,
    Pastel,
    // Paint
    Bristle,
    DryBrush,
    FlatBrush,
    Gouache,
    // Watercolour
    Wash,
    WetEdge,
    Bleed,
    // Texture
    Spray,
    Stipple,
    Hatching,
    Crosshatch,
}

/// Brush categories in menu order, with their brushes.
pub const CATEGORIES: [(&str, &[BrushStyle]); 7] = {
    use BrushStyle::*;
    [
        ("Basic", &[Round, Pointed, Marker, Highlighter, Calligraphy, FlatNib, Dots]),
        ("Inks", &[Ink, DipPen, BrushPen, Splatter]),
        ("Pencils", &[Pencil, Graphite, ColouredPencil]),
        ("Dry media", &[Charcoal, CharcoalStick, Chalk, Pastel]),
        ("Paint", &[Bristle, DryBrush, FlatBrush, Gouache]),
        ("Watercolour", &[Wash, WetEdge, Bleed]),
        ("Texture", &[Spray, Stipple, Hatching, Crosshatch]),
    ]
};

impl BrushStyle {
    pub fn label(self) -> &'static str {
        use BrushStyle::*;
        match self {
            Round => "Round",
            Pointed => "Pointed",
            Marker => "Marker",
            Highlighter => "Highlighter",
            Calligraphy => "Calligraphy 45°",
            FlatNib => "Flat nib",
            Dots => "Dots",
            Ink => "Ink",
            DipPen => "Dip pen",
            BrushPen => "Brush pen",
            Splatter => "Ink splatter",
            Pencil => "Pencil",
            Graphite => "Soft graphite",
            ColouredPencil => "Coloured pencil",
            Charcoal => "Charcoal",
            CharcoalStick => "Charcoal stick",
            Chalk => "Chalk",
            Pastel => "Pastel",
            Bristle => "Bristle",
            DryBrush => "Dry brush",
            FlatBrush => "Flat brush",
            Gouache => "Gouache",
            Wash => "Wash",
            WetEdge => "Wet edge",
            Bleed => "Bleed",
            Spray => "Spray",
            Stipple => "Stipple",
            Hatching => "Hatching",
            Crosshatch => "Crosshatch",
        }
    }

    /// Whether pen pressure or speed changes the width.
    pub fn varies(self) -> bool {
        use BrushStyle::*;
        !matches!(self, Marker | Highlighter | Dots | Spray | Stipple | Hatching | Crosshatch)
    }

    #[cfg(test)]
    pub fn all() -> impl Iterator<Item = BrushStyle> {
        CATEGORIES.iter().flat_map(|(_, b)| b.iter().copied())
    }
}

/// Repeatable random number in 0..1 for (`a`, `b`).
fn rnd(a: u64, b: u64) -> f64 {
    let mut z = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_add(0x632B_E59B_D9B4_E019).rotate_left(29);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// The drawn curve sampled evenly: positions, unit normals, arc length and
/// the relative width (pressure / speed) at each sample.
struct Line {
    p: Vec<Point>,
    n: Vec<Vec2>,
    s: Vec<f64>,
    w: Vec<f64>,
    len: f64,
}

impl Line {
    fn new(sp: &SubPath, raw: &[Point], rel: &[f64], step: f64) -> Option<Line> {
        let mut bez = BezPath::new();
        geom::subpath_to_bez(sp, &mut bez);
        let mut poly: Vec<Point> = Vec::new();
        kurbo::flatten(bez.iter(), step * 0.1, |el| match el {
            kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => poly.push(p),
            _ => {}
        });
        if poly.len() < 2 {
            return None;
        }
        let p = crate::freehand::resample(&poly, step);
        if p.len() < 2 {
            return None;
        }
        let k = p.len();
        let mut s = vec![0.0; k];
        for i in 1..k {
            s[i] = s[i - 1] + (p[i] - p[i - 1]).hypot();
        }
        let len = s[k - 1].max(1e-9);
        let n = (0..k)
            .map(|i| {
                let d = p[(i + 1).min(k - 1)] - p[i.saturating_sub(1)];
                let h = d.hypot();
                if h > 1e-12 { Vec2::new(-d.y / h, d.x / h) } else { Vec2::new(0.0, 1.0) }
            })
            .collect();
        // Relative widths: the drawn points' widths by arc fraction.
        let mut acc = vec![0.0; raw.len()];
        for i in 1..raw.len() {
            acc[i] = acc[i - 1] + (raw[i] - raw[i - 1]).hypot();
        }
        let total = acc.last().copied().unwrap_or(1.0).max(1e-9);
        let mut j = 0;
        let w = s
            .iter()
            .map(|si| {
                let at = si / len * total;
                while j + 2 < acc.len() && acc[j + 1] < at {
                    j += 1;
                }
                let (a, b) = (rel[j], rel[(j + 1).min(rel.len() - 1)]);
                let seg = (acc[(j + 1).min(acc.len() - 1)] - acc[j]).max(1e-9);
                a + (b - a) * ((at - acc[j]) / seg).clamp(0.0, 1.0)
            })
            .collect();
        Some(Line { p, n, s, w, len })
    }
}

/// How a strand's width tapers into its ends: lengths in brush widths.
#[derive(Clone, Copy)]
struct Taper {
    start: f64,
    end: f64,
    /// Width at the very tip, relative.
    tip: f64,
    /// Straight taper (pointed) rather than rounded.
    sharp: bool,
}

const ROUNDED: Taper = Taper { start: 2.0, end: 2.0, tip: 0.15, sharp: false };
const NONE: Taper = Taper { start: 0.0, end: 0.0, tip: 1.0, sharp: false };

fn taper_at(t: &Taper, from_start: f64, from_end: f64, width: f64, span: f64) -> f64 {
    let mut v: f64 = 1.0;
    for (d, len) in [(from_start, t.start), (from_end, t.end)] {
        let len = (len * width).min(span * 0.45);
        if len > 1e-9 && d < len {
            let e = (d / len).clamp(0.0, 1.0);
            v = v.min(if t.sharp { t.tip + (1.0 - t.tip) * e } else { t.tip + (1.0 - t.tip) * e.sqrt() });
        }
    }
    v
}

/// A strand: part of the line, offset across it.
struct Strand {
    /// Offset across, as a fraction of the half width (−1..1).
    off: f64,
    /// Width as a fraction of the brush width.
    frac: f64,
    /// Arc lengths cut from the start and the end.
    trim0: f64,
    trim1: f64,
    taper: Taper,
    /// Grain: (period in widths, depth 0..1).
    grain: Option<(f64, f64)>,
    opacity: f32,
    seed: u64,
}

impl Strand {
    fn new(off: f64, frac: f64) -> Self {
        Strand { off, frac, trim0: 0.0, trim1: 0.0, taper: ROUNDED, grain: None, opacity: 1.0, seed: 0 }
    }
}

/// Path shape of `pts` (a polyline) with `stroke`, or None when too short.
fn stroke_shape(pts: &[Point], stroke: Stroke, opacity: f32, px: f64) -> Option<Shape> {
    if pts.len() < 2 {
        return None;
    }
    let raw = SubPath { nodes: pts.iter().map(|p| Node::corner(*p)).collect(), closed: false };
    let sp = geom::simplify_subpath(&raw, px * 0.4);
    if sp.nodes.len() < 2 {
        return None;
    }
    let mut s = Shape::new(ShapeKind::Path(PathData { subpaths: vec![sp] }));
    s.fill = None;
    s.stroke = Some(stroke);
    s.opacity = opacity;
    Some(s)
}

/// The pieces of `st` along `line`; `gaps`: grain level under which the
/// strand breaks (dry media).
fn strand_shapes(line: &Line, st: &Strand, opts: &BrushOpts, px: f64, gaps: Option<f64>, out: &mut Vec<Shape>) {
    let width = opts.width;
    let (a, b) = (st.trim0, line.len - st.trim1);
    if b - a < width * 0.5 {
        return;
    }
    // Runs of samples within [a, b], split where the grain breaks.
    let mut runs: Vec<Vec<usize>> = vec![Vec::new()];
    for i in 0..line.p.len() {
        let s = line.s[i];
        if s < a || s > b {
            continue;
        }
        let broken = gaps.is_some_and(|g| grain(s / (width * 1.3) + st.seed as f64 * 7.31) < g);
        if broken {
            if !runs.last().unwrap().is_empty() {
                runs.push(Vec::new());
            }
        } else {
            runs.last_mut().unwrap().push(i);
        }
    }
    for run in runs.into_iter().filter(|r| r.len() >= 2) {
        let (s0, s1) = (line.s[run[0]], line.s[*run.last().unwrap()]);
        let span = (s1 - s0).max(1e-9);
        let pts: Vec<Point> = run.iter().map(|&i| line.p[i] + line.n[i] * (st.off * width * 0.5 * line.w[i])).collect();
        // Pieces of a broken strand taper a little at each break.
        let taper = if gaps.is_some() { Taper { start: st.taper.start.min(0.6), end: st.taper.end.min(0.6), tip: 0.3, sharp: false } } else { st.taper };
        let n = PROFILE_POINTS.min(run.len());
        let profile = (0..n)
            .map(|k| {
                let t = k as f64 / (n - 1).max(1) as f64;
                let i = run[((run.len() - 1) as f64 * t).round() as usize];
                let s = line.s[i];
                let mut v = line.w[i] * taper_at(&taper, s - s0, s1 - s, width, span);
                if let Some((period, depth)) = st.grain {
                    v *= 1.0 - depth + depth * grain(s / (width * period).max(1e-6) + st.seed as f64 * 3.7);
                }
                WidthPoint { t, w: v.clamp(0.02, 1.0) }
            })
            .collect();
        let stroke = Stroke { width: (width * st.frac).max(px * 0.5), color: opts.color, cap: LineCap::Round, join: LineJoin::Round, profile, ..Default::default() };
        if let Some(s) = stroke_shape(&pts, stroke, st.opacity, px) {
            out.push(s);
        }
    }
}

/// A round dot.
fn dot(c: Point, r: f64, opts: &BrushOpts, opacity: f32) -> Shape {
    let mut s = Shape::new(ShapeKind::Ellipse { w: r * 2.0, h: r * 2.0 });
    s.transform = kurbo::Affine::translate((c.x - r, c.y - r));
    s.fill = Some(crate::document::Paint::Solid(opts.color));
    s.stroke = None;
    s.opacity = opacity;
    s
}

/// Dots scattered along the line: every `step` widths, `per` dots within
/// `spread` half widths, radii `r` (in widths).
fn scatter(line: &Line, opts: &BrushOpts, step: f64, per: usize, spread: f64, r: (f64, f64), opacity: (f32, f32), out: &mut Vec<Shape>) {
    let w = opts.width;
    let step = (w * step).max(line.len / (MAX_DOTS / per.max(1)) as f64);
    let mut j = 0;
    let mut at = 0.0;
    let mut k = 0u64;
    while at <= line.len {
        while j + 1 < line.s.len() && line.s[j + 1] < at {
            j += 1;
        }
        for d in 0..per as u64 {
            let id = k * 16 + d;
            // Denser towards the middle: the sum of two uniforms.
            let across = (rnd(id, 1) + rnd(id, 2) - 1.0) * spread * w * 0.5 * line.w[j];
            let along = (rnd(id, 3) - 0.5) * step;
            let t = line.p[j] + line.n[j] * across + Vec2::new(line.n[j].y, -line.n[j].x) * along;
            let rad = w * (r.0 + (r.1 - r.0) * rnd(id, 4));
            let op = opacity.0 + (opacity.1 - opacity.0) * rnd(id, 5) as f32;
            out.push(dot(t, rad.max(0.2), opts, op));
        }
        at += step;
        k += 1;
    }
}

/// Short parallel ticks along the line at `angle` (radians), one path.
fn hatch(line: &Line, opts: &BrushOpts, px: f64, angles: &[f64], out: &mut Vec<Shape>) {
    let w = opts.width;
    let gap = w * 0.4;
    let mut bez = BezPath::new();
    let mut j = 0;
    let mut at = gap * 0.5;
    let mut k = 0u64;
    while at <= line.len {
        while j + 1 < line.s.len() && line.s[j + 1] < at {
            j += 1;
        }
        for (a, ang) in angles.iter().enumerate() {
            let d = Vec2::new(ang.cos(), ang.sin());
            let half = w * 0.5 * line.w[j] * (0.8 + 0.4 * rnd(k, a as u64));
            let c = line.p[j] + line.n[j] * ((rnd(k, 9 + a as u64) - 0.5) * w * 0.15);
            bez.move_to(c - d * half);
            bez.line_to(c + d * half);
        }
        at += gap;
        k += 1;
    }
    let mut s = Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&bez)));
    s.fill = None;
    s.stroke = Some(Stroke { width: (w * 0.07).max(px * 0.75), color: opts.color, cap: LineCap::Round, join: LineJoin::Round, ..Default::default() });
    out.push(s);
}

/// One stroke on the fitted curve with a width profile: the simple brushes.
fn single(sp: SubPath, pts: &[Point], rel: &[f64], px: f64, opts: &BrushOpts) -> Shape {
    use BrushStyle::*;
    let width = opts.width;
    let mut stroke = Stroke { width, color: opts.color, cap: LineCap::Round, join: LineJoin::Round, ..Default::default() };
    let mut opacity = 1.0;
    match opts.style {
        Dots => stroke.dash = vec![0.0, 1.8],
        Marker => {}
        Highlighter => {
            stroke.cap = LineCap::Butt;
            opacity = 0.45;
        }
        _ => {
            let mut acc = vec![0.0; pts.len()];
            for i in 1..pts.len() {
                acc[i] = acc[i - 1] + (pts[i] - pts[i - 1]).hypot();
            }
            let total = acc.last().copied().unwrap_or(1.0).max(1e-9);
            let taper = match opts.style {
                Ink | Pointed => Taper { start: 5.0, end: 5.0, tip: 0.02, sharp: true },
                DipPen => Taper { start: 3.0, end: 6.0, tip: 0.02, sharp: true },
                BrushPen => Taper { start: 1.0, end: 7.0, tip: 0.03, sharp: true },
                Calligraphy | FlatNib => NONE,
                _ => ROUNDED,
            };
            let n_pts = match opts.style {
                Calligraphy | FlatNib | Charcoal => (total / (width * 0.5).max(px * 4.0)).clamp(24.0, 160.0) as usize,
                _ => PROFILE_POINTS,
            };
            let mut j = 0;
            for k in 0..=n_pts {
                let t = k as f64 / n_pts as f64;
                let at = t * total;
                while j + 1 < acc.len() - 1 && acc[j + 1] < at {
                    j += 1;
                }
                let seg = (acc[j + 1] - acc[j]).max(1e-9);
                let f = ((at - acc[j]) / seg).clamp(0.0, 1.0);
                let mut v = rel[j] + (rel[j + 1] - rel[j]) * f;
                match opts.style {
                    Calligraphy | FlatNib => {
                        // Direction over a short window against the nib angle.
                        let nib = if opts.style == FlatNib { 0.0 } else { std::f64::consts::FRAC_PI_4 };
                        let (a, b) = (pts[j.saturating_sub(2)], pts[(j + 3).min(pts.len() - 1)]);
                        let d = b - a;
                        let across = if d.hypot() > 1e-9 { (d.y.atan2(d.x) - nib).sin().abs() } else { 1.0 };
                        v *= 0.12 + 0.88 * across;
                    }
                    Charcoal => v *= 0.55 + 0.45 * grain(at / (width * 0.6).max(1e-6)),
                    DipPen => v = v.powf(1.8),
                    _ => {}
                }
                v *= taper_at(&taper, at, total - at, width, total);
                stroke.profile.push(WidthPoint { t, w: v.clamp(0.02, 1.0) });
            }
        }
    }
    let mut s = Shape::new(ShapeKind::Path(PathData { subpaths: vec![sp] }));
    s.fill = None;
    s.stroke = Some(stroke);
    s.opacity = opacity;
    s
}

/// `n` strands spread across `spread` half widths, widths `frac` and
/// opacities `op` picked per strand; `f` sets the rest and returns the
/// grain level under which the strand breaks.
#[allow(clippy::too_many_arguments)]
fn strands(line: &Line, opts: &BrushOpts, px: f64, out: &mut Vec<Shape>, n: usize, spread: f64, frac: (f64, f64), op: (f32, f32), f: &dyn Fn(usize, &mut Strand) -> Option<f64>) {
        for i in 0..n {
            let u = if n > 1 { i as f64 / (n - 1) as f64 * 2.0 - 1.0 } else { 0.0 };
            let id = i as u64;
            let off = (u + (rnd(id, 11) - 0.5) * 0.5 / n as f64) * spread;
            let frac = frac.0 + (frac.1 - frac.0) * rnd(id, 12);
            let mut st = Strand::new(off, frac);
            st.opacity = op.0 + (op.1 - op.0) * rnd(id, 13) as f32;
            st.seed = id + 1;
            let gaps = f(i, &mut st);
            strand_shapes(line, &st, opts, px, gaps, out);
        }
}

/// The strands, dots or ticks of a media brush.
fn media(line: &Line, opts: &BrushOpts, px: f64, out: &mut Vec<Shape>) {
    use BrushStyle::*;
    let w = opts.width;
    match opts.style {
        Splatter => {
            let st = Strand { taper: Taper { start: 4.0, end: 5.0, tip: 0.02, sharp: true }, ..Strand::new(0.0, 1.0) };
            strand_shapes(line, &st, opts, px, None, out);
            // Drops thrown off the line, a few far.
            let mut j = 0;
            let mut at = 0.0;
            let mut k = 0u64;
            while at <= line.len {
                while j + 1 < line.s.len() && line.s[j + 1] < at {
                    j += 1;
                }
                if rnd(k, 1) < 0.45 {
                    let side = if rnd(k, 2) < 0.5 { -1.0 } else { 1.0 };
                    let dist = w * (0.7 + 1.6 * rnd(k, 3).powi(2));
                    let r = w * (0.05 + 0.18 * rnd(k, 4).powi(2));
                    out.push(dot(line.p[j] + line.n[j] * side * dist, r, opts, 1.0));
                }
                at += w * 0.9;
                k += 1;
            }
        }
        Pencil => strands(line, opts, px, out, 3, 0.35, (0.3, 0.42), (0.5, 0.7), &|_, st| {
            st.grain = Some((0.35, 0.55));
            st.taper = Taper { start: 1.0, end: 1.5, tip: 0.3, sharp: false };
            None
        }),
        Graphite => strands(line, opts, px, out, 6, 0.75, (0.16, 0.26), (0.35, 0.6), &|_, st| {
            st.grain = Some((0.25, 0.6));
            st.taper = Taper { start: 1.5, end: 2.0, tip: 0.2, sharp: false };
            None
        }),
        ColouredPencil => strands(line, opts, px, out, 5, 0.6, (0.18, 0.28), (0.6, 0.85), &|_, st| {
            st.grain = Some((0.3, 0.5));
            Some(0.22)
        }),
        CharcoalStick => strands(line, opts, px, out, 7, 0.85, (0.15, 0.3), (0.45, 0.9), &|_, st| {
            st.grain = Some((0.4, 0.7));
            Some(0.18)
        }),
        Chalk => strands(line, opts, px, out, 9, 0.9, (0.1, 0.18), (0.7, 0.95), &|_, st| {
            st.grain = Some((0.3, 0.5));
            Some(0.42)
        }),
        Pastel => strands(line, opts, px, out, 5, 0.7, (0.25, 0.38), (0.55, 0.8), &|_, st| {
            st.grain = Some((0.5, 0.45));
            Some(0.28)
        }),
        Bristle => {
            // A solid core with bristle streaks that run out at the tail.
            let core = Strand { trim1: w * 2.5, taper: Taper { start: 0.6, end: 3.0, tip: 0.2, sharp: false }, ..Strand::new(0.0, 0.62) };
            strand_shapes(line, &core, opts, px, None, out);
            strands(line, opts, px, out, 9, 0.9, (0.1, 0.24), (0.7, 1.0), &|i, st| {
                st.trim0 = rnd(i as u64, 21) * w * 0.6;
                st.trim1 = rnd(i as u64, 22).powi(2) * w * 4.0;
                st.taper = Taper { start: 0.5, end: 2.5, tip: 0.1, sharp: true };
                st.grain = Some((2.0, 0.3));
                None
            });
        }
        DryBrush => strands(line, opts, px, out, 11, 0.95, (0.08, 0.18), (0.75, 1.0), &|i, st| {
            st.trim1 = rnd(i as u64, 22) * w * 3.0;
            st.grain = Some((1.0, 0.4));
            Some(0.3)
        }),
        FlatBrush => strands(line, opts, px, out, 7, 0.85, (0.22, 0.34), (0.85, 1.0), &|i, st| {
            st.taper = Taper { start: 0.2, end: 2.0, tip: 0.2, sharp: false };
            st.trim1 = rnd(i as u64, 22).powi(3) * w * 3.5;
            st.grain = Some((3.0, 0.2));
            None
        }),
        Gouache => {
            let core = Strand { trim1: w * 1.2, ..Strand::new(0.0, 0.92) };
            strand_shapes(line, &core, opts, px, None, out);
            strands(line, opts, px, out, 5, 0.8, (0.1, 0.18), (0.45, 0.7), &|i, st| {
                st.trim0 = line.len * (0.45 + 0.3 * rnd(i as u64, 21));
                st.taper = Taper { start: 1.0, end: 2.0, tip: 0.1, sharp: true };
                None
            });
        }
        Wash => {
            for (k, (off, frac, op)) in [(0.0, 1.0, 0.22), (0.12, 0.8, 0.22), (-0.15, 0.55, 0.28)].into_iter().enumerate() {
                let st = Strand { grain: Some((4.0, 0.35)), opacity: op, seed: k as u64 + 1, ..Strand::new(off, frac) };
                strand_shapes(line, &st, opts, px, None, out);
            }
        }
        WetEdge => {
            let body = Strand { opacity: 0.2, grain: Some((5.0, 0.2)), seed: 1, ..Strand::new(0.0, 1.0) };
            strand_shapes(line, &body, opts, px, None, out);
            for (k, side) in [-0.94, 0.94].into_iter().enumerate() {
                let edge = Strand { opacity: 0.55, grain: Some((1.5, 0.5)), seed: k as u64 + 2, ..Strand::new(side, 0.07) };
                strand_shapes(line, &edge, opts, px, None, out);
            }
        }
        Bleed => {
            let core = Strand { opacity: 0.35, grain: Some((3.0, 0.3)), seed: 1, ..Strand::new(0.0, 0.7) };
            strand_shapes(line, &core, opts, px, None, out);
            scatter(line, opts, 0.5, 2, 1.6, (0.12, 0.3), (0.08, 0.2), out);
        }
        Spray => scatter(line, opts, 0.12, 4, 1.0, (0.025, 0.06), (0.5, 0.9), out),
        Stipple => scatter(line, opts, 0.3, 1, 0.9, (0.06, 0.13), (0.9, 1.0), out),
        Hatching => hatch(line, opts, px, &[-std::f64::consts::FRAC_PI_4 * 1.5], out),
        Crosshatch => hatch(line, opts, px, &[-std::f64::consts::FRAC_PI_4 * 1.5, -std::f64::consts::FRAC_PI_4 * 0.5], out),
        _ => {}
    }
}

/// A brush stroke along the drawn points: one shape for the simple
/// brushes, a group for the media ones.
pub fn brush_shape(pts: &[Point], times: &[f64], pressure: &[Option<f32>], px: f64, opts: &BrushOpts) -> Option<Shape> {
    use BrushStyle::*;
    let len = crate::freehand::path_length(pts);
    if len < px * 3.0 || pts.len() < 2 {
        return None;
    }
    let sp = crate::freehand::fit(pts, opts.smooth, px, false)?;
    let vary = match opts.style {
        Ink | DipPen | BrushPen | Splatter => opts.vary.max(0.6),
        _ => opts.vary,
    };
    let rel = widths_from(pts, times, pressure, vary, opts.source);
    let simple = matches!(opts.style, Round | Pointed | Marker | Highlighter | Calligraphy | FlatNib | Dots | Ink | DipPen | BrushPen | Charcoal);
    let mut s = if simple {
        single(sp, pts, &rel, px, opts)
    } else {
        let rel = if opts.style.varies() { rel } else { vec![1.0; pts.len()] };
        let line = Line::new(&sp, pts, &rel, (opts.width * 0.15).clamp(px * 1.5, px * 6.0))?;
        let mut kids = Vec::new();
        media(&line, opts, px, &mut kids);
        if kids.is_empty() {
            return None;
        }
        let mut g = Shape::new(ShapeKind::Group { children: kids });
        g.fill = None;
        g.stroke = None;
        g
    };
    s.name = format!("{} stroke", opts.style.label());
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_brush_draws_along_the_line() {
        let pts: Vec<Point> = (0..150).map(|i| Point::new(i as f64 * 2.0, (i as f64 * 0.05).sin() * 30.0)).collect();
        let times: Vec<f64> = (0..150).map(|i| i as f64 / 60.0).collect();
        let pressure: Vec<Option<f32>> = (0..150).map(|i| Some(0.3 + 0.7 * (i as f32 / 149.0))).collect();
        let mut fonts = crate::fonts::FontManager::new(egui::Context::default());
        let mut n = 0;
        for style in BrushStyle::all() {
            n += 1;
            let opts = BrushOpts { width: 16.0, style, ..Default::default() };
            let s = brush_shape(&pts, &times, &pressure, 1.0, &opts).unwrap_or_else(|| panic!("{style:?} drew nothing"));
            assert!(s.name.starts_with(style.label()));
            if let Some(kids) = s.kind.children() {
                assert!(!kids.is_empty() && kids.len() <= MAX_DOTS + 20, "{style:?}: {}", kids.len());
            }
            // The paint stays near the line: within the width plus splatter.
            let b = crate::layout::geo_bounds(&s, &mut fonts).unwrap();
            let reach = if style == BrushStyle::Splatter { 16.0 * 2.6 } else { 16.0 };
            assert!(b.x0 > -reach && b.x1 < 298.0 + reach && b.y0 > -30.0 - reach && b.y1 < 30.0 + reach, "{style:?} {b:?}");
            assert!(b.width() > 200.0, "{style:?} {b:?}");
            // The same input draws the same stroke (live redraws don't flicker).
            assert_eq!(brush_shape(&pts, &times, &pressure, 1.0, &opts), Some(s.clone()), "{style:?}");
        }
        assert!(n >= 28, "{n} brushes");
        // Dry media break up; bristles run out at different places.
        let opts = |style| BrushOpts { width: 16.0, style, ..Default::default() };
        let chalk = brush_shape(&pts, &times, &pressure, 1.0, &opts(BrushStyle::Chalk)).unwrap();
        assert!(chalk.kind.children().unwrap().len() > 9 * 2, "chalk pieces");
        let bristle = brush_shape(&pts, &times, &pressure, 1.0, &opts(BrushStyle::Bristle)).unwrap();
        assert!(bristle.kind.children().unwrap().len() >= 8);
    }
}
