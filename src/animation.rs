//! Keyframe animation.
//!
//! The document always holds its state at the playhead (`Timeline::time`):
//! canvas tools, undo and saving work on it unchanged. A shape's transform
//! track stores its motion relative to a *rest* state that is never stored:
//! rest = (motion at the playhead)⁻¹ · current geometry. Moving the playhead
//! applies `motion(new) · motion(old)⁻¹`; paths and groups bake transforms,
//! so this works for every kind of shape. Motion turns and scales around
//! the centroid of the shape's points, which follows the shape exactly.
//!
//! Edits are picked up after the fact by comparing the document with the
//! state at the last sync: an affine change of an animated shape (or of any
//! shape while recording) becomes a key at the playhead.

use std::collections::HashMap;

use kurbo::{Affine, Point, Vec2};
use serde::{Deserialize, Serialize};

use crate::document::{BlendMode, Document, Gradient, Paint, Rgba, Shadow, Shape, ShapeKind};

/// Length, frame rate and playhead of a document's animation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timeline {
    /// Seconds.
    pub duration: f64,
    pub fps: f64,
    /// Playhead, seconds: the document shows this moment.
    pub time: f64,
    /// In and out points: the part that plays and exports (None: all of it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<(f64, f64)>,
    /// Playback (and animated SVG) starts over at the end.
    pub looping: bool,
}

impl Default for Timeline {
    fn default() -> Self {
        Timeline { duration: 3.0, fps: 30.0, time: 0.0, range: None, looping: true }
    }
}

impl Timeline {
    /// Where playback and export start and end.
    pub fn span(&self) -> (f64, f64) {
        match self.range {
            Some((a, b)) => (a.clamp(0.0, self.duration), b.clamp(0.0, self.duration).max(a.clamp(0.0, self.duration))),
            None => (0.0, self.duration),
        }
    }

    /// Frames between the in and out points.
    pub fn span_frames(&self) -> usize {
        let (a, b) = self.span();
        (((b - a) * self.fps).round() as usize).max(1)
    }

    /// `t` on the nearest frame, within the timeline.
    pub fn snap(&self, t: f64) -> f64 {
        ((t * self.fps).round() / self.fps).clamp(0.0, self.duration)
    }
}

/// An animatable property.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Prop {
    /// Position, rotation, scale and skew.
    Transform,
    /// Geometry: path points, corner radii, star and polygon settings…
    Shape,
    Opacity,
    /// Fill colour or gradient.
    Fill,
    StrokeColor,
    StrokeWidth,
    /// Dash and gap lengths.
    Dash,
    DashOffset,
    Shadow,
    InnerShadow,
    Blur,
    Noise,
    Visible,
    Blend,
    /// Text: the characters (typed on and off between keys).
    Text,
    FontSize,
    FontWeight,
    LetterSpacing,
    LineHeight,
    /// Text on a path: how far along it starts.
    PathOffset,
    /// Live blends: how many shapes between the ends.
    BlendSteps,
    /// The outline of a clip mask (document space).
    Clip,
}

impl Prop {
    pub const ALL: [Prop; 22] = [
        Prop::Transform,
        Prop::Shape,
        Prop::Opacity,
        Prop::Fill,
        Prop::StrokeColor,
        Prop::StrokeWidth,
        Prop::Dash,
        Prop::DashOffset,
        Prop::Shadow,
        Prop::InnerShadow,
        Prop::Blur,
        Prop::Noise,
        Prop::Visible,
        Prop::Blend,
        Prop::Text,
        Prop::FontSize,
        Prop::FontWeight,
        Prop::LetterSpacing,
        Prop::LineHeight,
        Prop::PathOffset,
        Prop::BlendSteps,
        Prop::Clip,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Prop::Transform => "Transform",
            Prop::Shape => "Shape",
            Prop::Opacity => "Opacity",
            Prop::Fill => "Fill",
            Prop::StrokeColor => "Stroke colour",
            Prop::StrokeWidth => "Stroke width",
            Prop::Dash => "Dashes",
            Prop::DashOffset => "Dash offset",
            Prop::Shadow => "Shadow",
            Prop::InnerShadow => "Inner shadow",
            Prop::Blur => "Blur",
            Prop::Noise => "Noise",
            Prop::Visible => "Visible",
            Prop::Blend => "Blend mode",
            Prop::Text => "Text",
            Prop::FontSize => "Font size",
            Prop::FontWeight => "Font weight",
            Prop::LetterSpacing => "Letter spacing",
            Prop::LineHeight => "Line height",
            Prop::PathOffset => "Path offset",
            Prop::BlendSteps => "Blend steps",
            Prop::Clip => "Clip mask",
        }
    }

    /// The shape has this property (a solid fill, a stroke…).
    pub fn applies(self, s: &Shape) -> bool {
        read(s, self).is_some()
    }

    /// Listed in the timeline for a selected layer before it has keys; the
    /// rest show up once keyed (K over their input, or recording).
    pub fn primary(self) -> bool {
        matches!(self, Prop::Transform | Prop::Shape | Prop::Opacity | Prop::Fill | Prop::StrokeColor | Prop::StrokeWidth | Prop::Text | Prop::FontSize | Prop::BlendSteps)
    }

    /// Changes in steps (no blending between keys).
    pub fn discrete(self) -> bool {
        matches!(self, Prop::Visible | Prop::Blend)
    }
}


/// How a key eases into the next one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Ease {
    Linear,
    #[default]
    EaseInOut,
    EaseIn,
    EaseOut,
    /// Keep this value until the next key.
    Hold,
    /// CSS-style cubic Bézier: handles (x1, y1) and (x2, y2) of a curve
    /// from (0, 0) to (1, 1). The y values may leave 0..1 (overshoot).
    Bezier([f64; 4]),
    /// A smooth curve through the neighbouring keys (monotone Catmull-Rom):
    /// no stop at keys in the middle of a motion, no overshoot.
    Smooth,
}

impl Ease {
    /// Ready-made curves offered next to the editor.
    pub const PRESETS: [(Ease, &'static str); 10] = [
        (Ease::Linear, "Linear"),
        (Ease::EaseInOut, "Ease in and out"),
        (Ease::EaseIn, "Ease in"),
        (Ease::EaseOut, "Ease out"),
        (Ease::Bezier([0.25, 0.1, 0.25, 1.0]), "Ease (CSS)"),
        (Ease::Bezier([0.36, 0.0, 0.66, -0.56]), "Back in (anticipate)"),
        (Ease::Bezier([0.34, 1.56, 0.64, 1.0]), "Back out (overshoot)"),
        (Ease::Bezier([0.68, -0.5, 0.32, 1.5]), "Back in and out"),
        (Ease::Hold, "Hold"),
        (Ease::Smooth, "Smooth through keys"),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Ease::Linear => "Linear",
            Ease::EaseInOut => "Ease in and out",
            Ease::EaseIn => "Ease in",
            Ease::EaseOut => "Ease out",
            Ease::Hold => "Hold",
            Ease::Smooth => "Smooth through keys",
            Ease::Bezier(_) => Ease::PRESETS.iter().find(|(e, _)| *e == self).map_or("Custom curve", |p| p.1),
        }
    }

    pub fn apply(self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        match self {
            // Smooth needs the neighbouring keys (see Track::value_at).
            Ease::Linear | Ease::Smooth => u,
            Ease::EaseInOut => u * u * (3.0 - 2.0 * u),
            Ease::EaseIn => u * u * u,
            Ease::EaseOut => 1.0 - (1.0 - u).powi(3),
            Ease::Hold => 0.0,
            Ease::Bezier([x1, y1, x2, y2]) => cubic_bezier(x1.clamp(0.0, 1.0), y1, x2.clamp(0.0, 1.0), y2, u),
        }
    }

    /// The curve's handles, as a Bézier close to this ease (where to start
    /// editing it from).
    pub fn handles(self) -> [f64; 4] {
        match self {
            Ease::Linear | Ease::Hold | Ease::Smooth => [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
            // Smoothstep is exactly this cubic.
            Ease::EaseInOut => [1.0 / 3.0, 0.0, 2.0 / 3.0, 1.0],
            Ease::EaseIn => [1.0 / 3.0, 0.0, 2.0 / 3.0, 0.0],
            Ease::EaseOut => [1.0 / 3.0, 1.0, 2.0 / 3.0, 1.0],
            Ease::Bezier(h) => h,
        }
    }
}

/// y at x = `u` on the cubic Bézier (0,0), (x1,y1), (x2,y2), (1,1).
fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, u: f64) -> f64 {
    let b = |a: f64, c: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * s * a + 3.0 * m * s * s * c + s * s * s
    };
    // x(s) is monotonic for x1, x2 in 0..1: bisect.
    let (mut lo, mut hi) = (0.0, 1.0);
    let mut s = u;
    for _ in 0..30 {
        let x = b(x1, x2, s);
        if (x - u).abs() < 1e-9 {
            break;
        }
        if x < u {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    b(y1, y2, s)
}

/// Motion relative to the rest state: offset of the pivot, then rotation
/// (radians), scale and skew about it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Xf {
    pub dx: f64,
    pub dy: f64,
    pub rot: f64,
    pub sx: f64,
    pub sy: f64,
    pub skew: f64,
}

impl Default for Xf {
    fn default() -> Self {
        Xf { dx: 0.0, dy: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0 }
    }
}

impl Xf {
    /// `T(pivot + d) · R(rot) · [[sx, skew], [0, sy]] · T(-pivot)`.
    pub fn affine(&self, pivot: Point) -> Affine {
        let (sin, cos) = self.rot.sin_cos();
        let nz = |v: f64| if v.abs() < 1e-4 { 1e-4f64.copysign(v) } else { v };
        let (sx, sy) = (nz(self.sx), nz(self.sy));
        let lin = Affine::new([sx * cos, sx * sin, self.skew * cos - sy * sin, self.skew * sin + sy * cos, 0.0, 0.0]);
        Affine::translate(pivot.to_vec2() + Vec2::new(self.dx, self.dy)) * lin * Affine::translate(-pivot.to_vec2())
    }

    /// Inverse of [`Xf::affine`] for `pivot`; the angle is taken nearest `near`.
    pub fn from_affine(a: Affine, pivot: Point, near: f64) -> Xf {
        let [a0, b0, c0, d0, ..] = a.as_coeffs();
        let sx = a0.hypot(b0);
        let rot = b0.atan2(a0);
        let (sin, cos) = rot.sin_cos();
        let skew = cos * c0 + sin * d0;
        let sy = -sin * c0 + cos * d0;
        let d = a * pivot - pivot;
        let tau = std::f64::consts::TAU;
        let rot = near + (rot - near + std::f64::consts::PI).rem_euclid(tau) - std::f64::consts::PI;
        Xf { dx: d.x, dy: d.y, rot, sx, sy, skew }
    }

    /// The rotation, scale and skew alone.
    pub fn linear(&self) -> Affine {
        let mut a = self.affine(Point::ZERO).as_coeffs();
        (a[4], a[5]) = (0.0, 0.0);
        Affine::new(a)
    }

    fn lerp(&self, o: &Xf, u: f64) -> Xf {
        let l = |a: f64, b: f64| a + (b - a) * u;
        Xf { dx: l(self.dx, o.dx), dy: l(self.dy, o.dy), rot: l(self.rot, o.rot), sx: l(self.sx, o.sx), sy: l(self.sy, o.sy), skew: l(self.skew, o.skew) }
    }

    fn close(&self, o: &Xf) -> bool {
        let e = 1e-7;
        (self.dx - o.dx).abs() < 1e-5 && (self.dy - o.dy).abs() < 1e-5 && (self.rot - o.rot).abs() < e && (self.sx - o.sx).abs() < e && (self.sy - o.sy).abs() < e && (self.skew - o.skew).abs() < e
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Xf(Xf),
    Num(f64),
    Color(Rgba),
    /// Geometry. Paths are in their rest frame (without the motion); for
    /// other kinds the size comes from the transform, not from here.
    Shape(Box<ShapeKind>),
    /// Numbers blended one by one (a shadow, a dash pattern); lists of
    /// different lengths switch at the next key.
    List(Vec<f64>),
    /// On or off: switches at the next key.
    Flag(bool),
    /// Text: deleted back to what the two keys share, then typed on.
    Text(String),
    /// A gradient fill (solid fills are `Color`).
    Paint(Box<Paint>),
}

impl Value {
    fn lerp(&self, o: &Value, u: f64) -> Value {
        match (self, o) {
            (Value::Xf(a), Value::Xf(b)) => Value::Xf(a.lerp(b, u)),
            (Value::Num(a), Value::Num(b)) => Value::Num(a + (b - a) * u),
            (Value::Color(a), Value::Color(b)) => Value::Color(a.lerp(*b, u as f32)),
            (Value::Shape(a), Value::Shape(b)) => match lerp_kind(a, b, u) {
                Some(k) => Value::Shape(Box::new(k)),
                // Can't blend (different structure): switch at the next key.
                None if u >= 1.0 => o.clone(),
                None => self.clone(),
            },
            (Value::List(a), Value::List(b)) if a.len() == b.len() => Value::List(a.iter().zip(b).map(|(x, y)| x + (y - x) * u).collect()),
            (Value::Text(a), Value::Text(b)) => Value::Text(type_on(a, b, u)),
            (Value::Paint(_) | Value::Color(_), Value::Paint(_) | Value::Color(_)) if u > 0.0 && u < 1.0 => match lerp_paint(&self.paint(), &o.paint(), u) {
                Some(p) => Value::Paint(Box::new(p)),
                None => self.clone(),
            },
            _ if u >= 1.0 => o.clone(),
            _ => self.clone(),
        }
    }

    fn paint(&self) -> Paint {
        match self {
            Value::Paint(p) => (**p).clone(),
            Value::Color(c) => Paint::Solid(*c),
            _ => Paint::Solid(Rgba::BLACK),
        }
    }

    fn close(&self, o: &Value) -> bool {
        match (self, o) {
            (Value::Xf(a), Value::Xf(b)) => a.close(b),
            (Value::Num(a), Value::Num(b)) => (a - b).abs() < 1e-6,
            (Value::Color(a), Value::Color(b)) => (a.r - b.r).abs() < 1e-4 && (a.g - b.g).abs() < 1e-4 && (a.b - b.b).abs() < 1e-4 && (a.a - b.a).abs() < 1e-4,
            (Value::Shape(a), Value::Shape(b)) => kind_close(a, b),
            (Value::List(a), Value::List(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6),
            (Value::Flag(a), Value::Flag(b)) => a == b,
            (Value::Text(a), Value::Text(b)) => a == b,
            (Value::Paint(a), Value::Paint(b)) => a == b,
            _ => false,
        }
    }
}

/// `a` turning into `b` at `u`: deleted back to their common start, then
/// typed on, one character per step.
fn type_on(a: &str, b: &str, u: f64) -> String {
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let (na, nb) = (a.chars().count(), b.chars().count());
    let (del, add) = (na - common, nb - common);
    let k = (u.clamp(0.0, 1.0) * (del + add) as f64).round() as usize;
    if k <= del {
        a.chars().take(na - k).collect()
    } else {
        b.chars().take(common + k - del).collect()
    }
}

/// Blend fills: gradients of the same kind and stop count stop by stop; a
/// solid colour blends as a gradient of that colour.
fn lerp_paint(a: &Paint, b: &Paint, u: f64) -> Option<Paint> {
    let (ga, gb) = match (a, b) {
        (Paint::Gradient(ga), Paint::Gradient(gb)) => (ga.clone(), gb.clone()),
        (Paint::Solid(c), Paint::Gradient(g)) => (flat(g, *c), g.clone()),
        (Paint::Gradient(g), Paint::Solid(c)) => (g.clone(), flat(g, *c)),
        (Paint::Solid(x), Paint::Solid(y)) => return Some(Paint::Solid(x.lerp(*y, u as f32))),
        _ => return None,
    };
    if ga.kind != gb.kind || ga.stops.len() != gb.stops.len() || ga.mesh.is_some() || gb.mesh.is_some() {
        return None;
    }
    let l = |x: f64, y: f64| x + (y - x) * u;
    let lf = |x: f32, y: f32| x + (y - x) * u as f32;
    let mut g = ga.clone();
    for (s, t) in g.stops.iter_mut().zip(&gb.stops) {
        s.offset = lf(s.offset, t.offset);
        s.color = s.color.lerp(t.color, u as f32);
        s.mid = lf(s.mid, t.mid);
    }
    g.start = [l(ga.start[0], gb.start[0]), l(ga.start[1], gb.start[1])];
    g.end = [l(ga.end[0], gb.end[0]), l(ga.end[1], gb.end[1])];
    Some(Paint::Gradient(g))
}

/// `g` with every stop `c`.
fn flat(g: &Gradient, c: Rgba) -> Gradient {
    let mut g = g.clone();
    for s in &mut g.stops {
        s.color = c;
    }
    g
}

fn shadow_list(s: &Shadow) -> Value {
    let c = s.color;
    Value::List(vec![c.r as f64, c.g as f64, c.b as f64, c.a as f64, s.dx, s.dy, s.blur])
}

fn list_shadow(v: &[f64]) -> Option<Shadow> {
    let &[r, g, b, a, dx, dy, blur] = v else { return None };
    let c = |x: f64| x.clamp(0.0, 1.0) as f32;
    let mut color = Rgba::BLACK;
    (color.r, color.g, color.b, color.a) = (c(r), c(g), c(b), c(a));
    Some(Shadow { color, dx, dy, blur: blur.max(0.0) })
}

fn lerp_pt(a: Point, b: Point, u: f64) -> Point {
    a.lerp(b, u)
}

/// Blend two geometries of the same structure.
fn lerp_kind(a: &ShapeKind, b: &ShapeKind, u: f64) -> Option<ShapeKind> {
    let l = |x: f64, y: f64| x + (y - x) * u;
    let li = |x: u32, y: u32| l(x as f64, y as f64).round().max(0.0) as u32;
    Some(match (a, b) {
        (ShapeKind::Path(pa), ShapeKind::Path(pb)) => {
            if pa.subpaths.len() != pb.subpaths.len() || pa.subpaths.iter().zip(&pb.subpaths).any(|(x, y)| x.nodes.len() != y.nodes.len()) {
                return None;
            }
            let mut out = pa.clone();
            for (sp, sb) in out.subpaths.iter_mut().zip(&pb.subpaths) {
                for (n, nb) in sp.nodes.iter_mut().zip(&sb.nodes) {
                    // A missing handle sits on its node.
                    let h = |ha: Option<Point>, hb: Option<Point>, pa: Point, pb: Point| (ha.is_some() || hb.is_some()).then(|| lerp_pt(ha.unwrap_or(pa), hb.unwrap_or(pb), u));
                    let p0 = n.p;
                    n.hin = h(n.hin, nb.hin, p0, nb.p);
                    n.hout = h(n.hout, nb.hout, p0, nb.p);
                    n.p = lerp_pt(p0, nb.p, u);
                    n.corner = l(n.corner, nb.corner);
                }
            }
            ShapeKind::Path(out)
        }
        (ShapeKind::Rect { w, h, radii }, ShapeKind::Rect { radii: rb, .. }) => ShapeKind::Rect { w: *w, h: *h, radii: std::array::from_fn(|i| l(radii[i], rb[i])) },
        (ShapeKind::Star { w, h, points, inner, corner }, ShapeKind::Star { points: pb, inner: ib, corner: cb, .. }) => ShapeKind::Star { w: *w, h: *h, points: li(*points, *pb).max(3), inner: l(*inner, *ib), corner: l(*corner, *cb) },
        (ShapeKind::Polygon { w, h, sides, corner }, ShapeKind::Polygon { sides: sb, corner: cb, .. }) => ShapeKind::Polygon { w: *w, h: *h, sides: li(*sides, *sb).max(3), corner: l(*corner, *cb) },
        (ShapeKind::Prim { w, h, prim }, ShapeKind::Prim { prim: pb, .. }) if prim.kind == pb.kind => {
            let mut p = prim.clone();
            p.p = std::array::from_fn(|i| l(prim.p[i], pb.p[i]));
            ShapeKind::Prim { w: *w, h: *h, prim: p }
        }
        _ => return None,
    })
}

fn kind_close(a: &ShapeKind, b: &ShapeKind) -> bool {
    match (a, b) {
        (ShapeKind::Path(pa), ShapeKind::Path(pb)) => {
            let near = |x: Option<Point>, y: Option<Point>| match (x, y) {
                (Some(x), Some(y)) => (x - y).hypot() < 1e-6,
                (None, None) => true,
                _ => false,
            };
            pa.subpaths.len() == pb.subpaths.len()
                && pa.subpaths.iter().zip(&pb.subpaths).all(|(x, y)| {
                    x.closed == y.closed && x.nodes.len() == y.nodes.len() && x.nodes.iter().zip(&y.nodes).all(|(n, m)| near(Some(n.p), Some(m.p)) && near(n.hin, m.hin) && near(n.hout, m.hout) && (n.corner - m.corner).abs() < 1e-9)
                })
        }
        _ => geometry_only(a) == geometry_only(b),
    }
}

/// The kind without its size (that belongs to the transform).
fn geometry_only(k: &ShapeKind) -> ShapeKind {
    let mut k = k.clone();
    if k.size().is_some() {
        k.set_size(1.0, 1.0);
    }
    k
}

/// The shape's geometry as a key value, in the frame `frame` (its motion:
/// paths are stored without it).
pub fn rest_shape(s: &Shape, frame: Affine) -> Option<Value> {
    match &s.kind {
        ShapeKind::Path(pd) => {
            let mut pd = pd.clone();
            pd.transform(frame.inverse());
            Some(Value::Shape(Box::new(ShapeKind::Path(pd))))
        }
        ShapeKind::Rect { .. } | ShapeKind::Star { .. } | ShapeKind::Polygon { .. } | ShapeKind::Prim { .. } => Some(Value::Shape(Box::new(s.kind.clone()))),
        _ => None,
    }
}

/// Give `s` the geometry `k`: a path in the frame of `o` (its animated
/// ancestors) and its motion `x`; other kinds keep their size.
fn write_shape(s: &mut Shape, k: &ShapeKind, o: Affine, x: Option<Xf>, origin: Vec2) {
    match (k, &mut s.kind) {
        (ShapeKind::Path(rest), ShapeKind::Path(pd)) => {
            let mut v = Vec::new();
            for sp in &rest.subpaths {
                for n in &sp.nodes {
                    v.push(n.p);
                    v.extend(n.hin);
                    v.extend(n.hout);
                }
            }
            let frame = match x {
                Some(x) => o * x.affine(centroid(&v) + origin),
                None => o,
            };
            let mut p = rest.clone();
            p.transform(frame);
            *pd = p;
        }
        (_, kind) if std::mem::discriminant(k) == std::mem::discriminant(kind) && !matches!(kind, ShapeKind::Path(_)) => {
            let size = kind.size();
            *kind = k.clone();
            if let Some((w, h)) = size {
                kind.set_size(w, h);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    /// Seconds.
    pub t: f64,
    pub value: Value,
    /// Easing towards the next key.
    #[serde(default)]
    pub ease: Ease,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub prop: Prop,
    /// Sorted by time.
    pub keys: Vec<Key>,
}

impl Track {
    pub fn value_at(&self, t: f64) -> Option<Value> {
        let first = self.keys.first()?;
        if t <= first.t {
            return Some(first.value.clone());
        }
        for (i, w) in self.keys.windows(2).enumerate() {
            if t < w[1].t {
                if self.prop.discrete() {
                    return Some(w[0].value.clone());
                }
                let u = (t - w[0].t) / (w[1].t - w[0].t).max(1e-9);
                if w[0].ease == Ease::Smooth
                    && let Some(v) = self.smooth(i, u)
                {
                    return Some(v);
                }
                return Some(w[0].value.lerp(&w[1].value, w[0].ease.apply(u)));
            }
        }
        self.keys.last().map(|k| k.value.clone())
    }

    /// Between keys `i` and `i + 1` at `u` (0..1) along a cubic Hermite curve
    /// whose slopes come from the neighbouring keys (Catmull-Rom), flattened
    /// at turning points and at the ends so it never overshoots a key.
    fn smooth(&self, i: usize, u: f64) -> Option<Value> {
        let k = &self.keys;
        let (p1, p2) = (comps(&k[i].value)?, comps(&k[i + 1].value)?);
        let slope = |j: usize| -> Option<Vec<f64>> {
            if j == 0 || j + 1 >= k.len() {
                return Some(vec![0.0; p1.len()]);
            }
            let (a, b, c) = (comps(&k[j - 1].value)?, comps(&k[j].value)?, comps(&k[j + 1].value)?);
            let (ta, tb, tc) = (k[j - 1].t, k[j].t, k[j + 1].t);
            Some(
                (0..b.len())
                    .map(|n| {
                        let (d0, d1) = ((b[n] - a[n]) / (tb - ta).max(1e-9), (c[n] - b[n]) / (tc - tb).max(1e-9));
                        if d0 * d1 <= 0.0 { 0.0 } else { (c[n] - a[n]) / (tc - ta).max(1e-9) }
                    })
                    .collect(),
            )
        };
        let (m1, m2) = (slope(i)?, slope(i + 1)?);
        if m1.len() != p1.len() || m2.len() != p1.len() || p2.len() != p1.len() {
            return None;
        }
        let dt = k[i + 1].t - k[i].t;
        let (u2, u3) = (u * u, u * u * u);
        let (h00, h10, h01, h11) = (2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, -2.0 * u3 + 3.0 * u2, u3 - u2);
        let v: Vec<f64> = (0..p1.len()).map(|n| h00 * p1[n] + h10 * dt * m1[n] + h01 * p2[n] + h11 * dt * m2[n]).collect();
        from_comps(&k[i].value, &v)
    }

    /// Index of the key at `t` (within half a frame at 240 fps).
    pub fn key_at(&self, t: f64) -> Option<usize> {
        self.keys.iter().position(|k| (k.t - t).abs() < 1.0 / 480.0)
    }

    /// Set the value at `t`, adding a key there if needed.
    pub fn set(&mut self, t: f64, value: Value) {
        match self.key_at(t) {
            Some(i) => self.keys[i].value = value,
            None => {
                let i = self.keys.partition_point(|k| k.t < t);
                self.keys.insert(i, Key { t, value, ease: Ease::default() });
            }
        }
    }

    pub fn sort(&mut self) {
        self.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
    }
}

/// Numeric parts of a value that can follow a curve.
fn comps(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Num(n) => Some(vec![*n]),
        Value::Xf(x) => Some(vec![x.dx, x.dy, x.rot, x.sx, x.sy, x.skew]),
        Value::Color(c) => Some(vec![c.r as f64, c.g as f64, c.b as f64, c.a as f64]),
        _ => None,
    }
}

fn from_comps(like: &Value, v: &[f64]) -> Option<Value> {
    Some(match (like, v) {
        (Value::Num(_), [n]) => Value::Num(*n),
        (Value::Xf(_), [dx, dy, rot, sx, sy, skew]) => Value::Xf(Xf { dx: *dx, dy: *dy, rot: *rot, sx: *sx, sy: *sy, skew: *skew }),
        (Value::Color(_), [r, g, b, a]) => Value::Color(Rgba { r: (*r as f32).clamp(0.0, 1.0), g: (*g as f32).clamp(0.0, 1.0), b: (*b as f32).clamp(0.0, 1.0), a: (*a as f32).clamp(0.0, 1.0) }),
        _ => return None,
    })
}

/// Keyframe tracks of one shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Anim {
    pub tracks: Vec<Track>,
    /// Where rotation and scale act: the pivot's offset from the centre of
    /// the shape's points at rest (in the frame its motion acts in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<[f64; 2]>,
}

/// The pivot offset of `a` (zero: the centre).
pub fn origin_of(a: Option<&Anim>) -> Vec2 {
    a.and_then(|a| a.origin).map_or(Vec2::ZERO, |[x, y]| Vec2::new(x, y))
}

impl Anim {
    pub fn track(&self, p: Prop) -> Option<&Track> {
        self.tracks.iter().find(|t| t.prop == p)
    }

    pub fn track_mut(&mut self, p: Prop) -> &mut Track {
        if let Some(i) = self.tracks.iter().position(|t| t.prop == p) {
            return &mut self.tracks[i];
        }
        self.tracks.push(Track { prop: p, keys: Vec::new() });
        self.tracks.last_mut().unwrap()
    }

    pub fn xf_at(&self, t: f64) -> Option<Xf> {
        match self.track(Prop::Transform)?.value_at(t)? {
            Value::Xf(x) => Some(x),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.iter().all(|t| t.keys.is_empty())
    }
}

// ---------------------------------------------------------------------------
// Values on shapes
// ---------------------------------------------------------------------------

pub fn read(s: &Shape, p: Prop) -> Option<Value> {
    let text = match &s.kind {
        ShapeKind::Text(t) => Some(t),
        _ => None,
    };
    match p {
        Prop::Transform => Some(Value::Xf(Xf::default())),
        Prop::Shape => rest_shape(s, Affine::IDENTITY),
        Prop::Opacity => Some(Value::Num(s.opacity as f64)),
        Prop::Fill => match &s.fill {
            Some(Paint::Solid(c)) => Some(Value::Color(*c)),
            Some(g @ Paint::Gradient(gr)) if gr.mesh.is_none() => Some(Value::Paint(Box::new(g.clone()))),
            _ => None,
        },
        Prop::StrokeColor => s.stroke.as_ref().map(|st| Value::Color(st.color)),
        Prop::StrokeWidth => s.stroke.as_ref().map(|st| Value::Num(st.width)),
        Prop::Dash => s.stroke.as_ref().filter(|st| !st.dash.is_empty()).map(|st| Value::List(st.dash.clone())),
        Prop::DashOffset => s.stroke.as_ref().filter(|st| !st.dash.is_empty()).map(|st| Value::Num(st.dash_offset)),
        Prop::Shadow => s.shadow.as_ref().map(shadow_list),
        Prop::InnerShadow => s.inner_shadow.as_ref().map(shadow_list),
        Prop::Blur => Some(Value::Num(s.blur)),
        Prop::Noise => Some(Value::Num(s.noise as f64)),
        Prop::Visible => Some(Value::Flag(s.visible)),
        Prop::Blend => Some(Value::Text(s.blend.label().to_string())),
        Prop::Text => text.map(|t| Value::Text(t.content.clone())),
        Prop::FontSize => text.map(|t| Value::Num(t.size)),
        Prop::FontWeight => text.map(|t| Value::Num(t.weight as f64)),
        Prop::LetterSpacing => text.map(|t| Value::Num(t.letter_spacing)),
        Prop::LineHeight => text.map(|t| Value::Num(t.line_height)),
        Prop::PathOffset => text.filter(|t| t.path.is_some()).map(|t| Value::Num(t.path_offset)),
        Prop::Clip => match s.clip.as_deref().map(|c| &c.kind) {
            Some(k @ ShapeKind::Path(_)) => Some(Value::Shape(Box::new(k.clone()))),
            _ => None,
        },
        Prop::BlendSteps => match s.live {
            Some(crate::paint_ops::LiveEffect::Blend { steps }) => Some(Value::Num(steps as f64)),
            _ => None,
        },
    }
}

fn write(s: &mut Shape, p: Prop, v: Value) {
    if let ShapeKind::Text(t) = &mut s.kind {
        match (p, &v) {
            (Prop::Text, Value::Text(c)) => t.content = c.clone(),
            (Prop::FontSize, Value::Num(n)) => t.size = n.max(0.5),
            (Prop::FontWeight, Value::Num(n)) => t.weight = n.round().clamp(1.0, 1000.0) as u16,
            (Prop::LetterSpacing, Value::Num(n)) => t.letter_spacing = *n,
            (Prop::LineHeight, Value::Num(n)) => t.line_height = n.max(0.0),
            (Prop::PathOffset, Value::Num(n)) => t.path_offset = *n,
            _ => {}
        }
    }
    match (p, v) {
        (Prop::Opacity, Value::Num(n)) => s.opacity = n.clamp(0.0, 1.0) as f32,
        (Prop::Fill, Value::Color(c)) => {
            if matches!(s.fill, Some(Paint::Solid(_) | Paint::Gradient(_))) {
                s.fill = Some(Paint::Solid(c));
            }
        }
        (Prop::Fill, Value::Paint(paint)) => {
            if matches!(s.fill, Some(Paint::Solid(_) | Paint::Gradient(_))) {
                s.fill = Some(*paint);
            }
        }
        (Prop::StrokeColor, Value::Color(c)) => {
            if let Some(st) = &mut s.stroke {
                st.color = c;
            }
        }
        (Prop::StrokeWidth, Value::Num(n)) => {
            if let Some(st) = &mut s.stroke {
                st.width = n.max(0.0);
            }
        }
        (Prop::Dash, Value::List(d)) => {
            if let Some(st) = &mut s.stroke {
                st.dash = d.into_iter().map(|x| x.max(0.0)).collect();
            }
        }
        (Prop::DashOffset, Value::Num(n)) => {
            if let Some(st) = &mut s.stroke {
                st.dash_offset = n;
            }
        }
        (Prop::Shadow, Value::List(l)) => {
            if s.shadow.is_some() {
                s.shadow = list_shadow(&l).or(s.shadow.take());
            }
        }
        (Prop::InnerShadow, Value::List(l)) => {
            if s.inner_shadow.is_some() {
                s.inner_shadow = list_shadow(&l).or(s.inner_shadow.take());
            }
        }
        (Prop::Blur, Value::Num(n)) => s.blur = n.max(0.0),
        (Prop::Noise, Value::Num(n)) => s.noise = n.clamp(0.0, 1.0) as f32,
        (Prop::Visible, Value::Flag(b)) => s.visible = b,
        (Prop::Clip, Value::Shape(k)) => {
            if let Some(c) = s.clip.as_deref_mut().filter(|c| matches!(c.kind, ShapeKind::Path(_))) {
                c.kind = *k;
            }
        }
        (Prop::BlendSteps, Value::Num(n)) => {
            if let Some(crate::paint_ops::LiveEffect::Blend { steps }) = &mut s.live {
                *steps = n.round().clamp(0.0, 500.0) as u32;
            }
        }
        (Prop::Blend, Value::Text(name)) => {
            if let Some(m) = BlendMode::ALL.iter().find(|m| m.label() == name) {
                s.blend = *m;
            }
        }
        _ => {}
    }
}

/// Points that move exactly with the shape under an affine transform.
pub fn points(s: &Shape, out: &mut Vec<Point>) {
    match &s.kind {
        ShapeKind::Path(pd) => {
            for sp in &pd.subpaths {
                for n in &sp.nodes {
                    out.push(n.p);
                    out.extend(n.hin);
                    out.extend(n.hout);
                }
            }
        }
        ShapeKind::Rect { w, h, .. } | ShapeKind::Ellipse { w, h } | ShapeKind::Star { w, h, .. } | ShapeKind::Polygon { w, h, .. } | ShapeKind::Prim { w, h, .. } => {
            for p in [(0.0, 0.0), (*w, 0.0), (0.0, *h), (*w, *h)] {
                out.push(s.transform * Point::new(p.0, p.1));
            }
        }
        ShapeKind::Text(_) => {
            for p in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)] {
                out.push(s.transform * Point::new(p.0, p.1));
            }
        }
        ShapeKind::Compound { children, .. } | ShapeKind::Group { children } => {
            for c in children {
                points(c, out);
            }
        }
    }
}

fn centroid(pts: &[Point]) -> Point {
    if pts.is_empty() {
        return Point::ZERO;
    }
    let s = pts.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2());
    (s / pts.len() as f64).to_point()
}

/// The affine taking `a` onto `b` (same order), if there is one.
pub fn fit(a: &[Point], b: &[Point]) -> Option<Affine> {
    if a.len() != b.len() {
        return None;
    }
    if a.is_empty() {
        return Some(Affine::IDENTITY);
    }
    let (ma, mb) = (centroid(a), centroid(b));
    let extent = a.iter().chain(b).map(|p| (*p - ma).hypot()).fold(1.0, f64::max);
    let tol = 1e-7 * extent;
    let translate = Affine::translate(mb - ma);
    // Covariance of the source points, and cross-covariance with the target.
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    let (mut xx, mut xy, mut yx, mut yy) = (0.0, 0.0, 0.0, 0.0);
    for (p, q) in a.iter().zip(b) {
        let (u, v) = (*p - ma, *q - mb);
        sxx += u.x * u.x;
        sxy += u.x * u.y;
        syy += u.y * u.y;
        xx += v.x * u.x;
        xy += v.x * u.y;
        yx += v.y * u.x;
        yy += v.y * u.y;
    }
    let det = sxx * syy - sxy * sxy;
    let m = if det.abs() > 1e-12 * (sxx + syy).powi(2).max(1e-300) {
        let (i00, i01, i11) = (syy / det, -sxy / det, sxx / det);
        let (l00, l01) = (xx * i00 + xy * i01, xx * i01 + xy * i11);
        let (l10, l11) = (yx * i00 + yy * i01, yx * i01 + yy * i11);
        let lin = Affine::new([l00, l10, l01, l11, 0.0, 0.0]);
        Affine::translate(mb.to_vec2()) * lin * Affine::translate(-ma.to_vec2())
    } else {
        // Collinear or a single point: a translation is all we can tell.
        translate
    };
    a.iter().zip(b).all(|(p, q)| (m * *p - *q).hypot() <= tol).then_some(m)
}

pub fn near_identity(a: Affine) -> bool {
    let c = a.as_coeffs();
    let i = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    c.iter().zip(i).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// How far a shape's points are, on average, from where they rest: its own
/// motion plus its animated children's. The pivot a shape's motion turns
/// about is the centroid of its points at rest, so it's
/// `centroid(current) − offset` (in the frame the motion acts in), whatever
/// the children are doing.
fn offset(s: &Shape, t: f64, old: Option<&OldAnims>) -> (Vec2, usize) {
    let a = before(s, old);
    let x = a.as_ref().and_then(|a| a.xf_at(t)).unwrap_or_default();
    // The rest centre goes where the motion about the pivot (centre + e) takes it.
    let e = origin_of(a.as_ref());
    let pivot_shift = e - (x.linear() * e.to_point()).to_vec2();
    let (mut sum, mut n) = (Vec2::ZERO, 0usize);
    match s.kind.children() {
        Some(children) => {
            for c in children {
                let (o, k) = offset(c, t, old);
                sum += o * k as f64;
                n += k;
            }
        }
        None => {
            let mut v = Vec::new();
            points(s, &mut v);
            n = v.len();
        }
    }
    let inner = if n > 0 { sum / n as f64 } else { Vec2::ZERO };
    (Vec2::new(x.dx, x.dy) + pivot_shift + (x.linear() * inner.to_point()).to_vec2(), n)
}

pub fn has_any(doc: &Document) -> bool {
    fn walk(s: &Shape) -> bool {
        s.anim.as_ref().is_some_and(|a| !a.is_empty()) || s.kind.children().is_some_and(|c| c.iter().any(walk))
    }
    doc.shapes().iter().any(|s| walk(s))
}

// ---------------------------------------------------------------------------
// Moving the playhead
// ---------------------------------------------------------------------------

/// Old tracks per shape, when keys changed rather than the time.
type OldAnims = HashMap<u64, Option<Anim>>;

/// The tracks `s` had before: from `old` when keys changed (a shape missing
/// there had none), else its own.
fn before(s: &Shape, old: Option<&OldAnims>) -> Option<Anim> {
    match old {
        Some(m) => m.get(&s.id).cloned().flatten(),
        None => s.anim.as_deref().cloned(),
    }
}

/// Bring `s` from its state at `t1` (with `old` tracks, or its own) to `t2`
/// with its current tracks. `o` is the motion of its animated ancestors.
fn transition(s: &mut Shape, o: Affine, t1: f64, t2: f64, old: Option<&OldAnims>) {
    let new = s.anim.clone();
    let prev = before(s, old);
    let mut inner = o;
    // Keyed path geometry is placed outright (its motion included).
    let geometry = new.as_ref().and_then(|a| a.track(Prop::Shape)).and_then(|t| t.value_at(t2));
    let placed = matches!((&geometry, &s.kind), (Some(Value::Shape(k)), ShapeKind::Path(_)) if matches!(**k, ShapeKind::Path(_)));
    let (e1, e2) = (origin_of(prev.as_ref()), origin_of(new.as_deref()));
    if placed {
        if let Some(Value::Shape(k)) = &geometry {
            write_shape(s, k, o, new.as_ref().and_then(|a| a.xf_at(t2)), e2);
        }
    } else if let Some(x2) = new.as_ref().and_then(|a| a.xf_at(t2)) {
        // No motion before: the shape was at rest.
        let x1 = prev.as_ref().and_then(|a| a.xf_at(t1)).unwrap_or_default();
        let mut pts = Vec::new();
        points(s, &mut pts);
        let pivot = o.inverse() * centroid(&pts) - offset(s, t1, old).0;
        let (a1, a2) = (x1.affine(pivot + e1), x2.affine(pivot + e2));
        let delta = o * a2 * a1.inverse() * o.inverse();
        if !near_identity(delta) {
            s.apply_transform(delta);
        }
        inner = o * a2;
    }
    if let Some(a) = &new {
        for tr in &a.tracks {
            match (tr.prop, tr.value_at(t2)) {
                (Prop::Transform, _) | (_, None) => {}
                (Prop::Shape, Some(Value::Shape(k))) => {
                    if !placed {
                        write_shape(s, &k, o, None, Vec2::ZERO);
                    }
                }
                (p, Some(v)) => write(s, p, v),
            }
        }
    }
    if let Some(children) = s.kind.children_mut() {
        for c in children {
            transition(c, inner, t1, t2, old);
        }
    }
}

fn subtree_animated(s: &Shape, old: Option<&OldAnims>) -> bool {
    s.anim.is_some() || old.is_some_and(|m| m.contains_key(&s.id)) || s.kind.children().is_some_and(|c| c.iter().any(|c| subtree_animated(c, old)))
}

fn transition_doc(doc: &mut Document, t1: f64, t2: f64, old: Option<&OldAnims>) {
    let ids: Vec<u64> = doc.shapes().iter().filter(|s| subtree_animated(s, old)).map(|s| s.id).collect();
    for id in ids {
        let Some(mut s) = doc.get(id).cloned() else { continue };
        transition(&mut s, Affine::IDENTITY, t1, t2, old);
        doc.replace(s);
    }
}

/// Show the document at `t` (seconds).
pub fn seek(doc: &mut Document, t: f64) {
    let Some(tl) = doc.animation.clone() else { return };
    let t = t.clamp(0.0, tl.duration);
    if (t - tl.time).abs() > 1e-12 {
        transition_doc(doc, tl.time, t, None);
    }
    if let Some(a) = &mut doc.animation {
        a.time = t;
    }
}

/// Move `s` (and its subtree) by `d` through its whole animation: its
/// current geometry and every keyed outline and clip go along. (Transform
/// keys are relative, so they stay as they are.)
pub fn translate(s: &mut Shape, d: kurbo::Vec2) {
    s.apply_transform(Affine::translate(d));
    fn keys(s: &mut Shape, d: kurbo::Vec2) {
        if let Some(a) = s.anim.as_deref_mut() {
            for tr in a.tracks.iter_mut().filter(|t| matches!(t.prop, Prop::Shape | Prop::Clip)) {
                for k in &mut tr.keys {
                    if let Value::Shape(kind) = &mut k.value
                        && let ShapeKind::Path(pd) = &mut **kind
                    {
                        pd.transform(Affine::translate(d));
                    }
                }
            }
        }
        if let Some(children) = s.kind.children_mut() {
            children.iter_mut().for_each(|c| keys(c, d));
        }
    }
    keys(s, d);
}

/// A copy of the document at `t`.
pub fn frame(doc: &Document, t: f64) -> Document {
    let mut d = doc.clone();
    seek(&mut d, t);
    d
}

/// Change keys with `f`, keeping the document in step: the shapes take
/// their new values at the playhead. Tracks that disappear leave the
/// current value as it is.
pub fn edit_keys(doc: &mut Document, f: impl FnOnce(&mut Document)) {
    let mut old = OldAnims::new();
    for id in doc.all_ids() {
        if let Some(a) = doc.get(id).and_then(|s| s.anim.as_deref()) {
            old.insert(id, Some(a.clone()));
        }
    }
    f(doc);
    // Drop empty tracks and anims.
    for id in doc.all_ids() {
        let Some(s) = doc.get(id) else { continue };
        let Some(a) = s.anim.as_deref() else { continue };
        if a.tracks.iter().any(|t| t.keys.is_empty()) || a.is_empty() {
            let mut s = s.clone();
            if let Some(a) = s.anim.as_deref_mut() {
                a.tracks.retain(|t| !t.keys.is_empty());
            }
            if s.anim.as_deref().is_some_and(Anim::is_empty) {
                s.anim = None;
            }
            doc.replace(s);
        }
    }
    // A removed transform track keeps the current geometry: compare as if
    // it was never there.
    for (id, a) in old.iter_mut() {
        let keeps = doc.get(*id).and_then(|s| s.anim.as_deref()).and_then(|n| n.track(Prop::Transform)).is_some();
        if !keeps {
            if let Some(a) = a {
                a.tracks.retain(|t| t.prop != Prop::Transform);
            }
        }
    }
    let t = doc.animation.as_ref().map_or(0.0, |a| a.time);
    transition_doc(doc, t, t, Some(&old));
}

// ---------------------------------------------------------------------------
// Picking up edits
// ---------------------------------------------------------------------------

/// Compare `s` with how it was (`b`) and key what changed at `t`.
/// `o_old`/`o_new`: the frame of its animated ancestors before and after.
fn capture(b: &Shape, s: &mut Shape, o_old: Affine, o_new: Affine, t: f64, record: bool, focus: &[u64]) {
    // Keys are only changed here and through `edit_keys`: tools that rebuild
    // shapes from copies taken earlier (a drag from where it started) must
    // not take keys made since with them.
    s.anim = b.anim.clone();
    let (mut pb, mut ps) = (Vec::new(), Vec::new());
    points(b, &mut pb);
    points(s, &mut ps);
    // A group whose only child was moved looks moved itself: when the
    // edited layer is inside, the change is the child's.
    let inside = |id: u64| s.kind.children().is_some_and(|c| c.iter().any(|c| contains(c, id)));
    let delegated = !focus.contains(&s.id) && focus.iter().any(|f| inside(*f));
    let moved = if delegated { None } else { fit(&pb, &ps) };
    let has_xf = s.anim.as_deref().and_then(|a| a.track(Prop::Transform)).is_some();
    let x_old = b.anim.as_deref().and_then(|a| a.xf_at(t)).unwrap_or_default();
    let pivot = o_old.inverse() * centroid(&pb) - offset(b, t, None).0 + origin_of(b.anim.as_deref());
    let a_old = x_old.affine(pivot);
    let changed = moved.filter(|m| !near_identity(*m));
    let keyed_move = changed.is_some() && (has_xf || record);
    let child_new = match (changed, moved) {
        (Some(m), _) if keyed_move => {
            let a_new = o_new.inverse() * m * o_old * a_old;
            let x_new = Xf::from_affine(a_new, pivot, x_old.rot);
            if !x_new.close(&x_old) {
                let anim = s.anim.get_or_insert_with(Default::default);
                let tr = anim.track_mut(Prop::Transform);
                if tr.keys.is_empty() && t > 1e-9 {
                    tr.set(0.0, Value::Xf(Xf::default()));
                }
                tr.set(t, Value::Xf(x_new));
            }
            m * o_old * a_old
        }
        (_, Some(m)) => m * o_old * a_old,
        (_, None) => {
            // Reshaped: same motion around the new centroid.
            let pivot = o_new.inverse() * centroid(&ps) - offset(s, t, None).0 + origin_of(s.anim.as_deref());
            o_new * x_old.affine(pivot)
        }
    };
    // A path moved without keying it moves through its whole animation:
    // its keyed geometry goes along.
    if let (Some(m), false, ShapeKind::Path(_)) = (changed, keyed_move, &s.kind) {
        let f = o_old * a_old;
        let conj = f.inverse() * m * f;
        if let Some(tr) = s.anim.as_deref_mut().and_then(|a| a.tracks.iter_mut().find(|t| t.prop == Prop::Shape)) {
            for k in &mut tr.keys {
                if let Value::Shape(kind) = &mut k.value {
                    if let ShapeKind::Path(pd) = &mut **kind {
                        pd.transform(conj);
                    }
                }
            }
        }
    }
    for p in Prop::ALL.into_iter().filter(|p| *p != Prop::Transform) {
        let (vb, vs) = match p {
            Prop::Shape => (rest_shape(b, o_old * a_old), rest_shape(s, child_new)),
            _ => (read(b, p), read(s, p)),
        };
        let (Some(vb), Some(vs)) = (vb, vs) else { continue };
        if vb.close(&vs) {
            continue;
        }
        let tracked = s.anim.as_deref().and_then(|a| a.track(p)).is_some();
        if tracked || record {
            let anim = s.anim.get_or_insert_with(Default::default);
            let tr = anim.track_mut(p);
            if tr.keys.is_empty() && t > 1e-9 {
                tr.set(0.0, vb);
            }
            tr.set(t, vs);
        }
    }
    let base: HashMap<u64, &Shape> = b.kind.children().map(|c| c.iter().map(|c| (c.id, c)).collect()).unwrap_or_default();
    let made = generated(s);
    if let Some(children) = s.kind.children_mut() {
        let n = children.len();
        for (i, c) in children.iter_mut().enumerate() {
            if made(i, n) {
                continue;
            }
            if let Some(bc) = base.get(&c.id) {
                capture(bc, c, o_old * a_old, child_new, t, record, focus);
            }
        }
    }
}

/// Children of `s` that a live effect makes (a blend's steps, a warp's
/// results): rebuilt from the rest, so never keyed. Takes (index, count).
fn generated(s: &Shape) -> fn(usize, usize) -> bool {
    match s.live {
        Some(crate::paint_ops::LiveEffect::Blend { .. }) => |i, n| i > 0 && i + 1 < n,
        Some(crate::paint_ops::LiveEffect::Warp(_)) => |_, _| true,
        None => |_, _| false,
    }
}

/// Key `p` of shape `id` at `t` with the value it has now (a key that is
/// there already is updated, not removed: an edit may just have made it).
/// Call inside [`edit_keys`].
pub fn key_current(doc: &mut Document, id: u64, p: Prop, t: f64) {
    let Some(mut s) = doc.get(id).cloned() else { return };
    let current = match p {
        Prop::Transform => s.anim.as_deref().and_then(|a| a.xf_at(t)).map(Value::Xf).or(Some(Value::Xf(Xf::default()))),
        Prop::Shape => rest_shape(&s, frame_of(doc, id, t)),
        _ => read(&s, p),
    };
    let Some(v) = current else { return };
    s.anim.get_or_insert_with(Default::default).track_mut(p).set(t, v);
    doc.replace(s);
}

/// The motion of shape `id` at `t`, its animated ancestors' included (what
/// takes its rest geometry to where it is).
pub fn frame_of(doc: &Document, id: u64, t: f64) -> Affine {
    let mut chain = vec![id];
    let mut p = doc.parent_of(id);
    while let Some(x) = p {
        chain.push(x);
        p = doc.parent_of(x);
    }
    let mut o = Affine::IDENTITY;
    for sid in chain.into_iter().rev() {
        let Some(s) = doc.get(sid) else { continue };
        if let Some(x) = s.anim.as_deref().and_then(|a| a.xf_at(t)) {
            let mut pts = Vec::new();
            points(s, &mut pts);
            let pivot = o.inverse() * centroid(&pts) - offset(s, t, None).0 + origin_of(s.anim.as_deref());
            o = o * x.affine(pivot);
        }
    }
    o
}

/// The centre of shape `id`'s points at rest, in the frame its motion acts
/// in (document space when its ancestors are at rest): the pivot rotation and
/// scale use, before its `origin`.
pub fn rest_centre(doc: &Document, id: u64, t: f64) -> Option<Point> {
    let o = match doc.parent_of(id) {
        Some(p) => frame_of(doc, p, t),
        None => Affine::IDENTITY,
    };
    let s = doc.get(id)?;
    let mut pts = Vec::new();
    points(s, &mut pts);
    (!pts.is_empty()).then(|| o.inverse() * centroid(&pts) - offset(s, t, None).0)
}

/// Shape `id` with it, its ancestors and everything inside it at rest (no
/// transform keys): where it sits when nothing moves.
pub fn at_rest(doc: &Document, id: u64) -> Option<Shape> {
    let mut d = doc.clone();
    let mut ids = vec![id];
    let mut p = d.parent_of(id);
    while let Some(x) = p {
        ids.push(x);
        p = d.parent_of(x);
    }
    fn subtree(s: &Shape, out: &mut Vec<u64>) {
        out.push(s.id);
        s.kind.children().into_iter().flatten().for_each(|c| subtree(c, out));
    }
    subtree(d.get(id)?, &mut ids);
    edit_keys(&mut d, |d| {
        for id in &ids {
            let Some(mut s) = d.get(*id).cloned() else { continue };
            if let Some(tr) = s.anim.as_deref_mut().and_then(|a| a.tracks.iter_mut().find(|t| t.prop == Prop::Transform)) {
                tr.keys.truncate(1);
                tr.keys[0].value = Value::Xf(Xf::default());
                d.replace(s);
            }
        }
    });
    d.get(id).cloned()
}

fn contains(s: &Shape, id: u64) -> bool {
    s.id == id || s.kind.children().is_some_and(|c| c.iter().any(|c| contains(c, id)))
}

/// Key the edits made since `base` (the document at the last sync).
/// `focus`: the layers being edited (the selection), to tell a moved group
/// from its moved only child.
pub fn capture_edits(base: &Document, doc: &mut Document, record: bool, focus: &[u64]) {
    let Some(t) = doc.animation.as_ref().map(|a| a.time) else { return };
    // Untouched shapes are still shared with the base.
    let changed: Vec<u64> = doc.shapes().iter().filter(|s| base.get(s.id).is_some_and(|b| !std::ptr::eq(&***s, b) && ***s != *b)).map(|s| s.id).collect();
    for id in changed {
        let (Some(b), Some(s)) = (base.get(id), doc.get(id)) else { continue };
        if !record && !subtree_animated(b, None) {
            continue;
        }
        let mut s = s.clone();
        capture(b, &mut s, Affine::IDENTITY, Affine::IDENTITY, t, record, focus);
        doc.replace(s);
    }
}

// ---------------------------------------------------------------------------
// Keys from frames (imported animations)
// ---------------------------------------------------------------------------

/// Animate shapes from their state at every frame: `frames[k]` holds the
/// same shapes (same structure) at frame `k`. Returns frame 0's shapes with
/// linear keys that give the others, minus the keys their neighbours already
/// give. Leaves that only move rigidly get a transform track (about the
/// frame where they're largest); ones that change shape get shape keys.
pub fn from_frames(frames: &[Vec<Shape>], fps: f64) -> Vec<Shape> {
    let Some(first) = frames.first() else { return Vec::new() };
    let mut out = first.clone();
    for (i, s) in out.iter_mut().enumerate() {
        let states: Vec<&Shape> = frames.iter().filter_map(|f| f.get(i)).collect();
        if states.len() == frames.len() {
            bake(s, &states, fps);
        }
    }
    out
}

fn bake(s: &mut Shape, states: &[&Shape], fps: f64) {
    let t = |k: usize| k as f64 / fps;
    let key = |k: usize, value: Value| Key { t: t(k), value, ease: Ease::Linear };
    let mut tracks = Vec::new();
    if s.kind.children().is_none() {
        let pts: Vec<Vec<Point>> = states
            .iter()
            .map(|st| {
                let mut v = Vec::new();
                points(st, &mut v);
                v
            })
            .collect();
        let same = |a: &[Point], b: &[Point]| a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (*p - *q).hypot() < 1e-9);
        if pts.iter().any(|p| !same(p, &pts[0])) {
            let spread = |p: &[Point]| {
                let c = centroid(p);
                p.iter().map(|q| (*q - c).hypot2()).sum::<f64>()
            };
            let r = (0..pts.len()).max_by(|a, b| spread(&pts[*a]).total_cmp(&spread(&pts[*b]))).unwrap_or(0);
            let fits: Option<Vec<Affine>> = pts.iter().map(|p| fit(&pts[r], p)).collect();
            match fits {
                Some(m) => {
                    let c = centroid(&pts[r]);
                    let mut near = 0.0;
                    let keys = m
                        .iter()
                        .enumerate()
                        .map(|(k, a)| {
                            let x = Xf::from_affine(*a, c, near);
                            near = x.rot;
                            key(k, Value::Xf(x))
                        })
                        .collect();
                    tracks.push(Track { prop: Prop::Transform, keys });
                    // Collapsed at some frame (scaled to zero): the motion
                    // can't be undone from there, so the geometry is kept.
                    if m.iter().any(|a| a.determinant().abs() < 1e-6) {
                        if let Some(v) = rest_shape(states[r], Affine::IDENTITY) {
                            tracks.push(Track { prop: Prop::Shape, keys: vec![key(0, v)] });
                        }
                    }
                }
                None => {
                    let keys: Option<Vec<Key>> = states.iter().enumerate().map(|(k, st)| rest_shape(st, Affine::IDENTITY).map(|v| key(k, v))).collect();
                    if let Some(keys) = keys {
                        tracks.push(Track { prop: Prop::Shape, keys });
                    }
                }
            }
        }
    }
    for p in Prop::ALL {
        if matches!(p, Prop::Transform | Prop::Shape) {
            continue;
        }
        let Some(vals) = states.iter().map(|st| read(st, p)).collect::<Option<Vec<Value>>>() else { continue };
        if vals.iter().all(|v| v.close(&vals[0])) {
            continue;
        }
        tracks.push(Track { prop: p, keys: vals.into_iter().enumerate().map(|(k, v)| key(k, v)).collect() });
    }
    for tr in &mut tracks {
        simplify(tr);
    }
    s.anim = (!tracks.is_empty()).then(|| Box::new(Anim { tracks, origin: None }));
    let kids: Vec<Vec<&Shape>> = match s.kind.children() {
        Some(c) => (0..c.len()).map(|i| states.iter().filter_map(|st| st.kind.children().and_then(|c| c.get(i))).collect()).collect(),
        None => Vec::new(),
    };
    if let Some(children) = s.kind.children_mut() {
        for (c, st) in children.iter_mut().zip(kids) {
            if st.len() == states.len() {
                bake(c, &st, fps);
            }
        }
    }
}

/// Close enough to leave out a key: within a twentieth of a pixel, a
/// thousandth (of a radian, a scale, an opacity), or half a colour step.
fn near(a: &Value, b: &Value) -> bool {
    let n = |x: f64, y: f64, e: f64| (x - y).abs() <= e;
    let col = |x: &Rgba, y: &Rgba| [(x.r, y.r), (x.g, y.g), (x.b, y.b), (x.a, y.a)].iter().all(|(p, q)| (p - q).abs() <= 1.0 / 512.0);
    let pt = |x: Point, y: Point| (x - y).hypot() <= 0.05;
    match (a, b) {
        (Value::Xf(x), Value::Xf(y)) => n(x.dx, y.dx, 0.05) && n(x.dy, y.dy, 0.05) && n(x.rot, y.rot, 1e-3) && n(x.sx, y.sx, 1e-3) && n(x.sy, y.sy, 1e-3) && n(x.skew, y.skew, 1e-3),
        (Value::Num(x), Value::Num(y)) => n(*x, *y, 1e-3 * x.abs().max(1.0)),
        (Value::Color(x), Value::Color(y)) => col(x, y),
        (Value::List(x), Value::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| n(*p, *q, 1e-3 * p.abs().max(1.0))),
        (Value::Paint(x), Value::Paint(y)) => match (&**x, &**y) {
            (Paint::Gradient(g), Paint::Gradient(h)) => {
                g.kind == h.kind
                    && g.stops.len() == h.stops.len()
                    && g.stops.iter().zip(&h.stops).all(|(p, q)| (p.offset - q.offset).abs() <= 1e-3 && col(&p.color, &q.color))
                    && (0..2).all(|i| n(g.start[i], h.start[i], 1e-3) && n(g.end[i], h.end[i], 1e-3))
            }
            _ => x == y,
        },
        (Value::Shape(x), Value::Shape(y)) => match (&**x, &**y) {
            (ShapeKind::Path(p), ShapeKind::Path(q)) => {
                let h = |x: Option<Point>, y: Option<Point>, a: Point, b: Point| pt(x.unwrap_or(a), y.unwrap_or(b));
                p.subpaths.len() == q.subpaths.len()
                    && p.subpaths.iter().zip(&q.subpaths).all(|(s, r)| s.closed == r.closed && s.nodes.len() == r.nodes.len() && s.nodes.iter().zip(&r.nodes).all(|(m, k)| pt(m.p, k.p) && h(m.hin, k.hin, m.p, k.p) && h(m.hout, k.hout, m.p, k.p)))
            }
            _ => kind_close(x, y),
        },
        _ => a.close(b),
    }
}

/// Drop the keys that linear blending between the ones kept gives anyway.
fn simplify(tr: &mut Track) {
    let keys = std::mem::take(&mut tr.keys);
    if keys.len() < 3 {
        tr.keys = keys;
        return;
    }
    let discrete = tr.prop.discrete();
    let fits = |a: usize, b: usize| {
        (a + 1..b).all(|j| {
            let v = if discrete {
                keys[a].value.clone()
            } else {
                let u = (keys[j].t - keys[a].t) / (keys[b].t - keys[a].t).max(1e-12);
                keys[a].value.lerp(&keys[b].value, u)
            };
            near(&v, &keys[j].value)
        })
    };
    let mut kept = vec![0];
    let mut a = 0;
    let mut b = 2;
    while b < keys.len() {
        // Long spans are checked in pieces: no key is more than 120 frames
        // from the next.
        if b - a > 120 || !fits(a, b) {
            a = b - 1;
            kept.push(a);
        }
        b += 1;
    }
    kept.push(keys.len() - 1);
    tr.keys = kept.into_iter().map(|i| keys[i].clone()).collect();
}

// ---------------------------------------------------------------------------
// App integration
// ---------------------------------------------------------------------------

/// A key in the timeline: shape, property, key index.
pub type KeyRef = (u64, Prop, usize);

#[derive(Default)]
pub struct AnimState {
    /// The document at the last sync, and which document it was.
    base: Option<(u64, Document)>,
    /// Edits of any shape become keys.
    pub record: bool,
    pub playing: bool,
    last_tick: f64,
    pub selected: Vec<KeyRef>,
    /// The pointer is over the timeline (Delete removes keys there).
    pub hovered: bool,
    /// Layers folded in the timeline (only their summary row shows).
    pub collapsed: std::collections::HashSet<u64>,
    /// Copied keys: layer, property, time after the first copied key, value, easing.
    pub clipboard: Vec<(u64, Prop, f64, Value, Ease)>,
    /// What K keys: properties under the pointer (of the layer, or of the
    /// selection when `None`), as drawn last frame.
    pub hover_props: Vec<(Option<u64>, Prop)>,
    /// `hover_props` of the frame before (for hints drawn before the panels).
    pub hover_last: Vec<(Option<u64>, Prop)>,
    pub onion: Onion,
    /// Onion skin outlines and what they were made for.
    pub onion_cache: Option<(OnionKey, Vec<(kurbo::BezPath, i32)>)>,
}

/// How a property of the selection is animated at the playhead.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Static,
    /// Has keys, none at the playhead.
    Animated,
    /// A key at the playhead.
    Keyed,
}

/// Onion skinning: outlines of animated layers a few frames before and after.
#[derive(Clone, Copy, PartialEq)]
pub struct Onion {
    pub on: bool,
    /// Ghosts on each side.
    pub count: usize,
    /// Frames between ghosts.
    pub step: usize,
}

impl Default for Onion {
    fn default() -> Self {
        Onion { on: false, count: 2, step: 3 }
    }
}

/// Document revision, playhead and settings the onion cache is for.
pub type OnionKey = (u64, u64, usize, usize, u64);

/// Outlines of the animated layers at `t` (and everything inside animated
/// groups), in document space.
pub fn onion_paths(doc: &Document, t: f64, fonts: &mut crate::fonts::FontManager) -> Vec<kurbo::BezPath> {
    fn walk(s: &Shape, inside: bool, fonts: &mut crate::fonts::FontManager, out: &mut Vec<kurbo::BezPath>) {
        if !s.visible || !(inside || subtree_animated(s, None)) {
            return;
        }
        let inside = inside || s.anim.is_some();
        match s.kind.children() {
            Some(children) if !matches!(s.kind, ShapeKind::Compound { .. }) => {
                for c in children {
                    walk(c, inside, fonts, out);
                }
            }
            _ => out.push(crate::render::compute_outline(s, fonts).0),
        }
    }
    let f = frame(doc, t);
    let mut out = Vec::new();
    for s in f.shapes() {
        walk(s, false, fonts, &mut out);
    }
    out
}

impl AnimState {
    /// Forget the sync point (the document was replaced: undo, open…).
    pub fn rebase(&mut self) {
        self.base = None;
    }
}

impl crate::app::App {
    /// Per frame: play, and turn edits into keys.
    pub fn anim_tick(&mut self, ctx: &egui::Context) {
        if self.anim.playing {
            match self.doc.animation.clone() {
                Some(tl) => {
                    // Loops between the in and out points.
                    let (a, b) = tl.span();
                    let dt = (self.now - self.anim.last_tick).clamp(0.0, 0.25);
                    let mut t = if tl.time < a - 1e-9 || tl.time > b + 1e-9 { a } else { tl.time + dt };
                    if t >= b {
                        if tl.looping {
                            t = a + (t - a).rem_euclid((b - a).max(1e-6));
                        } else {
                            t = b;
                            self.anim.playing = false;
                        }
                    }
                    self.anim_seek(t);
                    ctx.request_repaint();
                }
                None => self.anim.playing = false,
            }
        }
        self.anim.last_tick = self.now;
        self.anim_sync();
        // Panels report what's under the pointer again this frame.
        self.anim.hover_last = std::mem::take(&mut self.anim.hover_props);
    }

    /// The selection's `p`: keyed at the playhead, animated, or neither.
    pub fn anim_key_state(&self, p: Prop) -> KeyState {
        let t = self.anim_time();
        let mut state = KeyState::Static;
        for id in &self.selection {
            let Some(tr) = self.doc.get(*id).and_then(|s| s.anim.as_deref()).and_then(|a| a.track(p)) else { continue };
            if tr.key_at(t).is_some() {
                return KeyState::Keyed;
            }
            state = KeyState::Animated;
        }
        state
    }

    /// Play or pause. Playing from the out point (with looping off) starts
    /// over at the in point.
    pub fn anim_toggle_play(&mut self) {
        let tl = self.doc.animation.get_or_insert_with(Default::default).clone();
        if !self.anim.playing {
            let (a, b) = tl.span();
            if tl.time >= b - 1e-6 || tl.time < a - 1e-9 {
                self.anim_seek(a);
            }
        }
        self.anim.playing = !self.anim.playing;
    }

    /// Note that the pointer over `rect` points at `props` (for K).
    pub fn anim_hover(&mut self, ui: &egui::Ui, rect: egui::Rect, id: Option<u64>, props: &[Prop]) {
        // The innermost region wins: it reports before the one around it.
        if self.settings.show_timeline && self.anim.hover_props.is_empty() && ui.rect_contains_pointer(rect) {
            self.anim.hover_props = props.iter().map(|p| (id, *p)).collect();
        }
    }

    /// K: key what the pointer was over. False if it was over nothing keyable.
    pub fn anim_key_hovered(&mut self) -> bool {
        let targets: Vec<(u64, Prop)> = self
            .anim
            .hover_props
            .iter()
            .flat_map(|(id, p)| match id {
                Some(id) => vec![(*id, *p)],
                None => self.selection.iter().map(|id| (*id, *p)).collect(),
            })
            .filter(|(id, p)| self.doc.get(*id).is_some_and(|s| *p == Prop::Transform || p.applies(s)))
            .collect();
        if targets.is_empty() {
            return false;
        }
        let t = self.anim_time();
        self.anim_edit("keyframe", |doc| {
            for (id, p) in &targets {
                key_current(doc, *id, *p, t);
            }
        });
        let names: Vec<&str> = targets.iter().map(|(_, p)| p.label()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        self.notify(format!("Keyed {} at {:.2} s", names.join(", "), t));
        true
    }

    /// Set the in (`start`) or out point at the playhead.
    pub fn anim_set_point(&mut self, start: bool) {
        let t = self.anim_time();
        self.checkpoint("in/out point");
        let tl = self.doc.animation.get_or_insert_with(Default::default);
        let (a, b) = tl.span();
        let (a, b) = if start { (t, b.max(t + 1.0 / tl.fps)) } else { (a.min(t - 1.0 / tl.fps).max(0.0), t) };
        tl.range = Some((a, b.min(tl.duration)));
        self.anim.base = Some((self.doc_uid, self.doc.clone()));
    }

    pub fn anim_clear_points(&mut self) {
        self.checkpoint("in/out point");
        if let Some(tl) = &mut self.doc.animation {
            tl.range = None;
        }
        self.anim.base = Some((self.doc_uid, self.doc.clone()));
    }

    /// Copy the selected keys.
    pub fn anim_copy(&mut self) {
        let mut out = Vec::new();
        for (id, p, i) in &self.anim.selected {
            if let Some(k) = self.doc.get(*id).and_then(|s| s.anim.as_deref()).and_then(|a| a.track(*p)).and_then(|t| t.keys.get(*i)) {
                out.push((*id, *p, k.t, k.value.clone(), k.ease));
            }
        }
        let t0 = out.iter().map(|k| k.2).fold(f64::INFINITY, f64::min);
        for k in &mut out {
            k.2 -= t0;
        }
        if !out.is_empty() {
            let n = out.len();
            self.anim.clipboard = out;
            self.notify(format!("Copied {n} key{}", if n == 1 { "" } else { "s" }));
        }
    }

    /// Paste copied keys at the playhead: onto the selected layers when they
    /// came from one layer, else back onto the layers they came from.
    pub fn anim_paste(&mut self) {
        let clip = self.anim.clipboard.clone();
        if clip.is_empty() {
            return;
        }
        let t0 = self.anim_time();
        let one = clip.iter().all(|k| k.0 == clip[0].0);
        let targets: Vec<u64> = if one && !self.selection.is_empty() { self.selection.clone() } else { vec![] };
        let Some(duration) = self.doc.animation.as_ref().map(|a| a.duration) else { return };
        let mut placed: Vec<(u64, Prop, f64)> = Vec::new();
        self.anim_edit("paste keys", |doc| {
            for (src, p, dt, v, ease) in &clip {
                let ids = if targets.is_empty() { vec![*src] } else { targets.clone() };
                for id in ids {
                    let Some(mut s) = doc.get(id).cloned() else { continue };
                    let fits = match (p, v) {
                        (Prop::Transform, _) => true,
                        (Prop::Shape, Value::Shape(k)) => read(&s, Prop::Shape).is_some_and(|c| matches!(c, Value::Shape(c) if std::mem::discriminant(&*c) == std::mem::discriminant(&**k))),
                        _ => p.applies(&s),
                    };
                    let t = (t0 + dt).min(duration);
                    if !fits {
                        continue;
                    }
                    let tr = s.anim.get_or_insert_with(Default::default).track_mut(*p);
                    tr.set(t, v.clone());
                    if let Some(i) = tr.key_at(t) {
                        tr.keys[i].ease = *ease;
                    }
                    doc.replace(s);
                    placed.push((id, *p, t));
                }
            }
        });
        // Select what was pasted.
        self.anim.selected = placed
            .iter()
            .filter_map(|(id, p, t)| {
                let i = self.doc.get(*id)?.anim.as_deref()?.track(*p)?.key_at(*t)?;
                Some((*id, *p, i))
            })
            .collect();
        if placed.is_empty() {
            self.notify("The copied keys don't fit the selected layers");
        }
    }

    /// Key edits since the last sync and make the current document the new one.
    pub(crate) fn anim_sync(&mut self) {
        let uid = self.doc_uid;
        match &self.anim.base {
            Some((u, b)) if *u == uid && b.revision() == self.doc.revision() && b.animation == self.doc.animation => return,
            Some((u, b)) if *u == uid && self.doc.animation.is_some() => {
                let b = b.clone();
                capture_edits(&b, &mut self.doc, self.anim.record, &self.selection);
            }
            _ => {}
        }
        self.anim.base = Some((uid, self.doc.clone()));
    }

    pub fn anim_time(&self) -> f64 {
        self.doc.animation.as_ref().map_or(0.0, |a| a.time)
    }

    /// Move the playhead (edits made before are keyed first).
    pub fn anim_seek(&mut self, t: f64) {
        self.anim_sync();
        if self.doc.animation.is_none() {
            self.doc.animation = Some(Timeline::default());
        }
        seek(&mut self.doc, t);
        self.anim.base = Some((self.doc_uid, self.doc.clone()));
    }

    /// Change keys (undoable), keeping shapes in step.
    pub fn anim_edit(&mut self, label: &str, f: impl FnOnce(&mut Document)) {
        self.anim_sync();
        self.checkpoint(label);
        if self.doc.animation.is_none() {
            self.doc.animation = Some(Timeline::default());
        }
        edit_keys(&mut self.doc, f);
        self.anim.base = Some((self.doc_uid, self.doc.clone()));
    }

    /// [`App::anim_edit`] without its own undo step (the caller made one).
    pub fn anim_edit_quiet(&mut self, f: impl FnOnce(&mut Document)) {
        self.anim_sync();
        if self.doc.animation.is_none() {
            self.doc.animation = Some(Timeline::default());
        }
        edit_keys(&mut self.doc, f);
        self.anim.base = Some((self.doc_uid, self.doc.clone()));
    }

    /// Key `p` on `id` at the playhead with its current value.
    pub fn anim_toggle_key(&mut self, id: u64, p: Prop) {
        let t = self.anim_time();
        self.anim_edit("keyframe", |doc| key_current(doc, id, p, t));
        self.anim.selected.clear();
    }

    /// Delete the selected keys.
    pub fn anim_delete_selected(&mut self) {
        let sel = std::mem::take(&mut self.anim.selected);
        if sel.is_empty() {
            return;
        }
        self.anim_edit("delete keys", |doc| {
            for id in sel.iter().map(|k| k.0).collect::<std::collections::BTreeSet<_>>() {
                let Some(mut s) = doc.get(id).cloned() else { continue };
                if let Some(a) = s.anim.as_deref_mut() {
                    for tr in &mut a.tracks {
                        let mut i = 0;
                        tr.keys.retain(|_| {
                            let keep = !sel.contains(&(id, tr.prop, i));
                            i += 1;
                            keep
                        });
                    }
                }
                doc.replace(s);
            }
        });
    }

    /// Set the easing of the selected keys.
    pub fn anim_set_ease(&mut self, ease: Ease) {
        let sel = self.anim.selected.clone();
        self.anim_edit("easing", |doc| {
            for (id, p, i) in &sel {
                let Some(mut s) = doc.get(*id).cloned() else { continue };
                if let Some(k) = s.anim.as_deref_mut().and_then(|a| a.tracks.iter_mut().find(|t| t.prop == *p)).and_then(|t| t.keys.get_mut(*i)) {
                    k.ease = ease;
                }
                doc.replace(s);
            }
        });
    }

    /// Shift the selected keys by `dt` seconds (kept within the timeline).
    pub fn anim_move_selected(&mut self, dt: f64) {
        let sel = self.anim.selected.clone();
        let Some(tl) = self.doc.animation.clone() else { return };
        let mut moved: Vec<(u64, Prop, f64)> = Vec::new();
        self.anim_edit("move keys", |doc| {
            for id in sel.iter().map(|k| k.0).collect::<std::collections::BTreeSet<_>>() {
                let Some(mut s) = doc.get(id).cloned() else { continue };
                if let Some(a) = s.anim.as_deref_mut() {
                    for tr in &mut a.tracks {
                        let picked: Vec<usize> = sel.iter().filter(|k| k.0 == id && k.1 == tr.prop).map(|k| k.2).collect();
                        let mut keep = Vec::new();
                        let mut shifted = Vec::new();
                        for (i, k) in tr.keys.drain(..).enumerate() {
                            if picked.contains(&i) {
                                shifted.push(Key { t: tl.snap(k.t + dt), ..k });
                            } else {
                                keep.push(k);
                            }
                        }
                        // A moved key replaces one it lands on.
                        keep.retain(|k| !shifted.iter().any(|m| (m.t - k.t).abs() < 1.0 / 480.0));
                        for k in &shifted {
                            moved.push((id, tr.prop, k.t));
                        }
                        keep.extend(shifted);
                        tr.keys = keep;
                        tr.sort();
                    }
                }
                doc.replace(s);
            }
        });
        // Keep the same keys selected at their new indices.
        self.anim.selected = moved
            .into_iter()
            .filter_map(|(id, p, t)| {
                let i = self.doc.get(id)?.anim.as_deref()?.track(p)?.key_at(t)?;
                Some((id, p, i))
            })
            .collect();
    }
}
