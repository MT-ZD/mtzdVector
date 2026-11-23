//! Tool helpers: answers that list only what an edit changed, paints from
//! JSON / CSS, components (instances with overrides by layer name), layout
//! (stacks, centring), stacking order and Iconify icons.

use std::collections::{BTreeMap, HashMap, HashSet};

use kurbo::Affine;
use serde_json::{Value, json};

use super::tools::{bounds, existing, ids_arg, ids_bounds, num_arg, point_arg, rect_json, str_arg};
use crate::app::App;
use crate::document::{Component, Document, Gradient, GradientKind, GradientStop, InstanceLink, Override, Paint, Rgba, Shape, ShapeKind};

// ---------------------------------------------------------------------------
// What changed
// ---------------------------------------------------------------------------

/// The document before an edit, to report what the edit changed.
pub struct Before {
    revs: HashMap<u64, u64>,
    doc: Document,
}

pub fn before(app: &App) -> Before {
    let revs = app.doc.all_ids().into_iter().filter_map(|id| Some((id, app.doc.get(id)?.rev))).collect();
    Before { revs, doc: app.doc.clone() }
}

fn unstamped(s: &Shape) -> Shape {
    fn clear(s: &mut Shape) {
        s.rev = 0;
        if let Some(c) = s.kind.children_mut() {
            c.iter_mut().for_each(clear);
        }
    }
    let mut s = s.clone();
    clear(&mut s);
    s
}

/// Main component / instance role, for listings.
pub fn component_json(s: &Shape) -> Option<Value> {
    match s.component.as_ref()? {
        Component::Main { .. } => Some(json!("main")),
        Component::Instance(l) => Some(json!({"instance_of": l.main})),
    }
}

/// A text's layout box (its frame, or its lines) in document space: what
/// to align by, unlike `bounds` (the letters' outlines).
pub fn text_box(app: &mut App, s: &Shape) -> Option<kurbo::Rect> {
    let ShapeKind::Text(td) = &s.kind else { return None };
    let r = match td.frame {
        Some([w, h]) => kurbo::Rect::new(0.0, 0.0, w, h),
        None => {
            let l = app.fonts.layout(td);
            let lines = l.lines.iter().filter(|l| !l.hidden);
            let (w, h) = lines.fold((0.0f64, 0.0f64), |(w, h), l| (w.max(l.x1), h.max(l.y1)));
            kurbo::Rect::new(0.0, 0.0, w, h)
        }
    };
    Some(s.transform.transform_rect_bbox(r))
}

/// id, name, type, bounds (+ component role, text box).
pub fn brief(app: &mut App, s: &Shape) -> Value {
    let mut v = json!({"id": s.id, "name": s.name, "type": s.kind.type_name()});
    if let Some(b) = bounds(app, s) {
        v["bounds"] = rect_json(b);
    }
    if let Some(c) = component_json(s) {
        v["component"] = c;
    }
    if let Some(l) = &s.layout {
        v["layout"] = layout_json(l);
    }
    if let Some(b) = text_box(app, s).map(rect_json).filter(|b| *b != v["bounds"]) {
        v["box"] = b;
    }
    v
}

/// Every layer inside `s` (not `s`), briefly, up to `max`.
fn layers(s: &Shape, max: usize) -> Vec<Value> {
    fn walk(list: &[Shape], out: &mut Vec<Value>, max: usize) {
        for c in list {
            if out.len() >= max {
                return;
            }
            let mut v = json!({"id": c.id, "name": c.name, "type": c.kind.type_name()});
            if let ShapeKind::Text(t) = &c.kind {
                v["text"] = json!(t.content.chars().take(40).collect::<String>());
            }
            out.push(v);
            if let Some(ch) = c.kind.children() {
                walk(ch, out, max);
            }
        }
    }
    let mut out = Vec::new();
    walk(s.kind.children().map(|c| c.as_slice()).unwrap_or(&[]), &mut out, max);
    out
}

/// Children ids of `parent` (None: top level), top first.
fn order(doc: &Document, parent: Option<u64>) -> Vec<u64> {
    let ids: Vec<u64> = match parent {
        None => doc.shapes().iter().map(|s| s.id).collect(),
        Some(p) => doc.get(p).and_then(|s| s.kind.children()).map(|c| c.iter().map(|s| s.id).collect()).unwrap_or_default(),
    };
    ids.into_iter().rev().collect()
}

/// What an edit did since `b`: shapes created (with their layers), changed
/// (the outermost shape of each edit) and deleted, and restacked parents.
pub fn changes(app: &mut App, b: &Before) -> Value {
    const MAX: usize = 40;
    let now: Vec<u64> = app.doc.all_ids();
    let now_set: HashSet<u64> = now.iter().copied().collect();
    let is_new = |id: &u64| !b.revs.contains_key(id);
    let created: Vec<u64> = now.iter().copied().filter(|id| is_new(id) && app.doc.parent_of(*id).is_none_or(|p| !is_new(&p))).collect();
    let deleted: Vec<u64> = b
        .doc
        .all_ids()
        .into_iter()
        .filter(|id| !now_set.contains(id) && b.doc.parent_of(*id).is_none_or(|p| now_set.contains(&p)))
        .collect();
    // An edit restamps its shape, the shape's layers and its ancestors: the
    // edited shape is the outermost one whose layers all changed.
    let touched: HashSet<u64> = now.iter().copied().filter(|id| b.revs.get(id).is_some_and(|r| app.doc.get(*id).is_some_and(|s| s.rev != *r))).collect();
    let all_touched = |id: u64| {
        touched.contains(&id) && app.doc.get(id).and_then(|s| s.kind.children()).is_none_or(|c| c.iter().all(|c| touched.contains(&c.id) || is_new(&c.id)))
    };
    let inside_new = |id: u64| {
        let mut p = app.doc.parent_of(id);
        while let Some(x) = p {
            if is_new(&x) {
                return true;
            }
            p = app.doc.parent_of(x);
        }
        false
    };
    let mut changed: Vec<u64> = now
        .iter()
        .copied()
        .filter(|id| all_touched(*id) && app.doc.parent_of(*id).is_none_or(|p| !all_touched(p)) && !inside_new(*id))
        .filter(|id| match (b.doc.get(*id), app.doc.get(*id)) {
            (Some(o), Some(n)) => unstamped(o) != unstamped(n),
            _ => false,
        })
        .collect();
    // Restacking moves shapes without restamping them.
    let mut parents: Vec<Option<u64>> = vec![None];
    parents.extend(now.iter().copied().filter(|id| !is_new(id) && app.doc.get(*id).is_some_and(|s| s.kind.children().is_some())).map(Some));
    let mut restacked = Vec::new();
    for p in parents {
        let keep = |v: Vec<u64>| -> Vec<u64> { v.into_iter().filter(|id| !is_new(id) && now_set.contains(id)).collect() };
        let (old, new) = (keep(order(&b.doc, p)), keep(order(&app.doc, p)));
        if old != new && old.len() == new.len() {
            restacked.push(json!({"parent": p, "top_first": order(&app.doc, p).into_iter().take(MAX).collect::<Vec<_>>()}));
        }
    }
    let mut out = serde_json::Map::new();
    let list = |app: &mut App, ids: &[u64], with_layers: bool| -> Vec<Value> {
        let shapes: Vec<Shape> = ids.iter().take(MAX).filter_map(|id| app.doc.get(*id).cloned()).collect();
        shapes
            .into_iter()
            .map(|s| {
                let mut v = brief(app, &s);
                if with_layers && s.kind.children().is_some() {
                    v["layers"] = json!(layers(&s, MAX));
                }
                v
            })
            .collect()
    };
    if !created.is_empty() {
        out.insert("created".into(), json!(list(app, &created, true)));
    }
    changed.truncate(MAX);
    if !changed.is_empty() {
        out.insert("changed".into(), json!(list(app, &changed, false)));
    }
    if !deleted.is_empty() {
        out.insert("deleted".into(), json!(deleted));
    }
    if !restacked.is_empty() {
        out.insert("restacked".into(), json!(restacked));
    }
    if out.is_empty() {
        out.insert("unchanged".into(), json!(true));
    }
    out.insert("selection".into(), json!(app.selection));
    out.insert("undo".into(), json!(app.history.can_undo()));
    Value::Object(out)
}

// ---------------------------------------------------------------------------
// Paints
// ---------------------------------------------------------------------------

/// `#RRGGBB[AA]`, `rgb(r, g, b)`, `rgba(r, g, b, a)` or `transparent`.
pub fn css_color(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("transparent") {
        return Some(Rgba { a: 0.0, ..Rgba::BLACK });
    }
    if let Some(body) = s.strip_prefix("rgba(").or(s.strip_prefix("rgb(")).and_then(|b| b.strip_suffix(')')) {
        let v: Vec<f32> = body.split([',', ' ', '/']).filter(|p| !p.is_empty()).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        let (r, g, b) = (*v.first()?, *v.get(1)?, *v.get(2)?);
        return Some(Rgba { r: r / 255.0, g: g / 255.0, b: b / 255.0, a: v.get(3).copied().unwrap_or(1.0) });
    }
    Rgba::from_hex(s)
}

/// Split on commas outside parentheses.
fn split_top(s: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0usize, Vec::new());
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

/// Stops with missing offsets spread evenly between their neighbours.
fn stops(list: Vec<(Rgba, Option<f32>)>) -> Result<Vec<GradientStop>, String> {
    if list.len() < 2 {
        return Err("a gradient needs at least 2 colours".into());
    }
    let n = list.len();
    let mut out: Vec<GradientStop> = list.iter().enumerate().map(|(i, (c, o))| GradientStop::new(o.unwrap_or(i as f32 / (n - 1) as f32).clamp(0.0, 1.0), *c)).collect();
    out.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    Ok(out)
}

/// Start and end in the box's 0..1 units for a CSS angle (0 = up, 90 = right).
fn angle_ends(deg: f64) -> ([f64; 2], [f64; 2]) {
    let (s, c) = deg.to_radians().sin_cos();
    let r = |v: f64| (v * 1e9).round() / 1e9;
    ([r(0.5 - s / 2.0), r(0.5 + c / 2.0)], [r(0.5 + s / 2.0), r(0.5 - c / 2.0)])
}

/// `linear-gradient(90deg, #f00, #00f 80%)` / `radial-gradient(#fff, #000)`.
fn css_gradient(s: &str) -> Result<Paint, String> {
    let bad = || format!("bad gradient {s:?}: e.g. linear-gradient(90deg, #ff0000, #0000ff) or radial-gradient(#ffffff, #000000)");
    let (kind, body) = if let Some(b) = s.strip_prefix("linear-gradient(") {
        (GradientKind::Linear, b)
    } else if let Some(b) = s.strip_prefix("radial-gradient(") {
        (GradientKind::Radial, b)
    } else {
        return Err(bad());
    };
    let parts = split_top(body.strip_suffix(')').ok_or_else(bad)?);
    let mut deg = 180.0;
    let mut list = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if i == 0 && kind == GradientKind::Linear {
            if let Some(d) = p.strip_suffix("deg").and_then(|d| d.trim().parse::<f64>().ok()) {
                deg = d;
                continue;
            }
            if let Some(dir) = p.strip_prefix("to ") {
                let (h, v) = (dir.contains("right") as i32 - dir.contains("left") as i32, dir.contains("bottom") as i32 - dir.contains("top") as i32);
                deg = (h as f64).atan2(-v as f64).to_degrees();
                continue;
            }
        }
        // Radial shape / position words are skipped (centred circle).
        if i == 0 && kind == GradientKind::Radial && css_color(p.split_whitespace().next().unwrap_or("")).is_none() {
            continue;
        }
        let (col, pos) = match p.rsplit_once(' ') {
            Some((c, o)) if o.ends_with('%') && css_color(c).is_some() => (c, o.trim_end_matches('%').parse::<f32>().ok().map(|v| v / 100.0)),
            _ => (*p, None),
        };
        list.push((css_color(col).ok_or_else(bad)?, pos));
    }
    let (start, end) = match kind {
        GradientKind::Linear => angle_ends(deg),
        _ => ([0.5, 0.5], [1.0, 0.5]),
    };
    Ok(Paint::Gradient(Gradient { kind, stops: stops(list)?, start, end, mesh: None }))
}

/// A fill from JSON: "#RRGGBB[AA]", null (none), a CSS linear-/radial-
/// gradient string, {gradient: linear | radial, stops: [colour | [colour,
/// offset]…], angle? | start? / end? (0..1 of the shape's box)}, or the
/// native get_shapes form ({"Gradient": {…}}, {"Solid": {…}}).
pub fn paint(v: &Value) -> Result<Option<Paint>, String> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.contains("gradient(") => css_gradient(s.trim()).map(Some),
        Value::String(s) => css_color(s).map(|c| Some(Paint::Solid(c))).ok_or(format!("bad colour {s} (use #RRGGBB, #RRGGBBAA or a CSS gradient)")),
        Value::Object(o) if o.contains_key("Gradient") || o.contains_key("Solid") || o.contains_key("Image") => {
            serde_json::from_value(v.clone()).map(Some).map_err(|e| format!("fill: {e}"))
        }
        Value::Object(o) => {
            let kind = match o.get("gradient").and_then(|k| k.as_str()).unwrap_or("linear") {
                "linear" => GradientKind::Linear,
                "radial" => GradientKind::Radial,
                "conic" => GradientKind::Conic,
                k => return Err(format!("gradient is linear, radial or conic, not {k}")),
            };
            let list = o.get("stops").and_then(|s| s.as_array()).ok_or("a gradient fill needs stops: [\"#RRGGBB\", …] or [[\"#RRGGBB\", offset 0..1], …]")?;
            let mut parsed = Vec::new();
            for s in list {
                let (c, off) = match s {
                    Value::Array(a) => (a.first().and_then(|c| c.as_str()), a.get(1).and_then(|o| o.as_f64())),
                    Value::Object(m) => (m.get("color").and_then(|c| c.as_str()), m.get("offset").and_then(|o| o.as_f64())),
                    _ => (s.as_str(), None),
                };
                let c = c.and_then(css_color).ok_or(format!("bad gradient stop {s}"))?;
                parsed.push((c, off.map(|o| o as f32)));
            }
            let pt = |k: &str| o.get(k).and_then(|p| p.as_array()).and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]));
            let (mut start, mut end) = match (kind, o.get("angle").and_then(|a| a.as_f64())) {
                (GradientKind::Linear, a) => angle_ends(a.unwrap_or(90.0)),
                _ => ([0.5, 0.5], [1.0, 0.5]),
            };
            start = pt("start").unwrap_or(start);
            end = pt("end").unwrap_or(end);
            Ok(Some(Paint::Gradient(Gradient { kind, stops: stops(parsed)?, start, end, mesh: None })))
        }
        _ => Err("a fill is a colour string, a gradient, or null".into()),
    }
}

// ---------------------------------------------------------------------------
// Stacking
// ---------------------------------------------------------------------------

/// The shape `below` / `above` names, checked before anything changes.
pub fn stack_anchor(app: &App, args: &Value) -> Result<Option<(u64, bool)>, String> {
    let (b, a) = (args.get("below").and_then(|v| v.as_u64()), args.get("above").and_then(|v| v.as_u64()));
    let anchor = match (b, a) {
        (Some(_), Some(_)) => return Err("give below or above, not both".into()),
        (Some(id), None) => (id, false),
        (None, Some(id)) => (id, true),
        (None, None) => return Ok(None),
    };
    existing(app, &[anchor.0])?;
    Ok(Some(anchor))
}

/// Put new shapes (bottom first) just below / above the anchor, in its group.
pub fn restack(app: &mut App, ids: &[u64], anchor: Option<(u64, bool)>) {
    let Some((at, above)) = anchor else { return };
    let ids: Vec<u64> = if above { ids.iter().rev().copied().collect() } else { ids.to_vec() };
    for id in ids {
        app.doc.move_beside(id, at, above);
    }
}

/// Existing shapes under the new ones and entirely inside them (so maybe
/// hidden by them), for a warning.
pub fn covered(app: &mut App, ids: &[u64]) -> Vec<u64> {
    let filled = |s: &Shape| s.fill.is_some() || s.kind.children().is_some();
    let new: Vec<Shape> = ids.iter().filter_map(|id| app.doc.get(*id).cloned()).filter(filled).collect();
    let Some(nb) = new.iter().filter_map(|s| bounds(app, s)).reduce(|a, b| a.union(b)) else { return Vec::new() };
    let Some(first) = ids.first() else { return Vec::new() };
    let parent = app.doc.parent_of(*first);
    let siblings: Vec<Shape> = order(&app.doc, parent).into_iter().rev().take_while(|id| !ids.contains(id)).filter_map(|id| app.doc.get(id).cloned()).collect();
    siblings
        .into_iter()
        .filter(|s| s.visible)
        .filter(|s| bounds(app, s).is_some_and(|b| b.area() > 0.0 && nb.contains(b.origin()) && nb.contains(kurbo::Point::new(b.x1, b.y1))))
        .map(|s| s.id)
        .take(12)
        .collect()
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Every layer of a main, with its path of names ("Card/Title").
fn main_layers(main: &Shape) -> Vec<(String, Shape)> {
    fn walk(list: &[Shape], prefix: &str, out: &mut Vec<(String, Shape)>) {
        for c in list {
            let path = if prefix.is_empty() { c.name.clone() } else { format!("{prefix}/{}", c.name) };
            out.push((path.clone(), c.clone()));
            if let Some(ch) = c.kind.children() {
                walk(ch, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(main.kind.children().map(|c| c.as_slice()).unwrap_or(&[]), "", &mut out);
    out
}

/// Keys an override (instance layer) may set.
pub const OVERRIDE_KEYS: &[&str] =
    &["text", "fill", "stroke", "stroke_width", "opacity", "visible", "name", "font_size", "font_family", "weight", "italic", "align", "line_height", "letter_spacing"];

/// Overrides keyed by layer name, name path ("Card/Title") or main layer id.
fn overrides(main: &Shape, spec: &Value) -> Result<BTreeMap<u64, Override>, String> {
    let mut out = BTreeMap::new();
    let Some(map) = spec.get("overrides") else { return Ok(out) };
    let map = map.as_object().ok_or("overrides is {\"<layer name>\": {text, fill, …}}")?;
    let layers = main_layers(main);
    for (key, o) in map {
        let layer = layers
            .iter()
            .find(|(path, s)| path == key || s.name == *key || key.parse::<u64>().ok() == Some(s.id))
            .map(|(_, s)| s)
            .ok_or_else(|| format!("{:?} has no layer {key:?} (layers: {})", main.name, layers.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(", ")))?;
        let obj = o.as_object().ok_or(format!("override {key:?} is an object like {{\"text\": \"…\", \"fill\": \"#RRGGBB\"}}"))?;
        if let Some(bad) = obj.keys().find(|k| !OVERRIDE_KEYS.contains(&k.as_str())) {
            return Err(format!("override {key:?}: no field `{bad}` (fields: {})", OVERRIDE_KEYS.join(", ")));
        }
        let mut s = layer.clone();
        super::tools::set_style(&mut s, o)?;
        super::tools::set_text(&mut s, o)?;
        if let Some(v) = o.get("visible").and_then(|v| v.as_bool()) {
            s.visible = v;
        }
        let mut r = Override::default();
        r.record(layer, &s);
        if !r.is_empty() {
            out.insert(layer.id, r);
        }
    }
    Ok(out)
}

fn main_of(app: &App, id: u64) -> Result<(Shape, Affine), String> {
    let m = app.doc.get(id).cloned().ok_or(format!("no shape with id {id}"))?;
    match &m.component {
        Some(Component::Main { frame, .. }) => {
            let f = *frame;
            Ok((m, f))
        }
        Some(Component::Instance(l)) => Err(format!("{id} is an instance of {}: pass the main", l.main)),
        None => Err(format!("{id} isn't a component (create_component makes one; list_components shows them)")),
    }
}

/// Instance layers with the main layer each mirrors.
fn instance_layers(s: &Shape) -> Vec<Value> {
    let inv: HashMap<u64, u64> = match &s.component {
        Some(Component::Instance(l)) => l.ids.iter().map(|(m, i)| (*i, *m)).collect(),
        _ => HashMap::new(),
    };
    let mut v = layers(s, 60);
    for l in &mut v {
        if let Some(m) = l["id"].as_u64().and_then(|i| inv.get(&i)) {
            l["main_layer"] = json!(m);
        }
    }
    v
}

/// New instances of a main: `at` (top-left), `overrides` by layer name,
/// `name`; one call, one undo step.
pub fn instantiate(app: &mut App, args: &Value) -> Result<Value, String> {
    let id = args.get("component").and_then(|v| v.as_u64()).ok_or("component (a main component's id) is required")?;
    let (main, frame) = main_of(app, id)?;
    let mb = bounds(app, &main).ok_or("the component is empty")?;
    let anchor = stack_anchor(app, args)?;
    let parent = parent_arg(app, args)?;
    let specs: Vec<Value> = match args.get("instances") {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        Some(_) => return Err("instances is a non-empty list of {at, overrides?, name?}".into()),
        None => vec![args.clone()],
    };
    // Build everything first: a bad override changes nothing.
    let mut built = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        if let Some(bad) = spec.as_object().and_then(|o| o.keys().find(|k| !["at", "overrides", "name", "component", "parent", "index", "below", "above", "instances", "select"].contains(&k.as_str()))) {
            return Err(format!("instances[{i}]: no field `{bad}` (at, overrides, name)"));
        }
        let at = point_arg(spec, "at").unwrap_or(kurbo::Point::new(mb.x1 + 40.0 + i as f64 * (mb.width() + 40.0), mb.y0));
        let mut link = InstanceLink { main: id, xf: Affine::translate(at - mb.origin()) * frame, ids: BTreeMap::new(), overrides: overrides(&main, spec).map_err(|e| if specs.len() > 1 { format!("instances[{i}]: {e}") } else { e })? };
        let children = crate::components::build_children(&main, &mut link, &mut app.doc);
        let mut g = Shape::new(ShapeKind::Group { children });
        g.fill = None;
        g.name = str_arg(spec, "name").map(String::from).unwrap_or_else(|| main.name.clone());
        g.component = Some(Component::Instance(link));
        built.push(g);
    }
    app.checkpoint("");
    let mut ids = Vec::new();
    for g in built {
        ids.push(app.doc.add_shape(g));
    }
    insert_into(app, &ids, parent, args.get("index").and_then(|v| v.as_u64()).map(|i| i as usize));
    restack(app, &ids, anchor);
    settle(app);
    if args.get("select").and_then(|v| v.as_bool()).unwrap_or(false) {
        app.selection = ids.clone();
    }
    let shapes: Vec<Shape> = ids.iter().filter_map(|i| app.doc.get(*i).cloned()).collect();
    let out: Vec<Value> = shapes
        .into_iter()
        .map(|s| {
            let mut v = brief(app, &s);
            v["layers"] = json!(instance_layers(&s));
            v
        })
        .collect();
    Ok(json!({"instances": out}))
}

/// Mains with their layers (for overrides) and instances.
pub fn list_components(app: &mut App) -> Value {
    let mains = app.mains();
    let shapes: Vec<Shape> = mains.into_iter().filter_map(|id| app.doc.get(id).cloned()).collect();
    let v: Vec<Value> = shapes
        .into_iter()
        .map(|m| {
            let mut v = brief(app, &m);
            v["layers"] = json!(main_layers(&m).into_iter().take(60).map(|(path, s)| json!({"id": s.id, "path": path, "type": s.kind.type_name()})).collect::<Vec<_>>());
            v["instances"] = json!(app.instances_of(m.id));
            if let Some(Component::Main { set: Some(set), props, .. }) = &m.component {
                v["set"] = json!(set);
                v["props"] = json!(props);
            }
            v
        })
        .collect();
    json!({"components": v})
}

/// Make `ids` a main component named `name` (one shape that isn't a group
/// is wrapped in one, named after it).
pub fn create_component(app: &mut App, args: &Value) -> Result<Value, String> {
    let ids = ids_arg(args);
    existing(app, &ids)?;
    if ids.is_empty() {
        return Err("ids is required".into());
    }
    if ids.len() == 1 && app.doc.get(ids[0]).is_some_and(|s| s.component.is_some()) {
        return Err(format!("{} is already a component", ids[0]));
    }
    let single = (ids.len() == 1).then(|| app.doc.get(ids[0]).map(|s| s.name.clone())).flatten();
    app.finish_text_edit();
    let keep = app.selection.clone();
    app.selection = ids;
    app.create_component();
    let id = app.selection.first().copied().ok_or("could not make a component")?;
    app.selection = keep;
    app.selection.retain(|i| app.doc.get(*i).is_some());
    if let Some(s) = app.doc.get_mut(id) {
        if let Some(n) = str_arg(args, "name") {
            s.name = n.into();
        } else if let Some(n) = single.filter(|_| s.name.starts_with("Component ")) {
            s.name = n;
        }
    }
    let s = app.doc.get(id).cloned().ok_or("could not make a component")?;
    let mut v = brief(app, &s);
    v["layers"] = json!(layers(&s, 60));
    Ok(v)
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Stack `ids` in a row / column with a gap, and / or centre (align) them
/// inside another shape. One undo step.
pub fn arrange(app: &mut App, args: &Value) -> Result<Value, String> {
    let ids = ids_arg(args);
    existing(app, &ids)?;
    if ids.is_empty() {
        return Err("ids is required".into());
    }
    if let Some((id, g)) = ids.iter().find_map(|id| app.layout_parent(*id).map(|g| (*id, g))) {
        return Err(format!("{id} is an item of auto layout {g}, which places it: change the layout with auto_layout (gap, padding, align, order)"));
    }
    let stack = str_arg(args, "stack");
    let within = args.get("within").and_then(|v| v.as_u64());
    if stack.is_none() && within.is_none() {
        return Err("give stack (row | column) and / or within (a shape id to align inside)".into());
    }
    if let Some(w) = within {
        existing(app, &[w])?;
        if ids.contains(&w) {
            return Err("within can't be one of ids".into());
        }
    }
    let align = str_arg(args, "align").unwrap_or(if stack.is_some() { "start" } else { "center" });
    let frac = match align {
        "start" | "left" | "top" => 0.0,
        "center" | "middle" => 0.5,
        "end" | "right" | "bottom" => 1.0,
        a => return Err(format!("align is start, center or end, not {a}")),
    };
    let gap = num_arg(args, "gap").unwrap_or(0.0);
    let boxes: Vec<kurbo::Rect> = ids.iter().map(|id| ids_bounds(app, &[*id]).ok_or(format!("{id} has no bounds"))).collect::<Result<_, _>>()?;
    let row = match stack {
        None => None,
        Some("row" | "horizontal" | "h") => Some(true),
        Some("column" | "vertical" | "v") => Some(false),
        Some(d) => return Err(format!("stack is row or column, not {d}")),
    };
    let axis = str_arg(args, "axis").unwrap_or("both");
    if !["both", "x", "y"].contains(&axis) {
        return Err("axis is both, x or y".into());
    }
    app.checkpoint("");
    if let Some(row) = row {
        let start = point_arg(args, "at").unwrap_or(boxes[0].origin());
        let cross = boxes.iter().map(|b| if row { b.height() } else { b.width() }).fold(0.0, f64::max);
        let mut pos = if row { start.x } else { start.y };
        for (id, b) in ids.iter().zip(&boxes) {
            let (main, other) = if row { (b.width(), b.height()) } else { (b.height(), b.width()) };
            let off = (cross - other) * frac;
            let to = if row { kurbo::Point::new(pos, start.y + off) } else { kurbo::Point::new(start.x + off, pos) };
            app.translate_ids(&[*id], to - b.origin());
            pos += main + gap;
        }
    }
    if let Some(w) = within {
        let wb = ids_bounds(app, &[w]).ok_or(format!("{w} has no bounds"))?;
        let gb = ids_bounds(app, &ids).ok_or("nothing to align")?;
        let pad = num_arg(args, "padding").unwrap_or(0.0);
        let inner = wb.inset(-pad);
        let target = |lo: f64, hi: f64, size: f64| lo + (hi - lo - size) * frac;
        let dx = if axis != "y" { target(inner.x0, inner.x1, gb.width()) - gb.x0 } else { 0.0 };
        let dy = if axis != "x" { target(inner.y0, inner.y1, gb.height()) - gb.y0 } else { 0.0 };
        app.translate_ids(&ids, kurbo::Vec2::new(dx, dy));
    }
    settle(app);
    let out: Vec<Value> = ids.iter().map(|id| json!({"id": id, "bounds": ids_bounds(app, &[*id]).map(rect_json)})).collect();
    Ok(json!(out))
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

/// An Iconify icon ("mdi:home", "twemoji:frog") as SVG, `size` px tall;
/// `color` tints monochrome icons.
pub fn icon_svg(name: &str, size: f64, color: Option<&str>) -> Result<String, String> {
    let (set, icon) = name.split_once(':').ok_or("name is set:icon, e.g. mdi:home or twemoji:frog (browse icon-sets.iconify.design)")?;
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok(set) || !ok(icon) {
        return Err(format!("bad icon name {name}"));
    }
    let mut url = format!("https://api.iconify.design/{set}/{icon}.svg?height={}", size.round().max(1.0));
    if let Some(c) = color {
        let c = css_color(c).ok_or(format!("bad colour {c}"))?;
        url.push_str(&format!("&color=%23{}", c.to_hex().trim_start_matches('#')));
    }
    let body = crate::fonts::http_get(&url).map_err(|e| format!("Iconify: {e}"))?;
    let svg = String::from_utf8_lossy(&body).to_string();
    if !svg.contains("<svg") {
        return Err(format!("no icon {name} on Iconify (browse icon-sets.iconify.design)"));
    }
    Ok(svg)
}

// ---------------------------------------------------------------------------
// Auto layout
// ---------------------------------------------------------------------------

/// Let instances and auto layouts catch up with an edit (the window does
/// this every frame; answers should show the result).
pub fn settle(app: &mut App) {
    for _ in 0..4 {
        let rev = app.doc.revision();
        app.components_tick();
        app.layout_key = None;
        app.layout_tick();
        if app.doc.revision() == rev {
            break;
        }
    }
}

pub fn layout_json(l: &crate::layout::AutoLayout) -> Value {
    use crate::layout::{Align, Direction};
    json!({
        "direction": if l.dir == Direction::Row { "row" } else { "column" },
        "gap": l.gap,
        "padding": l.padding,
        "align": match l.align { Align::Start => "start", Align::Center => "center", Align::End => "end" },
        "background": l.background,
    })
}

/// Padding: n | [vertical, horizontal] | [top, right, bottom, left].
fn padding(v: &Value) -> Result<[f64; 4], String> {
    let bad = || "padding is a number, [vertical, horizontal] or [top, right, bottom, left]".to_string();
    if let Some(n) = v.as_f64() {
        return Ok([n; 4]);
    }
    let a: Vec<f64> = v.as_array().ok_or_else(bad)?.iter().map(|x| x.as_f64()).collect::<Option<_>>().ok_or_else(bad)?;
    match a.as_slice() {
        [y, x] => Ok([*y, *x, *y, *x]),
        [t, r, b, l] => Ok([*t, *r, *b, *l]),
        _ => Err(bad()),
    }
}

/// A resize constraint: start | end | both | center | scale (left / top,
/// right / bottom, stretch also read).
pub fn pin(v: &str) -> Result<crate::layout::Pin, String> {
    use crate::layout::Pin;
    Ok(match v {
        "start" | "left" | "top" => Pin::Start,
        "end" | "right" | "bottom" => Pin::End,
        "both" | "stretch" | "left_right" | "top_bottom" => Pin::Both,
        "center" => Pin::Center,
        "scale" => Pin::Scale,
        _ => return Err(format!("constraint {v}: start, end, both, center or scale")),
    })
}

/// Put new top-level shapes into `parent` (at flow position `index`: after
/// an auto layout's background; default: last / on top).
pub fn insert_into(app: &mut App, ids: &[u64], parent: Option<u64>, index: Option<usize>) {
    let Some(p) = parent else { return };
    let bg = app.doc.get(p).and_then(|s| s.layout.as_ref()).and_then(|l| l.background).is_some() as usize;
    for (k, id) in ids.iter().enumerate() {
        let n = app.doc.get(p).and_then(|s| s.kind.children()).map_or(0, |c| c.len());
        let at = index.map(|i| (i + bg + k).min(n)).unwrap_or(n);
        app.doc.move_into(*id, Some(p), at);
    }
    app.layout_placed.remove(&p);
}

/// The parent a tool's `parent` names, checked up front.
pub fn parent_arg(app: &App, args: &Value) -> Result<Option<u64>, String> {
    let Some(p) = args.get("parent").and_then(|v| v.as_u64()) else { return Ok(None) };
    app.doc.get(p).and_then(|g| g.kind.children()).ok_or(format!("{p} is not a group"))?;
    Ok(Some(p))
}

/// Create, change or remove an auto layout. One undo step.
pub fn auto_layout(app: &mut App, args: &Value) -> Result<Value, String> {
    use crate::layout::{Align, Direction};
    let id = args.get("id").and_then(|v| v.as_u64());
    let ids = ids_arg(args);
    if id.is_some() == !ids.is_empty() {
        return Err("give id (a group or auto layout to set up or change) or ids (shapes to wrap in a new one)".into());
    }
    existing(app, &ids)?;
    if let Some(i) = id {
        existing(app, &[i])?;
    }
    let is_layout = id.is_some_and(|i| app.doc.get(i).is_some_and(|s| s.layout.is_some()));
    // Validate everything before changing anything.
    let dir = match str_arg(args, "direction") {
        None => None,
        Some("row" | "horizontal") => Some(Direction::Row),
        Some("column" | "vertical") => Some(Direction::Column),
        Some(d) => return Err(format!("direction is row or column, not {d}")),
    };
    let align = match str_arg(args, "align") {
        None => None,
        Some("start" | "top" | "left") => Some(Align::Start),
        Some("center" | "middle") => Some(Align::Center),
        Some("end" | "bottom" | "right") => Some(Align::End),
        Some(a) => return Err(format!("align is start, center or end, not {a}")),
    };
    let pad = args.get("padding").map(padding).transpose()?;
    let fill = args.get("fill").map(paint).transpose()?;
    let stroke = args.get("stroke").map(super::tools::color).transpose()?;
    let order: Option<Vec<u64>> = args.get("order").map(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).ok_or("order is a list of member ids")).transpose()?;
    if args.get("remove").and_then(|v| v.as_bool()) == Some(true) {
        let i = id.filter(|_| is_layout).ok_or("remove needs the id of an auto layout")?;
        app.checkpoint("");
        if let Some(mut g) = app.doc.get(i).cloned() {
            g.layout = None;
            app.doc.replace(g);
        }
        app.layout_placed.remove(&i);
        return Ok(json!({"id": i, "layout": null, "note": "the group and its background rectangle stay; delete_shapes the background if unwanted"}));
    }
    let gid = if is_layout {
        app.checkpoint("");
        id.unwrap()
    } else {
        // AddAutoLayout makes the undo step (and the group, unless it is one).
        let keep = app.selection.clone();
        app.finish_text_edit();
        app.selection = id.map(|i| vec![i]).unwrap_or(ids.clone());
        app.add_auto_layout();
        let g = app.selection.first().copied().filter(|g| app.doc.get(*g).is_some_and(|s| s.layout.is_some()));
        app.selection = keep;
        app.selection.retain(|i| app.doc.get(*i).is_some());
        g.ok_or("could not make an auto layout")?
    };
    let mut g = app.doc.get(gid).cloned().ok_or("the auto layout vanished")?;
    let mut l = g.layout.clone().unwrap_or_default();
    // A new one is a row unless told otherwise (predictable, unlike the
    // menu command's guess from where the shapes sit).
    if let Some(d) = dir.or((!is_layout).then_some(Direction::Row)) {
        l.dir = d;
    }
    if let Some(a) = align {
        l.align = a;
    }
    if let Some(n) = num_arg(args, "gap") {
        l.gap = n;
    }
    if let Some(p) = pad {
        l.padding = p;
    }
    if let Some(n) = str_arg(args, "name") {
        g.name = n.into();
    }
    // The background rectangle: created when styled and missing.
    let wants_bg = fill.is_some() || stroke.is_some() || args.get("radius").is_some();
    let has_bg = l.background.is_some_and(|b| g.kind.children().is_some_and(|c| c.iter().any(|c| c.id == b)));
    if wants_bg && !has_bg {
        let mut bg = Shape::new(ShapeKind::Rect { w: 1.0, h: 1.0, radii: [0.0; 4] });
        bg.fill = None;
        bg.stroke = None;
        bg.name = "Background".into();
        bg.id = app.doc.fresh_id();
        l.background = Some(bg.id);
        if let Some(c) = g.kind.children_mut() {
            c.insert(0, bg);
        }
    }
    if let Some(c) = g.kind.children_mut() {
        if let Some(bg) = l.background.and_then(|b| c.iter_mut().find(|s| s.id == b)) {
            if let Some(f) = fill {
                bg.fill = f;
                bg.fill_key = None;
            }
            if let Some(s) = stroke {
                match s {
                    Some(col) => bg.stroke.get_or_insert_with(crate::document::Stroke::default).color = col,
                    None => bg.stroke = None,
                }
            }
            if let Some(w) = num_arg(args, "stroke_width") {
                bg.stroke.get_or_insert_with(crate::document::Stroke::default).width = w;
            }
            if let (Some(r), ShapeKind::Rect { radii, .. }) = (num_arg(args, "radius"), &mut bg.kind) {
                *radii = [r; 4];
            }
        }
        let members: Vec<u64> = c.iter().map(|s| s.id).filter(|i| Some(*i) != l.background).collect();
        // A new layout flows in the order ids were given.
        let order = order.clone().or_else(|| (!ids.is_empty() && ids.iter().all(|i| members.contains(i))).then(|| ids.clone()));
        if let Some(order) = &order {
            if let Some(bad) = order.iter().find(|i| !members.contains(i)) {
                app.history.cancel(&mut app.doc);
                return Err(format!("{bad} isn't a member of {gid} (members: {members:?})"));
            }
            let rank = |i: u64| if Some(i) == l.background { 0 } else { order.iter().position(|x| *x == i).map(|p| p + 1).unwrap_or(usize::MAX) };
            c.sort_by_key(|s| rank(s.id));
        }
    }
    g.layout = Some(l);
    app.doc.replace(g);
    app.layout_placed.remove(&gid);
    app.layout_key = None;
    app.relayout(gid);
    settle(app);
    let g = app.doc.get(gid).cloned().ok_or("the auto layout vanished")?;
    let mut v = brief(app, &g);
    v["layout"] = layout_json(g.layout.as_ref().unwrap());
    let bg = g.layout.as_ref().and_then(|l| l.background);
    let members: Vec<Shape> = g.kind.children().cloned().unwrap_or_default().into_iter().filter(|c| Some(c.id) != bg).collect();
    v["members"] = json!(members.iter().map(|m| brief(app, m)).collect::<Vec<_>>());
    Ok(v)
}
