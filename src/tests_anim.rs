//! Keyframe animation: seeking, picking up edits, groups, undo, saving.

use kurbo::{Affine, Point};

use crate::animation::{self, Ease, Prop, Timeline, Value, capture_edits, points, seek};
use crate::document::{Document, Paint, PathData, Rgba, Shape, ShapeKind, SubPath};

fn pts(doc: &Document, id: u64) -> Vec<Point> {
    let mut v = Vec::new();
    points(doc.get(id).unwrap(), &mut v);
    v
}

fn close(a: &[Point], b: &[Point]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (*p - *q).hypot() < 1e-6)
}

fn rect_doc() -> (Document, u64) {
    let mut d = Document::default();
    let mut s = Shape::new(ShapeKind::Rect { w: 100.0, h: 50.0, radii: [0.0; 4] });
    s.transform = Affine::translate((10.0, 20.0));
    let id = d.add_shape(s);
    d.animation = Some(Timeline::default());
    (d, id)
}

/// Edit shape `id` with `f` at the playhead and key the change.
fn edit(d: &mut Document, id: u64, record: bool, f: impl FnOnce(&mut Shape)) {
    let base = d.clone();
    let mut s = d.get(id).unwrap().clone();
    f(&mut s);
    d.replace(s);
    capture_edits(&base, d, record, &[id]);
}

#[test]
fn recorded_move_plays_back() {
    let (mut d, id) = rect_doc();
    let start = pts(&d, id);
    seek(&mut d, 1.0);
    edit(&mut d, id, true, |s| s.apply_transform(Affine::translate((100.0, 0.0))));
    let tr = d.get(id).unwrap().anim.as_deref().unwrap().track(Prop::Transform).unwrap().clone();
    assert_eq!(tr.keys.len(), 2, "a rest key at 0 and the move at 1 s");
    let end = pts(&d, id);
    seek(&mut d, 0.0);
    assert!(close(&pts(&d, id), &start));
    // Linear halfway is halfway; the default easing is symmetric too.
    seek(&mut d, 0.5);
    assert!((pts(&d, id)[0].x - (start[0].x + 50.0)).abs() < 1e-6);
    seek(&mut d, 1.0);
    assert!(close(&pts(&d, id), &end));
    // Past the last key it holds.
    seek(&mut d, 2.5);
    assert!(close(&pts(&d, id), &end));
}

#[test]
fn edits_without_record_only_key_animated_properties() {
    let (mut d, id) = rect_doc();
    seek(&mut d, 1.0);
    edit(&mut d, id, false, |s| s.apply_transform(Affine::translate((5.0, 0.0))));
    assert!(d.get(id).unwrap().anim.is_none(), "a plain move");
    edit(&mut d, id, true, |s| s.opacity = 0.5);
    let a = d.get(id).unwrap().anim.as_deref().unwrap().clone();
    assert!(a.track(Prop::Transform).is_none());
    assert_eq!(a.track(Prop::Opacity).unwrap().keys.len(), 2);
    // Now opacity is animated: changing it keys even without record.
    seek(&mut d, 2.0);
    edit(&mut d, id, false, |s| s.opacity = 0.2);
    assert_eq!(d.get(id).unwrap().anim.as_deref().unwrap().track(Prop::Opacity).unwrap().keys.len(), 3);
    seek(&mut d, 0.0);
    assert_eq!(d.get(id).unwrap().opacity, 1.0);
}

#[test]
fn rotating_a_path_turns_about_its_centre() {
    let mut d = Document::default();
    let nodes = [(0.0, 0.0), (100.0, 0.0), (100.0, 40.0), (0.0, 40.0)].map(|(x, y)| crate::document::Node { p: Point::new(x, y), hin: None, hout: None, smooth: false, corner: 0.0 });
    let id = d.add_shape(Shape::new(ShapeKind::Path(PathData { subpaths: vec![SubPath { nodes: nodes.to_vec(), closed: true }] })));
    d.animation = Some(Timeline::default());
    let c = Point::new(50.0, 20.0);
    seek(&mut d, 2.0);
    edit(&mut d, id, true, |s| s.apply_transform(Affine::rotate_about(2.0, c)));
    seek(&mut d, 1.0);
    // Halfway through the default ease: half the turn, about the centre.
    let quarter: Vec<Point> = nodes.iter().map(|n| Affine::rotate_about(1.0, c) * n.p).collect();
    assert!(close(&pts(&d, id), &quarter), "{:?}", pts(&d, id));
}

#[test]
fn groups_and_children_animate_together() {
    let (mut d, a) = rect_doc();
    let mut b = Shape::new(ShapeKind::Ellipse { w: 30.0, h: 30.0 });
    b.transform = Affine::translate((200.0, 0.0));
    let b = d.add_shape(b);
    let mut g = Shape::new(ShapeKind::Group { children: vec![d.get(a).unwrap().clone(), d.get(b).unwrap().clone()] });
    g.fill = None;
    d.remove(a);
    d.remove(b);
    let g = d.add_shape(g);
    let (a0, b0) = (pts(&d, a), pts(&d, b));
    // The child slides right by 1 s, the group turns by 2 s.
    seek(&mut d, 1.0);
    edit(&mut d, a, true, |s| s.apply_transform(Affine::translate((50.0, 0.0))));
    seek(&mut d, 2.0);
    let gc = Point::new(150.0, 30.0);
    edit(&mut d, g, true, |s| s.apply_transform(Affine::rotate_about(0.5, gc)));
    assert!(d.get(b).unwrap().anim.is_none(), "turning the group doesn't key its children");
    let a_child = d.get(a).unwrap().anim.as_deref().unwrap().track(Prop::Transform).unwrap().keys.len();
    assert_eq!(a_child, 2, "nor does it add keys to an animated child");
    let at2 = (pts(&d, a), pts(&d, b));
    for t in [0.0, 0.7, 1.5, 2.0, 0.0, 2.0] {
        seek(&mut d, t);
    }
    assert!(close(&pts(&d, a), &at2.0) && close(&pts(&d, b), &at2.1), "back where it was");
    seek(&mut d, 0.0);
    assert!(close(&pts(&d, a), &a0) && close(&pts(&d, b), &b0));
}

#[test]
fn moving_a_static_group_keeps_the_childs_motion() {
    let (mut d, a) = rect_doc();
    let mut g = Shape::new(ShapeKind::Group { children: vec![d.get(a).unwrap().clone()] });
    g.fill = None;
    d.remove(a);
    let g = d.add_shape(g);
    seek(&mut d, 1.0);
    edit(&mut d, a, true, |s| s.apply_transform(Affine::translate((80.0, 0.0))));
    // Move the whole group later on: a rest change, not a key.
    seek(&mut d, 0.5);
    edit(&mut d, g, false, |s| s.apply_transform(Affine::translate((0.0, 30.0))));
    assert_eq!(d.get(a).unwrap().anim.as_deref().unwrap().track(Prop::Transform).unwrap().keys.len(), 2);
    seek(&mut d, 0.0);
    let p0 = pts(&d, a)[0];
    seek(&mut d, 1.0);
    let p1 = pts(&d, a)[0];
    assert!((p1.x - p0.x - 80.0).abs() < 1e-6 && (p1.y - p0.y).abs() < 1e-6, "{p0:?} {p1:?}");
    assert!((p0.y - 50.0).abs() < 1e-6, "moved down with its group: {p0:?}");
}

#[test]
fn colour_and_easing() {
    let (mut d, id) = rect_doc();
    let red = Rgba::from_hex("#FF0000").unwrap();
    seek(&mut d, 1.0);
    edit(&mut d, id, true, |s| s.fill = Some(Paint::Solid(red)));
    let fill = |d: &Document| d.get(id).unwrap().fill.as_ref().and_then(|f| f.solid()).unwrap();
    seek(&mut d, 0.25);
    let eased = fill(&d).r;
    animation::edit_keys(&mut d, |d| {
        let mut s = d.get(id).unwrap().clone();
        s.anim.as_deref_mut().unwrap().track_mut(Prop::Fill).keys[0].ease = Ease::Linear;
        d.replace(s);
    });
    let linear = fill(&d).r;
    assert!(linear > eased, "editing keys updates the shape at the playhead: {eased} {linear}");
    animation::edit_keys(&mut d, |d| {
        let mut s = d.get(id).unwrap().clone();
        s.anim.as_deref_mut().unwrap().track_mut(Prop::Fill).keys[0].ease = Ease::Hold;
        d.replace(s);
    });
    assert_eq!(fill(&d), Rgba::from_hex("#D9D9D9").unwrap());
}

#[test]
fn deleting_a_transform_track_keeps_the_current_pose() {
    let (mut d, id) = rect_doc();
    seek(&mut d, 1.0);
    edit(&mut d, id, true, |s| s.apply_transform(Affine::translate((40.0, 0.0))));
    seek(&mut d, 0.5);
    let here = pts(&d, id);
    animation::edit_keys(&mut d, |d| {
        let mut s = d.get(id).unwrap().clone();
        s.anim = None;
        d.replace(s);
    });
    assert!(close(&pts(&d, id), &here));
}

#[test]
fn animation_survives_saving() {
    let (mut d, id) = rect_doc();
    seek(&mut d, 1.0);
    edit(&mut d, id, true, |s| s.apply_transform(Affine::scale_about(2.0, Point::new(60.0, 45.0))));
    seek(&mut d, 0.4);
    let json = serde_json::to_string(&d).unwrap();
    let mut back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.animation, d.animation);
    for t in [1.0, 0.0] {
        seek(&mut d, t);
        seek(&mut back, t);
        assert!(close(&pts(&d, id), &pts(&back, id)));
    }
    assert!(matches!(back.get(id).unwrap().anim.as_deref().unwrap().track(Prop::Transform).unwrap().keys[1].value, Value::Xf(_)));
}

#[test]
fn affine_fit_rejects_reshaping() {
    let a = [Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(0.0, 1.0), Point::new(1.0, 1.0)];
    let m = Affine::new([1.2, 0.3, -0.4, 0.9, 5.0, 6.0]);
    let b: Vec<Point> = a.iter().map(|p| m * *p).collect();
    let f = animation::fit(&a, &b).unwrap();
    assert!(close(&b, &a.iter().map(|p| f * *p).collect::<Vec<_>>()));
    let mut c = b.clone();
    c[3].x += 0.5;
    assert!(animation::fit(&a, &c).is_none());
}

fn fonts() -> crate::fonts::FontManager {
    crate::fonts::FontManager::new(egui::Context::default())
}

pub fn moving_doc_for_tests() -> (Document, u64) {
    moving_doc()
}

/// A red square moving right and turning blue over a second.
fn moving_doc() -> (Document, u64) {
    let (mut d, id) = rect_doc();
    edit(&mut d, id, false, |s| s.fill = Some(Paint::Solid(Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 })));
    seek(&mut d, 1.0);
    edit(&mut d, id, true, |s| {
        s.apply_transform(Affine::translate((200.0, 0.0)));
        s.fill = Some(Paint::Solid(Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }));
    });
    animation::seek(&mut d, 0.0);
    // Small: video encoding is slow in test builds.
    d.set_size(320.0, 120.0);
    d.animation.as_mut().unwrap().duration = 1.0;
    d.animation.as_mut().unwrap().fps = 10.0;
    (d, id)
}

#[test]
fn animated_exports() {
    use crate::io::export::{Format, Options, export};
    let (d, _) = moving_doc();
    let mut f = fonts();
    let dir = std::env::temp_dir().join("mtzd-anim-test");
    std::fs::create_dir_all(&dir).unwrap();
    let opts = |format| Options { format, ..Default::default() };

    // SVG: still renders (frame 0) and carries keyframes for the motion and colour.
    let p = dir.join("a.svg");
    export(&d, &[], d.primary().id, &opts(Format::Svg), &p, &mut f).unwrap();
    let svg = std::fs::read_to_string(&p).unwrap();
    assert!(svg.contains("@keyframes k0") && svg.contains("transform:matrix(1,0,0,1,200,0)"), "{svg}");
    assert!(svg.contains("@keyframes f0") && svg.contains("fill:#0000FF"), "{svg}");
    assert!(svg.contains(r#"class="m0""#));
    usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap();

    // Lottie: a keyed path and fill colour over 10 frames.
    let p = dir.join("a.json");
    export(&d, &[], d.primary().id, &opts(Format::Lottie), &p, &mut f).unwrap();
    let j: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(j["op"], 10);
    assert_eq!(j["fr"], 10.0);
    let layer = &j["layers"][0];
    assert_eq!(layer["ty"], 4);
    let it = &layer["shapes"][0]["it"];
    assert_eq!(it[0]["ks"]["a"], 0, "the outline stays: the layer moves");
    assert_eq!(it[0]["ks"]["k"]["v"][0], serde_json::json!([10.0, 20.0]));
    let pos = &layer["ks"]["p"];
    assert_eq!(pos["a"], 1);
    let last = pos["k"].as_array().unwrap().last().unwrap();
    assert_eq!(last["t"], 10);
    assert_eq!(last["s"], serde_json::json!([200.0, 0.0, 0]));
    let fill = it.as_array().unwrap().iter().find(|x| x["ty"] == "fl").unwrap();
    assert_eq!(fill["c"]["a"], 1);
    assert_eq!(j["layers"][1]["ty"], 1, "the white artboard behind");

    // WebM: 10 frames of the artboard.
    let p = dir.join("a.webm");
    export(&d, &[], d.primary().id, &opts(Format::Webm), &p, &mut f).unwrap();
    let data = std::fs::read(&p).unwrap();
    assert_eq!(&data[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
}

/// `cargo test write_anim_demo -- --ignored`: /tmp/mtzd-anim-demo.{mvec,svg,json,webm}
/// with rotation, a stroke, a fading group and moving text.
#[test]
#[ignore]
fn write_anim_demo() {
    use crate::document::{Stroke, TextData};
    use crate::io::export::{Format, Options, export};
    let mut d = Document::default();
    d.set_size(480.0, 270.0);
    d.animation = Some(Timeline { duration: 2.0, fps: 24.0, ..Default::default() });
    let mut star = Shape::new(ShapeKind::Star { w: 100.0, h: 100.0, points: 5, inner: 0.5, corner: 4.0 });
    star.transform = Affine::translate((40.0, 40.0));
    star.fill = Some(Paint::Solid(Rgba::from_hex("#F5B700").unwrap()));
    star.stroke = Some(Stroke { color: Rgba::from_hex("#7A4B00").unwrap(), width: 4.0, ..Default::default() });
    let star = d.add_shape(star);
    let mut a = Shape::new(ShapeKind::Ellipse { w: 60.0, h: 60.0 });
    a.transform = Affine::translate((300.0, 40.0));
    a.fill = Some(Paint::Solid(Rgba::from_hex("#2E86DE").unwrap()));
    let mut b = Shape::new(ShapeKind::Rect { w: 60.0, h: 60.0, radii: [10.0; 4] });
    b.transform = Affine::translate((380.0, 40.0));
    b.fill = Some(Paint::Solid(Rgba::from_hex("#10AC84").unwrap()));
    let group = d.add_shape(Shape::new(ShapeKind::Group { children: vec![a, b] }));
    let mut t = Shape::new(ShapeKind::Text(TextData { content: "Hello".into(), size: 40.0, ..Default::default() }));
    t.fill = Some(Paint::Solid(Rgba::from_hex("#202020").unwrap()));
    t.transform = Affine::translate((40.0, 190.0));
    let text = d.add_shape(t);

    let nodes = [(200.0, 40.0), (260.0, 40.0), (260.0, 100.0), (200.0, 100.0)].map(|(x, y)| crate::document::Node { p: Point::new(x, y), hin: None, hout: None, smooth: false, corner: 0.0 });
    let mut blob = Shape::new(ShapeKind::Path(PathData { subpaths: vec![SubPath { nodes: nodes.to_vec(), closed: true }] }));
    blob.fill = Some(Paint::Solid(Rgba::from_hex("#E84393").unwrap()));
    let blob = d.add_shape(blob);
    let mut card = Shape::new(ShapeKind::Rect { w: 80.0, h: 50.0, radii: [0.0; 4] });
    card.transform = Affine::translate((360.0, 190.0));
    card.fill = Some(Paint::Solid(Rgba::from_hex("#6C5CE7").unwrap()));
    let card = d.add_shape(card);
    seek(&mut d, 2.0);
    edit(&mut d, blob, true, |s| {
        if let ShapeKind::Path(pd) = &mut s.kind {
            pd.subpaths[0].nodes[2].p = Point::new(290.0, 130.0);
            pd.subpaths[0].nodes[1].hout = Some(Point::new(290.0, 50.0));
        }
    });
    edit(&mut d, card, true, |s| {
        if let ShapeKind::Rect { radii, .. } = &mut s.kind {
            *radii = [25.0; 4];
        }
    });
    edit(&mut d, star, true, |s| {
        let mut v = Vec::new();
        points(s, &mut v);
        let c = v.iter().fold(Point::ZERO, |a, p| a + p.to_vec2() / v.len() as f64);
        s.apply_transform(Affine::rotate_about(2.5, c));
    });
    edit(&mut d, group, true, |s| {
        s.opacity = 0.1;
        s.apply_transform(Affine::translate((0.0, 120.0)));
    });
    edit(&mut d, text, true, |s| s.apply_transform(Affine::translate((240.0, 0.0))));
    seek(&mut d, 0.0);
    let mut f = fonts();
    crate::io::native::save(&d, std::path::Path::new("/tmp/mtzd-anim-demo.mvec"), None, crate::io::native::SaveOptions { backup: false, preview: false }).unwrap();
    for (fmt, ext) in [(Format::Svg, "svg"), (Format::Lottie, "json"), (Format::Webm, "webm")] {
        let p = std::path::PathBuf::from(format!("/tmp/mtzd-anim-demo.{ext}"));
        export(&d, &[], d.primary().id, &Options { format: fmt, ..Default::default() }, &p, &mut f).unwrap();
    }
}

/// Click ◇ on `p` of `id` at the playhead.
fn key(d: &mut Document, id: u64, p: Prop) {
    let t = d.animation.as_ref().unwrap().time;
    animation::edit_keys(d, |d| animation::key_current(d, id, p, t));
}

#[test]
fn keying_after_moving_keeps_the_move() {
    // Key, go to 1 s, move, key again (it was keyed by the move already).
    let (mut d, id) = rect_doc();
    let start = pts(&d, id);
    key(&mut d, id, Prop::Transform);
    seek(&mut d, 1.0);
    edit(&mut d, id, false, |s| s.apply_transform(Affine::translate((100.0, 0.0))));
    key(&mut d, id, Prop::Transform);
    let moved = pts(&d, id);
    assert!((moved[0].x - start[0].x - 100.0).abs() < 1e-6);
    seek(&mut d, 0.0);
    assert!(close(&pts(&d, id), &start));
    seek(&mut d, 1.0);
    assert!(close(&pts(&d, id), &moved));
}

fn square_path() -> (Document, u64) {
    let mut d = Document::default();
    let nodes = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)].map(|(x, y)| crate::document::Node { p: Point::new(x, y), hin: None, hout: None, smooth: false, corner: 0.0 });
    let id = d.add_shape(Shape::new(ShapeKind::Path(PathData { subpaths: vec![SubPath { nodes: nodes.to_vec(), closed: true }] })));
    d.animation = Some(Timeline::default());
    (d, id)
}

fn node_pts(d: &Document, id: u64) -> Vec<Point> {
    match &d.get(id).unwrap().kind {
        ShapeKind::Path(pd) => pd.subpaths[0].nodes.iter().map(|n| n.p).collect(),
        _ => panic!(),
    }
}

/// Drag node `i` of the path by `v`.
fn drag_node(s: &mut Shape, i: usize, v: (f64, f64)) {
    if let ShapeKind::Path(pd) = &mut s.kind {
        pd.subpaths[0].nodes[i].p += kurbo::Vec2::new(v.0, v.1);
    }
}

#[test]
fn path_points_morph() {
    let (mut d, id) = square_path();
    key(&mut d, id, Prop::Shape);
    seek(&mut d, 1.0);
    // A shape track keys node edits without Record.
    edit(&mut d, id, false, |s| drag_node(s, 2, (50.0, 50.0)));
    assert_eq!(node_pts(&d, id)[2], Point::new(150.0, 150.0));
    seek(&mut d, 0.0);
    assert_eq!(node_pts(&d, id)[2], Point::new(100.0, 100.0));
    seek(&mut d, 0.5);
    assert!((node_pts(&d, id)[2] - Point::new(125.0, 125.0)).hypot() < 1e-6);
    assert_eq!(node_pts(&d, id)[0], Point::new(0.0, 0.0));
}

#[test]
fn morph_and_motion_together() {
    let (mut d, id) = square_path();
    key(&mut d, id, Prop::Shape);
    key(&mut d, id, Prop::Transform);
    seek(&mut d, 1.0);
    edit(&mut d, id, false, |s| drag_node(s, 2, (50.0, 50.0)));
    edit(&mut d, id, false, |s| s.apply_transform(Affine::translate((200.0, 0.0))));
    let end = node_pts(&d, id);
    assert_eq!(end[0], Point::new(200.0, 0.0));
    assert_eq!(end[2], Point::new(350.0, 150.0));
    seek(&mut d, 0.0);
    assert_eq!(node_pts(&d, id)[2], Point::new(100.0, 100.0));
    seek(&mut d, 0.3);
    seek(&mut d, 1.0);
    let back = node_pts(&d, id);
    assert!(back.iter().zip(&end).all(|(a, b)| (*a - *b).hypot() < 1e-6), "{back:?}");
}

#[test]
fn moving_a_morphing_path_moves_its_whole_animation() {
    let (mut d, id) = square_path();
    key(&mut d, id, Prop::Shape);
    seek(&mut d, 1.0);
    edit(&mut d, id, false, |s| drag_node(s, 2, (50.0, 50.0)));
    // No transform track, no Record: the move is for good.
    edit(&mut d, id, false, |s| s.apply_transform(Affine::translate((0.0, 300.0))));
    seek(&mut d, 0.0);
    assert!((node_pts(&d, id)[0] - Point::new(0.0, 300.0)).hypot() < 1e-9);
    assert!((node_pts(&d, id)[2] - Point::new(100.0, 400.0)).hypot() < 1e-9);
}

#[test]
fn corner_radius_animates() {
    let (mut d, id) = rect_doc();
    key(&mut d, id, Prop::Shape);
    seek(&mut d, 1.0);
    edit(&mut d, id, false, |s| {
        if let ShapeKind::Rect { radii, .. } = &mut s.kind {
            *radii = [20.0; 4];
        }
    });
    seek(&mut d, 0.5);
    let ShapeKind::Rect { radii, w, .. } = d.get(id).unwrap().kind.clone() else { panic!() };
    assert!((radii[0] - 10.0).abs() < 1e-9);
    assert_eq!(w, 100.0);
    let saved = serde_json::to_string(&d).unwrap();
    let back: Document = serde_json::from_str(&saved).unwrap();
    assert_eq!(back.get(id).unwrap().anim, d.get(id).unwrap().anim);
}

#[test]
fn holding_still_at_the_end_of_a_drag_keeps_the_key() {
    // A drag rebuilds the shape every frame from a copy taken when it
    // started (without the keys the drag has made since).
    let (mut d, id) = rect_doc();
    let start = pts(&d, id);
    key(&mut d, id, Prop::Transform);
    seek(&mut d, 1.0);
    let original = d.get(id).unwrap().clone();
    for x in [30.0, 60.0, 100.0, 100.0, 100.0] {
        let base = d.clone();
        let mut s = original.clone();
        s.apply_transform(Affine::translate((x, 0.0)));
        d.replace(s);
        capture_edits(&base, &mut d, false, &[id]);
    }
    assert_eq!(d.get(id).unwrap().anim.as_deref().unwrap().track(Prop::Transform).unwrap().keys.len(), 2);
    seek(&mut d, 0.0);
    assert!(close(&pts(&d, id), &start));
}

#[test]
fn bezier_easing() {
    // Straight handles are linear; the presets' handles match their curves.
    let lin = Ease::Bezier([1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0]);
    for u in [0.0, 0.1, 0.5, 0.77, 1.0] {
        assert!((lin.apply(u) - u).abs() < 1e-6);
        for e in [Ease::EaseInOut, Ease::EaseIn, Ease::EaseOut] {
            assert!((Ease::Bezier(e.handles()).apply(u) - e.apply(u)).abs() < 1e-6, "{e:?} at {u}");
        }
    }
    // Overshoot goes past the end value and comes back.
    let back = Ease::Bezier([0.34, 1.56, 0.64, 1.0]);
    assert!((0..100).map(|i| back.apply(i as f64 / 100.0)).fold(0.0, f64::max) > 1.05);
    assert!((back.apply(1.0) - 1.0).abs() < 1e-9);
    // Keys use it, and it survives saving.
    let (mut d, id) = rect_doc();
    key(&mut d, id, Prop::Transform);
    seek(&mut d, 1.0);
    edit(&mut d, id, false, |s| s.apply_transform(Affine::translate((100.0, 0.0))));
    let mut s = d.get(id).unwrap().clone();
    s.anim.as_deref_mut().unwrap().track_mut(Prop::Transform).keys[0].ease = back;
    d.replace(s);
    let x = |d: &Document| pts(d, id)[0].x;
    let end = x(&d);
    seek(&mut d, 0.75);
    assert!(x(&d) > end, "overshoots");
    let saved = serde_json::to_string(&d).unwrap();
    let back_doc: Document = serde_json::from_str(&saved).unwrap();
    assert_eq!(back_doc.get(id).unwrap().anim, d.get(id).unwrap().anim);
}

#[test]
fn exports_cover_the_in_and_out_points() {
    let (mut d, _) = moving_doc();
    d.animation.as_mut().unwrap().range = Some((0.2, 0.6));
    let mut f = fonts();
    let pages = crate::io::export::animation_pages(&d, &[], Some(d.primary().id), true, &mut f).unwrap();
    assert_eq!(pages.len(), 5, "4 frames at 10 fps and the closing one");
    // The first page is the document at the in point.
    let at = |doc: &Document| doc.shapes()[0].transform.translation().x;
    assert!((at(&pages[0]) - at(&animation::frame(&d, 0.2))).abs() < 1e-9);
    assert!((at(&pages[4]) - at(&animation::frame(&d, 0.6))).abs() < 1e-9);
}

#[test]
fn effects_and_dashes_animate() {
    use crate::document::{Shadow, Stroke};
    let (mut d, id) = rect_doc();
    edit(&mut d, id, false, |s| {
        s.shadow = Some(Shadow { color: Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, dx: 0.0, dy: 0.0, blur: 0.0 });
        s.stroke = Some(Stroke { dash: vec![2.0, 2.0], ..Stroke::default() });
    });
    seek(&mut d, 1.0);
    edit(&mut d, id, true, |s| {
        s.shadow = Some(Shadow { color: Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }, dx: 10.0, dy: 20.0, blur: 8.0 });
        s.blur = 4.0;
        s.noise = 0.5;
        let st = s.stroke.as_mut().unwrap();
        st.dash = vec![6.0, 2.0];
        st.dash_offset = 4.0;
    });
    let a = d.get(id).unwrap().anim.as_deref().unwrap().clone();
    for p in [Prop::Shadow, Prop::Blur, Prop::Noise, Prop::Dash, Prop::DashOffset] {
        assert_eq!(a.track(p).map(|t| t.keys.len()), Some(2), "{p:?}");
    }
    seek(&mut d, 0.5);
    let s = d.get(id).unwrap();
    let sh = s.shadow.clone().unwrap();
    assert!((sh.dx - 5.0).abs() < 1e-9 && (sh.dy - 10.0).abs() < 1e-9 && (sh.color.a - 0.5).abs() < 1e-6, "{sh:?}");
    assert!((s.blur - 2.0).abs() < 1e-9 && (s.noise - 0.25).abs() < 1e-6);
    let st = s.stroke.as_ref().unwrap();
    assert_eq!(st.dash, vec![4.0, 2.0]);
    assert!((st.dash_offset - 2.0).abs() < 1e-9);
    seek(&mut d, 0.0);
    let s = d.get(id).unwrap();
    assert_eq!(s.blur, 0.0);
    assert_eq!(s.shadow.as_ref().unwrap().dx, 0.0);
}

#[test]
fn steps_and_typing() {
    use crate::document::{BlendMode, TextData};
    let mut d = Document::default();
    let id = d.add_shape(Shape::new(ShapeKind::Text(TextData { content: "Hello".into(), ..Default::default() })));
    d.animation = Some(Timeline { duration: 4.0, ..Default::default() });
    for p in [Prop::Text, Prop::Visible, Prop::Blend] {
        key(&mut d, id, p);
    }
    seek(&mut d, 1.0);
    edit(&mut d, id, false, |s| {
        if let ShapeKind::Text(t) = &mut s.kind {
            t.content = "Help me".into();
        }
        s.visible = false;
        s.blend = BlendMode::Multiply;
    });
    let text = |d: &Document| match &d.get(id).unwrap().kind {
        ShapeKind::Text(t) => t.content.clone(),
        _ => unreachable!(),
    };
    // "Hello" → "Help me": two characters deleted, then four typed.
    let lin = |d: &mut Document| animation::edit_keys(d, |d| d.get_mut(id).unwrap().anim.as_deref_mut().unwrap().track_mut(Prop::Text).keys[0].ease = Ease::Linear);
    lin(&mut d);
    seek(&mut d, 1.0 / 6.0);
    assert_eq!(text(&d), "Hell");
    seek(&mut d, 2.0 / 6.0);
    assert_eq!(text(&d), "Hel");
    seek(&mut d, 4.0 / 6.0);
    assert_eq!(text(&d), "Help ");
    // Visibility and blend mode switch at the key, not halfway.
    seek(&mut d, 0.9);
    let s = d.get(id).unwrap();
    assert!(s.visible && s.blend == BlendMode::Normal);
    seek(&mut d, 1.0);
    let s = d.get(id).unwrap();
    assert!(!s.visible && s.blend == BlendMode::Multiply);
    assert_eq!(text(&d), "Help me");
}

#[test]
fn text_attributes_animate() {
    use crate::document::TextData;
    let mut d = Document::default();
    let id = d.add_shape(Shape::new(ShapeKind::Text(TextData { content: "A".into(), size: 10.0, weight: 400, ..Default::default() })));
    d.animation = Some(Timeline::default());
    seek(&mut d, 1.0);
    edit(&mut d, id, true, |s| {
        if let ShapeKind::Text(t) = &mut s.kind {
            t.size = 30.0;
            t.weight = 700;
            t.letter_spacing = 4.0;
            t.line_height = 2.0;
        }
    });
    let a = d.get(id).unwrap().anim.as_deref().unwrap().clone();
    for p in [Prop::FontSize, Prop::FontWeight, Prop::LetterSpacing, Prop::LineHeight] {
        assert!(a.track(p).is_some(), "{p:?}");
    }
    assert!(a.track(Prop::Text).is_none(), "the characters didn't change");
    seek(&mut d, 0.0);
    let ShapeKind::Text(t) = &d.get(id).unwrap().kind else { unreachable!() };
    assert_eq!((t.size, t.weight), (10.0, 400));
}

#[test]
fn gradient_fills_blend() {
    use crate::document::{Gradient, GradientKind};
    let (mut d, id) = rect_doc();
    let red = Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    let blue = Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
    edit(&mut d, id, false, |s| s.fill = Some(Paint::Solid(red)));
    key(&mut d, id, Prop::Fill);
    seek(&mut d, 1.0);
    edit(&mut d, id, false, |s| s.fill = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, red, blue))));
    animation::edit_keys(&mut d, |d| d.get_mut(id).unwrap().anim.as_deref_mut().unwrap().track_mut(Prop::Fill).keys[0].ease = Ease::Linear);
    // Halfway: a gradient from red to purple.
    seek(&mut d, 0.5);
    let Some(Paint::Gradient(g)) = &d.get(id).unwrap().fill else { panic!("{:?}", d.get(id).unwrap().fill) };
    assert_eq!(g.stops[0].color, red);
    assert!((g.stops[1].color.r - 0.5).abs() < 1e-6 && (g.stops[1].color.b - 0.5).abs() < 1e-6);
    seek(&mut d, 0.0);
    assert_eq!(d.get(id).unwrap().fill, Some(Paint::Solid(red)));
}

#[test]
fn visibility_fades_in_vector_exports() {
    use crate::io::export::{Format, Options, export};
    let (mut d, id) = moving_doc();
    seek(&mut d, 0.5);
    edit(&mut d, id, true, |s| s.visible = false);
    seek(&mut d, 0.0);
    let mut f = fonts();
    let p = std::env::temp_dir().join("mtzd-anim-vis.svg");
    export(&d, &[], d.primary().id, &Options { format: Format::Svg, ..Default::default() }, &p, &mut f).unwrap();
    let svg = std::fs::read_to_string(&p).unwrap();
    assert!(svg.contains("@keyframes k0") && svg.contains("opacity:0"), "{svg}");
}

/// `cargo test write_props_demo -- --ignored`: /tmp/mtzd-props.mvec, a shape
/// with keyed effects and dashes, for screenshots of the tinted inspector.
#[test]
#[ignore]
fn write_props_demo() {
    use crate::document::{Shadow, Stroke};
    let (mut d, id) = rect_doc();
    edit(&mut d, id, false, |s| {
        s.fill = Some(Paint::Solid(Rgba { r: 1.0, g: 0.4, b: 0.2, a: 1.0 }));
        s.shadow = Some(Shadow::default());
        s.blur = 1.0;
        s.stroke = Some(Stroke { dash: vec![2.0, 2.0], width: 4.0, ..Stroke::default() });
        s.apply_transform(Affine::translate((200.0, 200.0)));
    });
    seek(&mut d, 1.0);
    edit(&mut d, id, true, |s| {
        s.shadow = Some(Shadow { dx: 30.0, dy: 30.0, blur: 20.0, ..Shadow::default() });
        s.blur = 3.0;
        s.stroke.as_mut().unwrap().dash_offset = 4.0;
    });
    seek(&mut d, 0.5);
    crate::io::native::save(&d, std::path::Path::new("/tmp/mtzd-props.mvec"), None, crate::io::native::SaveOptions { backup: false, preview: false }).unwrap();
}

/// Shift+Space plays and pauses wherever the pointer is; without an
/// animation it does nothing (no timeline gets made).
#[test]
fn shift_space_plays_anywhere() {
    use crate::commands::{lookup, Cmd, Press};
    let press = Press { key: egui::Key::Space, physical: Some(egui::Key::Space), mods: egui::Modifiers::SHIFT };
    assert_eq!(lookup(&press), Some(Cmd::PlayPause));
    let mut app = crate::app::App::new(egui::Context::default(), None);
    let ctx = egui::Context::default();
    Cmd::PlayPause.run(&mut app, &ctx);
    assert!(app.doc.animation.is_none() && !app.anim.playing);
    app.doc = moving_doc().0;
    Cmd::PlayPause.run(&mut app, &ctx);
    assert!(app.anim.playing);
    Cmd::PlayPause.run(&mut app, &ctx);
    assert!(!app.anim.playing);
}

/// Timing of a 1080p WebM export (frames and encoding), for profiling.
#[test]
#[ignore]
fn webm_timing() {
    use std::time::Instant;
    let mut f = crate::fonts::FontManager::new(egui::Context::default());
    let mut d = crate::document::Document::default();
    let mut s = crate::document::Shape::new(crate::document::ShapeKind::Ellipse { w: 160.0, h: 160.0 });
    s.transform = kurbo::Affine::translate((880.0, 80.0));
    let id = d.add_shape(s);
    d.animation = Some(crate::animation::Timeline { duration: 0.34, fps: 30.0, ..Default::default() });
    let mut s = d.get(id).unwrap().clone();
    let tr = s.anim.get_or_insert_with(Default::default).track_mut(crate::animation::Prop::Transform);
    tr.set(0.0, crate::animation::Value::Xf(Default::default()));
    tr.set(1.0, crate::animation::Value::Xf(crate::animation::Xf { dy: 720.0, ..Default::default() }));
    d.replace(s);
    let t0 = Instant::now();
    let r = crate::io::export::rasterize(&d, &mut f, 1.0).unwrap();
    eprintln!("one rasterize {:?} {}x{}", t0.elapsed(), r.width(), r.height());
    let t0 = Instant::now();
    let pages = crate::io::export::animation_pages(&d, &[], Some(d.primary().id), true, &mut f).unwrap();
    eprintln!("frames {:?} ({})", t0.elapsed(), pages.len());
    let t0 = Instant::now();
    let mut last = Instant::now();
    let out = crate::io::anim_export::webm(&pages, 30.0, 1.0, 80, &mut f, &mut |a, b| {
        eprintln!("  {a}/{b} +{:?}", last.elapsed());
        last = Instant::now();
    })
    .unwrap();
    eprintln!("webm {:?} {} bytes", t0.elapsed(), out.len());
}
