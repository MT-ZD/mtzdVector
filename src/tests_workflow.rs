//! UI workflow tests: auto layout, constraints, export slices and formats,
//! SVG options.

use kurbo::{Affine, Rect};

use crate::app::App;
use crate::document::{ExportSlice, Paint, Rgba, Shape, ShapeKind};
use crate::fonts::FontManager;
use crate::io::export::{Format, SliceTarget, export_slices};
use crate::io::svg::{SvgOptions, export_svg};
use crate::layout::{Constraints, Direction, Pin};

fn rect(x: f64, y: f64, w: f64, h: f64) -> Shape {
    let mut s = Shape::new(ShapeKind::Rect { w, h, radii: [0.0; 4] });
    s.transform = Affine::translate((x, y));
    s.fill = Some(Paint::Solid(Rgba::from_hex("#3366cc").unwrap()));
    s.stroke = None;
    s
}

fn bounds(app: &mut App, id: u64) -> Rect {
    let s = app.doc.get(id).unwrap().clone();
    crate::layout::geo_bounds(&s, &mut app.fonts).unwrap()
}

fn tmp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mtzd-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn auto_layout_stacks_and_reorders() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 40.0, 20.0));
    let b = app.doc.add_shape(rect(100.0, 5.0, 30.0, 40.0));
    let c = app.doc.add_shape(rect(300.0, 0.0, 20.0, 20.0));
    app.selection = vec![a, b, c];
    app.add_auto_layout();
    let g = app.selection[0];
    let l = app.doc.get(g).unwrap().layout.clone().unwrap();
    assert_eq!(l.dir, Direction::Row);
    let (ba, bb, bc) = (bounds(&mut app, a), bounds(&mut app, b), bounds(&mut app, c));
    // Even gaps, centred on the tallest member.
    assert!((bb.x0 - ba.x1 - l.gap).abs() < 1e-6 && (bc.x0 - bb.x1 - l.gap).abs() < 1e-6, "{ba:?} {bb:?} {bc:?}");
    assert!((ba.center().y - bb.center().y).abs() < 1e-6);
    // The background hugs content plus padding.
    let bg = bounds(&mut app, l.background.unwrap());
    assert!((bg.width() - (90.0 + 2.0 * l.gap + 24.0)).abs() < 1e-6, "{bg:?}");
    assert!((bg.x0 - (ba.x0 - 12.0)).abs() < 1e-6);
    // Growing a member pushes the rest along.
    if let Some(s) = app.doc.get_mut(a) {
        s.kind.set_size(80.0, 20.0);
    }
    app.layout_tick();
    let (ba, bb) = (bounds(&mut app, a), bounds(&mut app, b));
    assert!((bb.x0 - ba.x1 - l.gap).abs() < 1e-6);
    // Dragging the first member past the last re-orders the stack.
    if let Some(s) = app.doc.get_mut(a) {
        s.apply_transform(Affine::translate((400.0, 0.0)));
    }
    app.layout_tick();
    let (ba, bb, bc) = (bounds(&mut app, a), bounds(&mut app, b), bounds(&mut app, c));
    assert!(bb.x0 < bc.x0 && bc.x0 < ba.x0, "{ba:?} {bb:?} {bc:?}");
    // A column, with more padding.
    if let Some(s) = app.doc.get_mut(g) {
        let l = s.layout.as_mut().unwrap();
        l.dir = Direction::Column;
        l.padding = [20.0; 4];
    }
    app.layout_tick();
    let (bb, bc) = (bounds(&mut app, b), bounds(&mut app, c));
    assert!((bc.y0 - bb.y1 - l.gap).abs() < 1e-6, "{bb:?} {bc:?}");
    let bgr = bounds(&mut app, l.background.unwrap());
    assert!((bb.y0 - bgr.y0 - 20.0).abs() < 1e-6);
    // Stable: another tick changes nothing.
    let rev = app.doc.revision();
    app.layout_key = None;
    app.layout_tick();
    assert_eq!(app.doc.revision(), rev);
}

#[test]
fn constraints_on_group_resize() {
    let mut app = App::new(egui::Context::default(), None);
    // A 200×100 card: a full-width bar pinned left & right, an icon pinned right.
    let card = rect(0.0, 0.0, 200.0, 100.0);
    let mut bar = rect(10.0, 10.0, 180.0, 20.0);
    bar.constraints = Some(Constraints { h: Pin::Both, v: Pin::Start });
    let mut icon = rect(170.0, 70.0, 20.0, 20.0);
    icon.constraints = Some(Constraints { h: Pin::End, v: Pin::End });
    let ids = [app.doc.add_shape(card), app.doc.add_shape(bar), app.doc.add_shape(icon)];
    app.selection = ids.to_vec();
    app.group_selection();
    let g = app.selection[0];
    // Twice as wide, 50 px taller.
    let mut s = app.doc.get(g).unwrap().clone();
    let a = Affine::scale_non_uniform(2.0, 1.5);
    app.resize_shape(&mut s, a);
    app.doc.replace(s);
    let (bar, icon, card) = (bounds(&mut app, ids[1]), bounds(&mut app, ids[2]), bounds(&mut app, ids[0]));
    assert!((card.width() - 400.0).abs() < 1e-6 && (card.height() - 150.0).abs() < 1e-6, "card scales: {card:?}");
    assert!((bar.x0 - 10.0).abs() < 1e-6 && (bar.x1 - 390.0).abs() < 1e-6 && (bar.height() - 20.0).abs() < 1e-6, "{bar:?}");
    assert!((icon.width() - 20.0).abs() < 1e-6 && (icon.x1 - 390.0).abs() < 1e-6 && (icon.y1 - 140.0).abs() < 1e-6, "{icon:?}");
}

#[test]
fn slices_and_formats() {
    let mut app = App::new(egui::Context::default(), None);
    app.doc.artboards[0].w = 64.0;
    app.doc.artboards[0].h = 32.0;
    let (ax, ay) = (app.doc.artboards[0].x, app.doc.artboards[0].y);
    let mut s = rect(ax + 8.0, ay + 8.0, 16.0, 16.0);
    s.name = "Icon".into();
    s.exports = [1.0, 2.0, 3.0].map(|k| ExportSlice::new(k, Format::Png)).to_vec();
    s.exports.push(ExportSlice::new(1.0, Format::Svg));
    let id = app.doc.add_shape(s);
    app.doc.artboards[0].exports = vec![ExportSlice::new(1.0, Format::Webp), ExportSlice::new(1.0, Format::Avif), ExportSlice::new(1.0, Format::Dxf), ExportSlice::new(1.0, Format::Pdf)];
    let targets = crate::io::export::slice_targets(&app.doc);
    assert_eq!(targets.len(), 2);
    assert!(targets.contains(&SliceTarget::Shape(id)));
    let dir = tmp_dir("slices");
    let mut f = FontManager::new(egui::Context::default());
    let files = export_slices(&app.doc, &targets, &dir, &mut f).unwrap();
    let names: Vec<String> = files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    for n in ["Icon.png", "Icon@2x.png", "Icon@3x.png", "Icon.svg", "Artboard 1.webp", "Artboard 1.avif", "Artboard 1.dxf", "Artboard 1.pdf"] {
        assert!(names.contains(&n.to_string()), "{n} in {names:?}");
    }
    let png = image::open(dir.join("Icon@3x.png")).unwrap();
    assert_eq!((png.width(), png.height()), (48, 48));
    let webp = image::open(dir.join("Artboard 1.webp")).unwrap();
    assert_eq!((webp.width(), webp.height()), (64, 32));
    let avif = std::fs::read(dir.join("Artboard 1.avif")).unwrap();
    assert_eq!(&avif[4..8], b"ftyp");
    assert!(avif.windows(4).any(|w| w == b"avif"));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(Format::from_ext("JPEG"), Some(Format::Jpg));
    assert_eq!(Format::from_ext("dxf"), Some(Format::Dxf));
}

#[test]
fn svg_options() {
    let mut d = crate::document::Document::default();
    d.artboards[0].w = 100.0;
    d.artboards[0].h = 50.0;
    for _ in 0..2 {
        let mut s = rect(1.123456, 2.0, 10.0, 10.0);
        s.name = "Same name".into();
        d.add_shape(s);
    }
    let mut f = FontManager::new(egui::Context::default());
    let svg = export_svg(&d, &mut f, &SvgOptions { precision: 1, ..Default::default() });
    assert!(svg.contains(r#"id="Same_name""#) && svg.contains(r#"id="Same_name-2""#), "{svg}");
    assert!(svg.contains("1.1") && !svg.contains("1.123"), "{svg}");
    let svg = export_svg(&d, &mut f, &SvgOptions { ids: false, minify: true, responsive: true, ..Default::default() });
    assert!(!svg.contains("Same_name"));
    let root = svg.split('>').next().unwrap();
    assert!(!root.contains(" width=") && root.contains("viewBox"), "{root}");
    assert_eq!(svg.trim_end().lines().count(), 1, "{svg}");
    assert!(usvg::Tree::from_str(&svg, &usvg::Options::default()).is_ok());
    // Precision goes back to normal afterwards.
    assert_eq!(crate::io::num(1.23456), "1.235");
    // A second export reuses the plain ids.
    let again = export_svg(&d, &mut f, &SvgOptions::default());
    assert!(again.contains(r#"id="Same_name""#) && !again.contains("Same_name-3"));
}

#[test]
fn variants_switch_keeps_overrides() {
    let mut app = App::new(egui::Context::default(), None);
    let mk = |app: &mut App, x: f64, col: &str| {
        let mut bg = rect(x, 0.0, 80.0, 30.0);
        bg.name = "Bg".into();
        bg.fill = Some(Paint::Solid(Rgba::from_hex(col).unwrap()));
        let mut dot = rect(x + 5.0, 5.0, 10.0, 10.0);
        dot.name = "Dot".into();
        let ids = vec![app.doc.add_shape(bg), app.doc.add_shape(dot)];
        app.selection = ids;
        app.create_component();
        let id = app.selection[0];
        app.doc.get_mut(id).unwrap().name = if x == 0.0 { "Default".into() } else { "Hover".into() };
        id
    };
    let a = mk(&mut app, 0.0, "#cccccc");
    let b = mk(&mut app, 200.0, "#333333");
    app.selection = vec![a, b];
    app.combine_as_variants();
    assert_eq!(app.variants_of(a).len(), 2);
    let opts = app.variant_options(a);
    assert_eq!(opts["Variant"], vec!["Default".to_string(), "Hover".to_string()]);
    let inst = app.insert_instance(a, kurbo::Point::new(500.0, 300.0)).unwrap();
    app.components_tick();
    let before = bounds(&mut app, inst);
    // Override the dot's colour inside the instance.
    let dot = app.doc.get(inst).unwrap().kind.children().unwrap()[1].id;
    let red = Rgba::from_hex("#ff0000").unwrap();
    let mut d = app.doc.get(dot).unwrap().clone();
    d.fill = Some(Paint::Solid(red));
    app.doc.replace(d);
    app.components_tick();
    assert_eq!(app.override_count(inst), 1);
    assert!(app.set_instance_variant(inst, "Variant", "Hover"));
    app.components_tick();
    let link = app.instance_link(inst).unwrap().clone();
    assert_eq!(link.main, b);
    let kids = app.doc.get(inst).unwrap().kind.children().unwrap().clone();
    assert_eq!(kids[0].fill.as_ref().and_then(|f| f.solid()), Rgba::from_hex("#333333"));
    assert_eq!(kids[1].fill.as_ref().and_then(|f| f.solid()), Some(red), "override kept");
    assert_eq!(kids[1].id, dot, "layer ids kept");
    let after = bounds(&mut app, inst);
    assert!((after.x0 - before.x0).abs() < 1e-6 && (after.y0 - before.y0).abs() < 1e-6, "{before:?} {after:?}");
    // A new variant joins the set with a fresh value.
    let c = app.add_variant(b).unwrap();
    assert_eq!(app.variants_of(a).len(), 3);
    assert_eq!(crate::variants::props_of(app.doc.get(c).unwrap())["Variant"], "Variant 3");
    // Instance text properties.
    assert!(app.instance_texts(inst).is_empty());
}

#[test]
fn library_components_place_and_update() {
    let dir = tmp_dir("library");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Brand kit.mvec");
    // The library: one main using a colour key.
    let mut lib = App::new(egui::Context::default(), None);
    let key = lib.doc.add_color_key("Brand", Rgba::from_hex("#0055ff").unwrap());
    let mut a = rect(0.0, 0.0, 40.0, 40.0);
    a.name = "Face".into();
    a.fill_key = Some(key);
    a.fill = Some(Paint::Solid(Rgba::from_hex("#0055ff").unwrap()));
    let mut b = rect(10.0, 10.0, 5.0, 5.0);
    b.name = "Eye".into();
    let ids = vec![lib.doc.add_shape(a), lib.doc.add_shape(b)];
    lib.selection = ids;
    lib.create_component();
    let src = lib.selection[0];
    lib.doc.get_mut(src).unwrap().name = "Badge".into();
    crate::io::save_native(&lib.doc, &path).unwrap();

    let mut app = App::new(egui::Context::default(), None);
    let comps = app.libraries.get(&path).unwrap().components();
    assert_eq!(comps.len(), 1);
    assert_eq!(comps[0].1, "Badge");
    let i1 = app.insert_library_component(&path, src, kurbo::Point::new(300.0, 300.0)).unwrap();
    let i2 = app.insert_library_component(&path, src, kurbo::Point::new(400.0, 300.0)).unwrap();
    app.components_tick();
    let local = app.instance_link(i1).unwrap().main;
    assert_eq!(app.instance_link(i2).unwrap().main, local, "one local copy");
    assert_eq!(app.library_copy(&path, src), Some(local));
    let group = app.doc.parent_of(local).unwrap();
    let g = app.doc.get(group).unwrap();
    assert!(!g.visible && g.locked && g.name == "Library: Brand kit");
    assert!(app.doc.colors.iter().any(|k| k.name == "Brand"));
    assert!(app.library_updates().is_empty());
    // Override in an instance, then change the library.
    let eye = app.doc.get(i1).unwrap().kind.children().unwrap()[1].id;
    let mut e = app.doc.get(eye).unwrap().clone();
    e.fill = Some(Paint::Solid(Rgba::from_hex("#ff0000").unwrap()));
    app.doc.replace(e);
    app.components_tick();
    lib.doc.set_key_color(key, Rgba::from_hex("#00aa00").unwrap());
    if let Some(Some(c)) = lib.doc.get_mut(src).map(|s| s.kind.children_mut()) {
        c[1].kind.set_size(8.0, 8.0);
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    crate::io::save_native(&lib.doc, &path).unwrap();
    assert_eq!(app.library_updates(), vec![(local, true)]);
    assert_eq!(app.update_library_components(None), 1);
    for _ in 0..3 {
        app.components_tick();
    }
    assert!(app.library_updates().is_empty());
    let kids = app.doc.get(i1).unwrap().kind.children().unwrap().clone();
    assert_eq!(kids[0].fill.as_ref().and_then(|f| f.solid()), Rgba::from_hex("#00aa00"), "key colour follows");
    assert_eq!(kids[1].fill.as_ref().and_then(|f| f.solid()), Rgba::from_hex("#ff0000"), "override kept");
    assert_eq!(kids[1].id, eye);
    let eb = bounds(&mut app, eye);
    assert!((eb.width() - 8.0).abs() < 1e-6, "{eb:?}");
    let k = app.doc.colors.iter().find(|k| k.name == "Brand").unwrap();
    assert_eq!(k.color, Rgba::from_hex("#00aa00").unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writes /tmp/mtzd-variants.mvec (a variant set and an instance) and
/// /tmp/mtzd-library.mvec, for headless screenshots.
#[test]
#[ignore]
fn write_variant_documents() {
    let mut app = App::new(egui::Context::default(), None);
    let (ax, ay) = (app.doc.artboards[0].x, app.doc.artboards[0].y);
    let mut mains = Vec::new();
    for (i, (name, col)) in [("Default", "#3366cc"), ("Hover", "#2850a0")].iter().enumerate() {
        let x = ax + 60.0 + i as f64 * 200.0;
        let mut bg = rect(x, ay + 60.0, 140.0, 44.0);
        bg.name = "Background".into();
        bg.fill = Some(Paint::Solid(Rgba::from_hex(col).unwrap()));
        let mut t = Shape::new(ShapeKind::Text(crate::document::TextData { content: "Button".into(), size: 18.0, ..Default::default() }));
        t.transform = Affine::translate((x + 20.0, ay + 70.0));
        t.name = "Label".into();
        t.fill = Some(Paint::Solid(Rgba::WHITE));
        app.selection = vec![app.doc.add_shape(bg), app.doc.add_shape(t)];
        app.create_component();
        let id = app.selection[0];
        app.doc.get_mut(id).unwrap().name = name.to_string();
        mains.push(id);
    }
    app.selection = mains.clone();
    app.combine_as_variants();
    app.doc.get_mut(mains[0]).unwrap().name = "Button".into();
    app.insert_instance(mains[0], kurbo::Point::new(ax + 200.0, ay + 250.0));
    app.components_tick();
    // The instance first in the layer list (index 0 for `select 0`).
    let inst = app.selection[0];
    let s = app.doc.remove(inst).unwrap();
    app.doc.insert_shape(0, s);
    crate::io::save_native(&app.doc, std::path::Path::new("/tmp/mtzd-variants.mvec")).unwrap();
    crate::io::save_native(&app.doc, std::path::Path::new("/tmp/mtzd-library.mvec")).unwrap();
}

#[test]
fn flatten_auto_layout_group() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 40.0, 20.0));
    let b = app.doc.add_shape(rect(100.0, 0.0, 30.0, 20.0));
    app.selection = vec![a, b];
    app.add_auto_layout();
    let g = app.selection[0];
    let before = bounds(&mut app, g);
    app.flatten();
    let id = app.selection[0];
    let s = app.doc.get(id).unwrap().clone();
    assert!(matches!(s.kind, ShapeKind::Path(_)), "{:?}", s.kind.type_name());
    assert!(app.doc.get(g).is_none() && s.layout.is_none());
    let after = bounds(&mut app, id);
    assert!((after.width() - before.width()).abs() < 1e-6, "{before:?} {after:?}");
}

#[test]
fn shear_box_into_parallelogram() {
    let mut app = App::new(egui::Context::default(), None);
    let id = app.doc.add_shape(rect(0.0, 0.0, 100.0, 50.0));
    app.selection = vec![id];
    app.shear_selection(true, 45.0);
    let b = bounds(&mut app, id);
    assert!((b.width() - 150.0).abs() < 1e-6 && (b.height() - 50.0).abs() < 1e-6, "{b:?}");
    // Dragging the top edge 30 px right keeps the bottom edge in place.
    let bx = Rect::new(0.0, 0.0, 100.0, 50.0);
    let xf = crate::canvas::shear_xf(bx, true, Some(0.0), kurbo::Point::new(50.0, 0.0), kurbo::Vec2::new(30.0, 0.0), false, false);
    assert!(((xf * kurbo::Point::new(0.0, 0.0)).x - 30.0).abs() < 1e-9);
    assert!(((xf * kurbo::Point::new(0.0, 50.0)).x).abs() < 1e-9);
    // Shift snaps to 15° steps.
    let xf = crate::canvas::shear_xf(bx, true, Some(0.0), kurbo::Point::new(50.0, 0.0), kurbo::Vec2::new(14.0, 0.0), false, true);
    let ang = (xf.as_coeffs()[2]).atan().to_degrees().abs();
    assert!((ang - 15.0).abs() < 1e-6, "{ang}");
}

#[test]
fn sheared_box_follows_the_slant() {
    let mut app = App::new(egui::Context::default(), None);
    let id = app.doc.add_shape(rect(0.0, 0.0, 100.0, 200.0));
    app.selection = vec![id];
    // A vertical shear: the sides stay upright, top and bottom slant.
    app.shear_selection(false, 30.0);
    let (frame, local) = app.sel_frame().unwrap();
    let corners: Vec<kurbo::Point> = [(local.x0, local.y0), (local.x1, local.y0), (local.x1, local.y1), (local.x0, local.y1)].iter().map(|&(x, y)| frame * kurbo::Point::new(x, y)).collect();
    let s = app.doc.get(id).unwrap().clone();
    let shape: Vec<kurbo::Point> = [(0.0, 0.0), (100.0, 0.0), (100.0, 200.0), (0.0, 200.0)].iter().map(|&(x, y)| s.transform * kurbo::Point::new(x, y)).collect();
    // The box is the parallelogram itself.
    for (c, p) in corners.iter().zip(&shape) {
        assert!((*c - *p).hypot() < 1e-6, "{corners:?} vs {shape:?}");
    }
    // Width along the slanted top and the upright height.
    assert!((local.height() - 200.0).abs() < 1e-6, "{local:?}");
    assert!((local.width() - 100.0 / 30f64.to_radians().cos()).abs() < 1e-6, "{local:?}");
}

#[test]
fn quick_actions_find_commands_by_other_names() {
    use crate::commands::Cmd;
    let app = App::new(egui::Context::default(), None);
    let top = |q: &str| crate::ui::palette::matches(&app, q).into_iter().take(5).map(|x| x.0).collect::<Vec<_>>();
    assert!(top("distort").contains(&Cmd::EnvelopeWarp), "{:?}", top("distort"));
    assert!(top("Dist").contains(&Cmd::EnvelopeWarp) || top("Dist").contains(&Cmd::DistributeH));
    assert!(top("vectorize").contains(&Cmd::TraceImage));
    assert!(top("skew").contains(&Cmd::Tool(crate::app::Tool::Select)));
    assert!(top("pathfinder").contains(&Cmd::Union));
    assert!(top("settings").contains(&Cmd::Preferences));
    // The real name still wins.
    assert_eq!(top("warp")[0], Cmd::EnvelopeWarp);
    // The alias that matched is reported; a name match reports none.
    let m = crate::ui::palette::matches(&app, "distort");
    assert_eq!(m.iter().find(|x| x.0 == Cmd::EnvelopeWarp).unwrap().1, Some("distort"));
    assert_eq!(crate::ui::palette::matches(&app, "warp")[0].1, None);
}

#[test]
fn layer_nesting_and_auto_layout_drops() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 40.0, 20.0));
    let b = app.doc.add_shape(rect(100.0, 0.0, 30.0, 20.0));
    let c = app.doc.add_shape(rect(500.0, 500.0, 10.0, 10.0));
    app.selection = vec![a, b];
    app.group_selection();
    let g = app.selection[0];
    // Into a group (on top), then beside a nested layer, then out again.
    assert!(app.doc.move_into(c, Some(g), usize::MAX));
    assert_eq!(app.doc.parent_of(c), Some(g));
    assert_eq!(app.doc.get(g).unwrap().kind.children().unwrap().last().unwrap().id, c);
    assert!(app.doc.move_beside(c, a, false));
    assert_eq!(app.doc.get(g).unwrap().kind.children().unwrap()[0].id, c);
    assert!(!app.doc.move_into(g, Some(a), 0), "not into a leaf");
    assert!(app.doc.move_beside(c, g, true));
    assert_eq!(app.doc.parent_of(c), None);
    // Geometry is kept.
    let bc = bounds(&mut app, c);
    assert!((bc.x0 - 500.0).abs() < 1e-9);
    // An auto layout: a shape dropped over it joins in the slot it was
    // dropped in; dragged out again it leaves.
    let x = app.doc.add_shape(rect(0.0, 200.0, 40.0, 20.0));
    let y = app.doc.add_shape(rect(100.0, 200.0, 40.0, 20.0));
    app.selection = vec![x, y];
    app.add_auto_layout();
    let al = app.selection[0];
    app.layout_tick();
    let (bx, by) = (bounds(&mut app, x), bounds(&mut app, y));
    let mid = kurbo::Point::new((bx.x1 + by.x0) / 2.0, bx.center().y);
    if let Some(s) = app.doc.get_mut(c) {
        s.transform = Affine::translate((mid.x - 5.0, mid.y - 5.0));
    }
    assert!(app.layout_drop(&[c], mid));
    app.layout_tick();
    assert_eq!(app.doc.parent_of(c), Some(al));
    let (bx, bc, by) = (bounds(&mut app, x), bounds(&mut app, c), bounds(&mut app, y));
    assert!(bx.x1 < bc.x0 && bc.x1 < by.x0, "{bx:?} {bc:?} {by:?}");
    let far = kurbo::Point::new(2000.0, 2000.0);
    assert!(app.layout_drop(&[c], far));
    assert_ne!(app.doc.parent_of(c), Some(al));
}

#[test]
fn auto_layout_fill_paints_its_background() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 40.0, 20.0));
    let b = app.doc.add_shape(rect(100.0, 0.0, 30.0, 20.0));
    app.selection = vec![a, b];
    app.add_auto_layout();
    let g = app.selection[0];
    let bg = app.doc.get(g).and_then(|s| s.layout.as_ref()).and_then(|l| l.background).unwrap();
    // Fill and stroke edits on the layout reach its background, not the members.
    assert_eq!(app.style_targets(), vec![bg]);
    // Clicking the background (between the members) picks the layout.
    let gap = bounds(&mut app, bg);
    app.selection.clear();
    let p = kurbo::Point::new(gap.x0 + 2.0, gap.y0 + 2.0);
    if let Some(mut s) = app.doc.get(bg).cloned() {
        s.fill = Some(Paint::Solid(Rgba::WHITE));
        app.doc.replace(s);
    }
    assert_eq!(app.pick(p, false), Some(g));
    assert_eq!(app.pick(p, true), Some(g));
    // Members dropped below it are put back on top of it.
    app.doc.move_into(b, Some(g), 0);
    app.layout_key = None;
    app.layout_tick();
    let kids: Vec<u64> = app.doc.get(g).unwrap().kind.children().unwrap().iter().map(|c| c.id).collect();
    assert_eq!(kids[0], bg, "{kids:?}");
}

#[test]
fn blend_dialog_previews_and_cancels() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 10.0, 10.0));
    let b = app.doc.add_shape(rect(100.0, 0.0, 10.0, 10.0));
    app.selection = vec![a, b];
    app.ui.blend.steps = 3;
    app.begin_blend();
    // The blend is on the canvas while the dialog is open.
    let g = app.ui.blend.target.unwrap();
    let n = |app: &App, g: u64| app.doc.get(g).and_then(|s| s.kind.children().map(|c| c.len())).unwrap_or(0);
    assert_eq!(n(&app, g), 5);
    app.ui.blend.steps = 7;
    app.blend_preview();
    assert_eq!(n(&app, g), 9);
    // Cancel puts the two shapes back, selected.
    app.end_blend(false);
    assert!(app.doc.get(g).is_none() && app.doc.get(a).is_some() && app.doc.get(b).is_some());
    assert_eq!(app.selection, vec![a, b]);
    // Apply keeps it; re-opening edits the same blend.
    app.begin_blend();
    let g = app.ui.blend.target.unwrap();
    app.end_blend(true);
    assert!(app.doc.get(g).is_some_and(|s| s.live.is_some()));
    app.selection = vec![g];
    app.begin_blend();
    assert_eq!(app.ui.blend.target, Some(g));
    app.ui.blend.steps = 2;
    app.end_blend(true);
    assert_eq!(n(&app, g), 4);
}
