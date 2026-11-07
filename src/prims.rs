//! Parametric primitives (Affinity-style shapes): triangle, diamond,
//! trapezoid, arrow, cross, donut, pie, crescent, heart, tear, callouts,
//! cloud, cog and spiral. Each lives in its local box (0,0)-(w,h) and keeps
//! up to four parameters that stay editable after placing: in the Design
//! panel and with on-canvas handles.

use kurbo::{Arc, BezPath, Point, Shape as _, Vec2};
use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrimKind {
    Triangle,
    Diamond,
    Trapezoid,
    Arrow,
    Cross,
    Donut,
    Pie,
    Crescent,
    Heart,
    Tear,
    Callout,
    CalloutEllipse,
    Cloud,
    Cog,
    Spiral,
    Qr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prim {
    pub kind: PrimKind,
    /// Parameters, meaning per kind (see `PrimKind::params`).
    pub p: [f64; 4],
    /// Text payload (QR code contents).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

/// One editable parameter.
pub struct Param {
    pub name: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// Shown ×100 with a % suffix.
    pub percent: bool,
    pub int: bool,
    /// Unit suffix; "bool" marks an on/off toggle.
    pub suffix: &'static str,
}

const fn pct(name: &'static str, min: f64, max: f64, default: f64) -> Param {
    Param { name, min, max, default, percent: true, int: false, suffix: "%" }
}
const fn int(name: &'static str, min: f64, max: f64, default: f64) -> Param {
    Param { name, min, max, default, percent: false, int: true, suffix: "" }
}
const fn deg(name: &'static str, min: f64, max: f64, default: f64) -> Param {
    Param { name, min, max, default, percent: false, int: false, suffix: "°" }
}
const fn num(name: &'static str, min: f64, max: f64, default: f64) -> Param {
    Param { name, min, max, default, percent: false, int: false, suffix: "" }
}
const fn toggle(name: &'static str, default: f64) -> Param {
    Param { name, min: 0.0, max: 1.0, default, percent: false, int: true, suffix: "bool" }
}

impl PrimKind {
    pub const ALL: [PrimKind; 16] = [
        PrimKind::Triangle,
        PrimKind::Diamond,
        PrimKind::Trapezoid,
        PrimKind::Arrow,
        PrimKind::Cross,
        PrimKind::Donut,
        PrimKind::Pie,
        PrimKind::Crescent,
        PrimKind::Heart,
        PrimKind::Tear,
        PrimKind::Callout,
        PrimKind::CalloutEllipse,
        PrimKind::Cloud,
        PrimKind::Cog,
        PrimKind::Spiral,
        PrimKind::Qr,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PrimKind::Triangle => "Triangle",
            PrimKind::Diamond => "Diamond",
            PrimKind::Trapezoid => "Trapezoid",
            PrimKind::Arrow => "Arrow",
            PrimKind::Cross => "Cross",
            PrimKind::Donut => "Donut",
            PrimKind::Pie => "Pie",
            PrimKind::Crescent => "Crescent",
            PrimKind::Heart => "Heart",
            PrimKind::Tear => "Tear",
            PrimKind::Callout => "Callout",
            PrimKind::CalloutEllipse => "Round Callout",
            PrimKind::Cloud => "Cloud",
            PrimKind::Cog => "Cog",
            PrimKind::Spiral => "Spiral",
            PrimKind::Qr => "QR Code",
        }
    }

    pub fn params(self) -> &'static [Param] {
        const TRIANGLE: &[Param] = &[pct("Apex", 0.0, 1.0, 0.5)];
        const DIAMOND: &[Param] = &[pct("Middle", 0.0, 1.0, 0.5)];
        const TRAPEZOID: &[Param] = &[pct("Inset", 0.0, 0.5, 0.25)];
        const ARROW: &[Param] = &[pct("Head", 0.05, 1.0, 0.4), pct("Shaft", 0.05, 1.0, 0.4), toggle("Double-headed", 0.0)];
        const CROSS: &[Param] = &[pct("Thickness", 0.05, 1.0, 0.34)];
        const DONUT: &[Param] = &[pct("Hole", 0.0, 0.98, 0.5)];
        const PIE: &[Param] = &[deg("Start", -360.0, 360.0, 0.0), deg("Sweep", 1.0, 360.0, 270.0), pct("Hole", 0.0, 0.98, 0.0)];
        const CRESCENT: &[Param] = &[pct("Offset", 0.0, 1.0, 0.35), pct("Size", 0.3, 1.2, 0.9)];
        const HEART: &[Param] = &[pct("Cleft", 0.0, 0.6, 0.28), pct("Round", 0.0, 1.0, 0.55)];
        const TEAR: &[Param] = &[pct("Sides", 0.0, 1.0, 0.6), pct("Bulb", 0.3, 0.9, 0.6), pct("Lean", -0.5, 0.5, 0.0)];
        const CALLOUT: &[Param] = &[pct("Tail", 0.0, 1.0, 0.3), pct("Length", 0.05, 0.6, 0.25), pct("Width", 0.03, 0.6, 0.15), pct("Corner", 0.0, 0.5, 0.15)];
        const CALLOUT_E: &[Param] = &[pct("Tail", 0.0, 1.0, 0.3), pct("Length", 0.05, 0.6, 0.25), pct("Width", 0.03, 0.6, 0.18)];
        const CLOUD: &[Param] = &[int("Bumps", 4.0, 24.0, 9.0), pct("Puff", 0.0, 1.0, 0.5)];
        const COG: &[Param] = &[int("Teeth", 3.0, 64.0, 12.0), pct("Depth", 0.02, 0.5, 0.18), pct("Width", 0.1, 0.9, 0.5), pct("Hole", 0.0, 0.9, 0.3)];
        const SPIRAL: &[Param] = &[num("Turns", 0.5, 12.0, 3.0), pct("Inner", 0.0, 0.9, 0.0)];
        const QR: &[Param] = &[Param { name: "Correction", min: 0.0, max: 3.0, default: 1.0, percent: false, int: true, suffix: "ecc" }, int("Margin", 0.0, 8.0, 1.0)];
        match self {
            PrimKind::Qr => QR,
            PrimKind::Triangle => TRIANGLE,
            PrimKind::Diamond => DIAMOND,
            PrimKind::Trapezoid => TRAPEZOID,
            PrimKind::Arrow => ARROW,
            PrimKind::Cross => CROSS,
            PrimKind::Donut => DONUT,
            PrimKind::Pie => PIE,
            PrimKind::Crescent => CRESCENT,
            PrimKind::Heart => HEART,
            PrimKind::Tear => TEAR,
            PrimKind::Callout => CALLOUT,
            PrimKind::CalloutEllipse => CALLOUT_E,
            PrimKind::Cloud => CLOUD,
            PrimKind::Cog => COG,
            PrimKind::Spiral => SPIRAL,
        }
    }

    pub fn default_prim(self) -> Prim {
        let mut p = [0.0; 4];
        for (i, s) in self.params().iter().enumerate() {
            p[i] = s.default;
        }
        let text = if self == PrimKind::Qr { "https://example.com".to_string() } else { String::new() };
        Prim { kind: self, p, text }
    }

    /// Open path: drawn as a line, no fill by default.
    pub fn is_open(self) -> bool {
        self == PrimKind::Spiral
    }

    /// Phosphor glyph for lists and headers.
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            PrimKind::Triangle => i::TRIANGLE,
            PrimKind::Diamond => i::DIAMOND,
            PrimKind::Trapezoid => i::PARALLELOGRAM,
            PrimKind::Arrow => i::ARROW_FAT_RIGHT,
            PrimKind::Cross => i::PLUS,
            PrimKind::Donut => i::CIRCLE_NOTCH,
            PrimKind::Pie => i::CHART_PIE,
            PrimKind::Crescent => i::MOON,
            PrimKind::Heart => i::HEART,
            PrimKind::Tear => i::DROP,
            PrimKind::Callout => i::CHAT,
            PrimKind::CalloutEllipse => i::CHAT_CIRCLE,
            PrimKind::Cloud => i::CLOUD,
            PrimKind::Cog => i::GEAR,
            PrimKind::Spiral => i::SPIRAL,
            PrimKind::Qr => i::QR_CODE,
        }
    }
}

impl Prim {
    /// Clamp every parameter into its range (ints rounded).
    pub fn clamped(mut self) -> Self {
        for (i, s) in self.kind.params().iter().enumerate() {
            let v = self.p[i].clamp(s.min, s.max);
            self.p[i] = if s.int { v.round() } else { v };
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

fn poly(pts: &[Point]) -> BezPath {
    let mut b = BezPath::new();
    if let Some((f, rest)) = pts.split_first() {
        b.move_to(*f);
        for p in rest {
            b.line_to(*p);
        }
        b.close_path();
    }
    b
}

/// Append an elliptical arc (centre c, radii r, from angle a0 sweeping s);
/// `b` must already be at the arc's start.
fn arc_to(b: &mut BezPath, c: Point, r: Vec2, a0: f64, s: f64) {
    Arc::new(c, r, a0, s, 0.0).to_cubic_beziers(0.01, |p1, p2, p| b.curve_to(p1, p2, p));
}

fn on_ellipse(c: Point, r: Vec2, a: f64) -> Point {
    Point::new(c.x + r.x * a.cos(), c.y + r.y * a.sin())
}

/// Full ellipse as a closed path.
fn ellipse(c: Point, r: Vec2) -> BezPath {
    let mut b = BezPath::new();
    b.move_to(on_ellipse(c, r, 0.0));
    arc_to(&mut b, c, r, 0.0, TAU);
    b.close_path();
    b
}

/// Append `hole` so that it cuts out of `outer` under the non-zero rule.
fn with_hole(mut outer: BezPath, hole: BezPath) -> BezPath {
    let hole = if outer.area().signum() == hole.area().signum() { hole.reverse_subpaths() } else { hole };
    outer.extend(hole.iter());
    outer
}

/// Tear construction in unit-box coordinates.
struct Tear {
    tip: Point,
    c: Point,
    r: Vec2,
    tr: Point,
    ar: f64,
    sweep: f64,
    ctrl_r: Point,
    ctrl_l: Point,
    /// Side curve midpoints and outward normals (for the Sides handle).
    mid_r: Point,
    n_r: Vec2,
}

fn tear_geom(p: &[f64; 4]) -> Tear {
    let (sides, bulb, lean) = (p[0], p[1], p[2]);
    let ry = bulb / 2.0;
    let c = Point::new(0.5, 1.0 - ry);
    let r = Vec2::new(0.5, ry);
    let tip = Point::new(0.5 + lean, 0.0);
    // Tangents from the tip in circle space (y scaled so the ellipse is round).
    let k = 0.5 / ry;
    let tc = Vec2::new(tip.x - c.x, (tip.y - c.y) * k);
    let d = tc.hypot().max(0.5001);
    let phi = tc.atan2();
    let alpha = (0.5 / d).acos();
    let (ar, al) = (phi + alpha, phi - alpha);
    let at = |a: f64| Point::new(c.x + 0.5 * a.cos(), c.y + 0.5 * a.sin() / k);
    let (tr, tl) = (at(ar), at(al));
    let sweep = (al - ar).rem_euclid(TAU);
    // Sides: bow out (0) … straight (≈0.6) … pinched in (1).
    let side = |a: Point, b: Point| {
        let m = a.midpoint(b);
        let mut n = Vec2::new(b.y - a.y, a.x - b.x).normalize();
        if (m - c).dot(n) < 0.0 {
            n = -n;
        }
        let len = (b - a).hypot();
        (m, n, m + n * ((0.6 - sides) * 0.35 * len))
    };
    let (mid_r, n_r, ctrl_r) = side(tip, tr);
    let (_, _, ctrl_l) = side(tl, tip);
    Tear { tip, c, r, tr, ar, sweep, ctrl_r, ctrl_l, mid_r, n_r }
}

/// QR code modules traced into outlines (holes wind the other way).
fn qr_path(prim: &Prim, w: f64, h: f64) -> BezPath {
    use qrcodegen::{QrCode, QrCodeEcc};
    let ecc = [QrCodeEcc::Low, QrCodeEcc::Medium, QrCodeEcc::Quartile, QrCodeEcc::High][prim.p[0].clamp(0.0, 3.0) as usize];
    let text = if prim.text.is_empty() { " " } else { prim.text.as_str() };
    let Ok(qr) = QrCode::encode_text(text, ecc) else { return BezPath::new() };
    let n = qr.size();
    let margin = prim.p[1].clamp(0.0, 8.0).round() as i32;
    let total = (n + 2 * margin) as f64;
    let on = |x: i32, y: i32| x >= 0 && y >= 0 && x < n && y < n && qr.get_module(x, y);
    // Directed boundary edges, filled cell on the right (clockwise on screen).
    let mut next: std::collections::HashMap<(i32, i32), Vec<(i32, i32)>> = Default::default();
    for y in 0..n {
        for x in 0..n {
            if !on(x, y) {
                continue;
            }
            if !on(x, y - 1) {
                next.entry((x, y)).or_default().push((x + 1, y));
            }
            if !on(x + 1, y) {
                next.entry((x + 1, y)).or_default().push((x + 1, y + 1));
            }
            if !on(x, y + 1) {
                next.entry((x + 1, y + 1)).or_default().push((x, y + 1));
            }
            if !on(x - 1, y) {
                next.entry((x, y + 1)).or_default().push((x, y));
            }
        }
    }
    let map = |v: (i32, i32)| Point::new((v.0 + margin) as f64 / total * w, (v.1 + margin) as f64 / total * h);
    let mut b = BezPath::new();
    let mut starts: Vec<(i32, i32)> = next.keys().copied().collect();
    starts.sort();
    for s0 in starts {
        while next.get(&s0).map(|v| !v.is_empty()).unwrap_or(false) {
            let mut loop_pts = vec![s0];
            let mut prev = s0;
            let mut cur = next.get_mut(&s0).unwrap().pop().unwrap();
            while cur != s0 {
                loop_pts.push(cur);
                let outs = next.get_mut(&cur).unwrap();
                // At a diagonal touch, turn right to keep loops simple.
                let din = (cur.0 - prev.0, cur.1 - prev.1);
                let right = (-din.1, din.0);
                let i = outs.iter().position(|o| (o.0 - cur.0, o.1 - cur.1) == right).unwrap_or(outs.len() - 1);
                prev = cur;
                cur = outs.swap_remove(i);
            }
            // Drop collinear vertices.
            let m = loop_pts.len();
            let corners: Vec<(i32, i32)> = (0..m)
                .filter(|&i| {
                    let (a, p, q) = (loop_pts[(i + m - 1) % m], loop_pts[i], loop_pts[(i + 1) % m]);
                    (p.0 - a.0) * (q.1 - p.1) - (p.1 - a.1) * (q.0 - p.0) != 0
                })
                .map(|i| loop_pts[i])
                .collect();
            if let Some((f, rest)) = corners.split_first() {
                b.move_to(map(*f));
                for v in rest {
                    b.line_to(map(*v));
                }
                b.close_path();
            }
        }
    }
    b
}

/// Scale a path so its bounds fill (0,0)-(w,h) exactly.
fn fit(path: BezPath, w: f64, h: f64) -> BezPath {
    let b = path.bounding_box();
    if b.width() < 1e-9 || b.height() < 1e-9 {
        return path;
    }
    let m = kurbo::Affine::new([w / b.width(), 0.0, 0.0, h / b.height(), 0.0, 0.0]) * kurbo::Affine::translate((-b.x0, -b.y0));
    m * path
}

pub fn path(prim: &Prim, w: f64, h: f64) -> BezPath {
    let p = prim.clone().clamped().p;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let c = Point::new(cx, cy);
    let r = Vec2::new(cx, cy);
    match prim.kind {
        PrimKind::Triangle => poly(&[Point::new(p[0] * w, 0.0), Point::new(w, h), Point::new(0.0, h)]),
        PrimKind::Diamond => poly(&[Point::new(cx, 0.0), Point::new(w, p[0] * h), Point::new(cx, h), Point::new(0.0, p[0] * h)]),
        PrimKind::Trapezoid => poly(&[Point::new(p[0] * w, 0.0), Point::new(w - p[0] * w, 0.0), Point::new(w, h), Point::new(0.0, h)]),
        PrimKind::Arrow => {
            let double = p[2] > 0.5;
            let head = p[0] * w * if double { 0.5 } else { 1.0 };
            let (t0, t1) = (cy - p[1] * h / 2.0, cy + p[1] * h / 2.0);
            if double {
                poly(&[
                    Point::new(0.0, cy),
                    Point::new(head, 0.0),
                    Point::new(head, t0),
                    Point::new(w - head, t0),
                    Point::new(w - head, 0.0),
                    Point::new(w, cy),
                    Point::new(w - head, h),
                    Point::new(w - head, t1),
                    Point::new(head, t1),
                    Point::new(head, h),
                ])
            } else {
                poly(&[
                    Point::new(0.0, t0),
                    Point::new(w - head, t0),
                    Point::new(w - head, 0.0),
                    Point::new(w, cy),
                    Point::new(w - head, h),
                    Point::new(w - head, t1),
                    Point::new(0.0, t1),
                ])
            }
        }
        PrimKind::Cross => {
            let (tx, ty) = (p[0] * w / 2.0, p[0] * h / 2.0);
            poly(&[
                Point::new(cx - tx, 0.0),
                Point::new(cx + tx, 0.0),
                Point::new(cx + tx, cy - ty),
                Point::new(w, cy - ty),
                Point::new(w, cy + ty),
                Point::new(cx + tx, cy + ty),
                Point::new(cx + tx, h),
                Point::new(cx - tx, h),
                Point::new(cx - tx, cy + ty),
                Point::new(0.0, cy + ty),
                Point::new(0.0, cy - ty),
                Point::new(cx - tx, cy - ty),
            ])
        }
        PrimKind::Donut => {
            let outer = ellipse(c, r);
            if p[0] <= 0.0 { outer } else { with_hole(outer, ellipse(c, r * p[0])) }
        }
        PrimKind::Pie => {
            let (a0, s) = (p[0].to_radians(), p[1].to_radians());
            let hole = p[2];
            if s >= TAU - 1e-6 {
                let outer = ellipse(c, r);
                return if hole > 0.0 { with_hole(outer, ellipse(c, r * hole)) } else { outer };
            }
            let mut b = BezPath::new();
            if hole > 0.0 {
                b.move_to(on_ellipse(c, r, a0));
                arc_to(&mut b, c, r, a0, s);
                b.line_to(on_ellipse(c, r * hole, a0 + s));
                arc_to(&mut b, c, r * hole, a0 + s, -s);
            } else {
                b.move_to(c);
                b.line_to(on_ellipse(c, r, a0));
                arc_to(&mut b, c, r, a0, s);
            }
            b.close_path();
            b
        }
        PrimKind::Crescent => {
            // Unit circle minus a circle of radius k offset to the right by d.
            let (d, k) = ((p[0] * 1.2).max(0.01), p[1]);
            let unit = |q: Point| Point::new(cx + q.x * cx, cy + q.y * cy);
            if d >= 1.0 + k {
                return ellipse(c, r);
            }
            if d <= (1.0 - k).abs() {
                // No intersection: the inner circle swallows the outer one or sits inside it.
                return if k >= 1.0 { BezPath::new() } else { with_hole(ellipse(c, r), ellipse(unit(Point::new(d, 0.0)), r * k)) };
            }
            // Intersections at x = (1 - k² + d²) / 2d, y = ±√(1 - x²).
            let x = ((1.0 - k * k + d * d) / (2.0 * d)).clamp(-1.0, 1.0);
            let y = (1.0 - x * x).max(0.0).sqrt();
            let a = x.acos();
            let ai = y.atan2(x - d);
            let mut b = BezPath::new();
            b.move_to(unit(Point::new(x, -y)));
            // Outer circle the long way round the left, to the lower intersection...
            Arc::new(Point::ZERO, Vec2::new(1.0, 1.0), -a, -(TAU - 2.0 * a), 0.0).to_cubic_beziers(0.01, |p1, p2, q| b.curve_to(unit(p1), unit(p2), unit(q)));
            // ...then back along the inner circle's left side.
            Arc::new(Point::new(d, 0.0), Vec2::new(k, k), ai, 2.0 * (PI - ai), 0.0).to_cubic_beziers(0.01, |p1, p2, q| b.curve_to(unit(p1), unit(p2), unit(q)));
            b.close_path();
            b
        }
        PrimKind::Heart => {
            // Two lobes meeting in a cleft at the top, a point at the bottom.
            let (cleft, full) = (p[0], p[1]);
            let top = Point::new(0.5, cleft * 0.5);
            let tip = Point::new(0.5, 1.0);
            let mut b = BezPath::new();
            b.move_to(top);
            b.curve_to(Point::new(0.62, -0.08), Point::new(1.05, -0.05), Point::new(1.0, 0.32));
            b.curve_to(Point::new(0.97, 0.55 + 0.1 * full), Point::new(0.62 + 0.1 * full, 0.8), tip);
            b.curve_to(Point::new(0.38 - 0.1 * full, 0.8), Point::new(0.03, 0.55 + 0.1 * full), Point::new(0.0, 0.32));
            b.curve_to(Point::new(-0.05, -0.05), Point::new(0.38, -0.08), top);
            b.close_path();
            fit(b, w, h)
        }
        PrimKind::Tear => {
            let t = tear_geom(&p);
            let u = |q: Point| Point::new(q.x * w, q.y * h);
            let mut b = BezPath::new();
            b.move_to(u(t.tip));
            b.quad_to(u(t.ctrl_r), u(t.tr));
            // Round part: the ellipse the long way round, through the bottom.
            Arc::new(t.c, t.r, t.ar, t.sweep, 0.0).to_cubic_beziers(0.002, |p1, p2, q| b.curve_to(u(p1), u(p2), u(q)));
            b.quad_to(u(t.ctrl_l), u(t.tip));
            b.close_path();
            b
        }
        PrimKind::Qr => qr_path(prim, w, h),
        PrimKind::Callout => {
            let (tx, tl, tw, cr) = (p[0] * w, p[1] * h, p[2] * w, p[3]);
            let bh = h - tl;
            let rad = cr * w.min(bh);
            let base_c = (tx + (cx - tx) * 0.35).clamp(rad + tw / 2.0, (w - rad - tw / 2.0).max(rad + tw / 2.0));
            let (b0, b1) = (base_c - tw / 2.0, base_c + tw / 2.0);
            let q = Vec2::new(rad, rad);
            let mut b = BezPath::new();
            b.move_to(Point::new(rad, 0.0));
            b.line_to(Point::new(w - rad, 0.0));
            arc_to(&mut b, Point::new(w - rad, rad), q, -FRAC_PI_2, FRAC_PI_2);
            b.line_to(Point::new(w, bh - rad));
            arc_to(&mut b, Point::new(w - rad, bh - rad), q, 0.0, FRAC_PI_2);
            b.line_to(Point::new(b1, bh));
            b.line_to(Point::new(tx, h));
            b.line_to(Point::new(b0, bh));
            b.line_to(Point::new(rad, bh));
            arc_to(&mut b, Point::new(rad, bh - rad), q, FRAC_PI_2, FRAC_PI_2);
            b.line_to(Point::new(0.0, rad));
            arc_to(&mut b, Point::new(rad, rad), q, PI, FRAC_PI_2);
            b.close_path();
            b
        }
        PrimKind::CalloutEllipse => {
            let (tx, tl, tw) = (p[0] * w, p[1] * h, p[2]);
            let bh = h - tl;
            let ec = Point::new(cx, bh / 2.0);
            let er = Vec2::new(cx, bh / 2.0);
            // Tail base: two angles around the ellipse point facing the tail tip.
            let dir = ((h - ec.y) / er.y.max(1e-9)).atan2((tx - ec.x) / er.x.max(1e-9));
            let half = tw * PI * 0.5;
            let (a0, a1) = (dir - half, dir + half);
            let mut b = BezPath::new();
            b.move_to(on_ellipse(ec, er, a1));
            arc_to(&mut b, ec, er, a1, TAU - (a1 - a0));
            b.line_to(Point::new(tx, h));
            b.close_path();
            b
        }
        PrimKind::Cloud => {
            // Union of circles on a ring: each bump is the outer arc of its
            // circle between the intersections with its neighbours. Puff
            // grows the circles (deeper notches, rounder bumps).
            let n = p[0].round().max(4.0) as usize;
            let cc = Point::new(0.0, 0.0);
            let centres: Vec<Point> = (0..n).map(|i| on_ellipse(cc, Vec2::new(1.0, 1.0), -FRAC_PI_2 + TAU * i as f64 / n as f64)).collect();
            let half = (PI / n as f64).sin(); // half the centre spacing
            let rho = half * (1.15 + 0.85 * p[1]);
            let hgt = (rho * rho - half * half).max(0.0).sqrt();
            let cross: Vec<Point> = (0..n)
                .map(|i| {
                    let (a, q) = (centres[i], centres[(i + 1) % n]);
                    let m = a.midpoint(q);
                    m + (m - cc).normalize() * hgt
                })
                .collect();
            let mut b = BezPath::new();
            b.move_to(cross[n - 1]);
            for i in 0..n {
                let ci = centres[i];
                let a0 = (cross[(i + n - 1) % n] - ci).atan2();
                let a1 = (cross[i] - ci).atan2();
                arc_to(&mut b, ci, Vec2::new(rho, rho), a0, (a1 - a0).rem_euclid(TAU));
            }
            b.close_path();
            fit(b, w, h)
        }
        PrimKind::Cog => {
            let n = p[0].round().max(3.0) as usize;
            let depth = p[1];
            let tooth = p[2];
            let hole = p[3];
            let (ro, ri) = (1.0, 1.0 - depth);
            let unit = |rad: f64, a: f64| Point::new(cx + rad * a.cos() * cx, cy + rad * a.sin() * cy);
            let step = TAU / n as f64;
            let mut pts = Vec::with_capacity(n * 4);
            for i in 0..n {
                let a = -FRAC_PI_2 + step * i as f64;
                let t = step * tooth * 0.5;
                // Slight taper: the tooth tip is narrower than its root.
                pts.push(unit(ri, a - t * 1.15));
                pts.push(unit(ro, a - t * 0.75));
                pts.push(unit(ro, a + t * 0.75));
                pts.push(unit(ri, a + t * 1.15));
            }
            let outer = poly(&pts);
            if hole > 0.0 { with_hole(outer, ellipse(c, r * hole)) } else { outer }
        }
        PrimKind::Spiral => {
            // Archimedean spiral from the inner radius out to the box edge,
            // one cubic per quarter turn.
            let turns = p[0].max(0.1);
            let r0 = p[1];
            let total = turns * TAU;
            let rad = |t: f64| r0 + (1.0 - r0) * t / total;
            let dr = (1.0 - r0) / total;
            let at = |t: f64| Point::new(cx + rad(t) * t.cos() * cx, cy + rad(t) * t.sin() * cy);
            let d_at = |t: f64| Vec2::new((dr * t.cos() - rad(t) * t.sin()) * cx, (dr * t.sin() + rad(t) * t.cos()) * cy);
            let steps = (turns * 4.0).ceil().max(1.0) as usize;
            let dt = total / steps as f64;
            let mut b = BezPath::new();
            b.move_to(at(0.0));
            for i in 0..steps {
                let (t0, t1) = (dt * i as f64, dt * (i + 1) as f64);
                b.curve_to(at(t0) + d_at(t0) * (dt / 3.0), at(t1) - d_at(t1) * (dt / 3.0), at(t1));
            }
            b
        }
    }
}

// ---------------------------------------------------------------------------
// On-canvas handles
// ---------------------------------------------------------------------------

/// Point on the box ellipse at normalized radius `rad` and angle `a`.
fn radial(w: f64, h: f64, rad: f64, a: f64) -> Point {
    Point::new(w / 2.0 + rad * a.cos() * w / 2.0, h / 2.0 + rad * a.sin() * h / 2.0)
}

/// Normalized (radius, angle) of a local point.
fn polar(q: Point, w: f64, h: f64) -> (f64, f64) {
    let (x, y) = ((q.x - w / 2.0) / (w / 2.0).max(1e-9), (q.y - h / 2.0) / (h / 2.0).max(1e-9));
    ((x * x + y * y).sqrt(), y.atan2(x))
}

/// Angle from the top (12 o'clock), clockwise, in 0..2π.
fn from_top(a: f64) -> f64 {
    (a + FRAC_PI_2).rem_euclid(TAU)
}

/// Count from a handle placed on the second vertex: n = 2π / angle.
fn count_from(a: f64, min: f64, max: f64) -> f64 {
    let d = from_top(a).max(1e-3);
    (TAU / d).round().clamp(min, max)
}

/// Handle positions (local box coordinates). `m` is a small inset in local
/// units (about 14 screen px) that keeps handles off the box's edge handles.
pub fn handles(prim: &Prim, w: f64, h: f64, m: f64) -> Vec<Point> {
    let pr = prim.clone().clamped();
    let p = pr.p;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let c = Point::new(cx, cy);
    let r = Vec2::new(cx, cy);
    match prim.kind {
        PrimKind::Triangle => vec![Point::new(p[0] * w, m)],
        PrimKind::Diamond => vec![Point::new(m, p[0] * h)],
        PrimKind::Trapezoid => vec![Point::new(p[0] * w, m)],
        PrimKind::Arrow => {
            let head = p[0] * w * if p[2] > 0.5 { 0.5 } else { 1.0 };
            vec![Point::new(w - head, cy - p[1] * h / 2.0)]
        }
        PrimKind::Cross => vec![Point::new(cx + p[0] * w / 2.0, m)],
        PrimKind::Donut => vec![Point::new(cx + p[0] * cx, cy)],
        PrimKind::Pie => {
            let (a0, s) = (p[0].to_radians(), p[1].to_radians());
            vec![on_ellipse(c, r, a0), on_ellipse(c, r, a0 + s), on_ellipse(c, r * p[2].max(0.0), a0 + s / 2.0)]
        }
        PrimKind::Crescent => vec![Point::new(cx + p[0] * 1.2 * cx, cy)],
        PrimKind::Heart => vec![Point::new(cx, p[0] * 0.5 * h)],
        PrimKind::Tear => {
            let t = tear_geom(&p);
            let u = |q: Point| Point::new(q.x * w, q.y * h);
            let side = t.mid_r + t.n_r * ((0.6 - p[0]) * 0.35 * (t.tr - t.tip).hypot() / 2.0);
            vec![u(side), Point::new(w, (1.0 - p[1] / 2.0) * h), Point::new(t.tip.x * w, m)]
        }
        PrimKind::Callout | PrimKind::CalloutEllipse => vec![Point::new(p[0] * w, h)],
        PrimKind::Cloud => vec![Point::new(cx, p[1] * 0.25 * h)],
        PrimKind::Cog => {
            let n = p[0].max(3.0);
            let step = TAU / n;
            let t = step * p[2] * 0.5;
            vec![
                radial(w, h, 1.0 - p[1], -FRAC_PI_2),
                // Middle of the first tooth's right flank.
                radial(w, h, 1.0 - p[1] / 2.0, -FRAC_PI_2 + t * 0.95),
                radial(w, h, 1.0, -FRAC_PI_2 + step),
                radial(w, h, p[3], 0.0),
            ]
        }
        PrimKind::Spiral => {
            let total = p[0] * TAU;
            vec![Point::new(cx + p[1] * cx, cy), radial(w, h, 1.0, total)]
        }
        PrimKind::Qr => vec![],
    }
}

/// Move handle `i` to local point `q`, updating the parameters.
pub fn drag_handle(prim: &mut Prim, i: usize, q: Point, w: f64, h: f64, m: f64) {
    let (w, h) = (w.max(1e-9), h.max(1e-9));
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (rad, ang) = polar(q, w, h);
    let p = &mut prim.p;
    match prim.kind {
        PrimKind::Triangle => p[0] = q.x / w,
        PrimKind::Diamond => p[0] = q.y / h,
        PrimKind::Trapezoid => p[0] = q.x / w,
        PrimKind::Arrow => {
            let k = if p[2] > 0.5 { 0.5 } else { 1.0 };
            p[0] = (w - q.x) / (w * k);
            p[1] = (cy - q.y) * 2.0 / h;
        }
        PrimKind::Cross => p[0] = (q.x - cx) * 2.0 / w,
        PrimKind::Donut => p[0] = (q.x - cx) / cx,
        PrimKind::Pie => match i {
            0 => {
                // Move the start, keep the end.
                let end = p[0] + p[1];
                let a = ang.to_degrees();
                p[0] = a;
                p[1] = (end - a).rem_euclid(360.0).max(1.0);
            }
            1 => p[1] = (ang.to_degrees() - p[0]).rem_euclid(360.0).max(1.0),
            _ => p[2] = rad,
        },
        PrimKind::Crescent => p[0] = (q.x - cx) / cx / 1.2,
        PrimKind::Heart => p[0] = q.y / h * 2.0,
        PrimKind::Tear => match i {
            0 => {
                let t = tear_geom(p);
                let u = Point::new(q.x / w, q.y / h);
                let len = (t.tr - t.tip).hypot().max(1e-9);
                p[0] = 0.6 - 2.0 * (u - t.mid_r).dot(t.n_r) / (0.35 * len);
            }
            1 => p[1] = 2.0 * (1.0 - q.y / h),
            _ => p[2] = q.x / w - 0.5,
        },
        PrimKind::Callout | PrimKind::CalloutEllipse => p[0] = q.x / w,
        PrimKind::Cloud => p[1] = q.y / h / 0.25,
        PrimKind::Cog => match i {
            0 => p[1] = 1.0 - (cy - q.y) / cy,
            1 => {
                let step = TAU / p[0].max(3.0);
                let a = (ang + FRAC_PI_2 + PI).rem_euclid(TAU) - PI;
                p[2] = a / (0.95 * 0.5 * step);
            }
            2 => p[0] = count_from(ang, 3.0, 64.0),
            _ => p[3] = rad,
        },
        PrimKind::Spiral => match i {
            0 => p[1] = (q.x - cx) / cx,
            _ => {
                // Continue from the current end so turns can pass whole circles.
                let total = p[0] * TAU;
                let d = (ang - total + PI).rem_euclid(TAU) - PI;
                p[0] = (total + d) / TAU;
            }
        },
        PrimKind::Qr => {}
    }
    let _ = m;
    *prim = prim.clone().clamped();
}

// ---------------------------------------------------------------------------
// Star and polygon handles (same scheme; those kinds live in ShapeKind)
// ---------------------------------------------------------------------------

/// Star: [inner, points, corner]; polygon: [sides, corner].
pub fn radial_handles(points: u32, inner: Option<f64>, corner: f64, w: f64, h: f64, m: f64) -> Vec<Point> {
    let n = points.max(3) as f64;
    let mut v = Vec::new();
    if let Some(inner) = inner {
        v.push(radial(w, h, inner, -FRAC_PI_2 + PI / n));
    }
    v.push(radial(w, h, 1.0, -FRAC_PI_2 + TAU / n));
    v.push(Point::new(w / 2.0, m + corner));
    v
}

/// Returns the new (points, inner, corner).
pub fn radial_drag(i: usize, q: Point, points: u32, inner: Option<f64>, corner: f64, w: f64, h: f64, m: f64) -> (u32, Option<f64>, f64) {
    let (rad, ang) = polar(q, w.max(1e-9), h.max(1e-9));
    let i = if inner.is_some() { i } else { i + 1 };
    match i {
        0 => (points, Some(rad.clamp(0.05, 1.0)), corner),
        1 => (count_from(ang, 3.0, 64.0) as u32, inner, corner),
        _ => (points, inner, (q.y - m).max(0.0)),
    }
}
