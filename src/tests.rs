use kurbo::{Affine, Shape as _};

use crate::boolean;
use crate::document::*;
use crate::fonts::FontManager;
use crate::{geom, io, render};

fn sample_doc() -> Document {
    let mut doc = Document::default();
    doc.set_size(400.0, 300.0);
    let mut r = Shape::new(ShapeKind::Rect { w: 100.0, h: 80.0, radii: [10.0; 4] });
    r.transform = Affine::translate((20.0, 30.0));
    r.stroke = Some(Stroke { width: 3.0, ..Default::default() });
    doc.add_shape(r);
    let mut e = Shape::new(ShapeKind::Ellipse { w: 120.0, h: 60.0 });
    e.transform = Affine::translate((150.0, 50.0));
    e.fill = Some(Paint::Solid(Rgba::from_hex("#FF4D4F").unwrap().with_alpha(0.5)));
    doc.add_shape(e);
    let mut s = Shape::new(ShapeKind::Star { w: 100.0, h: 100.0, points: 5, inner: 0.5, corner: 4.0 });
    s.transform = Affine::translate((250.0, 150.0));
    doc.add_shape(s);
    // Bitmap fill.
    let mut png = Vec::new();
    image::RgbaImage::from_fn(4, 4, |x, y| image::Rgba([(x * 60) as u8, (y * 60) as u8, 128, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let img = doc.add_image(ImageAsset::from_encoded(png).unwrap());
    let mut b = Shape::new(ShapeKind::Rect { w: 80.0, h: 80.0, radii: [0.0; 4] });
    b.transform = Affine::translate((20.0, 180.0));
    b.fill = Some(Paint::Image(ImageFill { image: img, mode: ImageMode::Fill, scale: 1.0, opacity: 1.0 }));
    doc.add_shape(b);
    doc
}

#[test]
fn primitive_geometry_and_handle_roundtrip() {
    use crate::prims::{self, PrimKind};
    for k in PrimKind::ALL {
        let mut variants = vec![k.default_prim()];
        if k == PrimKind::Qr {
            let path = prims::path(&k.default_prim(), 100.0, 100.0);
            assert!(path.area().abs() > 100.0);
            continue;
        }
        // Extremes of every parameter.
        for (i, spec) in k.params().iter().enumerate() {
            for v in [spec.min, spec.max] {
                let mut p = k.default_prim();
                p.p[i] = v;
                variants.push(p);
            }
        }
        for prim in variants {
            let path = prims::path(&prim, 200.0, 100.0);
            let b = path.bounding_box();
            assert!(b.x0.is_finite() && b.y1.is_finite(), "{k:?} {prim:?}");
            assert!(b.x0 > -1.0 && b.y0 > -1.0 && b.x1 < 201.0 && b.y1 < 101.0, "{k:?} {:?} {b:?}", prim.p);
            if !k.is_open() && !(k == PrimKind::Crescent && prim.p[1] >= 1.0 && prim.p[0] < 0.01) {
                assert!(path.area().abs() > 1.0, "{k:?} {:?} has area", prim.p);
            }
        }
        // Handles land back on the same parameters.
        let prim = k.default_prim();
        let hs = prims::handles(&prim, 200.0, 100.0, 5.0);
        assert!(!hs.is_empty(), "{k:?} has a handle");
        for (i, h) in hs.iter().enumerate() {
            let mut q = prim.clone();
            prims::drag_handle(&mut q, i, *h, 200.0, 100.0, 5.0);
            for j in 0..4 {
                assert!((q.p[j] - prim.p[j]).abs() < 1e-6, "{k:?} handle {i} param {j}: {} vs {}", q.p[j], prim.p[j]);
            }
        }
    }
    // Holes are cut out under the non-zero rule.
    let donut = prims::path(&PrimKind::Donut.default_prim(), 100.0, 100.0);
    assert_eq!(donut.winding(kurbo::Point::new(50.0, 50.0)), 0);
    assert_ne!(donut.winding(kurbo::Point::new(10.0, 50.0)), 0);
    let cog = prims::path(&PrimKind::Cog.default_prim(), 100.0, 100.0);
    assert_eq!(cog.winding(kurbo::Point::new(50.0, 50.0)), 0);
    // Crescent: left edge filled, the bite on the right empty.
    let cr = prims::path(&PrimKind::Crescent.default_prim(), 100.0, 100.0);
    assert_ne!(cr.winding(kurbo::Point::new(5.0, 50.0)), 0);
    assert_eq!(cr.winding(kurbo::Point::new(70.0, 50.0)), 0);
    // Primitives scale by size and survive a native save.
    let mut s = Shape::new(ShapeKind::Prim { w: 50.0, h: 50.0, prim: PrimKind::Cog.default_prim() });
    s.apply_transform(Affine::scale(2.0));
    assert_eq!(s.kind.size(), Some((100.0, 100.0)));
    let json = serde_json::to_string(&s).unwrap();
    let back: Shape = serde_json::from_str(&json).unwrap();
    assert_eq!(back.kind, s.kind);
}

#[test]
fn rounded_rect_geometry() {
    let p = geom::local_path(&ShapeKind::Rect { w: 100.0, h: 50.0, radii: [10.0; 4] });
    let b = p.bounding_box();
    assert!((b.width() - 100.0).abs() < 1e-6 && (b.height() - 50.0).abs() < 1e-6, "{b:?}");
    // 4 lines + 4 arcs.
    let curves = p.elements().iter().filter(|e| matches!(e, kurbo::PathEl::CurveTo(..))).count();
    assert_eq!(curves, 4);
    // Area should be close to w*h - (4 - pi) r^2.
    let expected = 100.0 * 50.0 - (4.0 - std::f64::consts::PI) * 100.0;
    assert!((p.area().abs() - expected).abs() < 1.0, "area {}", p.area());
}

#[test]
fn rounded_star_is_closed_and_bounded() {
    let p = geom::local_path(&ShapeKind::Star { w: 100.0, h: 100.0, points: 5, inner: 0.5, corner: 5.0 });
    let b = p.bounding_box();
    assert!(b.width() < 100.0 && b.width() > 80.0, "{b:?}");
    assert!(matches!(p.elements().last(), Some(kurbo::PathEl::ClosePath)));
}

#[test]
fn bez_roundtrip_ellipse() {
    let pd = geom::ellipse_nodes(100.0, 50.0);
    let bez = geom::pathdata_to_bez(&pd);
    let back = geom::bez_to_pathdata(&bez);
    assert_eq!(back.subpaths.len(), 1);
    assert_eq!(back.subpaths[0].nodes.len(), 4);
    assert!(back.subpaths[0].closed);
    assert!(back.subpaths[0].nodes.iter().all(|n| n.smooth));
}

#[test]
fn shape_builder_regions() {
    let a = geom::polys_to_bez(&[vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]]);
    let b = geom::polys_to_bez(&[vec![[5.0, 5.0], [15.0, 5.0], [15.0, 15.0], [5.0, 15.0]]]);
    let pa = boolean::path_to_polys(&a, FillRule::NonZero);
    let pb = boolean::path_to_polys(&b, FillRule::NonZero);
    let regions = boolean::arrangement(&[pa.clone(), pb.clone()]);
    assert_eq!(regions.len(), 3);
    let total: f64 = regions.iter().map(|r| boolean::area(&r.polys)).sum();
    assert!((total - 175.0).abs() < 0.5, "total {total}");
    let inter = regions.iter().find(|r| (boolean::area(&r.polys) - 25.0).abs() < 0.5).unwrap();
    assert_eq!(inter.owner, 1);
    let u = boolean::op(&pa, &pb, boolean::BoolOp::Union);
    assert!((boolean::area(&u) - 175.0).abs() < 0.5);
    let smooth = geom::polys_to_pathdata_smooth(&u);
    assert!(smooth.node_count() >= 8);
}

#[test]
fn shape_builder_circles_smooth() {
    let c1 = geom::local_path(&ShapeKind::Ellipse { w: 100.0, h: 100.0 });
    let c2 = Affine::translate((50.0, 0.0)) * c1.clone();
    let p1 = boolean::path_to_polys(&c1, FillRule::NonZero);
    let p2 = boolean::path_to_polys(&c2, FillRule::NonZero);
    let regions = boolean::arrangement(&[p1.clone(), p2.clone()]);
    assert_eq!(regions.len(), 3);
    let u = boolean::op(&p1, &p2, boolean::BoolOp::Union);
    let pd = geom::polys_to_pathdata_smooth(&u);
    // Curve fitting should give a compact editable path, not hundreds of nodes.
    assert!(pd.node_count() < 40, "nodes: {}", pd.node_count());
    let fitted = geom::pathdata_to_bez(&pd);
    let (a1, a2) = (boolean::area(&u), fitted.area().abs());
    assert!((a1 - a2).abs() / a1 < 0.005, "area {a1} vs {a2}, nodes {}", pd.node_count());
    eprintln!("union of circles: {} nodes", pd.node_count());
}

fn fonts() -> FontManager {
    FontManager::new(egui::Context::default())
}

#[test]
fn text_layout_has_outlines() {
    let mut f = fonts();
    let td = TextData { content: "Hello\nWorld".into(), family: f.families[0].clone(), ..Default::default() };
    let l = f.layout(&td);
    assert!(!l.path.elements().is_empty());
    assert!(l.carets.len() >= 12);
    assert!(l.bounds.height() > td.size * 2.0);
}

#[test]
fn text_case_transforms() {
    use crate::document::TextCase;
    let mut f = fonts();
    let fam = f.families[0].clone();
    let lay = |f: &mut FontManager, content: &str, case: TextCase| f.layout(&TextData { content: content.into(), family: fam.clone(), case, ..Default::default() });
    let bounds = |l: &crate::fonts::TextLayout| crate::geom::path_bounds(&l.path).unwrap();
    // UPPERCASE draws like typed capitals; carets keep the typed offsets.
    let up = lay(&mut f, "Hello world", TextCase::Upper);
    let typed = lay(&mut f, "HELLO WORLD", TextCase::AsTyped);
    assert!((bounds(&up).width() - bounds(&typed).width()).abs() < 1e-6);
    assert_eq!(up.carets.last().unwrap().index, "Hello world".len());
    // One character can become two (ß → SS): both glyphs map back to it.
    let ss = lay(&mut f, "aß", TextCase::Upper);
    assert_eq!(ss.carets.iter().map(|c| c.index).collect::<Vec<_>>(), vec![0, 1, 3]);
    assert!(bounds(&ss).width() > bounds(&lay(&mut f, "AS", TextCase::AsTyped)).width());
    // lowercase and Title Case.
    let low = lay(&mut f, "HELLO", TextCase::Lower);
    assert!((bounds(&low).width() - bounds(&lay(&mut f, "hello", TextCase::AsTyped)).width()).abs() < 1e-6);
    let title = lay(&mut f, "hello big world", TextCase::Title);
    assert!((bounds(&title).width() - bounds(&lay(&mut f, "Hello Big World", TextCase::AsTyped)).width()).abs() < 1e-6);
    // Small caps: lowercase letters as capitals no taller than the x-height region.
    let caps = bounds(&lay(&mut f, "H", TextCase::AsTyped));
    let sc = bounds(&lay(&mut f, "x", TextCase::SmallCaps));
    let x = bounds(&lay(&mut f, "X", TextCase::AsTyped));
    assert!(sc.height() < x.height() * 0.9 && sc.height() > caps.height() * 0.5, "{sc:?} {x:?}");
    // Baselines line up and capitals stay full size.
    assert!((sc.y1 - x.y1).abs() < 0.5);
    let mixed = bounds(&lay(&mut f, "Hx", TextCase::SmallCaps));
    assert!((mixed.height() - caps.height()).abs() < 0.5);
    // As typed is not written to files.
    let json = serde_json::to_string(&TextData::default()).unwrap();
    assert!(!json.contains("case"));
}

#[test]
fn svg_roundtrip() {
    let mut f = fonts();
    let doc = sample_doc();
    let svg = io::svg::export(&doc, &mut f);
    assert!(svg.contains("<path") && svg.contains("data:image/png;base64"));
    let back = io::svg::import(svg.as_bytes(), &mut f).unwrap();
    // background rect + 4 shapes (+ image as separate rect), exact count may vary
    assert!(back.shapes().len() >= 4, "{}", back.shapes().len());
    assert_eq!(back.primary().w, 400.0);
    assert!(!back.images.is_empty());
}

#[test]
fn pdf_roundtrip() {
    let mut f = fonts();
    let doc = sample_doc();
    // Points are scaled to pixels at 96 DPI: pixel sizes round-trip.
    let bytes = io::pdf::export(&doc, &mut f);
    let path = std::env::temp_dir().join("mtzd-test.pdf");
    std::fs::write(&path, &bytes).unwrap();
    let back = io::pdf::import(&path).unwrap();
    assert!((back.primary().w - 400.0).abs() < 1e-9, "{}", back.primary().w);
    assert_eq!(back.dpi, 96.0);
    assert!(back.shapes().len() >= 4, "{}", back.shapes().len());
    // The rounded rect should come back in the same place (y-flip handled).
    let mut cache = render::GeoCache::default();
    let found = back.shapes().iter().any(|s| {
        let b = cache.update(s, &mut f).bounds.unwrap();
        (b.x0 - 20.0).abs() < 0.5 && (b.y0 - 30.0).abs() < 0.5 && (b.width() - 100.0).abs() < 0.5
    });
    assert!(found);
}

#[test]
fn eps_roundtrip() {
    let mut f = fonts();
    let doc = sample_doc();
    let eps = io::eps::export(&doc, &mut f);
    let path = std::env::temp_dir().join("mtzd-test.eps");
    std::fs::write(&path, eps).unwrap();
    if std::process::Command::new("gs").arg("--version").output().is_err() {
        return;
    }
    let back = io::eps::import(&path).unwrap();
    assert!(back.shapes().len() >= 3, "{}", back.shapes().len());
}

#[test]
fn native_roundtrip() {
    let doc = sample_doc();
    let path = std::env::temp_dir().join("mtzd-test.mvec");
    io::save_native(&doc, &path).unwrap();
    let back = io::load_native(&path).unwrap();
    assert_eq!(back.shapes().len(), doc.shapes().len());
    assert!(back.images.values().all(|i| i.rgba.is_some()));
}

#[test]
fn undo_redo() {
    let mut doc = Document::default();
    let mut h = History::default();
    h.checkpoint(&doc, "", 0.0);
    doc.add_shape(Shape::new(ShapeKind::Ellipse { w: 1.0, h: 1.0 }));
    assert!(h.undo(&mut doc));
    assert!(doc.shapes().is_empty());
    assert!(h.redo(&mut doc));
    assert_eq!(doc.shapes().len(), 1);
}


#[test]
#[ignore]
fn write_demo() {
    let mut doc = sample_doc();
    doc.set_size(800.0, 500.0);
    let mut t = Shape::new(ShapeKind::Text(TextData { content: "Hello, vectors".into(), size: 48.0, weight: 700, ..Default::default() }));
    t.transform = Affine::translate((380.0, 60.0));
    t.fill = Some(Paint::Solid(Rgba::from_hex("#1E1E1E").unwrap()));
    doc.add_shape(t);
    let mut p = Shape::new(ShapeKind::Polygon { w: 140.0, h: 140.0, sides: 6, corner: 12.0 });
    p.transform = Affine::translate((420.0, 260.0));
    p.fill = Some(Paint::Solid(Rgba::from_hex("#0D99FF").unwrap()));
    doc.add_shape(p);
    io::save_native(&doc, std::path::Path::new("/tmp/mtzd-demo.mvec")).unwrap();

    // Demo 3: stroke options, shadows, frame text, rotation.
    let mut d = Document::default();
    d.set_size(900.0, 560.0);
    d.unit = crate::units::Unit::Mm;
    let mut card = Shape::new(ShapeKind::Rect { w: 220.0, h: 140.0, radii: [16.0; 4] });
    card.transform = Affine::translate((60.0, 60.0));
    card.fill = Some(Paint::Solid(Rgba::from_hex("#FFFFFF").unwrap()));
    card.shadow = Some(Shadow { color: Rgba::BLACK.with_alpha(0.35), dx: 6.0, dy: 10.0, blur: 24.0 });
    card.name = "Card".into();
    d.add_shape(card);
    let mut dashed = Shape::new(ShapeKind::Ellipse { w: 160.0, h: 160.0 });
    dashed.transform = Affine::translate((340.0, 50.0));
    dashed.fill = Some(Paint::Solid(Rgba::from_hex("#FFE7A3").unwrap()));
    dashed.stroke = Some(Stroke { width: 6.0, color: Rgba::from_hex("#D97706").unwrap(), dash: vec![3.0, 2.0], cap: LineCap::Round, ..Default::default() });
    d.add_shape(dashed);
    let mut outside = Shape::new(ShapeKind::Rect { w: 140.0, h: 140.0, radii: [0.0; 4] });
    outside.transform = Affine::translate((580.0, 60.0));
    outside.fill = Some(Paint::Solid(Rgba::from_hex("#0D99FF").unwrap()));
    outside.stroke = Some(Stroke { width: 12.0, color: Rgba::from_hex("#1E3A8A").unwrap(), align: StrokeAlign::Outside, join: LineJoin::Round, ..Default::default() });
    outside.name = "Outside stroke".into();
    d.add_shape(outside);
    let mut arrow = PathData::default();
    arrow.subpaths.push(SubPath {
        nodes: vec![
            Node { p: kurbo::Point::new(80.0, 320.0), hin: None, hout: Some(kurbo::Point::new(180.0, 240.0)), smooth: false, corner: 0.0 },
            Node { p: kurbo::Point::new(360.0, 330.0), hin: Some(kurbo::Point::new(260.0, 420.0)), hout: None, smooth: false, corner: 0.0 },
        ],
        closed: false,
    });
    let mut a = Shape::new(ShapeKind::Path(arrow));
    a.fill = None;
    a.stroke = Some(Stroke { width: 4.0, color: Rgba::from_hex("#DC2626").unwrap(), start_arrow: Arrowhead::Circle, end_arrow: Arrowhead::Triangle, dash: vec![4.0, 2.0], ..Default::default() });
    a.name = "Arrow".into();
    d.add_shape(a);
    let mut star = Shape::new(ShapeKind::Star { w: 150.0, h: 150.0, points: 5, inner: 0.5, corner: 6.0 });
    star.apply_transform(Affine::translate((440.0, 280.0)));
    star.apply_transform(Affine::rotate_about(0.35, kurbo::Point::new(515.0, 355.0)));
    star.fill = Some(Paint::Solid(Rgba::from_hex("#A855F7").unwrap()));
    star.shadow = Some(Shadow { color: Rgba::from_hex("#581C87").unwrap().with_alpha(0.5), dx: 0.0, dy: 8.0, blur: 14.0 });
    star.name = "Rotated star".into();
    d.add_shape(star);
    let mut ft = Shape::new(ShapeKind::Text(TextData {
        content: "Frame text wraps inside its box, while artistic text grows with its content.".into(),
        size: 20.0,
        frame: Some([230.0, 120.0]),
        ..Default::default()
    }));
    ft.transform = Affine::translate((640.0, 300.0));
    ft.fill = Some(Paint::Solid(Rgba::from_hex("#1E1E1E").unwrap()));
    ft.name = "Frame Text".into();
    d.add_shape(ft);
    io::save_native(&d, std::path::Path::new("/tmp/mtzd-demo3.mvec")).unwrap();

    // Second demo: overlapping shapes + a curvy path.
    let mut d2 = Document::default();
    d2.set_size(800.0, 500.0);
    for (i, c) in ["#FF4D4F", "#0D99FF", "#FFD666"].iter().enumerate() {
        let mut e = Shape::new(ShapeKind::Ellipse { w: 220.0, h: 220.0 });
        let (x, y) = [(120.0, 100.0), (250.0, 100.0), (185.0, 210.0)][i];
        e.transform = Affine::translate((x, y));
        e.fill = Some(Paint::Solid(Rgba::from_hex(c).unwrap().with_alpha(0.85)));
        d2.add_shape(e);
    }
    let mut pd = geom::ellipse_nodes(200.0, 140.0);
    pd.transform(Affine::translate((540.0, 160.0)));
    pd.subpaths[0].nodes[1].p.x += 40.0;
    let mut path = Shape::new(ShapeKind::Path(pd));
    path.fill = Some(Paint::Solid(Rgba::from_hex("#73D13D").unwrap()));
    path.stroke = Some(Stroke { width: 4.0, color: Rgba::from_hex("#14532D").unwrap(), join: LineJoin::Round, cap: LineCap::Round, ..Default::default() });
    d2.add_shape(path);
    io::save_native(&d2, std::path::Path::new("/tmp/mtzd-demo2.mvec")).unwrap();
}

#[test]
fn stroke_options_geometry() {
    use crate::strokes::{self, StrokePart};
    let rect = geom::local_path(&ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
    // Dashes split the outline into many open pieces.
    let st = Stroke { width: 2.0, dash: vec![4.0, 2.0], ..Default::default() };
    let parts = strokes::stroke_parts(&rect, &st, FillRule::NonZero);
    let StrokePart::Line(l) = &parts[0] else { panic!() };
    let moves = l.elements().iter().filter(|e| matches!(e, kurbo::PathEl::MoveTo(_))).count();
    assert!(moves > 20, "{moves}");
    // Inside alignment stays within the shape; outside stays outside.
    for (align, inside) in [(StrokeAlign::Inside, true), (StrokeAlign::Outside, false)] {
        let st = Stroke { width: 10.0, align, ..Default::default() };
        let area = strokes::stroke_area(&rect, &st, FillRule::NonZero);
        let b = boolean::bounds(&area).unwrap();
        if inside {
            assert!(b.x0 >= -1e-6 && b.x1 <= 100.0 + 1e-6, "{b:?}");
            assert!((boolean::area(&area) - (100.0 * 100.0 - 80.0 * 80.0)).abs() < 2.0);
        } else {
            assert!((b.x0 + 10.0).abs() < 0.5 && (b.x1 - 110.0).abs() < 0.5, "{b:?}");
        }
    }
    // Arrowheads on an open line extend past its end.
    let mut line = kurbo::BezPath::new();
    line.move_to((0.0, 0.0));
    line.line_to((100.0, 0.0));
    let st = Stroke { width: 2.0, end_arrow: Arrowhead::Triangle, ..Default::default() };
    let parts = strokes::stroke_parts(&line, &st, FillRule::NonZero);
    assert_eq!(parts.len(), 2);
    let StrokePart::Fill(head, _) = &parts[1] else { panic!() };
    assert!(head.bounding_box().x1 > 104.0);
    assert!(!strokes::is_plain(&st, &line));
}

#[test]
fn svg_exports_dashes_shadow_and_units() {
    let mut f = fonts();
    let mut doc = sample_doc();
    doc.unit = crate::units::Unit::Mm;
    let id = doc.shapes()[0].id;
    let mut s = doc.get(id).unwrap().clone();
    s.stroke = Some(Stroke { width: 2.0, dash: vec![3.0, 1.0], ..Default::default() });
    s.shadow = Some(Shadow { color: Rgba::BLACK.with_alpha(0.5), dx: 3.0, dy: 4.0, blur: 10.0 });
    doc.replace(s);
    let svg = io::svg::export(&doc, &mut f);
    assert!(svg.contains(r#"stroke-dasharray="6 2""#), "{svg}");
    assert!(svg.contains("feDropShadow") && svg.contains(r#"stdDeviation="5""#), "{svg}");
    assert!(svg.contains(r#"width="105.833mm""#), "{}", &svg[..200]);
    // Shadow and dashes survive a re-import.
    let back = io::svg::import(svg.as_bytes(), &mut f).unwrap();
    let sh = back.shapes().iter().find_map(|s| s.shadow.clone()).expect("shadow imported");
    assert!((sh.blur - 10.0).abs() < 1e-3 && (sh.dx - 3.0).abs() < 1e-3);
    assert!(back.shapes().iter().any(|s| s.stroke.as_ref().map(|st| st.dash.len() == 2 && (st.dash[0] - 3.0).abs() < 1e-3).unwrap_or(false)));
}

#[test]
fn frame_text_wraps_and_resizes_frame() {
    let mut f = fonts();
    let td = TextData { content: "The quick brown fox jumps over the lazy dog".into(), size: 20.0, frame: Some([120.0, 400.0]), ..Default::default() };
    let layout = f.layout(&td);
    let lines = layout.carets.iter().map(|c| c.line).max().unwrap() + 1;
    assert!(lines >= 3, "{lines}");
    assert_eq!(layout.bounds.width(), 120.0);
    if !layout.path.elements().is_empty() {
        assert!(layout.path.bounding_box().x1 <= 121.0, "{:?}", layout.path.bounding_box());
    }
    // A short frame overflows.
    let short = TextData { frame: Some([120.0, 30.0]), ..td.clone() };
    assert!(f.layout(&short).overflow);
    // Scaling frame text resizes the frame and keeps the font size.
    let mut s = Shape::new(ShapeKind::Text(td.clone()));
    s.apply_transform(Affine::scale_non_uniform(2.0, 0.5));
    let ShapeKind::Text(t) = &s.kind else { panic!() };
    assert_eq!(t.frame, Some([240.0, 200.0]));
    assert_eq!(t.size, 20.0);
    // Artistic text scales its glyphs instead.
    let mut a = Shape::new(ShapeKind::Text(TextData { frame: None, ..td }));
    a.apply_transform(Affine::scale(2.0));
    assert!((a.transform.as_coeffs()[0] - 2.0).abs() < 1e-9);
}

#[test]
fn rotated_primitive_keeps_parameters_when_scaled() {
    let mut r = Shape::new(ShapeKind::Rect { w: 100.0, h: 50.0, radii: [0.0; 4] });
    r.apply_transform(Affine::rotate(0.5));
    // Scale along the shape's own axes (what the oriented handles do).
    let frame = Affine::rotate(0.5);
    r.apply_transform(frame * Affine::scale_non_uniform(2.0, 3.0) * frame.inverse());
    let Some((w, h)) = r.kind.size() else { panic!() };
    assert!((w - 200.0).abs() < 1e-6 && (h - 150.0).abs() < 1e-6, "{w} {h}");
    let [a, b, ..] = r.transform.as_coeffs();
    assert!((b.atan2(a) - 0.5).abs() < 1e-9 && ((a * a + b * b).sqrt() - 1.0).abs() < 1e-9);
}

#[test]
fn shadow_blur_preserves_mass() {
    let (w, h) = (64, 64);
    let mut m = vec![0.0f32; w * h];
    for y in 24..40 {
        for x in 24..40 {
            m[y * w + x] = 1.0;
        }
    }
    let before: f32 = m.iter().sum();
    render::gaussian_blur(&mut m, w, h, 3.0);
    let after: f32 = m.iter().sum();
    assert!((before - after).abs() / before < 0.01, "{before} {after}");
    let edge = m[32 * w + 24];
    assert!(edge > 0.3 && edge < 0.75 && m[32 * w + 20] > 0.0, "{edge}");
}

mod app_tests {
    use super::*;
    use crate::app::{App, Tool};
    use crate::boolean::BoolOp;

    fn app_with_squares() -> (App, u64, u64) {
        let mut app = App::new(egui::Context::default(), None);
        let mut a = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
        a.fill = Some(Paint::Solid(Rgba::from_hex("#FF0000").unwrap()));
        let mut b = Shape::new(ShapeKind::Ellipse { w: 100.0, h: 100.0 });
        b.transform = Affine::translate((50.0, 50.0));
        b.fill = Some(Paint::Solid(Rgba::from_hex("#0000FF").unwrap()));
        let ia = app.doc.add_shape(a);
        let ib = app.doc.add_shape(b);
        app.selection = vec![ia, ib];
        (app, ia, ib)
    }

    /// Drive the real UI with synthetic input.
    struct Driver {
        ctx: egui::Context,
        app: App,
        t: f64,
        pos: egui::Pos2,
        mods: egui::Modifiers,
        /// Viewport commands sent during the last frame.
        cmds: Vec<egui::ViewportCommand>,
    }

    impl Driver {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::ui::theme::setup(&ctx);
            let (mut app, _, _) = {
                let mut app = App::new(ctx.clone(), None);
                let mut a = Shape::new(ShapeKind::Rect { w: 300.0, h: 300.0, radii: [0.0; 4] });
                a.transform = Affine::translate((100.0, 100.0));
                let ia = app.doc.add_shape(a);
                (app, ia, 0)
            };
            app.selection.clear();
            let mut d = Driver { ctx, app, t: 0.0, pos: egui::pos2(1.0, 1.0), mods: Default::default(), cmds: vec![] };
            d.frames(12, vec![]);
            d
        }
        fn frame(&mut self, events: Vec<egui::Event>) {
            self.t += 1.0 / 30.0;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
                time: Some(self.t),
                focused: true,
                events,
                ..Default::default()
            };
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| app.frame_ui(ui));
            out.textures_delta.clear();
            self.cmds = out.viewport_output.remove(&egui::ViewportId::ROOT).map(|v| v.commands).unwrap_or_default();
        }
        fn frames(&mut self, n: usize, events: Vec<egui::Event>) {
            self.frame(events);
            for _ in 1..n {
                self.frame(vec![]);
            }
        }
        fn move_to(&mut self, p: egui::Pos2) {
            self.pos = p;
            self.frame(vec![egui::Event::PointerMoved(p)]);
        }
        fn button(&mut self, b: egui::PointerButton, pressed: bool) {
            let e = egui::Event::PointerButton { pos: self.pos, button: b, pressed, modifiers: self.mods };
            self.frame(vec![e]);
        }
        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            self.move_to(from);
            self.button(egui::PointerButton::Primary, true);
            for i in 1..=10 {
                self.move_to(from + (to - from) * (i as f32 / 10.0));
            }
            self.button(egui::PointerButton::Primary, false);
            self.frames(5, vec![]);
        }
    }

    #[test]
    fn title_bar_moves_and_edges_resize_the_window() {
        use egui::ViewportCommand as V;
        let mut d = Driver::new();
        let sent = |d: &Driver, f: &dyn Fn(&V) -> bool| d.cmds.iter().any(f);
        // Dragging empty title bar space moves the window.
        let p = egui::pos2(500.0, 10.0);
        d.move_to(p);
        d.button(egui::PointerButton::Primary, true);
        let mut moved = false;
        for i in 1..6 {
            d.move_to(p + egui::vec2(i as f32 * 4.0, 0.0));
            moved |= sent(&d, &|c| matches!(c, V::StartDrag));
        }
        d.button(egui::PointerButton::Primary, false);
        assert!(moved, "no StartDrag");
        // Pressing the bottom-right corner starts a resize.
        d.move_to(egui::pos2(1597.0, 997.0));
        d.button(egui::PointerButton::Primary, true);
        assert!(sent(&d, &|c| matches!(c, V::BeginResize(egui::ResizeDirection::SouthEast))), "{:?}", d.cmds);
        d.button(egui::PointerButton::Primary, false);
        // Maximize button (second from the right).
        d.move_to(egui::pos2(1600.0 - 63.0, 16.0));
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        assert!(sent(&d, &|c| matches!(c, V::Maximized(true))), "{:?}", d.cmds);
        // Menus still open from the bar.
        d.move_to(egui::pos2(40.0, 16.0));
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        d.frames(2, vec![]);
        assert!(!sent(&d, &|c| matches!(c, V::StartDrag)));
        assert!(egui::Popup::is_any_open(&d.ctx), "File menu did not open");
    }

    #[test]
    fn artboard_tool_creates_moves_and_deletes() {
        let mut d = Driver::new();
        d.app.view.zoom = 0.25;
        d.app.view.origin = egui::vec2(200.0, 200.0);
        d.frames(3, vec![]);
        let first = d.app.doc.primary().id;
        let shape = d.app.doc.shapes()[0].id;
        // Drag on empty canvas with the Artboard tool: a new artboard.
        d.app.set_tool(Tool::Artboard);
        d.frames(2, vec![]);
        let a = d.app.view.to_screen(kurbo::Point::new(2400.0, 0.0));
        let b = d.app.view.to_screen(kurbo::Point::new(2800.0, 300.0));
        d.drag(a, b);
        assert_eq!(d.app.doc.artboards.len(), 2);
        let new = d.app.doc.artboards[1].clone();
        assert!((new.w - 400.0).abs() < 8.0 && (new.h - 300.0).abs() < 8.0, "{new:?}");
        assert_eq!(d.app.active_artboard, new.id);
        // Drag the first artboard by its label: the shape on it comes along.
        d.app.set_tool(Tool::Select);
        d.frames(2, vec![]);
        let label = d.app.artboard_labels.iter().find(|(id, _)| *id == first).unwrap().1.center();
        let before = d.app.doc.get(shape).unwrap().transform.translation();
        d.drag(label, label + egui::vec2(0.0, 50.0));
        let moved = d.app.doc.get(shape).unwrap().transform.translation() - before;
        assert!((moved.y - 200.0).abs() < 8.0 && moved.x.abs() < 1e-6, "{moved:?}");
        assert!((d.app.doc.primary().y - 200.0).abs() < 8.0);
        assert!(d.app.artboard_selected);
        // Undo puts both back.
        d.app.undo();
        assert_eq!(d.app.doc.primary().y, 0.0);
        assert_eq!(d.app.doc.get(shape).unwrap().transform.translation(), before);
        // Delete removes the selected artboard but keeps the artwork.
        d.app.activate_artboard(new.id, true);
        crate::commands::Cmd::Delete.run(&mut d.app, &egui::Context::default());
        assert_eq!(d.app.doc.artboards.len(), 1);
        assert!(d.app.doc.get(shape).is_some());
        assert!(!crate::commands::Cmd::DeleteArtboard.enabled(&d.app), "the last artboard stays");
    }

    #[test]
    fn artboard_paste_rehome_and_snap() {
        use crate::platform::clipboard::{self, ClipContent};
        let mut app = App::new(egui::Context::default(), None);
        app.doc.set_size(400.0, 300.0);
        let a = app.doc.primary().id;
        let b = app.doc.add_artboard(kurbo::Rect::new(600.0, 0.0, 1000.0, 300.0), None);
        let sq = |x: f64, y: f64| {
            let mut s = Shape::new(ShapeKind::Rect { w: 20.0, h: 20.0, radii: [0.0; 4] });
            s.transform = Affine::translate((x, y));
            s
        };
        let id = app.doc.add_shape(sq(10.0, 10.0));
        let other = app.doc.add_shape(sq(700.0, 100.0));
        let s = app.doc.get(id).unwrap().clone();
        assert_eq!(app.member_of(&s), Some(a));
        // Copied on A, pasted while B is active: same spot relative to B.
        let mut p = clipboard::build_payload(&app.doc, &[id]);
        p.origin = Some((0.0, 0.0));
        app.activate_artboard(b, false);
        app.paste_content(ClipContent::Internal(p.clone()), None);
        let bb = app.selection_bounds().unwrap();
        assert_eq!((bb.x0, bb.y0), (610.0, 10.0));
        // Copied from outside any artboard with B selected: centred in B.
        p.origin = None;
        app.activate_artboard(b, true);
        app.paste_content(ClipContent::Internal(p), None);
        let c = app.selection_bounds().unwrap().center();
        assert_eq!((c.x, c.y), (800.0, 150.0));
        // Dragging a shape from A into B puts it on top of B's stack.
        app.translate_ids(&[id], kurbo::Vec2::new(600.0, 200.0));
        app.doc.move_to_index(id, 0);
        app.rehome(&[(id, Some(a))]);
        let (zi, zo) = (app.doc.index_of(id).unwrap(), app.doc.index_of(other).unwrap());
        assert!(zi > zo, "moved above B's artwork ({zi} vs {zo})");
        // Out of every artboard: past their right edge.
        assert!(app.move_to_artboard(id, Some(b), None));
        let s = app.doc.get(id).unwrap().clone();
        assert_eq!(app.member_of(&s), None);
        // Into A from B keeps the offset inside the artboard.
        let s = app.doc.get(other).unwrap().clone();
        let from = app.member_of(&s);
        assert!(app.move_to_artboard(other, from, Some(a)));
        assert_eq!(app.doc.get(other).unwrap().transform.translation(), kurbo::Vec2::new(100.0, 100.0));
        // Restacking next to a sibling.
        app.doc.move_next_to(other, id, false);
        assert_eq!(app.doc.index_of(other).unwrap() + 1, app.doc.index_of(id).unwrap());
    }

    #[test]
    fn artboard_drag_snaps_to_other_artboards() {
        let mut d = Driver::new();
        d.app.view.zoom = 0.25;
        d.app.view.origin = egui::vec2(200.0, 200.0);
        let first = d.app.doc.primary().id;
        let b = d.app.doc.add_artboard(kurbo::Rect::new(2200.0, 0.0, 2600.0, 300.0), None);
        d.frames(3, vec![]);
        // Drag B 50 doc px down and a bit: it snaps back level with A's top.
        let label = d.app.artboard_labels.iter().find(|(id, _)| *id == b).unwrap().1.center();
        d.drag(label, label + egui::vec2(20.0, 3.0));
        let ab = d.app.doc.artboard(b).unwrap().clone();
        assert_eq!(ab.y, 0.0, "{ab:?}");
        assert!((ab.x - 2280.0).abs() < 1.0, "{ab:?}");
        // Ctrl turns snapping off.
        d.mods = egui::Modifiers::COMMAND;
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        let label = d.app.artboard_labels.iter().find(|(id, _)| *id == b).unwrap().1.center();
        d.drag(label, label + egui::vec2(0.0, 3.0));
        assert_eq!(d.app.doc.artboard(b).unwrap().y, 12.0);
        // Alt+drag clones the artboard with its artwork; the original stays.
        d.mods = egui::Modifiers::ALT;
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        let shapes = d.app.doc.shapes().len();
        let before = d.app.doc.artboard(first).unwrap().clone();
        let label = d.app.artboard_labels.iter().find(|(id, _)| *id == first).unwrap().1.center();
        d.drag(label, label + egui::vec2(0.0, 150.0));
        d.mods = Default::default();
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        assert_eq!(d.app.doc.artboards.len(), 3);
        assert_eq!(d.app.doc.artboard(first).unwrap(), &before);
        let copy = d.app.doc.artboards[2].clone();
        assert!(copy.name.ends_with("copy") && (copy.y - before.y - 600.0).abs() < 30.0, "{copy:?}");
        assert_eq!(d.app.doc.shapes().len(), shapes * 2);
        assert_eq!(d.app.doc.shapes()[0].transform.translation(), kurbo::Vec2::new(100.0, 100.0));
    }

    #[test]
    fn pen_handles_alt_break_resume_and_backspace() {
        let mut d = Driver::new();
        d.app.view.zoom = 1.0;
        d.app.view.origin = egui::vec2(100.0, 100.0);
        d.app.snap = false;
        d.app.set_tool(Tool::Pen);
        d.frames(2, vec![]);
        let s = |d: &Driver, x: f64, y: f64| d.app.view.to_screen(kurbo::Point::new(x, y));
        let nodes = |d: &Driver| match &d.app.doc.get(d.app.selection[0]).unwrap().kind {
            ShapeKind::Path(pd) => pd.subpaths[0].nodes.clone(),
            _ => panic!("not a path"),
        };
        let click = |d: &mut Driver, p: egui::Pos2| {
            d.move_to(p);
            d.button(egui::PointerButton::Primary, true);
            d.button(egui::PointerButton::Primary, false);
            d.frames(2, vec![]);
        };
        // Drag to place a smooth node: both handles, mirrored.
        d.drag(s(&d, 600.0, 400.0), s(&d, 650.0, 400.0));
        let n = nodes(&d);
        assert_eq!(n[0].hout, Some(kurbo::Point::new(650.0, 400.0)));
        assert_eq!(n[0].hin, Some(kurbo::Point::new(550.0, 400.0)));
        let path = d.app.selection[0];
        // A corner, then Alt+drag: only the outgoing handle moves.
        let q = s(&d, 700.0, 500.0);
        click(&mut d, q);
        d.mods = egui::Modifiers::ALT;
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        d.drag(s(&d, 800.0, 500.0), s(&d, 800.0, 560.0));
        d.mods = Default::default();
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        let n = nodes(&d);
        assert_eq!(n.len(), 3);
        assert_eq!((n[2].hout, n[2].hin, n[2].smooth), (Some(kurbo::Point::new(800.0, 560.0)), None, false));
        // Clicking the last node again drops its handle.
        let q = s(&d, 800.0, 500.0);
        click(&mut d, q);
        assert_eq!(nodes(&d).len(), 3);
        assert_eq!(nodes(&d)[2].hout, None);
        // Backspace takes back the last node.
        crate::commands::Cmd::Delete.run(&mut d.app, &egui::Context::default());
        assert_eq!(nodes(&d).len(), 2);
        // Finish, then click the path's first end with the pen: drawing
        // continues from there (the path is reversed).
        d.app.finish_pen();
        d.frames(2, vec![]);
        let q = s(&d, 600.0, 400.0);
        click(&mut d, q);
        assert_eq!(d.app.pen.as_ref().map(|p| p.id), Some(path));
        assert_eq!(nodes(&d).last().unwrap().p, kurbo::Point::new(600.0, 400.0));
        let q = s(&d, 500.0, 300.0);
        click(&mut d, q);
        assert_eq!(nodes(&d).len(), 3);
        d.app.finish_pen();
        // Node tool, Ctrl+drag from a corner node pulls out mirrored handles.
        d.app.set_tool(Tool::Node);
        d.app.selection = vec![path];
        d.frames(2, vec![]);
        let corner = nodes(&d)[1].p;
        d.mods = egui::Modifiers::COMMAND;
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        d.drag(s(&d, corner.x, corner.y), s(&d, corner.x + 30.0, corner.y));
        let n = nodes(&d)[1].clone();
        assert!(n.smooth && n.hout == Some(corner + kurbo::Vec2::new(30.0, 0.0)) && n.hin == Some(corner - kurbo::Vec2::new(30.0, 0.0)), "{n:?}");
    }

    #[test]
    fn rename_commits_on_click_away_and_shape_tool_click_selects() {
        let mut d = Driver::new();
        let id = d.app.doc.shapes()[0].id;
        d.app.selection = vec![id];
        crate::commands::Cmd::Rename.run(&mut d.app, &egui::Context::default());
        d.frames(3, vec![]);
        assert!(d.app.renaming.is_some());
        assert!(d.ctx.egui_wants_keyboard_input(), "rename field focused");
        // Typing replaces the (selected) name; clicking the canvas commits.
        d.frame(vec![egui::Event::Text("Hero".into())]);
        d.frames(2, vec![]);
        let c = d.app.canvas_rect.center();
        d.move_to(c);
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        d.frames(3, vec![]);
        assert!(d.app.renaming.is_none(), "rename closed");
        assert_eq!(d.app.doc.get(id).unwrap().name, "Hero");
        assert!(!d.ctx.egui_wants_keyboard_input());
        // A click with the rectangle tool selects; only a drag draws.
        d.app.set_tool(Tool::Rect);
        d.app.selection.clear();
        d.frames(2, vec![]);
        let n = d.app.doc.shapes().len();
        let on = d.app.view.to_screen(kurbo::Point::new(250.0, 250.0));
        d.move_to(on);
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        d.frames(2, vec![]);
        assert_eq!(d.app.doc.shapes().len(), n);
        assert_eq!(d.app.selection, vec![id]);
        d.drag(on, on + egui::vec2(60.0, 40.0));
        assert_eq!(d.app.doc.shapes().len(), n + 1);
    }

    #[test]
    fn instances_update_live_in_the_ui_loop() {
        let mut d = Driver::new();
        let rect0 = d.app.doc.shapes()[0].id;
        d.app.selection = vec![rect0];
        d.app.create_component();
        let main = d.app.selection[0];
        let inst = d.app.insert_instance(main, kurbo::Point::new(900.0, 300.0)).unwrap();
        d.frames(3, vec![]);
        let kid = |d: &Driver| d.app.doc.get(inst).unwrap().kind.children().unwrap()[0].clone();
        // Recolour the main's layer from the Design panel path.
        d.app.selection = vec![rect0];
        d.app.apply_color(Rgba::from_hex("#00FF00").unwrap(), false);
        d.frames(1, vec![]);
        assert_eq!(kid(&d).fill, d.app.doc.get(rect0).unwrap().fill, "fill follows");
        // …and what is drawn follows too (the cache sees a new revision).
        let k = kid(&d);
        let cached = d.app.cache.get(k.id).map(|c| c.rev);
        assert_eq!(cached, Some(k.rev), "render cache is current");
        assert_ne!(k.rev, 0);
        // Resize the main's layer on the canvas: the instance follows mid-drag.
        d.app.selection = vec![main];
        d.frames(2, vec![]);
        let b = d.app.selection_bounds().unwrap();
        let from = d.app.view.to_screen(kurbo::Point::new(b.x1, b.y1));
        d.move_to(from);
        d.button(egui::PointerButton::Primary, true);
        for i in 1..=5 {
            d.move_to(from + egui::vec2(10.0 * i as f32, 0.0));
        }
        let w_main = d.app.doc.get(rect0).unwrap().kind.size().unwrap().0;
        assert!(w_main > 300.0, "{w_main}");
        assert_eq!(kid(&d).kind.size().unwrap().0, w_main, "size follows during the drag");
        d.button(egui::PointerButton::Primary, false);
    }

    #[test]
    fn knife_smoothing_alt_handle_and_label_rename() {
        // Smoothing keeps the ends and flattens a zigzag.
        let zig: Vec<kurbo::Point> = (0..40).map(|i| kurbo::Point::new(i as f64 * 5.0, if i % 2 == 0 { 0.0 } else { 6.0 })).collect();
        let sm = crate::geom::smooth_polyline(&zig, 0.6);
        assert_eq!((sm[0], *sm.last().unwrap()), (zig[0], *zig.last().unwrap()));
        let mid = &sm[sm.len() / 4..sm.len() * 3 / 4];
        let (lo, hi) = mid.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
        assert!(hi - lo < 1.5, "zigzag flattened: {}", hi - lo);
        assert_eq!(crate::geom::smooth_polyline(&zig, 0.0), zig);

        let mut d = Driver::new();
        d.app.view.zoom = 1.0;
        d.app.view.origin = egui::vec2(100.0, 100.0);
        d.app.snap = false;
        // A smooth node; Alt pressed in the middle of a handle drag breaks it.
        let mut pd = PathData { subpaths: vec![SubPath { nodes: vec![Node::corner(kurbo::Point::new(500.0, 500.0)), Node::corner(kurbo::Point::new(700.0, 500.0))], closed: false }] };
        pd.subpaths[0].nodes[0].hout = Some(kurbo::Point::new(550.0, 450.0));
        pd.subpaths[0].nodes[0].hin = Some(kurbo::Point::new(450.0, 550.0));
        pd.subpaths[0].nodes[0].smooth = true;
        let mut sh = Shape::new(ShapeKind::Path(pd));
        sh.fill = None;
        let id = d.app.doc.add_shape(sh);
        d.app.selection = vec![id];
        d.app.set_tool(Tool::Node);
        d.app.node_sel = vec![(id, 0, 0)];
        d.frames(2, vec![]);
        let s = |d: &Driver, x: f64, y: f64| d.app.view.to_screen(kurbo::Point::new(x, y));
        let a = s(&d, 550.0, 450.0);
        d.move_to(a);
        d.button(egui::PointerButton::Primary, true);
        for i in 1..=4 {
            d.move_to(a + egui::vec2(0.0, -5.0 * i as f32));
        }
        d.mods = egui::Modifiers::ALT;
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        let b = s(&d, 600.0, 400.0);
        d.move_to(b);
        d.mods = Default::default();
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        d.move_to(b + egui::vec2(1.0, 0.0));
        d.button(egui::PointerButton::Primary, false);
        let n = match &d.app.doc.get(id).unwrap().kind { ShapeKind::Path(pd) => pd.subpaths[0].nodes[0].clone(), _ => unreachable!() };
        assert!(!n.smooth, "turned sharp");
        assert_eq!(n.hin, Some(kurbo::Point::new(450.0, 550.0)), "other handle untouched once Alt broke it");

        // Double-clicking an artboard's label renames it in place.
        d.app.set_tool(Tool::Select);
        d.app.view.origin = egui::vec2(300.0, 300.0);
        d.frames(2, vec![]);
        let ab = d.app.doc.primary().id;
        let label = d.app.artboard_labels.iter().find(|(i, _)| *i == ab).unwrap().1.center();
        d.move_to(label);
        for _ in 0..2 {
            d.button(egui::PointerButton::Primary, true);
            d.button(egui::PointerButton::Primary, false);
        }
        d.frames(2, vec![]);
        assert!(d.app.rename_on_canvas && d.app.renaming.as_ref().map(|r| r.0) == Some(ab));
        d.frame(vec![egui::Event::Text("Cover".into())]);
        d.frame(vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        d.frames(2, vec![]);
        assert_eq!(d.app.doc.primary().name, "Cover");
        assert!(d.app.renaming.is_none());
    }

    #[test]
    fn studio_dock_resize_sticks() {
        let mut d = Driver::new();
        let edge = d.app.canvas_rect.right();
        let y = 500.0;
        // Widen the studio by dragging its left edge 120 px to the left.
        d.drag(egui::pos2(edge + 1.0, y), egui::pos2(edge - 119.0, y));
        d.frames(10, vec![]);
        let after = d.app.canvas_rect.right();
        assert!((edge - after - 120.0).abs() < 4.0, "edge {edge} -> {after}");
        // Resize the first tab group with its splitter.
        let h0 = d.app.ui.group_h[0];
        let top = d.app.canvas_rect.top() - crate::ui::theme::RULER;
        let split_y = top + crate::ui::theme::TAB_H + h0;
        let x = after + 120.0;
        d.drag(egui::pos2(x, split_y), egui::pos2(x, split_y + 60.0));
        d.frames(5, vec![]);
        assert!((d.app.ui.group_h[0] - h0 - 60.0).abs() < 6.0, "{} -> {}", h0, d.app.ui.group_h[0]);
    }

    #[test]
    fn design_panel_renders_every_kind_and_tool() {
        use crate::app::Tool;
        use crate::document::{Shadow, TextData};
        let mut d = Driver::new();
        let add = |d: &mut Driver, k: ShapeKind| {
            let mut s = Shape::new(k);
            s.fill = Some(Paint::Solid(Rgba::BLACK));
            s.stroke = Some(Default::default());
            s.shadow = Some(Shadow::default());
            s.blur = 2.0;
            s.noise = 0.2;
            s.outlines = vec![crate::document::OutlineStep::new(Rgba::WHITE, 2.0)];
            d.app.doc.add_shape(s)
        };
        let ids = vec![
            d.app.doc.shapes()[0].id,
            add(&mut d, ShapeKind::Ellipse { w: 50.0, h: 50.0 }),
            add(&mut d, ShapeKind::Star { w: 50.0, h: 50.0, points: 5, inner: 0.5, corner: 0.0 }),
            add(&mut d, ShapeKind::Polygon { w: 50.0, h: 50.0, sides: 6, corner: 0.0 }),
            add(&mut d, ShapeKind::Path(geom::ellipse_nodes(30.0, 30.0))),
            add(&mut d, ShapeKind::Text(TextData { content: "Hi".into(), ..Default::default() })),
            add(&mut d, ShapeKind::Prim { w: 50.0, h: 50.0, prim: crate::prims::PrimKind::Arrow.default_prim() }),
            add(&mut d, ShapeKind::Prim { w: 50.0, h: 50.0, prim: crate::prims::PrimKind::Cog.default_prim() }),
        ];
        d.app.ui.tool_flyout = Some(crate::ui::tools_panel::Group::Shapes);
        let tools = [Tool::Select, Tool::Node, Tool::Star, Tool::Polygon, Tool::Text, Tool::Corner, Tool::ShapeBuilder, Tool::Pen, Tool::Knife, Tool::Fill, Tool::Prim(crate::prims::PrimKind::Cog)];
        let mut sels: Vec<Vec<u64>> = ids.iter().map(|i| vec![*i]).collect();
        sels.push(vec![]);
        sels.push(ids.clone());
        for t in tools {
            d.app.set_tool(t);
            for sel in &sels {
                d.app.selection = sel.clone();
                for key in [None, Some("fill"), Some("stroke")] {
                    d.app.ui.paint_open = key.map(String::from);
                    d.frames(2, vec![]);
                }
            }
        }
    }

    #[test]
    fn primitives_create_and_reshape_with_handles() {
        use crate::prims::PrimKind;
        let mut d = Driver::new();
        d.app.view.zoom = 1.0;
        d.app.view.origin = egui::vec2(100.0, 100.0);
        d.app.snap = false;
        d.app.set_tool(Tool::Prim(PrimKind::Triangle));
        let at = |d: &Driver, x: f64, y: f64| d.app.view.to_screen(kurbo::Point::new(x, y));
        let (a, b) = (at(&d, 600.0, 500.0), at(&d, 800.0, 700.0));
        d.drag(a, b);
        let id = *d.app.selection.first().expect("created");
        let s = d.app.doc.get(id).unwrap();
        assert!(matches!(s.kind, ShapeKind::Prim { ref prim, .. } if prim.kind == PrimKind::Triangle), "{:?}", s.kind);
        assert_eq!(s.kind.size().map(|(w, h)| (w.round(), h.round())), Some((200.0, 200.0)));
        // The apex handle sits just under the top centre (clear of the
        // box's edge handle); drag it to the left quarter.
        let h = at(&d, 700.0, 514.0);
        assert_eq!(d.app.prim_handle_at(h), Some((id, 0)));
        d.drag(h, at(&d, 650.0, 520.0));
        let ShapeKind::Prim { prim, .. } = d.app.doc.get(id).unwrap().kind.clone() else { panic!() };
        assert!((prim.p[0] - 0.25).abs() < 0.01, "apex {}", prim.p[0]);
        assert_eq!(d.app.selection, vec![id], "handle drag doesn't create or deselect");
        // New triangles start from the last-edited parameters.
        assert!((d.app.prim_params[&PrimKind::Triangle].p[0] - 0.25).abs() < 0.01);
        // Undo restores the apex.
        d.app.undo();
        let ShapeKind::Prim { prim, .. } = d.app.doc.get(id).unwrap().kind.clone() else { panic!() };
        assert!((prim.p[0] - 0.5).abs() < 1e-9);
        // Clicking the shapes button while a shape tool is active opens the
        // floating flyout beside it; clicking again closes it.
        let (_, btn) = *d.app.ui.flyout_anchor.iter().find(|(g, _)| *g == crate::ui::tools_panel::Group::Shapes).unwrap();
        let left = d.app.canvas_rect.left();
        d.move_to(btn.center());
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        d.frames(3, vec![]);
        assert_eq!(d.app.ui.tool_flyout, Some(crate::ui::tools_panel::Group::Shapes));
        let area = d.ctx.memory(|m| m.area_rect(egui::Id::new("tool_flyout"))).expect("flyout shown");
        assert!(area.left() > btn.right() && area.height() > 300.0, "{area:?}");
        assert_eq!(d.app.canvas_rect.left(), left, "floating: the canvas doesn't move");
        d.move_to(btn.center());
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        d.frames(2, vec![]);
        assert_eq!(d.app.ui.tool_flyout, None);
        assert_eq!(crate::ui::tools_panel::Group::Shapes.tools().len(), 4 + PrimKind::ALL.len());
    }

    #[test]
    fn fill_tool_lays_gradient_with_stops_and_midpoints() {
        use crate::gradient_handles::GradPart;
        let mut d = Driver::new();
        d.app.view.zoom = 1.0;
        d.app.view.origin = egui::vec2(100.0, 100.0);
        let id = d.app.doc.shapes()[0].id; // 300×300 at (100,100), solid fill
        let at = |d: &Driver, x: f64, y: f64| d.app.view.to_screen(kurbo::Point::new(x, y));
        d.app.set_tool(Tool::Fill);
        d.frames(2, vec![]);
        // Dragging over an unselected shape selects it and lays a gradient.
        d.drag(at(&d, 100.0, 250.0), at(&d, 400.0, 250.0));
        assert_eq!(d.app.selection, vec![id]);
        let Some(Paint::Gradient(g)) = d.app.doc.get(id).unwrap().fill.clone() else { panic!("gradient") };
        assert!((g.start[0]).abs() < 0.02 && (g.end[0] - 1.0).abs() < 0.02 && (g.start[1] - 0.5).abs() < 0.02, "{:?} {:?}", g.start, g.end);
        assert_eq!(g.stops[1].color, Rgba::WHITE, "solid fill fades to white");
        assert_eq!(d.app.gradient_handle_at(at(&d, 400.0, 250.0)), Some((id, GradPart::End)));
        // Double-click the line adds a stop at that point.
        let q = at(&d, 175.0, 250.0);
        d.move_to(q);
        for _ in 0..2 {
            d.button(egui::PointerButton::Primary, true);
            d.button(egui::PointerButton::Primary, false);
        }
        d.frames(3, vec![]);
        let Some(Paint::Gradient(g)) = d.app.doc.get(id).unwrap().fill.clone() else { panic!() };
        assert_eq!(g.stops.len(), 3, "{:?}", g.stops);
        assert!((g.stops[1].offset - 0.25).abs() < 0.02);
        // Drag the midpoint between stops 1 and 2 (at 0.625) towards 0.4.
        let mid = at(&d, 100.0 + 300.0 * 0.625, 250.0);
        assert_eq!(d.app.gradient_handle_at(mid), Some((id, GradPart::Mid(1))));
        d.drag(mid, at(&d, 100.0 + 300.0 * 0.4, 250.0));
        let Some(Paint::Gradient(g)) = d.app.doc.get(id).unwrap().fill.clone() else { panic!() };
        assert!((g.stops[1].mid - 0.2).abs() < 0.03, "mid {}", g.stops[1].mid);
        // Biased blend: half way at the midpoint.
        let t = g.stops[1].offset + (g.stops[2].offset - g.stops[1].offset) * g.stops[1].mid;
        let c = g.color_at(t);
        let (a, b) = (g.stops[1].color, g.stops[2].color);
        assert!((c.r - (a.r + b.r) / 2.0).abs() < 0.01);
        // Export expands biased segments so other apps see the same ramp.
        let ls = g.linear_stops();
        assert!(ls.len() > 3);
        for x in [0.3f32, 0.45, 0.7] {
            let lin = Gradient { stops: ls.clone(), ..g.clone() };
            assert!((lin.color_at(x).r - g.color_at(x).r).abs() < 0.03);
        }
        // Add a stop at 0.6, then drag the one at 0.25 past it: they
        // re-sort and the drag (and selection) follow the moved stop.
        assert!(d.app.gradient_add_stop(at(&d, 280.0, 250.0)));
        let s1 = at(&d, 175.0, 250.0);
        assert_eq!(d.app.gradient_handle_at(s1), Some((id, GradPart::Stop(1))));
        d.drag(s1, at(&d, 100.0 + 300.0 * 0.9, 250.0));
        let Some(Paint::Gradient(g)) = d.app.doc.get(id).unwrap().fill.clone() else { panic!() };
        let offs: Vec<f32> = g.stops.iter().map(|s| s.offset).collect();
        assert!(g.stops.windows(2).all(|w| w[0].offset <= w[1].offset), "{offs:?}");
        assert!((offs[1] - 0.6).abs() < 0.02 && (offs[2] - 0.9).abs() < 0.02, "{offs:?}");
        assert_eq!(d.app.ui.gradient_stop, 2);
        // Delete removes the selected stop, but keeps two.
        let ctx = d.ctx.clone();
        for _ in 0..3 {
            crate::commands::Cmd::Delete.run(&mut d.app, &ctx);
        }
        let Some(Paint::Gradient(g)) = d.app.doc.get(id).unwrap().fill.clone() else { panic!() };
        assert_eq!(g.stops.len(), 2);
        assert!(d.app.doc.get(id).is_some(), "shape kept");
        // Old files without midpoints load with even blends.
        let st: crate::document::GradientStop = serde_json::from_str(r#"{"offset":0.5,"color":{"r":1.0,"g":0.0,"b":0.0,"a":1.0}}"#).unwrap();
        assert_eq!(st.mid, 0.5);
    }

    #[test]
    fn integer_scrub_accumulates_small_motions() {
        // A rounded integer field (star points) must still climb when the
        // pointer moves 1 px per frame.
        let ctx = egui::Context::default();
        let mut value = 5.0f64;
        let mut t = 0.0;
        let mut run = |events: Vec<egui::Event>, value: &mut f64| {
            t += 1.0 / 60.0;
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0))), time: Some(t), events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut v = *value;
                    if crate::ui::widgets::num_field(ui, "Points", &mut v, crate::ui::widgets::Num::default().range(3.0, 64.0).speed(0.125).decimals(0), 160.0).changed() {
                        *value = v.round();
                    }
                });
            });
            out.textures_delta.clear();
        };
        run(vec![], &mut value);
        let p = egui::pos2(20.0, 20.0);
        run(vec![egui::Event::PointerMoved(p)], &mut value);
        run(vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], &mut value);
        for i in 1..=40 {
            run(vec![egui::Event::PointerMoved(p + egui::vec2(i as f32, 0.0))], &mut value);
        }
        assert!(value >= 8.0, "points climbed to {value}");
    }

    #[test]
    fn selection_hits_geometry_not_bounds() {
        use crate::document::TextData;
        let mut d = Driver::new();
        d.app.view.zoom = 1.0;
        d.app.view.origin = egui::vec2(100.0, 100.0);
        d.app.snap = false;
        let mut e = Shape::new(ShapeKind::Ellipse { w: 200.0, h: 200.0 });
        e.transform = Affine::translate((600.0, 100.0));
        let eid = d.app.doc.add_shape(e);
        d.frames(2, vec![]);
        let at = |d: &Driver, x: f64, y: f64| d.app.view.to_screen(kurbo::Point::new(x, y));
        // Marquee over the ellipse's empty bounding-box corner: nothing.
        d.drag(at(&d, 560.0, 60.0), at(&d, 620.0, 120.0));
        assert!(!d.app.selection.contains(&eid), "{:?}", d.app.selection);
        // Crossing its outline selects it.
        d.drag(at(&d, 560.0, 180.0), at(&d, 640.0, 220.0));
        assert!(d.app.selection.contains(&eid));
        // Text: a click in its box but off the glyphs misses.
        let mut t = Shape::new(ShapeKind::Text(TextData { content: "ii".into(), ..Default::default() }));
        t.transform = Affine::translate((600.0, 600.0));
        let tid = d.app.doc.add_shape(t);
        d.frames(2, vec![]);
        let b = d.app.cache.get(tid).and_then(|c| c.bounds).unwrap();
        let probe = kurbo::Point::new(b.x1 + 0.0, b.y0) + kurbo::Vec2::new(-1.0, 1.0);
        let path = d.app.cache.get(tid).unwrap().doc_path.clone();
        if path.winding(probe) == 0 && crate::geom::distance_to_path(&path, probe) > 8.0 {
            assert_eq!(d.app.hit_shape(probe), None);
        }
        let on = kurbo::Point::new(b.x0 + 2.0, b.center().y);
        let near_glyph = crate::geom::distance_to_path(&path, on) < 3.0 || path.winding(on) != 0;
        if near_glyph {
            assert_eq!(d.app.hit_shape(on), Some(tid));
        }
        // Text tools still place the caret anywhere in the box.
        d.app.set_tool(Tool::Text);
        assert_eq!(d.app.hit_shape(kurbo::Point::new(b.center().x, b.center().y)), Some(tid));
    }

    #[test]
    fn window_buttons_work_over_the_welcome_screen() {
        let mut d = Driver::new();
        d.app.ui.welcome_open = true;
        d.frames(3, vec![]);
        // Maximize is the middle of the three 42 px buttons at the top right.
        let p = egui::pos2(1600.0 - 42.0 * 1.5, crate::ui::theme::MENUBAR_H / 2.0);
        d.move_to(p);
        d.button(egui::PointerButton::Primary, true);
        let e = egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: d.mods };
        d.frame(vec![e]);
        assert!(d.cmds.iter().any(|c| matches!(c, egui::ViewportCommand::Maximized(true))), "{:?}", d.cmds);
        assert!(d.app.ui.welcome_open, "the welcome screen stays");
    }

    #[test]
    fn document_tabs_keep_their_own_state() {
        let mut d = Driver::new();
        let first_uid = d.app.doc_uid;
        let a = d.app.doc.shapes()[0].id;
        d.app.selection = vec![a];
        d.app.dirty = true;
        // New opens a second tab (the first isn't blank).
        d.app.run_cmd(crate::commands::Cmd::New, &d.ctx.clone());
        assert_eq!(d.app.tab_count(), 2);
        assert_eq!(d.app.tab, 1);
        assert!(d.app.doc.shapes().is_empty() && d.app.selection.is_empty() && !d.app.dirty);
        assert_ne!(d.app.doc_uid, first_uid);
        // A blank tab is reused by the next New.
        d.app.run_cmd(crate::commands::Cmd::New, &d.ctx.clone());
        assert_eq!(d.app.tab_count(), 2);
        d.app.doc.add_shape(Shape::new(ShapeKind::Ellipse { w: 10.0, h: 10.0 }));
        d.frames(3, vec![]);
        // Back to the first: its shapes, selection and dirty flag return.
        d.app.switch_tab(0);
        assert_eq!(d.app.doc.shapes()[0].id, a);
        assert_eq!(d.app.selection, vec![a]);
        assert!(d.app.dirty && d.app.doc_uid == first_uid);
        let infos = d.app.tab_infos();
        assert!(infos[0].active && infos[0].dirty && !infos[1].active);
        // Ctrl+Tab cycles.
        d.app.run_cmd(crate::commands::Cmd::NextTab, &d.ctx.clone());
        assert_eq!(d.app.tab, 1);
        assert!(matches!(d.app.doc.shapes()[0].kind, ShapeKind::Ellipse { .. }));
        // Closing the clean second tab needs no prompt; the dirty first one asks.
        d.app.request_close_tab(1);
        assert_eq!(d.app.tab_count(), 1);
        assert_eq!(d.app.doc.shapes()[0].id, a);
        d.app.request_close_tab(0);
        assert_eq!(d.app.session.prompt, Some(crate::session::Guarded::CloseTab));
        d.app.answer_prompt(Some(false), &d.ctx.clone());
        assert_eq!(d.app.tab_count(), 1);
        assert!(d.app.doc.shapes().is_empty() && d.app.ui.welcome_open, "last tab closed: blank document and start screen");
    }

    #[test]
    fn quitting_asks_about_each_unsaved_tab() {
        let mut d = Driver::new();
        d.app.dirty = true;
        d.app.push_tab();
        d.app.doc.add_shape(Shape::new(ShapeKind::Ellipse { w: 10.0, h: 10.0 }));
        d.app.dirty = true;
        d.app.push_tab(); // a third, clean tab is active
        let ctx = d.ctx.clone();
        d.app.run_cmd(crate::commands::Cmd::Quit, &ctx);
        assert_eq!(d.app.session.prompt, Some(crate::session::Guarded::Quit));
        assert_eq!(d.app.tab, 0, "switched to the first unsaved tab");
        d.app.answer_prompt(Some(false), &ctx);
        assert_eq!(d.app.session.prompt, Some(crate::session::Guarded::Quit), "then the next one");
        assert_eq!(d.app.tab, 1);
        assert!(!d.app.session.allow_close);
        d.app.answer_prompt(Some(false), &ctx);
        assert!(d.app.session.allow_close && d.app.session.prompt.is_none());
    }

    #[test]
    fn tab_bar_switches_and_closes() {
        let mut d = Driver::new();
        d.app.push_tab();
        d.frames(3, vec![]);
        assert_eq!(d.app.tab, 1);
        // Tabs start after the menus; find the first tab by clicking along the bar.
        let y = crate::ui::theme::MENUBAR_H / 2.0;
        let mut hit = None;
        for x in (250..700).step_by(10) {
            let p = egui::pos2(x as f32, y);
            d.move_to(p);
            d.button(egui::PointerButton::Primary, true);
            d.button(egui::PointerButton::Primary, false);
            if d.app.tab == 0 {
                hit = Some(p);
                break;
            }
        }
        let p = hit.expect("clicking the first tab activates it");
        // Middle-click closes it.
        d.move_to(p);
        d.button(egui::PointerButton::Middle, true);
        d.button(egui::PointerButton::Middle, false);
        assert_eq!(d.app.tab_count(), 1);
    }

    #[test]
    fn pivot_snaps_to_geometry() {
        let mut d = Driver::new();
        let star = d.app.doc.add_shape({
            let mut s = Shape::new(ShapeKind::Star { w: 200.0, h: 200.0, points: 5, inner: 0.5, corner: 0.0 });
            s.transform = Affine::translate((500.0, 100.0));
            s
        });
        let rect = d.app.doc.shapes()[0].id;
        d.app.selection = vec![rect];
        d.frames(3, vec![]);
        let pv = d.app.pivot_point().unwrap();
        // The star's inner vertex right of its top point.
        let path = d.app.cache.update(&d.app.doc.get(star).unwrap().clone(), &mut d.app.fonts).doc_path.clone();
        let node = match path.elements()[1] {
            kurbo::PathEl::LineTo(q) => q,
            _ => panic!(),
        };
        let target = node + kurbo::Vec2::new(2.0 / d.app.view.zoom as f64, 1.0 / d.app.view.zoom as f64);
        let from = d.app.view.to_screen(pv);
        d.move_to(from);
        d.button(egui::PointerButton::Primary, true);
        for i in 1..=10 {
            d.move_to(from + (d.app.view.to_screen(target) - from) * (i as f32 / 10.0));
        }
        let got = d.app.pivot_point().unwrap();
        assert!((got - node).hypot() < 1e-6, "{got:?} vs node {node:?}");
        assert!(!d.app.snap_guides.is_empty());
        d.button(egui::PointerButton::Primary, false);
        assert!(d.app.snap_guides.is_empty());
        // Near an edge (no node): onto the outline.
        let ShapeKind::Rect { .. } = d.app.doc.get(rect).unwrap().kind else { panic!() };
        let edge = kurbo::Point::new(250.0, 400.0 + 2.0 / d.app.view.zoom as f64);
        let pv = d.app.pivot_point().unwrap();
        let from = d.app.view.to_screen(pv);
        d.move_to(from);
        d.button(egui::PointerButton::Primary, true);
        for i in 1..=10 {
            d.move_to(from + (d.app.view.to_screen(edge) - from) * (i as f32 / 10.0));
        }
        let got = d.app.pivot_point().unwrap();
        assert!((got.y - 400.0).abs() < 1e-6 && (got.x - 250.0).abs() < 0.5, "{got:?}");
        d.button(egui::PointerButton::Primary, false);
    }

    #[test]
    fn color_keys_link_fills_and_strokes() {
        let (mut app, a, b) = app_with_squares();
        let red = Rgba::from_hex("#FF0000").unwrap();
        let green = Rgba::from_hex("#00FF00").unwrap();
        // Collecting the colours in use links the shapes using them.
        assert_eq!(app.keys_from_document(), 2);
        let k = app.doc.colors.iter().find(|k| k.color == red).unwrap().id;
        assert_eq!(app.doc.colors[0].name, "Colour 1");
        assert_eq!(app.doc.get(a).unwrap().fill_key, Some(k));
        // Apply the key to b's stroke.
        app.selection = vec![b];
        app.apply_color_key(k, true);
        assert_eq!(app.doc.get(b).unwrap().stroke.as_ref().unwrap().color, red);
        // Editing the key recolours every user.
        app.doc.set_key_color(k, green);
        assert_eq!(app.doc.get(a).unwrap().fill, Some(Paint::Solid(green)));
        assert_eq!(app.doc.get(b).unwrap().stroke.as_ref().unwrap().color, green);
        // A colour changed some other way is unlinked rather than overwritten.
        app.selection = vec![a];
        app.apply_color(Rgba::WHITE, false);
        assert_eq!(app.doc.get(a).unwrap().fill_key, None);
        app.doc.set_key_color(k, red);
        assert_eq!(app.doc.get(a).unwrap().fill, Some(Paint::Solid(Rgba::WHITE)));
        // Saved with the document; deleting unlinks; undo brings it back.
        let json = serde_json::to_string(&app.doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(back.colors.len(), 2);
        assert_eq!(back.get(b).unwrap().stroke_key, Some(k));
        app.checkpoint("");
        app.doc.remove_color_key(k);
        assert_eq!(app.doc.get(b).unwrap().stroke_key, None);
        assert_eq!(app.doc.colors.len(), 1);
        app.undo();
        assert_eq!(app.doc.colors.len(), 2);
        assert_eq!(app.doc.get(b).unwrap().stroke_key, Some(k));
        // Pasted shapes keep links only to matching keys of this document.
        let mut s = app.doc.get(b).unwrap().clone();
        s.stroke_key = Some(99);
        app.doc.fix_key_links(&mut s);
        assert_eq!(s.stroke_key, None);
        let mut s = app.doc.get(b).unwrap().clone();
        app.doc.fix_key_links(&mut s);
        assert_eq!(s.stroke_key, Some(k));
    }

    #[test]
    fn compound_cutter_stays_grabbable_and_copyable() {
        // a: square 0..100, b: circle 50..150, a minus b.
        let (mut app, a, b) = app_with_squares();
        app.selection = vec![a, b];
        app.make_compound(BoolOp::Subtract);
        let comp = app.doc.shapes()[0].id;
        assert!(app.doc.get(comp).unwrap().kind.is_container());
        // Inside the hole nothing visible is hit, but with the cutter
        // selected, pressing there picks it (so it can be dragged).
        let hole = kurbo::Point::new(80.0, 80.0);
        assert_eq!(app.hit_shape(hole), None);
        app.selection = vec![b];
        assert_eq!(app.pick(hole, false), Some(b));
        // Copy puts the child on the clipboard.
        let payload = crate::platform::clipboard::build_payload(&app.doc, &[b]);
        assert_eq!(payload.shapes.len(), 1);
        assert_eq!(payload.shapes[0].id, b);
        let n = app.doc.shapes().len();
        let ids = app.insert_payload(&payload);
        assert_eq!(ids.len(), 1);
        assert_eq!(app.doc.shapes().len(), n + 1, "pasted as a standalone shape");
        // An ancestor in the copy covers its children.
        assert_eq!(crate::platform::clipboard::build_payload(&app.doc, &[comp, b]).shapes.len(), 1);
    }

    #[test]
    fn shift_keeps_node_drags_straight() {
        let mut d = Driver::new();
        let id = d.app.doc.shapes()[0].id;
        d.app.selection = vec![id];
        d.app.convert_to_curves();
        d.app.set_tool(Tool::Node);
        d.frames(3, vec![]);
        let n0 = match &d.app.doc.get(id).unwrap().kind {
            ShapeKind::Path(pd) => pd.subpaths[0].nodes[0].p,
            _ => panic!(),
        };
        let from = d.app.view.to_screen(n0);
        d.mods = egui::Modifiers::SHIFT;
        d.frame(vec![egui::Event::ModifiersChanged(egui::Modifiers::SHIFT)]);
        d.drag(from, from + egui::vec2(80.0, 9.0));
        d.mods = egui::Modifiers::NONE;
        d.frame(vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
        let n1 = match &d.app.doc.get(id).unwrap().kind {
            ShapeKind::Path(pd) => pd.subpaths[0].nodes[0].p,
            _ => panic!(),
        };
        assert!((n1.y - n0.y).abs() < 1e-6, "kept horizontal: {n0:?} -> {n1:?}");
        assert!(n1.x - n0.x > 50.0 / d.app.view.zoom as f64);
    }

    #[test]
    fn eyedropper_picks_the_visible_blend() {
        let mut d = Driver::new();
        // A 50% blue square over the white artboard, a red one under part of it.
        let first = d.app.doc.shapes()[0].id;
        d.app.doc.remove(first);
        let mut red = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
        red.fill = Some(Paint::Solid(Rgba::from_hex("#FF0000").unwrap()));
        d.app.doc.add_shape(red);
        let mut blue = Shape::new(ShapeKind::Rect { w: 200.0, h: 100.0, radii: [0.0; 4] });
        blue.fill = Some(Paint::Solid(Rgba::from_hex("#0000FF").unwrap()));
        blue.opacity = 0.5;
        d.app.doc.add_shape(blue);
        d.frames(3, vec![]);
        let at = |d: &mut Driver, x, y| d.app.sample_color(kurbo::Point::new(x, y)).unwrap();
        let over_white = at(&mut d, 150.0, 50.0);
        assert_eq!(over_white.to_hex(), "#8080FF", "{over_white:?}");
        let over_red = at(&mut d, 50.0, 50.0);
        assert_eq!(over_red.to_hex(), "#800080");
        assert!((over_red.a - 1.0).abs() < 1e-6);
        // Just outside the shape (within the hit tolerance): the artboard.
        assert_eq!(at(&mut d, 201.5, 50.0).to_hex(), "#FFFFFF");
    }

    #[test]
    fn pen_previews_the_snapped_node() {
        let mut d = Driver::new();
        d.app.set_tool(Tool::Pen);
        d.frames(2, vec![]);
        // Near the square's corner (100, 100).
        let near = d.app.view.to_screen(kurbo::Point::new(100.0, 100.0)) + egui::vec2(3.0, 2.0);
        d.move_to(near);
        d.frames(2, vec![]);
        let h = d.app.pen_hover.expect("preview");
        assert!((h - kurbo::Point::new(100.0, 100.0)).hypot() < 1e-6, "{h:?}");
        assert!(!d.app.snap_guides.is_empty());
        // Leaving the tool clears the preview and its guides.
        d.app.set_tool(Tool::Select);
        d.frames(2, vec![]);
        assert!(d.app.pen_hover.is_none() && d.app.snap_guides.is_empty());
    }

    #[test]
    fn star_cog_and_spiral_handles() {
        use crate::prim_handles::{drag_local, local_handles};
        use crate::prims::PrimKind;
        let m = 5.0;
        // Star: inner, points, corner.
        let mut k = ShapeKind::Star { w: 200.0, h: 200.0, points: 5, inner: 0.5, corner: 0.0 };
        let hs = local_handles(&k, m);
        assert_eq!(hs.len(), 3);
        drag_local(&mut k, 0, kurbo::Point::new(100.0, 100.0) + (hs[0] - kurbo::Point::new(100.0, 100.0)) * 0.6, m);
        drag_local(&mut k, 1, kurbo::Point::new(100.0 + 100.0 * (std::f64::consts::TAU / 8.0 - std::f64::consts::FRAC_PI_2).cos(), 100.0 + 100.0 * (std::f64::consts::TAU / 8.0 - std::f64::consts::FRAC_PI_2).sin()), m);
        drag_local(&mut k, 2, kurbo::Point::new(100.0, m + 12.0), m);
        let ShapeKind::Star { points, inner, corner, .. } = k else { panic!() };
        assert_eq!(points, 8);
        assert!((inner - 0.3).abs() < 0.01, "{inner}");
        assert!((corner - 12.0).abs() < 1e-9);
        // Polygon: sides and corner.
        let mut k = ShapeKind::Polygon { w: 100.0, h: 100.0, sides: 6, corner: 0.0 };
        assert_eq!(local_handles(&k, m).len(), 2);
        drag_local(&mut k, 0, kurbo::Point::new(100.0, 50.0), m); // a quarter turn: 4 sides
        let ShapeKind::Polygon { sides, .. } = k else { panic!() };
        assert_eq!(sides, 4);
        // Cog: four handles; the third sets the teeth.
        let mut k = ShapeKind::Prim { w: 100.0, h: 100.0, prim: PrimKind::Cog.default_prim() };
        assert_eq!(local_handles(&k, m).len(), 4);
        drag_local(&mut k, 2, kurbo::Point::new(100.0, 50.0), m);
        let ShapeKind::Prim { prim, .. } = &k else { panic!() };
        assert_eq!(prim.p[0], 4.0);
        // Spiral: the end handle winds past whole turns continuously.
        let mut k = ShapeKind::Prim { w: 100.0, h: 100.0, prim: PrimKind::Spiral.default_prim() };
        for step in 1..=16 {
            let a = std::f64::consts::TAU * (3.0 + step as f64 / 8.0);
            drag_local(&mut k, 1, kurbo::Point::new(50.0 + 50.0 * a.cos(), 50.0 + 50.0 * a.sin()), m);
        }
        let ShapeKind::Prim { prim, .. } = &k else { panic!() };
        assert!((prim.p[0] - 5.0).abs() < 0.01, "turns {}", prim.p[0]);
        // Tear: three handles, each round-trips.
        let k = ShapeKind::Prim { w: 100.0, h: 160.0, prim: PrimKind::Tear.default_prim() };
        let hs = local_handles(&k, m);
        assert_eq!(hs.len(), 3);
        for (i, h) in hs.iter().enumerate() {
            let mut k2 = k.clone();
            drag_local(&mut k2, i, *h, m);
            let (ShapeKind::Prim { prim: a, .. }, ShapeKind::Prim { prim: b, .. }) = (&k2, &k) else { panic!() };
            assert!(a.p.iter().zip(b.p).all(|(x, y)| (x - y).abs() < 1e-6), "tear handle {i}: {:?}", a.p);
        }
        // QR: the text survives a save, and the code changes with it.
        let mut q = PrimKind::Qr.default_prim();
        let a = crate::prims::path(&q, 100.0, 100.0);
        q.text = "something else entirely".into();
        let b = crate::prims::path(&q, 100.0, 100.0);
        assert_ne!(a, b);
        let back: crate::prims::Prim = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(back, q);
    }

    #[test]
    fn fill_tool_gradient_points_snap() {
        use crate::gradient_handles::GradPart;
        let mut d = Driver::new();
        d.app.view.zoom = 1.0;
        d.app.view.origin = egui::vec2(100.0, 100.0);
        d.app.snap = true;
        let id = d.app.doc.shapes()[0].id; // 300×300 at (100,100)
        let at = |d: &Driver, x: f64, y: f64| d.app.view.to_screen(kurbo::Point::new(x, y));
        d.app.set_tool(Tool::Fill);
        d.frames(2, vec![]);
        // Laying: both ends land on the edge midpoints.
        d.drag(at(&d, 103.0, 252.0), at(&d, 397.0, 248.0));
        let Some(Paint::Gradient(g)) = d.app.doc.get(id).unwrap().fill.clone() else { panic!("gradient") };
        assert!((g.start[0]).abs() < 1e-6 && (g.start[1] - 0.5).abs() < 1e-6, "{:?}", g.start);
        assert!((g.end[0] - 1.0).abs() < 1e-6 && (g.end[1] - 0.5).abs() < 1e-6, "{:?}", g.end);
        // Dragging the end handle snaps it to the corner.
        let end = at(&d, 400.0, 250.0);
        assert_eq!(d.app.gradient_handle_at(end), Some((id, GradPart::End)));
        d.drag(end, at(&d, 402.0, 397.0));
        let Some(Paint::Gradient(g)) = d.app.doc.get(id).unwrap().fill.clone() else { panic!() };
        assert!((g.end[0] - 1.0).abs() < 1e-6 && (g.end[1] - 1.0).abs() < 1e-6, "{:?}", g.end);
        assert!(d.app.snap_guides.is_empty(), "guides clear on release");
        // With snapping off the point lands where dropped.
        d.app.snap = false;
        d.drag(at(&d, 400.0, 400.0), at(&d, 397.0, 396.0));
        let Some(Paint::Gradient(g)) = d.app.doc.get(id).unwrap().fill.clone() else { panic!() };
        assert!((g.end[0] - 0.99).abs() < 1e-6, "{:?}", g.end);
    }

    #[test]
    fn picker_look_is_a_global_setting() {
        use crate::color::{PickerMode, PickerPrefs, picker_prefs, set_picker_prefs};
        let mut d = Driver::new();
        let mut s = d.app.settings.clone();
        s.picker = PickerPrefs { wheel: true, mode: PickerMode::Rgb };
        let ctx = d.ctx.clone();
        d.app.apply_settings(s, &ctx);
        assert_eq!(picker_prefs(&d.ctx), PickerPrefs { wheel: true, mode: PickerMode::Rgb });
        // A change made in any picker lands in the settings.
        set_picker_prefs(&d.ctx, PickerPrefs { wheel: false, mode: PickerMode::Rgb });
        d.frames(2, vec![]);
        assert_eq!(d.app.settings.picker, PickerPrefs { wheel: false, mode: PickerMode::Rgb });
        // And survives a settings.json round trip.
        let json = serde_json::to_string(&d.app.settings).unwrap();
        let back: crate::settings::Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.picker.mode, PickerMode::Rgb);
        let old: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.picker, PickerPrefs::default());
    }

    #[test]
    fn fill_tool_end_handles_select_end_stops() {
        use crate::gradient_handles::GradPart;
        let mut d = Driver::new();
        d.app.view.zoom = 1.0;
        d.app.view.origin = egui::vec2(100.0, 100.0);
        let id = d.app.doc.shapes()[0].id;
        let at = |d: &Driver, x: f64, y: f64| d.app.view.to_screen(kurbo::Point::new(x, y));
        d.app.set_tool(Tool::Fill);
        d.frames(2, vec![]);
        d.drag(at(&d, 100.0, 250.0), at(&d, 400.0, 250.0));
        d.app.ui.gradient_stop = 0;
        let end = at(&d, 400.0, 250.0);
        assert_eq!(d.app.gradient_handle_at(end), Some((id, GradPart::End)));
        d.move_to(end);
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        assert_eq!(d.app.ui.gradient_stop, 1);
        let start = at(&d, 100.0, 250.0);
        d.move_to(start);
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        assert_eq!(d.app.ui.gradient_stop, 0);
    }

    #[test]
    fn rotation_knob_and_oriented_scale() {
        let mut d = Driver::new();
        let id = d.app.doc.shapes()[0].id;
        d.app.selection = vec![id];
        d.frames(2, vec![]);
        // Drag the knob (above the top-center) a quarter turn around the center.
        let c = d.app.view.to_screen(kurbo::Point::new(250.0, 250.0));
        let top = d.app.view.to_screen(kurbo::Point::new(250.0, 100.0));
        let knob = top - egui::vec2(0.0, 26.0);
        let r = c.distance(knob);
        d.move_to(knob);
        d.button(egui::PointerButton::Primary, true);
        for i in 1..=12 {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_2 * (i as f32 / 12.0);
            d.move_to(c + egui::vec2(a.cos(), a.sin()) * r);
        }
        d.button(egui::PointerButton::Primary, false);
        d.frames(3, vec![]);
        let s = d.app.doc.get(id).unwrap().clone();
        let [a, b, ..] = s.transform.as_coeffs();
        assert!((b.atan2(a).to_degrees() - 90.0).abs() < 1.0, "{}", b.atan2(a).to_degrees());
        assert!(matches!(s.kind, ShapeKind::Rect { .. }));
        // Rotated about the pivot (center): bounds unchanged for a square.
        let bb = d.app.selection_bounds().unwrap();
        assert!((bb.center().x - 250.0).abs() < 0.5 && (bb.center().y - 250.0).abs() < 0.5, "{bb:?}");
        // Move the pivot to the top-left corner (it snaps), then rotate again.
        let pv = d.app.view.to_screen(kurbo::Point::new(250.0, 250.0));
        let (frame, lr) = d.app.sel_frame().unwrap();
        let corner = d.app.view.to_screen(frame * kurbo::Point::new(lr.x0 + 2.0, lr.y0 + 2.0));
        d.drag(pv, corner);
        assert_eq!(d.app.pivot, Some((0.0, 0.0)));
    }

    #[test]
    fn move_snaps_to_other_shape_and_reports_target() {
        let mut d = Driver::new();
        let mut b = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
        b.transform = Affine::translate((450.0, 600.0));
        d.app.doc.add_shape(b);
        d.frames(2, vec![]);
        let z = d.app.view.zoom as f64;
        // Drag A so its right edge (400) lands 2 screen px short of B's left edge (450).
        let from = d.app.view.to_screen(kurbo::Point::new(250.0, 250.0));
        let dx = (50.0 - 2.0 / z) as f32 * z as f32;
        d.move_to(from);
        d.button(egui::PointerButton::Primary, true);
        for i in 1..=8 {
            d.move_to(from + egui::vec2(dx * i as f32 / 8.0, 0.0));
        }
        let g = d.app.snap_guides.iter().find(|g| g.vertical).cloned().expect("vertical snap");
        assert!(g.is_shape && (g.pos - 450.0).abs() < 1e-6 && (g.target.x0 - 450.0).abs() < 1e-6, "{g:?}");
        d.button(egui::PointerButton::Primary, false);
        let a = d.app.doc.shapes()[0].transform.as_coeffs()[4];
        assert!((a - 150.0).abs() < 1e-6, "{a}");
    }

    #[test]
    fn smart_spacing_equal_and_matching_gaps() {
        use kurbo::Rect;
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(300.0, 0.0, 400.0, 100.0);
        // Between two neighbours: centred with equal gaps.
        let m = Rect::new(148.0, 10.0, 248.0, 90.0);
        let (off, gaps) = crate::snap::spacing(m, &[a, b], true, 5.0).expect("equal spacing");
        assert!((off - 2.0).abs() < 1e-9, "{off}");
        assert_eq!(gaps.len(), 2);
        // Beyond the last shape: repeats the existing 200 px gap.
        let m = Rect::new(597.0, 0.0, 697.0, 100.0);
        let (off, _) = crate::snap::spacing(m, &[a, b], true, 5.0).expect("matching gap");
        assert!((off - 3.0).abs() < 1e-9, "{off}");
        // Vertically too.
        let m = Rect::new(0.0, 151.0, 100.0, 251.0);
        let c = Rect::new(0.0, 300.0, 100.0, 400.0);
        let (off, g) = crate::snap::spacing(m, &[a, c], false, 5.0).expect("vertical");
        assert!((off + 1.0).abs() < 1e-9 && !g[0].along_x, "{off}");
        // Nothing nearby.
        assert!(crate::snap::spacing(Rect::new(120.0, 500.0, 140.0, 520.0), &[a, b], true, 5.0).is_none());
        // Shift constrains the pen to 45°.
        let q = crate::canvas::constrain_45(kurbo::Point::ZERO, kurbo::Point::new(10.0, 9.0));
        assert!((q.x - q.y).abs() < 1e-9);
    }

    #[test]
    fn create_and_node_drags_snap() {
        let mut d = Driver::new();
        let z = d.app.view.zoom as f64;
        // Rectangle tool: press 2 screen px from A's corner (400, 400).
        d.app.set_tool(crate::app::Tool::Rect);
        let off = 2.0 / z;
        let from = d.app.view.to_screen(kurbo::Point::new(400.0 + off, 400.0 + off));
        let to = d.app.view.to_screen(kurbo::Point::new(520.0, 520.0));
        d.drag(from, to);
        let s = d.app.doc.shapes().last().unwrap().clone();
        let [.., x, y] = s.transform.as_coeffs();
        assert!((x - 400.0).abs() < 1e-6 && (y - 400.0).abs() < 1e-6, "{x},{y}");
    }

    #[test]
    fn guides_from_ruler_snap_and_remove() {
        let mut d = Driver::new();
        let cr = d.app.canvas_rect;
        let ruler_y = cr.top() - crate::ui::theme::RULER / 2.0;
        // Drag a horizontal guide out of the top ruler to y ≈ 700.
        let target = d.app.view.to_screen(kurbo::Point::new(800.0, 700.3));
        d.drag(egui::pos2(target.x, ruler_y), target);
        assert_eq!(d.app.doc.guides.len(), 1);
        let g = d.app.doc.guides[0];
        assert!(!g.vertical && (g.pos - 700.0).abs() < 0.6, "{g:?}");
        assert!(d.app.history.can_undo());
        // Moving A (bottom edge 400) near the guide snaps onto it.
        let z = d.app.view.zoom as f32;
        let from = d.app.view.to_screen(kurbo::Point::new(250.0, 250.0));
        let dy = (g.pos as f32 - 400.0 - 2.0 / z) * z;
        d.move_to(from);
        d.button(egui::PointerButton::Primary, true);
        for i in 1..=8 {
            d.move_to(from + egui::vec2(0.0, dy * i as f32 / 8.0));
        }
        d.button(egui::PointerButton::Primary, false);
        d.frames(3, vec![]);
        let ty = d.app.doc.shapes()[0].transform.as_coeffs()[5];
        assert!((ty + 300.0 - g.pos).abs() < 1e-6, "{ty}");
        // Drag the guide back onto the ruler: removed.
        d.app.selection.clear();
        let gs = d.app.view.to_screen(kurbo::Point::new(1500.0, g.pos));
        d.drag(egui::pos2(gs.x, gs.y), egui::pos2(gs.x, ruler_y));
        assert!(d.app.doc.guides.is_empty());
    }

    #[test]
    fn gradient_handle_drag_and_eyedropper() {
        use crate::document::{Gradient, GradientKind};
        let mut d = Driver::new();
        let id = d.app.doc.shapes()[0].id;
        let red = Rgba::from_hex("#FF0000").unwrap();
        if let Some(s) = d.app.doc.get_mut(id) {
            s.fill = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, red, Rgba::from_hex("#0000FF").unwrap())));
        }
        d.app.selection = vec![id];
        d.frames(3, vec![]);
        // Handles only show while the gradient is being edited.
        let end = d.app.view.to_screen(kurbo::Point::new(400.0, 250.0));
        assert!(d.app.gradient_handle_at(end).is_none());
        d.app.ui.gradient_edit = Some(id);
        assert_eq!(d.app.gradient_handle_at(end), Some((id, crate::gradient_handles::GradPart::End)));
        let to = d.app.view.to_screen(kurbo::Point::new(250.0, 400.0));
        d.drag(end, to);
        let Some(Paint::Gradient(g)) = &d.app.doc.get(id).unwrap().fill else { panic!() };
        assert!((g.end[0] - 0.5).abs() < 0.01 && (g.end[1] - 1.0).abs() < 0.01, "{:?}", g.end);
        assert_eq!(d.app.doc.shapes()[0].transform.as_coeffs()[4], 100.0, "shape moved");
        // Eyedropper: sample the top of the gradient (red) onto a new shape.
        let mut b = Shape::new(ShapeKind::Rect { w: 50.0, h: 50.0, radii: [0.0; 4] });
        b.transform = Affine::translate((600.0, 600.0));
        let ib = d.app.doc.add_shape(b);
        d.app.selection = vec![ib];
        d.app.set_tool(crate::app::Tool::Eyedropper);
        d.frames(3, vec![]);
        let top = d.app.view.to_screen(kurbo::Point::new(250.0, 102.0));
        d.move_to(top);
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        d.frames(2, vec![]);
        let c = d.app.doc.get(ib).unwrap().fill.as_ref().and_then(|f| f.solid()).unwrap();
        assert!(c.r > 0.95 && c.b < 0.05, "{c:?}");
        assert_eq!(d.app.tool, crate::app::Tool::Select, "returns to the previous tool");
        assert!(d.app.settings.last_picked.is_some_and(|c| c.r > 0.95), "remembers the colour");
    }

    #[test]
    fn eyedropper_targets_the_open_colour_editor() {
        let hex = |c: Rgba| c.with_alpha(1.0).to_hex();
        let run = |open: Option<&str>, shift: bool| {
            let mut d = Driver::new();
            let id = d.app.doc.shapes()[0].id;
            let mut sh = d.app.doc.get(id).unwrap().clone();
            sh.fill = Some(Paint::Solid(Rgba::from_hex("#112233").unwrap()));
            sh.stroke = Some(crate::document::Stroke { color: Rgba::from_hex("#445566").unwrap(), width: 2.0, ..Default::default() });
            d.app.doc.replace(sh);
            let mut src = Shape::new(ShapeKind::Rect { w: 80.0, h: 80.0, radii: [0.0; 4] });
            src.transform = Affine::translate((600.0, 100.0));
            src.fill = Some(Paint::Solid(Rgba::from_hex("#FF0000").unwrap()));
            d.app.doc.add_shape(src);
            d.app.selection = vec![id];
            d.app.ui.paint_open = open.map(String::from);
            d.app.set_tool(crate::app::Tool::Eyedropper);
            d.frames(3, vec![]);
            if shift {
                d.mods = egui::Modifiers::SHIFT;
                d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
            }
            d.move_to(d.app.view.to_screen(kurbo::Point::new(640.0, 140.0)));
            d.button(egui::PointerButton::Primary, true);
            d.button(egui::PointerButton::Primary, false);
            d.frames(2, vec![]);
            let s = d.app.doc.get(id).unwrap();
            (hex(s.fill.as_ref().and_then(|f| f.solid()).unwrap()), hex(s.stroke.as_ref().unwrap().color))
        };
        let (old_fill, old_stroke) = ("#112233".to_string(), "#445566".to_string());
        let red = "#FF0000".to_string();
        assert_eq!(run(Some("stroke"), false), (old_fill.clone(), red.clone()), "stroke editor open");
        assert_eq!(run(Some("fill"), false), (red.clone(), old_stroke.clone()), "fill editor open");
        assert_eq!(run(None, false), (red.clone(), old_stroke), "nothing open");
        assert_eq!(run(Some("fill"), true), (old_fill, red), "Shift");
    }

    #[test]
    fn alt_click_cycles_through_stacked_layers() {
        let mut d = Driver::new();
        d.app.snap = false;
        let a = d.app.doc.shapes()[0].id;
        let add = |d: &mut Driver, x: f64| {
            let mut s = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
            s.transform = Affine::translate((x, 200.0));
            d.app.doc.add_shape(s)
        };
        let b = add(&mut d, 180.0);
        let c = add(&mut d, 190.0);
        d.frames(3, vec![]);
        let at = d.app.view.to_screen(kurbo::Point::new(220.0, 220.0));
        d.mods = egui::Modifiers::ALT;
        d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
        d.move_to(at);
        let shapes = d.app.doc.shapes().len();
        for want in [c, b, a, c] {
            d.button(egui::PointerButton::Primary, true);
            d.button(egui::PointerButton::Primary, false);
            d.frames(2, vec![]);
            assert_eq!(d.app.selection, vec![want]);
        }
        assert_eq!(d.app.doc.shapes().len(), shapes, "a click duplicates nothing");
        // Grouped layers are stepped through one by one.
        {
            let (mut g, ids) = {
                let mut g = Driver::new();
                g.app.snap = false;
                let a = g.app.doc.shapes()[0].id;
                let b = add(&mut g, 180.0);
                let c = add(&mut g, 190.0);
                g.app.selection = vec![a, b, c];
                g.app.group_selection();
                g.frames(3, vec![]);
                (g, [a, b, c])
            };
            let [a, b, c] = ids;
            let group = g.app.selection[0];
            assert_ne!(group, c);
            g.mods = egui::Modifiers::ALT;
            g.frame(vec![egui::Event::ModifiersChanged(g.mods)]);
            let at = g.app.view.to_screen(kurbo::Point::new(220.0, 220.0));
            g.move_to(at);
            for want in [c, b, a, c] {
                g.button(egui::PointerButton::Primary, true);
                g.button(egui::PointerButton::Primary, false);
                g.frames(2, vec![]);
                assert_eq!(g.app.selection, vec![want]);
            }
        }
        // Alt+drag still duplicates, and keeps the copy selected.
        d.drag(at, at + egui::vec2(60.0, 0.0));
        assert_eq!(d.app.doc.shapes().len(), shapes + 1);
        assert!(d.app.selection != vec![b] && d.app.selection.len() == 1);
    }

    #[test]
    fn dragging_a_covered_selection_moves_it() {
        let setup = || {
            let mut d = Driver::new();
            d.app.snap = false;
            let below = d.app.doc.shapes()[0].id;
            let mut top = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
            top.transform = Affine::translate((200.0, 200.0));
            let top = d.app.doc.add_shape(top);
            d.app.selection = vec![below];
            d.frames(3, vec![]);
            // Off-centre: the selection's pivot sits at (250, 250).
            let at = d.app.view.to_screen(kurbo::Point::new(220.0, 220.0));
            (d, below, top, at)
        };
        let x = |d: &Driver, id| d.app.doc.get(id).unwrap().transform.as_coeffs()[4];
        // A drag moves the selected shape, not the one over it.
        let (mut d, below, top, at) = setup();
        d.drag(at, at + egui::vec2(60.0, 0.0));
        assert_eq!(d.app.selection, vec![below]);
        assert!(x(&d, below) > 100.5, "{}", x(&d, below));
        assert_eq!(x(&d, top), 200.0);
        // A click still selects the shape on top.
        let (mut d, below, top, at) = setup();
        d.move_to(at);
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        d.frames(2, vec![]);
        assert_eq!(d.app.selection, vec![top]);
        assert_eq!(x(&d, below), 100.0);
        // Pressing the top shape outside the selection selects it at once.
        let (mut d, below, top, at) = setup();
        d.app.doc.get_mut(below).unwrap().transform = Affine::translate((-900.0, 100.0));
        d.frames(2, vec![]);
        d.drag(at, at + egui::vec2(60.0, 0.0));
        assert_eq!(d.app.selection, vec![top]);
        assert!(x(&d, top) > 200.5);
    }

    #[test]
    fn scaling_a_sheared_shape_snaps() {
        // A 300x300 rect at (100, 100), sheared along x; a tall target at x = 600.
        let run = |local: kurbo::Point, cmd: bool| {
            let mut d = Driver::new();
            d.app.snap = true;
            let id = d.app.doc.shapes()[0].id;
            let mut sh = d.app.doc.get(id).unwrap().clone();
            sh.transform = Affine::new([1.0, 0.0, 0.5, 1.0, 100.0, 100.0]);
            let start = sh.transform * local;
            d.app.doc.replace(sh);
            let mut b = Shape::new(ShapeKind::Rect { w: 100.0, h: 700.0, radii: [0.0; 4] });
            b.transform = Affine::translate((600.0, 37.0));
            d.app.doc.add_shape(b);
            d.app.selection = vec![id];
            d.frames(3, vec![]);
            let z = d.app.view.zoom as f64;
            let from = d.app.view.to_screen(start);
            let to = d.app.view.to_screen(kurbo::Point::new(600.0 - 2.0 / z, start.y));
            if cmd {
                d.mods = egui::Modifiers::COMMAND;
                d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
            }
            d.drag(from, to);
            assert!(d.app.snap_guides.is_empty(), "guides clear on release");
            let s = d.app.doc.get(id).unwrap();
            let ShapeKind::Rect { w, h, .. } = s.kind else { panic!() };
            (s.transform * kurbo::Point::new(w * local.x / 300.0, h * local.y / 300.0), z)
        };
        // East edge handle: it slides along x until it reaches the target.
        let (p, _) = run(kurbo::Point::new(300.0, 150.0), false);
        assert!((p.x - 600.0).abs() < 1e-3, "edge {p:?}");
        // South-east corner.
        let (p, _) = run(kurbo::Point::new(300.0, 300.0), false);
        assert!((p.x - 600.0).abs() < 1e-3, "corner {p:?}");
        // Ctrl turns snapping off.
        let (p, z) = run(kurbo::Point::new(300.0, 150.0), true);
        assert!((p.x - 600.0).abs() > 0.5 / z, "ctrl {p:?}");
    }

    #[test]
    fn eyedropper_samples_image_pixels() {
        use crate::document::{ImageAsset, ImageFill, ImageMode};
        let mut d = Driver::new();
        let id = d.app.doc.shapes()[0].id;
        // A green square below the image (which covers 100..400).
        let mut g = Shape::new(ShapeKind::Rect { w: 300.0, h: 300.0, radii: [0.0; 4] });
        g.transform = Affine::translate((100.0, 100.0));
        g.fill = Some(Paint::Solid(Rgba::from_hex("#00FF00").unwrap()));
        let gid = d.app.doc.add_shape(g);
        d.app.doc.move_to_index(gid, 0);
        // 2x2 image: red, transparent / blue, white.
        let px = vec![255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255];
        let asset = ImageAsset { width: 2, height: 2, encoded: vec![], rgba: Some(std::sync::Arc::new(px)) };
        d.app.doc.images.insert(7, asset);
        d.app.doc.get_mut(id).unwrap().fill = Some(Paint::Image(ImageFill { image: 7, mode: ImageMode::Stretch, scale: 1.0, opacity: 1.0 }));
        d.frames(3, vec![]);
        let at = |d: &mut Driver, x, y| d.app.sample_color(kurbo::Point::new(x, y)).unwrap().to_hex();
        assert_eq!(at(&mut d, 150.0, 150.0), "#FF0000");
        assert_eq!(at(&mut d, 150.0, 350.0), "#0000FF");
        assert_eq!(at(&mut d, 350.0, 350.0), "#FFFFFF");
        assert_eq!(at(&mut d, 350.0, 150.0), "#00FF00", "transparent pixel shows the shape below");
    }

    #[test]
    fn segment_drag_moves_ctrl_drag_bends() {
        let square = || {
            let mut b = kurbo::BezPath::new();
            b.move_to((100.0, 100.0));
            b.line_to((400.0, 100.0));
            b.line_to((400.0, 400.0));
            b.line_to((100.0, 400.0));
            b.close_path();
            let mut s = Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&b)));
            s.fill = Some(Paint::Solid(Rgba::BLACK));
            s
        };
        let nodes = |d: &Driver, id| match &d.app.doc.get(id).unwrap().kind {
            ShapeKind::Path(p) => p.subpaths[0].nodes.clone(),
            _ => panic!(),
        };
        for ctrl in [false, true] {
            let mut d = Driver::new();
            let first = d.app.doc.shapes()[0].id;
            d.app.doc.remove(first);
            let id = d.app.doc.add_shape(square());
            d.app.selection = vec![id];
            d.app.set_tool(crate::app::Tool::Node);
            d.app.snap = false;
            d.frames(3, vec![]);
            let before = nodes(&d, id);
            let from = d.app.view.to_screen(kurbo::Point::new(250.0, 100.0));
            let to = d.app.view.to_screen(kurbo::Point::new(250.0, 160.0));
            if ctrl {
                d.mods = egui::Modifiers::COMMAND;
                d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
            }
            d.drag(from, to);
            d.mods = Default::default();
            d.frame(vec![egui::Event::ModifiersChanged(d.mods)]);
            let after = nodes(&d, id);
            let top: Vec<_> = after.iter().filter(|n| n.p.y < 250.0).collect();
            assert_eq!(top.len(), 2);
            if ctrl {
                // Nodes stay; the segment curves.
                assert_eq!(after.iter().map(|n| n.p).collect::<Vec<_>>(), before.iter().map(|n| n.p).collect::<Vec<_>>());
                assert_ne!(after, before, "segment not bent");
            } else {
                // Both ends move together; the segment stays straight.
                for n in &top {
                    assert!((n.p.y - 160.0).abs() < 0.5, "{:?}", n.p);
                }
                let path = d.app.cache.get(id).unwrap().doc_path.clone();
                let bb = kurbo::Shape::bounding_box(&*path);
                assert!((bb.y0 - 160.0).abs() < 0.5, "segment bowed: {bb:?}");
                assert_eq!(after.iter().filter(|n| n.p.y > 250.0).count(), 2, "bottom nodes moved");
            }
        }
    }

    #[test]
    fn right_press_opens_context_menu_that_stays_open() {
        let mut d = Driver::new();
        // Right-press on the rectangle, wiggle a little, release.
        let p = d.app.view.to_screen(kurbo::Point::new(250.0, 250.0));
        d.move_to(p);
        d.button(egui::PointerButton::Secondary, true);
        d.move_to(p + egui::vec2(6.0, 4.0));
        assert!(d.app.ui.ctx_menu.is_some());
        d.button(egui::PointerButton::Secondary, false);
        d.frames(5, vec![]);
        assert!(d.app.ui.ctx_menu.is_some(), "menu closed on release");
        assert_eq!(d.app.selection.len(), 1);
        // A click elsewhere closes it.
        d.move_to(egui::pos2(300.0, 900.0));
        d.button(egui::PointerButton::Primary, true);
        d.button(egui::PointerButton::Primary, false);
        d.frames(3, vec![]);
        assert!(d.app.ui.ctx_menu.is_none());
    }

    #[test]
    fn boolean_ops_replace_selection() {
        for op in [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect, BoolOp::Exclude] {
            let (mut app, _, _) = app_with_squares();
            crate::commands::Cmd::all().into_iter().find(|c| c.meta().label == crate::ops::op_name(op)).unwrap().run(&mut app, &egui::Context::default());
            assert_eq!(app.doc.shapes().len(), 1, "{op:?}");
            assert_eq!(app.selection.len(), 1);
            // Non-destructive by default, like Figma.
            assert!(matches!(app.doc.shapes()[0].kind, ShapeKind::Compound { .. }));
            app.flatten();
            assert!(matches!(app.doc.shapes()[0].kind, ShapeKind::Path(_)));
            assert!(app.history.undo(&mut app.doc));
            assert!(app.history.undo(&mut app.doc));
            assert_eq!(app.doc.shapes().len(), 2);
        }
    }

    #[test]
    fn shape_builder_merge_and_subtract() {
        let (mut app, _, _) = app_with_squares();
        app.tool = Tool::ShapeBuilder;
        app.refresh_builder();
        let n = app.builder.as_ref().unwrap().regions.len();
        assert_eq!(n, 3);
        // Merge everything -> one shape.
        app.commit_builder(&[0, 1, 2], crate::app::BuilderMode::Add);
        assert_eq!(app.doc.shapes().len(), 1);

        let (mut app, _, _) = app_with_squares();
        app.refresh_builder();
        // Find the overlap region (owner = top shape, area ~ 50x50 quarter circle) and delete it.
        let regions = &app.builder.as_ref().unwrap().regions;
        let overlap = regions
            .iter()
            .position(|r| r.owner == 1 && boolean::area(&r.polys) < 2500.0)
            .unwrap();
        app.commit_builder(&[overlap], crate::app::BuilderMode::Subtract);
        assert_eq!(app.doc.shapes().len(), 2);
        // Colors preserved per owner.
        let colors: Vec<String> = app.doc.shapes().iter().map(|s| s.fill.as_ref().unwrap().solid().unwrap().to_hex()).collect();
        assert!(colors.contains(&"#FF0000".to_string()) && colors.contains(&"#0000FF".to_string()));
    }

    #[test]
    fn shape_builder_new_keeps_originals() {
        let (mut app, a, b) = app_with_squares();
        app.refresh_builder();
        let regions = &app.builder.as_ref().unwrap().regions;
        let overlap = regions.iter().position(|r| r.owner == 1 && boolean::area(&r.polys) < 2500.0).unwrap();
        app.commit_builder(&[overlap], crate::app::BuilderMode::New);
        assert_eq!(app.doc.shapes().len(), 3);
        assert!(app.doc.get(a).is_some() && app.doc.get(b).is_some());
        assert!(matches!(app.doc.shapes().last().unwrap().kind, ShapeKind::Path(_)));
        assert_eq!(app.selection, vec![a, b]);
    }

    #[test]
    fn convert_and_round_corners() {
        let (mut app, ia, _) = app_with_squares();
        app.selection = vec![ia];
        app.convert_to_curves();
        let ShapeKind::Path(pd) = &app.doc.get(ia).unwrap().kind else { panic!() };
        assert_eq!(pd.node_count(), 4);
        app.set_corner_radius_all(20.0);
        let s = app.doc.get(ia).unwrap().clone();
        let b = app.cache.update(&s, &mut app.fonts).doc_path.clone();
        let curves = b.elements().iter().filter(|e| matches!(e, kurbo::PathEl::CurveTo(..))).count();
        assert_eq!(curves, 4);
        // Expanding live corners bakes them into nodes.
        app.convert_to_curves();
        let ShapeKind::Path(pd) = &app.doc.get(ia).unwrap().kind else { panic!() };
        assert_eq!(pd.node_count(), 8);
    }

    #[test]
    fn transforms_bake_into_primitives() {
        let (mut app, ia, _) = app_with_squares();
        let mut s = app.doc.get(ia).unwrap().clone();
        s.apply_transform(Affine::translate((10.0, 10.0)) * Affine::scale_non_uniform(2.0, 0.5));
        assert_eq!(s.kind.size(), Some((200.0, 50.0)));
        s.apply_transform(Affine::rotate(0.3));
        assert_eq!(s.kind.size(), Some((200.0, 50.0)));
        app.doc.replace(s);
        app.selection = vec![ia];
        app.flip(true);
        app.duplicate_selection(10.0);
        assert_eq!(app.doc.shapes().len(), 3);
        app.delete_selection();
        assert_eq!(app.doc.shapes().len(), 2);
        app.align(0);
    }

    #[test]
    fn text_shape_outlines_and_convert() {
        let mut app = App::new(egui::Context::default(), None);
        let fam = app.fonts.families.iter().find(|f| f.contains("Sans")).cloned().unwrap_or(app.fonts.families[0].clone());
        let t = Shape::new(ShapeKind::Text(TextData { content: "Abc".into(), family: fam, ..Default::default() }));
        let id = app.doc.add_shape(t);
        app.selection = vec![id];
        app.convert_to_curves();
        let ShapeKind::Path(pd) = &app.doc.get(id).unwrap().kind else { panic!("not converted") };
        assert!(pd.node_count() > 10);
    }

    // ------------------------------------------------------------------
    // Command registry
    // ------------------------------------------------------------------

    #[test]
    fn command_shortcuts_are_unique_and_labeled() {
        use crate::commands::{Cmd, format_shortcut};
        let mut seen = std::collections::HashMap::new();
        for c in Cmd::all() {
            let m = c.meta();
            assert!(!m.label.is_empty(), "{c:?} has no label");
            assert!(!m.icon.is_empty(), "{c:?} has no icon");
            for k in m.keys {
                let key = format_shortcut(k);
                if let Some(prev) = seen.insert(key.clone(), c) {
                    panic!("shortcut {key} bound to both {prev:?} and {c:?}");
                }
            }
        }
        // Registry is complete: every command appears exactly once.
        let all = Cmd::all();
        let set: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(set.len(), all.len());
    }

    #[test]
    fn command_lookup_matches_presses() {
        use crate::commands::{Cmd, Press, lookup};
        use egui::{Key, Modifiers};
        let p = |key, mods| Press { key, physical: Some(key), mods };
        assert_eq!(lookup(&p(Key::Z, Modifiers::COMMAND)), Some(Cmd::Undo));
        assert_eq!(lookup(&p(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT)), Some(Cmd::Redo));
        assert_eq!(lookup(&p(Key::V, Modifiers::NONE)), Some(Cmd::Tool(Tool::Select)));
        assert_eq!(lookup(&p(Key::V, Modifiers::SHIFT)), Some(Cmd::FlipV));
        assert_eq!(lookup(&p(Key::V, Modifiers::COMMAND)), Some(Cmd::Paste));
        assert_eq!(lookup(&p(Key::V, Modifiers::ALT)), Some(Cmd::AlignVCenter));
        // "+" usually needs Shift; it must still zoom.
        assert_eq!(lookup(&p(Key::Plus, Modifiers::COMMAND | Modifiers::SHIFT)), Some(Cmd::ZoomIn));
        // Ctrl+Shift+] arrives as a different logical key on most layouts.
        let front = Press { key: Key::CloseCurlyBracket, physical: Some(Key::CloseBracket), mods: Modifiers::COMMAND | Modifiers::SHIFT };
        assert_eq!(lookup(&front), Some(Cmd::BringToFront));
        assert_eq!(lookup(&p(Key::Q, Modifiers::ALT)), None);
    }

    #[test]
    fn commands_enable_and_run_on_sample_doc() {
        use crate::commands::Cmd;
        let ctx = egui::Context::default();
        let (mut app, ia, ib) = app_with_squares();
        assert!(Cmd::Union.enabled(&app));
        assert!(!Cmd::DistributeH.enabled(&app), "needs 3 objects");
        assert!(!Cmd::PasteStyle.enabled(&app), "no style copied yet");
        Cmd::Duplicate.run(&mut app, &ctx);
        assert_eq!(app.doc.shapes().len(), 4);
        Cmd::Undo.run(&mut app, &ctx);
        assert_eq!(app.doc.shapes().len(), 2);
        app.selection = vec![ia];
        Cmd::CopyStyle.run(&mut app, &ctx);
        app.selection = vec![ib];
        assert!(Cmd::PasteStyle.enabled(&app));
        Cmd::PasteStyle.run(&mut app, &ctx);
        assert_eq!(app.doc.get(ib).unwrap().fill, app.doc.get(ia).unwrap().fill);
        Cmd::ToggleHidden.run(&mut app, &ctx);
        assert!(!app.doc.get(ib).unwrap().visible);
        Cmd::Tool(Tool::Star).run(&mut app, &ctx);
        assert_eq!(app.tool, Tool::Star);
        assert_eq!(app.shape_tool, Tool::Star);
        // Disabled commands are no-ops.
        app.selection.clear();
        Cmd::Delete.run(&mut app, &ctx);
        assert_eq!(app.doc.shapes().len(), 2);
        Cmd::SelectAll.run(&mut app, &ctx);
        assert_eq!(app.selection.len(), 1, "hidden shapes are not selected");
    }

    #[test]
    fn palette_fuzzy_search() {
        use crate::commands::fuzzy_score;
        assert!(fuzzy_score("cc", "Convert to Curves").is_some());
        assert!(fuzzy_score("xyz", "Convert to Curves").is_none());
        assert!(fuzzy_score("exp", "Expand Stroke").unwrap() > fuzzy_score("exp", "Export SVG…").unwrap() - 100);
        let app = App::new(egui::Context::default(), None);
        let m: Vec<crate::commands::Cmd> = crate::ui::palette::matches(&app, "zoom fit").into_iter().map(|x| x.0).collect();
        assert_eq!(m.first(), Some(&crate::commands::Cmd::ZoomFit));
    }

    // ------------------------------------------------------------------
    // Platform: URI lists and clipboard payloads
    // ------------------------------------------------------------------

    #[test]
    fn uri_list_parsing() {
        use crate::platform::parse_uri_list;
        let list = "# comment\r\nfile:///home/me/My%20Logo.svg\r\nfile://localhost/tmp/a%25b.png\nhttps://example.com/x.png\n\n";
        let files = parse_uri_list(list);
        assert_eq!(files, vec![std::path::PathBuf::from("/home/me/My Logo.svg"), std::path::PathBuf::from("/tmp/a%b.png")]);
        assert_eq!(parse_uri_list("file:///t/%C5%BC%C3%B3%C5%82w.pdf"), vec![std::path::PathBuf::from("/t/żółw.pdf")]);
    }

    #[test]
    fn clipboard_payload_roundtrip() {
        use crate::platform::clipboard::{self, ClipContent};
        let doc = sample_doc();
        let mut app = App::new(egui::Context::default(), None);
        app.doc = doc;
        // Rect with stroke (index 0) and the image-filled rect (index 3).
        let ids = vec![app.doc.shapes()[0].id, app.doc.shapes()[3].id];
        let payload = clipboard::build_payload(&app.doc, &ids);
        assert_eq!(payload.shapes.len(), 2);
        assert_eq!(payload.images.len(), 1);
        let json = clipboard::payload_to_json(&payload);
        let back = match clipboard::decode(clipboard::MIME_INTERNAL, json).unwrap() {
            ClipContent::Internal(p) => p,
            _ => panic!("expected internal payload"),
        };
        assert_eq!(back.shapes.len(), 2);
        assert!(back.images.values().all(|i| i.rgba.is_some()), "images decoded");
        // Paste into the document: images remapped, shapes offset.
        let before = app.doc.shapes().len();
        app.paste_content(ClipContent::Internal(back), Some(kurbo::Point::new(500.0, 500.0)));
        assert_eq!(app.doc.shapes().len(), before + 2);
        for id in &app.selection {
            if let Some(Paint::Image(f)) = &app.doc.get(*id).unwrap().fill {
                assert!(app.doc.images.contains_key(&f.image));
            }
        }
        let b = app.selection_bounds().unwrap();
        assert!((b.center().x - 500.0).abs() < 1.0 && (b.center().y - 500.0).abs() < 1.0, "{b:?}");
    }

    #[test]
    fn clipboard_svg_is_cropped_to_selection() {
        use crate::platform::clipboard;
        let doc = sample_doc();
        let mut fonts = fonts();
        // Rect 100x80 at (20,30) with a 3px stroke.
        let payload = clipboard::build_payload(&doc, &[doc.shapes()[0].id]);
        let svg = clipboard::payload_svg(&payload, &mut fonts);
        assert!(clipboard::looks_like_svg(&svg));
        let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap();
        assert!((tree.size().width() - 103.0).abs() < 1.01, "width {}", tree.size().width());
        assert!((tree.size().height() - 83.0).abs() < 1.01, "height {}", tree.size().height());
        let bb = tree.root().abs_bounding_box();
        assert!(bb.left() >= -0.01 && bb.top() >= -0.01, "{bb:?}");
        // MIME priority.
        let offered = vec!["text/plain".to_string(), "image/svg+xml".to_string(), "image/png".to_string()];
        assert_eq!(clipboard::choose_mime(&offered).as_deref(), Some("image/svg+xml"));
        let offered = vec!["text/plain".to_string(), clipboard::MIME_INTERNAL.to_string(), "image/svg+xml".to_string()];
        assert_eq!(clipboard::choose_mime(&offered).as_deref(), Some(clipboard::MIME_INTERNAL));
    }

    // ------------------------------------------------------------------
    // Utilities
    // ------------------------------------------------------------------

    #[test]
    fn expand_stroke_outlines() {
        let mut app = App::new(egui::Context::default(), None);
        let mut r = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
        r.fill = None;
        r.stroke = Some(Stroke { width: 10.0, color: Rgba::from_hex("#FF0000").unwrap(), ..Default::default() });
        let id = app.doc.add_shape(r);
        app.selection = vec![id];
        app.expand_stroke();
        assert_eq!(app.doc.shapes().len(), 1);
        let s = app.doc.shapes()[0].clone();
        assert!(s.stroke.is_none());
        assert_eq!(s.fill.as_ref().and_then(|f| f.solid()).map(|c| c.to_hex()).as_deref(), Some("#FF0000"));
        let path = app.cache.update(&s, &mut app.fonts).doc_path.clone();
        let b = geom::path_bounds(&path).unwrap();
        assert!((b.width() - 110.0).abs() < 0.5 && (b.height() - 110.0).abs() < 0.5, "{b:?}");
        // Ring: area = 110² − 90².
        let area = boolean::area(&boolean::path_to_polys(&path, s.fill_rule));
        assert!((area - (110.0f64 * 110.0 - 90.0 * 90.0)).abs() < 40.0, "area {area}");
        // The center is not filled, the band is.
        assert_eq!(app.hit_shapes(kurbo::Point::new(50.0, 50.0), true).len(), 0);
        assert_eq!(app.hit_shapes(kurbo::Point::new(1.0, 50.0), true), vec![s.id]);

        // With a fill, the fill stays underneath the outline.
        let mut app = App::new(egui::Context::default(), None);
        let mut e = Shape::new(ShapeKind::Ellipse { w: 80.0, h: 80.0 });
        e.stroke = Some(Stroke { width: 4.0, ..Default::default() });
        let id = app.doc.add_shape(e);
        app.selection = vec![id];
        app.expand_stroke();
        assert_eq!(app.doc.shapes().len(), 2);
        assert!(app.doc.shapes()[0].stroke.is_none() && app.doc.shapes()[0].fill.is_some());
        assert_eq!(app.selection.len(), 2);
    }

    #[test]
    fn multi_hit_returns_all_shapes_top_first() {
        let (mut app, ia, ib) = app_with_squares();
        app.view = crate::render::View { zoom: 1.0, origin: egui::vec2(0.0, 0.0) };
        // (75,75) is inside both the square and the circle.
        assert_eq!(app.hit_shapes(kurbo::Point::new(75.0, 75.0), true), vec![ib, ia]);
        assert_eq!(app.hit_shape(kurbo::Point::new(75.0, 75.0)), Some(ib));
        assert_eq!(app.hit_shapes(kurbo::Point::new(10.0, 10.0), true), vec![ia]);
        app.doc.get_mut(ib).unwrap().locked = true;
        assert_eq!(app.hit_shapes(kurbo::Point::new(75.0, 75.0), true), vec![ia]);
        assert_eq!(app.hit_shapes(kurbo::Point::new(75.0, 75.0), false), vec![ib, ia]);
        // Context menu on an unselected shape selects it.
        app.selection.clear();
        app.open_context_menu(egui::pos2(10.0, 10.0), false);
        assert_eq!(app.selection, vec![ia]);
        assert_eq!(app.ui.ctx_menu.as_ref().unwrap().hits, vec![ia]);
        app.open_context_menu(egui::pos2(300.0, 300.0), false);
        assert!(app.selection.is_empty());
    }

    #[test]
    fn distribute_and_break_nodes() {
        let mut app = App::new(egui::Context::default(), None);
        let mut ids = Vec::new();
        for (x, w) in [(0.0, 10.0), (17.0, 30.0), (100.0, 20.0)] {
            let mut r = Shape::new(ShapeKind::Rect { w, h: 10.0, radii: [0.0; 4] });
            r.transform = Affine::translate((x, 0.0));
            ids.push(app.doc.add_shape(r));
        }
        app.selection = ids.clone();
        app.distribute(true);
        let b = app.cache.update(&app.doc.get(ids[1]).unwrap().clone(), &mut app.fonts).bounds.unwrap();
        // Total span 120, widths 60 → gaps of 30.
        assert!((b.x0 - 40.0).abs() < 1e-6, "{b:?}");

        // Break a closed square at node 0 and 2 → two open pieces.
        let sp = SubPath { nodes: (0..4).map(|i| Node::corner(kurbo::Point::new([0.0, 1.0, 1.0, 0.0][i], [0.0, 0.0, 1.0, 1.0][i]))).collect(), closed: true };
        let parts = crate::ops::break_subpath(&sp, &[0, 2]);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| !p.closed));
        assert_eq!(parts.iter().map(|p| p.nodes.len()).sum::<usize>(), 6);
        // Break an open path in the middle.
        let open = SubPath { nodes: sp.nodes.clone(), closed: false };
        let parts = crate::ops::break_subpath(&open, &[1]);
        assert_eq!(parts.iter().map(|p| p.nodes.len()).collect::<Vec<_>>(), vec![2, 3]);
    }
}


/// Real system clipboard round trip (Wayland data-control). Overwrites the
/// clipboard, so it only runs on demand: `cargo test system_clipboard -- --ignored`.
#[test]
#[ignore]
fn system_clipboard_offers_svg() {
    use crate::platform::clipboard::{self, ClipContent, SystemClipboard};
    let cb = SystemClipboard::probe();
    assert!(cb.available(), "data-control protocol not available");
    let doc = sample_doc();
    let mut fonts = fonts();
    let payload = clipboard::build_payload(&doc, &[doc.shapes()[0].id, doc.shapes()[1].id]);
    let svg = clipboard::payload_svg(&payload, &mut fonts);
    cb.copy(vec![
        (clipboard::MIME_INTERNAL, clipboard::payload_to_json(&payload)),
        (clipboard::MIME_SVG, svg.clone().into_bytes()),
        ("text/plain", svg.clone().into_bytes()),
    ])
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents, get_mime_types};
    let mimes = get_mime_types(ClipboardType::Regular, Seat::Unspecified).unwrap();
    eprintln!("offered: {mimes:?}");
    assert!(mimes.contains(clipboard::MIME_SVG) && mimes.contains(clipboard::MIME_INTERNAL) && mimes.contains("text/plain"));
    let (mut pipe, _) = get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Specific(clipboard::MIME_SVG)).unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut pipe, &mut data).unwrap();
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).expect("SVG from clipboard parses");
    eprintln!("svg size {:?}, {} bytes", tree.size(), data.len());
    assert!(tree.root().has_children());
    // Our own reader prefers the lossless payload.
    match cb.read() {
        Some(ClipContent::Internal(p)) => assert_eq!(p.shapes.len(), 2),
        _ => panic!("expected internal payload"),
    }
}

/// Clip, compound, outline/blur/noise effects, text on path, Iconify.
/// Run just these with `cargo test fx_`.
mod fx_tests {
    use super::*;
    use kurbo::BezPath;
    use crate::app::App;
    use crate::boolean::BoolOp;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Shape {
        let mut s = Shape::new(ShapeKind::Rect { w, h, radii: [0.0; 4] });
        s.transform = Affine::translate((x, y));
        s
    }

    fn app_with(shapes: Vec<Shape>) -> (App, Vec<u64>) {
        let mut app = App::new(egui::Context::default(), None);
        let ids: Vec<u64> = shapes.into_iter().map(|s| app.doc.add_shape(s)).collect();
        app.selection = ids.clone();
        (app, ids)
    }

    fn fill_area(app: &mut App, id: u64) -> f64 {
        let s = app.doc.get(id).unwrap().clone();
        let c = app.cache.update(&s, &mut app.fonts);
        boolean::area(&boolean::path_to_polys(&c.fill_path, c.fill_rule))
    }

    #[test]
    fn fx_components_sync_overrides_and_detach() {
        use crate::document::Component;
        let mut red = rect(0.0, 0.0, 100.0, 100.0);
        red.fill = Some(Paint::Solid(Rgba::from_hex("#FF0000").unwrap()));
        let (mut app, ids) = app_with(vec![red, rect(120.0, 0.0, 50.0, 50.0)]);
        app.create_component();
        let main = app.selection[0];
        assert!(app.is_main(main));
        assert_eq!(app.doc.get(main).unwrap().name, "Component 1");
        let inst = app.insert_instance(main, kurbo::Point::new(1085.0, 550.0)).unwrap();
        let tick = |app: &mut App| {
            for _ in 0..4 {
                app.components_tick();
            }
        };
        tick(&mut app);
        let kids = |app: &App, id: u64| app.doc.get(id).unwrap().kind.children().unwrap().clone();
        let ik = kids(&app, inst);
        assert_eq!(ik.len(), 2);
        assert!(ik.iter().all(|c| !ids.contains(&c.id)), "instance layers have their own ids");
        // Instance placed with its centre at the point: (1000,500) for the main's (0,0).
        let b = { let s = app.doc.get(inst).unwrap().clone(); app.cache.update(&s, &mut app.fonts).bounds.unwrap() };
        assert_eq!((b.x0, b.y0), (1000.0, 500.0));
        // Editing the main reaches the instance.
        let blue = Some(Paint::Solid(Rgba::from_hex("#0000FF").unwrap()));
        app.doc.get_mut(ids[1]).unwrap().fill = blue.clone();
        tick(&mut app);
        assert_eq!(kids(&app, inst)[1].fill, blue);
        // Moving the whole main doesn't move its instances…
        app.translate_ids(&[main], kurbo::Vec2::new(0.0, 300.0));
        tick(&mut app);
        assert_eq!(kids(&app, inst)[0].transform.translation(), kurbo::Vec2::new(1000.0, 500.0));
        // …but moving a layer inside it does.
        app.translate_ids(&[ids[0]], kurbo::Vec2::new(10.0, 0.0));
        tick(&mut app);
        assert_eq!(kids(&app, inst)[0].transform.translation(), kurbo::Vec2::new(1010.0, 500.0));
        // Resizing the main resizes its instances (anchored at their corner).
        let o = match app.doc.get(main).unwrap().component { Some(Component::Main { frame, .. }) => frame.translation(), _ => unreachable!() };
        let grow = Affine::translate(o) * Affine::scale(2.0) * Affine::translate(-o);
        let mut m = app.doc.get(main).unwrap().clone();
        m.apply_transform(grow);
        app.doc.replace(m);
        tick(&mut app);
        assert_eq!(kids(&app, inst)[0].kind.size(), Some((200.0, 200.0)));
        assert_eq!(kids(&app, inst)[0].transform.translation(), kurbo::Vec2::new(1020.0, 500.0));
        let mut m = app.doc.get(main).unwrap().clone();
        m.apply_transform(grow.inverse());
        app.doc.replace(m);
        tick(&mut app);
        assert_eq!(kids(&app, inst)[0].kind.size(), Some((100.0, 100.0)));
        // Restyling an instance layer is an override that survives main edits.
        let green = Some(Paint::Solid(Rgba::from_hex("#00FF00").unwrap()));
        let layer = kids(&app, inst)[0].id;
        app.doc.get_mut(layer).unwrap().fill = green.clone();
        tick(&mut app);
        assert_eq!(app.override_count(inst), 1);
        app.doc.get_mut(ids[0]).unwrap().fill = blue.clone();
        app.doc.get_mut(ids[0]).unwrap().opacity = 0.5;
        tick(&mut app);
        let k0 = kids(&app, inst)[0].clone();
        assert_eq!((k0.fill.clone(), k0.opacity, k0.id), (green.clone(), 0.5, layer));
        // Deleting a layer of an instance hides it instead.
        app.doc.remove(kids(&app, inst)[1].id);
        tick(&mut app);
        assert_eq!(kids(&app, inst).len(), 2);
        assert!(!kids(&app, inst)[1].visible);
        // Reset drops the overrides.
        app.selection = vec![layer];
        app.reset_instance();
        tick(&mut app);
        assert_eq!(app.override_count(inst), 0);
        assert_eq!(kids(&app, inst)[0].fill, blue);
        assert!(kids(&app, inst)[1].visible);
        // Copy + paste of the main, and Ctrl+D of an instance, make instances.
        let p = crate::platform::clipboard::build_payload(&app.doc, &[main]);
        let pasted = app.insert_payload(&p)[0];
        assert_eq!(app.instance_link(pasted).map(|l| l.main), Some(main));
        app.selection = vec![inst];
        app.duplicate_selection(10.0);
        let dup = app.selection[0];
        tick(&mut app);
        assert_eq!(app.override_count(dup), 0, "remapped layer ids, no phantom overrides");
        assert_eq!(app.instances_of(main).len(), 3);
        // Save / load keeps the links and overrides.
        app.doc.get_mut(kids(&app, dup)[0].id).unwrap().name = "Renamed".into();
        tick(&mut app);
        assert_eq!(app.override_count(dup), 1, "rename recorded");
        let path = std::env::temp_dir().join("mtzd-components.mvec");
        io::save_native(&app.doc, &path).unwrap();
        let mut back = App::new(egui::Context::default(), None);
        back.doc = io::load_native(&path).unwrap();
        let before = back.doc.shapes().to_vec();
        tick(&mut back);
        assert_eq!(back.override_count(dup), 1);
        assert_eq!(back.doc.get(dup).unwrap().kind.children().unwrap()[0].name, "Renamed");
        assert_eq!(back.doc.shapes().len(), before.len());
        // Deleting the main detaches its instances.
        app.doc.remove(main);
        tick(&mut app);
        assert!(app.instance_link(inst).is_none());
        assert!(matches!(app.doc.get(inst).unwrap().kind, ShapeKind::Group { .. }));
        assert!(app.doc.get(inst).unwrap().component.is_none());
        let _ = Component::main(Affine::IDENTITY);
    }

    #[test]
    fn fx_compound_is_live_and_releasable() {
        let (mut app, _) = app_with(vec![rect(0.0, 0.0, 100.0, 100.0), rect(50.0, 0.0, 100.0, 100.0)]);
        app.make_compound(BoolOp::Union);
        assert_eq!(app.doc.shapes().len(), 1);
        let id = app.selection[0];
        assert!((fill_area(&mut app, id) - 15000.0).abs() < 1.0);
        // Change the operation: geometry follows without baking.
        let mut s = app.doc.get(id).unwrap().clone();
        if let ShapeKind::Compound { op, .. } = &mut s.kind {
            *op = BoolOp::Subtract;
        }
        app.doc.replace(s);
        assert!((fill_area(&mut app, id) - 5000.0).abs() < 1.0);
        // Moving the compound moves its children.
        app.doc.get_mut(id).unwrap().apply_transform(Affine::translate((10.0, 0.0)));
        let s = app.doc.get(id).unwrap().clone();
        assert!((app.cache.update(&s, &mut app.fonts).bounds.unwrap().x0 - 10.0).abs() < 1e-6);
        // SVG export bakes the result.
        let svg = io::svg::export(&app.doc, &mut app.fonts);
        assert!(svg.contains("<path"));
        app.ungroup();
        assert_eq!(app.doc.shapes().len(), 2);
        assert!(app.doc.shapes().iter().all(|s| matches!(s.kind, ShapeKind::Rect { .. })));
        // Convert to curves bakes a compound into a path.
        app.select_all();
        app.make_compound(BoolOp::Intersect);
        app.convert_to_curves();
        assert!(matches!(app.doc.shapes()[0].kind, ShapeKind::Path(_)));
    }

    #[test]
    fn group_ungroup_and_nested_editing() {
        let mut r1 = rect(0.0, 0.0, 50.0, 50.0);
        r1.fill = Some(Paint::Solid(Rgba::from_hex("#FF0000").unwrap()));
        let (mut app, ids) = app_with(vec![rect(-100.0, 0.0, 10.0, 10.0), r1, rect(100.0, 0.0, 50.0, 50.0)]);
        app.selection = vec![ids[1], ids[2]];
        crate::commands::Cmd::Group.run(&mut app, &egui::Context::default());
        assert_eq!(app.doc.shapes().len(), 2);
        let gid = app.selection[0];
        // The group sits where the top-most member was.
        assert_eq!(app.doc.index_of(gid), Some(1));
        assert_eq!(app.doc.parent_of(ids[1]), Some(gid));
        // Bounds span the children; clicks on a child pick the group; gaps miss.
        let g = app.doc.get(gid).unwrap().clone();
        let b = app.cache.update(&g, &mut app.fonts).bounds.unwrap();
        assert!((b.x0 - 0.0).abs() < 1e-6 && (b.x1 - 150.0).abs() < 1e-6, "{b:?}");
        app.selection.clear();
        assert_eq!(app.pick(kurbo::Point::new(10.0, 10.0), false), Some(gid));
        assert_eq!(app.hit_shape(kurbo::Point::new(75.0, 10.0)), None);
        assert_eq!(app.pick(kurbo::Point::new(110.0, 10.0), true), Some(ids[2]));
        // Moving the group moves the children.
        app.doc.get_mut(gid).unwrap().apply_transform(Affine::translate((5.0, 0.0)));
        let c = app.doc.get(ids[1]).unwrap().clone();
        assert!((app.cache.update(&c, &mut app.fonts).bounds.unwrap().x0 - 5.0).abs() < 1e-6);
        // Booleans inside groups; grouping a child of a group nests.
        app.selection = vec![ids[1]];
        app.group_selection();
        let inner = app.selection[0];
        assert_eq!(app.doc.parent_of(inner), Some(gid));
        // Ungroup carries group opacity to the children.
        app.doc.get_mut(gid).unwrap().opacity = 0.5;
        let svg = io::svg::export(&app.doc, &mut app.fonts);
        assert_eq!(svg.matches("<path").count(), 3, "grouped export: {svg}");
        assert_eq!(svg.matches("opacity=\"0.5\"").count(), 2, "{svg}");
        app.selection = vec![gid];
        app.ungroup();
        assert_eq!(app.doc.shapes().len(), 3);
        assert!(app.selection.contains(&inner));
        assert_eq!(app.doc.get(ids[2]).unwrap().opacity, 0.5);
        // Exports flatten groups.
        let svg = io::svg::export(&app.doc, &mut app.fonts);
        assert_eq!(svg.matches("<path").count(), 3, "{svg}");
    }

    #[test]
    fn fx_boolean_children_are_editable() {
        let (mut app, ids) = app_with(vec![rect(0.0, 0.0, 100.0, 100.0), rect(50.0, 0.0, 100.0, 100.0)]);
        app.make_compound(BoolOp::Subtract);
        let cid = app.selection[0];
        assert_eq!(app.doc.get(cid).unwrap().name, "Subtract");
        // Children keep their ids and are reachable.
        assert_eq!(app.doc.parent_of(ids[1]), Some(cid));
        assert_eq!(app.doc.root_of(ids[1]), Some(cid));
        assert!((fill_area(&mut app, cid) - 5000.0).abs() < 1.0);
        // Moving a child re-computes the boolean.
        app.doc.get_mut(ids[1]).unwrap().apply_transform(Affine::translate((50.0, 0.0)));
        assert!((fill_area(&mut app, cid) - 10000.0).abs() < 1.0);
        // replace() on a child works too.
        let mut c = app.doc.get(ids[1]).unwrap().clone();
        c.transform = Affine::translate((25.0, 0.0));
        assert!(app.doc.replace(c));
        assert!((fill_area(&mut app, cid) - 2500.0).abs() < 1.0);
        // Click picks the boolean; Ctrl+click the child; with a child selected a
        // plain click picks siblings.
        let inside_b = kurbo::Point::new(110.0, 50.0);
        let inside_a = kurbo::Point::new(10.0, 50.0);
        app.selection.clear();
        assert_eq!(app.pick(inside_a, false), Some(cid));
        assert_eq!(app.pick(inside_a, true), Some(ids[0]));
        app.selection = vec![ids[0]];
        // The subtracted area is outside the result, but with a child
        // selected the cutter there is still picked (so it can be dragged).
        assert_eq!(app.hit_shape(inside_b), None);
        assert_eq!(app.pick(inside_b, false), Some(ids[1]));
        // Duplicating a boolean gives its children fresh ids.
        let dup = app.doc.get(cid).unwrap().clone();
        let nid = app.doc.add_shape(dup);
        let all = app.doc.all_ids();
        let uniq: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(all.len(), uniq.len(), "{all:?}");
        app.doc.remove(nid);
        // Deleting the children removes the empty boolean.
        app.doc.remove(ids[0]);
        assert!(app.doc.get(cid).is_some());
        app.doc.remove(ids[1]);
        assert!(app.doc.get(cid).is_none());
        assert!(app.doc.shapes().is_empty());
    }

    #[test]
    fn fx_clip_masks_render_hits_and_export() {
        let mut big = rect(0.0, 0.0, 200.0, 200.0);
        big.stroke = Some(Stroke { width: 10.0, ..Default::default() });
        let (mut app, ids) = app_with(vec![big, rect(50.0, 50.0, 50.0, 50.0)]);
        app.make_clip();
        assert_eq!(app.doc.shapes().len(), 1);
        let id = ids[0];
        assert!(app.doc.get(id).unwrap().clip.is_some());
        assert!((fill_area(&mut app, id) - 2500.0).abs() < 1.0);
        let s = app.doc.get(id).unwrap().clone();
        let b = app.cache.update(&s, &mut app.fonts).bounds.unwrap();
        assert!((b.x0 - 50.0).abs() < 1e-6 && (b.x1 - 100.0).abs() < 1e-6, "{b:?}");
        // Hit testing respects the mask.
        assert_eq!(app.hit_shape(kurbo::Point::new(75.0, 75.0)), Some(id));
        assert_eq!(app.hit_shape(kurbo::Point::new(10.0, 10.0)), None);
        // Moving moves the mask along.
        app.doc.get_mut(id).unwrap().apply_transform(Affine::translate((5.0, 0.0)));
        assert_eq!(app.hit_shape(kurbo::Point::new(102.0, 75.0)), Some(id));
        let svg = io::svg::export(&app.doc, &mut app.fonts);
        assert!(svg.contains("<clipPath id=\"mask0\"") && svg.contains("clip-path=\"url(#mask0)\""), "{svg}");
        let pdf = io::pdf::export(&app.doc, &mut app.fonts);
        assert!(!pdf.is_empty());
        // Serde roundtrip keeps the clip.
        let json = serde_json::to_string(&app.doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();
        assert!(back.shapes()[0].clip.is_some());
        app.selection = vec![id];
        app.release_clip();
        assert_eq!(app.doc.shapes().len(), 2);
        assert!(app.doc.get(id).unwrap().clip.is_none());
    }

    #[test]
    fn fx_gradient_outlines_render_export_and_expand() {
        use crate::document::{Gradient, GradientKind};
        let red = Rgba::from_hex("#FF0000").unwrap();
        let blue = Rgba::from_hex("#0000FF").unwrap();
        let mut s = rect(0.0, 0.0, 100.0, 100.0);
        let mut step = OutlineStep::new(Rgba::WHITE, 10.0);
        step.gradient = Some(Gradient::new(GradientKind::Linear, red, blue));
        s.outlines = vec![OutlineStep::new(Rgba::BLACK, 4.0), step];
        // Serde keeps the gradient (and plain steps stay compact).
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json.matches("\"gradient\"").count(), 1, "{json}");
        let back: Shape = serde_json::from_str(&json).unwrap();
        assert!(back.outlines[1].gradient.is_some());
        let (mut app, ids) = app_with(vec![s]);
        let s = app.doc.get(ids[0]).unwrap().clone();
        let c = app.cache.update(&s, &mut app.fonts);
        assert!(matches!(c.rings[0].1, Paint::Gradient(_)), "outermost ring is the gradient step");
        let ring = c.rings[0].0.clone();
        // The gradient spans the ring's own box: red at its left edge.
        let lb = crate::render::ring_bounds(&s, &ring);
        let Paint::Gradient(g) = &c.rings[0].1 else { unreachable!() };
        let gs = crate::render::GradSampler::new(&s, lb, g, 1.0);
        let left = gs.premul_at(kurbo::Point::new(lb.x0 + 0.5, 50.0));
        assert!(left[0] > 0.95 && left[2] < 0.05, "{left:?}");
        // Exports: SVG gradient def for the ring, PDF writes.
        let svg = io::svg::export(&app.doc, &mut app.fonts);
        assert!(svg.contains("id=\"ring0_0\"") && svg.contains("fill=\"url(#ring0_0)\""), "{svg}");
        assert!(!io::pdf::export(&app.doc, &mut app.fonts).is_empty());
        // Expand: two ring paths under the shape, which loses its effect.
        app.selection = vec![ids[0]];
        app.expand_outlines();
        assert_eq!(app.doc.shapes().len(), 3);
        assert!(app.doc.get(ids[0]).unwrap().outlines.is_empty());
        assert_eq!(app.doc.shapes()[2].id, ids[0], "original stays on top");
        let baked = app.doc.shapes()[0].clone();
        assert!(matches!(baked.kind, ShapeKind::Path(_)));
        let Some(Paint::Gradient(bg)) = &baked.fill else { panic!("baked ring keeps its gradient") };
        let path = app.cache.update(&baked, &mut app.fonts).doc_path.clone();
        let bs = crate::render::GradSampler::new(&baked, crate::render::shape_local_bounds(&baked, &path), bg, 1.0);
        for x in [lb.x0 + 0.5, 50.0, lb.x1 - 0.5] {
            let (a, b) = (gs.premul_at(kurbo::Point::new(x, 50.0)), bs.premul_at(kurbo::Point::new(x, 50.0)));
            assert!((0..4).all(|k| (a[k] - b[k]).abs() < 0.02), "{x}: {a:?} vs {b:?}");
        }
        assert_eq!(app.doc.shapes()[1].fill, Some(Paint::Solid(Rgba::BLACK)));
        assert_eq!(app.selection.len(), 3);
        app.undo();
        assert_eq!(app.doc.shapes().len(), 1);
    }

    #[test]
    fn fx_outline_steps_build_rings() {
        let mut s = rect(0.0, 0.0, 100.0, 100.0);
        s.outlines = vec![OutlineStep::new(Rgba::WHITE, 5.0), OutlineStep::new(Rgba::BLACK, 10.0)];
        let (mut app, ids) = app_with(vec![s]);
        let s = app.doc.get(ids[0]).unwrap().clone();
        let c = app.cache.update(&s, &mut app.fonts);
        assert_eq!(c.rings.len(), 2);
        // Outermost first: the black ring spans 5..15 outside the edge.
        assert_eq!(c.rings[0].1, Paint::Solid(Rgba::BLACK));
        let outer = boolean::bounds(&boolean::path_to_polys(&c.rings[0].0, FillRule::EvenOdd)).unwrap();
        assert!((outer.x0 + 15.0).abs() < 0.2, "{outer:?}");
        let inner_area = boolean::area(&boolean::path_to_polys(&c.rings[1].0, FillRule::EvenOdd));
        // Rounded 5px ring around a 100x100 square: 4*100*5 + pi*25.
        assert!((inner_area - (2000.0 + std::f64::consts::PI * 25.0)).abs() < 5.0, "{inner_area}");
        // Hits in the ring area select the shape.
        assert_eq!(app.hit_shape(kurbo::Point::new(-10.0, 50.0)), Some(ids[0]));
        let svg = io::svg::export(&app.doc, &mut app.fonts);
        assert_eq!(svg.matches("fill-rule=\"evenodd\"").count(), 2, "{svg}");
    }

    #[test]
    fn fx_blur_and_noise_export_filters() {
        let mut s = rect(0.0, 0.0, 100.0, 100.0);
        s.blur = 8.0;
        s.noise = 0.3;
        let (mut app, _) = app_with(vec![s]);
        let svg = io::svg::export(&app.doc, &mut app.fonts);
        assert!(svg.contains("feGaussianBlur stdDeviation=\"4\""), "{svg}");
        assert!(svg.contains("feTurbulence"));
        assert!(svg.contains("url(#noise0)") && svg.contains("url(#blur0)"));
        // Style copy carries the effects.
        app.copy_style();
        let id = app.doc.add_shape(rect(200.0, 0.0, 10.0, 10.0));
        app.selection = vec![id];
        app.paste_style();
        let t = app.doc.get(id).unwrap();
        assert_eq!((t.blur, t.noise), (8.0, 0.3));
    }

    #[test]
    fn fx_text_follows_path() {
        let mut f = fonts();
        let mut guide = BezPath::new();
        guide.move_to((0.0, 100.0));
        guide.line_to((400.0, 100.0));
        let straight = TextData { content: "Wavy".into(), family: f.families[0].clone(), ..Default::default() };
        let flat = f.layout(&straight);
        let on = TextData { path: Some(geom::bez_to_pathdata(&guide)), path_offset: 20.0, ..straight.clone() };
        let l = f.layout(&on);
        let gb = geom::path_bounds(&l.path).unwrap();
        // Glyphs sit above the baseline at y=100, starting at the offset.
        assert!(gb.y1 <= 100.0 + straight.size * 0.35 && gb.y0 > 100.0 - straight.size * 1.2, "{gb:?}");
        assert!((gb.x0 - 20.0).abs() < straight.size * 0.3, "{gb:?}");
        assert!((gb.width() - geom::path_bounds(&flat.path).unwrap().width()).abs() < 1.0);
        let op = l.on_path.as_ref().unwrap();
        assert_eq!(op.len(), l.carets.len());
        // Vertical guide: glyphs rotate by 90°.
        let mut down = BezPath::new();
        down.move_to((50.0, 0.0));
        down.line_to((50.0, 400.0));
        let v = f.layout(&TextData { path: Some(geom::bez_to_pathdata(&down)), ..straight.clone() });
        let vb = geom::path_bounds(&v.path).unwrap();
        assert!(vb.height() > vb.width(), "{vb:?}");

        // Command flow: text + path → path text; release restores a guide.
        let mut app = App::new(egui::Context::default(), None);
        let mut ts = Shape::new(ShapeKind::Text(straight));
        ts.transform = Affine::translate((10.0, 10.0));
        let tid = app.doc.add_shape(ts);
        let gid = app.doc.add_shape(Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&guide))));
        app.selection = vec![tid, gid];
        assert!(crate::commands::Cmd::TextOnPath.enabled(&app));
        app.text_on_path();
        assert_eq!(app.doc.shapes().len(), 1);
        let s = app.doc.get(tid).unwrap().clone();
        let b = app.cache.update(&s, &mut app.fonts).bounds.unwrap();
        // Guide was in document space; text transform is undone.
        assert!(b.y0 < 100.0 && b.y1 <= 110.0 && (b.width() - 400.0).abs() < 1e-6, "{b:?}");
        app.release_text_path();
        assert_eq!(app.doc.shapes().len(), 2);
    }

    #[test]
    fn fx_iconify_urls_parse_and_mesh() {
        use crate::icons;
        assert_eq!(icons::search_url("arrow left", "mdi", 32), "https://api.iconify.design/search?query=arrow+left&limit=32&prefix=mdi");
        assert_eq!(icons::svg_url("mdi:home", "#1E1E1E").unwrap(), "https://api.iconify.design/mdi/home.svg?height=96&color=%231E1E1E");
        assert!(icons::svg_url("nocolon", "#000").is_none());
        let r = icons::parse_search(br#"{"icons":["mdi:home","ph:house"],"total":2,"limit":64}"#).unwrap();
        assert_eq!(r, vec!["mdi:home", "ph:house"]);
        assert!(icons::parse_search(b"{}").is_err());
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96" viewBox="0 0 24 24"><path fill="#1E1E1E" d="M10 20v-6h4v6h5v-8h3L12 3L2 12h3v8z"/></svg>"##;
        let mut f = fonts();
        let m = icons::icon_mesh(svg, &mut f).unwrap();
        assert!(!m.indices.is_empty());
        assert!(m.verts.iter().all(|v| v[0] >= -0.01 && v[0] <= 1.01 && v[1] >= -0.01 && v[1] <= 1.01));
        // Insert through the app (no network: seed the cache directly).
        let mut app = App::new(egui::Context::default(), None);
        app.icons.svgs.insert("mdi:home".into(), icons::IconState::Ready(svg.into()));
        app.insert_icon("mdi:home");
        assert_eq!(app.selection.len(), 1);
        assert_eq!(app.doc.shapes()[0].name, "mdi:home");
    }
}

/// Writes /tmp/mtzd-demo4.mvec with every effect, for `--script` screenshots.
#[test]
fn fx_write_demo() {
    use kurbo::BezPath;
    let mut f = fonts();
    let mut doc = Document::default();
    let solid = |h: &str| Some(Paint::Solid(Rgba::from_hex(h).unwrap()));
    // Clip: striped rect clipped by a circle.
    let mut stripes = Shape::new(ShapeKind::Rect { w: 220.0, h: 220.0, radii: [0.0; 4] });
    stripes.transform = Affine::translate((40.0, 40.0));
    stripes.fill = solid("#3D8BFF");
    stripes.stroke = Some(Stroke { width: 30.0, color: Rgba::from_hex("#FFC233").unwrap(), ..Default::default() });
    let mut mask = Shape::new(ShapeKind::Ellipse { w: 200.0, h: 200.0 });
    mask.transform = Affine::translate((50.0, 50.0));
    stripes.clip = Some(Box::new(mask));
    stripes.name = "Clipped".into();
    doc.add_shape(stripes);
    // Compound: square minus circle, live.
    let mut a = Shape::new(ShapeKind::Rect { w: 180.0, h: 180.0, radii: [20.0; 4] });
    a.transform = Affine::translate((320.0, 60.0));
    let mut b = Shape::new(ShapeKind::Ellipse { w: 120.0, h: 120.0 });
    b.transform = Affine::translate((400.0, 140.0));
    let mut c = Shape::new(ShapeKind::Compound { op: crate::boolean::BoolOp::Subtract, children: vec![a, b] });
    c.fill = solid("#FF5A5F");
    c.fill_rule = FillRule::EvenOdd;
    let mut ring = OutlineStep::new(Rgba::from_hex("#1E1E1E").unwrap(), 8.0);
    ring.gradient = Some(crate::document::Gradient::new(crate::document::GradientKind::Linear, Rgba::from_hex("#7B61FF").unwrap(), Rgba::from_hex("#2EC4E8").unwrap()));
    c.outlines = vec![OutlineStep::new(Rgba::WHITE, 6.0), ring];
    doc.add_shape(c);
    // Blur + noise.
    let mut bl = Shape::new(ShapeKind::Star { w: 180.0, h: 180.0, points: 5, inner: 0.5, corner: 0.0 });
    bl.transform = Affine::translate((560.0, 60.0));
    bl.fill = solid("#7B61FF");
    bl.blur = 10.0;
    doc.add_shape(bl);
    let mut nz = Shape::new(ShapeKind::Rect { w: 180.0, h: 180.0, radii: [90.0; 4] });
    nz.transform = Affine::translate((780.0, 60.0));
    nz.fill = solid("#2EC27E");
    nz.noise = 0.6;
    doc.add_shape(nz);
    // Text on a wave.
    let mut wave = BezPath::new();
    wave.move_to((60.0, 420.0));
    wave.curve_to((260.0, 300.0), (460.0, 540.0), (900.0, 400.0));
    let td = TextData { content: "Text flowing along a curved path".into(), family: f.families[0].clone(), size: 36.0, path: Some(geom::bez_to_pathdata(&wave)), ..Default::default() };
    let mut t = Shape::new(ShapeKind::Text(td));
    t.fill = solid("#1E1E1E");
    doc.add_shape(t);
    let mut sh = Shape::new(ShapeKind::Rect { w: 140.0, h: 100.0, radii: [16.0; 4] });
    sh.transform = Affine::translate((1000.0, 80.0));
    sh.fill = solid("#FFFFFF");
    sh.stroke = Some(Stroke { width: 2.0, color: Rgba::from_hex("#DDDDDD").unwrap(), ..Default::default() });
    sh.shadow = Some(Shadow { blur: 24.0, dy: 10.0, dx: 0.0, ..Default::default() });
    doc.add_shape(sh);
    io::save_native(&doc, std::path::Path::new("/tmp/mtzd-demo4.mvec")).unwrap();
    std::fs::write("/tmp/mtzd-demo4.pdf", io::pdf::export(&doc, &mut f)).unwrap();
}

#[test]
fn fx_pdf_rasterizes_blur_noise_and_shadow() {
    let mut f = fonts();
    let mut s = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
    s.fill = Some(Paint::Solid(Rgba::from_hex("#FF0000").unwrap()));
    // Blur keeps coverage mass (≈ area × scale²).
    let plain = render::rasterize_shape(&s, &mut f, 2.0, 4096.0).unwrap();
    s.blur = 20.0;
    let blurred = render::rasterize_shape(&s, &mut f, 2.0, 4096.0).unwrap();
    let mass = |r: &render::Raster| r.rgba.chunks(4).map(|p| p[3] as f64 / 255.0).sum::<f64>();
    assert!((mass(&plain) - 40000.0).abs() / 40000.0 < 0.02, "{}", mass(&plain));
    assert!((mass(&blurred) - mass(&plain)).abs() / mass(&plain) < 0.03);
    assert!(blurred.rect.width() > plain.rect.width());
    // Noise varies the colour of covered pixels.
    s.blur = 0.0;
    s.noise = 0.5;
    let noisy = render::rasterize_shape(&s, &mut f, 1.0, 4096.0).unwrap();
    let reds: std::collections::HashSet<u8> = noisy.rgba.chunks(4).filter(|p| p[3] == 255).map(|p| p[0]).collect();
    assert!(reds.len() > 20, "{}", reds.len());
    // PDF: blur/noise shapes become images; shadows are rasterized under vectors.
    let mut doc = Document::default();
    doc.add_shape(s.clone());
    let mut sh = Shape::new(ShapeKind::Ellipse { w: 80.0, h: 80.0 });
    sh.transform = Affine::translate((300.0, 100.0));
    sh.shadow = Some(Shadow::default());
    doc.add_shape(sh);
    let pdf = io::pdf::export(&doc, &mut f);
    let txt = String::from_utf8_lossy(&pdf);
    assert!(txt.contains("/Fx0 Do") && txt.contains("/Fx1 Do"), "missing raster draws");
    assert!(txt.contains("/SMask"));
    std::fs::write("/tmp/mtzd-fx.pdf", &pdf).unwrap();
}

#[test]
fn export_png_jpg_and_selection() {
    use crate::io::export::{self, Area, Format, Options};
    let mut f = fonts();
    let mut doc = Document::default();
    doc.set_size(200.0, 100.0);
    let mut a = Shape::new(ShapeKind::Rect { w: 50.0, h: 50.0, radii: [0.0; 4] });
    a.transform = Affine::translate((10.0, 10.0));
    a.fill = Some(Paint::Solid(Rgba::from_hex("#FF0000").unwrap()));
    let ia = doc.add_shape(a);
    let mut b = Shape::new(ShapeKind::Ellipse { w: 40.0, h: 40.0 });
    b.transform = Affine::translate((150.0, 50.0));
    b.shadow = Some(Shadow { dx: 0.0, dy: 0.0, blur: 4.0, ..Default::default() });
    doc.add_shape(b);
    let dir = std::env::temp_dir().join("mtzd-export-test");
    std::fs::create_dir_all(&dir).unwrap();
    // Whole document at 2x, with background.
    let p = dir.join("doc.png");
    export::export(&doc, &[], 0, &Options { format: Format::Png, scale: 2.0, ..Default::default() }, &p, &mut f).unwrap();
    let img = image::open(&p).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (400, 200));
    assert_eq!(img.get_pixel(60, 60).0, [255, 0, 0, 255]);
    assert_eq!(img.get_pixel(390, 10).0, [255, 255, 255, 255]);
    // Transparent background.
    export::export(&doc, &[], 0, &Options { format: Format::Png, scale: 1.0, background: false, ..Default::default() }, &p, &mut f).unwrap();
    let img = image::open(&p).unwrap().to_rgba8();
    assert_eq!(img.get_pixel(195, 5).0[3], 0);
    // Selection crops to the shape.
    export::export(&doc, &[ia], 0, &Options { format: Format::Png, area: Area::Selection, scale: 1.0, ..Default::default() }, &p, &mut f).unwrap();
    let img = image::open(&p).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (50, 50));
    assert_eq!(img.get_pixel(25, 25).0, [255, 0, 0, 255]);
    // JPG flattens onto white; vector formats write files.
    let pj = dir.join("doc.jpg");
    export::export(&doc, &[], 0, &Options { format: Format::Jpg, scale: 1.0, background: false, ..Default::default() }, &pj, &mut f).unwrap();
    let j = image::open(&pj).unwrap().to_rgb8();
    assert!(j.get_pixel(195, 5).0.iter().all(|c| *c > 240));
    for fmt in [Format::Svg, Format::Pdf, Format::Eps] {
        let pv = dir.join(format!("sel.{}", fmt.ext()));
        export::export(&doc, &[ia], 0, &Options { format: fmt, area: Area::Selection, ..Default::default() }, &pv, &mut f).unwrap();
        assert!(std::fs::metadata(&pv).unwrap().len() > 100);
    }
    let svg = std::fs::read_to_string(dir.join("sel.svg")).unwrap();
    assert!(svg.contains("viewBox=\"0 0 50 50\""), "{svg}");
}

#[test]
fn text_edit_word_motion_and_keys() {
    use crate::text_edit::*;
    let s = "Hello, brave  world\nnext line";
    assert_eq!(word_left(s, 19), 14);
    assert_eq!(word_left(s, 14), 7);
    assert_eq!(word_right(s, 0), 5);
    assert_eq!(word_right(s, 5), 12);
    assert_eq!(word_at(s, 9), (7, 12));
    assert_eq!(paragraph_at(s, 22), (20, 29));
    // Keys through the app: select word left, type over it, copy/cut.
    let mut app = crate::app::App::new(egui::Context::default(), None);
    let td = TextData { content: "one two".into(), ..Default::default() };
    let id = app.doc.add_shape(Shape::new(ShapeKind::Text(td)));
    app.begin_text_edit(id);
    let key = |k: egui::Key, m: egui::Modifiers| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
    let ctx = egui::Context::default();
    let cs = egui::Modifiers { ctrl: true, command: true, shift: true, ..Default::default() };
    app.text_keys(&ctx, &[key(egui::Key::ArrowLeft, cs)]);
    assert_eq!(app.selected_text().as_deref(), Some("two"));
    app.text_keys(&ctx, &[egui::Event::Text("2".into())]);
    let content = |app: &crate::app::App| match &app.doc.get(id).unwrap().kind {
        ShapeKind::Text(t) => t.content.clone(),
        _ => unreachable!(),
    };
    assert_eq!(content(&app), "one 2");
    app.text_keys(&ctx, &[key(egui::Key::A, egui::Modifiers::COMMAND), egui::Event::Cut]);
    assert_eq!(content(&app), "");
    // Ctrl+Backspace deletes a word.
    app.text_keys(&ctx, &[egui::Event::Text("alpha beta".into()), key(egui::Key::Backspace, egui::Modifiers::COMMAND)]);
    assert_eq!(content(&app), "alpha ");
    // Up/Down move between lines.
    app.text_keys(&ctx, &[egui::Event::Text("\nsecond".into())]);
    let s = app.doc.get(id).unwrap().clone();
    app.cache.update(&s, &mut app.fonts);
    app.text_keys(&ctx, &[key(egui::Key::ArrowUp, egui::Modifiers::NONE)]);
    let te = app.text_edit.as_ref().unwrap();
    assert!(te.cursor <= 6, "{}", te.cursor);
}

#[test]
fn gradient_math_and_exports() {
    use crate::document::{Gradient, GradientKind};
    use crate::io::export::{self, Format, Options};
    let red = Rgba::from_hex("#FF0000").unwrap();
    let blue = Rgba::from_hex("#0000FF").unwrap();
    let g = Gradient::new(GradientKind::Linear, red, blue);
    assert_eq!(g.color_at(0.0), red);
    assert_eq!(g.color_at(2.0), blue);
    assert!((g.color_at(0.5).r - 0.5).abs() < 1e-6);
    assert!((g.t_at([0.25, 0.9]) - 0.25).abs() < 1e-6);
    let r = Gradient::new(GradientKind::Radial, red, blue);
    assert!((r.t_at([0.5, 0.75]) - 0.5).abs() < 1e-6);
    let mut one = g.clone();
    one.stops[0].offset = 0.3;
    one.stops.pop();
    assert_eq!(one.padded_stops().iter().map(|s| s.offset).collect::<Vec<_>>(), vec![0.0, 0.3, 1.0]);

    let mut f = fonts();
    let mut doc = Document::default();
    doc.set_size(200.0, 100.0);
    let mut a = Shape::new(ShapeKind::Rect { w: 200.0, h: 100.0, radii: [0.0; 4] });
    a.fill = Some(Paint::Gradient(g.clone()));
    doc.add_shape(a);
    // PNG (resvg renders our SVG): red on the left, blue on the right.
    let dir = std::env::temp_dir().join("mtzd-grad-test");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("g.png");
    export::export(&doc, &[], 0, &Options { format: Format::Png, scale: 1.0, ..Default::default() }, &p, &mut f).unwrap();
    let img = image::open(&p).unwrap().to_rgba8();
    let l = img.get_pixel(2, 50).0;
    let rr = img.get_pixel(197, 50).0;
    assert!(l[0] > 240 && l[2] < 15 && rr[2] > 240 && rr[0] < 15, "{l:?} {rr:?}");
    // SVG round trip keeps a gradient with the same geometry.
    let svg = crate::io::svg::export(&doc, &mut f);
    assert!(svg.contains("<linearGradient"), "{svg}");
    let back = crate::io::svg::import(svg.as_bytes(), &mut f).unwrap();
    let s = back.shapes().iter().find(|s| matches!(s.fill, Some(Paint::Gradient(_)))).expect("gradient imported");
    let Some(Paint::Gradient(gb)) = &s.fill else { unreachable!() };
    assert!((gb.start[0]).abs() < 1e-3 && (gb.end[0] - 1.0).abs() < 1e-3 && (gb.start[1] - 0.5).abs() < 1e-3, "{gb:?}");
    // PDF uses an axial shading, EPS a Level 3 shfill.
    let pdf = String::from_utf8_lossy(&crate::io::pdf::export(&doc, &mut f)).to_string();
    assert!(pdf.contains("/ShadingType 2") && pdf.contains("/Sh0"), "no shading");
    let eps = crate::io::eps::export(&doc, &mut f);
    assert!(eps.contains("shfill") && eps.contains("%%LanguageLevel: 3"));
    // Transparent stops fall back to a raster in PDF; the rasterizer shades per pixel.
    let mut fade = g.clone();
    fade.stops[1].color = blue.with_alpha(0.0);
    let mut s = Shape::clone(&doc.shapes()[0]);
    s.fill = Some(Paint::Gradient(fade));
    assert!(crate::io::pdf::needs_raster(&s));
    let ras = crate::render::rasterize_shape(&s, &mut f, 1.0, 4096.0).unwrap();
    let px = |x: usize, y: usize| {
        let i = (y * ras.w + x) * 4;
        [ras.rgba[i], ras.rgba[i + 1], ras.rgba[i + 2], ras.rgba[i + 3]]
    };
    let (x0, y0) = ((0.0 - ras.rect.x0).ceil() as usize + 2, (50.0 - ras.rect.y0) as usize);
    let left = px(x0, y0);
    let right = px(x0 + 190, y0);
    assert!(left[0] > 240 && left[3] > 240 && right[3] < 40, "{left:?} {right:?}");
}

#[test]
fn gradient_follows_path_transforms() {
    use crate::document::{Gradient, GradientKind};
    let mut s = Shape::new(ShapeKind::Rect { w: 100.0, h: 50.0, radii: [0.0; 4] });
    s.fill = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, Rgba::BLACK, Rgba::WHITE)));
    let pd = crate::geom::kind_nodes(&s.kind).unwrap();
    s.kind = ShapeKind::Path(pd);
    let mut f = fonts();
    let ends = |s: &Shape, f: &mut FontManager| {
        let (path, _) = crate::render::compute_outline(s, f);
        let lb = crate::render::shape_local_bounds(s, &path);
        let m = s.transform * Affine::new([lb.width(), 0.0, 0.0, lb.height(), lb.x0, lb.y0]);
        let Some(Paint::Gradient(g)) = &s.fill else { panic!() };
        (m * kurbo::Point::new(g.start[0], g.start[1]), m * kurbo::Point::new(g.end[0], g.end[1]))
    };
    let (a0, b0) = ends(&s, &mut f);
    // Rotating 90° about the centre turns the horizontal gradient vertical.
    let c = kurbo::Point::new(50.0, 25.0);
    s.apply_transform(Affine::rotate_about(std::f64::consts::FRAC_PI_2, c));
    let (a1, b1) = ends(&s, &mut f);
    let rot = Affine::rotate_about(std::f64::consts::FRAC_PI_2, c);
    assert!((a1 - rot * a0).hypot() < 1e-6 && (b1 - rot * b0).hypot() < 1e-6, "{a1:?} {b1:?}");
}

#[test]
#[ignore]
fn write_gradient_demo() {
    use crate::document::{Gradient, GradientKind, GradientStop};
    let mut f = fonts();
    let mut doc = Document::default();
    doc.set_size(900.0, 400.0);
    let hex = |h: &str| Rgba::from_hex(h).unwrap();
    let mut a = Shape::new(ShapeKind::Rect { w: 250.0, h: 250.0, radii: [24.0; 4] });
    a.transform = Affine::translate((50.0, 75.0));
    let mut g = Gradient::new(GradientKind::Linear, hex("#FF5F6D"), hex("#4361EE"));
    g.stops.insert(1, GradientStop::new(0.5, hex("#FFC371")));
    g.start = [0.0, 0.0];
    g.end = [1.0, 1.0];
    a.fill = Some(Paint::Gradient(g));
    a.name = "Linear".into();
    doc.add_shape(a);
    let mut b = Shape::new(ShapeKind::Ellipse { w: 250.0, h: 250.0 });
    b.transform = Affine::translate((330.0, 75.0));
    let mut g = Gradient::new(GradientKind::Radial, hex("#FFFFFF"), hex("#2EC4B6"));
    g.start = [0.35, 0.35];
    g.end = [1.0, 0.35];
    b.fill = Some(Paint::Gradient(g));
    b.name = "Radial".into();
    doc.add_shape(b);
    let mut c = Shape::new(ShapeKind::Star { w: 250.0, h: 250.0, points: 5, inner: 0.5, corner: 0.0 });
    c.transform = Affine::translate((610.0, 75.0)) * Affine::rotate_about(0.3, kurbo::Point::new(125.0, 125.0));
    c.fill = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, hex("#7B2FF7"), hex("#7B2FF7").with_alpha(0.0))));
    c.stroke = Some(crate::document::Stroke { width: 3.0, color: hex("#1E1E1E"), ..Default::default() });
    c.name = "Fade".into();
    doc.add_shape(c);
    crate::io::save_native(&doc, std::path::Path::new("/tmp/mtzd-demo5.mvec")).unwrap();
    std::fs::write("/tmp/mtzd-demo5.pdf", crate::io::pdf::export(&doc, &mut f)).unwrap();
    std::fs::write("/tmp/mtzd-demo5.svg", crate::io::svg::export(&doc, &mut f)).unwrap();
    std::fs::write("/tmp/mtzd-demo5.eps", crate::io::eps::export(&doc, &mut f)).unwrap();
}

#[test]
fn path_ops_reverse_join_simplify_offset_knife() {
    use crate::app::App;
    let mut app = App::new(egui::Context::default(), None);
    let open = |pts: &[(f64, f64)]| {
        let nodes = pts.iter().map(|&(x, y)| Node::corner(kurbo::Point::new(x, y))).collect();
        let mut s = Shape::new(ShapeKind::Path(PathData { subpaths: vec![SubPath { nodes, closed: false }] }));
        s.fill = None;
        s
    };
    let pd = |app: &App, id: u64| match &app.doc.get(id).unwrap().kind {
        ShapeKind::Path(p) => p.clone(),
        _ => panic!("not a path"),
    };
    // Reverse.
    let a = app.doc.add_shape(open(&[(0.0, 0.0), (10.0, 0.0), (20.0, 5.0)]));
    app.selection = vec![a];
    app.reverse_paths();
    assert_eq!(pd(&app, a).subpaths[0].nodes[0].p, kurbo::Point::new(20.0, 5.0));
    // Join the end of one path to the start of another; coincident ends merge.
    let b = app.doc.add_shape(open(&[(0.0, 0.0), (0.0, 30.0)]));
    app.tool = crate::app::Tool::Node;
    app.node_sel = vec![(a, 0, 2), (b, 0, 0)];
    app.join_nodes().unwrap();
    assert!(app.doc.get(b).is_none(), "second path absorbed");
    let j = pd(&app, a);
    assert_eq!(j.subpaths.len(), 1);
    let ps: Vec<_> = j.subpaths[0].nodes.iter().map(|n| (n.p.x, n.p.y)).collect();
    assert_eq!(ps, vec![(20.0, 5.0), (10.0, 0.0), (0.0, 0.0), (0.0, 30.0)]);
    // Join both ends of the result: closes it.
    app.node_sel = vec![(a, 0, 0), (a, 0, 3)];
    app.join_nodes().unwrap();
    assert!(pd(&app, a).subpaths[0].closed);
    assert!(app.join_nodes().is_err());

    // Simplify a densely sampled circle.
    let mut circle = kurbo::BezPath::new();
    for i in 0..=120 {
        let t = i as f64 / 120.0 * std::f64::consts::TAU;
        let p = kurbo::Point::new(100.0 + 50.0 * t.cos(), 100.0 + 50.0 * t.sin());
        if i == 0 { circle.move_to(p) } else { circle.line_to(p) }
    }
    circle.close_path();
    let mut c = Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&circle)));
    c.fill = Some(Paint::Solid(Rgba::BLACK));
    let ci = app.doc.add_shape(c);
    app.selection = vec![ci];
    let before = pd(&app, ci).subpaths[0].nodes.len();
    app.simplify_paths();
    let after = pd(&app, ci).subpaths[0].nodes.len();
    assert!(after < before / 4 && after >= 3, "{before} -> {after}");
    let mut f = fonts();
    let (outline, _) = render::compute_outline(app.doc.get(ci).unwrap(), &mut f);
    let area = boolean::area(&boolean::path_to_polys(&outline, FillRule::NonZero));
    assert!((area - std::f64::consts::PI * 2500.0).abs() < 100.0, "{area}");

    // Offset a 100 × 100 square by 10 (round joins).
    let mut sq = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
    sq.transform = Affine::translate((300.0, 300.0));
    let si = app.doc.add_shape(sq);
    app.selection = vec![si];
    app.offset_selection(10.0, true);
    let oi = app.selection[0];
    assert_ne!(oi, si);
    let (o, _) = render::compute_outline(app.doc.get(oi).unwrap(), &mut f);
    let ob = geom::path_bounds(&o).unwrap();
    assert!((ob.width() - 120.0).abs() < 0.5 && (ob.x0 - 290.0).abs() < 0.5, "{ob:?}");
    let area = boolean::area(&boolean::path_to_polys(&o, FillRule::NonZero));
    let expect = 100.0 * 100.0 + 4.0 * 100.0 * 10.0 + std::f64::consts::PI * 100.0;
    assert!((area - expect).abs() < 40.0, "{area} vs {expect}");
    // Inset.
    app.selection = vec![si];
    app.offset_selection(-20.0, false);
    let (o, _) = render::compute_outline(app.doc.get(app.selection[0]).unwrap(), &mut f);
    let ob = geom::path_bounds(&o).unwrap();
    assert!((ob.width() - 60.0).abs() < 0.5, "{ob:?}");

    // Knife: a vertical cut through the square makes two pieces with its style.
    app.selection = vec![si];
    let n = app.knife_cut(&[kurbo::Point::new(350.0, 250.0), kurbo::Point::new(350.0, 450.0)]);
    assert_eq!(n, 1);
    assert!(app.doc.get(si).is_none());
    assert_eq!(app.selection.len(), 2);
    let widths: Vec<f64> = app
        .selection
        .clone()
        .iter()
        .map(|id| {
            let (p, _) = render::compute_outline(app.doc.get(*id).unwrap(), &mut f);
            geom::path_bounds(&p).unwrap().width()
        })
        .collect();
    assert!(widths.iter().all(|w| (w - 50.0).abs() < 0.5), "{widths:?}");
}

#[test]
fn field_maths_and_step_and_repeat() {
    use crate::units::{Unit, eval_expr};
    assert_eq!(eval_expr("120/2", None), Some(60.0));
    assert_eq!(eval_expr(" (10 + 5) * 2 px", None), Some(30.0));
    assert_eq!(eval_expr("-3 + 2*4", None), Some(5.0));
    assert_eq!(eval_expr("45°", None), Some(45.0));
    assert_eq!(eval_expr("1,5", None), Some(1.5));
    assert_eq!(eval_expr("1/0", None), None);
    assert_eq!(eval_expr("2 +", None), None);
    // Mixed units in a px field at 96 dpi: 1 in = 96 px.
    assert!((eval_expr("1in + 4", Some((Unit::Px, 96.0))).unwrap() - 100.0).abs() < 1e-9);
    assert!((eval_expr("25.4mm", Some((Unit::In, 96.0))).unwrap() - 1.0).abs() < 1e-9);

    use crate::app::App;
    let mut app = App::new(egui::Context::default(), None);
    let mut s = Shape::new(ShapeKind::Rect { w: 10.0, h: 10.0, radii: [0.0; 4] });
    s.transform = Affine::translate((0.0, 0.0));
    let a = app.doc.add_shape(s);
    app.selection = vec![a];
    app.duplicate_selection(10.0);
    let b = app.selection[0];
    // Move the copy by (30, 5) and duplicate again: the move repeats.
    app.translate_ids(&[b], kurbo::Vec2::new(20.0, -5.0));
    app.duplicate_selection(10.0);
    let c = app.selection[0];
    let x = |app: &App, id: u64| app.doc.get(id).unwrap().transform.as_coeffs();
    assert_eq!((x(&app, b)[4], x(&app, b)[5]), (30.0, 5.0));
    assert_eq!((x(&app, c)[4], x(&app, c)[5]), (60.0, 10.0));
    app.duplicate_selection(10.0);
    let d = app.selection[0];
    assert_eq!((x(&app, d)[4], x(&app, d)[5]), (90.0, 15.0));
    // Duplicating inside a group keeps the copy in the group.
    app.selection = vec![a, b];
    app.group_selection();
    let g = app.selection[0];
    app.selection = vec![a];
    app.duplicate_selection(10.0);
    let copy = app.selection[0];
    assert_eq!(app.doc.parent_of(copy), Some(g));
    assert_eq!(app.doc.get(g).unwrap().kind.children().unwrap().len(), 3);
}

#[test]
fn artboards_migrate_page_and_export() {
    use crate::io::export::{self, Area, Format, Options};
    // Files from before artboards load as one artboard.
    let legacy = r##"{"width":640.0,"height":480.0,"background":{"r":1.0,"g":0.0,"b":0.0,"a":1.0},"shapes":[],"images":{},"next_id":1}"##;
    let p = std::env::temp_dir().join("mtzd-legacy.mvec");
    std::fs::write(&p, legacy).unwrap();
    let d = io::load_native(&p).unwrap();
    assert_eq!(d.artboards.len(), 1);
    assert_eq!((d.primary().w, d.primary().h, d.primary().x), (640.0, 480.0, 0.0));
    assert_eq!(d.primary().background.to_hex(), "#FF0000");

    // Two artboards side by side, each with a square.
    let mut doc = Document::default();
    doc.set_size(100.0, 50.0);
    let b = doc.add_artboard(kurbo::Rect::new(200.0, 0.0, 260.0, 40.0), None);
    doc.artboard_mut(b).unwrap().background = Rgba::from_hex("#0000FF").unwrap();
    for (x, c) in [(10.0, "#FF0000"), (210.0, "#00FF00")] {
        let mut s = Shape::new(ShapeKind::Rect { w: 20.0, h: 20.0, radii: [0.0; 4] });
        s.transform = Affine::translate((x, 10.0));
        s.fill = Some(Paint::Solid(Rgba::from_hex(c).unwrap()));
        doc.add_shape(s);
    }
    assert_eq!(doc.artboard_at(kurbo::Point::new(230.0, 20.0)).map(|a| a.id), Some(b));
    assert_eq!(doc.bounds(), kurbo::Rect::new(0.0, 0.0, 260.0, 50.0));
    // A page moves its artboard (and the artwork) to the origin.
    let page = doc.page(b);
    assert_eq!((page.artboards.len(), page.primary().x, page.primary().w), (1, 0.0, 60.0));
    assert_eq!(page.shapes()[1].transform.translation().x, 10.0);
    // Save/load keeps every artboard.
    let p = std::env::temp_dir().join("mtzd-artboards.mvec");
    io::save_native(&doc, &p).unwrap();
    let back = io::load_native(&p).unwrap();
    assert_eq!(back.artboards, doc.artboards);

    let mut f = fonts();
    let dir = std::env::temp_dir().join("mtzd-artboard-export");
    std::fs::create_dir_all(&dir).unwrap();
    // The active artboard only.
    let png = dir.join("one.png");
    export::export(&doc, &[], b, &Options { format: Format::Png, scale: 1.0, ..Default::default() }, &png, &mut f).unwrap();
    let img = image::open(&png).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (60, 40));
    assert_eq!(img.get_pixel(20, 20).0, [0, 255, 0, 255]);
    assert_eq!(img.get_pixel(55, 5).0, [0, 0, 255, 255]);
    // All artboards: one file each, or one PDF page each.
    let files = export::export(&doc, &[], b, &Options { format: Format::Png, area: Area::AllArtboards, scale: 1.0, ..Default::default() }, &dir.join("all.png"), &mut f).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(image::open(&files[0]).unwrap().to_rgba8().dimensions(), (100, 50));
    assert!(files[1].file_name().unwrap().to_string_lossy().contains("Artboard 2"));
    let pdf = dir.join("all.pdf");
    export::export(&doc, &[], b, &Options { format: Format::Pdf, area: Area::AllArtboards, ..Default::default() }, &pdf, &mut f).unwrap();
    assert_eq!(lopdf::Document::load(&pdf).unwrap().get_pages().len(), 2);
    // Opening it back: one artboard per page, side by side, artwork on its page.
    let back = io::pdf::import(&pdf).unwrap();
    assert_eq!(back.artboards.len(), 2);
    let (p1, p2) = (back.artboards[0].clone(), back.artboards[1].clone());
    // PDF points come back as the same pixel sizes.
    assert_eq!((p1.w, p1.h, p2.w, p2.h), (100.0, 50.0, 60.0, 40.0));
    assert!(p2.x > p1.x + p1.w && p2.y == 0.0, "{p2:?}");
    let mut cache = render::GeoCache::default();
    let on_p2 = back.shapes().iter().filter(|s| cache.update(s, &mut f).bounds.and_then(|b| back.member_of(b)).map(|a| a.id) == Some(p2.id)).count();
    assert!(on_p2 >= 1, "page 2 artwork");
    // Single-page exporters draw the first artboard even when it isn't at the origin.
    doc.primary_mut().x = 500.0;
    let svg = io::svg::export(&doc, &mut f);
    assert!(svg.contains(r#"viewBox="0 0 100 50""#));
}

/// A group with shadow, outline and noise, used by the group effect tests.
fn fx_group_doc() -> Document {
    let mut doc = Document::default();
    let mut a = Shape::new(ShapeKind::Rect { w: 120.0, h: 120.0, radii: [12.0; 4] });
    a.transform = Affine::translate((100.0, 100.0));
    a.fill = Some(Paint::Solid(Rgba::from_hex("#3D8BFF").unwrap()));
    let mut b = Shape::new(ShapeKind::Ellipse { w: 120.0, h: 120.0 });
    b.transform = Affine::translate((180.0, 160.0));
    b.fill = Some(Paint::Solid(Rgba::from_hex("#FF5A5F").unwrap()));
    let mut g = Shape::new(ShapeKind::Group { children: vec![a, b] });
    g.name = "Fx Group".into();
    g.shadow = Some(Shadow { blur: 20.0, dy: 12.0, dx: 0.0, ..Default::default() });
    g.outlines = vec![OutlineStep::new(Rgba::from_hex("#1E1E1E").unwrap(), 6.0)];
    g.noise = 0.4;
    doc.add_shape(g);
    doc
}

#[test]
fn fx_group_effects_export() {
    let mut f = fonts();
    let doc = fx_group_doc();
    io::save_native(&doc, std::path::Path::new("/tmp/mtzd-groupfx.mvec")).unwrap();
    let svg = io::svg::export(&doc, &mut f);
    assert!(svg.contains(r#"filter="url(#shadow0)""#), "{svg}");
    assert!(svg.contains(r#"filter="url(#noise0)""#), "{svg}");
    // One ring path around both members, plus the two members.
    assert_eq!(svg.matches("fill-rule=\"evenodd\"").count(), 1, "{svg}");
    let shadow_at = svg.find("url(#shadow0)").unwrap();
    assert!(svg[shadow_at..].contains("#3D8BFF") && svg[shadow_at..].contains("#FF5A5F"));
    // PDF rasterizes the whole group: one effect image, members not drawn as vectors.
    let pdf = io::pdf::export(&doc, &mut f);
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("/Fx0"));
    assert!(!text.contains("/Fx1"));
    std::fs::write("/tmp/mtzd-groupfx.svg", svg).unwrap();
}

#[test]
fn fx_inner_shadow() {
    let mut f = fonts();
    // A white square with a hard black inner shadow 10 px down: a dark band
    // along the top edge, white elsewhere, nothing outside.
    let mut s = Shape::new(ShapeKind::Rect { w: 100.0, h: 100.0, radii: [0.0; 4] });
    s.transform = Affine::translate((50.0, 50.0));
    s.fill = Some(Paint::Solid(Rgba::WHITE));
    s.inner_shadow = Some(Shadow { color: Rgba::BLACK, dx: 0.0, dy: 10.0, blur: 0.0 });
    let r = render::rasterize_shape(&s, &mut f, 1.0, 4096.0).unwrap();
    let px = |x: f64, y: f64| {
        let (i, j) = (((x - r.rect.x0) * r.w as f64 / r.rect.width()) as usize, ((y - r.rect.y0) * r.h as f64 / r.rect.height()) as usize);
        let k = (j * r.w + i) * 4;
        [r.rgba[k], r.rgba[k + 1], r.rgba[k + 2], r.rgba[k + 3]]
    };
    assert_eq!(px(100.0, 55.0), [0, 0, 0, 255], "band at the top");
    assert_eq!(px(100.0, 100.0), [255, 255, 255, 255], "centre untouched");
    assert_eq!(px(100.0, 145.0), [255, 255, 255, 255], "no band at the bottom");
    assert_eq!(px(100.0, 45.0)[3], 0, "nothing outside");
    // SVG: one filter with the inner shadow merged over the graphic.
    let mut doc = Document::default();
    s.shadow = Some(Shadow::default());
    doc.add_shape(s.clone());
    let svg = io::svg::export(&doc, &mut f);
    assert!(svg.contains(r#"tableValues="1 0""#) && svg.contains(r#"<feMergeNode in="drop"/><feMergeNode in="SourceGraphic"/><feMergeNode in="inner"/>"#), "{svg}");
    // PDF rasterizes it; round-trips through the native format.
    assert!(String::from_utf8_lossy(&io::pdf::export(&doc, &mut f)).contains("/Fx0"));
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.shapes()[0].inner_shadow, s.inner_shadow);
}

#[test]
#[ignore]
fn write_inner_shadow_demo() {
    let mut f = fonts();
    let mut doc = fx_group_doc();
    let id = doc.shapes()[0].id;
    if let Some(g) = doc.get_mut(id) {
        g.inner_shadow = Some(Shadow { color: Rgba::BLACK.with_alpha(0.5), dx: 0.0, dy: 8.0, blur: 10.0 });
        g.noise = 0.0;
    }
    let mut a = Shape::new(ShapeKind::Rect { w: 200.0, h: 120.0, radii: [24.0; 4] });
    a.transform = Affine::translate((420.0, 100.0));
    a.fill = Some(Paint::Solid(Rgba::from_hex("#F2F2F2").unwrap()));
    a.inner_shadow = Some(Shadow::inner());
    doc.add_shape(a);
    let mut b = Shape::new(ShapeKind::Ellipse { w: 140.0, h: 140.0 });
    b.transform = Affine::translate((700.0, 90.0));
    b.fill = Some(Paint::Solid(Rgba::from_hex("#7B61FF").unwrap()));
    b.stroke = Some(Stroke { width: 6.0, color: Rgba::from_hex("#FFC233").unwrap(), ..Default::default() });
    b.shadow = Some(Shadow::default());
    b.inner_shadow = Some(Shadow { color: Rgba::WHITE.with_alpha(0.6), dx: -6.0, dy: -6.0, blur: 12.0 });
    doc.add_shape(b);
    io::save_native(&doc, std::path::Path::new("/tmp/mtzd-inner.mvec")).unwrap();
    std::fs::write("/tmp/mtzd-inner.svg", io::svg::export(&doc, &mut f)).unwrap();
    std::fs::write("/tmp/mtzd-inner.pdf", io::pdf::export(&doc, &mut f)).unwrap();
}

#[test]
#[ignore]
fn write_case_demo() {
    use crate::document::TextCase;
    let mut doc = Document::default();
    let cases = [(TextCase::AsTyped, "As typed"), (TextCase::Upper, "Upper"), (TextCase::Lower, "Lower"), (TextCase::Title, "Title"), (TextCase::SmallCaps, "Small caps")];
    for (col, fam) in ["Noto Sans", "DejaVu Sans"].into_iter().enumerate() {
        for (row, (case, _)) in cases.iter().enumerate() {
            let td = TextData { content: format!("The quick Brown fox · {fam}"), family: fam.into(), size: 40.0, case: *case, ..Default::default() };
            let mut t = Shape::new(ShapeKind::Text(td));
            t.fill = Some(Paint::Solid(Rgba::from_hex("#1E1E1E").unwrap()));
            t.transform = Affine::translate((60.0 + col as f64 * 900.0, 80.0 + row as f64 * 90.0));
            doc.add_shape(t);
        }
    }
    io::save_native(&doc, std::path::Path::new("/tmp/mtzd-case.mvec")).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn desktop_entry_installs_once() {
    use crate::platform::desktop::{APP_ID, THUMBNAILER_FILE, install_into};
    const MIME: &str = "application/x-mtzdvector";
    let root = std::env::temp_dir().join(format!("mtzd-desktop-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    // Files from the old app id are replaced.
    std::fs::create_dir_all(root.join("applications")).unwrap();
    std::fs::write(root.join("applications/mtzd-vector.desktop"), "old").unwrap();
    let exe = std::path::Path::new("/opt/mtzd vector/mtzd-vector");
    let r = install_into(&root, exe).unwrap();
    assert_eq!((r.written, r.removed, r.mime_changed), (11, 1, true), "8 icons, entry, MIME, thumbnailer");
    assert!(!root.join("applications/mtzd-vector.desktop").exists());
    let entry = std::fs::read_to_string(root.join(format!("applications/{APP_ID}.desktop"))).unwrap();
    assert!(entry.contains("Name=mtzdVector") && entry.contains(&format!("Icon={APP_ID}")) && entry.contains(&format!("StartupWMClass={APP_ID}")));
    assert!(entry.contains(&format!("MimeType={MIME};")));
    assert!(entry.contains("Exec=\"/opt/mtzd vector/mtzd-vector\" %F"), "{entry}");
    let th = std::fs::read_to_string(root.join(format!("thumbnailers/{THUMBNAILER_FILE}"))).unwrap();
    assert!(th.contains("Exec=\"/opt/mtzd vector/mtzd-vector\" --thumbnail %i %o %s") && th.contains("TryExec=/opt/mtzd vector/mtzd-vector"), "{th}");
    let mime = std::fs::read_to_string(root.join(format!("mime/packages/{APP_ID}.xml"))).unwrap();
    assert!(mime.contains(MIME) && mime.contains("*.mvec"));
    let png = std::fs::read(root.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png"))).unwrap();
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!((img.width(), img.height()), (256, 256));
    // Nothing is rewritten when up to date; a moved executable updates entry + thumbnailer.
    assert_eq!(install_into(&root, exe).unwrap(), Default::default());
    assert_eq!(install_into(&root, std::path::Path::new("/usr/bin/mtzd-vector")).unwrap().written, 2);
    std::fs::remove_dir_all(&root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn flatpak_registers_its_thumbnailer_unless_native_works() {
    use crate::platform::desktop::{APP_ID, THUMBNAILER_FILE, install_flatpak_thumbnailer};
    let root = std::env::temp_dir().join(format!("mtzd-flatpak-thumb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join(format!("thumbnailers/{THUMBNAILER_FILE}"));
    // The old name goes; the entry runs the Flatpak.
    std::fs::create_dir_all(root.join("thumbnailers")).unwrap();
    std::fs::write(root.join(format!("thumbnailers/{APP_ID}.thumbnailer")), "old").unwrap();
    let r = install_flatpak_thumbnailer(&root).unwrap();
    assert_eq!((r.written, r.removed), (1, 1));
    let th = std::fs::read_to_string(&path).unwrap();
    assert!(th.contains("Exec=flatpak run --command=mtzd-vector pro.mtzd.MtzdVector --thumbnail %i %o %s") && th.contains("MimeType=application/x-mtzdvector;"), "{th}");
    assert_eq!(install_flatpak_thumbnailer(&root).unwrap(), Default::default());
    // A native build whose executable exists keeps its entry; a stale one is replaced.
    let exe = std::env::current_exe().unwrap();
    let native = format!("[Thumbnailer Entry]\nTryExec={}\nExec=x\n", exe.display());
    std::fs::write(&path, &native).unwrap();
    assert_eq!(install_flatpak_thumbnailer(&root).unwrap().written, 0);
    std::fs::write(&path, "[Thumbnailer Entry]\nTryExec=/nonexistent/mtzd-vector\n").unwrap();
    assert_eq!(install_flatpak_thumbnailer(&root).unwrap().written, 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn thumbnail_renders_the_first_artboard() {
    let doc = fx_group_doc();
    let data = serde_json::to_vec(&doc).unwrap();
    let png = crate::thumbnail::render(&data, 128).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    // 1920×1080 artboard fitted into 128 px.
    assert_eq!((img.width(), img.height()), (128, 72));
    // The group sits near the top-left on white.
    assert_eq!(img.get_pixel(120, 65).0, [255, 255, 255, 255]);
    let p = img.get_pixel(10, 10).0;
    assert!(p[2] > p[0] + 60, "blue square at the top-left: {p:?}");
    // Saved files start with the container magic the MIME type matches.
    assert!(io::native::is_container(&io::native::encode(&doc, None, Default::default()).unwrap()));
    assert!(crate::thumbnail::render(b"not a document", 128).is_err());
}

#[test]
fn native_container_round_trip() {
    use crate::io::native::{self, SaveOptions};
    let mut f = fonts();
    let doc = sample_doc();
    assert!(!doc.images.is_empty());
    let bytes = native::encode(&doc, Some(&mut f), SaveOptions { backup: false, preview: true }).unwrap();
    // MIME sniffing: an uncompressed "mimetype" entry first (ODF style).
    assert!(bytes.starts_with(b"PK\x03\x04"));
    assert_eq!(&bytes[30..38], b"mimetype");
    assert_eq!(&bytes[38..38 + native::MIME.len()], native::MIME.as_bytes());
    let back = native::decode(&bytes).unwrap();
    assert_eq!(back.shapes().len(), doc.shapes().len());
    assert_eq!(back.images.len(), doc.images.len());
    for (id, img) in &doc.images {
        assert_eq!(back.images[id].encoded, img.encoded, "image bytes survive");
        assert!(back.images[id].rgba.is_some(), "decoded on load");
    }
    assert_eq!(back.format_version, native::FORMAT_VERSION);
    // The JSON doesn't carry the image bytes any more.
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let mut json = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("document.json").unwrap(), &mut json).unwrap();
    assert!(!json.contains("encoded"), "images are separate entries");
    assert!(json.contains("\"format_version\":2"));
    // A preview is embedded and used for thumbnails.
    let preview = native::embedded_preview(&bytes).expect("preview.png");
    let p = image::load_from_memory(&preview).unwrap();
    assert_eq!(p.width().max(p.height()), native::PREVIEW_PX);
    let t = image::load_from_memory(&crate::thumbnail::render(&bytes, 128).unwrap()).unwrap();
    assert_eq!(t.width().max(t.height()), 128);
}

#[test]
fn native_legacy_json_and_versions() {
    use crate::io::native;
    // Format 1: a single JSON with base64 images loads.
    let doc = sample_doc();
    let legacy = serde_json::to_vec(&doc).unwrap();
    let back = native::decode(&legacy).unwrap();
    assert_eq!(back.shapes().len(), doc.shapes().len());
    assert_eq!(back.images.len(), doc.images.len());
    // Newer formats are refused with an explanation, not half-loaded.
    let mut v: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
    v["format_version"] = 99.into();
    let err = native::decode(&serde_json::to_vec(&v).unwrap()).unwrap_err();
    assert!(err.contains("newer version") && err.contains("99"), "{err}");
    // Damaged files give an error, never a panic.
    let mut f = fonts();
    let bytes = native::encode(&doc, Some(&mut f), Default::default()).unwrap();
    assert!(native::decode(&bytes[..bytes.len() / 2]).is_err());
    assert!(native::decode(b"PK\x03\x04garbage").is_err());
    assert!(native::decode(b"{not json").is_err());
}

#[test]
fn native_save_is_atomic_with_backup() {
    use crate::io::native::{self, SaveOptions};
    let dir = std::env::temp_dir().join(format!("mtzd-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.mvec");
    let mut doc = sample_doc();
    native::save(&doc, &path, None, SaveOptions::default()).unwrap();
    let first = std::fs::read(&path).unwrap();
    doc.add_shape(Shape::new(ShapeKind::Rect { w: 5.0, h: 5.0, radii: [0.0; 4] }));
    native::save(&doc, &path, None, SaveOptions { backup: true, preview: false }).unwrap();
    assert_eq!(std::fs::read(dir.join("doc.mvec~")).unwrap(), first, "previous version kept");
    assert_eq!(native::load(&path).unwrap().shapes().len(), doc.shapes().len());
    // No temp files left behind.
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    // A failed write leaves the existing file untouched.
    let ro = dir.join("missing-dir").join("x.mvec");
    assert!(native::save(&doc, &ro, None, SaveOptions::default()).is_err());
    assert_eq!(native::load(&path).unwrap().shapes().len(), doc.shapes().len());
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------
// Importer robustness: deterministic mutation fuzzing. Every importer must
// return Ok or Err for any input — never panic or hang. `fuzz_importers_quick`
// runs in the normal suite; `fuzz_importers_long` (ignored) runs far more
// cases: `cargo test fuzz_importers_long -- --ignored`.

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const TOKENS: &[&[u8]] = &[
    b"<g>", b"</g>", b"<path d=\"M0 0L1e308 1e308\"/>", b"NaN", b"-1e309", b"999999999999", b"\x00", b"\xff\xfe",
    b"<use href=\"#a\"/>", b"id=\"a\"", b"transform=\"matrix(0 0 0 0 0 0)\"", b"stroke-width=\"1e30\"", b"r=\"-5\"",
    b" obj ", b"endobj", b"stream\n", b"endstream", b"/Length 999999", b" R ", b"<<", b">>", b"[", b"]", b"q", b"Q", b"cm", b" re f ",
    b"{", b"}", b"\"shapes\":", b"\"kind\":", b"PK\x03\x04", b"0", b"-0.0",
];

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut d = seed.to_vec();
    for _ in 0..1 + rng.below(8) {
        if d.is_empty() {
            d.extend_from_slice(TOKENS[rng.below(TOKENS.len())]);
            continue;
        }
        let i = rng.below(d.len());
        match rng.below(7) {
            0 => d[i] ^= 1 << rng.below(8),
            1 => d[i] = rng.next() as u8,
            2 => {
                let t = TOKENS[rng.below(TOKENS.len())];
                d.splice(i..i, t.iter().copied());
            }
            3 => {
                let n = rng.below(64).min(d.len() - i);
                d.drain(i..i + n);
            }
            4 => {
                let n = rng.below(256).min(d.len() - i);
                let chunk: Vec<u8> = d[i..i + n].to_vec();
                d.splice(i..i, chunk);
            }
            5 => d.truncate(i),
            _ => {
                // Digits become extreme numbers.
                if d[i].is_ascii_digit() {
                    d.splice(i..i + 1, b"99999999999999999999".iter().copied());
                }
            }
        }
    }
    d
}

fn fuzz_seeds(f: &mut FontManager) -> Vec<(&'static str, Vec<u8>)> {
    let doc = sample_doc();
    let fx = fx_group_doc();
    vec![
        ("svg", io::svg::export(&doc, f).into_bytes()),
        ("svg", io::svg::export(&fx, f).into_bytes()),
        ("svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><defs><linearGradient id="g"><stop offset="0"/></linearGradient></defs><g transform="rotate(30)"><rect width="5" height="5" fill="url(#g)"/><text x="1" y="5">Hi</text><circle r="2"/></g></svg>"#.to_vec()),
        ("pdf", io::pdf::export(&doc, f)),
        ("pdf", io::pdf::export(&fx, f)),
        ("native", io::native::encode(&doc, None, Default::default()).unwrap()),
        ("native", serde_json::to_vec(&fx).unwrap()),
    ]
}

fn fuzz_run(cases: usize, seed: u64) {
    let mut f = fonts();
    let seeds = fuzz_seeds(&mut f);
    let mut rng = Rng(seed);
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut failures = Vec::new();
    for case in 0..cases {
        let (kind, base) = &seeds[case % seeds.len()];
        let data = mutate(&mut rng, base);
        let t = std::time::Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match *kind {
            "svg" => io::svg::import(&data, &mut f).map(|_| ()),
            "pdf" => io::pdf::import_bytes(&data).map(|_| ()),
            _ => io::native::decode(&data).map(|_| ()),
        }));
        let dt = t.elapsed();
        if r.is_err() || dt > std::time::Duration::from_secs(3) {
            failures.push((kind.to_string(), case, dt, data));
            if failures.len() > 5 {
                break;
            }
        }
    }
    std::panic::set_hook(prev);
    for (kind, case, dt, data) in &failures {
        let path = format!("/tmp/mtzd-fuzz-{kind}-{case}.bin");
        let _ = std::fs::write(&path, data);
        eprintln!("{kind} case {case}: {} in {dt:?} (input saved to {path})", if *dt > std::time::Duration::from_secs(3) { "slow" } else { "panic" });
    }
    assert!(failures.is_empty(), "{} importer failures", failures.len());
}

#[test]
fn fuzz_importers_quick() {
    fuzz_run(600, 0x5EED_1234_ABCD_0001);
}

#[test]
#[ignore]
fn fuzz_importers_long() {
    let seed = std::env::var("MTZD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0xC0FFEE);
    let cases = std::env::var("MTZD_FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(30_000);
    fuzz_run(cases, seed);
}

#[test]
fn importers_survive_pathological_inputs() {
    let mut f = fonts();
    // Deep nesting must not overflow the stack.
    let deep = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{}<rect width=\"1\" height=\"1\"/>{}</svg>", "<g>".repeat(5000), "</g>".repeat(5000));
    let _ = io::svg::import(deep.as_bytes(), &mut f);
    let deep_json = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(io::native::decode(deep_json.as_bytes()).is_err());
    // Billion-laughs style entity expansion is rejected or bounded.
    let laughs = r#"<?xml version="1.0"?><!DOCTYPE l [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;"><!ENTITY e "&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;"><!ENTITY f "&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;">]><svg xmlns="http://www.w3.org/2000/svg"><text>&f;&f;&f;&f;&f;&f;</text></svg>"#;
    let t = std::time::Instant::now();
    let _ = io::svg::import(laughs.as_bytes(), &mut f);
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
    // A zip bomb entry is refused by size before inflating.
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("document.json", o).unwrap();
    let zeros = vec![b' '; 8 << 20];
    for _ in 0..80 {
        std::io::Write::write_all(&mut zip, &zeros).unwrap();
    }
    let bomb = zip.finish().unwrap().into_inner();
    assert!(bomb.len() < 4 << 20, "compressed {}", bomb.len());
    let err = io::native::decode(&bomb).unwrap_err();
    assert!(err.contains("too large"), "{err}");
}

#[test]
#[ignore]
fn fuzz_repro() {
    let path = std::env::var("MTZD_FUZZ_FILE").unwrap();
    let data = std::fs::read(&path).unwrap();
    let mut f = fonts();
    let r = if path.contains("-pdf-") { io::pdf::import_bytes(&data).map(|_| ()) } else if path.contains("-svg-") { io::svg::import(&data, &mut f).map(|_| ()) } else { io::native::decode(&data).map(|_| ()) };
    eprintln!("{:?}", r.err());
}

#[cfg(unix)]
#[test]
fn single_instance_forwards_files() {
    use crate::platform::single::{Role, claim_at, take_requests};
    let dir = std::env::temp_dir().join(format!("mtzd-single-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("instance");
    // A stale socket file from a crashed instance doesn't block startup.
    std::fs::write(&sock, b"").unwrap();
    let woke = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = woke.clone();
    assert!(matches!(claim_at(&sock, &[], move || { w.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }), Role::Primary));
    // A second launch hands its files over (made absolute) and exits.
    let files = vec![std::path::PathBuf::from("relative/doc.mvec"), std::path::PathBuf::from("/abs/other.mvec")];
    assert!(matches!(claim_at(&sock, &files, || {}), Role::Forwarded));
    let got = take_requests();
    assert_eq!(got.len(), 1);
    assert!(got[0][0].is_absolute() && got[0][0].ends_with("relative/doc.mvec"));
    assert_eq!(got[0][1], std::path::PathBuf::from("/abs/other.mvec"));
    assert_eq!(woke.load(std::sync::atomic::Ordering::SeqCst), 1);
    // A launch without files just raises the window.
    assert!(matches!(claim_at(&sock, &[], || {}), Role::Forwarded));
    assert_eq!(take_requests(), vec![Vec::<std::path::PathBuf>::new()]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "benchmark: cargo test --release bench_large_documents -- --ignored --nocapture"]
fn bench_large_documents() {
    crate::bench::run_all();
}

#[test]
#[ignore = "writes a synthetic document: MTZD_BENCH_OUT=/tmp/big.mvec MTZD_BENCH_N=10000"]
fn bench_write_document() {
    let n = std::env::var("MTZD_BENCH_N").ok().and_then(|v| v.parse().ok()).unwrap_or(10_000);
    let out = std::env::var("MTZD_BENCH_OUT").expect("MTZD_BENCH_OUT");
    io::native::save(&crate::bench::synthetic(n), std::path::Path::new(&out), None, Default::default()).unwrap();
}

/// The id index must agree with a plain tree walk through any sequence of
/// edits (adds, removals, restacks, grouping, undo snapshots).
#[test]
fn id_index_matches_tree_walk() {
    fn brute<S: std::borrow::Borrow<Shape>>(list: &[S], id: u64, parent: Option<u64>) -> Option<(u64, Option<u64>)> {
        for s in list {
            let s = s.borrow();
            if s.id == id {
                return Some((s.id, parent));
            }
            if let Some(c) = s.kind.children() {
                if let Some(r) = brute(c, id, Some(s.id)) {
                    return Some(r);
                }
            }
        }
        None
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut doc = Document::default();
    let mut snaps = Vec::new();
    for step in 0..3000 {
        let ids = doc.all_ids();
        let top: Vec<u64> = doc.shapes().iter().map(|s| s.id).collect();
        match rng.below(9) {
            0 | 1 => {
                doc.add_shape(Shape::new(ShapeKind::Rect { w: 1.0, h: 1.0, radii: [0.0; 4] }));
            }
            2 if !top.is_empty() => {
                doc.insert_shape(rng.below(top.len() + 1), Shape::new(ShapeKind::Ellipse { w: 1.0, h: 1.0 }));
            }
            3 if !ids.is_empty() => {
                doc.remove(ids[rng.below(ids.len())]);
            }
            4 if top.len() > 1 => doc.move_next_to(top[rng.below(top.len())], top[rng.below(top.len())], rng.below(2) == 0),
            5 if !top.is_empty() => doc.move_to_index(top[rng.below(top.len())], rng.below(top.len())),
            6 if top.len() > 2 => {
                // Group two top-level shapes in place.
                let (a, b) = (top[rng.below(top.len())], top[rng.below(top.len())]);
                if a != b {
                    let children = vec![doc.get(a).unwrap().clone(), doc.get(b).unwrap().clone()];
                    doc.remove(b);
                    doc.replace_with(a, vec![Shape::new(ShapeKind::Group { children })]);
                }
            }
            7 if !ids.is_empty() => {
                if let Some(s) = doc.get_mut(ids[rng.below(ids.len())]) {
                    s.name = format!("edited {step}");
                }
            }
            8 => {
                if rng.below(2) == 0 || snaps.is_empty() {
                    snaps.push(doc.snapshot());
                } else {
                    doc.restore(snaps.swap_remove(rng.below(snaps.len())));
                }
            }
            _ => {}
        }
        // Every id, plus some that don't exist.
        for id in doc.all_ids().into_iter().chain([0, 999_999, step as u64]) {
            let want = brute(doc.shapes(), id, None);
            assert_eq!(doc.get(id).map(|s| s.id), want.map(|w| w.0), "get {id} at step {step}");
            assert_eq!(doc.parent_of(id), want.and_then(|w| w.1), "parent_of {id} at step {step}");
            assert_eq!(doc.index_of(id), doc.shapes().iter().position(|s| s.id == id), "index_of {id} at step {step}");
        }
    }
}

#[test]
#[ignore = "writes /tmp/text-showcase.png for a visual check"]
fn text_showcase() {
    let mut f = FontManager::new(egui::Context::default());
    let mut doc = Document::default();
    doc.set_size(900.0, 700.0);
    let content = "Rich text: big red bold, underlined, struck.\nCJK 漢字かな and Arabic: مرحبا بالعالم ok. Emoji ☺ ★";
    let mut td = TextData { content: content.into(), size: 24.0, ..Default::default() };
    let find = |w: &str| { let i = content.find(w).unwrap(); i..i + w.len() };
    td.apply(find("big"), |c, _| c.size = Some(40.0));
    td.apply(find("red"), |c, _| c.fill = Some(Rgba::from_hex("#E03030").unwrap()));
    td.apply(find("bold"), |c, _| c.weight = Some(800));
    td.apply(find("underlined"), |c, _| c.underline = Some(true));
    td.apply(find("struck"), |c, _| c.strike = Some(true));
    let mut t = Shape::new(ShapeKind::Text(td));
    t.transform = Affine::translate((20.0, 20.0));
    t.fill = Some(Paint::Solid(Rgba::BLACK));
    doc.add_shape(t);
    let para = "Hyphenation and justification across two columns: extraordinarily incomprehensible characteristics notwithstanding, typographers internationally appreciate representational establishments. ".repeat(6);
    let mut td2 = TextData { content: para, size: 16.0, frame: Some([860.0, 260.0]), columns: 2, column_gap: 24.0, hyphenate: true, align: TextAlign::Justify, ..Default::default() };
    td2.exclusions = Some(std::sync::Arc::new(vec![kurbo::Circle::new((430.0, 130.0), 70.0).to_path(0.1)]));
    let mut t2 = Shape::new(ShapeKind::Text(td2));
    t2.transform = Affine::translate((20.0, 200.0));
    t2.fill = Some(Paint::Solid(Rgba::from_hex("#223344").unwrap()));
    doc.add_shape(t2);
    let img = io::export::rasterize(&doc, &mut f, 1.0).unwrap();
    img.save("/tmp/text-showcase.png").unwrap();
}
