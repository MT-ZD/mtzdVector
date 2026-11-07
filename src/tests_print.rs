//! Print output tests: CMYK, spot colours, bleed, crop marks, PDF/X-4.

use kurbo::Affine;

use crate::document::{Document, Gradient, GradientKind, Paint, Rgba, Shape, ShapeKind};
use crate::fonts::FontManager;
use crate::io::pdf::{PdfOptions, export_pdf};
use crate::print::{ColorOutput, PrintOptions, cmyk_profiles};

fn fonts() -> FontManager {
    FontManager::new(egui::Context::default())
}

fn sample_doc() -> Document {
    let mut d = Document::default();
    let a = &mut d.artboards[0];
    a.x = 0.0;
    a.y = 0.0;
    a.w = 400.0;
    a.h = 300.0;
    a.background = Rgba::WHITE;
    let key = d.add_color_key("Pantone 485 C", Rgba::from_hex("#da291c").unwrap());
    if let Some(k) = d.colors.iter_mut().find(|k| k.id == key) {
        k.spot = true;
        k.cmyk = Some([0.0, 0.95, 1.0, 0.0]);
    }
    // Full-bleed background: runs past the artboard edges.
    let mut bg = Shape::new(ShapeKind::Rect { w: 440.0, h: 340.0, radii: [0.0; 4] });
    bg.transform = Affine::translate((-20.0, -20.0));
    bg.fill = Some(Paint::Solid(Rgba::from_hex("#ffe7a0").unwrap()));
    bg.stroke = None;
    d.add_shape(bg);
    let mut spot = Shape::new(ShapeKind::Ellipse { w: 120.0, h: 120.0 });
    spot.transform = Affine::translate((40.0, 40.0));
    spot.fill = Some(Paint::Solid(Rgba::from_hex("#da291c").unwrap()));
    spot.fill_key = Some(key);
    spot.stroke = None;
    d.add_shape(spot);
    let mut g = Shape::new(ShapeKind::Rect { w: 160.0, h: 120.0, radii: [0.0; 4] });
    g.transform = Affine::translate((200.0, 40.0));
    g.fill = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, Rgba::from_hex("#0077cc").unwrap(), Rgba::from_hex("#00cc77").unwrap())));
    g.stroke = None;
    g.opacity = 0.8;
    d.add_shape(g);
    d
}

fn text_of(pdf: &[u8]) -> String {
    String::from_utf8_lossy(pdf).into_owned()
}

/// Uncompressed object text (streams stay compressed, dictionaries don't).
fn has(pdf: &[u8], needle: &str) -> bool {
    text_of(pdf).contains(needle)
}

#[test]
fn cmyk_spot_bleed_marks() {
    let d = sample_doc();
    let mut f = fonts();
    let opts = PdfOptions { print: PrintOptions { color: ColorOutput::Cmyk, profile: None, pdfx: false, bleed_mm: 3.0, crop_marks: true }, ..Default::default() };
    let pdf = export_pdf(&[d.clone()], &mut f, &opts).unwrap();
    std::fs::write(std::env::temp_dir().join("mtzd-marks-test.pdf"), &pdf).ok();
    assert!(has(&pdf, "/Separation /Pantone_485_C") || has(&pdf, "/Separation/Pantone_485_C"), "spot ink");
    assert!(has(&pdf, "/All"), "registration colour for the marks");
    assert!(has(&pdf, "/TrimBox") && has(&pdf, "/BleedBox"));
    assert!(has(&pdf, "/DeviceCMYK"));
    // The page grows by the bleed plus room for the marks; the trim box is the artboard.
    let doc = lopdf::Document::load_mem(&pdf).unwrap();
    let page = doc.get_pages().into_values().next().unwrap();
    let dict = doc.get_dictionary(page).unwrap();
    let rect = |k: &[u8]| -> Vec<f32> { dict.get(k).unwrap().as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect() };
    let (media, trim, bleed) = (rect(b"MediaBox"), rect(b"TrimBox"), rect(b"BleedBox"));
    let k = 72.0 / 96.0;
    assert!(((trim[2] - trim[0]) - 400.0 * k).abs() < 0.01, "{trim:?}");
    let b = 3.0 / 25.4 * 72.0;
    assert!((trim[0] - bleed[0] - b).abs() < 0.01, "{trim:?} {bleed:?}");
    assert!(media[2] > bleed[2] + 10.0, "{media:?}");
    // No RGB anywhere in CMYK output.
    assert!(!has(&pdf, "/DeviceRGB"));
    // Plain export has none of that.
    let plain = export_pdf(&[d], &mut f, &PdfOptions::default()).unwrap();
    assert!(!has(&plain, "/TrimBox") && !has(&plain, "/DeviceCMYK"));
    assert!(has(&plain, "/Separation"), "spot inks also in RGB PDFs");
}

#[test]
fn pdfx4_needs_and_embeds_profile() {
    let d = sample_doc();
    let mut f = fonts();
    let mut po = PrintOptions { color: ColorOutput::Cmyk, profile: None, pdfx: true, bleed_mm: 3.0, crop_marks: false };
    assert!(export_pdf(&[d.clone()], &mut f, &PdfOptions { print: po.clone(), ..Default::default() }).is_err());
    let Some(p) = cmyk_profiles().first() else {
        eprintln!("no CMYK profile installed; skipped");
        return;
    };
    po.profile = Some(p.path.clone());
    let pdf = export_pdf(&[d], &mut f, &PdfOptions { print: po, title: "Poster".into(), ..Default::default() }).unwrap();
    assert!(text_of(&pdf).starts_with("%PDF-1.6"));
    for needle in ["/OutputIntents", "/GTS_PDFX", "/DestOutputProfile", "GTS_PDFXVersion", "PDF/X-4", "/Trapped", "/Metadata", "/ID", "/TrimBox"] {
        assert!(has(&pdf, needle), "missing {needle}");
    }
    assert!(has(&pdf, &p.name), "output condition named after the profile");
    std::fs::write(std::env::temp_dir().join("mtzd-pdfx-test.pdf"), &pdf).ok();
}

#[test]
fn stamp_dates() {
    // 2026-11-07 12:34:56 UTC.
    let secs = 1_794_054_896;
    let s = crate::io::pdf::stamp_iso(secs);
    assert_eq!(s, "2026-11-07T12:34:56Z");
    assert_eq!(crate::io::pdf::stamp_iso(0), "1970-01-01T00:00:00Z");
    assert_eq!(crate::io::pdf::stamp_iso(951_782_400), "2000-02-29T00:00:00Z");
}
