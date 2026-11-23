//! Lottie (Bodymovin JSON) and dotLottie import.
//!
//! The animation is evaluated at every frame into plain shapes (layers as
//! groups, shape groups merged into paths that share their fill and
//! stroke), and [`crate::animation::from_frames`] turns those frames into
//! keys. Supported:
//! - layers: shape, solid, image, null, text and precomp; parenting;
//!   in/out points, time stretch and remapping; auto-orient;
//! - masks (add, subtract, intersect, difference, inverted, expanded) and
//!   track mattes (alpha and luma, inverted);
//! - shapes: paths, rectangles, ellipses, stars and polygons; fills,
//!   strokes (dashes) and gradient fills;
//! - modifiers: trim paths, repeaters, merge paths, round corners, offset
//!   paths and pucker & bloat;
//! - blend modes; drop shadow, Gaussian blur, Fill and Tint effects;
//! - text: drawn from the file's glyphs when it has them, with text
//!   animators per letter (position, scale, rotation, opacity, fill,
//!   tracking); otherwise editable text;
//! - expressions: `loopOut` / `loopIn` (cycle, ping-pong, offset, continue)
//!   and `wiggle`.
//!
//! Other expressions (they need a JavaScript engine and After Effects'
//! API) play the keyframes as stored; 3D layers are flattened.

use std::collections::HashMap;
use std::path::Path;

use kurbo::{Affine, BezPath, ParamCurve, ParamCurveArclen, PathEl, PathSeg, Point, Rect, Shape as _, Vec2};
use serde_json::Value as J;

use crate::document::{
    BlendMode, Document, FillRule, Mask, MaskMode, Gradient, GradientKind, GradientStop, ImageAsset, ImageFill, ImageMode, LineCap, LineJoin, Paint, Rgba, Shadow, Shape, ShapeKind, Stroke, StrokeAlign, TextAlign, TextData,
};

/// Looks like Lottie JSON (rather than any other JSON).
pub fn is_lottie(data: &[u8]) -> bool {
    if data.starts_with(b"PK") {
        return true;
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(4096)]);
    head.trim_start().starts_with('{') && head.contains("\"layers\"") || (head.contains("\"fr\"") && head.contains("\"op\""))
}

/// A Lottie JSON file or a dotLottie archive as a document. `dir`: where
/// external image files are looked up.
pub fn import(data: &[u8], dir: Option<&Path>) -> Result<Document, String> {
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let json: J = if data.starts_with(b"PK") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| format!("Not a dotLottie archive: {e}"))?;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf).map_err(|e| e.to_string())?;
            let name = f.name().map_err(|e| e.to_string())?.to_string();
            files.insert(name.trim_start_matches('/').to_string(), buf);
        }
        // The first animation the manifest lists, else any.
        let first = files
            .get("manifest.json")
            .and_then(|m| serde_json::from_slice::<J>(m).ok())
            .and_then(|m| m["animations"][0]["id"].as_str().map(|s| s.to_string()))
            .and_then(|id| ["animations/", "a/"].iter().map(|d| format!("{d}{id}.json")).find(|k| files.contains_key(k)));
        let key = first.or_else(|| {
            let mut keys: Vec<&String> = files.keys().filter(|k| (k.starts_with("animations/") || k.starts_with("a/")) && k.ends_with(".json")).collect();
            keys.sort();
            keys.first().map(|k| k.to_string())
        });
        let key = key.ok_or("The dotLottie archive has no animation")?;
        serde_json::from_slice(&files[&key]).map_err(|e| format!("Bad Lottie JSON: {e}"))?
    } else {
        serde_json::from_slice(data).map_err(|e| format!("Bad Lottie JSON: {e}"))?
    };
    if !json["layers"].is_array() {
        return Err("Not a Lottie animation (no layers)".into());
    }
    let mut doc = Document::default();
    FPS.with(|c| c.set(num(&json["fr"], 30.0).max(1.0)));
    let lot = Lottie::new(&json, &files, dir, &mut doc);
    let (w, h) = (num(&json["w"], 512.0), num(&json["h"], 512.0));
    let fr = num(&json["fr"], 30.0).max(1.0);
    let (ip, op) = (num(&json["ip"], 0.0), num(&json["op"], 60.0));
    let n = (op - ip).round().max(1.0) as usize;
    let frames: Vec<Vec<Shape>> = (0..=n)
        .map(|k| {
            // The out point is exclusive: the closing frame shows the layers
            // the last one does.
            let f = ip + k as f64;
            lot.closing.set(k == n);
            let mut ids = 0;
            let mut out = Vec::new();
            lot.comp(json["layers"].as_array().unwrap(), f, Affine::IDENTITY, &mut ids, &mut out);
            out
        })
        .collect();
    doc.set_size(w, h);
    doc.primary_mut().background = Rgba::WHITE.with_alpha(0.0);
    let animated = n > 0 && frames.windows(2).any(|f| f[0] != f[1]);
    let shapes = if animated { crate::animation::from_frames(&frames, fr) } else { frames.into_iter().next().unwrap_or_default() };
    // The document numbers every shape afresh (the importer's own ids
    // would clash with the ones it gives the layers).
    fn unnumber(s: &mut Shape) {
        s.id = 0;
        if let Some(c) = s.kind.children_mut() {
            c.iter_mut().for_each(unnumber);
        }
    }
    for mut s in shapes {
        unnumber(&mut s);
        doc.add_shape(s);
    }
    if animated {
        doc.animation = Some(crate::animation::Timeline { duration: n as f64 / fr, fps: fr, ..Default::default() });
    }
    Ok(doc)
}

fn num(v: &J, default: f64) -> f64 {
    v.as_f64().unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Animated properties
// ---------------------------------------------------------------------------

/// Keyframes of a property, if it has any.
fn keyframes(p: &J) -> Option<&Vec<J>> {
    let k = p["k"].as_array()?;
    (k.first().is_some_and(|f| f.is_object() && f.get("t").is_some())).then_some(k)
}

/// Numbers of a keyframe value (`s`/`e` may be a number or an array).
fn nums(v: &J) -> Vec<f64> {
    match v {
        J::Number(n) => vec![n.as_f64().unwrap_or(0.0)],
        J::Array(a) => a.iter().filter_map(|x| x.as_f64()).collect(),
        _ => Vec::new(),
    }
}

/// Easing handle component `d` (Lottie gives one per dimension or one for all).
fn handle(h: &J, axis: &str, d: usize, default: f64) -> f64 {
    match &h[axis] {
        J::Array(a) => a.get(d).or(a.last()).and_then(|x| x.as_f64()).unwrap_or(default),
        J::Number(n) => n.as_f64().unwrap_or(default),
        _ => default,
    }
}

/// The segment of `k` that frame `f` falls in: (start key, end value, eased
/// progress per dimension), or the held value.
enum Seg<'a> {
    Hold(&'a J),
    Blend(&'a J, &'a J, &'a J, f64),
}

fn segment(k: &[J], f: f64) -> Seg<'_> {
    let t = |i: usize| num(&k[i]["t"], 0.0);
    if f <= t(0) || k.len() == 1 {
        return Seg::Hold(&k[0]["s"]);
    }
    let i = (0..k.len()).rev().find(|i| t(*i) <= f).unwrap_or(0);
    let start = &k[i];
    // Old files: the last key has only a time; the one before has `e`.
    let end = if !start["e"].is_null() { &start["e"] } else { k.get(i + 1).map(|n| &n["s"]).unwrap_or(&J::Null) };
    if i + 1 >= k.len() || end.is_null() || start["s"].is_null() {
        let held = if start["s"].is_null() { k.get(i.wrapping_sub(1)).map(|p| if p["e"].is_null() { &p["s"] } else { &p["e"] }).unwrap_or(&start["s"]) } else { &start["s"] };
        return Seg::Hold(held);
    }
    if start["h"].as_i64() == Some(1) || start["h"].as_bool() == Some(true) {
        return Seg::Hold(&start["s"]);
    }
    let u = (f - t(i)) / (t(i + 1) - t(i)).max(1e-9);
    Seg::Blend(start, &start["s"], end, u.clamp(0.0, 1.0))
}

fn ease(key: &J, d: usize, u: f64) -> f64 {
    let (o, i) = (&key["o"], &key["i"]);
    if o.is_null() || i.is_null() {
        return u;
    }
    crate::animation::Ease::Bezier([handle(o, "x", d, 0.0), handle(o, "y", d, 0.0), handle(i, "x", d, 1.0), handle(i, "y", d, 1.0)]).apply(u)
}

thread_local! {
    /// Frame rate of the animation being read (for expressions in seconds).
    static FPS: std::cell::Cell<f64> = const { std::cell::Cell::new(30.0) };
}

/// The common expressions: `loopOut` / `loopIn` (cycle, pingpong, offset,
/// continue) and `wiggle`. Others are ignored (the keyframes play).
enum Expr {
    Loop { out: bool, mode: LoopMode, keys: usize },
    Wiggle { freq: f64, amp: f64 },
}

#[derive(Clone, Copy, PartialEq)]
enum LoopMode {
    Cycle,
    PingPong,
    Offset,
    Continue,
}

fn parse_expr(x: &str) -> Vec<Expr> {
    let mut out = Vec::new();
    let args = |at: usize| -> Vec<String> {
        let rest = &x[at..];
        let open = rest.find('(').map(|i| i + 1).unwrap_or(0);
        let close = rest[open..].find(')').map(|i| open + i).unwrap_or(rest.len());
        rest[open..close].split(',').map(|a| a.trim().trim_matches(|c| c == '\'' || c == '"').to_string()).filter(|a| !a.is_empty()).collect()
    };
    for (name, out_side) in [("loopOut", true), ("loopIn", false), ("loopOutDuration", true), ("loopInDuration", false)] {
        if let Some(at) = x.find(&format!("{name}(")) {
            if name.ends_with("Duration") || out.iter().any(|e| matches!(e, Expr::Loop { out, .. } if *out == out_side)) {
                continue;
            }
            let a = args(at);
            let mode = match a.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("pingpong") => LoopMode::PingPong,
                Some("offset") => LoopMode::Offset,
                Some("continue") => LoopMode::Continue,
                _ => LoopMode::Cycle,
            };
            let keys = a.get(1).and_then(|n| n.parse::<f64>().ok()).unwrap_or(0.0).max(0.0) as usize;
            out.push(Expr::Loop { out: out_side, mode, keys });
        }
    }
    if let Some(at) = x.find("wiggle(") {
        let a = args(at);
        let n = |i: usize| a.get(i).and_then(|v| v.parse::<f64>().ok());
        if let (Some(freq), Some(amp)) = (n(0), n(1)) {
            out.push(Expr::Wiggle { freq, amp });
        }
    }
    out
}

/// Smooth noise in -1..1, the same for the same seed and time.
fn noise(seed: u64, t: f64) -> f64 {
    let h = |i: i64| {
        let mut z = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) as f64 / u64::MAX as f64) * 2.0 - 1.0
    };
    let (i, u) = (t.floor() as i64, t - t.floor());
    let s = u * u * (3.0 - 2.0 * u);
    h(i) + (h(i + 1) - h(i)) * s
}

/// How a looping expression maps frame `f`.
struct Looped {
    /// The frame on the keyed range to read.
    at: f64,
    /// `offset`: whole cycles to add the range's change for, over `a..b`.
    offset: Option<(f64, f64, f64)>,
    /// `continue`: frames past the end, to extrapolate by.
    past: f64,
}

fn loop_frame(k: &[J], f: f64, exprs: &[Expr]) -> Looped {
    let t = |i: usize| num(&k[i]["t"], 0.0);
    let n = k.len();
    let (first, last) = (t(0), t(n - 1));
    for e in exprs {
        let Expr::Loop { out, mode, keys } = e else { continue };
        if (*out && f <= last) || (!*out && f >= first) {
            continue;
        }
        if *mode == LoopMode::Continue {
            let end = if *out { last } else { first };
            return Looped { at: end, offset: None, past: f - end };
        }
        let (a, b) = match (*out, *keys) {
            (true, 0) => (first, last),
            (true, c) => (t(n.saturating_sub(1 + c)), last),
            (false, 0) => (first, last),
            (false, c) => (first, t(c.min(n - 1))),
        };
        let period = b - a;
        if period <= 1e-9 {
            continue;
        }
        let d = f - a;
        let cycles = (d / period).floor();
        let r = d - cycles * period;
        let at = match mode {
            LoopMode::PingPong if (cycles as i64).rem_euclid(2) == 1 => b - r,
            _ => a + r,
        };
        let offset = (*mode == LoopMode::Offset).then_some((cycles, a, b));
        return Looped { at, offset, past: 0.0 };
    }
    Looped { at: f, offset: None, past: 0.0 }
}

/// A numeric property at frame `f` (empty if missing).
fn vec_at(p: &J, f: f64) -> Vec<f64> {
    let Some(x) = p.get("x").and_then(|x| x.as_str()) else { return vec_raw(p, f) };
    let exprs = parse_expr(x);
    if exprs.is_empty() {
        return vec_raw(p, f);
    }
    let mut v = match keyframes(p) {
        Some(k) if !k.is_empty() => {
            let l = loop_frame(k, f, &exprs);
            let mut v = vec_raw(p, l.at);
            if let Some((cycles, a, b)) = l.offset {
                let (s, e) = (vec_raw(p, a), vec_raw(p, b));
                for (d, x) in v.iter_mut().enumerate() {
                    *x += (e.get(d).copied().unwrap_or(0.0) - s.get(d).copied().unwrap_or(0.0)) * cycles;
                }
            }
            if l.past != 0.0 {
                // Keep going at the speed the keys end (or start) with.
                let near = vec_raw(p, l.at - l.past.signum());
                for (d, x) in v.iter_mut().enumerate() {
                    // `near` is one frame inside the range, so this extends outward.
                    *x += (*x - near.get(d).copied().unwrap_or(*x)) * l.past.abs();
                }
            }
            v
        }
        _ => vec_raw(p, f),
    };
    for e in &exprs {
        if let Expr::Wiggle { freq, amp } = e {
            let secs = f / FPS.with(|c| c.get());
            let seed = x.bytes().fold(1469598103934665603u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211));
            for (d, x) in v.iter_mut().enumerate() {
                *x += amp * noise(seed.wrapping_add(d as u64 * 7919), secs * freq);
            }
        }
    }
    v
}

fn vec_raw(p: &J, f: f64) -> Vec<f64> {
    if p.is_null() {
        return Vec::new();
    }
    // Position split into x and y.
    if p["s"].as_bool() == Some(true) || (p.get("x").is_some_and(|x| x.is_object()) && p.get("k").is_none()) {
        return vec![scalar(&p["x"], f, 0.0), scalar(&p["y"], f, 0.0)];
    }
    let Some(k) = keyframes(p) else { return nums(&p["k"]) };
    match segment(k, f) {
        Seg::Hold(v) => nums(v),
        Seg::Blend(key, a, b, u) => {
            let (a, b) = (nums(a), nums(b));
            let mut out: Vec<f64> = a.iter().enumerate().map(|(d, x)| x + (b.get(d).copied().unwrap_or(*x) - x) * ease(key, d, u)).collect();
            // Spatial tangents: the value follows a curve.
            let (to, ti) = (nums(&key["to"]), nums(&key["ti"]));
            if to.len() >= 2 && ti.len() >= 2 && a.len() >= 2 && b.len() >= 2 && (to.iter().chain(&ti).any(|v| v.abs() > 1e-9)) {
                let s = ease(key, 0, u);
                let c = kurbo::CubicBez::new((a[0], a[1]), (a[0] + to[0], a[1] + to[1]), (b[0] + ti[0], b[1] + ti[1]), (b[0], b[1]));
                // Progress is distance along the curve (as lottie-web).
                let len = kurbo::ParamCurveArclen::arclen(&c, 1e-3);
                let q = if len > 1e-9 { c.eval(kurbo::ParamCurveArclen::inv_arclen(&c, s.clamp(0.0, 1.0) * len, 1e-3)) } else { c.eval(s) };
                out[0] = q.x;
                out[1] = q.y;
            }
            out
        }
    }
}

fn scalar(p: &J, f: f64, default: f64) -> f64 {
    vec_at(p, f).first().copied().unwrap_or(default)
}

fn point(p: &J, f: f64, default: Point) -> Point {
    let v = vec_at(p, f);
    Point::new(v.first().copied().unwrap_or(default.x), v.get(1).copied().unwrap_or(default.y))
}

/// A Lottie path: vertices with in and out tangents (relative), closed.
#[derive(Clone, Default)]
struct LPath {
    v: Vec<Point>,
    i: Vec<Vec2>,
    o: Vec<Vec2>,
    c: bool,
}

fn lpath(v: &J) -> LPath {
    // Keyframe values wrap the path in an array.
    let v = if v.is_array() { &v[0] } else { v };
    let pts = |k: &str| -> Vec<Point> { v[k].as_array().map(|a| a.iter().map(|p| Point::new(num(&p[0], 0.0), num(&p[1], 0.0))).collect()).unwrap_or_default() };
    let n = pts("v");
    let tan = |k: &str| -> Vec<Vec2> {
        let mut t: Vec<Vec2> = pts(k).into_iter().map(|p| p.to_vec2()).collect();
        t.resize(n.len(), Vec2::ZERO);
        t
    };
    LPath { i: tan("i"), o: tan("o"), v: n, c: v["c"].as_bool().unwrap_or(false) }
}

fn path_at(p: &J, f: f64) -> LPath {
    let Some(k) = keyframes(p) else { return lpath(&p["k"]) };
    // Looping path animations (cycle and ping-pong).
    let f = match p.get("x").and_then(|x| x.as_str()).map(parse_expr) {
        Some(e) if !e.is_empty() && !k.is_empty() => loop_frame(k, f, &e).at,
        _ => f,
    };
    match segment(k, f) {
        Seg::Hold(v) => lpath(v),
        Seg::Blend(key, a, b, u) => {
            let (a, b) = (lpath(a), lpath(b));
            if a.v.len() != b.v.len() {
                return if u < 1.0 { a } else { b };
            }
            let u = ease(key, 0, u);
            let lp = |x: Point, y: Point| x.lerp(y, u);
            let lv = |x: Vec2, y: Vec2| x.lerp(y, u);
            LPath {
                v: a.v.iter().zip(&b.v).map(|(x, y)| lp(*x, *y)).collect(),
                i: a.i.iter().zip(&b.i).map(|(x, y)| lv(*x, *y)).collect(),
                o: a.o.iter().zip(&b.o).map(|(x, y)| lv(*x, *y)).collect(),
                c: a.c,
            }
        }
    }
}

fn to_bez(p: &LPath) -> BezPath {
    let mut b = BezPath::new();
    let Some(first) = p.v.first() else { return b };
    b.move_to(*first);
    let n = p.v.len();
    let seg = |b: &mut BezPath, j: usize, k: usize| {
        let (c1, c2) = (p.v[j] + p.o[j], p.v[k] + p.i[k]);
        if p.o[j].hypot2() < 1e-18 && p.i[k].hypot2() < 1e-18 {
            b.line_to(p.v[k]);
        } else {
            b.curve_to(c1, c2, p.v[k]);
        }
    };
    for k in 1..n {
        seg(&mut b, k - 1, k);
    }
    if p.c && n > 1 {
        seg(&mut b, n - 1, 0);
        b.close_path();
    }
    b
}

/// A layer or group transform at `f`, and its opacity (0..1).
fn transform(ks: &J, f: f64) -> (Affine, f64) {
    transform_turned(ks, f, 0.0)
}

/// A layer's transform, turned along its motion path when it auto-orients.
fn layer_transform(l: &J, f: f64) -> (Affine, f64) {
    let ks = &l["ks"];
    if l["ao"].as_i64() != Some(1) {
        return transform(ks, f);
    }
    let (a, b) = (point(&ks["p"], f - 0.05, Point::ZERO), point(&ks["p"], f + 0.05, Point::ZERO));
    let turn = if a.distance(b) > 1e-9 { (b.y - a.y).atan2(b.x - a.x).to_degrees() } else {
        // Standing still: keep facing the way it last moved.
        let (a, b) = (point(&ks["p"], f - 1.0, Point::ZERO), point(&ks["p"], f, Point::ZERO));
        if a.distance(b) > 1e-9 { (b.y - a.y).atan2(b.x - a.x).to_degrees() } else { 0.0 }
    };
    transform_turned(ks, f, turn)
}

fn transform_turned(ks: &J, f: f64, turn: f64) -> (Affine, f64) {
    if ks.is_null() {
        return (Affine::IDENTITY, 1.0);
    }
    let a = point(&ks["a"], f, Point::ZERO);
    let p = point(&ks["p"], f, Point::ZERO);
    let s = vec_at(&ks["s"], f);
    let (sx, sy) = (s.first().copied().unwrap_or(100.0) / 100.0, s.get(1).copied().unwrap_or(s.first().copied().unwrap_or(100.0)) / 100.0);
    let r = turn + if ks["r"].is_null() { scalar(&ks["rz"], f, 0.0) } else { scalar(&ks["r"], f, 0.0) };
    let sk = scalar(&ks["sk"], f, 0.0);
    let skew = if sk.abs() > 1e-9 {
        let sa = scalar(&ks["sa"], f, 0.0).to_radians();
        Affine::rotate(sa) * Affine::new([1.0, 0.0, (-sk.to_radians()).tan(), 1.0, 0.0, 0.0]) * Affine::rotate(-sa)
    } else {
        Affine::IDENTITY
    };
    let m = Affine::translate(p.to_vec2()) * Affine::rotate(r.to_radians()) * skew * Affine::scale_non_uniform(sx, sy) * Affine::translate(-a.to_vec2());
    (m, scalar(&ks["o"], f, 100.0) / 100.0)
}

fn color(v: &[f64], alpha: f64) -> Rgba {
    // Very old files use 0..255.
    let k = if v.iter().take(3).any(|x| *x > 1.0) { 255.0 } else { 1.0 };
    let c = |i: usize| (v.get(i).copied().unwrap_or(0.0) / k).clamp(0.0, 1.0) as f32;
    // A fourth (alpha) value is ignored, as players do: opacity is its own
    // property.
    Rgba { r: c(0), g: c(1), b: c(2), a: alpha.clamp(0.0, 1.0) as f32 }
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

struct Lottie<'a> {
    assets: HashMap<String, &'a J>,
    images: HashMap<String, u64>,
    fonts: HashMap<String, (String, u16, bool)>,
    /// Font name → (family, style, ascent) as the glyphs are keyed.
    font_keys: HashMap<String, (String, String, f64)>,
    /// Glyph outlines: (character, family, style) → (advance, shapes), at
    /// size 100.
    glyphs: HashMap<(String, String, String), (f64, &'a J)>,
    fr: f64,
    /// Evaluating the closing frame (at the out point).
    closing: std::cell::Cell<bool>,
}

/// A fill or stroke and where it sits in its group.
#[derive(Clone)]
enum Style {
    Fill(Paint, FillRule),
    Stroke(Stroke),
}

/// Path modifiers, applied in order to the paths before them in a group
/// (and then the enclosing groups'). Lengths are in document units.
#[derive(Clone, Copy)]
enum Modifier {
    Trim(Trim),
    Round(f64),
    Offset { d: f64, round: bool },
    /// Pucker (negative) and bloat (positive), as a fraction.
    Pucker(f64),
}

impl Modifier {
    fn apply(&self, p: BezPath) -> BezPath {
        match *self {
            Modifier::Trim(t) => trim_path(&p, t),
            Modifier::Round(r) => round_corners(&p, r),
            Modifier::Offset { d, round } => {
                if d.abs() < 1e-9 {
                    return p;
                }
                crate::path_ops::offset_path(&p, FillRule::NonZero, d, round).map(|pd| crate::geom::pathdata_to_bez(&pd)).unwrap_or(p)
            }
            Modifier::Pucker(a) => pucker(&p, a),
        }
    }
}

/// Trim paths: start, end (0..1), offset (turns), each path on its own.
#[derive(Clone, Copy)]
struct Trim {
    s: f64,
    e: f64,
    o: f64,
    each: bool,
}

impl<'a> Lottie<'a> {
    fn new(json: &'a J, files: &HashMap<String, Vec<u8>>, dir: Option<&Path>, doc: &mut Document) -> Self {
        let mut assets = HashMap::new();
        let mut images = HashMap::new();
        for a in json["assets"].as_array().into_iter().flatten() {
            let Some(id) = a["id"].as_str() else { continue };
            assets.insert(id.to_string(), a);
            let Some(p) = a["p"].as_str() else { continue };
            if a["layers"].is_array() {
                continue;
            }
            let bytes = if let Some(b64) = p.strip_prefix("data:").and_then(|d| d.split_once(";base64,")).map(|(_, b)| b) {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()
            } else {
                let u = a["u"].as_str().unwrap_or("").trim_start_matches('/');
                let name = format!("{u}{p}");
                files.get(&name).or_else(|| files.get(&format!("images/{p}"))).or_else(|| files.get(&format!("i/{p}"))).cloned().or_else(|| dir.and_then(|d| std::fs::read(d.join(&name)).ok()))
            };
            if let Some(asset) = bytes.and_then(|b| ImageAsset::from_encoded(b).ok()) {
                images.insert(id.to_string(), doc.add_image(asset));
            }
        }
        let mut fonts = HashMap::new();
        for f in json["fonts"]["list"].as_array().into_iter().flatten() {
            let Some(name) = f["fName"].as_str() else { continue };
            let style = f["fStyle"].as_str().unwrap_or("").to_ascii_lowercase();
            let weight = f["fWeight"].as_str().and_then(|w| w.parse().ok()).unwrap_or(if style.contains("bold") { 700 } else if style.contains("light") { 300 } else if style.contains("medium") { 500 } else { 400 });
            fonts.insert(name.to_string(), (f["fFamily"].as_str().unwrap_or(name).to_string(), weight, style.contains("italic")));
        }
        let mut font_keys = HashMap::new();
        for f in json["fonts"]["list"].as_array().into_iter().flatten() {
            let Some(name) = f["fName"].as_str() else { continue };
            font_keys.insert(name.to_string(), (f["fFamily"].as_str().unwrap_or("").to_string(), f["fStyle"].as_str().unwrap_or("").to_string(), num(&f["ascent"], 75.0)));
        }
        let mut glyphs = HashMap::new();
        for c in json["chars"].as_array().into_iter().flatten() {
            let (Some(ch), Some(fam)) = (c["ch"].as_str(), c["fFamily"].as_str()) else { continue };
            glyphs.insert((ch.to_string(), fam.to_string(), c["style"].as_str().unwrap_or("").to_string()), (num(&c["w"], 0.0), &c["data"]["shapes"]));
        }
        Lottie { assets, images, fonts, font_keys, glyphs, fr: num(&json["fr"], 30.0).max(1.0), closing: std::cell::Cell::new(false) }
    }

    /// The layers of a composition at frame `f`, bottom first, under `base`.
    fn comp(&self, layers: &[J], f: f64, base: Affine, ids: &mut u64, out: &mut Vec<Shape>) {
        let by_ind: HashMap<i64, &J> = layers.iter().filter_map(|l| l["ind"].as_i64().map(|i| (i, l))).collect();
        // Top layer first in the file.
        for (idx, l) in layers.iter().enumerate().rev() {
            // Hidden layers, and matte sources (drawn as their layer's mask).
            if l["hd"].as_bool() == Some(true) || l["td"].as_i64().unwrap_or(0) > 0 {
                continue;
            }
            let Some(mut g) = self.layer(l, f, base, &by_ind, ids) else { continue };
            // Track matte: the layer above (or the one `tp` names).
            let tt = l["tt"].as_i64().unwrap_or(0);
            if tt > 0 {
                let src = l["tp"].as_i64().and_then(|i| by_ind.get(&i).copied()).or_else(|| idx.checked_sub(1).map(|i| &layers[i]));
                if let Some(mut m) = src.and_then(|src| self.layer(src, f, base, &by_ind, ids)) {
                    // The matte counts where it's shown, even if hidden.
                    let shown = m.visible;
                    m.visible = true;
                    if !shown {
                        m.opacity = 0.0;
                    }
                    g.mask = Some(Box::new(Mask { shape: m, mode: if tt >= 3 { MaskMode::Luminance } else { MaskMode::Alpha }, invert: tt == 2 || tt == 4 }));
                }
            }
            out.push(g);
        }
    }

    /// One layer as a group (`None` for nulls, cameras, audio…).
    fn layer(&self, l: &J, f: f64, base: Affine, by_ind: &HashMap<i64, &J>, ids: &mut u64) -> Option<Shape> {
        let ty = l["ty"].as_i64().unwrap_or(-1);
        if !matches!(ty, 0 | 1 | 2 | 4 | 5) {
            return None;
        }
        let st = num(&l["st"], 0.0);
        // Keyframe times are in the composition's time (as lottie-web reads
        // them); the start time only shifts a precomp's contents.
        let (own, opacity) = layer_transform(l, f);
        // Parents' transforms (their opacity doesn't carry over).
        let mut m = own;
        let mut parent = l["parent"].as_i64();
        let mut guard = 0;
        while let Some(p) = parent.and_then(|i| by_ind.get(&i)) {
            m = layer_transform(p, f).0 * m;
            parent = p["parent"].as_i64();
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        let m = base * m;
        *ids += 1;
        let mut g = Shape::new(ShapeKind::Group { children: Vec::new() });
        g.id = *ids;
        g.fill = None;
        g.name = l["nm"].as_str().unwrap_or("Layer").to_string();
        g.opacity = opacity.clamp(0.0, 1.0) as f32;
        let fv = if self.closing.get() { f - 1e-3 } else { f };
        g.visible = fv >= num(&l["ip"], f64::MIN) && fv < num(&l["op"], f64::MAX);
        g.blend = l["bm"].as_u64().and_then(|b| BlendMode::ALL.get(b as usize).copied()).unwrap_or_default();
        self.effects(l, f, m, &mut g);
        let clip = masks(l, f, m);
        let mut children = Vec::new();
        match ty {
            0 => {
                if let Some(sub) = l["refId"].as_str().and_then(|r| self.assets.get(r)).and_then(|a| a["layers"].as_array()) {
                    let sr = num(&l["sr"], 1.0).max(1e-6);
                    let inner = if l["tm"].is_null() { (f - st) / sr } else { scalar(&l["tm"], f, 0.0) * self.fr };
                    self.comp(sub, inner, m, ids, &mut children);
                }
            }
            1 => {
                let (w, h) = (num(&l["sw"], 0.0), num(&l["sh"], 0.0));
                let mut s = self.leaf(ids, m * Rect::new(0.0, 0.0, w, h).to_path(0.1));
                s.name = "Solid".into();
                s.fill = Some(Paint::Solid(l["sc"].as_str().and_then(Rgba::from_hex).unwrap_or(Rgba::BLACK)));
                children.push(s);
            }
            2 => {
                let asset = l["refId"].as_str().and_then(|r| self.assets.get(r));
                let (w, h) = asset.map(|a| (num(&a["w"], 100.0), num(&a["h"], 100.0))).unwrap_or((100.0, 100.0));
                *ids += 1;
                let mut s = Shape::new(ShapeKind::Rect { w, h, radii: [0.0; 4] });
                s.id = *ids;
                s.name = "Image".into();
                s.transform = m;
                s.fill = l["refId"].as_str().and_then(|r| self.images.get(r)).map(|img| Paint::Image(ImageFill { image: *img, mode: ImageMode::Stretch, scale: 1.0, opacity: 1.0 }));
                children.push(s);
            }
            4 => self.items(l["shapes"].as_array().map(|v| v.as_slice()).unwrap_or(&[]), f, m, 1.0, &[], &[], ids, &mut children),
            5 => {
                if let Some(t) = self.text(l, f, m, ids) {
                    children.push(t);
                }
            }
            _ => {}
        }
        // Groups don't paint: the layer's mask clips every shape in it.
        if let Some(c) = &clip {
            for ch in &mut children {
                clip_leaves(ch, c);
            }
        }
        g.kind = ShapeKind::Group { children };
        self.color_effects(l, f, &mut g);
        Some(g)
    }

    /// Fill and Tint effects, which recolour the layer.
    fn color_effects(&self, l: &J, f: f64, g: &mut Shape) {
        for e in l["ef"].as_array().into_iter().flatten() {
            if e["en"].as_i64() == Some(0) {
                continue;
            }
            let param = |i: usize| &e["ef"][i]["v"];
            match e["ty"].as_i64() {
                Some(21) => {
                    let c = color(&vec_at(param(2), f), 1.0);
                    let o = scalar(param(6), f, 1.0).clamp(0.0, 1.0) as f32;
                    recolor(g, &|x: Rgba| Rgba { r: x.r + (c.r - x.r) * o, g: x.g + (c.g - x.g) * o, b: x.b + (c.b - x.b) * o, a: x.a });
                }
                Some(20) => {
                    let (black, white) = (color(&vec_at(param(0), f), 1.0), color(&vec_at(param(1), f), 1.0));
                    let amount = (scalar(param(2), f, 100.0) / 100.0).clamp(0.0, 1.0) as f32;
                    recolor(g, &|x: Rgba| {
                        let lum = 0.2126 * x.r + 0.7152 * x.g + 0.0722 * x.b;
                        let t = |a: f32, b: f32, c: f32| c + (a + (b - a) * lum - c) * amount;
                        Rgba { r: t(black.r, white.r, x.r), g: t(black.g, white.g, x.g), b: t(black.b, white.b, x.b), a: x.a }
                    });
                }
                _ => {}
            }
        }
    }

    /// Drop shadow and Gaussian blur layer effects.
    fn effects(&self, l: &J, f: f64, m: Affine, g: &mut Shape) {
        let k = m.determinant().abs().sqrt();
        for e in l["ef"].as_array().into_iter().flatten() {
            if e["en"].as_i64() == Some(0) {
                continue;
            }
            let param = |i: usize| &e["ef"][i]["v"];
            match e["ty"].as_i64() {
                Some(25) => {
                    let c = vec_at(param(0), f);
                    let alpha = scalar(param(1), f, 127.5) / 255.0;
                    let dir = scalar(param(2), f, 135.0).to_radians();
                    let dist = scalar(param(3), f, 5.0) * k;
                    let soft = scalar(param(4), f, 0.0) * k;
                    let mut color = color(&c, alpha);
                    color.a = alpha as f32;
                    g.shadow = Some(Shadow { color, dx: dist * dir.sin(), dy: -dist * dir.cos(), blur: soft / 2.0 });
                }
                Some(29) => g.blur = scalar(param(0), f, 0.0) * 0.6 * k,
                _ => {}
            }
        }
    }

    fn leaf(&self, ids: &mut u64, path: BezPath) -> Shape {
        *ids += 1;
        let mut s = Shape::new(ShapeKind::Path(crate::geom::bez_to_pathdata(&path)));
        s.id = *ids;
        s.fill = None;
        s
    }

    /// Shape items of a group under `m`, bottom first. `inherited`: the
    /// fills and strokes of enclosing groups that apply to these paths
    /// (topmost first); `opacity`: the groups' opacity.
    #[allow(clippy::too_many_arguments)]
    fn items(&self, items: &[J], f: f64, m: Affine, opacity: f64, inherited: &[Style], inherited_mods: &[Modifier], ids: &mut u64, out: &mut Vec<Shape>) {
        let expanded;
        let items = if items.iter().any(|i| i["ty"] == "rp" && i["hd"].as_bool() != Some(true)) {
            expanded = repeat(items, f);
            expanded.as_slice()
        } else {
            items
        };
        let tr = items.iter().find(|i| i["ty"] == "tr");
        let (gm, go) = match tr {
            Some(t) if t["_m"].is_array() => {
                let v: Vec<f64> = t["_m"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap_or(0.0)).collect();
                (Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]), num(&t["_o"], 1.0))
            }
            Some(t) => transform(t, f),
            None => (Affine::IDENTITY, 1.0),
        };
        let (m, opacity) = (m * gm, opacity * go);
        let k = m.determinant().abs().sqrt();
        let live: Vec<(usize, &J)> = items.iter().enumerate().filter(|(_, i)| i["hd"].as_bool() != Some(true)).collect();
        // Styles at their index.
        let mut styles: Vec<(usize, Style)> = Vec::new();
        let mut mods: Vec<(usize, Modifier)> = Vec::new();
        for (i, it) in &live {
            match it["ty"].as_str().unwrap_or("") {
                "fl" => {
                    let c = color(&vec_at(&it["c"], f), scalar(&it["o"], f, 100.0) / 100.0 * opacity);
                    styles.push((*i, Style::Fill(Paint::Solid(c), if it["r"].as_i64() == Some(2) { FillRule::EvenOdd } else { FillRule::NonZero })));
                }
                "gf" => {
                    if let Some((mut g,)) = gradient(it, f, opacity) {
                        for q in [&mut g.start, &mut g.end] {
                            let d = m * Point::new(q[0], q[1]);
                            *q = [d.x, d.y];
                        }
                        styles.push((*i, Style::Fill(Paint::Gradient(g), if it["r"].as_i64() == Some(2) { FillRule::EvenOdd } else { FillRule::NonZero })));
                    }
                }
                "st" | "gs" => {
                    let a = scalar(&it["o"], f, 100.0) / 100.0 * opacity;
                    let c = if it["ty"] == "gs" { gradient(it, f, opacity).and_then(|g| g.0.stops.first().map(|s| s.color)).unwrap_or(Rgba::BLACK) } else { color(&vec_at(&it["c"], f), a) };
                    let w = scalar(&it["w"], f, 1.0) * k;
                    let mut st = Stroke { color: c, width: w, ..Stroke::default() };
                    st.cap = match it["lc"].as_i64() {
                        Some(2) => LineCap::Round,
                        Some(3) => LineCap::Square,
                        _ => LineCap::Butt,
                    };
                    st.join = match it["lj"].as_i64() {
                        Some(2) => LineJoin::Round,
                        Some(3) => LineJoin::Bevel,
                        _ => LineJoin::Miter,
                    };
                    st.miter_limit = num(&it["ml"], 4.0);
                    // Dashes in multiples of the width.
                    for d in it["d"].as_array().into_iter().flatten() {
                        let v = scalar(&d["v"], f, 0.0) * k / w.max(1e-9);
                        match d["n"].as_str() {
                            Some("o") => st.dash_offset = v,
                            Some(_) => st.dash.push(v),
                            None => {}
                        }
                    }
                    if st.dash.len() % 2 == 1 {
                        let d = st.dash.clone();
                        st.dash.extend(d);
                    }
                    styles.push((*i, Style::Stroke(st)));
                }
                "tm" => mods.push((
                    *i,
                    Modifier::Trim(Trim { s: scalar(&it["s"], f, 0.0) / 100.0, e: scalar(&it["e"], f, 100.0) / 100.0, o: scalar(&it["o"], f, 0.0) / 360.0, each: it["m"].as_i64() != Some(2) }),
                )),
                "rd" => mods.push((*i, Modifier::Round(scalar(&it["r"], f, 0.0).max(0.0) * k))),
                "op" => mods.push((*i, Modifier::Offset { d: scalar(&it["a"], f, 0.0) * k, round: it["lj"].as_i64() == Some(2) })),
                "pb" => mods.push((*i, Modifier::Pucker(scalar(&it["a"], f, 0.0) / 100.0))),
                _ => {}
            }
        }
        // What applies to an item at index i: later styles here, then the
        // enclosing groups'.
        let applying = |i: usize| -> Vec<Style> { styles.iter().filter(|(j, _)| *j > i).map(|(_, s)| s.clone()).chain(inherited.iter().cloned()).collect() };
        let mods_at = |i: usize| -> Vec<Modifier> { mods.iter().filter(|(j, _)| *j > i).map(|(_, m)| *m).chain(inherited_mods.iter().copied()).collect() };
        // Merge paths: the geometry before the last one becomes one path.
        let merge = live.iter().rev().find(|(_, it)| it["ty"] == "mm").map(|(i, it)| (*i, *it));
        // Bottom first; neighbouring paths with the same styles merge.
        let mut run: Option<(BezPath, usize)> = None;
        let flush = |run: &mut Option<(BezPath, usize)>, out: &mut Vec<Shape>, ids: &mut u64| {
            if let Some((p, at)) = run.take() {
                let p = mods_at(at).iter().fold(p, |p, m| m.apply(p));
                if let Some(s) = self.styled(p, &applying(at), ids) {
                    out.push(s);
                }
            }
        };
        for (i, it) in live.iter().rev() {
            if merge.is_some_and(|(at, _)| *i < at) {
                continue;
            }
            let geom = match it["ty"].as_str().unwrap_or("") {
                "mm" => {
                    let before: Vec<J> = live.iter().filter(|(j, _)| j < i).map(|(_, it)| (*it).clone()).collect();
                    Some(merged(&before, f, it["mm"].as_i64().unwrap_or(1)))
                }
                "sh" | "rc" | "el" | "sr" => primitive(it, f),
                "gr" => {
                    flush(&mut run, out, ids);
                    let inner: Vec<Style> = applying(*i);
                    let sub = it["it"].as_array().map(|v| v.as_slice()).unwrap_or(&[]);
                    *ids += 1;
                    let mut g = Shape::new(ShapeKind::Group { children: Vec::new() });
                    g.id = *ids;
                    g.fill = None;
                    g.name = it["nm"].as_str().unwrap_or("Group").to_string();
                    let mut children = Vec::new();
                    self.items(sub, f, m, opacity, &inner, &mods_at(*i), ids, &mut children);
                    // A group of one is just its member.
                    if children.len() == 1 {
                        out.push(children.pop().unwrap());
                    } else {
                        g.kind = ShapeKind::Group { children };
                        out.push(g);
                    }
                    None
                }
                _ => None,
            };
            let Some(p) = geom else { continue };
            let p = m * p;
            let key = |at: usize| {
                let after = |j: usize| j > at;
                (styles.iter().filter(|(j, _)| after(*j)).map(|(j, _)| *j).collect::<Vec<_>>(), mods.iter().filter(|(j, _)| after(*j)).map(|(j, _)| *j).collect::<Vec<_>>())
            };
            // Modifiers other than "trim everything together" work per shape.
            let joinable = !mods_at(*i).iter().any(|m| matches!(m, Modifier::Offset { .. }));
            match &mut run {
                Some((path, at)) if key(*at) == key(*i) && joinable => path.extend(p),
                _ => {
                    flush(&mut run, out, ids);
                    run = Some((p, *i));
                }
            }
        }
        flush(&mut run, out, ids);
    }

    /// A path with its fills and strokes (topmost first).
    fn styled(&self, p: BezPath, styles: &[Style], ids: &mut u64) -> Option<Shape> {
        let fills: Vec<(&Paint, FillRule, usize)> = styles.iter().enumerate().filter_map(|(k, s)| if let Style::Fill(p, r) = s { Some((p, *r, k)) } else { None }).collect();
        let strokes: Vec<(&Stroke, usize)> = styles.iter().enumerate().filter_map(|(k, s)| if let Style::Stroke(st) = s { Some((st, k)) } else { None }).collect();
        if fills.is_empty() && strokes.is_empty() {
            return None;
        }
        let mut s = self.leaf(ids, p);
        s.name = "Path".into();
        // Bottom fill first; the others paint over it.
        let mut f = fills.iter().rev();
        if let Some((paint, rule, _)) = f.next() {
            s.fill = Some(fixed(paint, &s));
            s.fill_rule = *rule;
        }
        s.fills = f.map(|(p, ..)| fixed(p, &s)).collect();
        if let Some((st, k)) = strokes.first() {
            let mut st = (*st).clone();
            // A fill over the stroke hides its inner half.
            if fills.first().is_some_and(|(_, _, fk)| fk < k) {
                st.align = StrokeAlign::Outside;
                st.width /= 2.0;
            }
            s.stroke = Some(st);
            s.strokes = strokes[1..].iter().rev().map(|(st, _)| (*st).clone()).collect();
        }
        Some(s)
    }

    fn text(&self, l: &J, f: f64, m: Affine, ids: &mut u64) -> Option<Shape> {
        let k = &l["t"]["d"]["k"];
        let doc = match k.as_array() {
            Some(keys) => keys.iter().rev().find(|d| num(&d["t"], 0.0) <= f).or(keys.first()).map(|d| &d["s"])?,
            None => &k["s"],
        };
        let content = doc["t"].as_str()?.replace(['\r', '\u{3}'], "\n");
        let size = num(&doc["s"], 24.0);
        if let Some(g) = self.glyph_text(l, doc, &content, f, m, ids) {
            return Some(g);
        }
        let anim = animators(l, &content, f, color(&nums(&doc["fc"]), 1.0));
        let (family, weight, italic) = doc["f"].as_str().and_then(|n| self.fonts.get(n)).cloned().unwrap_or(("sans-serif".into(), 400, false));
        let align = match doc["j"].as_i64() {
            Some(1) => TextAlign::Right,
            Some(2) => TextAlign::Center,
            _ => TextAlign::Left,
        };
        // Without outlines the text stays editable: animators that move
        // every letter alike apply to the whole, and letters hidden from
        // the end type on.
        let shown = anim.iter().rposition(|a| a.opacity > 0.5).map(|i| i + 1).unwrap_or(0);
        let content = if !anim.is_empty() && anim.iter().take(shown).all(|a| a.opacity > 0.5) {
            let mut n = 0;
            let mut cut = String::new();
            for c in content.chars() {
                if n == shown {
                    break;
                }
                if c != '\n' {
                    n += 1;
                }
                cut.push(c);
            }
            cut
        } else {
            content
        };
        let whole = anim.first().filter(|a0| anim.iter().all(|a| a.same(a0))).cloned();
        let td = TextData {
            content,
            family,
            weight,
            italic,
            size,
            letter_spacing: num(&doc["tr"], 0.0) * size / 1000.0,
            line_height: (num(&doc["lh"], size * 1.2) / size.max(1e-9)).clamp(0.5, 4.0),
            align,
            ..Default::default()
        };
        *ids += 1;
        let mut s = Shape::new(ShapeKind::Text(td));
        s.id = *ids;
        s.name = "Text".into();
        // Lottie places the first baseline at the origin.
        s.transform = m * Affine::translate((0.0, -size * 0.8));
        s.fill = Some(Paint::Solid(color(&nums(&doc["fc"]), 1.0)));
        if let Some(a) = whole {
            s.transform = m * a.matrix(Point::new(0.0, 0.0)) * Affine::translate((0.0, -size * 0.8));
            s.opacity = a.opacity.clamp(0.0, 1.0) as f32;
            if let (Some(c), Some(Paint::Solid(base))) = (a.fill, &mut s.fill) {
                *base = c;
            }
        }
        if let (Some(sc), Some(sw)) = (doc["sc"].as_array(), doc["sw"].as_f64()) {
            if sw > 0.0 {
                let c: Vec<f64> = sc.iter().filter_map(|x| x.as_f64()).collect();
                s.stroke = Some(Stroke { color: color(&c, 1.0), width: sw, ..Stroke::default() });
            }
        }
        Some(s)
    }
}

impl Lottie<'_> {
    /// Text drawn from the file's glyph outlines (when it has them for
    /// every letter), one shape per letter so text animators can move
    /// each.
    fn glyph_text(&self, l: &J, doc: &J, content: &str, f: f64, m: Affine, ids: &mut u64) -> Option<Shape> {
        let (family, style, ascent) = self.font_keys.get(doc["f"].as_str()?)?.clone();
        let size = num(&doc["s"], 24.0);
        let k = size / 100.0;
        let glyph = |c: char| self.glyphs.get(&(c.to_string(), family.clone(), style.clone()));
        if content.chars().any(|c| c != '\n' && glyph(c).is_none()) {
            return None;
        }
        let tracking = num(&doc["tr"], 0.0) * size / 1000.0;
        let advance = |c: char| glyph(c).map(|g| g.0 * k).unwrap_or(0.0);
        // Lines, wrapped to the text box when there is one.
        let boxed = doc["sz"].as_array().map(|v| (num(&v[0], 0.0), num(&v[1], 0.0)));
        let mut lines: Vec<String> = Vec::new();
        for para in content.split('\n') {
            match boxed {
                Some((w, _)) if w > 0.0 => {
                    let mut line = String::new();
                    for word in para.split_inclusive(' ') {
                        let width = |s: &str| s.trim_end().chars().map(|c| advance(c) + tracking).sum::<f64>();
                        if !line.is_empty() && width(&format!("{line}{word}")) > w {
                            lines.push(std::mem::take(&mut line));
                        }
                        line.push_str(word);
                    }
                    lines.push(line);
                }
                _ => lines.push(para.to_string()),
            }
        }
        let lh = num(&doc["lh"], size * 1.2);
        let origin = match doc["ps"].as_array() {
            Some(ps) => Point::new(num(&ps[0], 0.0), num(&ps[1], 0.0) + ascent * k),
            None => Point::ZERO,
        };
        let joined: String = lines.join("\n");
        let anim = animators(l, &joined, f, color(&nums(&doc["fc"]), 1.0));
        let base_fill = color(&nums(&doc["fc"]), 1.0);
        let stroke = match (doc["sc"].as_array(), doc["sw"].as_f64()) {
            (Some(sc), Some(sw)) if sw > 0.0 => Some(color(&sc.iter().filter_map(|x| x.as_f64()).collect::<Vec<_>>(), 1.0)).map(|c| (c, sw)),
            _ => None,
        };
        let stroke_over = doc["of"].as_bool().unwrap_or(true);
        let alignment = vec_at(&l["t"]["m"]["a"], f);
        let (al_x, al_y) = (alignment.first().copied().unwrap_or(0.0), alignment.get(1).copied().unwrap_or(0.0));
        *ids += 1;
        let mut g = Shape::new(ShapeKind::Group { children: Vec::new() });
        g.id = *ids;
        g.fill = None;
        g.name = "Text".into();
        let mut letters = Vec::new();
        let mut index = 0;
        for (row, line) in lines.iter().enumerate() {
            let width: f64 = line.chars().map(|c| advance(c) + tracking).sum::<f64>() - if line.is_empty() { 0.0 } else { tracking };
            // Point text aligns on its origin, box text in the box.
            let room = boxed.map(|b| b.0).unwrap_or(0.0);
            let mut x = match doc["j"].as_i64() {
                Some(1) => room - width,
                Some(2) => (room - width) / 2.0,
                _ => 0.0,
            };
            let y = origin.y + row as f64 * lh;
            for c in line.chars() {
                let an = advance(c);
                let a = anim.get(index).cloned().unwrap_or_default();
                index += 1;
                let (_, shapes) = glyph(c).copied()?;
                // Letters turn and scale about their middle (shifted by the
                // grouping alignment), as lottie-web places them.
                let pivot = Point::new(origin.x + x + an / 2.0 + al_x * an * 0.005, y + al_y * ascent * k * 0.01);
                let lm = m * Affine::translate(pivot.to_vec2()) * a.matrix(Point::ZERO) * Affine::translate((-an / 2.0 - al_x * an * 0.005, -al_y * ascent * k * 0.01)) * Affine::scale(k);
                x += an + tracking + a.tracking * size / 1000.0;
                if shapes.as_array().is_none_or(|v| v.is_empty()) || c == ' ' {
                    continue;
                }
                let fill = Style::Fill(Paint::Solid(a.fill.unwrap_or(base_fill)), FillRule::NonZero);
                let mut styles = vec![fill];
                if let Some((sc, sw)) = stroke {
                    let st = Style::Stroke(Stroke { color: sc, width: sw, ..Stroke::default() });
                    if stroke_over {
                        styles.insert(0, st);
                    } else {
                        styles.push(st);
                    }
                }
                let mut parts = Vec::new();
                self.items(shapes.as_array().unwrap(), f, lm, 1.0, &styles, &[], ids, &mut parts);
                let op = a.opacity.clamp(0.0, 1.0) as f32;
                let mut letter = if parts.len() == 1 {
                    parts.pop().unwrap()
                } else {
                    *ids += 1;
                    let mut lg = Shape::new(ShapeKind::Group { children: parts });
                    lg.id = *ids;
                    lg.fill = None;
                    lg
                };
                letter.name = c.to_string();
                letter.opacity *= op;
                letters.push(letter);
            }
        }
        g.kind = ShapeKind::Group { children: letters };
        Some(g)
    }
}

/// What text animators do to one letter.
#[derive(Clone)]
struct LetterAnim {
    offset: Vec2,
    scale: Vec2,
    rotation: f64,
    opacity: f64,
    fill: Option<Rgba>,
    tracking: f64,
}

impl Default for LetterAnim {
    fn default() -> Self {
        LetterAnim { offset: Vec2::ZERO, scale: Vec2::new(1.0, 1.0), rotation: 0.0, opacity: 1.0, fill: None, tracking: 0.0 }
    }
}

impl LetterAnim {
    fn matrix(&self, pivot: Point) -> Affine {
        Affine::translate(pivot.to_vec2() + self.offset) * Affine::rotate(self.rotation.to_radians()) * Affine::scale_non_uniform(self.scale.x, self.scale.y) * Affine::translate(-pivot.to_vec2())
    }

    fn same(&self, o: &LetterAnim) -> bool {
        (self.offset - o.offset).hypot() < 1e-6 && (self.scale - o.scale).hypot() < 1e-6 && (self.rotation - o.rotation).abs() < 1e-6 && (self.opacity - o.opacity).abs() < 1e-6 && self.fill == o.fill && (self.tracking - o.tracking).abs() < 1e-9
    }
}

/// Text animators (range selectors) for each letter of `content`
/// (newlines left out): position, scale, rotation, opacity, fill colour
/// and tracking, by character, character without spaces, word or line.
fn animators(l: &J, content: &str, f: f64, base: Rgba) -> Vec<LetterAnim> {
    let letters: Vec<char> = content.chars().filter(|c| *c != '\n').collect();
    let mut out = vec![LetterAnim::default(); letters.len()];
    let Some(list) = l["t"]["a"].as_array() else { return out };
    // Each letter's index for every "based on" kind, and the totals.
    let (mut word, mut line, mut solid) = (0usize, 0usize, 0usize);
    let mut idx: Vec<[usize; 4]> = Vec::new();
    let mut prev_space = false;
    for c in content.chars() {
        if c == '\n' {
            line += 1;
            word += 1;
            prev_space = true;
            continue;
        }
        if c == ' ' {
            prev_space = true;
        } else if prev_space && !idx.is_empty() {
            word += 1;
            prev_space = false;
        } else {
            prev_space = false;
        }
        idx.push([idx.len(), solid, word, line]);
        if c != ' ' {
            solid += 1;
        }
    }
    let totals = [letters.len(), solid, word + 1, line + 1];
    for an in list {
        let sel = &an["s"];
        let props = &an["a"];
        let based = (sel["b"].as_i64().unwrap_or(1).clamp(1, 4) - 1) as usize;
        let total = totals[based].max(1) as f64;
        let percent = sel["r"].as_i64() != Some(2);
        let div = if percent { 100.0 / total } else { 1.0 };
        let o = scalar(&sel["o"], f, 0.0) / div;
        let (mut s, mut e) = (scalar(&sel["s"], f, 0.0) / div + o, if sel["e"].is_null() { total } else { scalar(&sel["e"], f, 100.0) / div } + o);
        if s > e {
            std::mem::swap(&mut s, &mut e);
        }
        let amount = scalar(&sel["a"], f, 100.0) / 100.0;
        let (ne, xe) = (scalar(&sel["ne"], f, 0.0) / 100.0, scalar(&sel["xe"], f, 0.0) / 100.0);
        let (x1, y1) = if ne > 0.0 { (ne, 0.0) } else { (0.0, -ne) };
        let (x2, y2) = if xe > 0.0 { (1.0 - xe, 1.0) } else { (1.0, 1.0 + xe) };
        let easer = crate::animation::Ease::Bezier([x1, y1, x2, y2]);
        let shape = sel["sh"].as_i64().unwrap_or(1);
        let p = vec_at(&props["p"], f);
        let sc = vec_at(&props["s"], f);
        let r = scalar(&props["r"], f, 0.0);
        let op = (!props["o"].is_null()).then(|| scalar(&props["o"], f, 100.0));
        let fc = (!props["fc"].is_null()).then(|| color(&vec_at(&props["fc"], f), 1.0));
        let tr = scalar(&props["t"], f, 0.0);
        for (i, a) in out.iter_mut().enumerate() {
            let ind = idx[i][based] as f64;
            let ramp = |ind: f64| if e == s { if ind >= e { 1.0 } else { 0.0 } } else { (0.5 / (e - s) + (ind - s) / (e - s)).clamp(0.0, 1.0) };
            let mult = match shape {
                2 => ramp(ind),
                3 => 1.0 - ramp(ind),
                4 => {
                    let m = ramp(ind);
                    if e == s { 0.0 } else if m < 0.5 { m * 2.0 } else { 1.0 - 2.0 * (m - 0.5) }
                }
                5 => {
                    if e == s { 0.0 } else {
                        let tot = e - s;
                        let x = -tot / 2.0 + (ind + 0.5 - s).clamp(0.0, tot);
                        let r = tot / 2.0;
                        (1.0 - x * x / (r * r)).max(0.0).sqrt()
                    }
                }
                6 => {
                    if e == s { 0.0 } else { (1.0 + (std::f64::consts::PI + std::f64::consts::TAU * (ind + 0.5 - s).clamp(0.0, e - s) / (e - s)).cos()) / 2.0 }
                }
                _ => {
                    if ind >= s.floor() {
                        if ind - s < 0.0 { (e.min(1.0) - (s - ind)).clamp(0.0, 1.0) } else { (e - ind).clamp(0.0, 1.0) }
                    } else {
                        0.0
                    }
                }
            };
            let mult = easer.apply(mult) * amount;
            if mult.abs() < 1e-9 {
                continue;
            }
            if p.len() >= 2 {
                a.offset += Vec2::new(p[0], p[1]) * mult;
            }
            if sc.len() >= 2 {
                a.scale.x *= 1.0 + (sc[0] / 100.0 - 1.0) * mult;
                a.scale.y *= 1.0 + (sc[1] / 100.0 - 1.0) * mult;
            }
            a.rotation += r * mult;
            if let Some(o) = op {
                a.opacity += (o - 100.0) / 100.0 * mult;
            }
            if let Some(c) = fc {
                let b = a.fill.unwrap_or(base);
                let t = mult.clamp(0.0, 1.0) as f32;
                a.fill = Some(Rgba { r: b.r + (c.r - b.r) * t, g: b.g + (c.g - b.g) * t, b: b.b + (c.b - b.b) * t, a: b.a });
            }
            a.tracking += tr * mult;
        }
    }
    for a in &mut out {
        a.opacity = a.opacity.clamp(0.0, 1.0);
    }
    out
}

fn clip_leaves(s: &mut Shape, clip: &Shape) {
    match s.kind.children_mut() {
        Some(children) => {
            for c in children {
                clip_leaves(c, clip);
            }
        }
        // An inner layer's own mask wins.
        None if s.clip.is_none() => s.clip = Some(Box::new(clip.clone())),
        None => {}
    }
}

/// Layer masks as one clip outline, combined in order: add, subtract,
/// intersect and difference (lighten and darken count as add and
/// intersect); inverted masks and expansion included.
fn masks(l: &J, f: f64, m: Affine) -> Option<Shape> {
    use crate::boolean::{op, path_to_polys, BoolOp};
    let list = l["masksProperties"].as_array()?;
    let big = || path_to_polys(&Rect::new(-1e5, -1e5, 1e5, 1e5).to_path(0.1), FillRule::NonZero);
    let mut acc: Option<crate::boolean::Polys> = None;
    let k = m.determinant().abs().sqrt();
    for mk in list {
        let mode = mk["mode"].as_str().unwrap_or("a");
        if mode == "n" {
            continue;
        }
        let mut lp = path_at(&mk["pt"], f);
        lp.c |= mk["cl"].as_bool() != Some(false);
        let mut p = m * to_bez(&lp);
        let grow = scalar(&mk["x"], f, 0.0) * k;
        if grow.abs() > 1e-6 {
            if let Some(pd) = crate::path_ops::offset_path(&p, FillRule::NonZero, grow, true) {
                p = crate::geom::pathdata_to_bez(&pd);
            }
        }
        let mut polys = path_to_polys(&p, FillRule::NonZero);
        if mk["inv"].as_bool() == Some(true) {
            polys = op(&big(), &polys, BoolOp::Subtract);
        }
        // A see-through mask adds nothing (and takes nothing away).
        if scalar(&mk["o"], f, 100.0) <= 0.5 {
            continue;
        }
        acc = Some(match (mode, acc) {
            ("s", a) => op(&a.unwrap_or_else(big), &polys, BoolOp::Subtract),
            ("i" | "d", a) => op(&a.unwrap_or_else(big), &polys, BoolOp::Intersect),
            ("f", Some(a)) => op(&a, &polys, BoolOp::Exclude),
            (_, Some(a)) => op(&a, &polys, BoolOp::Union),
            (_, None) => polys,
        });
    }
    let acc = acc?;
    let mut s = Shape::new(ShapeKind::Path(crate::geom::bez_to_pathdata(&crate::geom::polys_to_bez(&acc))));
    s.name = "Mask".into();
    s.fill_rule = FillRule::NonZero;
    Some(s)
}

/// Every colour a shape paints with, through `map`.
fn recolor(s: &mut Shape, map: &dyn Fn(Rgba) -> Rgba) {
    if let Some(children) = s.kind.children_mut() {
        for c in children {
            recolor(c, map);
        }
    }
    match &mut s.fill {
        Some(Paint::Solid(c)) => *c = map(*c),
        Some(Paint::Gradient(g)) => {
            for st in &mut g.stops {
                st.color = map(st.color);
            }
        }
        _ => {}
    }
    if let Some(st) = &mut s.stroke {
        st.color = map(st.color);
    }
}

/// A gradient in document coordinates as one in the shape's box units.
fn fixed(p: &Paint, s: &Shape) -> Paint {
    match p {
        Paint::Gradient(g) => {
            let ShapeKind::Path(pd) = &s.kind else { return p.clone() };
            let b = crate::geom::path_bounds(&crate::geom::pathdata_to_bez(pd)).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
            let rel = |q: [f64; 2]| [(q[0] - b.x0) / b.width().max(1e-9), (q[1] - b.y0) / b.height().max(1e-9)];
            let mut g = g.clone();
            g.start = rel(g.start);
            g.end = rel(g.end);
            Paint::Gradient(g)
        }
        _ => p.clone(),
    }
}

/// A gradient fill or stroke (start and end still in the item's space;
/// the caller maps them).
fn gradient(it: &J, f: f64, opacity: f64) -> Option<(Gradient,)> {
    let n = it["g"]["p"].as_u64()? as usize;
    let v = vec_at(&it["g"]["k"], f);
    if n == 0 || v.len() < n * 4 {
        return None;
    }
    let alpha_stops: Vec<(f64, f64)> = v[n * 4..].chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect();
    let alpha_at = |x: f64| -> f64 {
        if alpha_stops.is_empty() {
            return 1.0;
        }
        let i = alpha_stops.iter().position(|(o, _)| *o >= x).unwrap_or(alpha_stops.len());
        match i {
            0 => alpha_stops[0].1,
            i if i >= alpha_stops.len() => alpha_stops[alpha_stops.len() - 1].1,
            i => {
                let (a, b) = (alpha_stops[i - 1], alpha_stops[i]);
                a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0).max(1e-9)
            }
        }
    };
    let o = scalar(&it["o"], f, 100.0) / 100.0 * opacity;
    let stops = v[..n * 4].chunks(4).map(|c| GradientStop::new(c[0] as f32, color(&c[1..4], alpha_at(c[0]) * o))).collect();
    let (s, e) = (point(&it["s"], f, Point::ZERO), point(&it["e"], f, Point::new(100.0, 0.0)));
    let kind = if it["t"].as_i64() == Some(2) { GradientKind::Radial } else { GradientKind::Linear };
    Some((Gradient { kind, stops, start: [s.x, s.y], end: [e.x, e.y], mesh: None },))
}

fn star(it: &J, f: f64) -> BezPath {
    let c = point(&it["p"], f, Point::ZERO);
    let n = scalar(&it["pt"], f, 5.0).round().max(3.0) as usize;
    let outer = scalar(&it["or"], f, 50.0);
    let inner = scalar(&it["ir"], f, outer / 2.0);
    let star = it["sy"].as_i64() != Some(2);
    let rot = scalar(&it["r"], f, 0.0).to_radians() - std::f64::consts::FRAC_PI_2;
    let count = if star { n * 2 } else { n };
    let mut b = BezPath::new();
    for k in 0..count {
        let a = rot + std::f64::consts::TAU * k as f64 / count as f64;
        let r = if star && k % 2 == 1 { inner } else { outer };
        let p = c + Vec2::new(a.cos(), a.sin()) * r;
        if k == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    b.close_path();
    b
}

// ---------------------------------------------------------------------------
// Shape modifiers
// ---------------------------------------------------------------------------

/// A rectangle, ellipse, star or path item in its group's space.
fn primitive(it: &J, f: f64) -> Option<BezPath> {
    match it["ty"].as_str()? {
        "sh" => {
            let mut p = path_at(&it["ks"], f);
            // Old files say "closed" on the item instead of in the path.
            p.c |= it["closed"].as_bool() == Some(true);
            Some(to_bez(&p))
        }
        "rc" => {
            let (c, s) = (point(&it["p"], f, Point::ZERO), point(&it["s"], f, Point::ZERO));
            let r = scalar(&it["r"], f, 0.0).max(0.0).min(s.x.abs().min(s.y.abs()) / 2.0);
            Some(directed(rect_path(c, s.x.abs() / 2.0, s.y.abs() / 2.0, r), it))
        }
        "el" => {
            let (c, s) = (point(&it["p"], f, Point::ZERO), point(&it["s"], f, Point::ZERO));
            Some(directed(ellipse_path(c, s.x.abs() / 2.0, s.y.abs() / 2.0), it))
        }
        "sr" => Some(directed(star(it, f), it)),
        _ => None,
    }
}

/// Shapes drawn the other way round (`d: 3`), which moves where dashes
/// and trims go.
fn directed(p: BezPath, it: &J) -> BezPath {
    if it["d"].as_i64() == Some(3) { p.reverse_subpaths() } else { p }
}

/// Handle length for a quarter circle.
const KAPPA: f64 = 0.5519;

/// An ellipse as Lottie draws it: from the top, clockwise.
fn ellipse_path(c: Point, rx: f64, ry: f64) -> BezPath {
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let mut b = BezPath::new();
    b.move_to((c.x, c.y - ry));
    b.curve_to((c.x + kx, c.y - ry), (c.x + rx, c.y - ky), (c.x + rx, c.y));
    b.curve_to((c.x + rx, c.y + ky), (c.x + kx, c.y + ry), (c.x, c.y + ry));
    b.curve_to((c.x - kx, c.y + ry), (c.x - rx, c.y + ky), (c.x - rx, c.y));
    b.curve_to((c.x - rx, c.y - ky), (c.x - kx, c.y - ry), (c.x, c.y - ry));
    b.close_path();
    b
}

/// A rectangle as Lottie draws it: from the top right, clockwise (down
/// the right side first), corners rounded by `r`.
fn rect_path(c: Point, hw: f64, hh: f64, r: f64) -> BezPath {
    let (l, t, rt, bt) = (c.x - hw, c.y - hh, c.x + hw, c.y + hh);
    let mut b = BezPath::new();
    if r <= 1e-9 {
        b.move_to((rt, t));
        b.line_to((rt, bt));
        b.line_to((l, bt));
        b.line_to((l, t));
        b.close_path();
        return b;
    }
    let k = r * (1.0 - KAPPA);
    b.move_to((rt, t + r));
    b.line_to((rt, bt - r));
    b.curve_to((rt, bt - k), (rt - k, bt), (rt - r, bt));
    b.line_to((l + r, bt));
    b.curve_to((l + k, bt), (l, bt - k), (l, bt - r));
    b.line_to((l, t + r));
    b.curve_to((l, t + k), (l + k, t), (l + r, t));
    b.line_to((rt - r, t));
    b.curve_to((rt - k, t), (rt, t + k), (rt, t + r));
    b.close_path();
    b
}

/// Every path in `items` (and nested groups), bottom first, in the
/// items' space.
fn geometry(items: &[J], f: f64, m: Affine, out: &mut Vec<BezPath>) {
    let m = m * items.iter().find(|i| i["ty"] == "tr").map(|t| transform(t, f).0).unwrap_or(Affine::IDENTITY);
    for it in items.iter().rev().filter(|i| i["hd"].as_bool() != Some(true)) {
        if it["ty"] == "gr" {
            geometry(it["it"].as_array().map(|v| v.as_slice()).unwrap_or(&[]), f, m, out);
        } else if let Some(p) = primitive(it, f) {
            out.push(m * p);
        }
    }
}

/// Merge paths: 1 merge, 2 add, 3 subtract, 4 intersect, 5 exclude. Each
/// upper path combines with the ones below it.
fn merged(items: &[J], f: f64, mode: i64) -> BezPath {
    use crate::boolean::{op, path_to_polys, BoolOp};
    let mut paths = Vec::new();
    geometry(items, f, Affine::IDENTITY, &mut paths);
    let bop = match mode {
        2 => BoolOp::Union,
        3 => BoolOp::Subtract,
        4 => BoolOp::Intersect,
        5 => BoolOp::Exclude,
        _ => {
            let mut all = BezPath::new();
            for p in paths {
                all.extend(p);
            }
            return all;
        }
    };
    // One path has nothing to combine with: it stays as drawn.
    if paths.len() == 1 {
        return paths.pop().unwrap();
    }
    let mut it = paths.iter();
    let Some(first) = it.next() else { return BezPath::new() };
    let mut acc = path_to_polys(first, FillRule::NonZero);
    for p in it {
        acc = op(&acc, &path_to_polys(p, FillRule::NonZero), bop);
    }
    crate::geom::polys_to_bez(&acc)
}

/// Repeaters: the items before each one, copied with its transform
/// stepped. Copies are kept up to the most the animation needs (the rest
/// transparent), so the shapes stay the same from frame to frame.
fn repeat(items: &[J], f: f64) -> Vec<J> {
    let mut items = items.to_vec();
    while let Some(r) = items.iter().position(|i| i["ty"] == "rp" && i["hd"].as_bool() != Some(true)) {
        let rp = items[r].clone();
        let copies = scalar(&rp["c"], f, 1.0).max(0.0);
        let most = keyframes(&rp["c"]).map(|k| k.iter().flat_map(|k| [num(&k["s"][0], 0.0), num(&k["s"], 0.0)]).fold(copies, f64::max)).unwrap_or(copies);
        let (n, most) = (copies.ceil() as usize, (most.ceil() as usize).min(1000));
        let offset = scalar(&rp["o"], f, 0.0);
        let tr = &rp["tr"];
        let a = point(&tr["a"], f, Point::ZERO);
        let step = Affine::translate(a.to_vec2()) * transform(tr, f).0;
        let (so, eo) = (scalar(&tr["so"], f, 100.0) / 100.0, scalar(&tr["eo"], f, 100.0) / 100.0);
        let before: Vec<J> = items[..r].iter().filter(|i| i["ty"] != "tr").cloned().collect();
        let pow = |n: f64| {
            let whole = n.trunc();
            let mut m = Affine::IDENTITY;
            let s = if whole < 0.0 { step.inverse() } else { step };
            for _ in 0..(whole.abs() as usize).min(1000) {
                m = s * m;
            }
            let frac = n - whole;
            if frac.abs() > 1e-9 {
                // A part step: interpolate the transform's values.
                let mut t = tr.clone();
                let lerp = |v: Vec<f64>, rest: f64| v.iter().map(|x| rest + (x - rest) * frac).collect::<Vec<_>>();
                t["p"] = serde_json::json!({"k": lerp(vec_at(&tr["p"], f), 0.0)});
                t["s"] = serde_json::json!({"k": lerp(vec_at(&tr["s"], f), 100.0)});
                t["r"] = serde_json::json!({"k": scalar(&tr["r"], f, 0.0) * frac});
                m = Affine::translate(a.to_vec2()) * transform(&t, f).0 * m;
            }
            m
        };
        let mut copies_json = Vec::new();
        for c in 0..most {
            let m = pow(c as f64 + offset).as_coeffs();
            let o = if c >= n { 0.0 } else if n > 1 { so + (eo - so) * c as f64 / (n - 1) as f64 } else { so };
            let mut it = before.clone();
            it.push(serde_json::json!({"ty": "tr", "_m": m, "_o": o}));
            copies_json.push(serde_json::json!({"ty": "gr", "nm": format!("Copy {}", c + 1), "it": it}));
        }
        // "Above": later copies on top (first in the list).
        if rp["m"].as_i64() != Some(2) {
            copies_json.reverse();
        }
        copies_json.extend(items.drain(r + 1..));
        items = copies_json;
    }
    items
}

/// Round corners: each sharp corner gets a live corner radius.
fn round_corners(p: &BezPath, r: f64) -> BezPath {
    if r <= 1e-9 {
        return p.clone();
    }
    let mut pd = crate::geom::bez_to_pathdata(p);
    for sp in &mut pd.subpaths {
        for n in &mut sp.nodes {
            if n.hin.is_none() && n.hout.is_none() {
                n.corner = r;
            }
        }
    }
    crate::geom::pathdata_to_bez(&pd)
}

/// Pucker & bloat (as lottie-web): points move toward the centre by `a`
/// and the handles away, so bloat (`a > 0`) bulges and pucker pinches.
fn pucker(p: &BezPath, a: f64) -> BezPath {
    if a.abs() < 1e-9 {
        return p.clone();
    }
    let mut out = BezPath::new();
    for sub in split_els(p) {
        let ends: Vec<Point> = sub.iter().filter_map(|e| e.end_point()).collect();
        if ends.is_empty() {
            continue;
        }
        let c = ends.iter().fold(Point::ZERO, |s, q| s + q.to_vec2()).to_vec2() / ends.len() as f64;
        let c = c.to_point();
        let on = |q: Point| q + (c - q) * a;
        let off = |q: Point| q + (c - q) * -a;
        // Straight sides have their handles on the points, so they bow too.
        let (mut start, mut last) = (Point::ZERO, Point::ZERO);
        for e in sub {
            let el = match e {
                PathEl::MoveTo(q) => {
                    (start, last) = (q, q);
                    PathEl::MoveTo(on(q))
                }
                PathEl::LineTo(q) => PathEl::CurveTo(off(std::mem::replace(&mut last, q)), off(q), on(q)),
                PathEl::QuadTo(h, q) => {
                    last = q;
                    PathEl::QuadTo(off(h), on(q))
                }
                PathEl::CurveTo(h1, h2, q) => {
                    last = q;
                    PathEl::CurveTo(off(h1), off(h2), on(q))
                }
                PathEl::ClosePath => {
                    if last.distance(start) > 1e-9 {
                        out.push(PathEl::CurveTo(off(last), off(start), on(start)));
                    }
                    PathEl::ClosePath
                }
            };
            out.push(el);
        }
    }
    out
}

fn split_els(p: &BezPath) -> Vec<Vec<PathEl>> {
    let mut out: Vec<Vec<PathEl>> = Vec::new();
    for e in p.elements() {
        if matches!(e, PathEl::MoveTo(_)) || out.is_empty() {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push(*e);
    }
    out
}

// ---------------------------------------------------------------------------
// Trim paths
// ---------------------------------------------------------------------------

/// The part of `p` between `t.s` and `t.e` (shifted by `t.o`), per path or
/// along all of them.
fn trim_path(p: &BezPath, t: Trim) -> BezPath {
    let (mut a, mut b) = (t.s.min(t.e) + t.o, t.s.max(t.e) + t.o);
    if (b - a) >= 1.0 - 1e-9 {
        return p.clone();
    }
    if (b - a) <= 1e-9 {
        return BezPath::new();
    }
    let shift = a.floor();
    a -= shift;
    b -= shift;
    let subpaths = split(p);
    let mut out = BezPath::new();
    let take = |segs: &[PathSeg], total: f64, from: f64, to: f64, out: &mut BezPath| {
        // `from..to` in length along `segs`.
        let mut at = 0.0;
        let mut open = false;
        for seg in segs {
            let len = seg.arclen(1e-3);
            let (s0, s1) = (from.max(at), to.min(at + len));
            if s1 > s0 && len > 1e-12 {
                let u0 = seg.inv_arclen(s0 - at, 1e-3);
                let u1 = seg.inv_arclen(s1 - at, 1e-3);
                let part = seg.subsegment(u0..u1);
                if !open {
                    out.move_to(part.start());
                    open = true;
                }
                match part {
                    PathSeg::Line(l) => out.line_to(l.p1),
                    PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
                    PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
                }
            }
            at += len;
        }
        let _ = total;
    };
    let lens: Vec<f64> = subpaths.iter().map(|s| s.iter().map(|g| g.arclen(1e-3)).sum()).collect();
    if t.each {
        for (segs, total) in subpaths.iter().zip(&lens) {
            let (x, y) = (a * total, b * total);
            take(segs, *total, x, y.min(*total), &mut out);
            if y > *total {
                take(segs, *total, 0.0, y - total, &mut out);
            }
        }
    } else {
        let total: f64 = lens.iter().sum();
        let all: Vec<PathSeg> = subpaths.concat();
        let (x, y) = (a * total, b * total);
        take(&all, total, x, y.min(total), &mut out);
        if y > total {
            take(&all, total, 0.0, y - total, &mut out);
        }
    }
    out
}

fn split(p: &BezPath) -> Vec<Vec<PathSeg>> {
    let mut out: Vec<Vec<PathSeg>> = Vec::new();
    let mut cur = Vec::new();
    let mut start = Point::ZERO;
    let mut last = Point::ZERO;
    for el in p.elements() {
        match *el {
            PathEl::MoveTo(q) => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                start = q;
                last = q;
            }
            PathEl::LineTo(q) => {
                cur.push(PathSeg::Line(kurbo::Line::new(last, q)));
                last = q;
            }
            PathEl::QuadTo(a, q) => {
                cur.push(PathSeg::Quad(kurbo::QuadBez::new(last, a, q)));
                last = q;
            }
            PathEl::CurveTo(a, b, q) => {
                cur.push(PathSeg::Cubic(kurbo::CubicBez::new(last, a, b, q)));
                last = q;
            }
            PathEl::ClosePath => {
                if (last - start).hypot() > 1e-9 {
                    cur.push(PathSeg::Line(kurbo::Line::new(last, start)));
                }
                last = start;
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
