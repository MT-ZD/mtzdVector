//! Export with options: format, area (document or selection) and raster
//! settings. PNG/JPG are rendered by resvg from our own SVG output, so they
//! match the vector exports exactly (clips, filters, gradients, images).

use std::path::Path;

use kurbo::{Affine, Rect};

use crate::document::{Document, Rgba, Shape};
use crate::fonts::FontManager;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Format {
    Png,
    Jpg,
    Svg,
    Pdf,
    Eps,
    /// Lossless, or lossy with `Options::webp_lossy`.
    Webp,
    Avif,
    /// AutoCAD drawing exchange (outlines as polylines).
    Dxf,
}

impl Format {
    pub const ALL: [Format; 8] = [Format::Png, Format::Jpg, Format::Webp, Format::Avif, Format::Svg, Format::Pdf, Format::Eps, Format::Dxf];

    /// From a file extension (case-insensitive).
    pub fn from_ext(ext: &str) -> Option<Format> {
        let e = ext.to_ascii_lowercase();
        Format::ALL.into_iter().find(|f| f.ext() == e || (e == "jpeg" && *f == Format::Jpg))
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::Svg => "svg",
            Format::Pdf => "pdf",
            Format::Eps => "eps",
            Format::Webp => "webp",
            Format::Avif => "avif",
            Format::Dxf => "dxf",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpg => "JPG",
            Format::Svg => "SVG",
            Format::Pdf => "PDF",
            Format::Eps => "EPS",
            Format::Webp => "WebP",
            Format::Avif => "AVIF",
            Format::Dxf => "DXF",
        }
    }

    pub fn is_raster(self) -> bool {
        matches!(self, Format::Png | Format::Jpg | Format::Webp | Format::Avif)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// The active artboard.
    Artboard,
    /// Every artboard: pages of one PDF, or one file each.
    AllArtboards,
    Selection,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub format: Format,
    pub area: Area,
    /// Raster pixels per document pixel.
    pub scale: f64,
    /// PNG: keep the artboard background (otherwise transparent).
    pub background: bool,
    /// JPG, AVIF and lossy WebP quality 1..100.
    pub quality: u8,
    /// WebP: lossy at `quality` (otherwise lossless).
    pub webp_lossy: bool,
    /// SVG / PDF: text as outlines rather than real (selectable) text.
    pub text_outlines: bool,
    /// PDF: CMYK, PDF/X, bleed and crop marks.
    pub print: crate::print::PrintOptions,
    /// SVG: precision, ids, minify, responsive (`real_text` comes from `text_outlines`).
    pub svg: super::svg::SvgOptions,
    /// PDF: image quality and metadata.
    pub pdf: super::pdf::PdfSettings,
}

/// Starting points for PDF settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfPreset {
    /// Small files for sharing on screen.
    Web,
    /// Full quality RGB.
    Standard,
    /// CMYK with bleed, for online printers.
    Print,
    /// PDF/X-4 with bleed and crop marks, for print shops.
    Press,
}

impl PdfPreset {
    pub const ALL: [PdfPreset; 4] = [PdfPreset::Web, PdfPreset::Standard, PdfPreset::Print, PdfPreset::Press];

    /// Set the quality and print options, keeping metadata and a chosen profile.
    pub fn apply(self, o: &mut Options) {
        use crate::print::{ColorOutput, PrintOptions, cmyk_profiles};
        let chosen = o.print.profile.clone();
        let press_profile = || chosen.clone().or_else(|| cmyk_profiles().first().map(|p| p.path.clone()));
        o.text_outlines = false;
        (o.pdf.effect_dpi, o.pdf.image_ppi, o.pdf.jpeg_quality) = match self {
            PdfPreset::Web => (150.0, Some(150), Some(75)),
            PdfPreset::Standard => (300.0, None, None),
            PdfPreset::Print | PdfPreset::Press => (300.0, Some(300), None),
        };
        o.print = match self {
            PdfPreset::Web | PdfPreset::Standard => PrintOptions { profile: chosen.clone(), ..Default::default() },
            PdfPreset::Print => PrintOptions { color: ColorOutput::Cmyk, profile: press_profile(), bleed_mm: 3.0, ..Default::default() },
            PdfPreset::Press => {
                let profile = press_profile();
                PrintOptions { color: ColorOutput::Cmyk, pdfx: profile.is_some(), profile, bleed_mm: 3.0, crop_marks: true }
            }
        };
    }

    /// The preset `o` is exactly on, if any.
    pub fn of(o: &Options) -> Option<PdfPreset> {
        PdfPreset::ALL.into_iter().find(|p| {
            let mut x = o.clone();
            p.apply(&mut x);
            x == *o
        })
    }
}

/// One rendered page for the export preview.
pub struct PreviewPage {
    /// The trim area plus any bleed.
    pub image: image::RgbaImage,
    /// Trim size in document px.
    pub size: (f64, f64),
    /// Bleed in document px.
    pub bleed: f64,
    pub dpi: f64,
    pub pages: usize,
}

fn keeps_background(opts: &Options) -> bool {
    !(matches!(opts.format, Format::Png | Format::Webp | Format::Avif) && !opts.background)
}

/// Longest side of a preview, in pixels.
const PREVIEW_PX: f64 = 1000.0;

/// Render page `page` of the export as it will come out: PDF bleed included,
/// CMYK colours as the output profile reproduces them.
pub fn preview_page(doc: &Document, selection: &[u64], active: u64, opts: &Options, page: usize, fonts: &mut FontManager) -> Result<PreviewPage, String> {
    let keep = keeps_background(opts);
    let pages = if opts.area == Area::AllArtboards { doc.artboards.len().max(1) } else { 1 };
    let mut d = match opts.area {
        Area::Selection => selection_document(doc, selection, fonts, keep).ok_or("Nothing selected to export")?,
        Area::Artboard => doc.page(doc.artboard(active).map(|a| a.id).unwrap_or(doc.primary().id)),
        Area::AllArtboards => doc.page(doc.artboards[page.min(pages - 1)].id),
    };
    if !keep {
        d.primary_mut().background = Rgba::WHITE.with_alpha(0.0);
    }
    let pdf = opts.format == Format::Pdf;
    let bleed = if pdf { opts.print.bleed_mm.max(0.0) * d.dpi.max(1.0) / 25.4 } else { 0.0 };
    let size = (d.primary().w, d.primary().h);
    let a = d.primary_mut();
    a.x -= bleed;
    a.y -= bleed;
    a.w += 2.0 * bleed;
    a.h += 2.0 * bleed;
    let scale = (PREVIEW_PX / d.primary().w.max(d.primary().h).max(1.0)).min(4.0);
    let mut image = rasterize(&d, fonts, scale)?;
    if pdf {
        if let Some(conv) = opts.print.converter()? {
            soft_proof(&mut image, &conv);
        }
    }
    Ok(PreviewPage { image, size, bleed, dpi: d.dpi.max(1.0), pages })
}

/// Replace colours with what they become through `conv`'s CMYK.
fn soft_proof(image: &mut image::RgbaImage, conv: &crate::print::ColorConv) {
    let mut seen: std::collections::HashMap<[u8; 3], [u8; 3]> = std::collections::HashMap::new();
    for p in image.pixels_mut() {
        // 6 bits a channel: invisible here, and keeps ICC lookups few.
        let key = [p[0] & 0xFC, p[1] & 0xFC, p[2] & 0xFC];
        let out = *seen.entry(key).or_insert_with(|| {
            let v = |c: u8| (c | 2) as f32 / 255.0;
            let c = conv.rgb(conv.cmyk(Rgba { r: v(key[0]), g: v(key[1]), b: v(key[2]), a: 1.0 }));
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            [b(c.r), b(c.g), b(c.b)]
        });
        p.0[..3].copy_from_slice(&out);
    }
}

/// Total bytes the export would write, or None for very large rasters.
pub fn exported_size(doc: &Document, selection: &[u64], active: u64, opts: &Options, fonts: &mut FontManager) -> Result<Option<u64>, String> {
    let (w, h) = raster_size(doc.primary(), None, opts.scale);
    if opts.format.is_raster() && (w as u64) * (h as u64) > 40_000_000 {
        return Ok(None);
    }
    let dir = super::eps::private_temp_dir()?;
    let path = dir.join(format!("export.{}", opts.format.ext()));
    let out = export(doc, selection, active, opts, &path, fonts).map(|files| files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum());
    let _ = std::fs::remove_dir_all(&dir);
    out.map(Some)
}

impl Default for Options {
    fn default() -> Self {
        Options { format: Format::Png, area: Area::Artboard, scale: 1.0, background: true, quality: 90, webp_lossy: false, text_outlines: false, print: Default::default(), svg: Default::default(), pdf: Default::default() }
    }
}

/// Visual bounds of shapes: outline plus stroke, outline rings, shadow and blur.
pub fn visual_bounds(shapes: &[&Shape], fonts: &mut FontManager) -> Option<Rect> {
    let mut out: Option<Rect> = None;
    for s in shapes {
        let (path, text) = crate::render::compute_outline(s, fonts);
        let b = match text {
            Some(t) => Some(s.transform.transform_rect_bbox(t.bounds)),
            None => crate::geom::path_bounds(&path),
        };
        if let Some(b) = b {
            let pad = crate::render::effect_reach(s) + crate::render::shadow_reach(s) + s.blur * 1.5;
            out = crate::geom::union_rect(out, b.inflate(pad, pad));
        }
    }
    out
}

/// A document holding just `ids`, cropped to their visual bounds.
pub fn selection_document(doc: &Document, ids: &[u64], fonts: &mut FontManager, keep_background: bool) -> Option<Document> {
    // Paint order, skipping shapes whose container is also selected.
    let mut ids: Vec<u64> = ids.iter().copied().filter(|id| doc.get(*id).is_some()).collect();
    ids.sort_by_key(|id| doc.order_key(*id));
    let all = ids.clone();
    ids.retain(|id| {
        let mut p = doc.parent_of(*id);
        while let Some(x) = p {
            if all.contains(&x) {
                return false;
            }
            p = doc.parent_of(x);
        }
        true
    });
    let shapes: Vec<&Shape> = ids.iter().filter_map(|id| doc.get(*id)).collect();
    let b = visual_bounds(&shapes, fonts)?;
    let b = Rect::new(b.x0.floor(), b.y0.floor(), b.x1.ceil(), b.y1.ceil());
    let mut tmp = Document::default();
    tmp.set_size(b.width(), b.height());
    tmp.unit = doc.unit;
    tmp.dpi = doc.dpi;
    let bg = doc.artboard_for(b).unwrap_or(doc.primary()).background;
    tmp.primary_mut().background = if keep_background { bg } else { Rgba::WHITE.with_alpha(0.0) };
    tmp.images = doc.images.clone();
    let shift = Affine::translate((-b.x0, -b.y0));
    for s in shapes {
        let mut s = s.clone();
        s.apply_transform(shift);
        tmp.add_shape(s);
    }
    tmp.gc_images();
    Some(tmp)
}

/// Render a document to straight-alpha RGBA at `scale`.
pub fn rasterize(doc: &Document, fonts: &mut FontManager, scale: f64) -> Result<image::RgbaImage, String> {
    let svg = super::svg::export(doc, fonts);
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(&svg, &opt).map_err(|e| e.to_string())?;
    let (w, h) = raster_size(doc.primary(), None, scale);
    if (w as u64) * (h as u64) > 400_000_000 {
        return Err(format!("{w} × {h} px is too large"));
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("could not allocate image")?;
    // The SVG's width/height may be in physical units: map its viewBox.
    let sx = w as f32 / tree.size().width();
    let sy = h as f32 / tree.size().height();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
    let mut img = image::RgbaImage::new(w, h);
    for (i, p) in pixmap.pixels().iter().enumerate() {
        let c = p.demultiply();
        img.as_mut()[i * 4..i * 4 + 4].copy_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok(img)
}

/// Pixel size a raster export of `artboard` (or the bounds `sel`) would have.
pub fn raster_size(artboard: &crate::document::Artboard, sel: Option<Rect>, scale: f64) -> (u32, u32) {
    let (w, h) = sel.map(|r| (r.width(), r.height())).unwrap_or((artboard.w, artboard.h));
    ((w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32)
}

/// File for one artboard when exporting all of them: `name-Artboard 2.png`.
pub fn artboard_path(path: &Path, artboard: &str) -> std::path::PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let clean: String = artboard.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c }).collect();
    path.with_file_name(format!("{stem}-{clean}.{ext}"))
}

/// Export `doc` to `path` with `opts`: the artboard `active`, every
/// artboard, or the shapes `selection`. Returns the files written.
pub fn export(doc: &Document, selection: &[u64], active: u64, opts: &Options, path: &Path, fonts: &mut FontManager) -> Result<Vec<std::path::PathBuf>, String> {
    let keep_bg = keeps_background(opts);
    let prep = |mut d: Document| {
        if !keep_bg {
            d.primary_mut().background = Rgba::WHITE.with_alpha(0.0);
        }
        d
    };
    match opts.area {
        Area::Selection => {
            let d = selection_document(doc, selection, fonts, keep_bg).ok_or("Nothing selected to export")?;
            write_one(&d, opts, path, fonts)?;
            Ok(vec![path.to_path_buf()])
        }
        Area::Artboard => {
            let id = doc.artboard(active).map(|a| a.id).unwrap_or(doc.primary().id);
            write_one(&prep(doc.page(id)), opts, path, fonts)?;
            Ok(vec![path.to_path_buf()])
        }
        Area::AllArtboards if opts.format == Format::Pdf => {
            let pages: Vec<Document> = doc.artboards.iter().map(|a| prep(doc.page(a.id))).collect();
            std::fs::write(path, super::pdf::export_pdf(&pages, fonts, &pdf_options(opts, path))?).map_err(|e| e.to_string())?;
            Ok(vec![path.to_path_buf()])
        }
        Area::AllArtboards => {
            let mut out = Vec::new();
            for a in &doc.artboards {
                let p = if doc.artboards.len() == 1 { path.to_path_buf() } else { artboard_path(path, &a.name) };
                write_one(&prep(doc.page(a.id)), opts, &p, fonts)?;
                out.push(p);
            }
            Ok(out)
        }
    }
}

fn pdf_options(opts: &Options, path: &Path) -> super::pdf::PdfOptions {
    let title = if opts.pdf.title.is_empty() { path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() } else { opts.pdf.title.clone() };
    super::pdf::PdfOptions { real_text: !opts.text_outlines, print: opts.print.clone(), title, settings: opts.pdf.clone() }
}

/// Write a single-page document (first artboard at the origin).
fn write_one(src: &Document, opts: &Options, path: &Path, fonts: &mut FontManager) -> Result<(), String> {
    let write = |data: Vec<u8>| std::fs::write(path, data).map_err(|e| e.to_string());
    match opts.format {
        Format::Svg => write(super::svg::export_svg(src, fonts, &super::svg::SvgOptions { real_text: !opts.text_outlines, ..opts.svg.clone() }).into_bytes()),
        Format::Pdf => write(super::pdf::export_pdf(&[src.at_origin().into_owned()], fonts, &pdf_options(opts, path))?),
        Format::Eps => write(super::eps::export(src, fonts).into_bytes()),
        Format::Png => {
            let img = rasterize(src, fonts, opts.scale)?;
            img.save_with_format(path, image::ImageFormat::Png).map_err(|e| e.to_string())
        }
        Format::Jpg => {
            let img = rasterize(src, fonts, opts.scale)?;
            // JPEG has no alpha: flatten onto the background (white if transparent).
            let bg = src.primary().background;
            let bg = if bg.a > 0.0 { bg } else { Rgba::WHITE };
            let rgb = image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
                let p = img.get_pixel(x, y).0;
                let a = p[3] as f32 / 255.0;
                let mix = |c: u8, b: f32| ((c as f32 / 255.0 * a + b * (1.0 - a)) * 255.0).round() as u8;
                image::Rgb([mix(p[0], bg.r), mix(p[1], bg.g), mix(p[2], bg.b)])
            });
            let mut out = Vec::new();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, opts.quality.clamp(1, 100));
            rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
            write(out)
        }
        Format::Webp => {
            let img = rasterize(src, fonts, opts.scale)?;
            if opts.webp_lossy {
                let data = webp::Encoder::from_rgba(img.as_raw(), img.width(), img.height()).encode_simple(false, opts.quality.clamp(1, 100) as f32).map_err(|e| format!("WebP encoding failed: {e:?}"))?;
                write(data.to_vec())
            } else {
                img.save_with_format(path, image::ImageFormat::WebP).map_err(|e| e.to_string())
            }
        }
        Format::Avif => {
            let img = rasterize(src, fonts, opts.scale)?;
            let mut out = Vec::new();
            let enc = image::codecs::avif::AvifEncoder::new_with_speed_quality(&mut out, 6, opts.quality.clamp(1, 100));
            img.write_with_encoder(enc).map_err(|e| e.to_string())?;
            write(out)
        }
        Format::Dxf => write(super::dxf::export(src, fonts).into_bytes()),
    }
}

/// What a slice exports: a layer (cropped to it) or an artboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceTarget {
    Shape(u64),
    Artboard(u64),
}

/// Every layer and artboard with export settings.
pub fn slice_targets(doc: &Document) -> Vec<SliceTarget> {
    let mut out: Vec<SliceTarget> = doc.artboards.iter().filter(|a| !a.exports.is_empty()).map(|a| SliceTarget::Artboard(a.id)).collect();
    for id in doc.all_ids() {
        if doc.get(id).is_some_and(|s| !s.exports.is_empty() && s.visible) {
            out.push(SliceTarget::Shape(id));
        }
    }
    out
}

fn file_stem_of(name: &str) -> String {
    let s: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "Untitled".into() } else { s }
}

/// Export `targets` by their settings into `dir` (`NAME` + suffix + ext;
/// clashing names get a number). Targets without settings export once as
/// PNG @1x. Returns the files written.
pub fn export_slices(doc: &Document, targets: &[SliceTarget], dir: &Path, fonts: &mut FontManager) -> Result<Vec<std::path::PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut written: Vec<std::path::PathBuf> = Vec::new();
    for t in targets {
        let (name, slices, src) = match *t {
            SliceTarget::Artboard(id) => {
                let Some(a) = doc.artboard(id) else { continue };
                (a.name.clone(), a.exports.clone(), doc.page(id))
            }
            SliceTarget::Shape(id) => {
                let Some(s) = doc.get(id) else { continue };
                // Layers export on transparency, like other design tools.
                let Some(d) = selection_document(doc, &[id], fonts, false) else { continue };
                (s.name.clone(), s.exports.clone(), d)
            }
        };
        let slices = if slices.is_empty() { vec![crate::document::ExportSlice::new(1.0, Format::Png)] } else { slices };
        for sl in &slices {
            let base = format!("{}{}", file_stem_of(&name), sl.suffix);
            let mut path = dir.join(format!("{base}.{}", sl.format.ext()));
            let mut n = 2;
            while written.contains(&path) {
                path = dir.join(format!("{base} {n}.{}", sl.format.ext()));
                n += 1;
            }
            let opts = Options { format: sl.format, scale: sl.scale, background: matches!(t, SliceTarget::Artboard(_)), ..Default::default() };
            write_one(&src, &opts, &path, fonts)?;
            written.push(path);
        }
    }
    Ok(written)
}
