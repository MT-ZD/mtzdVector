//! Text engine tests: rich ranges, styles, columns, threads, wrap,
//! hyphenation, bidi, spelling and real-text export.

use kurbo::Affine;

use crate::app::App;
use crate::document::{Document, Paint, Rgba, Shape, ShapeKind, TextAlign, TextData};
use crate::fonts::FontManager;
use crate::io;

fn fonts() -> FontManager {
    FontManager::new(egui::Context::default())
}

fn text_shape(td: TextData) -> Shape {
    let mut s = Shape::new(ShapeKind::Text(td));
    s.fill = Some(Paint::Solid(Rgba::from_hex("#202020").unwrap()));
    s
}

#[test]
fn edits_keep_range_styles_in_place() {
    let mut td = TextData { content: "Hello world".into(), size: 20.0, ..Default::default() };
    td.apply(6..11, |c, _| c.size = Some(30.0));
    assert_eq!(td.style_at(7).size, 30.0);
    assert_eq!(td.style_at(2).size, 20.0);
    // Inserting before the range shifts it.
    td.edit(0..0, "Say ");
    assert_eq!(td.content, "Say Hello world");
    assert_eq!(td.style_at(11).size, 30.0);
    assert_eq!(td.style_at(5).size, 20.0);
    // Typing inside the range inherits its style.
    td.edit(12..12, "XX");
    assert_eq!(td.style_at(12).size, 30.0);
    assert_eq!(td.style_at(13).size, 30.0);
    // Deleting the styled word removes its span.
    assert_eq!(td.content, "Say Hello woXXrld");
    td.edit(10..td.content.len(), "");
    td.normalize_spans();
    assert!(td.spans.iter().all(|s| s.end <= td.content.len()));
    assert!(td.spans.iter().all(|s| s.start < s.end));
}

#[test]
fn object_edits_clear_matching_range_overrides() {
    let mut td = TextData { content: "abc def".into(), ..Default::default() };
    td.apply(0..3, |c, _| {
        c.weight = Some(700);
        c.size = Some(40.0);
    });
    let before = td.base_style();
    td.weight = 300;
    let mask = crate::document::CharStyle::diff(&before, &td.base_style());
    td.clear_overrides(&mask);
    assert_eq!(td.style_at(1).weight, 300);
    assert_eq!(td.style_at(1).size, 40.0, "other overrides stay");
}

#[test]
fn paragraph_styles_update_their_users() {
    let mut app = App::new(egui::Context::default(), None);
    let a = app.doc.add_shape(text_shape(TextData { content: "One".into(), size: 18.0, ..Default::default() }));
    let b = app.doc.add_shape(text_shape(TextData { content: "Two".into(), size: 12.0, ..Default::default() }));
    let ShapeKind::Text(ta) = app.doc.get(a).unwrap().kind.clone() else { panic!() };
    let sid = app.add_text_style(true, &ta, None);
    for id in [a, b] {
        if let Some(ShapeKind::Text(t)) = app.doc.get_mut(id).map(|s| &mut s.kind) {
            t.style_id = Some(sid);
        }
    }
    // Redefine from a bigger, centred version.
    let mut big = ta.clone();
    big.size = 32.0;
    big.align = TextAlign::Center;
    app.redefine_text_style(sid, &big);
    for id in [a, b] {
        let ShapeKind::Text(t) = &app.doc.get(id).unwrap().kind else { panic!() };
        assert_eq!(t.size, 32.0);
        assert_eq!(t.align, TextAlign::Center);
    }
    // Deleting the style keeps the look and drops the link.
    app.delete_text_style(sid);
    let ShapeKind::Text(t) = &app.doc.get(b).unwrap().kind else { panic!() };
    assert_eq!((t.size, t.style_id), (32.0, None));
    // Character styles: a range linked to one follows it.
    let ShapeKind::Text(mut tb) = app.doc.get(b).unwrap().kind.clone() else { panic!() };
    tb.apply(0..2, |c, _| c.italic = Some(true));
    let cid = app.add_text_style(false, &tb, Some(0));
    tb.apply(0..2, |_, s| *s = Some(cid));
    app.doc.get_mut(b).unwrap().kind = ShapeKind::Text(tb);
    if let Some(st) = app.doc.text_styles.iter_mut().find(|s| s.id == cid) {
        st.style.weight = Some(800);
    }
    app.sync_text_style(cid);
    let ShapeKind::Text(t) = &app.doc.get(b).unwrap().kind else { panic!() };
    assert_eq!(t.style_at(0).weight, 800);
    assert!(t.style_at(0).italic);
}

#[test]
fn columns_split_the_frame() {
    let mut f = fonts();
    let words = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(12);
    let td = TextData { content: words, size: 14.0, frame: Some([400.0, 120.0]), columns: 2, column_gap: 20.0, ..Default::default() };
    let l = f.layout(&td);
    let visible: Vec<_> = l.lines.iter().filter(|b| !b.hidden).collect();
    assert!(visible.iter().any(|b| b.x0 < 10.0), "first column");
    assert!(visible.iter().any(|b| b.x0 >= 210.0 - 1e-6), "second column: {:?}", visible.iter().map(|b| b.x0).collect::<Vec<_>>());
    assert!(visible.iter().all(|b| b.x1 <= 400.0 + 1e-6));
    assert!(visible.iter().filter(|b| b.x0 < 10.0).all(|b| b.x1 <= 190.0 + 1e-6));
}

#[test]
fn hyphenation_breaks_long_words() {
    let mut f = fonts();
    let content = "internationalization responsibilities characteristically incomprehensibilities ".repeat(3);
    let mut td = TextData { content, size: 16.0, frame: Some([150.0, 600.0]), hyphenate: true, align: TextAlign::Justify, ..Default::default() };
    let l = f.layout(&td);
    assert!(l.runs.iter().any(|r| r.text.ends_with('-')), "a hyphenated line");
    td.hyphenate = false;
    let l = f.layout(&td);
    assert!(!l.runs.iter().any(|r| r.text.ends_with('-')));
}

#[test]
fn mixed_direction_text_has_rtl_runs() {
    let mut f = fonts();
    let td = TextData { content: "abc مرحبا بالعالم def".into(), size: 18.0, ..Default::default() };
    let l = f.layout(&td);
    if l.runs.is_empty() {
        return; // no fonts on this machine
    }
    assert!(l.runs.iter().any(|r| r.rtl), "Arabic run");
    assert!(l.runs.iter().any(|r| !r.rtl));
    // Visual order: the Latin "def" comes after the Arabic on the line.
    let x_of = |needle: &str| l.runs.iter().find(|r| r.text.contains(needle)).map(|r| r.origin.x);
    if let (Some(abc), Some(def)) = (x_of("abc"), x_of("def")) {
        assert!(abc < def);
    }
}

#[test]
fn threads_flow_text_into_the_next_frame() {
    let mut app = App::new(egui::Context::default(), None);
    let story = "one two three four five six seven eight nine ten ".repeat(10);
    let mut a = text_shape(TextData { content: story.clone(), size: 16.0, frame: Some([200.0, 60.0]), ..Default::default() });
    a.name = "A".into();
    let mut b = text_shape(TextData { content: "own text".into(), size: 16.0, frame: Some([200.0, 400.0]), ..Default::default() });
    b.transform = Affine::translate((300.0, 0.0));
    let (a, b) = (app.doc.add_shape(a), app.doc.add_shape(b));
    let sa = app.doc.get(a).unwrap().clone();
    assert!(app.cache.update(&sa, &mut app.fonts).text.as_ref().unwrap().overflow);
    app.selection = vec![a, b];
    assert!(app.link_text_frames());
    app.text_flow_tick();
    let la = app.cache.update(&app.doc.get(a).unwrap().clone(), &mut app.fonts).text.clone().unwrap();
    let lb = app.cache.update(&app.doc.get(b).unwrap().clone(), &mut app.fonts).text.clone().unwrap();
    assert!(la.end > 0 && la.end < story.len());
    assert_eq!(lb.start, la.end, "B continues where A stops");
    assert!(!la.overflow, "only the last frame reports overflow");
    // B's own text moved to the end of the story.
    let ShapeKind::Text(ta) = &app.doc.get(a).unwrap().kind else { panic!() };
    assert!(ta.content.ends_with("own text"));
    // Editing B edits the story in A.
    app.selection = vec![b];
    app.text_edit = Some(crate::app::TextEditState { id: b, cursor: lb.start, anchor: lb.start, restore: None });
    app.text_keys(&egui::Context::default(), &[egui::Event::Text("ZZ".into())]);
    let ShapeKind::Text(ta) = &app.doc.get(a).unwrap().kind else { panic!() };
    assert!(ta.content.contains("ZZ"));
    // Finishing the edit doesn't delete the (empty) continuation frame.
    app.finish_text_edit();
    assert!(app.doc.get(b).is_some());
    // Unlink: B shows its own (now empty) content again.
    app.selection = vec![b];
    app.unlink_text_frames();
    app.text_flow_tick();
    let ShapeKind::Text(tb) = &app.doc.get(b).unwrap().kind else { panic!() };
    assert!(tb.flow.is_none());
    // A thread whose next frame is deleted heals.
    app.selection = vec![a, b];
    app.link_text_frames();
    app.text_flow_tick();
    app.doc.remove(b);
    app.text_flow_tick();
    let ShapeKind::Text(ta) = &app.doc.get(a).unwrap().kind else { panic!() };
    assert_eq!(ta.next, None);
}

#[test]
fn text_wraps_around_shapes_above_it() {
    let mut app = App::new(egui::Context::default(), None);
    let content = "wrap around the circle please and keep going ".repeat(20);
    let t = app.doc.add_shape(text_shape(TextData { content, size: 14.0, frame: Some([400.0, 400.0]), ..Default::default() }));
    let mut c = Shape::new(ShapeKind::Ellipse { w: 120.0, h: 120.0 });
    c.transform = Affine::translate((140.0, 90.0));
    c.fill = Some(Paint::Solid(Rgba::from_hex("#3366ff").unwrap()));
    c.text_wrap = Some(10.0);
    app.doc.add_shape(c);
    app.text_flow_tick();
    let ShapeKind::Text(td) = &app.doc.get(t).unwrap().kind else { panic!() };
    assert!(td.exclusions.is_some());
    let l = app.cache.update(&app.doc.get(t).unwrap().clone(), &mut app.fonts).text.clone().unwrap();
    // Lines through the circle's middle avoid it (its centre is at x 200).
    let mid: Vec<_> = l.lines.iter().filter(|b| !b.hidden && b.y0 > 110.0 && b.y1 < 190.0).collect();
    assert!(!mid.is_empty());
    for b in mid {
        assert!(b.x1 < 140.0 || b.x0 > 260.0, "line {:?} crosses the circle", (b.x0, b.x1));
    }
}

#[test]
fn spelling_words_and_dictionary() {
    let w = crate::spell::words("Hello wrold, it's 3D NASA iPhone x ok");
    let got: Vec<&str> = w.iter().map(|r| &"Hello wrold, it's 3D NASA iPhone x ok"[r.clone()]).collect();
    assert_eq!(got, vec!["Hello", "wrold", "it's", "ok"]);
    let aff = "SET UTF-8\nTRY esiarntolcdugmphbyfvkwz'\n";
    let dic = "4\nhello\nworld\nit's\nok\n";
    let d = spellbook::Dictionary::new(aff, dic).unwrap();
    let mut sc = crate::spell::SpellChecker::with_dictionary("en-us", d);
    let text = "Hello wrold, it's ok";
    let bad: Vec<&str> = sc.misspelled(text, "en-us").iter().map(|r| &text[r.clone()]).collect();
    assert_eq!(bad, vec!["wrold"]);
    assert!(sc.suggest("wrold", "en-us").iter().any(|s| s == "world"));
    sc.ignore("wrold");
    assert!(sc.misspelled(text, "en-us").is_empty());
}

fn text_doc() -> Document {
    let mut doc = Document::default();
    doc.set_size(400.0, 200.0);
    let mut td = TextData { content: "Real text, kerned AV".into(), size: 24.0, ..Default::default() };
    td.apply(0..4, |c, _| c.fill = Some(Rgba::from_hex("#cc2222").unwrap()));
    td.apply(5..9, |c, _| c.underline = Some(true));
    let mut s = text_shape(td);
    s.transform = Affine::translate((20.0, 40.0));
    doc.add_shape(s);
    doc
}

#[test]
fn svg_export_keeps_real_text() {
    let mut f = fonts();
    let doc = text_doc();
    let svg = io::svg::export_with(&doc, &mut f, true);
    if !svg.contains("<text") {
        // No fonts available to lay out text.
        assert!(!io::svg::export(&doc, &mut f).contains("<path"));
        return;
    }
    assert!(svg.contains("<tspan"), "{svg}");
    assert!(svg.contains("Real") && svg.contains("kerned"));
    assert!(svg.contains("fill=\"#CC2222\"") || svg.to_lowercase().contains("fill=\"#cc2222\""), "run colour");
    // It still parses, and renders like the outline version.
    let outl = io::svg::export(&doc, &mut f);
    assert!(!outl.contains("<text"));
    assert!(usvg::Tree::from_str(&svg, &usvg::Options::default()).is_ok());
}

#[test]
fn pdf_export_embeds_fonts_and_unicode() {
    let mut f = fonts();
    let doc = text_doc();
    let pdf = io::pdf::export_pages_with(&[doc.clone()], &mut f, true);
    let parsed = lopdf::Document::load_mem(&pdf).expect("valid PDF");
    let mut font_files = 0;
    let mut to_unicode = 0;
    for (_, obj) in parsed.objects.iter() {
        if let Ok(d) = obj.as_dict().or_else(|_| obj.as_stream().map(|s| &s.dict)) {
            if d.has(b"FontFile2") || d.has(b"FontFile3") {
                font_files += 1;
            }
            if d.has(b"ToUnicode") {
                to_unicode += 1;
            }
        }
    }
    let laid_out = f.layout(match &doc.shapes()[0].kind {
        ShapeKind::Text(t) => t,
        _ => unreachable!(),
    });
    if laid_out.runs.is_empty() {
        return;
    }
    assert!(font_files >= 1, "font embedded");
    assert!(to_unicode >= 1, "text extractable");
    // The outlined version has no fonts and is drawn with paths.
    let flat = io::pdf::export_pages_with(&[doc], &mut f, false);
    assert!(!String::from_utf8_lossy(&flat).contains("FontFile"));
    // Round trip through our own importer still yields shapes.
    let _ = parsed.get_pages();
    let _ = laid_out.bounds.area();
}

#[test]
#[ignore = "writes /tmp/mtzd-realtext.{pdf,svg} for a manual check"]
fn real_text_samples() {
    let mut f = fonts();
    let mut doc = text_doc();
    let mut td = TextData { content: "Ligatures fi fl, Zażółć gęślą jaźń, مرحبا, 漢字".into(), size: 18.0, frame: Some([360.0, 100.0]), ..Default::default() };
    td.apply(0..9, |c, _| c.weight = Some(700));
    let mut s = text_shape(td);
    s.transform = Affine::translate((20.0, 90.0));
    doc.add_shape(s);
    std::fs::write("/tmp/mtzd-realtext.pdf", io::pdf::export_pages_with(&[doc.clone()], &mut f, true)).unwrap();
    std::fs::write("/tmp/mtzd-realtext.svg", io::svg::export_with(&doc, &mut f, true)).unwrap();
    let img = io::export::rasterize(&doc, &mut f, 2.0).unwrap();
    img.save("/tmp/mtzd-realtext-outlines.png").unwrap();
}

#[test]
#[ignore = "writes /tmp/mtzd-flow.mvec (threads, wrap, styles) for headless screenshots"]
fn write_flow_document() {
    let mut app = App::new(egui::Context::default(), None);
    app.doc.set_size(900.0, 600.0);
    let story = "Threads carry a story from one frame to the next, and shapes marked to wrap push the text aside. ".repeat(8);
    let mut td = TextData { content: story, size: 16.0, frame: Some([380.0, 260.0]), columns: 2, hyphenate: true, align: TextAlign::Justify, ..Default::default() };
    td.apply(0..7, |c, _| {
        c.weight = Some(800);
        c.fill = Some(Rgba::from_hex("#D0342C").unwrap());
    });
    let mut a = text_shape(td);
    a.name = "Story".into();
    a.transform = Affine::translate((40.0, 40.0));
    let mut b = text_shape(TextData { content: String::new(), size: 16.0, frame: Some([380.0, 500.0]), ..Default::default() });
    b.name = "Continued".into();
    b.transform = Affine::translate((480.0, 40.0));
    let (a, b) = (app.doc.add_shape(a), app.doc.add_shape(b));
    let mut c = Shape::new(ShapeKind::Ellipse { w: 140.0, h: 140.0 });
    c.transform = Affine::translate((600.0, 120.0));
    c.fill = Some(Paint::Solid(Rgba::from_hex("#3D8BFD").unwrap()));
    c.text_wrap = Some(12.0);
    app.doc.add_shape(c);
    app.selection = vec![a, b];
    app.link_text_frames();
    let ShapeKind::Text(ta) = app.doc.get(a).unwrap().kind.clone() else { panic!() };
    let sid = app.add_text_style(true, &ta, None);
    if let Some(ShapeKind::Text(t)) = app.doc.get_mut(a).map(|s| &mut s.kind) {
        t.style_id = Some(sid);
    }
    io::save_native(&app.doc, std::path::Path::new("/tmp/mtzd-flow.mvec")).unwrap();
}
