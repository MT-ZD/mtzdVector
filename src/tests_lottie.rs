//! Lottie import: evaluation of layers, keyframes and shape items, and the
//! keys made from the frames.

use kurbo::{Point, Rect};
use serde_json::json;

use crate::animation::{self, Prop};
use crate::document::{Document, Paint, Shape, ShapeKind};
use crate::io::lottie;

fn import(j: serde_json::Value) -> Document {
    lottie::import(j.to_string().as_bytes(), None).unwrap()
}

fn bounds(s: &Shape) -> Rect {
    let mut v = Vec::new();
    animation::points(s, &mut v);
    v.iter().fold(Rect::from_points(v[0], v[0]), |r, p| r.union_pt(*p))
}

/// The first leaf under the top-level shape `i`.
fn leaf(d: &Document, i: usize) -> Shape {
    let mut s = (*d.shapes()[i]).clone();
    while let Some(c) = s.kind.children().and_then(|c| c.first()).cloned() {
        s = c;
    }
    s
}

fn at(d: &Document, t: f64) -> Document {
    animation::frame(d, t)
}

fn sq(t: f64, s: serde_json::Value) -> serde_json::Value {
    json!({"t": t, "s": s, "i": {"x": [1], "y": [1]}, "o": {"x": [0], "y": [0]}})
}

fn rect_layer(ind: i64, ks: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let mut l = json!({
        "ty": 4, "ind": ind, "nm": format!("L{ind}"), "ip": 0, "op": 60, "st": 0, "ks": ks,
        "shapes": [{"ty": "gr", "it": [
            {"ty": "rc", "p": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [20, 10]}, "r": {"a": 0, "k": 0}},
            {"ty": "fl", "c": {"a": 0, "k": [1, 0, 0, 1]}, "o": {"a": 0, "k": 100}},
            {"ty": "tr", "p": {"a": 0, "k": [0, 0]}, "a": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}, "r": {"a": 0, "k": 0}, "o": {"a": 0, "k": 100}}
        ]}]
    });
    for (k, v) in extra.as_object().unwrap() {
        l[k] = v.clone();
    }
    l
}

#[test]
fn moving_layer_with_linear_keys() {
    let d = import(json!({"v": "5.7", "fr": 30, "ip": 0, "op": 30, "w": 200, "h": 100, "layers": [
        rect_layer(1, json!({"p": {"a": 1, "k": [sq(0.0, json!([50, 50])), sq(30.0, json!([150, 50]))]}}), json!({}))
    ]}));
    assert_eq!(d.primary().w, 200.0);
    let tl = d.animation.clone().unwrap();
    assert_eq!((tl.duration, tl.fps), (1.0, 30.0));
    let s = leaf(&d, 0);
    assert!(matches!(s.fill, Some(Paint::Solid(c)) if c.r == 1.0 && c.g == 0.0));
    assert_eq!(bounds(&s), Rect::new(40.0, 45.0, 60.0, 55.0));
    // Linear motion: two keys are enough.
    let tr = s.anim.as_deref().unwrap().track(Prop::Transform).unwrap();
    assert_eq!(tr.keys.len(), 2, "{:?}", tr.keys.iter().map(|k| k.t).collect::<Vec<_>>());
    let half = bounds(&leaf(&at(&d, 0.5), 0));
    assert!((half.center().x - 100.0).abs() < 1e-6, "{half:?}");
}

#[test]
fn easing_parents_and_rotation() {
    // An eased move on a null parent, a spin on the child.
    let d = import(json!({"fr": 10, "ip": 0, "op": 10, "w": 100, "h": 100, "layers": [
        rect_layer(2, json!({"p": {"a": 0, "k": [0, 0]}, "r": {"a": 1, "k": [sq(0.0, json!([0])), sq(10.0, json!([90]))]}}), json!({"parent": 1})),
        {"ty": 3, "ind": 1, "ip": 0, "op": 10, "st": 0, "ks": {"p": {"a": 1, "k": [
            {"t": 0, "s": [10, 10], "o": {"x": [0.42], "y": [0]}, "i": {"x": [0.58], "y": [1]}},
            {"t": 10, "s": [90, 10]}
        ]}}}
    ]}));
    let c = |t: f64| bounds(&leaf(&at(&d, t), 0)).center();
    assert!((c(0.0) - Point::new(10.0, 10.0)).hypot() < 1e-6);
    assert!((c(1.0) - Point::new(90.0, 10.0)).hypot() < 1e-6);
    // Ease in and out: slower than linear at a fifth of the way.
    assert!(c(0.2).x < 10.0 + 80.0 * 0.2 - 3.0, "{:?}", c(0.2));
    // Turned a quarter at the end: 10 wide, 20 tall.
    let b = bounds(&leaf(&at(&d, 1.0), 0));
    assert!((b.width() - 10.0).abs() < 1e-3 && (b.height() - 20.0).abs() < 1e-3, "{b:?}");
}

#[test]
fn in_out_points_hold_and_scale_from_zero() {
    let d = import(json!({"fr": 10, "ip": 0, "op": 20, "w": 100, "h": 100, "layers": [
        rect_layer(1, json!({
            "p": {"a": 0, "k": [50, 50]},
            "s": {"a": 1, "k": [sq(0.0, json!([0, 0])), sq(10.0, json!([100, 100]))]},
            "o": {"a": 1, "k": [{"t": 0, "s": [100], "h": 1}, {"t": 15, "s": [40]}]}
        }), json!({"ip": 5, "op": 20}))
    ]}));
    let g = |t: f64| (*at(&d, t).shapes()[0]).clone();
    assert!(!g(0.0).visible && g(0.5).visible);
    assert_eq!(g(1.4).opacity, 1.0, "held");
    assert!((g(1.5).opacity - 0.4).abs() < 1e-6);
    // Grows out of nothing: the outline comes back from a point.
    let b = bounds(&leaf(&at(&d, 0.5), 0));
    assert!((b.width() - 10.0).abs() < 1e-6, "{b:?}");
    let b = bounds(&leaf(&at(&d, 1.0), 0));
    assert!((b.width() - 20.0).abs() < 1e-6 && (b.center().x - 50.0).abs() < 1e-6, "{b:?}");
    assert!(bounds(&leaf(&at(&d, 0.0), 0)).width() < 1e-9);
}

#[test]
fn morph_trim_gradient_and_precomp() {
    let path = |x: f64| json!([{"i": [[0, 0], [0, 0], [0, 0]], "o": [[0, 0], [0, 0], [0, 0]], "v": [[0, 0], [x, 0], [x, 50]], "c": false}]);
    let shapes = json!([{"ty": "gr", "it": [
        {"ty": "sh", "ks": {"a": 1, "k": [sq(0.0, path(50.0)), sq(10.0, path(100.0))]}},
        {"ty": "st", "c": {"a": 0, "k": [0, 0, 1, 1]}, "o": {"a": 0, "k": 100}, "w": {"a": 0, "k": 4}, "lc": 2, "lj": 2},
        {"ty": "tm", "s": {"a": 0, "k": 0}, "e": {"a": 1, "k": [sq(0.0, json!([0])), sq(10.0, json!([100]))]}, "o": {"a": 0, "k": 0}, "m": 1},
        {"ty": "tr", "p": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}}
    ]}, {"ty": "gr", "it": [
        {"ty": "el", "p": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [40, 40]}},
        {"ty": "gf", "t": 1, "s": {"a": 0, "k": [-20, 0]}, "e": {"a": 0, "k": [20, 0]}, "o": {"a": 0, "k": 100},
         "g": {"p": 2, "k": {"a": 0, "k": [0, 1, 0, 0, 1, 0, 0, 1]}}},
        {"ty": "tr", "p": {"a": 0, "k": [0, 0]}}
    ]}]);
    let d = import(json!({"fr": 10, "ip": 0, "op": 10, "w": 200, "h": 200,
        "assets": [{"id": "c1", "layers": [{"ty": 4, "ind": 1, "ip": 0, "op": 100, "st": 0, "ks": {}, "shapes": shapes}]}],
        "layers": [{"ty": 0, "ind": 1, "refId": "c1", "ip": 0, "op": 10, "st": 0, "ks": {"p": {"a": 0, "k": [100, 100]}}}]
    }));
    let precomp = (*d.shapes()[0]).clone();
    let layer = &precomp.kind.children().unwrap()[0];
    let kids = layer.kind.children().unwrap();
    assert_eq!(kids.len(), 2);
    // The ellipse (bottom: listed last) with a red to blue gradient across it.
    let Some(Paint::Gradient(g)) = &kids[0].fill else { panic!("{:?}", kids[0].fill) };
    assert_eq!(g.stops.len(), 2);
    assert!((g.start[0] - 0.0).abs() < 1e-6 && (g.end[0] - 1.0).abs() < 1e-6, "{g:?}");
    assert_eq!(bounds(&kids[0]).center(), Point::new(100.0, 100.0));
    // The stroke draws on: half of the growing path at 0.5 s.
    let len = |d: &Document| {
        let p = &d.shapes()[0].kind.children().unwrap()[0].kind.children().unwrap()[1];
        let ShapeKind::Path(pd) = &p.kind else { panic!() };
        kurbo::ParamCurveArclen::arclen(&crate::geom::pathdata_to_bez(pd).segments().next().map(|s| s).unwrap(), 1e-6)
            + crate::geom::pathdata_to_bez(pd).segments().skip(1).map(|s| kurbo::ParamCurveArclen::arclen(&s, 1e-6)).sum::<f64>()
    };
    let half = at(&d, 0.5);
    // Path 75 + 50 long at 0.5 s, half of it shown.
    assert!((len(&half) - 62.5).abs() < 0.5, "{}", len(&half));
    let st = d.shapes()[0].kind.children().unwrap()[0].kind.children().unwrap()[1].stroke.clone().unwrap();
    assert_eq!(st.width, 4.0);
}

#[test]
fn our_lottie_export_comes_back() {
    use crate::io::export::{Format, Options, export};
    let (d, _) = crate::tests_anim::moving_doc_for_tests();
    let mut f = crate::fonts::FontManager::new(egui::Context::default());
    let p = std::env::temp_dir().join("mtzd-roundtrip.json");
    export(&d, &[], d.primary().id, &Options { format: Format::Lottie, ..Default::default() }, &p, &mut f).unwrap();
    let back = crate::io::open(&p, &mut f).unwrap();
    assert_eq!(back.animation.as_ref().unwrap().fps, 10.0);
    // The square: the top layer (the white background is under it).
    let top = back.shapes().len() - 1;
    for t in [0.0, 0.5, 1.0] {
        let a = bounds(&leaf(&at(&d, t), 0));
        let b = bounds(&leaf(&at(&back, t), top));
        assert!((a.center() - b.center()).hypot() < 1e-3, "{t}: {a:?} vs {b:?}");
        let fill = |s: Shape| match s.fill {
            Some(Paint::Solid(c)) => c,
            _ => panic!(),
        };
        let (x, y) = (fill(leaf(&at(&d, t), 0)), fill(leaf(&at(&back, t), top)));
        assert!((x.r - y.r).abs() < 0.01 && (x.b - y.b).abs() < 0.01, "{t}: {x:?} vs {y:?}");
    }
}

/// `cargo test lottie_samples -- --ignored`: imports every .json in
/// /tmp/lottie-samples and saves /tmp/lottie-samples/<name>.mvec.
#[test]
#[ignore]
fn lottie_samples() {
    let mut f = crate::fonts::FontManager::new(egui::Context::default());
    let mut paths: Vec<_> = std::fs::read_dir(std::env::var("LOTTIE_DIR").unwrap_or("/tmp/lottie-samples".into())).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json" || e == "lottie")).collect();
    paths.sort();
    for p in paths {
        let t = std::time::Instant::now();
        match crate::io::open(&p, &mut f) {
            Ok(d) => {
                let keys: usize = d.all_ids().iter().filter_map(|id| d.get(*id)?.anim.as_deref().map(|a| a.tracks.iter().map(|t| t.keys.len()).sum::<usize>())).sum();
                println!("{}: {} ids, {} keys, {:?}, {:.2?}", p.display(), d.all_ids().len(), keys, d.animation.as_ref().map(|a| (a.duration, a.fps)), t.elapsed());
                crate::io::native::save(&d, &p.with_extension("mvec"), None, crate::io::native::SaveOptions { backup: false, preview: false }).unwrap();
                // Frames at 0, 1/3 and 2/3 of the way, for comparing with lottie-web.
                let dur = d.animation.as_ref().map_or(0.0, |a| a.duration);
                for (k, u) in [0.0, 1.0 / 3.0, 2.0 / 3.0].into_iter().enumerate() {
                    let fr = animation::frame(&d, d.animation.as_ref().map_or(0.0, |a| a.snap(dur * u)));
                    let out = p.with_file_name(format!("{}-ours-{k}.png", p.file_stem().unwrap().to_string_lossy()));
                    let opts = crate::io::export::Options { format: crate::io::export::Format::Png, ..Default::default() };
                    crate::io::export::export(&fr, &[], fr.primary().id, &opts, &out, &mut f).unwrap();
                }
            }
            Err(e) => println!("{}: {e}", p.display()),
        }
    }
}

#[test]
#[ignore]
fn lottie_debug_clip() {
    let d = lottie::import(&std::fs::read("/tmp/lottie-samples/adrock.json").unwrap(), None).unwrap();
    for s in d.shapes() {
        if s.clip.is_some() || s.name.contains("Solid") {
            println!("{} clip={:?} kids={}", s.name, s.clip.as_ref().map(|c| crate::geom::path_bounds(&crate::geom::pathdata_to_bez(match &c.kind { ShapeKind::Path(p) => p, _ => unreachable!() }))), s.kind.children().map_or(0, |c| c.len()));
        }
    }
}

/// Gradients, dashes, blend modes, shadows, blur and clips survive a trip
/// through Lottie (export, then import).
#[test]
fn styles_round_trip() {
    use crate::document::{BlendMode, Gradient, GradientKind, Rgba, Shadow, Stroke};
    use crate::io::export::{Format, Options, export};
    let mut d = Document::default();
    d.set_size(300.0, 200.0);
    let mut s = Shape::new(ShapeKind::Rect { w: 100.0, h: 60.0, radii: [0.0; 4] });
    s.transform = kurbo::Affine::translate((50.0, 50.0));
    let (red, blue) = (Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }, Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 });
    s.fill = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, red, blue)));
    s.stroke = Some(Stroke { width: 4.0, dash: vec![2.0, 1.0], ..Stroke::default() });
    s.blend = BlendMode::Multiply;
    s.shadow = Some(Shadow { color: Rgba::BLACK.with_alpha(0.5), dx: 6.0, dy: 8.0, blur: 4.0 });
    s.blur = 3.0;
    let mut clip = Shape::new(ShapeKind::Ellipse { w: 80.0, h: 80.0 });
    clip.transform = kurbo::Affine::translate((60.0, 40.0));
    let clip = {
        let mut c = clip;
        let p = crate::render::compute_outline(&c, &mut crate::fonts::FontManager::new(egui::Context::default())).0;
        c.kind = ShapeKind::Path(crate::geom::bez_to_pathdata(&p));
        c.transform = kurbo::Affine::IDENTITY;
        c
    };
    s.clip = Some(Box::new(clip));
    let id = d.add_shape(s);
    d.animation = Some(animation::Timeline { duration: 1.0, fps: 10.0, ..Default::default() });
    animation::seek(&mut d, 1.0);
    let base = d.clone();
    let mut m = d.get(id).unwrap().clone();
    m.stroke.as_mut().unwrap().dash_offset = 3.0;
    m.apply_transform(kurbo::Affine::translate((100.0, 0.0)));
    d.replace(m);
    animation::capture_edits(&base, &mut d, true, &[id]);
    animation::seek(&mut d, 0.0);

    let mut f = crate::fonts::FontManager::new(egui::Context::default());
    let p = std::env::temp_dir().join("mtzd-styles.json");
    export(&d, &[], d.primary().id, &Options { format: Format::Lottie, ..Default::default() }, &p, &mut f).unwrap();
    let j: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    let layer = &j["layers"][0];
    assert_eq!(layer["bm"], 1);
    assert_eq!(layer["ef"][0]["ty"], 25);
    assert_eq!(layer["ef"][1]["ty"], 29);
    assert_eq!(layer["hasMask"], true);
    assert!(layer["shapes"][0]["it"].as_array().unwrap().iter().any(|i| i["ty"] == "gf"));

    let back = crate::io::open(&p, &mut f).unwrap();
    let top = back.shapes().len() - 1;
    let g = (*back.shapes()[top]).clone();
    assert_eq!(g.blend, BlendMode::Multiply);
    let sh = g.shadow.clone().unwrap();
    assert!((sh.dx - 6.0).abs() < 0.01 && (sh.dy - 8.0).abs() < 0.01 && (sh.blur - 4.0).abs() < 0.01 && (sh.color.a - 0.5).abs() < 0.01, "{sh:?}");
    assert!((g.blur - 3.0).abs() < 0.01, "{}", g.blur);
    let l = leaf(&back, top);
    assert!(l.clip.is_some());
    let Some(Paint::Gradient(gr)) = &l.fill else { panic!("{:?}", l.fill) };
    assert!((gr.start[0]).abs() < 0.01 && (gr.end[0] - 1.0).abs() < 0.01 && gr.stops[0].color == red, "{gr:?}");
    let st = |d: &Document| leaf(d, top).stroke.unwrap();
    assert_eq!(st(&back).dash, vec![2.0, 1.0]);
    let end = at(&back, 1.0);
    assert!((st(&end).dash_offset - 3.0).abs() < 0.01, "{}", st(&end).dash_offset);
    assert!((bounds(&leaf(&end, top)).x0 - 150.0).abs() < 0.01, "{:?}", bounds(&leaf(&end, top)));
}

fn shape_layer(ind: i64, ks: serde_json::Value, items: serde_json::Value) -> serde_json::Value {
    json!({"ty": 4, "ind": ind, "nm": format!("L{ind}"), "ip": 0, "op": 60, "st": 0, "ks": ks, "shapes": items})
}

fn fill() -> serde_json::Value {
    json!({"ty": "fl", "c": {"a": 0, "k": [0, 0, 1, 1]}, "o": {"a": 0, "k": 100}})
}

fn rc(x: f64, y: f64, w: f64, h: f64) -> serde_json::Value {
    json!({"ty": "rc", "p": {"a": 0, "k": [x, y]}, "s": {"a": 0, "k": [w, h]}, "r": {"a": 0, "k": 0}})
}

fn leaves(s: &Shape, out: &mut Vec<Shape>) {
    match s.kind.children() {
        Some(c) => c.iter().for_each(|c| leaves(c, out)),
        None => out.push(s.clone()),
    }
}

fn all_leaves(d: &Document) -> Vec<Shape> {
    let mut out = Vec::new();
    for s in d.shapes() {
        leaves(s, &mut out);
    }
    out
}

fn area(s: &Shape) -> f64 {
    let ShapeKind::Path(pd) = &s.kind else { panic!("{:?}", s.kind.type_name()) };
    kurbo::Shape::area(&crate::geom::pathdata_to_bez(pd)).abs()
}

#[test]
fn loop_and_wiggle_expressions() {
    // Keys from 0 to 10 (x 0 → 100), looped: frame 15 is frame 5 again; a
    // ping-pong comes back; an offset loop keeps going.
    let keys = json!([sq(0.0, json!([0, 0])), sq(10.0, json!([100, 0]))]);
    let layer = |ind: i64, x: &str| rect_layer(ind, json!({"p": {"a": 1, "k": keys, "x": x}}), json!({}));
    let d = import(json!({"fr": 10, "ip": 0, "op": 40, "w": 100, "h": 100, "layers": [
        layer(1, "var $bm_rt;\n$bm_rt = loopOut('cycle');"),
        layer(2, "$bm_rt = loopOut(\"pingpong\");"),
        layer(3, "loopOut('offset')"),
        rect_layer(4, json!({"p": {"a": 0, "k": [50, 50], "x": "wiggle(2, 10)"}}), json!({})),
    ]}));
    let x = |t: f64, i: usize| bounds(&leaf(&at(&d, t), i)).center().x;
    // Layers come bottom first: the last in the file is index 0.
    assert!((x(1.5, 3) - 50.0).abs() < 1e-6, "{}", x(1.5, 3));
    assert!((x(1.5, 2) - 50.0).abs() < 1e-6 && (x(1.2, 2) - 80.0).abs() < 1e-6, "{} {}", x(1.5, 2), x(1.2, 2));
    assert!((x(1.5, 1) - 150.0).abs() < 1e-6, "{}", x(1.5, 1));
    // Wiggle: moves, but within its amplitude.
    let w: Vec<f64> = (0..40).map(|f| x(f as f64 / 10.0, 0)).collect();
    assert!(w.iter().any(|v| (v - 50.0).abs() > 0.5) && w.iter().all(|v| (v - 50.0).abs() <= 10.0 + 1e-6), "{w:?}");
}

#[test]
fn repeater_round_corners_and_merge() {
    let d = import(json!({"fr": 10, "ip": 0, "op": 10, "w": 300, "h": 100, "layers": [
        // Three copies, 50 apart, fading 100 → 0.
        shape_layer(1, json!({}), json!([
            rc(10.0, 10.0, 10.0, 10.0), fill(),
            {"ty": "rp", "c": {"a": 0, "k": 3}, "o": {"a": 0, "k": 0}, "m": 1, "tr": {
                "p": {"a": 0, "k": [50, 0]}, "a": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}, "r": {"a": 0, "k": 0},
                "so": {"a": 0, "k": 100}, "eo": {"a": 0, "k": 0}}},
        ])),
        // Round corners on a square path.
        shape_layer(2, json!({}), json!([
            {"ty": "sh", "ks": {"a": 0, "k": {"c": true, "v": [[0, 0], [40, 0], [40, 40], [0, 40]], "i": [[0, 0], [0, 0], [0, 0], [0, 0]], "o": [[0, 0], [0, 0], [0, 0], [0, 0]]}}},
            {"ty": "rd", "r": {"a": 0, "k": 10}}, fill(),
        ])),
        // Two overlapping squares merged: added, then intersected.
        shape_layer(3, json!({}), json!([rc(20.0, 20.0, 40.0, 40.0), rc(40.0, 20.0, 40.0, 40.0), {"ty": "mm", "mm": 2}, fill()])),
        shape_layer(4, json!({}), json!([rc(20.0, 20.0, 40.0, 40.0), rc(40.0, 20.0, 40.0, 40.0), {"ty": "mm", "mm": 4}, fill()])),
    ]}));
    let top = |i: usize| {
        let mut v = Vec::new();
        leaves(&d.shapes()[i], &mut v);
        v
    };
    // Bottom first: layer 4 is index 0.
    assert!((area(&top(0)[0]) - 800.0).abs() < 1.0, "{}", area(&top(0)[0]));
    assert!((area(&top(1)[0]) - 2400.0).abs() < 1.0, "{}", area(&top(1)[0]));
    let rounded = &top(2)[0];
    assert!(area(rounded) < 1600.0 - 50.0 && area(rounded) > 1600.0 - 4.0 * 100.0, "{}", area(rounded));
    let copies = top(3);
    assert_eq!(copies.len(), 3);
    // Group opacity ends up in the fill colour.
    let alpha = |c: &Shape| c.opacity * match c.fill {
        Some(Paint::Solid(k)) => k.a,
        _ => 1.0,
    };
    let mut xs: Vec<(f64, f32)> = copies.iter().map(|c| (bounds(c).center().x, alpha(c))).collect();
    xs.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert!((xs[0].0 - 10.0).abs() < 1e-6 && (xs[1].0 - 60.0).abs() < 1e-6 && (xs[2].0 - 110.0).abs() < 1e-6, "{xs:?}");
    assert!((xs[0].1 - 1.0).abs() < 1e-6 && (xs[1].1 - 0.5).abs() < 1e-6 && xs[2].1.abs() < 1e-6, "{xs:?}");
}

#[test]
fn mask_modes_and_auto_orient() {
    let mask = |mode: &str, x: f64, inv: bool| json!({"mode": mode, "inv": inv, "o": {"a": 0, "k": 100}, "pt": {"a": 0, "k": {"c": true, "v": [[x, 0], [x + 50.0, 0], [x + 50.0, 100], [x, 100]], "i": [[0, 0], [0, 0], [0, 0], [0, 0]], "o": [[0, 0], [0, 0], [0, 0], [0, 0]]}}});
    let big = || json!([rc(50.0, 50.0, 100.0, 100.0), fill()]);
    let d = import(json!({"fr": 10, "ip": 0, "op": 10, "w": 100, "h": 100, "layers": [
        // Add the left half, subtract the right 25 of it.
        json!({"ty": 4, "ind": 1, "ip": 0, "op": 10, "st": 0, "ks": {}, "shapes": big(), "hasMask": true, "masksProperties": [mask("a", 0.0, false), mask("s", 25.0, false)]}),
        // Inverted: everything but the left half.
        json!({"ty": 4, "ind": 2, "ip": 0, "op": 10, "st": 0, "ks": {}, "shapes": big(), "hasMask": true, "masksProperties": [mask("a", 0.0, true)]}),
        // Moving diagonally down-right, facing its way.
        rect_layer(3, json!({"p": {"a": 1, "k": [sq(0.0, json!([0, 0])), sq(10.0, json!([100, 100]))]}}), json!({"ao": 1})),
    ]}));
    let clip_area = |i: usize| {
        let mut v = Vec::new();
        leaves(&d.shapes()[i], &mut v);
        let c = v[0].clip.as_ref().expect("clipped");
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        // Clip ∩ the 100×100 square.
        let ShapeKind::Path(pd) = &c.kind else { panic!() };
        let polys = crate::boolean::path_to_polys(&crate::geom::pathdata_to_bez(pd), crate::document::FillRule::NonZero);
        let sq = crate::boolean::path_to_polys(&kurbo::Shape::to_path(&r, 0.1), crate::document::FillRule::NonZero);
        kurbo::Shape::area(&crate::geom::polys_to_bez(&crate::boolean::op(&polys, &sq, crate::boolean::BoolOp::Intersect))).abs()
    };
    assert!((clip_area(2) - 2500.0).abs() < 1.0, "{}", clip_area(2));
    assert!((clip_area(1) - 5000.0).abs() < 1.0, "{}", clip_area(1));
    // The 20×10 rect turned 45°: as tall as it is wide.
    let b = bounds(&leaf(&at(&d, 0.5), 0));
    assert!((b.width() - b.height()).abs() < 1e-3 && b.width() > 20.0, "{b:?}");
}

#[test]
fn glyph_text_with_animators() {
    let glyph = |ch: &str| json!({"ch": ch, "size": 100, "style": "Regular", "w": 50, "fFamily": "Test",
        "data": {"shapes": [{"ty": "gr", "it": [{"ty": "sh", "ks": {"a": 0, "k": {"c": true, "v": [[0, 0], [40, 0], [40, -60], [0, -60]], "i": [[0, 0], [0, 0], [0, 0], [0, 0]], "o": [[0, 0], [0, 0], [0, 0], [0, 0]]}}}]}]}});
    // "ABCD" at size 100, the selector's end sweeping 0 → 100 %: letters
    // fade in from the left (opacity 0 where not yet selected... inverted:
    // the animator sets opacity 0 on selected letters).
    let d = import(json!({"fr": 10, "ip": 0, "op": 10, "w": 300, "h": 100,
        "fonts": {"list": [{"fName": "Test-Regular", "fFamily": "Test", "fStyle": "Regular", "ascent": 70}]},
        "chars": [glyph("A"), glyph("B"), glyph("C"), glyph("D")],
        "layers": [{"ty": 5, "ind": 1, "ip": 0, "op": 10, "st": 0, "ks": {"p": {"a": 0, "k": [10, 80]}},
            "t": {"d": {"k": [{"t": 0, "s": {"s": 100, "f": "Test-Regular", "t": "ABCD", "j": 0, "tr": 0, "lh": 120, "fc": [1, 0, 0]}}]},
                "a": [{"s": {"t": 0, "r": 1, "b": 1, "sh": 1, "a": {"a": 0, "k": 100}, "s": {"a": 1, "k": [sq(0.0, json!([0])), sq(10.0, json!([100]))]}},
                       "a": {"o": {"a": 0, "k": 0}, "p": {"a": 0, "k": [0, 20]}}}]}}]}));
    let letters = |t: f64| all_leaves(&at(&d, t));
    let l0 = letters(0.0);
    assert_eq!(l0.len(), 4);
    // Frame 0: every letter selected (hidden, 20 lower).
    assert!(l0.iter().all(|l| l.opacity < 1e-3), "{:?}", l0.iter().map(|l| l.opacity).collect::<Vec<_>>());
    // Halfway: A and B are no longer selected: shown, in place.
    let l5 = letters(0.5);
    let mut by_x: Vec<&Shape> = l5.iter().collect();
    by_x.sort_by(|a, b| bounds(a).x0.total_cmp(&bounds(b).x0));
    assert!((by_x[0].opacity - 1.0).abs() < 1e-3 && (by_x[1].opacity - 1.0).abs() < 1e-3, "{:?}", by_x.iter().map(|l| l.opacity).collect::<Vec<_>>());
    assert!(by_x[2].opacity < 1e-3 && by_x[3].opacity < 1e-3);
    assert!((bounds(by_x[0]).y1 - 80.0).abs() < 1e-3 && (bounds(by_x[3]).y1 - 100.0).abs() < 1e-3, "{:?} {:?}", bounds(by_x[0]), bounds(by_x[3]));
    // Advances: 50 apart, starting at the layer's origin.
    assert!((bounds(by_x[1]).x0 - 60.0).abs() < 1e-3, "{:?}", bounds(by_x[1]));
    assert!(matches!(by_x[0].fill, Some(Paint::Solid(c)) if c.r == 1.0));
}

#[test]
fn export_keys_images_and_mattes() {
    use crate::document::{ImageAsset, ImageFill, ImageMode, Mask, MaskMode, Rgba};
    use crate::io::export::{Format, Options, export};
    let (mut d, id) = crate::tests_anim::moving_doc_for_tests();
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let img = d.add_image(ImageAsset::from_encoded(png).unwrap());
    let mut m = Shape::new(ShapeKind::Ellipse { w: 40.0, h: 40.0 });
    m.fill = Some(Paint::Solid(Rgba::WHITE));
    for page in [&mut d] {
        let s = page.get_mut(id).unwrap();
        s.fill = Some(Paint::Image(ImageFill { image: img, mode: ImageMode::Stretch, scale: 1.0, opacity: 1.0 }));
        s.mask = Some(Box::new(Mask { shape: m.clone(), mode: MaskMode::Alpha, invert: false }));
    }
    let mut f = crate::fonts::FontManager::new(egui::Context::default());
    let p = std::env::temp_dir().join("mtzd-lottie-images.json");
    export(&d, &[], d.primary().id, &Options { format: Format::Lottie, ..Default::default() }, &p, &mut f).unwrap();
    let j: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(j["assets"].as_array().unwrap().len(), 1);
    let layers = j["layers"].as_array().unwrap();
    let image = layers.iter().find(|l| l["ty"] == 2).expect("an image layer");
    assert_eq!(image["tt"], 1, "matted");
    assert!(image["hasMask"].as_bool().unwrap());
    // Eased motion: fewer keys than frames where it's nearly straight.
    assert!(image["ks"]["p"]["k"].as_array().unwrap().len() < 11, "{}", image["ks"]["p"]);
    // Steady motion needs only its ends; a turn adds a key.
    let line: Vec<serde_json::Value> = (0..=20).map(|t| json!([t as f64 * 5.0, if t <= 10 { 0.0 } else { (t - 10) as f64 }])).collect();
    assert_eq!(crate::io::anim_export::key_frames(&line), vec![0, 10, 20]);
    let pos = layers.iter().position(|l| l["ty"] == 2).unwrap();
    assert_eq!(layers[pos - 1]["td"], 1, "the matte sits just above");
    // And back: an image fill, masked.
    let back = crate::io::open(&p, &mut f).unwrap();
    let leaves = all_leaves(&back);
    let imgs: Vec<&Shape> = leaves.iter().filter(|s| matches!(s.fill, Some(Paint::Image(_)))).collect();
    assert_eq!(imgs.len(), 1);
    assert!(back.shapes().iter().any(|s| s.mask.is_some()));
}

/// `cargo test lottie_dup_ids -- --ignored --nocapture`: duplicate shape ids
/// after importing the files in $LOTTIE_DIR.
#[test]
#[ignore]
fn lottie_dup_ids() {
    let dir = std::env::var("LOTTIE_DIR").unwrap();
    let mut f = crate::fonts::FontManager::new(egui::Context::default());
    for e in walk(std::path::Path::new(&dir)) {
        let d = crate::io::open(&e, &mut f).unwrap();
        let mut seen = std::collections::HashMap::new();
        fn visit(s: &Shape, seen: &mut std::collections::HashMap<u64, Vec<String>>) {
            seen.entry(s.id).or_default().push(s.name.clone());
            if let Some(c) = s.kind.children() { c.iter().for_each(|c| visit(c, seen)); }
            if let Some(c) = &s.clip { visit(c, seen); }
            if let Some(m) = &s.mask { visit(&m.shape, seen); }
        }
        for s in d.shapes() { visit(s, &mut seen); }
        let dups: Vec<_> = seen.iter().filter(|(_, v)| v.len() > 1).collect();
        println!("{}: {} shapes, {} duplicated ids {:?}", e.display(), seen.len(), dups.len(), dups.iter().take(5).collect::<Vec<_>>());
    }
}

fn walk(p: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(p).unwrap().flatten() {
        let q = e.path();
        if q.is_dir() { out.extend(walk(&q)); } else if q.extension().is_some_and(|x| x == "json") { out.push(q); }
    }
    out.sort();
    out
}

/// Every imported shape has its own id (the timeline keys rows by them),
/// and ellipses start at the top like Lottie's (dashes and trims depend on
/// it).
#[test]
fn unique_ids_and_ellipse_start() {
    let ell = json!({"ty": "el", "p": {"a": 0, "k": [50, 50]}, "s": {"a": 0, "k": [40, 40]}});
    let stroke = json!({"ty": "st", "c": {"a": 0, "k": [0, 0, 0, 1]}, "o": {"a": 0, "k": 100}, "w": {"a": 0, "k": 2}});
    let layers: Vec<serde_json::Value> = (1..=4)
        .map(|i| rect_layer(i, json!({"p": {"a": 1, "k": [sq(0.0, json!([0, 0])), sq(10.0, json!([i * 10, 0]))]}}), json!({"shapes": [
            {"ty": "gr", "it": [ell.clone(), stroke.clone(), {"ty": "tr"}]},
            {"ty": "gr", "it": [rc(10.0, 10.0, 5.0, 5.0), fill(), {"ty": "tr"}]},
        ]})))
        .collect();
    let d = import(json!({"fr": 10, "ip": 0, "op": 10, "w": 100, "h": 100, "layers": layers}));
    let mut ids = Vec::new();
    fn visit(s: &Shape, ids: &mut Vec<u64>) {
        ids.push(s.id);
        s.kind.children().into_iter().flatten().for_each(|c| visit(c, ids));
    }
    d.shapes().iter().for_each(|s| visit(s, &mut ids));
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "duplicate ids");
    let e = all_leaves(&d).into_iter().find(|s| s.stroke.is_some()).unwrap();
    let ShapeKind::Path(pd) = &e.kind else { panic!() };
    let start = pd.subpaths[0].nodes[0].p;
    assert!((start.x - 50.0).abs() < 1e-6 && (start.y - 30.0).abs() < 1e-6, "{start:?}");
}

#[test]
#[ignore]
fn lottie_debug_layer() {
    let p = std::env::var("LOTTIE_FILE").unwrap();
    let name = std::env::var("LOTTIE_LAYER").unwrap();
    let t: f64 = std::env::var("LOTTIE_T").ok().and_then(|t| t.parse().ok()).unwrap_or(0.0);
    let d = lottie::import(&std::fs::read(p).unwrap(), None).unwrap();
    let d = if std::env::var("LOTTIE_RAW").is_ok() { d } else { at(&d, t) };
    fn show(s: &Shape, depth: usize) {
        let b = if s.kind.children().is_none() { Some(bounds(s)) } else { None };
        println!("{}{} {} vis={} op={} fill={:?} fills={} stroke={:?} clip={:?} b={:?}", "  ".repeat(depth), s.name, s.kind.type_name(), s.visible, s.opacity,
            s.fill.as_ref().map(|f| match f { Paint::Solid(c) => format!("{:.2},{:.2},{:.2},{:.2}", c.r, c.g, c.b, c.a), _ => "other".into() }),
            s.fills.len(), s.stroke.as_ref().map(|st| (st.width, st.color.a)), s.clip.as_ref().map(|c| bounds(c)), b);
        s.kind.children().into_iter().flatten().for_each(|c| show(c, depth + 1));
    }
    for s in d.shapes() {
        s.kind.children().into_iter().flatten().filter(|c| c.name == name).for_each(|c| show(c, 0));
        if s.name == name { show(s, 0); }
    }
}

/// A placed Lottie file comes as one group centred on the artboard, and
/// moves as a whole: keyed outlines (a morph) and moving layers alike, at
/// every frame, also after the group is dragged.
#[test]
fn placed_animation_is_one_movable_group() {
    use crate::app::App;
    let path = |x: f64| json!([{"i": [[0, 0], [0, 0], [0, 0]], "o": [[0, 0], [0, 0], [0, 0]], "v": [[0, 0], [x, 0], [x, 50]], "c": true}]);
    let file = json!({"fr": 10, "ip": 0, "op": 10, "w": 200, "h": 200, "layers": [
        // A morph (outline keys) …
        shape_layer(1, json!({}), json!([{"ty": "gr", "it": [
            {"ty": "sh", "ks": {"a": 1, "k": [sq(0.0, path(50.0)), sq(10.0, path(100.0))]}}, fill(), {"ty": "tr"}]}])),
        // … and a moving square (a transform track).
        rect_layer(2, json!({"p": {"a": 1, "k": [sq(0.0, json!([20, 150])), sq(10.0, json!([180, 150]))]}}), json!({})),
    ]});
    let p = std::env::temp_dir().join("mtzd-place-test.json");
    std::fs::write(&p, file.to_string()).unwrap();
    let mut app = App::new(egui::Context::default(), None);
    {
        let a = app.doc.primary_mut();
        (a.x, a.y, a.w, a.h) = (1000.0, 500.0, 400.0, 400.0);
    }
    app.place_path(&p, None);
    assert_eq!(app.doc.shapes().len(), 1, "one group");
    let g = app.selection[0];
    assert_eq!(app.doc.get(g).unwrap().name, "mtzd-place-test");
    // The file's frame (0..200) centred on the artboard (1000..1400, 500..900).
    let off = kurbo::Vec2::new(1100.0, 600.0);
    let leaves_at = |app: &mut App, t: f64| {
        app.anim_seek(t);
        let mut v = Vec::new();
        leaves(app.doc.get(g).unwrap(), &mut v);
        v.iter().map(bounds).collect::<Vec<Rect>>()
    };
    let check = |b: &[Rect], off: kurbo::Vec2, t: f64| {
        // Leaves bottom first: the square (layer 2), then the morph.
        let x = 20.0 + 160.0 * t;
        assert!((b[0].center() - (Point::new(x, 150.0) + off)).hypot() < 1e-6, "{t}: square {:?}", b[0]);
        let w = 50.0 + 50.0 * t;
        let want = Rect::new(0.0, 0.0, w, 50.0) + off;
        assert!((b[1].origin() - want.origin()).hypot() < 1e-6 && (b[1].width() - want.width()).abs() < 1e-6, "{t}: morph {:?} vs {want:?}", b[1]);
    };
    for t in [0.0, 0.5, 1.0, 0.25] {
        let b = leaves_at(&mut app, t);
        check(&b, off, t);
    }
    // Dragged (not keyed): everything moves at every frame.
    app.anim_seek(0.5);
    let d = kurbo::Vec2::new(-300.0, 40.0);
    app.doc.get_mut(g).unwrap().apply_transform(kurbo::Affine::translate(d));
    for t in [0.5, 0.0, 1.0] {
        let b = leaves_at(&mut app, t);
        check(&b, off + d, t);
    }
}
