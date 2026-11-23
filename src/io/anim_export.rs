//! Animated exports: WebM video, animated SVG (CSS keyframes) and Lottie.
//!
//! All three sample the timeline frame by frame: `pages` holds the exported
//! area at every frame plus one closing frame at the very end, so loops
//! join up. Vector formats describe each export item's motion as the affine
//! map from its first-frame outline to the current one, and its colours and
//! opacity as they are at each frame.

use std::fmt::Write;

use kurbo::{Affine, BezPath, PathEl, Point};
use serde_json::{Value, json};

use crate::document::{Document, FillRule, LineCap, LineJoin, Paint, Rgba};
use crate::fonts::FontManager;
use crate::io::{Item, num};

/// Frame rate and number of frames between the in and out points
/// (without the closing frame).
pub fn timing(doc: &Document) -> (f64, usize) {
    let tl = doc.animation.clone().unwrap_or_default();
    (tl.fps, tl.span_frames())
}

fn points_of(p: &BezPath) -> Vec<Point> {
    let mut out = Vec::new();
    for el in p.elements() {
        match *el {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => out.push(a),
            PathEl::QuadTo(a, b) => out.extend([a, b]),
            PathEl::CurveTo(a, b, c) => out.extend([a, b, c]),
            PathEl::ClosePath => {}
        }
    }
    out
}

/// Each item's map from frame 0, per frame; `None` for an item whose
/// outline changes in a way no affine map gives (it morphs).
fn motions(frames: &[Vec<Item>]) -> Vec<Option<Vec<Affine>>> {
    let base = &frames[0];
    (0..base.len())
        .map(|i| {
            let p0 = points_of(&base[i].path);
            frames
                .iter()
                .map(|f| {
                    let p = points_of(&f.get(i)?.path);
                    if p.len() != p0.len() {
                        return None;
                    }
                    crate::animation::fit(&p0, &p)
                })
                .collect()
        })
        .collect()
}

/// Layers hidden for part of the animation as visible but fully
/// transparent there, so every frame has the same items and the vector
/// formats can fade them in and out.
fn show_hidden(pages: &[Document]) -> Vec<Document> {
    let mut shown = std::collections::HashSet::new();
    for d in pages {
        for id in d.all_ids() {
            if d.get(id).is_some_and(|s| s.visible) {
                shown.insert(id);
            }
        }
    }
    pages
        .iter()
        .map(|d| {
            let mut d = d.clone();
            for id in d.all_ids() {
                let Some(s) = d.get(id).filter(|s| !s.visible && shown.contains(&id)) else { continue };
                let mut s = s.clone();
                s.visible = true;
                s.opacity = 0.0;
                d.replace(s);
            }
            d
        })
        .collect()
}

fn items<'a>(pages: &'a [Document], fonts: &mut FontManager, real_text: bool) -> Vec<Vec<Item<'a>>> {
    pages.iter().map(|d| crate::io::prepare_with(d, fonts, real_text)).collect()
}

// ---------------------------------------------------------------------------
// WebM
// ---------------------------------------------------------------------------

pub fn webm(pages: &[Document], fps: f64, scale: f64, quality: u8, fonts: &mut FontManager, progress: &mut dyn FnMut(usize, usize)) -> Result<Vec<u8>, String> {
    let frames = &pages[..pages.len().saturating_sub(1).max(1)];
    let (w, h) = super::export::raster_size(frames[0].primary(), None, scale);
    if (w as u64) * (h as u64) > 36_000_000 {
        return Err(format!("{w} × {h} px is too large for video"));
    }
    let bg = frames[0].primary().background;
    let bg = if bg.a > 0.0 { bg } else { Rgba::WHITE };
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    // SVG export needs the font manager, so it runs here; rendering the SVGs
    // is the slow part and runs on worker threads, a batch at a time.
    let batch = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let mut error = None;
    let mut ready = std::collections::VecDeque::new();
    let mut next = 0;
    let mut iter = std::iter::from_fn(|| {
        if ready.is_empty() && next < frames.len() && error.is_none() {
            let end = (next + batch).min(frames.len());
            let svgs: Vec<String> = frames[next..end].iter().map(|d| super::svg::export(d, fonts)).collect();
            next = end;
            let images: Vec<_> = std::thread::scope(|s| {
                let jobs: Vec<_> = svgs.iter().map(|svg| s.spawn(move || super::export::render_svg(svg, w, h))).collect();
                jobs.into_iter().map(|j| j.join().unwrap_or_else(|_| Err("frame rendering crashed".into()))).collect()
            });
            for r in images {
                match r {
                    Ok(img) => ready.push_back(img),
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
        }
        ready.pop_front()
    });
    let out = super::webm::encode(w, h, fps, quality, [c(bg.r), c(bg.g), c(bg.b)], &mut iter, frames.len(), progress)?;
    match error {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

// ---------------------------------------------------------------------------
// Animated SVG
// ---------------------------------------------------------------------------

/// CSS keyframes, leaving out frames between two equal ones.
fn keyframes(name: &str, values: &[String], out: &mut String) {
    let n = values.len() - 1;
    write!(out, "@keyframes {name}{{").unwrap();
    for (i, v) in values.iter().enumerate() {
        if i > 0 && i < n && values[i - 1] == *v && values[i + 1] == *v {
            continue;
        }
        write!(out, "{}%{{{v}}}", num(i as f64 * 100.0 / n as f64)).unwrap();
    }
    out.push('}');
}

fn color_css(prop: &str, c: Rgba) -> String {
    format!("{prop}:{};{prop}-opacity:{}", c.to_hex(), num(c.a as f64))
}

/// The pages as one SVG whose items move, fade and change colour with CSS
/// animations (frame 0 shows where the animation isn't supported).
pub fn svg(pages: &[Document], fps: f64, fonts: &mut FontManager, opts: &super::svg::SvgOptions) -> String {
    let mut svg = super::svg::export_svg_tagged(&pages[0], fonts, opts);
    if pages.len() < 2 {
        return svg;
    }
    let pages = &show_hidden(pages);
    let frames = items(pages, fonts, opts.real_text);
    let moves = motions(&frames);
    let dur = num((pages.len() - 1) as f64 / fps);
    let repeat = if pages[0].animation.as_ref().is_none_or(|a| a.looping) { "infinite" } else { "1 both" };
    let mut css = String::new();
    let anim = |name: &str| format!("animation:{name} {dur}s linear {repeat}");
    for (i, it0) in frames[0].iter().enumerate() {
        if it0.span > 0 || it0.path.is_empty() || frames.iter().any(|f| f.len() != frames[0].len()) {
            continue;
        }
        let mut rules = Vec::new();
        // Motion and opacity on the item's wrapper (the item's own opacity
        // gives way to it).
        // A morphing outline: its path data is animated instead.
        let still = vec![Affine::IDENTITY; frames.len()];
        let m = moves[i].as_ref().unwrap_or(&still);
        if moves[i].is_none() {
            let vals: Vec<String> = frames.iter().map(|f| format!("d:path(\"{}\")", super::svg::path_d(&f[i].path))).collect();
            keyframes(&format!("d{i}"), &vals, &mut css);
            rules.push(format!(".m{i} path{{{}}}", anim(&format!("d{i}"))));
        }
        let moving = m.iter().any(|a| !crate::animation::near_identity(*a));
        let op = |f: &Vec<Item>| f[i].shape.opacity as f64;
        let fading = frames.iter().any(|f| (op(f) - op(&frames[0])).abs() > 1e-4);
        if moving || fading {
            let vals: Vec<String> = frames
                .iter()
                .zip(m)
                .map(|(f, a)| {
                    let mut v = String::new();
                    if moving {
                        let c = a.as_coeffs();
                        write!(v, "transform:matrix({},{},{},{},{},{})", num(c[0]), num(c[1]), num(c[2]), num(c[3]), num(c[4]), num(c[5])).unwrap();
                    }
                    if fading {
                        let o = op(f);
                        if moving {
                            v.push(';');
                        }
                        write!(v, "opacity:{}", num(o)).unwrap();
                    }
                    v
                })
                .collect();
            keyframes(&format!("k{i}"), &vals, &mut css);
            rules.push(format!(".m{i}{{transform-box:view-box;transform-origin:0 0;{}}}", anim(&format!("k{i}"))));
            if fading {
                rules.push(format!(".m{i} [opacity]{{opacity:1}}"));
            }
        }
        // Solid fill and stroke colours (and the stroke width), on the
        // elements painted with the frame 0 colour.
        let fill = |f: &Vec<Item>| f[i].shape.fill.as_ref().and_then(Paint::solid);
        if let Some(c0) = fill(&frames[0]) {
            if frames.iter().any(|f| fill(f) != Some(c0)) {
                let vals: Vec<String> = frames.iter().map(|f| color_css("fill", fill(f).unwrap_or(c0))).collect();
                keyframes(&format!("f{i}"), &vals, &mut css);
                rules.push(format!(".m{i} [fill=\"{}\"]{{{}}}", c0.to_hex(), anim(&format!("f{i}"))));
            }
        }
        let stroke = |f: &Vec<Item>| f[i].shape.stroke.as_ref().map(|s| (s.color, s.width, s.is_dashed().then(|| (s.dash_px(), s.dash_offset * s.width))));
        if let Some(first @ (c0, w0, _)) = stroke(&frames[0]) {
            if frames.iter().any(|f| stroke(f).as_ref() != Some(&first)) {
                // The stroke width is in the item's frame 0 space: the motion
                // scales it already.
                let vals: Vec<String> = frames
                    .iter()
                    .zip(m)
                    .map(|(f, a)| {
                        let (c, w, dash) = stroke(f).unwrap_or((c0, w0, None));
                        let k = a.determinant().abs().sqrt().max(1e-9);
                        let mut v = format!("{};stroke-width:{}", color_css("stroke", c), num(w / k));
                        if let Some((d, off)) = dash {
                            let d: Vec<String> = d.iter().map(|x| num(x / k)).collect();
                            write!(v, ";stroke-dasharray:{};stroke-dashoffset:{}", d.join(" "), num(off / k)).unwrap();
                        }
                        v
                    })
                    .collect();
                keyframes(&format!("s{i}"), &vals, &mut css);
                rules.push(format!(".m{i} [stroke=\"{}\"]{{{}}}", c0.to_hex(), anim(&format!("s{i}"))));
            }
        }
        for r in rules {
            css.push_str(&r);
        }
        css.push('\n');
    }
    if css.trim().is_empty() {
        return svg;
    }
    // Right after the opening <svg …> tag.
    if let Some(at) = svg.find("<svg").and_then(|a| svg[a..].find('>').map(|b| a + b + 1)) {
        let style = if opts.minify { format!("<style>{}</style>", css.replace('\n', "")) } else { format!("\n  <style>\n{css}  </style>") };
        svg.insert_str(at, &style);
    }
    svg
}

// ---------------------------------------------------------------------------
// Lottie
// ---------------------------------------------------------------------------

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn pt(p: Point) -> Value {
    json!([r3(p.x), r3(p.y)])
}

/// One Lottie path per subpath: vertices with in/out tangents relative to
/// them.
fn lottie_paths(p: &BezPath) -> Vec<Value> {
    struct Sub {
        v: Vec<Point>,
        i: Vec<Point>,
        o: Vec<Point>,
        closed: bool,
    }
    let mut subs: Vec<Sub> = Vec::new();
    let mut cur: Option<Sub> = None;
    let flush = |cur: &mut Option<Sub>, subs: &mut Vec<Sub>| {
        if let Some(s) = cur.take().filter(|s| !s.v.is_empty()) {
            subs.push(s);
        }
    };
    for el in p.elements() {
        match *el {
            PathEl::MoveTo(a) => {
                flush(&mut cur, &mut subs);
                cur = Some(Sub { v: vec![a], i: vec![Point::ZERO], o: vec![Point::ZERO], closed: false });
            }
            PathEl::LineTo(a) => {
                if let Some(s) = &mut cur {
                    s.v.push(a);
                    s.i.push(Point::ZERO);
                    s.o.push(Point::ZERO);
                }
            }
            PathEl::QuadTo(q, a) => {
                if let Some(s) = &mut cur {
                    let p0 = *s.v.last().unwrap();
                    let c1 = p0 + (q - p0) * (2.0 / 3.0);
                    let c2 = a + (q - a) * (2.0 / 3.0);
                    *s.o.last_mut().unwrap() = (c1 - p0).to_point();
                    s.v.push(a);
                    s.i.push((c2 - a).to_point());
                    s.o.push(Point::ZERO);
                }
            }
            PathEl::CurveTo(c1, c2, a) => {
                if let Some(s) = &mut cur {
                    let p0 = *s.v.last().unwrap();
                    *s.o.last_mut().unwrap() = (c1 - p0).to_point();
                    s.v.push(a);
                    s.i.push((c2 - a).to_point());
                    s.o.push(Point::ZERO);
                }
            }
            PathEl::ClosePath => {
                if let Some(s) = &mut cur {
                    s.closed = true;
                    // A final segment back onto the start: merge the two.
                    if s.v.len() > 1 && (s.v[0] - *s.v.last().unwrap()).hypot() < 1e-6 {
                        s.v.pop();
                        s.i[0] = s.i.pop().unwrap();
                        s.o.pop();
                    }
                }
                flush(&mut cur, &mut subs);
            }
        }
    }
    flush(&mut cur, &mut subs);
    subs.into_iter()
        .map(|s| {
            json!({
                "c": s.closed,
                "v": s.v.iter().map(|p| pt(*p)).collect::<Vec<_>>(),
                "i": s.i.iter().map(|p| pt(*p)).collect::<Vec<_>>(),
                "o": s.o.iter().map(|p| pt(*p)).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// A Lottie path with `n` vertices: each one repeated in place (a sharp
/// corner that opens up into a rounded one), or the last one if it doesn't
/// divide evenly.
fn pad_vertices(v: &Value, n: usize) -> Value {
    let len = v["v"].as_array().map_or(0, |a| a.len());
    if len == 0 || len >= n {
        return v.clone();
    }
    let reps = if n % len == 0 { n / len } else { 1 };
    let mut out = v.clone();
    for key in ["v", "i", "o"] {
        let src = v[key].as_array().cloned().unwrap_or_default();
        let mut a: Vec<Value> = Vec::with_capacity(n);
        for x in &src {
            for r in 0..reps {
                // Only the outer copies keep the handles.
                let keep = (key == "i" && r == 0) || (key == "o" && r == reps - 1) || key == "v";
                a.push(if keep { x.clone() } else { json!([0, 0]) });
            }
        }
        while a.len() < n {
            a.push(if key == "v" { src.last().cloned().unwrap_or(json!([0, 0])) } else { json!([0, 0]) });
        }
        out[key] = json!(a);
    }
    out
}

/// The numbers in a value (shape paths included), or `None` for text and
/// flags, which can't be in between.
fn numbers(v: &Value, out: &mut Vec<f64>) -> bool {
    match v {
        Value::Number(n) => {
            out.push(n.as_f64().unwrap_or(0.0));
            true
        }
        Value::Array(a) => a.iter().all(|x| numbers(x, out)),
        Value::Object(o) => o.iter().all(|(k, x)| k == "c" || numbers(x, out)),
        _ => false,
    }
}

/// Which frames to key: the ends, and wherever the values stop moving in a
/// straight line (within rounding), so steady motion needs two keys.
pub(crate) fn key_frames(values: &[Value]) -> Vec<usize> {
    let n = values.len() - 1;
    let flat: Vec<Option<Vec<f64>>> = values.iter().map(|v| {
        let mut out = Vec::new();
        numbers(v, &mut out).then_some(out)
    }).collect();
    // Frames b can follow a linearly: every frame between is on the line.
    let linear = |a: usize, b: usize| -> bool {
        let (Some(va), Some(vb)) = (&flat[a], &flat[b]) else { return (a..=b).all(|t| values[t] == values[a]) };
        if va.len() != vb.len() || values[a].get("c") != values[b].get("c") {
            return false;
        }
        (a + 1..b).all(|t| {
            let u = (t - a) as f64 / (b - a) as f64;
            flat[t].as_ref().is_some_and(|vt| vt.len() == va.len() && va.iter().zip(vb).zip(vt).all(|((x, y), z)| (x + (y - x) * u - z).abs() <= 2e-3))
        })
    };
    let mut keys = vec![0];
    let mut a = 0;
    while a < n {
        let mut b = a + 1;
        while b < n && b - a < 240 && linear(a, b + 1) {
            b += 1;
        }
        keys.push(b);
        a = b;
    }
    keys
}

/// A Lottie property: static when every frame has the same value, else
/// linear keys where the motion changes.
fn prop(values: Vec<Value>) -> Value {
    if values.windows(2).all(|w| w[0] == w[1]) {
        return json!({"a": 0, "k": values[0]});
    }
    let mut keys = Vec::new();
    for t in key_frames(&values) {
        let v = &values[t];
        // Keyed values are arrays; shapes and numbers get wrapped.
        let s = if v.is_array() { v.clone() } else { json!([v]) };
        keys.push(json!({"t": t, "s": s, "i": {"x": [1], "y": [1]}, "o": {"x": [0], "y": [0]}}));
    }
    json!({"a": 1, "k": keys})
}

fn rgba(c: Rgba) -> (Value, f64) {
    (json!([r3(c.r as f64), r3(c.g as f64), r3(c.b as f64), 1]), r3(c.a as f64 * 100.0))
}

/// A gradient fill as its first colour (Lottie gradients aren't written).
fn paint_color(p: &Paint) -> Option<Rgba> {
    match p {
        Paint::Solid(c) => Some(*c),
        Paint::Gradient(g) => g.stops.first().map(|s| s.color),
        Paint::Image(_) => None,
    }
}

/// Drop shadow and Gaussian blur effects from (shadow, blur, layer scale)
/// per frame; only what some frame has.
fn lottie_effects(frames: &[(Option<crate::document::Shadow>, f64, f64)]) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(first) = frames.iter().find_map(|f| f.0.clone()) {
        let sh = |f: &(Option<crate::document::Shadow>, f64, f64)| f.0.clone().unwrap_or(crate::document::Shadow { color: first.color.with_alpha(0.0), ..first.clone() });
        let p = |nm: &str, ix: usize, ty: i64, v: Value| json!({"ty": ty, "nm": nm, "mn": format!("ADBE Drop Shadow-000{ix}"), "ix": ix, "v": v});
        let col = prop(frames.iter().map(|f| { let c = sh(f).color; json!([r3(c.r as f64), r3(c.g as f64), r3(c.b as f64), 1]) }).collect());
        let alpha = prop(frames.iter().map(|f| json!(r3(sh(f).color.a as f64 * 255.0))).collect());
        // Direction: 0° is up, clockwise.
        let dir = prop(frames.iter().map(|f| { let s = sh(f); json!(r3(s.dx.atan2(-s.dy).to_degrees())) }).collect());
        let dist = prop(frames.iter().map(|f| { let s = sh(f); json!(r3(s.dx.hypot(s.dy) / f.2)) }).collect());
        let soft = prop(frames.iter().map(|f| json!(r3(sh(f).blur * 2.0 / f.2))).collect());
        out.push(json!({"ty": 25, "nm": "Drop Shadow", "mn": "ADBE Drop Shadow", "np": 8, "ix": 1, "en": 1, "ef": [
            p("Shadow Color", 1, 2, col), p("Opacity", 2, 0, alpha), p("Direction", 3, 0, dir), p("Distance", 4, 0, dist), p("Softness", 5, 0, soft), p("Shadow Only", 6, 7, json!({"a": 0, "k": 0}))
        ]}));
    }
    if frames.iter().any(|f| f.1 > 0.0) {
        let v = prop(frames.iter().map(|f| json!(r3(f.1 / 0.6 / f.2))).collect());
        out.push(json!({"ty": 29, "nm": "Gaussian Blur", "mn": "ADBE Gaussian Blur 2", "np": 5, "ix": out.len() + 1, "en": 1, "ef": [
            {"ty": 0, "nm": "Blurriness", "mn": "ADBE Gaussian Blur 2-0001", "ix": 1, "v": v},
            {"ty": 7, "nm": "Blur Dimensions", "mn": "ADBE Gaussian Blur 2-0002", "ix": 2, "v": {"a": 0, "k": 1}},
            {"ty": 7, "nm": "Repeat Edge Pixels", "mn": "ADBE Gaussian Blur 2-0003", "ix": 3, "v": {"a": 0, "k": 0}}
        ]}));
    }
    out
}

/// Affine maps as Lottie layer transforms, T(p)·R(r°)·S(sx, sy), the
/// rotation unwrapped frame to frame; `None` if any of them skews.
fn decompose(m: &[Affine]) -> Option<Vec<(Point, f64, f64, f64)>> {
    let mut out: Vec<(Point, f64, f64, f64)> = Vec::with_capacity(m.len());
    for a in m {
        let [a0, b0, c0, d0, e, f] = a.as_coeffs();
        let sx = a0.hypot(b0);
        if sx < 1e-9 {
            return None;
        }
        let r = b0.atan2(a0);
        // R(−r)·M: [[sx, k], [0, sy]].
        let (cs, sn) = (r.cos(), r.sin());
        let k = cs * c0 + sn * d0;
        let sy = -sn * c0 + cs * d0;
        if k.abs() > 1e-6 * sx.max(sy.abs()) {
            return None;
        }
        let mut deg = r.to_degrees();
        if let Some(prev) = out.last() {
            deg += ((prev.1 - deg) / 360.0).round() * 360.0;
        }
        out.push((Point::new(e, f), deg, sx, sy));
    }
    Some(out)
}

/// An image fill as an image layer placed like the fill, masked to the
/// shape's outline (both following the shape from frame to frame).
fn image_layer(pages: &[Document], frames: &[Vec<Item>], i: usize, same: bool, img: &crate::document::ImageFill, assets: &mut Vec<Value>, n: usize) -> Option<Value> {
    use base64::Engine;
    let asset = pages[0].images.get(&img.image)?;
    let (iw, ih) = (asset.width.max(1) as f64, asset.height.max(1) as f64);
    let id = format!("image_{}", img.image);
    if !assets.iter().any(|a| a["id"] == id.as_str()) {
        let data = base64::engine::general_purpose::STANDARD.encode(asset.png_bytes());
        assets.push(json!({"id": id, "w": iw as i64, "h": ih as i64, "u": "", "p": format!("data:image/png;base64,{data}"), "e": 1}));
    }
    let at = |f: usize| if same { &frames[f][i] } else { &frames[0][i] };
    // Image pixels to the document, per frame.
    let place: Vec<Affine> = (0..=n)
        .map(|f| {
            let it = at(f);
            let lb = crate::render::shape_local_bounds(&it.shape, &it.path);
            let r = crate::render::image_rect(img.mode, lb, iw, ih);
            it.shape.transform * Affine::new([r.width() / iw, 0.0, 0.0, r.height() / ih, r.x0, r.y0])
        })
        .collect();
    let xf = decompose(&place)?;
    let masks: Vec<Vec<Value>> = (0..=n).map(|f| lottie_paths(&(place[f].inverse() * at(f).path.clone()))).collect();
    let mut layer = json!({
        "ddd": 0, "ty": 2, "nm": format!("{} image", at(0).shape.name), "refId": id, "sr": 1, "ao": 0, "bm": 0, "ip": 0, "op": n, "st": 0,
        "ks": {
            "o": prop((0..=n).map(|f| json!(r3(at(f).shape.opacity as f64 * img.opacity as f64 * 100.0))).collect()),
            "r": prop(xf.iter().map(|(_, r, ..)| json!(r3(*r))).collect()),
            "p": prop(xf.iter().map(|(p, ..)| json!([r3(p.x), r3(p.y), 0])).collect()),
            "a": {"a": 0, "k": [0, 0, 0]},
            "s": prop(xf.iter().map(|(_, _, sx, sy)| json!([r3(sx * 100.0), r3(sy * 100.0), 100])).collect()),
        },
    });
    if masks.iter().all(|m| m.len() == masks[0].len()) && !masks[0].is_empty() {
        let list: Vec<Value> = (0..masks[0].len())
            .map(|j| {
                let len = |v: &Value| v["v"].as_array().map_or(0, |a| a.len());
                let most = masks.iter().map(|m| len(&m[j])).max().unwrap_or(0);
                json!({"mode": if j == 0 { "a" } else { "f" }, "inv": false, "nm": format!("Mask {}", j + 1), "o": {"a": 0, "k": 100}, "x": {"a": 0, "k": 0}, "pt": prop(masks.iter().map(|m| pad_vertices(&m[j], most)).collect())})
            })
            .collect();
        layer["hasMask"] = json!(true);
        layer["masksProperties"] = json!(list);
    }
    Some(layer)
}

/// Adds an item's layers; a luminance or alpha mask becomes a track matte
/// over the topmost of them.
fn finish_item(layers: &mut Vec<Value>, mut mine: Vec<Value>, frames: &[Vec<Item>], i: usize, same: bool, n: usize) {
    let at = |f: usize| if same { &frames[f][i] } else { &frames[0][i] };
    let matte = at(0).mask.as_ref().and_then(|(items0, mode, invert)| {
        let count = items0.len();
        let frames_ok = (0..=n).all(|f| at(f).mask.as_ref().is_some_and(|m| m.0.len() == count));
        let mask_at = |f: usize, j: usize| if frames_ok { &at(f).mask.as_ref().unwrap().0[j] } else { &items0[j] };
        let mut groups = Vec::new();
        // Earlier items paint on top.
        for j in (0..count).rev() {
            let m0 = &items0[j];
            if m0.span > 0 || m0.path.is_empty() {
                continue;
            }
            let paths: Vec<Vec<Value>> = (0..=n).map(|f| lottie_paths(&mask_at(f, j).path)).collect();
            if !paths.iter().all(|p| p.len() == paths[0].len()) {
                continue;
            }
            let mut it: Vec<Value> = (0..paths[0].len())
                .map(|k| {
                    let len = |v: &Value| v["v"].as_array().map_or(0, |a| a.len());
                    let most = paths.iter().map(|p| len(&p[k])).max().unwrap_or(0);
                    json!({"ty": "sh", "nm": format!("Path {}", k + 1), "ks": prop(paths.iter().map(|p| pad_vertices(&p[k], most)).collect())})
                })
                .collect();
            let color = |f: usize| mask_at(f, j).shape.fill.as_ref().and_then(paint_color).unwrap_or(Rgba::WHITE);
            let (cs, os): (Vec<_>, Vec<_>) = (0..=n).map(|f| {
                let (c, a) = rgba(color(f));
                (c, a * mask_at(f, j).shape.opacity as f64)
            }).unzip();
            let rule = if m0.shape.fill_rule == FillRule::EvenOdd { 2 } else { 1 };
            it.push(json!({"ty": "fl", "nm": "Fill", "c": prop(cs), "o": prop(os.into_iter().map(|o| json!(r3(o))).collect()), "r": rule}));
            it.push(json!({"ty": "tr", "p": {"a": 0, "k": [0, 0]}, "a": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}, "r": {"a": 0, "k": 0}, "o": {"a": 0, "k": 100}}));
            groups.push(json!({"ty": "gr", "nm": "Mask", "it": it}));
        }
        if groups.is_empty() {
            return None;
        }
        let tt = match (mode, invert) {
            (crate::document::MaskMode::Alpha, false) => 1,
            (crate::document::MaskMode::Alpha, true) => 2,
            (_, false) => 3,
            (_, true) => 4,
        };
        Some((tt, json!({
            "ddd": 0, "ty": 4, "nm": "Mask", "td": 1, "sr": 1, "ao": 0, "bm": 0, "ip": 0, "op": n, "st": 0,
            "ks": {"o": {"a": 0, "k": 100}, "r": {"a": 0, "k": 0}, "p": {"a": 0, "k": [0, 0, 0]}, "a": {"a": 0, "k": [0, 0, 0]}, "s": {"a": 0, "k": [100, 100, 100]}},
            "shapes": groups,
        })))
    });
    match (matte, mine.last_mut()) {
        (Some((tt, layer)), Some(top)) => {
            top["tt"] = json!(tt);
            layers.extend(mine);
            layers.push(layer);
        }
        _ => layers.extend(mine),
    }
}

/// The pages as a Lottie (Bodymovin) animation: one shape layer per export
/// item, its outlines keyed per frame where they move, and keyed fill
/// (solid or linear / radial gradient), stroke (dashes too), opacity,
/// blend mode, drop shadow and blur effects, clip masks, image fills (image
/// layers masked to the shape) and luminance / alpha masks (track mattes).
/// Inner shadows and noise are left out; conic and mesh gradients become
/// their first colour; text is outlined.
pub fn lottie(pages: &[Document], fps: f64, name: &str, fonts: &mut FontManager) -> String {
    let pages = &show_hidden(pages);
    let frames = items(pages, fonts, false);
    let moves = motions(&frames);
    let page = &pages[0];
    let (w, h) = (page.primary().w, page.primary().h);
    let n = pages.len() - 1;
    let same = frames.iter().all(|f| f.len() == frames[0].len());
    let mut layers = Vec::new();
    let mut assets: Vec<Value> = Vec::new();
    let transform = json!({"ty": "tr", "p": {"a": 0, "k": [0, 0]}, "a": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}, "r": {"a": 0, "k": 0}, "o": {"a": 0, "k": 100}, "sk": {"a": 0, "k": 0}, "sa": {"a": 0, "k": 0}});
    for (i, it0) in frames[0].iter().enumerate() {
        if it0.span > 0 || it0.path.is_empty() {
            continue;
        }
        let at = |f: usize| if same { &frames[f][i] } else { it0 };
        let s0 = &it0.shape;
        let mut it = Vec::new();
        // Motion as the layer's position, rotation and scale; outlines keyed
        // per frame only for what that can't say (skew).
        let layer_xf: Option<Vec<(Point, f64, f64, f64)>> = if same { moves[i].as_deref().and_then(decompose) } else { None };
        let paths: Vec<Vec<Value>> = match layer_xf {
            Some(_) => vec![lottie_paths(&it0.path); n + 1],
            None => (0..=n).map(|f| lottie_paths(&at(f).path)).collect(),
        };
        // Stroke widths in the layer's space.
        let k = |f: usize| layer_xf.as_ref().map(|x| (x[f].2 * x[f].3).abs().sqrt().max(1e-9)).unwrap_or(1.0);
        // Document space to the layer's, per frame.
        let to_layer = |f: usize| match (&layer_xf, &moves[i]) {
            (Some(_), Some(m)) => m[f].inverse(),
            _ => Affine::IDENTITY,
        };
        let shape_count = paths[0].len();
        let consistent = paths.iter().all(|p| p.len() == shape_count);
        for k in 0..shape_count {
            let ks = if consistent {
                // Players need the same vertex count in every key.
                let len = |v: &Value| v["v"].as_array().map_or(0, |a| a.len());
                let most = paths.iter().map(|p| len(&p[k])).max().unwrap_or(0);
                let vs: Vec<Value> = paths.iter().map(|p| pad_vertices(&p[k], most)).collect();
                prop(vs)
            } else {
                json!({"a": 0, "k": paths[0][k]})
            };
            it.push(json!({"ty": "sh", "nm": format!("Path {}", k + 1), "ks": ks}));
        }
        // Stroke above fill: earlier items paint on top.
        if let Some(st) = &s0.stroke {
            if st.width > 0.0 && st.color.a > 0.0 {
                let (cs, os): (Vec<_>, Vec<_>) = (0..=n).map(|f| rgba(at(f).shape.stroke.as_ref().map(|s| s.color).unwrap_or(st.color))).unzip();
                let ws: Vec<Value> = (0..=n).map(|f| json!(r3(at(f).shape.stroke.as_ref().map(|s| s.width).unwrap_or(st.width) / k(f)))).collect();
                let lc = match st.cap {
                    LineCap::Butt => 1,
                    LineCap::Round => 2,
                    LineCap::Square => 3,
                };
                let lj = match st.join {
                    LineJoin::Miter => 1,
                    LineJoin::Round => 2,
                    LineJoin::Bevel => 3,
                };
                let mut stroke = json!({"ty": "st", "nm": "Stroke", "c": prop(cs), "o": prop(os.into_iter().map(Value::from).collect()), "w": prop(ws), "lc": lc, "lj": lj, "ml": r3(st.miter_limit)});
                if !st.dash.is_empty() {
                    // Lengths in the layer's space, per frame (the pattern of
                    // frame 0 where the number of dashes changes).
                    let at_f = |f: usize| at(f).shape.stroke.clone().filter(|s| s.dash.len() == st.dash.len()).unwrap_or_else(|| st.clone());
                    let mut d: Vec<Value> = (0..st.dash.len())
                        // Names must differ: players key the entries by name.
                        .map(|j| json!({"n": if j % 2 == 0 { "d" } else { "g" }, "nm": format!("{}{}", if j % 2 == 0 { "Dash" } else { "Gap" }, if j < 2 { String::new() } else { format!(" {}", j / 2 + 1) }), "v": prop((0..=n).map(|f| { let s = at_f(f); json!(r3(s.dash[j] * s.width / k(f))) }).collect())}))
                        .collect();
                    d.push(json!({"n": "o", "nm": "Offset", "v": prop((0..=n).map(|f| { let s = at_f(f); json!(r3(s.dash_offset * s.width / k(f))) }).collect())}));
                    stroke["d"] = json!(d);
                }
                it.push(stroke);
            }
        }
        let grad = |f: usize| match &at(f).shape.fill {
            Some(Paint::Gradient(g)) if g.mesh.is_none() && matches!(g.kind, crate::document::GradientKind::Linear | crate::document::GradientKind::Radial) => Some(g.clone()),
            _ => None,
        };
        if let Some(g0) = grad(0) {
            let r = if s0.fill_rule == FillRule::EvenOdd { 2 } else { 1 };
            let count = g0.linear_stops().len();
            let (mut ss, mut es, mut ks) = (Vec::new(), Vec::new(), Vec::new());
            for f in 0..=n {
                let g = grad(f).filter(|g| g.kind == g0.kind && g.linear_stops().len() == count).unwrap_or_else(|| g0.clone());
                let it = at(f);
                // Box units to the document, then to the layer.
                let lb = crate::render::shape_local_bounds(&it.shape, &it.path);
                let m = to_layer(f) * it.shape.transform * Affine::new([lb.width().max(1e-9), 0.0, 0.0, lb.height().max(1e-9), lb.x0, lb.y0]);
                ss.push(pt(m * Point::new(g.start[0], g.start[1])));
                es.push(pt(m * Point::new(g.end[0], g.end[1])));
                let stops = g.linear_stops();
                let mut v: Vec<Value> = Vec::new();
                for st in &stops {
                    v.extend([json!(r3(st.offset as f64)), json!(r3(st.color.r as f64)), json!(r3(st.color.g as f64)), json!(r3(st.color.b as f64))]);
                }
                for st in &stops {
                    v.extend([json!(r3(st.offset as f64)), json!(r3(st.color.a as f64))]);
                }
                ks.push(Value::Array(v));
            }
            let t = if g0.kind == crate::document::GradientKind::Radial { 2 } else { 1 };
            it.push(json!({"ty": "gf", "nm": "Gradient Fill", "o": {"a": 0, "k": 100}, "r": r, "t": t, "s": prop(ss), "e": prop(es), "h": {"a": 0, "k": 0}, "a": {"a": 0, "k": 0}, "g": {"p": count, "k": prop(ks)}}));
        } else if let Some(c0) = s0.fill.as_ref().and_then(paint_color) {
            let (cs, os): (Vec<_>, Vec<_>) = (0..=n).map(|f| rgba(at(f).shape.fill.as_ref().and_then(paint_color).unwrap_or(c0))).unzip();
            let r = if s0.fill_rule == FillRule::EvenOdd { 2 } else { 1 };
            it.push(json!({"ty": "fl", "nm": "Fill", "c": prop(cs), "o": prop(os.into_iter().map(Value::from).collect()), "r": r}));
        }
        // This item's layers, bottom first.
        let mut mine: Vec<Value> = Vec::new();
        if let Some(Paint::Image(img)) = &s0.fill
            && let Some(layer) = image_layer(pages, &frames, i, same, img, &mut assets, n)
        {
            mine.push(layer);
        }
        if it.len() == shape_count {
            finish_item(&mut layers, mine, &frames, i, same, n);
            continue;
        }
        it.push(transform.clone());
        let opacity = prop((0..=n).map(|f| json!(r3(at(f).shape.opacity as f64 * 100.0))).collect());
        let (p, r, sc) = match &layer_xf {
            Some(x) => (
                prop(x.iter().map(|(p, ..)| json!([r3(p.x), r3(p.y), 0])).collect()),
                prop(x.iter().map(|(_, r, ..)| json!(r3(*r))).collect()),
                prop(x.iter().map(|(_, _, sx, sy)| json!([r3(sx * 100.0), r3(sy * 100.0), 100])).collect()),
            ),
            None => (json!({"a": 0, "k": [0, 0, 0]}), json!({"a": 0, "k": 0}), json!({"a": 0, "k": [100, 100, 100]})),
        };
        let nm = if s0.name.is_empty() { s0.kind.type_name().to_string() } else { s0.name.clone() };
        let bm = crate::document::BlendMode::ALL.iter().position(|b| *b == s0.blend).unwrap_or(0);
        let mut layer = json!({
            "ddd": 0, "ty": 4, "nm": nm, "sr": 1, "ao": 0, "bm": bm, "ip": 0, "op": n, "st": 0,
            "ks": {"o": opacity, "r": r, "p": p, "a": {"a": 0, "k": [0, 0, 0]}, "s": sc},
            "shapes": [{"ty": "gr", "nm": "Shape", "it": it}],
        });
        let effects = lottie_effects(&(0..=n).map(|f| (at(f).shape.shadow.clone(), at(f).shape.blur, k(f))).collect::<Vec<_>>());
        if !effects.is_empty() {
            layer["ef"] = json!(effects);
        }
        // A clip as layer masks (the first adds, the rest toggle: even-odd).
        if let Some((c0, _)) = &it0.clip {
            let clips: Vec<Vec<Value>> = (0..=n).map(|f| lottie_paths(&(to_layer(f) * at(f).clip.as_ref().map_or_else(|| c0.clone(), |c| c.0.clone())))).collect();
            if clips.iter().all(|c| c.len() == clips[0].len()) {
                let masks: Vec<Value> = (0..clips[0].len())
                    .map(|j| {
                        let len = |v: &Value| v["v"].as_array().map_or(0, |a| a.len());
                        let most = clips.iter().map(|c| len(&c[j])).max().unwrap_or(0);
                        json!({"mode": if j == 0 { "a" } else { "f" }, "inv": false, "nm": format!("Mask {}", j + 1), "o": {"a": 0, "k": 100}, "x": {"a": 0, "k": 0}, "pt": prop(clips.iter().map(|c| pad_vertices(&c[j], most)).collect())})
                    })
                    .collect();
                layer["hasMask"] = json!(true);
                layer["masksProperties"] = json!(masks);
            }
        }
        mine.push(layer);
        finish_item(&mut layers, mine, &frames, i, same, n);
    }
    let bg = page.primary().background;
    if bg.a > 0.0 {
        layers.push(json!({
            "ddd": 0, "ty": 1, "nm": "Background", "sr": 1, "ao": 0, "bm": 0, "ip": 0, "op": n, "st": 0,
            "sc": bg.to_hex().to_lowercase(), "sw": w.round() as i64, "sh": h.round() as i64,
            "ks": {"o": {"a": 0, "k": r3(bg.a as f64 * 100.0)}, "r": {"a": 0, "k": 0}, "p": {"a": 0, "k": [0, 0, 0]}, "a": {"a": 0, "k": [0, 0, 0]}, "s": {"a": 0, "k": [100, 100, 100]}},
        }));
    }
    // Top layer first.
    let paint_order = layers.len() - usize::from(bg.a > 0.0);
    layers[..paint_order].reverse();
    for (k, l) in layers.iter_mut().enumerate() {
        l["ind"] = json!(k + 1);
    }
    json!({"v": "5.7.0", "fr": fps, "ip": 0, "op": n, "w": w.round() as i64, "h": h.round() as i64, "nm": name, "ddd": 0, "assets": assets, "layers": layers}).to_string()
}
