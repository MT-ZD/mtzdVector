//! Keyframe tools: `animate` writes keys straight into shapes' tracks (the
//! way the timeline does), `get_animation` reads them back compactly.

use serde_json::{Value as Json, json};

use crate::animation::{Ease, Prop, Timeline, Value};
use crate::app::App;
use crate::document::Rgba;

fn ease(v: Option<&Json>) -> Result<Ease, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(Ease::EaseInOut) };
    if let Some(a) = v.as_array() {
        let n: Vec<f64> = a.iter().filter_map(|x| x.as_f64()).collect();
        return match n[..] {
            [a, b, c, d] => Ok(Ease::Bezier([a, b, c, d])),
            _ => Err("a bezier ease is [x1, y1, x2, y2]".into()),
        };
    }
    let s = v.as_str().ok_or("ease is a name or [x1, y1, x2, y2]")?.to_lowercase().replace(['-', ' '], "_");
    Ok(match s.as_str() {
        "linear" => Ease::Linear,
        "ease_in_out" | "ease" => Ease::EaseInOut,
        "smooth" | "auto" | "catmull_rom" | "spline" => Ease::Smooth,
        "ease_in" | "accelerate" | "gravity" => Ease::EaseIn,
        "ease_out" | "decelerate" => Ease::EaseOut,
        "hold" | "step" => Ease::Hold,
        "back_in" | "anticipate" => Ease::Bezier([0.36, 0.0, 0.66, -0.56]),
        "back_out" | "overshoot" => Ease::Bezier([0.34, 1.56, 0.64, 1.0]),
        "back_in_out" => Ease::Bezier([0.68, -0.5, 0.32, 1.5]),
        "css" => Ease::Bezier([0.25, 0.1, 0.25, 1.0]),
        _ => return Err(format!("unknown ease {s} (linear, smooth, ease_in_out, ease_in, ease_out, hold, back_in, back_out, back_in_out, or [x1, y1, x2, y2])")),
    })
}

fn ease_name(e: Ease) -> Json {
    match e {
        Ease::Linear => json!("linear"),
        Ease::EaseInOut => json!("ease_in_out"),
        Ease::EaseIn => json!("ease_in"),
        Ease::EaseOut => json!("ease_out"),
        Ease::Hold => json!("hold"),
        Ease::Smooth => json!("smooth"),
        Ease::Bezier(b) => json!(b),
    }
}

fn color(v: &Json) -> Result<Rgba, String> {
    v.as_str().and_then(Rgba::from_hex).ok_or(format!("bad colour {v} (use #RRGGBB)"))
}

fn num(v: &Json, what: &str) -> Result<f64, String> {
    v.as_f64().ok_or(format!("{what} must be a number"))
}

fn pair(v: &Json) -> Option<(f64, f64)> {
    match v {
        Json::Number(n) => n.as_f64().map(|s| (s, s)),
        Json::Array(a) => Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)),
        _ => None,
    }
}

/// Fields of a key that set something other than the transform.
const PROPS: [(&str, Prop); 10] = [
    ("opacity", Prop::Opacity),
    ("fill", Prop::Fill),
    ("stroke", Prop::StrokeColor),
    ("stroke_width", Prop::StrokeWidth),
    ("blur", Prop::Blur),
    ("font_size", Prop::FontSize),
    ("letter_spacing", Prop::LetterSpacing),
    ("text", Prop::Text),
    ("visible", Prop::Visible),
    ("dash_offset", Prop::DashOffset),
];
const XF: [&str; 5] = ["move", "center", "rotate", "scale", "skew"];

/// A pivot from `origin`: [x, y] in document space (where the shapes rest)
/// or an anchor of their rest bounds ("center", "top", "bottom-left"…).
fn origin_point(v: &Json, rest: kurbo::Rect) -> Result<kurbo::Point, String> {
    if let Some((x, y)) = pair(v).filter(|_| v.is_array()) {
        return Ok(kurbo::Point::new(x, y));
    }
    let name = v.as_str().ok_or("origin is [x, y] or an anchor name")?.to_lowercase().replace(['_', ' '], "-");
    let (fx, fy) = match name.as_str() {
        "center" | "centre" => (0.5, 0.5),
        "top" => (0.5, 0.0),
        "bottom" => (0.5, 1.0),
        "left" => (0.0, 0.5),
        "right" => (1.0, 0.5),
        "top-left" => (0.0, 0.0),
        "top-right" => (1.0, 0.0),
        "bottom-left" => (0.0, 1.0),
        "bottom-right" => (1.0, 1.0),
        _ => return Err(format!("unknown anchor {name} (center, top, bottom, left, right, top-left, top-right, bottom-left, bottom-right, or [x, y])")),
    };
    Ok(kurbo::Point::new(rest.x0 + fx * rest.width(), rest.y0 + fy * rest.height()))
}

/// A sine layered on a property: `amp · sin(2π·freq·t + phase) · envelope(t)`.
struct Osc {
    field: String,
    amp: f64,
    freq: f64,
    phase: f64,
    from: f64,
    to: f64,
    /// (t, gain) points, smooth between; empty: 1 throughout.
    envelope: Vec<(f64, f64)>,
}

const OSC_FIELDS: [&str; 9] = ["rotate", "move_x", "move_y", "scale", "scale_x", "scale_y", "skew", "opacity", "blur"];

fn oscillators(args: &Json, duration: f64) -> Result<Vec<Osc>, String> {
    let Some(list) = args.get("oscillate") else { return Ok(Vec::new()) };
    let list: Vec<&Json> = match list {
        Json::Array(a) => a.iter().collect(),
        o => vec![o],
    };
    list.into_iter()
        .map(|o| {
            let field = o.get("prop").or(o.get("track")).and_then(|v| v.as_str()).ok_or("each oscillate needs prop")?.to_string();
            if !OSC_FIELDS.contains(&field.as_str()) {
                return Err(format!("oscillate prop is one of {}", OSC_FIELDS.join(", ")));
            }
            let amp = o.get("amp").and_then(|v| v.as_f64()).ok_or("oscillate needs amp")?;
            let freq = o.get("freq").and_then(|v| v.as_f64()).ok_or("oscillate needs freq (cycles per second)")?;
            if !(freq > 0.0 && freq <= 30.0) {
                return Err("oscillate freq is 0..30 cycles per second".into());
            }
            let envelope: Vec<(f64, f64)> = match o.get("envelope").and_then(|v| v.as_array()) {
                Some(e) => e.iter().map(|p| pair(p).filter(|_| p.is_array()).ok_or("envelope is [[t, gain], …]")).collect::<Result<_, _>>()?,
                None => Vec::new(),
            };
            let span = (envelope.first().map(|p| p.0), envelope.last().map(|p| p.0));
            Ok(Osc {
                field,
                amp,
                freq,
                phase: o.get("phase").and_then(|v| v.as_f64()).unwrap_or(0.0).to_radians(),
                from: o.get("from").and_then(|v| v.as_f64()).or(span.0).unwrap_or(0.0).max(0.0),
                to: o.get("to").and_then(|v| v.as_f64()).or(span.1).unwrap_or(duration).min(duration),
                envelope,
            })
        })
        .collect()
}

impl Osc {
    fn gain(&self, t: f64) -> f64 {
        let e = &self.envelope;
        if e.is_empty() {
            return 1.0;
        }
        match e.iter().position(|p| p.0 > t) {
            Some(0) => e[0].1,
            None => e[e.len() - 1].1,
            Some(i) => {
                let (a, b) = (e[i - 1], e[i]);
                let u = ((t - a.0) / (b.0 - a.0).max(1e-9)).clamp(0.0, 1.0);
                a.1 + (b.1 - a.1) * u * u * (3.0 - 2.0 * u)
            }
        }
    }

    fn at(&self, t: f64) -> f64 {
        self.amp * (std::f64::consts::TAU * self.freq * t + self.phase).sin() * self.gain(t)
    }
}

/// Bake `osc` into the shape's keys: samples (smooth between) on top of what
/// the keys give, replacing the keys inside its span.
fn bake(s: &mut crate::document::Shape, osc: &Osc, fps: f64) {
    if osc.to <= osc.from {
        return;
    }
    let step = (1.0 / (osc.freq * 12.0)).max(1.0 / fps);
    let n = ((osc.to - osc.from) / step).ceil().max(1.0) as usize;
    let times: Vec<f64> = (0..=n).map(|i| osc.from + (osc.to - osc.from) * i as f64 / n as f64).collect();
    let prop = match osc.field.as_str() {
        "opacity" => Prop::Opacity,
        "blur" => Prop::Blur,
        _ => Prop::Transform,
    };
    // Without a track the value is the shape's own.
    let own = match prop {
        Prop::Opacity => s.opacity as f64,
        Prop::Blur => s.blur,
        _ => 0.0,
    };
    let anim = s.anim.get_or_insert_with(Default::default);
    let base: Vec<Value> = times
        .iter()
        .map(|t| match prop {
            Prop::Transform => Value::Xf(anim.xf_at(*t).unwrap_or_default()),
            p => anim.track(p).and_then(|tr| tr.value_at(*t)).unwrap_or(Value::Num(own)),
        })
        .collect();
    let tr = anim.track_mut(prop);
    tr.keys.retain(|k| k.t < osc.from - 1e-9 || k.t > osc.to + 1e-9);
    for (t, b) in times.iter().zip(base) {
        let d = osc.at(*t);
        let v = match b {
            Value::Xf(mut x) => {
                match osc.field.as_str() {
                    "rotate" => x.rot += d.to_radians(),
                    "move_x" => x.dx += d,
                    "move_y" => x.dy += d,
                    "scale" => (x.sx, x.sy) = (x.sx + d, x.sy + d),
                    "scale_x" => x.sx += d,
                    "scale_y" => x.sy += d,
                    "skew" => x.skew = (x.skew.atan() + d.to_radians()).tan(),
                    _ => {}
                }
                Value::Xf(x)
            }
            Value::Num(n) => Value::Num(if prop == Prop::Opacity { (n + d).clamp(0.0, 1.0) } else { (n + d).max(0.0) }),
            v => v,
        };
        tr.set(*t, v);
        if let Some(i) = tr.key_at(*t) {
            tr.keys[i].ease = Ease::Smooth;
        }
    }
}

/// Keys for one shape, written into its tracks.
pub fn animate(app: &mut App, args: &Json) -> Result<Json, String> {
    let ids: Vec<u64> = match args.get("ids").and_then(|v| v.as_array()) {
        Some(a) => a.iter().filter_map(|v| v.as_u64()).collect(),
        None => args.get("id").and_then(|v| v.as_u64()).into_iter().collect(),
    };
    if ids.is_empty() {
        return Err("pass `id` (or `ids`)".into());
    }
    for id in &ids {
        app.doc.get(*id).ok_or(format!("no shape with id {id}"))?;
    }
    let empty = Vec::new();
    let keys = args.get("keys").and_then(|v| v.as_array()).unwrap_or(&empty);
    if keys.is_empty() && args.get("oscillate").is_none() && args.get("origin").is_none() {
        return Err("pass keys: [{t, move/center/rotate/scale/opacity/fill…, ease}] (and/or oscillate, origin)".into());
    }
    let mut keys: Vec<&Json> = keys.iter().collect();
    keys.sort_by(|a, b| a["t"].as_f64().unwrap_or(0.0).total_cmp(&b["t"].as_f64().unwrap_or(0.0)));
    let last_t = keys.iter().filter_map(|k| k["t"].as_f64()).fold(0.0, f64::max);
    let default_ease = ease(args.get("ease"))?;
    for k in &keys {
        let t = k["t"].as_f64().ok_or("every key needs `t` (seconds)")?;
        if t < 0.0 {
            return Err("key times start at 0".into());
        }
        ease(k.get("ease"))?;
        if let Some(bad) = k.as_object().and_then(|o| o.keys().find(|f| !["t", "ease"].contains(&f.as_str()) && !XF.contains(&f.as_str()) && !PROPS.iter().any(|p| p.0 == f.as_str()))) {
            return Err(format!("unknown key field `{bad}` (use t, ease, move, center, rotate, scale, skew, {})", PROPS.map(|p| p.0).join(", ")));
        }
    }
    let replace = args.get("replace").and_then(|v| v.as_bool()).unwrap_or(true);
    // Timeline: long enough for the keys unless told otherwise.
    let tl = app.doc.animation.clone().unwrap_or_default();
    let duration = match args.get("duration").and_then(|v| v.as_f64()) {
        Some(d) => d.max(0.05),
        None => tl.duration.max(last_t),
    };
    let oscs = oscillators(args, duration)?;
    // Where each shape rests: its centre (for `center`) and its pivot offset.
    app.anim_sync();
    let now = app.anim_time();
    let mut rest_center = std::collections::HashMap::new();
    let mut origins = std::collections::HashMap::new();
    for id in &ids {
        let s = app.doc.get(*id).cloned().unwrap();
        let b = app.cache.update(&s, &mut app.fonts).bounds.unwrap_or_default();
        let x = s.anim.as_deref().and_then(|a| a.xf_at(now)).unwrap_or_default();
        rest_center.insert(*id, b.center() - kurbo::Vec2::new(x.dx, x.dy));
        if let Some(o) = args.get("origin") {
            let rest = crate::animation::at_rest(&app.doc, *id).and_then(|r| outline_bounds(&r, &mut app.fonts)).unwrap_or(b);
            let p = origin_point(o, rest)?;
            let c = crate::animation::rest_centre(&app.doc, *id, now).unwrap_or(rest.center());
            origins.insert(*id, p - c);
        }
    }
    app.checkpoint("");
    app.anim_sync();
    let tl = app.doc.animation.get_or_insert_with(Timeline::default);
    tl.duration = duration;
    if let Some(f) = args.get("fps").and_then(|v| v.as_f64()) {
        tl.fps = f.clamp(1.0, 240.0);
    }
    if let Some(l) = args.get("loop").and_then(|v| v.as_bool()) {
        tl.looping = l;
    }
    let fps = tl.fps;
    let mut err = None;
    app.anim_edit_quiet(|doc| {
        for id in &ids {
            let Some(mut s) = doc.get(*id).cloned() else { continue };
            let anim = s.anim.get_or_insert_with(Default::default);
            if let Some(e) = origins.get(id) {
                anim.origin = (e.hypot() > 1e-9).then_some([e.x, e.y]);
            }
            let touched_xf = keys.iter().any(|k| XF.iter().any(|f| k.get(*f).is_some()));
            if replace {
                anim.tracks.retain(|tr| !(tr.prop == Prop::Transform && touched_xf || PROPS.iter().any(|(f, p)| *p == tr.prop && keys.iter().any(|k| k.get(*f).is_some()))));
            }
            for k in &keys {
                let t = k["t"].as_f64().unwrap_or(0.0);
                let e = k.get("ease").map_or(Ok(default_ease), |v| ease(Some(v))).unwrap_or_default();
                let put = |anim: &mut crate::animation::Anim, p: Prop, v: Value| {
                    let tr = anim.track_mut(p);
                    tr.set(t, v);
                    if let Some(i) = tr.key_at(t) {
                        tr.keys[i].ease = e;
                    }
                };
                if XF.iter().any(|f| k.get(*f).is_some()) {
                    // Unset parts carry on from the keys before.
                    let mut x = anim.xf_at(t).unwrap_or_default();
                    let r = (|| -> Result<(), String> {
                        if let Some(m) = k.get("move") {
                            (x.dx, x.dy) = pair(m).ok_or("move is [dx, dy]")?;
                        }
                        if let Some(c) = k.get("center") {
                            let (cx, cy) = pair(c).ok_or("center is [x, y]")?;
                            let rc = rest_center[id];
                            (x.dx, x.dy) = (cx - rc.x, cy - rc.y);
                        }
                        if let Some(r) = k.get("rotate") {
                            x.rot = num(r, "rotate")?.to_radians();
                        }
                        if let Some(sc) = k.get("scale") {
                            (x.sx, x.sy) = pair(sc).ok_or("scale is a number or [sx, sy]")?;
                        }
                        if let Some(sk) = k.get("skew") {
                            x.skew = num(sk, "skew")?.to_radians().tan();
                        }
                        Ok(())
                    })();
                    if let Err(e) = r {
                        err = Some(e);
                        return;
                    }
                    put(anim, Prop::Transform, Value::Xf(x));
                }
                for (f, p) in PROPS {
                    let Some(v) = k.get(f) else { continue };
                    let val = match p {
                        Prop::Fill | Prop::StrokeColor => color(v).map(Value::Color),
                        Prop::Text => v.as_str().map(|s| Value::Text(s.into())).ok_or("text must be a string".into()),
                        Prop::Visible => v.as_bool().map(Value::Flag).ok_or("visible must be true/false".into()),
                        _ => num(v, f).map(Value::Num),
                    };
                    match val {
                        Ok(val) => put(anim, p, val),
                        Err(e) => {
                            err = Some(e);
                            return;
                        }
                    }
                }
            }
            for o in &oscs {
                bake(&mut s, o, fps);
            }
            doc.replace(s);
        }
    });
    if let Some(e) = err {
        app.undo();
        return Err(e);
    }
    // Stay at the same moment, so a render shows what the playhead showed.
    app.anim_seek(now.min(duration));
    let total: usize = ids.iter().filter_map(|id| app.doc.get(*id)?.anim.as_deref().map(|a| a.tracks.iter().map(|t| t.keys.len()).sum::<usize>())).sum();
    let verbose = args.get("verbose").and_then(|v| v.as_bool()).unwrap_or(total <= 40);
    // Key times (all of them, or 24 spread evenly when there are more).
    let mut times: Vec<f64> = ids
        .iter()
        .filter_map(|id| app.doc.get(*id)?.anim.as_deref().cloned())
        .flat_map(|a| a.tracks.into_iter().flat_map(|t| t.keys.into_iter().map(|k| k.t)))
        .collect();
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    if times.len() > 24 {
        let n = times.len();
        times = (0..24).map(|i| times[i * (n - 1) / 23]).collect();
    }
    let mut out: Vec<Json> = ids.iter().map(|id| if verbose { tracks(app, *id) } else { summary(app, *id) }).collect();
    let mut envelope: Vec<Option<kurbo::Rect>> = vec![None; ids.len()];
    for t in &times {
        let frame = crate::animation::frame(&app.doc, *t);
        for (i, id) in ids.iter().enumerate() {
            let b = frame.get(*id).and_then(|s| outline_bounds(s, &mut app.fonts));
            envelope[i] = match (envelope[i], b) {
                (Some(a), Some(b)) => Some(a.union(b)),
                (a, b) => a.or(b),
            };
            if verbose {
                let at = out[i].as_object_mut().unwrap().entry("bounds_at_keys").or_insert(json!([]));
                at.as_array_mut().unwrap().push(json!({"t": t, "bounds": rect(b)}));
            }
        }
    }
    for (i, id) in ids.iter().enumerate() {
        out[i]["bounds_envelope"] = rect(envelope[i]);
        if let Some(c) = app.doc.get(*id).and_then(|s| s.anim.as_deref()?.origin).and_then(|[x, y]| Some(crate::animation::rest_centre(&app.doc, *id, app.anim_time())? + kurbo::Vec2::new(x, y))) {
            out[i]["origin"] = json!([(c.x * 100.0).round() / 100.0, (c.y * 100.0).round() / 100.0]);
        }
    }
    Ok(json!({"timeline": timeline(app), "animation": out}))
}

/// One shape's tracks in brief: key count and time span each.
fn summary(app: &App, id: u64) -> Json {
    let Some(s) = app.doc.get(id) else { return json!({"id": id, "error": "no such shape"}) };
    let mut out = serde_json::Map::new();
    for tr in s.anim.as_deref().map(|a| a.tracks.as_slice()).unwrap_or(&[]) {
        let (a, b) = (tr.keys.first().map_or(0.0, |k| k.t), tr.keys.last().map_or(0.0, |k| k.t));
        out.insert(format!("{:?}", tr.prop), json!({"keys": tr.keys.len(), "from": a, "to": b}));
    }
    json!({"id": id, "name": s.name, "tracks": out, "note": "summary: pass verbose=true (or get_animation) for every key"})
}

fn timeline(app: &App) -> Json {
    timeline_json(app)
}

pub fn timeline_json(app: &App) -> Json {
    app.doc.animation.as_ref().map(|t| json!({"duration": t.duration, "fps": t.fps, "time": t.time, "loop": t.looping, "range": t.range})).unwrap_or(Json::Null)
}

fn value_json(v: &Value) -> Json {
    match v {
        Value::Xf(x) => {
            let r = |v: f64| (v * 1000.0).round() / 1000.0;
            // Everything stored (skew only when used), identity values included.
            let mut o = serde_json::Map::new();
            o.insert("move".into(), json!([r(x.dx), r(x.dy)]));
            o.insert("rotate".into(), json!(r(x.rot.to_degrees())));
            o.insert("scale".into(), if (x.sx - x.sy).abs() < 1e-9 { json!(r(x.sx)) } else { json!([r(x.sx), r(x.sy)]) });
            if x.skew.abs() > 1e-9 {
                o.insert("skew".into(), json!(r(x.skew.atan().to_degrees())));
            }
            Json::Object(o)
        }
        Value::Num(n) => json!((n * 1000.0).round() / 1000.0),
        Value::Color(c) => json!(c.to_hex()),
        Value::Text(t) => json!(t),
        Value::Flag(b) => json!(b),
        Value::List(l) => json!(l),
        Value::Shape(_) => json!("(geometry)"),
        Value::Paint(_) => json!("(gradient)"),
    }
}

/// One shape's tracks: prop → keys [{t, value, ease}]. A key's ease shapes
/// the way to the next key, so the last one has none.
pub fn tracks(app: &App, id: u64) -> Json {
    let Some(s) = app.doc.get(id) else { return json!({"id": id, "error": "no such shape"}) };
    let mut out = serde_json::Map::new();
    for tr in s.anim.as_deref().map(|a| a.tracks.as_slice()).unwrap_or(&[]) {
        let n = tr.keys.len();
        let keys: Vec<Json> = tr
            .keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let mut o = json!({"t": (k.t * 1000.0).round() / 1000.0, "value": value_json(&k.value)});
                if i + 1 < n {
                    o["ease"] = ease_name(k.ease);
                }
                o
            })
            .collect();
        out.insert(format!("{:?}", tr.prop), json!(keys));
    }
    json!({"id": id, "name": s.name, "tracks": out})
}

/// Every animated shape (or `ids`), with the timeline.
pub fn get_animation(app: &App, args: &Json) -> Json {
    let ids: Vec<u64> = match args.get("ids").and_then(|v| v.as_array()) {
        Some(a) => a.iter().filter_map(|v| v.as_u64()).collect(),
        None => app.doc.all_ids().into_iter().filter(|id| app.doc.get(*id).is_some_and(|s| s.anim.as_deref().is_some_and(|a| !a.is_empty()))).collect(),
    };
    json!({"timeline": timeline(app), "shapes": ids.iter().map(|id| tracks(app, *id)).collect::<Vec<_>>()})
}

/// Remove keys of `ids` (all props, or `props`). `keep`: "rest" (default)
/// puts the shapes back where they rest (other props: their value at 0 s);
/// "current" keeps what the playhead shows now.
pub fn clear(app: &mut App, args: &Json) -> Result<Json, String> {
    let ids: Vec<u64> = args.get("ids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_u64()).collect()).unwrap_or_default();
    let props: Vec<String> = args.get("props").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default();
    let rest = match args.get("keep").and_then(|v| v.as_str()).unwrap_or("rest") {
        "rest" => true,
        "current" => false,
        k => return Err(format!("keep is \"rest\" or \"current\", not {k:?}")),
    };
    let gone = |p: Prop| props.is_empty() || props.contains(&format!("{p:?}").to_lowercase());
    app.checkpoint("");
    if rest {
        // First hold every cleared track at its rest value (the shapes go
        // there), then drop the tracks, which keeps what's shown.
        app.anim_edit_quiet(|doc| {
            for id in &ids {
                let Some(mut s) = doc.get(*id).cloned() else { continue };
                if let Some(a) = s.anim.as_deref_mut() {
                    for tr in a.tracks.iter_mut().filter(|t| gone(t.prop)) {
                        let v = if tr.prop == Prop::Transform { Some(Value::Xf(Default::default())) } else { tr.value_at(0.0) };
                        if let Some(v) = v {
                            tr.keys.truncate(1);
                            tr.keys[0].t = 0.0;
                            tr.keys[0].value = v;
                        }
                    }
                }
                doc.replace(s);
            }
        });
    }
    app.anim_edit_quiet(|doc| {
        for id in &ids {
            let Some(mut s) = doc.get(*id).cloned() else { continue };
            if let Some(a) = s.anim.as_deref_mut() {
                a.tracks.retain(|t| !gone(t.prop));
            }
            doc.replace(s);
        }
    });
    let mut out = get_animation(app, &json!({"ids": ids}));
    for (i, id) in ids.iter().enumerate() {
        out["shapes"][i]["bounds"] = shape_bounds(app, *id);
    }
    Ok(out)
}

/// Outline bounds [x, y, w, h] of `id` as shown now.
fn shape_bounds(app: &mut App, id: u64) -> Json {
    let Some(s) = app.doc.get(id).cloned() else { return Json::Null };
    rect(app.cache.update(&s, &mut app.fonts).bounds)
}

pub fn rect(r: Option<kurbo::Rect>) -> Json {
    let r1 = |v: f64| (v * 100.0).round() / 100.0;
    r.map_or(Json::Null, |r| json!([r1(r.x0), r1(r.y0), r1(r.width()), r1(r.height())]))
}

/// Outline bounds of a shape (a group: of its children), without the render
/// cache (which keys on revisions, not on copies at other moments).
pub fn outline_bounds(s: &crate::document::Shape, fonts: &mut crate::fonts::FontManager) -> Option<kurbo::Rect> {
    if let Some(children) = s.kind.children() {
        return children.iter().filter_map(|c| outline_bounds(c, fonts)).reduce(|a, b| a.union(b));
    }
    let (path, text) = crate::render::compute_outline(s, fonts);
    match text {
        Some(t) => Some(s.transform.transform_rect_bbox(t.bounds)),
        None => crate::geom::path_bounds(&path),
    }
}

/// Where shape `id` rests: its bounds with its motion (and its animated
/// ancestors') undone. None when it has no transform keys.
pub fn rest_bounds(app: &mut App, id: u64) -> Option<kurbo::Rect> {
    app.doc.get(id)?;
    let s = crate::animation::at_rest(&app.doc, id)?;
    outline_bounds(&s, &mut app.fonts)
}
