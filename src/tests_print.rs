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

mod export_settings {
    use super::*;
    use crate::document::{ImageAsset, ImageFill, ImageMode};
    use crate::io::export::{self, Area, Format, Options, PdfPreset};
    use crate::io::pdf::PdfSettings;

    /// A 1 inch square (at 96 dpi) showing a noisy 1000 px image.
    fn doc_with_image() -> Document {
        let mut d = Document::default();
        let mut v = 7u32;
        let px: Vec<u8> = (0..1000 * 1000)
            .flat_map(|_| {
                v = v.wrapping_mul(2_654_435_761).wrapping_add(1);
                [(v >> 24) as u8, (v >> 16) as u8, 128, 255]
            })
            .collect();
        let image = d.add_image(ImageAsset { width: 1000, height: 1000, encoded: vec![], rgba: Some(std::sync::Arc::new(px)) });
        let mut s = Shape::new(ShapeKind::Rect { w: 96.0, h: 96.0, radii: [0.0; 4] });
        s.fill = Some(Paint::Image(ImageFill { image, mode: ImageMode::Stretch, scale: 1.0, opacity: 1.0 }));
        d.add_shape(s);
        d.dpi = 96.0;
        d
    }

    fn pdf(d: &Document, settings: PdfSettings) -> Vec<u8> {
        export_pdf(&[d.clone()], &mut fonts(), &PdfOptions { settings, ..Default::default() }).unwrap()
    }

    fn has(data: &[u8], what: &[u8]) -> bool {
        data.windows(what.len()).any(|w| w == what)
    }

    #[test]
    fn images_are_downsampled_and_compressed() {
        let d = doc_with_image();
        let full = pdf(&d, PdfSettings::default());
        assert!(has(&full, b"/Width 1000") && !has(&full, b"/DCTDecode"));
        let small = pdf(&d, PdfSettings { image_ppi: Some(150), ..Default::default() });
        assert!(has(&small, b"/Width 150"), "150 ppi over one inch");
        assert!(small.len() * 10 < full.len(), "{} vs {}", small.len(), full.len());
        let jpeg = pdf(&d, PdfSettings { image_ppi: Some(150), jpeg_quality: Some(50), ..Default::default() });
        assert!(has(&jpeg, b"/DCTDecode") && jpeg.len() < small.len());
    }

    #[test]
    fn webp_quality_changes_the_file() {
        let d = doc_with_image();
        let dir = std::env::temp_dir().join(format!("mtzd-webp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = fonts();
        let active = d.primary().id;
        let mut size = |lossy: bool, quality: u8| {
            let p = dir.join(format!("{lossy}{quality}.webp"));
            let o = Options { format: Format::Webp, scale: 1.0, webp_lossy: lossy, quality, ..Default::default() };
            export::export(&d, &[], active, &o, &p, &mut f).unwrap();
            let bytes = std::fs::read(&p).unwrap();
            assert_eq!(image::guess_format(&bytes).unwrap(), image::ImageFormat::WebP);
            assert!(image::load_from_memory(&bytes).is_ok(), "decodes");
            bytes.len()
        };
        let (lossless, high, low) = (size(false, 90), size(true, 95), size(true, 5));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(low < high && high < lossless, "{low} {high} {lossless}");
    }

    #[test]
    fn metadata_is_written() {
        let d = Document::default();
        let out = pdf(&d, PdfSettings { author: "Ada".into(), subject: "Proof".into(), keywords: "a, b".into(), ..Default::default() });
        for w in [&b"(Ada)"[..], b"(Proof)", b"(a, b)"] {
            assert!(has(&out, w), "{}", String::from_utf8_lossy(w));
        }
    }

    #[test]
    fn presets_are_recognised_and_keep_metadata() {
        let mut o = Options { format: Format::Pdf, ..Default::default() };
        assert_eq!(PdfPreset::of(&o), Some(PdfPreset::Standard), "the default");
        o.pdf.author = "Ada".into();
        for p in PdfPreset::ALL {
            p.apply(&mut o);
            assert_eq!(PdfPreset::of(&o), Some(p));
            assert_eq!(o.pdf.author, "Ada");
        }
        assert_eq!(o.print.bleed_mm, 3.0);
        assert!(o.print.crop_marks);
        assert_eq!(o.print.pdfx, o.print.profile.is_some());
        o.pdf.effect_dpi = 600.0;
        assert_eq!(PdfPreset::of(&o), None, "edited: custom");
    }

    #[test]
    fn preview_includes_bleed_and_size_is_measured() {
        let mut d = Document::default();
        d.dpi = 96.0;
        let active = d.primary().id;
        let (w, h) = (d.primary().w, d.primary().h);
        let mut o = Options { format: Format::Pdf, area: Area::Artboard, ..Default::default() };
        o.print.bleed_mm = 25.4;
        let mut f = fonts();
        let p = export::preview_page(&d, &[], active, &o, 0, &mut f).unwrap();
        assert_eq!(p.size, (w, h));
        assert!((p.bleed - 96.0).abs() < 1e-6);
        let ratio = p.image.width() as f64 / p.image.height() as f64;
        assert!((ratio - (w + 192.0) / (h + 192.0)).abs() < 0.01, "{ratio}");
        assert!(export::exported_size(&d, &[], active, &o, &mut f).unwrap().unwrap() > 100);
    }
}
