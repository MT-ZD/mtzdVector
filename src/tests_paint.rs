//! Drawing and paint tests: masks, blend modes, multiple fills
//! and strokes, conic/mesh gradients, shape blends, warps and tracing.

use kurbo::{Affine, ParamCurveNearest as _, Point, Rect, Shape as _};

use crate::app::App;
use crate::document::{BlendMode, Document, Gradient, GradientKind, MaskMode, Paint, Rgba, Shape, ShapeKind, Stroke};
use crate::fonts::FontManager;
use crate::paint_ops::{self, TraceState, WarpStyle};

fn fonts() -> FontManager {
    FontManager::new(egui::Context::default())
}

fn rect(x: f64, y: f64, w: f64, h: f64, hex: &str) -> Shape {
    let mut s = Shape::new(ShapeKind::Rect { w, h, radii: [0.0; 4] });
    s.transform = Affine::translate((x, y));
    s.fill = Some(Paint::Solid(Rgba::from_hex(hex).unwrap()));
    s.stroke = None;
    s
}

/// Render the first artboard of `doc` and return the pixel at document (x, y).
fn pixel(doc: &Document, fonts: &mut FontManager, x: f64, y: f64) -> [u8; 4] {
    let a = &doc.artboards[0];
    let (w, h) = (a.w as u32, a.h as u32);
    let px = crate::render::svg_paint::render_document(doc, fonts, w, h).unwrap();
    let i = ((y - a.y) as usize * w as usize + (x - a.x) as usize) * 4;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

fn doc_100() -> Document {
    let mut d = Document::default();
    d.artboards[0].x = 0.0;
    d.artboards[0].y = 0.0;
    d.artboards[0].w = 100.0;
    d.artboards[0].h = 100.0;
    d.artboards[0].background = Rgba::WHITE;
    d
}

#[test]
fn mask_make_release_and_render() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 100.0, 100.0, "#ff0000"));
    let mut m = rect(0.0, 0.0, 50.0, 100.0, "#ffffff");
    m.name = "Mask".into();
    let b = app.doc.add_shape(m);
    app.selection = vec![a, b];
    app.make_mask(MaskMode::Luminance);
    assert_eq!(app.selection, vec![a]);
    assert!(app.doc.get(b).is_none());
    let masked = app.doc.get(a).unwrap().clone();
    assert!(masked.mask.is_some());
    // White mask half shows the red; the other half is hidden.
    let mut d = doc_100();
    d.add_shape(masked.clone());
    let mut f = fonts();
    let l = pixel(&d, &mut f, 25.0, 50.0);
    let r = pixel(&d, &mut f, 75.0, 50.0);
    assert!(l[0] > 240 && l[1] < 20, "{l:?}");
    assert!(r[1] > 240, "hidden side shows the white background: {r:?}");
    // Inverted: the other way round.
    let mut inv = masked.clone();
    inv.mask.as_mut().unwrap().invert = true;
    let mut d = doc_100();
    d.add_shape(inv);
    assert!(pixel(&d, &mut f, 25.0, 50.0)[1] > 240);
    assert!(pixel(&d, &mut f, 75.0, 50.0)[1] < 20);
    // PDF export takes the mask too.
    let pdf = crate::io::pdf::export(&d, &mut f);
    assert!(pdf.starts_with(b"%PDF"));
    // Release puts it back above.
    app.release_mask();
    assert!(app.doc.get(a).unwrap().mask.is_none());
    assert_eq!(app.selection.len(), 2);
    assert_eq!(app.doc.shapes().len(), 2);
    // Several shapes get grouped under one mask.
    let c = app.doc.add_shape(rect(0.0, 0.0, 10.0, 10.0, "#000000"));
    app.selection = app.doc.shapes().iter().map(|s| s.id).chain([c]).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    app.make_mask(MaskMode::Alpha);
    let g = app.doc.get(app.selection[0]).unwrap();
    assert!(matches!(g.kind, ShapeKind::Group { .. }));
    assert_eq!(g.mask.as_ref().unwrap().mode, MaskMode::Alpha);
}

#[test]
fn blend_mode_multiplies_with_backdrop() {
    let mut d = doc_100();
    d.add_shape(rect(0.0, 0.0, 100.0, 100.0, "#ffff00"));
    let mut top = rect(0.0, 0.0, 100.0, 100.0, "#00ffff");
    top.blend = BlendMode::Multiply;
    d.add_shape(top);
    let mut f = fonts();
    let p = pixel(&d, &mut f, 50.0, 50.0);
    // Yellow × cyan = green.
    assert!(p[0] < 10 && p[1] > 245 && p[2] < 10, "{p:?}");
    // SVG and PDF carry the mode.
    let svg = crate::io::svg::export(&d, &mut f);
    assert!(svg.contains("mix-blend-mode:multiply"), "{svg}");
    let pdf = crate::io::pdf::export(&d, &mut f);
    assert!(pdf.windows(12).any(|w| w == b"/BM /Multipl") || pdf.windows(11).any(|w| w == b"/BM/Multipl"));
}

#[test]
fn extra_fills_and_strokes_stack() {
    let mut d = doc_100();
    let mut s = rect(20.0, 20.0, 60.0, 60.0, "#ff0000");
    // Fills list paints above the main fill: a half-transparent blue on top.
    s.fills = vec![Paint::Solid(Rgba::from_hex("#0000ff").unwrap().with_alpha(0.5))];
    s.stroke = Some(Stroke { width: 4.0, color: Rgba::BLACK, ..Default::default() });
    s.strokes = vec![Stroke { width: 12.0, color: Rgba::from_hex("#00ff00").unwrap(), ..Default::default() }];
    d.add_shape(s);
    let mut f = fonts();
    let c = pixel(&d, &mut f, 50.0, 50.0);
    assert!(c[0] > 100 && c[2] > 100, "red under blue: {c:?}");
    // Edge: the black stroke on the wide green one.
    assert!(pixel(&d, &mut f, 20.0, 50.0)[1] < 40);
    assert!(pixel(&d, &mut f, 15.5, 50.0)[1] > 200);
}

#[test]
fn conic_and_mesh_gradients_sample() {
    let mut g = Gradient::new(GradientKind::Linear, Rgba::from_hex("#ff0000").unwrap(), Rgba::from_hex("#0000ff").unwrap());
    g.set_kind(GradientKind::Conic);
    let a = g.color_at_unit([1.0, 0.5]);
    let b = g.color_at_unit([0.0, 0.5]);
    assert!((a.r - b.r).abs() > 0.2 || (a.b - b.b).abs() > 0.2, "{a:?} {b:?}");
    g.set_kind(GradientKind::Mesh);
    let mesh = g.mesh.as_mut().unwrap();
    let n = mesh.colors.len();
    for c in mesh.colors.iter_mut() {
        *c = Rgba::BLACK;
    }
    mesh.colors[0] = Rgba::WHITE;
    assert!(n >= 4);
    let corner = g.color_at_unit([0.0, 0.0]);
    let far = g.color_at_unit([1.0, 1.0]);
    assert!(corner.r > 0.95 && far.r < 0.05);
    // Exports bake them.
    let mut d = doc_100();
    let mut s = rect(0.0, 0.0, 100.0, 100.0, "#000000");
    s.fill = Some(Paint::Gradient(g));
    d.add_shape(s);
    let mut f = fonts();
    let p = pixel(&d, &mut f, 2.0, 2.0);
    assert!(p[0] > 200, "{p:?}");
    assert!(crate::io::pdf::export(&d, &mut f).starts_with(b"%PDF"));
}

#[test]
fn blend_between_two_shapes() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 10.0, 10.0, "#000000"));
    let mut e = Shape::new(ShapeKind::Ellipse { w: 40.0, h: 40.0 });
    e.transform = Affine::translate((100.0, 0.0));
    e.fill = Some(Paint::Solid(Rgba::WHITE));
    let b = app.doc.add_shape(e);
    app.selection = vec![a, b];
    app.blend_selection(3);
    let g = app.doc.get(app.selection[0]).unwrap();
    let ShapeKind::Group { children } = &g.kind else { panic!() };
    assert_eq!(children.len(), 5);
    let mut f = fonts();
    // Midway: grey, centred between, sized between.
    let mid = &children[2];
    let Some(Paint::Solid(c)) = &mid.fill else { panic!() };
    assert!((c.r - 0.5).abs() < 0.05, "{c:?}");
    let (path, _) = crate::render::compute_outline(mid, &mut f);
    let bb = path.bounding_box();
    assert!((bb.center().x - (5.0 + 120.0) / 2.0).abs() < 2.0, "{bb:?}");
    assert!((bb.width() - 25.0).abs() < 3.0, "{bb:?}");
    // Undo restores the two originals.
    app.undo();
    assert!(app.doc.get(a).is_some() && app.doc.get(b).is_some());
    assert_eq!(app.doc.shapes().len(), 2);
}

#[test]
fn envelope_warp_preview_apply_cancel() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 200.0, 100.0, "#000000"));
    app.selection = vec![a];
    app.ui.warp.style = WarpStyle::Arch;
    app.ui.warp.bend = 0.5;
    // The canvas has drawn the rectangle (its geometry is cached).
    let src = app.doc.get(a).unwrap().clone();
    app.cache.update(&src, &mut app.fonts);
    app.begin_warp();
    let g = app.selection[0];
    // The result reuses the source's id; it must not reuse its cached geometry.
    let res = app.doc.get(g).unwrap().kind.children().unwrap()[0].clone();
    let cached = app.cache.update(&res, &mut app.fonts).bounds.unwrap();
    assert!((cached.y0 - -25.0).abs() < 0.5, "stale cache: {cached:?}");
    let warped = |app: &mut App| {
        let s = app.doc.get(app.selection[0]).unwrap().clone();
        assert!(s.live.is_some());
        let c = s.kind.children().unwrap()[0].clone();
        assert!(matches!(c.kind, ShapeKind::Path(_)));
        crate::render::compute_outline(&c, &mut fonts()).0.bounding_box()
    };
    // The middle rises by bend × half the height.
    let bb = warped(&mut app);
    assert!((bb.y0 - -25.0).abs() < 0.5, "{bb:?}");
    // Cancel restores the rectangle.
    app.end_warp(false);
    assert!(matches!(app.doc.get(a).unwrap().kind, ShapeKind::Rect { .. }));
    assert!(app.doc.get(g).is_none() && app.selection == vec![a]);
    // Apply keeps it, live: the parameters stay editable.
    app.begin_warp();
    app.ui.warp.style = WarpStyle::Flag;
    app.end_warp(true);
    let g = app.selection[0];
    app.set_warp_params(g, paint_ops::WarpParams { style: WarpStyle::Arch, bend: -0.5, hdist: 0.0, vdist: 0.0 });
    let bb = warped(&mut app);
    assert!((bb.y1 - 125.0).abs() < 0.5, "{bb:?}");
    // Moving the group moves the envelope too: editing keeps the place.
    let mut s = app.doc.get(g).unwrap().clone();
    s.apply_transform(Affine::translate((50.0, 0.0)));
    app.doc.replace(s);
    app.set_warp_params(g, paint_ops::WarpParams { style: WarpStyle::Arch, bend: -0.4, hdist: 0.0, vdist: 0.0 });
    let bb = warped(&mut app);
    assert!((bb.x0 - 50.0).abs() < 0.5 && (bb.y1 - 120.0).abs() < 0.5, "{bb:?}");
    // Release gives the rectangle back (moved); undo and expand gives a path.
    app.release_live(g);
    let r = app.doc.get(app.selection[0]).unwrap().clone();
    assert!(matches!(r.kind, ShapeKind::Rect { .. }));
    app.undo();
    app.selection = vec![g];
    app.expand_live(g);
    let e = app.doc.get(app.selection[0]).unwrap();
    assert!(e.live.is_none() && matches!(e.kind, ShapeKind::Path(_)));
    app.undo();
    assert!(app.doc.get(g).is_some_and(|s| s.live.is_some()));
    // Every style keeps corners in place when the bend is zero.
    for st in WarpStyle::ALL {
        let b = Rect::new(0.0, 0.0, 10.0, 10.0);
        let q = paint_ops::warp_point(Point::new(3.0, 7.0), b, st, 0.0, 0.0, 0.0);
        assert!((q - Point::new(3.0, 7.0)).hypot() < 1e-9, "{st:?}");
    }
}

#[test]
fn svg_layer_ids_reset_per_render() {
    // Thumbnails render through SVG every frame: ids must not pile up.
    let mut d = Document::default();
    d.add_shape(rect(0.0, 0.0, 10.0, 10.0, "#000000"));
    let mut f = fonts();
    let items = crate::io::prepare(&d, &mut f);
    let _ = crate::io::svg::export_items(&d, &items);
    let svg = crate::io::svg::export_items(&d, &items);
    assert!(svg.contains(r#"id="Rectangle""#) && !svg.contains("Rectangle-2"), "{svg}");
}

#[test]
fn warp_follows_the_envelope() {
    // Every point of the warped rectangle's outline lies on the warped
    // envelope (the edges of the rectangle are the envelope).
    let b = Rect::new(0.0, 0.0, 300.0, 120.0);
    let frame = Affine::new([300.0, 0.0, 0.0, 120.0, 0.0, 0.0]);
    let mut f = fonts();
    for style in WarpStyle::ALL {
        let p = paint_ops::WarpParams { style, bend: 0.56, hdist: -1.0, vdist: 0.45 };
        let s = rect(b.x0, b.y0, b.width(), b.height(), "#000000");
        let w = paint_ops::warp_shape_in(&s, frame, p, &mut f).unwrap();
        let ShapeKind::Path(pd) = &w.kind else { panic!() };
        let bez = crate::geom::pathdata_to_bez(pd);
        // Sample the true warped edges and find each one near the result.
        let mut pts = Vec::new();
        kurbo::flatten(bez.iter(), 0.01, |el| if let kurbo::PathEl::LineTo(q) | kurbo::PathEl::MoveTo(q) = el { pts.push(q) });
        for i in 0..=40 {
            let t = i as f64 / 40.0;
            for q in [Point::new(t * 300.0, 0.0), Point::new(t * 300.0, 120.0), Point::new(0.0, t * 120.0), Point::new(300.0, t * 120.0)] {
                let want = p.map(frame, q);
                let d = pts.windows(2).map(|w| kurbo::Line::new(w[0], w[1]).nearest(want, 1e-9).distance_sq.sqrt()).fold(f64::MAX, f64::min);
                assert!(d < 0.05, "{style:?} {q:?} off by {d}");
            }
        }
    }
}

#[test]
fn live_blend_follows_its_ends() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(rect(0.0, 0.0, 10.0, 10.0, "#000000"));
    let b = app.doc.add_shape(rect(100.0, 0.0, 10.0, 10.0, "#ffffff"));
    app.selection = vec![a, b];
    app.blend_selection(3);
    let g = app.selection[0];
    app.live_tick();
    let mid_x = |app: &App| {
        let c = app.doc.get(g).unwrap().kind.children().unwrap().clone();
        (c.len(), crate::render::compute_outline(&c[2], &mut fonts()).0.bounding_box().center().x)
    };
    assert_eq!(mid_x(&app), (5, 55.0));
    // Move the last end: the steps follow.
    let mut e = app.doc.get(b).unwrap().clone();
    e.apply_transform(Affine::translate((100.0, 0.0)));
    app.doc.replace(e);
    app.doc.fresh_rev();
    app.live_tick();
    let (n, x) = mid_x(&app);
    assert_eq!(n, 5);
    assert!((x - 105.0).abs() < 1.0, "{x}");
    // More steps.
    app.set_blend_steps(g, 5);
    assert_eq!(mid_x(&app).0, 7);
    // Expand: no longer live.
    app.expand_live(g);
    assert!(app.doc.get(g).unwrap().live.is_none());
}

#[test]
fn trace_image_into_colour_layers() {
    let mut app = App::new(egui::Context::default(), None);
    // 64×64 image: white with a black disc and a red square.
    let mut img = image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]));
    for y in 0..64 {
        for x in 0..64 {
            let (dx, dy) = (x as f64 - 20.0, y as f64 - 20.0);
            if dx * dx + dy * dy < 144.0 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
            if (40..56).contains(&x) && (40..56).contains(&y) {
                img.put_pixel(x, y, image::Rgba([220, 20, 20, 255]));
            }
        }
    }
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let iid = app.doc.add_image(crate::document::ImageAsset::from_encoded(png).unwrap());
    let mut s = rect(0.0, 0.0, 64.0, 64.0, "#000000");
    s.fill = Some(Paint::Image(crate::document::ImageFill { image: iid, mode: crate::document::ImageMode::Stretch, scale: 1.0, opacity: 1.0 }));
    let id = app.doc.add_shape(s);
    app.selection = vec![id];
    app.ui.trace = TraceState { open: false, colours: 3, detail: 0.0, speckle: 4.0, smooth: 0.5 };
    app.trace_selection();
    let g = app.doc.get(app.selection[0]).unwrap().clone();
    let ShapeKind::Group { children } = &g.kind else { panic!() };
    assert_eq!(children.len(), 3);
    // Lightest first (white covers everything), black last.
    let col = |s: &Shape| match &s.fill {
        Some(Paint::Solid(c)) => *c,
        _ => panic!(),
    };
    assert!(col(&children[0]).r > 0.9 && col(&children[0]).g > 0.9);
    assert!(col(&children[2]).r < 0.1);
    let mut f = fonts();
    let disc = crate::render::compute_outline(&children[2], &mut f).0;
    let bb = disc.bounding_box();
    assert!((bb.center().x - 20.5).abs() < 1.5 && (bb.width() - 24.0).abs() < 2.5, "{bb:?}");
    let area = disc.area().abs();
    assert!((area - std::f64::consts::PI * 144.0).abs() < 60.0, "{area}");
    // The red layer covers the square and the disc (stacked).
    let red = crate::render::compute_outline(&children[1], &mut f).0;
    assert!(red.contains(Point::new(48.0, 48.0)) && red.contains(Point::new(20.0, 20.0)));
    assert!(!red.contains(Point::new(5.0, 60.0)));
}

/// Demo for headless screenshots: /tmp/mtzd-paint.mvec.
#[test]
#[ignore]
fn write_paint_document() {
    let mut app = App::new(egui::Context::default(), None);
    app.doc.artboards[0].w = 800.0;
    app.doc.artboards[0].h = 500.0;
    let (ax, ay) = (app.doc.artboards[0].x, app.doc.artboards[0].y);
    let r = |x: f64, y: f64, w: f64, h: f64, c: &str| rect(ax + x, ay + y, w, h, c);
    // Blend modes over a stripe.
    app.doc.add_shape(r(20.0, 20.0, 360.0, 60.0, "#3060ff"));
    for (i, m) in [BlendMode::Multiply, BlendMode::Screen, BlendMode::Difference, BlendMode::Overlay].into_iter().enumerate() {
        let mut s = Shape::new(ShapeKind::Ellipse { w: 70.0, h: 70.0 });
        s.transform = Affine::translate((ax + 30.0 + i as f64 * 90.0, ay + 40.0));
        s.fill = Some(Paint::Solid(Rgba::from_hex("#ffb000").unwrap()));
        s.blend = m;
        s.name = m.label().into();
        app.doc.add_shape(s);
    }
    // A gradient-masked image-like rect.
    let base = app.doc.add_shape(r(420.0, 20.0, 160.0, 120.0, "#e04040"));
    let mut m = r(420.0, 20.0, 160.0, 120.0, "#ffffff");
    m.fill = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, Rgba::WHITE, Rgba::BLACK)));
    let mid = app.doc.add_shape(m);
    app.selection = vec![base, mid];
    app.make_mask(MaskMode::Luminance);
    // Stacked fills and strokes.
    let mut p = r(600.0, 20.0, 180.0, 120.0, "#ffffff");
    p.fill = Some(Paint::Solid(Rgba::from_hex("#ffe08a").unwrap()));
    p.fills = vec![Paint::Solid(Rgba::from_hex("#2060ff").unwrap().with_alpha(0.3))];
    p.stroke = Some(Stroke { width: 2.0, color: Rgba::BLACK, ..Default::default() });
    p.strokes = vec![Stroke { width: 10.0, color: Rgba::from_hex("#8ad0ff").unwrap(), ..Default::default() }];
    app.doc.add_shape(p);
    // Conic and mesh gradients.
    let mut g = Gradient::new(GradientKind::Linear, Rgba::from_hex("#ff3060").unwrap(), Rgba::from_hex("#30a0ff").unwrap());
    g.set_kind(GradientKind::Conic);
    let mut c = Shape::new(ShapeKind::Ellipse { w: 140.0, h: 140.0 });
    c.transform = Affine::translate((ax + 20.0, ay + 160.0));
    c.fill = Some(Paint::Gradient(g.clone()));
    app.doc.add_shape(c);
    g.set_kind(GradientKind::Mesh);
    if let Some(mesh) = g.mesh.as_mut() {
        let pal = ["#ff6b6b", "#ffd93d", "#6bcB77", "#4d96ff", "#9b5de5", "#f15bb5", "#00bbf9", "#fee440", "#00f5d4"];
        for (i, col) in mesh.colors.iter_mut().enumerate() {
            *col = Rgba::from_hex(pal[i % pal.len()]).unwrap();
        }
    }
    let mut ms = r(180.0, 160.0, 200.0, 140.0, "#000000");
    ms.fill = Some(Paint::Gradient(g));
    app.doc.add_shape(ms);
    // Blend between two shapes.
    let a = app.doc.add_shape(r(420.0, 170.0, 30.0, 30.0, "#1e1e1e"));
    let mut e = Shape::new(ShapeKind::Ellipse { w: 70.0, h: 70.0 });
    e.transform = Affine::translate((ax + 700.0, ay + 160.0));
    e.fill = Some(Paint::Solid(Rgba::from_hex("#ff8a00").unwrap()));
    let b = app.doc.add_shape(e);
    app.selection = vec![a, b];
    app.blend_selection(6);
    // Brush strokes with pressure and a warped rectangle.
    let pts: Vec<Point> = (0..120).map(|i| Point::new(ax + 30.0 + i as f64 * 3.0, ay + 380.0 + (i as f64 * 0.08).sin() * 40.0)).collect();
    let times: Vec<f64> = (0..120).map(|i| i as f64 / 60.0).collect();
    let pressure: Vec<Option<f32>> = (0..120).map(|i| Some(((i as f32 / 119.0) * std::f32::consts::PI).sin())).collect();
    if let Some(s) = crate::freehand::brush_shape(&pts, &times, &pressure, 1.0, &crate::freehand::BrushOpts { width: 18.0, vary: 0.9, ..Default::default() }) {
        app.doc.add_shape(s);
    }
    let w = app.doc.add_shape(r(440.0, 300.0, 300.0, 120.0, "#4d96ff"));
    app.selection = vec![w];
    app.begin_warp();
    app.ui.warp.style = WarpStyle::Arch;
    app.ui.warp.bend = 0.56;
    app.ui.warp.hdist = -1.0;
    app.ui.warp.vdist = 0.45;
    app.end_warp(true);
    app.selection.clear();
    crate::io::save_native(&app.doc, std::path::Path::new("/tmp/mtzd-paint.mvec")).unwrap();
}

#[test]
fn mesh_raster_slicing_and_folds() {
    use crate::document::MeshGrid;
    let red = Rgba::from_hex("#ff0000").unwrap();
    let blue = Rgba::from_hex("#0000ff").unwrap();
    let mut m = MeshGrid::new(3, 3, |u, _| red.lerp(blue, u as f32));
    // Corners keep their colours; the middle is between.
    let near = |a: Rgba, b: Rgba| (a.r - b.r).abs() < 0.03 && (a.b - b.b).abs() < 0.03;
    assert!(near(m.color_at([0.0, 0.5]), red) && near(m.color_at([1.0, 0.5]), blue));
    // Slicing keeps the look.
    let before: Vec<Rgba> = (0..=10).map(|i| m.color_at([i as f64 / 10.0, 0.3])).collect();
    assert!(m.insert_column(0.5) && m.insert_row(1.25));
    assert_eq!((m.cols, m.rows), (4, 4));
    assert_eq!(m.points.len(), 16);
    let after: Vec<Rgba> = (0..=10).map(|i| m.color_at([i as f64 / 10.0, 0.3])).collect();
    for (a, b) in before.iter().zip(&after) {
        assert!(near(*a, *b), "{a:?} {b:?}");
    }
    assert!(!m.insert_column(1.0), "on an existing column");
    // A boundary point dragged inwards: the uncovered corner takes nearby
    // colours (no hard-edged wedge), and every pixel is painted.
    let mut f = MeshGrid::new(3, 3, |u, _| red.lerp(blue, u as f32));
    f.points[2] = [0.7, 0.25];
    let img = f.raster(64);
    assert!(img.iter().all(|c| c.a > 0.99));
    let corner = f.color_at([0.98, 0.02]);
    assert!(corner.b > 0.5, "{corner:?}");
    // Grid lookup.
    let (gs, gt) = m.grid_at([0.5, 0.5]).unwrap();
    assert!(gs > 0.0 && gs < 3.0 && gt > 0.0 && gt < 3.0);
}

/// /tmp/mtzd-mesh.mvec: a rectangle with a 3×3 mesh whose top-right edge
/// point is pulled in (for screenshots).
#[test]
#[ignore]
fn write_mesh_document() {
    let mut app = App::new(egui::Context::default(), None);
    let (ax, ay) = (app.doc.artboards[0].x, app.doc.artboards[0].y);
    let mut s = Shape::new(ShapeKind::Rect { w: 700.0, h: 420.0, radii: [0.0; 4] });
    s.transform = kurbo::Affine::translate((ax + 100.0, ay + 100.0));
    s.stroke = None;
    let mut g = Gradient::new(GradientKind::Linear, Rgba::from_hex("#d00000").unwrap(), Rgba::from_hex("#0010c0").unwrap());
    g.set_kind(GradientKind::Mesh);
    if let Some(m) = g.mesh.as_mut() {
        m.points[2] = [0.85, 0.2];
    }
    s.fill = Some(Paint::Gradient(g));
    app.doc.add_shape(s);
    crate::io::save_native(&app.doc, std::path::Path::new("/tmp/mtzd-mesh.mvec")).unwrap();
}

/// Demo for headless screenshots: /tmp/mtzd-trace.mvec, the PNG in
/// MTZD_TRACE_PNG as an image to trace.
#[test]
#[ignore]
fn write_trace_document() {
    let Ok(path) = std::env::var("MTZD_TRACE_PNG") else { return };
    let mut app = App::new(egui::Context::default(), None);
    let png = std::fs::read(path).unwrap();
    let iid = app.doc.add_image(crate::document::ImageAsset::from_encoded(png).unwrap());
    let (ax, ay) = (app.doc.artboards[0].x, app.doc.artboards[0].y);
    let mut s = rect(ax + 40.0, ay + 40.0, 512.0, 512.0, "#000000");
    s.fill = Some(Paint::Image(crate::document::ImageFill { image: iid, mode: crate::document::ImageMode::Stretch, scale: 1.0, opacity: 1.0 }));
    app.doc.add_shape(s);
    crate::io::save_native(&app.doc, std::path::Path::new("/tmp/mtzd-trace.mvec")).unwrap();
}

#[test]
fn width_profile_applies_along_a_straight_line() {
    // A straight segment flattens to its two ends; the profile's wide
    // middle must still show.
    let mut path = kurbo::BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((400.0, 0.0));
    let profile = vec![crate::document::WidthPoint { t: 0.0, w: 0.1 }, crate::document::WidthPoint { t: 0.5, w: 1.0 }, crate::document::WidthPoint { t: 1.0, w: 0.1 }];
    let st = Stroke { width: 20.0, profile, ..Default::default() };
    let b = crate::geom::path_bounds(&crate::strokes::variable_outline(&path, &st)).unwrap();
    assert!(b.height() > 19.0, "{b:?}");
}
