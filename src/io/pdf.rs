//! PDF export (pdf-writer) and import (lopdf content-stream interpreter).

use std::collections::BTreeMap;
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use kurbo::{Affine, BezPath, PathEl, Point};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect as PRect, Ref};
use pdf_writer::types::{LineCapStyle, LineJoinStyle};

use crate::document::{Document, FillRule, ImageMode, LineCap, LineJoin, Paint, Rgba, Shape, ShapeKind, Stroke};
use crate::fonts::FontManager;
use crate::print::{ColorConv, PrintOptions};
use crate::{geom, render};

/// PDF export settings.
#[derive(Clone, Debug, Default)]
pub struct PdfOptions {
    /// Text as text (font subsets) where possible, else outlines.
    pub real_text: bool,
    pub print: PrintOptions,
    pub title: String,
}

/// How colours are written: RGB, or CMYK through a converter; spot colours
/// (keys marked spot) as Separation colour spaces.
struct Ink<'a> {
    conv: Option<&'a ColorConv>,
    /// Spot key id → (resource name, ref).
    spots: BTreeMap<u64, (String, Ref)>,
}

impl Ink<'_> {
    fn comps(&self, c: Rgba) -> Vec<f32> {
        match self.conv {
            Some(cv) => cv.cmyk(c).to_vec(),
            None => vec![c.r, c.g, c.b],
        }
    }

    fn fill(&self, c: &mut Content, col: Rgba, key: Option<u64>) {
        if let Some((name, _)) = key.and_then(|k| self.spots.get(&k)) {
            c.set_fill_color_space(pdf_writer::types::ColorSpaceOperand::Named(Name(name.as_bytes())));
            c.set_fill_color([1.0]);
            return;
        }
        match self.conv {
            Some(cv) => {
                let [cc, m, y, k] = cv.cmyk(col);
                c.set_fill_cmyk(cc, m, y, k);
            }
            None => {
                c.set_fill_rgb(col.r, col.g, col.b);
            }
        }
    }

    fn stroke(&self, c: &mut Content, col: Rgba, key: Option<u64>) {
        if let Some((name, _)) = key.and_then(|k| self.spots.get(&k)) {
            c.set_stroke_color_space(pdf_writer::types::ColorSpaceOperand::Named(Name(name.as_bytes())));
            c.set_stroke_color([1.0]);
            return;
        }
        match self.conv {
            Some(cv) => {
                let [cc, m, y, k] = cv.cmyk(col);
                c.set_stroke_cmyk(cc, m, y, k);
            }
            None => {
                c.set_stroke_rgb(col.r, col.g, col.b);
            }
        }
    }
}

/// Keys used by `items` (fills and strokes) that are spot colours.
fn spot_keys(doc: &Document, items: &[super::Item]) -> Vec<u64> {
    let mut out = Vec::new();
    for it in items {
        for k in [it.shape.fill_key, it.shape.stroke_key].into_iter().flatten() {
            if doc.color_key(k).is_some_and(|c| c.spot) && !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

fn emit_path(c: &mut Content, path: &BezPath) {
    super::for_each_el(path, |el, last| match *el {
        PathEl::MoveTo(p) => {
            c.move_to(p.x as f32, p.y as f32);
        }
        PathEl::LineTo(p) => {
            c.line_to(p.x as f32, p.y as f32);
        }
        PathEl::QuadTo(q, p) => {
            let p0 = last.unwrap_or(q);
            let c1 = p0 + (q - p0) * (2.0 / 3.0);
            let c2 = p + (q - p) * (2.0 / 3.0);
            c.cubic_to(c1.x as f32, c1.y as f32, c2.x as f32, c2.y as f32, p.x as f32, p.y as f32);
        }
        PathEl::CurveTo(a, b, p) => {
            c.cubic_to(a.x as f32, a.y as f32, b.x as f32, b.y as f32, p.x as f32, p.y as f32);
        }
        PathEl::ClosePath => {
            c.close_path();
        }
    });
}

fn coeffs32(a: Affine) -> [f32; 6] {
    let c = a.as_coeffs();
    [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32, c[4] as f32, c[5] as f32]
}

fn apply_stroke_style(c: &mut Content, s: &Stroke, ink: &Ink, key: Option<u64>) {
    ink.stroke(c, s.color, key);
    c.set_line_width(s.width as f32);
    c.set_line_join(match s.join {
        LineJoin::Miter => LineJoinStyle::MiterJoin,
        LineJoin::Round => LineJoinStyle::RoundJoin,
        LineJoin::Bevel => LineJoinStyle::BevelJoin,
    });
    c.set_line_cap(match s.cap {
        LineCap::Butt => LineCapStyle::ButtCap,
        LineCap::Round => LineCapStyle::RoundCap,
        LineCap::Square => LineCapStyle::ProjectingSquareCap,
    });
    c.set_miter_limit(s.miter_limit.max(1.0) as f32);
}

/// Write an RGBA8 image as an RGB XObject with an alpha soft mask.
fn write_rgba(pdf: &mut Pdf, r: Ref, mask: Option<Ref>, rgba: &[u8], w: usize, h: usize, conv: Option<&ColorConv>) {
    let (px, cmyk) = match conv {
        Some(cv) => (cv.cmyk8(rgba), true),
        None => (rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect(), false),
    };
    let data = deflate(&px);
    let mut x = pdf.image_xobject(r, &data);
    x.filter(Filter::FlateDecode);
    x.width(w as i32);
    x.height(h as i32);
    if cmyk {
        x.color_space().device_cmyk();
    } else {
        x.color_space().device_rgb();
    }
    x.bits_per_component(8);
    if let Some(m) = mask {
        x.s_mask(m);
    }
    x.finish();
    if let Some(m) = mask {
        let alpha: Vec<u8> = rgba.chunks(4).map(|p| p[3]).collect();
        let data = deflate(&alpha);
        let mut x = pdf.image_xobject(m, &data);
        x.filter(Filter::FlateDecode);
        x.width(w as i32);
        x.height(h as i32);
        x.color_space().device_gray();
        x.bits_per_component(8);
        x.finish();
    }
}

/// Resolution used for rasterized effects.
pub const EFFECT_DPI: f64 = 300.0;
const EFFECT_MAX_PX: f64 = 4096.0;

/// Shapes whose blur or noise can't be expressed in PDF are embedded as images.
pub fn needs_raster(s: &Shape) -> bool {
    s.mask.is_some() || s.blur > 0.0 || s.noise > 0.0 || s.inner_shadow.is_some() || matches!(&s.fill, Some(Paint::Gradient(g)) if !uniform_alpha(g)) || s.outlines.iter().any(|o| o.gradient.as_ref().is_some_and(|g| !uniform_alpha(g)))
}

/// All stops share one alpha (PDF shadings carry no alpha of their own).
fn uniform_alpha(g: &crate::document::Gradient) -> bool {
    g.stops.windows(2).all(|w| (w[0].color.a - w[1].color.a).abs() < 1e-3)
}

/// Write an axial/radial shading for `g`, in bounding-box units. Returns its ref.
fn write_shading(pdf: &mut Pdf, alloc: &mut dyn FnMut() -> Ref, g: &crate::document::Gradient, ink: &Ink) -> Ref {
    let cmyk = ink.conv.is_some();
    let nc = if cmyk { 4 } else { 3 };
    use pdf_writer::types::FunctionShadingType;
    if g.is_baked() {
        // Conic and mesh: a function shading over the unit square, sampled.
        const N: usize = 128;
        let mut samples = Vec::with_capacity(N * N * nc);
        let mesh = g.mesh.as_ref().filter(|_| g.kind == crate::document::GradientKind::Mesh).map(|m| m.raster(crate::document::MeshGrid::RASTER));
        for y in 0..N {
            for x in 0..N {
                let u = [x as f64 / (N - 1) as f64, y as f64 / (N - 1) as f64];
                let c = match &mesh {
                    Some(img) => crate::document::MeshGrid::sample(img, crate::document::MeshGrid::RASTER, u),
                    None => g.color_at_unit(u),
                };
                samples.extend(ink.comps(c).into_iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
            }
        }
        let data = deflate(&samples);
        let func = alloc();
        let mut f = pdf.sampled_function(func, &data);
        f.filter(Filter::FlateDecode);
        f.domain([0.0, 1.0, 0.0, 1.0]);
        f.range((0..nc).flat_map(|_| [0.0, 1.0]));
        f.size([N as i32, N as i32]);
        f.bits_per_sample(8);
        f.finish();
        let sh = alloc();
        let mut d = pdf.function_shading(sh);
        d.shading_type(FunctionShadingType::Function);
        if cmyk {
            d.color_space().device_cmyk();
        } else {
            d.color_space().device_rgb();
        }
        d.domain([0.0, 1.0, 0.0, 1.0]);
        d.function(func);
        d.finish();
        return sh;
    }
    // Padded to 0..1 so the function domain matches the shading's.
    let stops = g.padded_stops();
    // One exponential (linear) segment per pair of stops, stitched together.
    let mut segs = Vec::new();
    for w in stops.windows(2) {
        let r = alloc();
        pdf.exponential_function(r).domain([0.0, 1.0]).c0(ink.comps(w[0].color)).c1(ink.comps(w[1].color)).n(1.0);
        segs.push(r);
    }
    let func = alloc();
    let bounds: Vec<f32> = stops[1..stops.len() - 1].iter().map(|s| s.offset).collect();
    pdf.stitching_function(func).domain([0.0, 1.0]).functions(segs.iter().copied()).bounds(bounds).encode(segs.iter().flat_map(|_| [0.0, 1.0]));
    let sh = alloc();
    let mut d = pdf.function_shading(sh);
    let [sx, sy] = g.start.map(|v| v as f32);
    let [ex, ey] = g.end.map(|v| v as f32);
    match g.kind {
        crate::document::GradientKind::Linear => {
            d.shading_type(FunctionShadingType::Axial);
            d.coords([sx, sy, ex, ey]);
        }
        _ => {
            d.shading_type(FunctionShadingType::Radial);
            let r = ((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt();
            d.coords([sx, sy, 0.0, sx, sy, r]);
        }
    }
    if cmyk {
        d.color_space().device_cmyk();
    } else {
        d.color_space().device_rgb();
    }
    d.function(func);
    d.extend([true, true]);
    d.finish();
    sh
}

/// One-page PDF of the first artboard.
#[cfg_attr(not(test), allow(dead_code))] // tests and benchmarks
pub fn export(doc: &Document, fonts: &mut FontManager) -> Vec<u8> {
    export_pages(&[doc.at_origin().into_owned()], fonts)
}

/// A PDF with one page per document (each drawn from its first artboard,
/// which must be at the origin; see [`Document::page`]).
#[cfg_attr(not(test), allow(dead_code))] // tests and benchmarks
pub fn export_pages(pages: &[Document], fonts: &mut FontManager) -> Vec<u8> {
    export_pages_with(pages, fonts, false)
}

pub(crate) fn deflate_pub(data: &[u8]) -> Vec<u8> {
    deflate(data)
}

/// `export_pages`; with `real_text`, text is embedded as text (font subsets)
/// rather than outlines where possible.
#[cfg_attr(not(test), allow(dead_code))] // tests and benchmarks
pub fn export_pages_with(pages: &[Document], fonts: &mut FontManager, real_text: bool) -> Vec<u8> {
    export_pdf(pages, fonts, &PdfOptions { real_text, ..Default::default() }).unwrap_or_default()
}

/// Millimetres in document pixels.
fn mm(doc: &Document, v: f64) -> f64 {
    v * doc.dpi.max(1.0) / 25.4
}

/// PDF of `pages` (each drawn from its first artboard, at the origin) with
/// print settings: CMYK through an output profile, spot colours, bleed,
/// crop marks and PDF/X-4.
pub fn export_pdf(pages: &[Document], fonts: &mut FontManager, opts: &PdfOptions) -> Result<Vec<u8>, String> {
    let real_text = opts.real_text;
    let po = &opts.print;
    let conv = po.converter()?;
    let conv = conv.as_ref();
    let mut pdf = Pdf::new();
    if po.pdfx {
        pdf.set_version(1, 6);
    }
    // Spot colour spaces by key name, shared by all pages.
    let mut spot_spaces: BTreeMap<String, (Ref, String, Rgba, Option<[f32; 4]>)> = BTreeMap::new();
    let mut registration: Option<Ref> = None;
    let mut pdf_fonts = super::pdf_text::PdfFonts::default();
    let catalog_id = Ref::new(1);
    let tree_id = Ref::new(2);
    let mut next = 3;
    let mut page_ids = Vec::new();
    for doc in pages {
    let items = super::prepare_with(doc, fonts, real_text);
    let mut ink = Ink { conv, spots: BTreeMap::new() };
    for k in spot_keys(doc, &items) {
        let Some(key) = doc.color_key(k) else { continue };
        let n = spot_spaces.len();
        let e = spot_spaces.entry(key.name.clone()).or_insert_with(|| {
            let r = Ref::new(next);
            next += 1;
            (r, format!("Spot{n}"), key.color, key.cmyk)
        });
        ink.spots.insert(k, (e.1.clone(), e.0));
    }
    if po.crop_marks && conv.is_some() && registration.is_none() {
        registration = Some(Ref::new(next));
        next += 1;
    }
    let mut used_fonts: BTreeMap<String, Ref> = BTreeMap::new();
    // Items drawn by an enclosing group's raster.
    let mut covered = vec![false; items.len()];
    for (i, it) in items.iter().enumerate() {
        for c in covered.iter_mut().skip(i + 1).take(it.span) {
            *c = true;
        }
    }
    let page_id = Ref::new(next);
    let content_id = Ref::new(next + 1);
    next += 2;
    page_ids.push(page_id);
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };

    // Image XObjects.
    let mut image_refs: BTreeMap<u64, (Ref, String)> = BTreeMap::new();
    for item in &items {
        if let Some(Paint::Image(f)) = &item.shape.fill {
            if image_refs.contains_key(&f.image) {
                continue;
            }
            let Some(asset) = doc.images.get(&f.image) else { continue };
            let Some(rgba) = &asset.rgba else { continue };
            let (r, name) = (alloc(), format!("Im{}", image_refs.len()));
            let has_alpha = rgba.chunks(4).any(|p| p[3] < 255);
            let mask_ref = if has_alpha { Some(alloc()) } else { None };
            write_rgba(&mut pdf, r, mask_ref, rgba, asset.width as usize, asset.height as usize, conv);
            image_refs.insert(f.image, (r, name));
        }
    }

    // Rasterized effects: (xobject name, document rect, whole shape?) per item.
    let fx_scale = EFFECT_DPI / doc.dpi.max(1.0);
    let mut rasters: Vec<Option<(String, kurbo::Rect, bool)>> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let s: &Shape = &item.shape;
        let full = needs_raster(s) || item.span > 0;
        let r = if covered[idx] {
            None
        } else if s.mask.is_some() {
            // Masks: drawn by the SVG renderer.
            render::svg_paint::rasterize_shape_svg(s, doc, fonts, fx_scale, EFFECT_MAX_PX)
        } else if full {
            render::rasterize_shape(s, fonts, fx_scale, EFFECT_MAX_PX)
        } else if s.shadow.is_some() {
            render::rasterize_shadow(s, fonts, fx_scale, EFFECT_MAX_PX)
        } else {
            None
        };
        rasters.push(r.map(|r| {
            let (id, mask) = (alloc(), alloc());
            let name = format!("Fx{}", rasters.len());
            write_rgba(&mut pdf, id, Some(mask), &r.rgba, r.w, r.h, conv);
            image_refs.insert(u64::MAX - rasters.len() as u64, (id, name.clone()));
            (name, r.rect, full)
        }));
    }

    // Gradient shadings, per item (vector unless rasterized above).
    let mut shadings: Vec<(Ref, String)> = Vec::new();
    let mut shading_of: Vec<Option<String>> = Vec::new();
    // Gradient outline rings, per item and ring.
    let mut ring_shading: Vec<Vec<Option<String>>> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let full = rasters[idx].as_ref().map(|r| r.2).unwrap_or(false);
        let mut shade = |g: &crate::document::Gradient, shadings: &mut Vec<(Ref, String)>| {
            let r = write_shading(&mut pdf, &mut alloc, g, &ink);
            let name = format!("Sh{}", shadings.len());
            shadings.push((r, name.clone()));
            name
        };
        shading_of.push(match &item.shape.fill {
            Some(Paint::Gradient(g)) if !full => Some(shade(g, &mut shadings)),
            _ => None,
        });
        ring_shading.push(
            item.rings
                .iter()
                .map(|(_, p)| match p {
                    Paint::Gradient(g) if !full => Some(shade(g, &mut shadings)),
                    _ => None,
                })
                .collect(),
        );
    }

    // Graphics states for opacity: keyed by (fill alpha, stroke alpha) in 1/255 steps.
    let mut gstates: BTreeMap<(u8, u8), (Ref, String)> = BTreeMap::new();
    let gs_for = |fa: f32, sa: f32, gstates: &mut BTreeMap<(u8, u8), (Ref, String)>, alloc: &mut dyn FnMut() -> Ref| -> Option<String> {
        let key = ((fa.clamp(0.0, 1.0) * 255.0).round() as u8, (sa.clamp(0.0, 1.0) * 255.0).round() as u8);
        if key == (255, 255) {
            return None;
        }
        let n = gstates.len();
        Some(gstates.entry(key).or_insert_with(|| (alloc(), format!("GS{n}"))).1.clone())
    };

    // Blend modes: one graphics state each.
    let mut blend_gs: BTreeMap<&'static str, (Ref, String)> = BTreeMap::new();
    for item in &items {
        if !item.shape.blend.is_normal() {
            let n = blend_gs.len();
            blend_gs.entry(item.shape.blend.pdf()).or_insert_with(|| (alloc(), format!("BM{n}")));
        }
    }

    let mut c = Content::new();
    // Document px -> PDF points at the document DPI.
    let k = (72.0 / doc.dpi.max(1.0)) as f32;
    let (dw, dh) = (doc.primary().w as f32, doc.primary().h as f32);
    // Bleed, and room for crop marks outside it (document px).
    let bleed = mm(doc, po.bleed_mm.max(0.0)) as f32;
    let mark_gap = bleed.max(mm(doc, 3.0) as f32);
    let mark_len = mm(doc, 5.0) as f32;
    let margin = if po.crop_marks { mark_gap + mark_len + mm(doc, 1.0) as f32 } else { bleed };
    c.transform([k, 0.0, 0.0, -k, margin * k, (dh + margin) * k]);
    if doc.primary().background.a > 0.0 {
        let b = doc.primary().background;
        c.save_state();
        if let Some(n) = gs_for(b.a, 1.0, &mut gstates, &mut alloc) {
            c.set_parameters(Name(n.as_bytes()));
        }
        ink.fill(&mut c, b, None);
        c.rect(-bleed, -bleed, dw + 2.0 * bleed, dh + 2.0 * bleed);
        c.fill_nonzero();
        c.restore_state();
    }
    for (idx, item) in items.iter().enumerate() {
        let s: &Shape = &item.shape;
        if item.path.elements().is_empty() || covered[idx] || item.span > 0 && rasters[idx].is_none() {
            continue;
        }
        let blend = (!s.blend.is_normal()).then(|| blend_gs.get(s.blend.pdf()).map(|g| g.1.clone())).flatten();
        if let Some((name, r, full)) = &rasters[idx] {
            // The raster already includes clip, rings and shadow when `full`.
            c.save_state();
            if let Some(b) = &blend {
                c.set_parameters(Name(b.as_bytes()));
            }
            c.transform([r.width() as f32, 0.0, 0.0, -(r.height() as f32), r.x0 as f32, r.y1 as f32]);
            c.x_object(Name(name.as_bytes()));
            c.restore_state();
            if *full {
                continue;
            }
        }
        let fill_alpha = match &s.fill {
            Some(Paint::Solid(col)) => col.a * s.opacity,
            Some(Paint::Image(f)) => f.opacity * s.opacity,
            Some(Paint::Gradient(g)) => g.stops.first().map(|st| st.color.a).unwrap_or(1.0) * s.opacity,
            None => 1.0,
        };
        let stroke_alpha = s.stroke.as_ref().map(|st| st.color.a * s.opacity).unwrap_or(1.0);
        c.save_state();
        if let Some(b) = &blend {
            c.set_parameters(Name(b.as_bytes()));
        }
        if let Some((cp, rule)) = &item.clip {
            emit_path(&mut c, cp);
            if *rule == FillRule::EvenOdd {
                c.clip_even_odd();
            } else {
                c.clip_nonzero();
            }
            c.end_path();
        }
        for (k, (rp, paint)) in item.rings.iter().enumerate() {
            c.save_state();
            match (paint, ring_shading[idx].get(k).cloned().flatten()) {
                (Paint::Gradient(g), Some(name)) => {
                    let a = g.stops.first().map(|st| st.color.a).unwrap_or(1.0) * s.opacity;
                    if let Some(n) = gs_for(a, a, &mut gstates, &mut alloc) {
                        c.set_parameters(Name(n.as_bytes()));
                    }
                    emit_path(&mut c, rp);
                    c.clip_even_odd();
                    c.end_path();
                    let lb = render::ring_bounds(s, rp);
                    c.transform(coeffs32(s.transform * kurbo::Affine::new([lb.width().max(1e-9), 0.0, 0.0, lb.height().max(1e-9), lb.x0, lb.y0])));
                    c.shading(Name(name.as_bytes()));
                }
                (paint, _) => {
                    let col = paint.solid().unwrap_or(crate::document::Rgba::BLACK);
                    let a = col.a * s.opacity;
                    if let Some(n) = gs_for(a, a, &mut gstates, &mut alloc) {
                        c.set_parameters(Name(n.as_bytes()));
                    }
                    ink.fill(&mut c, col, None);
                    emit_path(&mut c, rp);
                    c.fill_even_odd();
                }
            }
            c.restore_state();
        }
        if let Some(n) = gs_for(fill_alpha, stroke_alpha, &mut gstates, &mut alloc) {
            c.set_parameters(Name(n.as_bytes()));
        }
        match &s.fill {
            Some(Paint::Solid(col)) if item.text.is_some() => {
                let t = item.text.as_ref().unwrap();
                let key = s.fill_key;
                pdf_fonts.show(&mut c, t, s.transform, *col, &|c, col| ink.fill(c, col, key), &mut alloc, &mut used_fonts);
            }
            Some(Paint::Solid(col)) => {
                ink.fill(&mut c, *col, s.fill_key);
                emit_path(&mut c, &item.path);
                if s.fill_rule == FillRule::EvenOdd {
                    c.fill_even_odd();
                } else {
                    c.fill_nonzero();
                }
            }
            Some(Paint::Image(f)) => {
                if let (Some(asset), Some((_, name))) = (doc.images.get(&f.image), image_refs.get(&f.image)) {
                    c.save_state();
                    emit_path(&mut c, &item.path);
                    if s.fill_rule == FillRule::EvenOdd {
                        c.clip_even_odd();
                    } else {
                        c.clip_nonzero();
                    }
                    c.end_path();
                    c.transform(coeffs32(s.transform));
                    let lb = render::shape_local_bounds(s, &item.path);
                    let (iw, ih) = (asset.width as f64, asset.height as f64);
                    let place = |r: kurbo::Rect, c: &mut Content| {
                        c.save_state();
                        c.transform([r.width() as f32, 0.0, 0.0, -(r.height() as f32), r.x0 as f32, r.y1 as f32]);
                        c.x_object(Name(name.as_bytes()));
                        c.restore_state();
                    };
                    if f.mode == ImageMode::Tile {
                        let sc = f.scale.max(0.01) as f64;
                        let (tw, th) = (iw * sc, ih * sc);
                        let nx = (lb.width() / tw).ceil() as i32;
                        let ny = (lb.height() / th).ceil() as i32;
                        if nx * ny <= 4000 {
                            for y in 0..ny {
                                for x in 0..nx {
                                    let x0 = lb.x0 + x as f64 * tw;
                                    let y0 = lb.y0 + y as f64 * th;
                                    place(kurbo::Rect::new(x0, y0, x0 + tw, y0 + th), &mut c);
                                }
                            }
                        }
                    } else {
                        place(render::image_rect(f.mode, lb, iw, ih), &mut c);
                    }
                    c.restore_state();
                }
            }
            Some(Paint::Gradient(_)) => {
                if let Some(name) = &shading_of[idx] {
                    c.save_state();
                    emit_path(&mut c, &item.path);
                    if s.fill_rule == FillRule::EvenOdd {
                        c.clip_even_odd();
                    } else {
                        c.clip_nonzero();
                    }
                    c.end_path();
                    // Shadings are defined in the shape's bounding-box units.
                    let lb = render::shape_local_bounds(s, &item.path);
                    c.transform(coeffs32(s.transform * kurbo::Affine::new([lb.width().max(1e-9), 0.0, 0.0, lb.height().max(1e-9), lb.x0, lb.y0])));
                    c.shading(Name(name.as_bytes()));
                    c.restore_state();
                }
            }
            None => {}
        }
        if let Some(st) = &s.stroke {
            if st.width > 0.0 {
                apply_stroke_style(&mut c, st, &ink, s.stroke_key);
                ink.fill(&mut c, st.color, s.stroke_key);
                // Dashes, alignment and arrowheads are emitted as geometry.
                for part in crate::strokes::stroke_parts(&item.path, st, s.fill_rule) {
                    match part {
                        crate::strokes::StrokePart::Line(l) => {
                            emit_path(&mut c, &l);
                            c.stroke();
                        }
                        crate::strokes::StrokePart::Fill(f, rule) => {
                            // Fill alpha must match the stroke color here.
                            if let Some(n) = gs_for(stroke_alpha, stroke_alpha, &mut gstates, &mut alloc) {
                                c.set_parameters(Name(n.as_bytes()));
                            }
                            emit_path(&mut c, &f);
                            if rule == FillRule::EvenOdd {
                                c.fill_even_odd();
                            } else {
                                c.fill_nonzero();
                            }
                        }
                    }
                }
            }
        }
        c.restore_state();
    }
    if po.crop_marks {
        // Trim marks at each corner, outside the bleed, in registration colour.
        c.save_state();
        match registration {
            Some(_) => {
                c.set_stroke_color_space(pdf_writer::types::ColorSpaceOperand::Named(Name(b"Reg")));
                c.set_stroke_color([1.0]);
            }
            None => {
                c.set_stroke_rgb(0.0, 0.0, 0.0);
            }
        }
        c.set_line_width(0.25 / k);
        for (x, sx) in [(0.0, -1.0), (dw, 1.0)] {
            for (y, sy) in [(0.0, -1.0), (dh, 1.0)] {
                c.move_to(x + sx * mark_gap, y);
                c.line_to(x + sx * (mark_gap + mark_len), y);
                c.move_to(x, y + sy * mark_gap);
                c.line_to(x, y + sy * (mark_gap + mark_len));
            }
        }
        c.stroke();
        c.restore_state();
    }
    let content = c.finish();

    let mut page = pdf.page(page_id);
    page.media_box(PRect::new(0.0, 0.0, (dw + 2.0 * margin) * k, (dh + 2.0 * margin) * k));
    if !po.is_plain() {
        page.trim_box(PRect::new(margin * k, margin * k, (margin + dw) * k, (margin + dh) * k));
        page.bleed_box(PRect::new((margin - bleed) * k, (margin - bleed) * k, (margin + dw + bleed) * k, (margin + dh + bleed) * k));
    }
    if conv.is_some() {
        page.group().transparency().color_space().device_cmyk();
    }
    page.parent(tree_id);
    page.contents(content_id);
    {
        let mut res = page.resources();
        if !ink.spots.is_empty() || registration.is_some() {
            let mut cs = res.color_spaces();
            for (name, r) in ink.spots.values() {
                cs.pair(Name(name.as_bytes()), *r);
            }
            if let Some(r) = registration {
                cs.pair(Name(b"Reg"), r);
            }
            cs.finish();
        }
        {
            let mut xo = res.x_objects();
            for (r, name) in image_refs.values() {
                xo.pair(Name(name.as_bytes()), *r);
            }
            xo.finish();
        }
        if !used_fonts.is_empty() {
            let mut fo = res.fonts();
            for (name, r) in &used_fonts {
                fo.pair(Name(name.as_bytes()), *r);
            }
            fo.finish();
        }
        if !shadings.is_empty() {
            let mut sh = res.shadings();
            for (r, name) in &shadings {
                sh.pair(Name(name.as_bytes()), *r);
            }
            sh.finish();
        }
        {
            let mut gs = res.ext_g_states();
            for (r, name) in gstates.values().chain(blend_gs.values()) {
                gs.pair(Name(name.as_bytes()), *r);
            }
            gs.finish();
        }
        res.finish();
    }
    page.finish();
    for ((fa, sa), (r, _)) in &gstates {
        pdf.ext_graphics(*r).non_stroking_alpha(*fa as f32 / 255.0).stroking_alpha(*sa as f32 / 255.0);
    }
    for (mode, (r, _)) in &blend_gs {
        pdf.ext_graphics(*r).pair(Name(b"BM"), Name(mode.as_bytes()));
    }
    pdf.stream(content_id, &content);
    }
    if !pdf_fonts.is_empty() {
        let mut alloc = || {
            let r = Ref::new(next);
            next += 1;
            r
        };
        pdf_fonts.write(&mut pdf, &mut alloc);
    }
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };
    // Spot inks: Separation spaces with a tint function to their process mix.
    for (name, (r, _, col, cmyk)) in &spot_spaces {
        let ink_name = name.replace(|c: char| !c.is_ascii_graphic() || "()<>[]{}/%#".contains(c), "_");
        let mut sep = pdf.color_space(*r).separation(Name(ink_name.as_bytes()));
        match conv {
            Some(cv) => {
                sep.alternate_color_space().device_cmyk();
                let c1 = cmyk.unwrap_or_else(|| cv.cmyk(*col));
                sep.tint_exponential().domain([0.0, 1.0]).c0([0.0; 4]).c1(c1).n(1.0);
            }
            None => {
                sep.alternate_color_space().device_rgb();
                sep.tint_exponential().domain([0.0, 1.0]).c0([1.0; 3]).c1([col.r, col.g, col.b]).n(1.0);
            }
        }
    }
    if let Some(r) = registration {
        let mut sep = pdf.color_space(r).separation(Name(b"All"));
        sep.alternate_color_space().device_cmyk();
        sep.tint_exponential().domain([0.0, 1.0]).c0([0.0; 4]).c1([1.0; 4]).n(1.0);
    }
    let pdfx = if po.pdfx { conv.and_then(|c| c.icc.clone().map(|icc| (icc, c.name.clone()))) } else { None };
    let (icc_ref, meta_ref, info_ref) = if pdfx.is_some() { (Some(alloc()), Some(alloc()), Some(alloc())) } else { (None, None, None) };
    let now = Stamp::now();
    let title = if opts.title.is_empty() { "Untitled".to_string() } else { opts.title.clone() };
    let producer = format!("mtzdVector {}", env!("CARGO_PKG_VERSION"));
    if let (Some((icc, _)), Some(r)) = (&pdfx, icc_ref) {
        let data = deflate(icc);
        pdf.icc_profile(r, &data).n(4).filter(Filter::FlateDecode);
    }
    if let (Some(r), true) = (meta_ref, pdfx.is_some()) {
        let xmp = now.xmp(&title, &producer);
        pdf.metadata(r, xmp.as_bytes());
    }
    if let Some(r) = info_ref {
        let mut info = pdf.document_info(r);
        info.title(pdf_writer::TextStr(&title));
        info.producer(pdf_writer::TextStr(&producer));
        info.creator(pdf_writer::TextStr("mtzdVector"));
        info.creation_date(now.date());
        info.modified_date(now.date());
        info.trapped(pdf_writer::types::TrappingStatus::NotTrapped);
        info.pair(Name(b"GTS_PDFXVersion"), pdf_writer::Str(b"PDF/X-4"));
        info.finish();
        let id = now.id(&title);
        pdf.set_file_id((id.clone(), id));
    }
    let mut cat = pdf.catalog(catalog_id);
    cat.pages(tree_id);
    if let Some(r) = meta_ref {
        cat.metadata(r);
    }
    if let (Some((_, name)), Some(r)) = (&pdfx, icc_ref) {
        let mut oi = cat.output_intents();
        let mut it = oi.push();
        it.subtype(pdf_writer::types::OutputIntentSubtype::PDFX);
        it.output_condition_identifier(pdf_writer::TextStr(name));
        it.output_condition(pdf_writer::TextStr(name));
        it.info(pdf_writer::TextStr(name));
        it.dest_output_profile(r);
        it.finish();
        oi.finish();
    }
    cat.finish();
    pdf.pages(tree_id).kids(page_ids.iter().copied()).count(page_ids.len() as i32);
    Ok(pdf.finish())
}

/// Export time, for PDF/X dates (Info and XMP must agree) and the file id.
struct Stamp {
    y: u16,
    mo: u8,
    d: u8,
    h: u8,
    mi: u8,
    s: u8,
    secs: u64,
}

impl Stamp {
    fn now() -> Self {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        Self::at(secs)
    }

    fn at(secs: u64) -> Self {
        // Days to civil date (Howard Hinnant's algorithm), UTC.
        let days = (secs / 86400) as i64;
        let rem = secs % 86400;
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let y = (yoe + era * 400 + if mo <= 2 { 1 } else { 0 }) as u16;
        Stamp { y, mo, d, h: (rem / 3600) as u8, mi: (rem / 60 % 60) as u8, s: (rem % 60) as u8, secs }
    }

    fn date(&self) -> pdf_writer::Date {
        pdf_writer::Date::new(self.y).month(self.mo).day(self.d).hour(self.h).minute(self.mi).second(self.s).utc_offset_hour(0).utc_offset_minute(0)
    }

    fn iso(&self) -> String {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", self.y, self.mo, self.d, self.h, self.mi, self.s)
    }

    fn id(&self, title: &str) -> Vec<u8> {
        use std::hash::Hash;
        let mut out = Vec::with_capacity(16);
        for salt in 0..2u8 {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (self.secs, title, salt, std::process::id()).hash(&mut h);
            out.extend(std::hash::Hasher::finish(&h).to_be_bytes());
        }
        out
    }

    fn uuid(&self, title: &str, salt: u8) -> String {
        let mut id = self.id(title);
        id[0] ^= salt;
        let h: String = id.iter().map(|b| format!("{b:02x}")).collect();
        format!("uuid:{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    }

    fn xmp(&self, title: &str, producer: &str) -> String {
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let date = self.iso();
        format!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\" xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\">\n\
<dc:format>application/pdf</dc:format>\n\
<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{title}</rdf:li></rdf:Alt></dc:title>\n\
<xmp:CreateDate>{date}</xmp:CreateDate>\n<xmp:ModifyDate>{date}</xmp:ModifyDate>\n<xmp:MetadataDate>{date}</xmp:MetadataDate>\n\
<xmp:CreatorTool>mtzdVector</xmp:CreatorTool>\n\
<pdf:Producer>{producer}</pdf:Producer>\n<pdf:Trapped>False</pdf:Trapped>\n\
<xmpMM:DocumentID>{doc}</xmpMM:DocumentID>\n<xmpMM:InstanceID>{inst}</xmpMM:InstanceID>\n<xmpMM:VersionID>1</xmpMM:VersionID>\n<xmpMM:RenditionClass>default</xmpMM:RenditionClass>\n\
<pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>\n\
</rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>",
            title = esc(title),
            producer = esc(producer),
            doc = self.uuid(title, 0),
            inst = self.uuid(title, 1),
        )
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct GState {
    ctm: Affine,
    fill: Rgba,
    stroke: Rgba,
    width: f64,
    join: LineJoin,
    cap: LineCap,
    fill_alpha: f32,
    stroke_alpha: f32,
}

fn num(o: &lopdf::Object) -> f64 {
    match o {
        lopdf::Object::Integer(i) => *i as f64,
        lopdf::Object::Real(r) => *r as f64,
        _ => 0.0,
    }
}

fn color_from(ops: &[lopdf::Object]) -> Option<Rgba> {
    let v: Vec<f32> = ops.iter().filter(|o| matches!(o, lopdf::Object::Integer(_) | lopdf::Object::Real(_))).map(|o| num(o) as f32).collect();
    Some(match v.len() {
        1 => Rgba::rgb(v[0], v[0], v[0]),
        3 => Rgba::rgb(v[0], v[1], v[2]),
        4 => cmyk(v[0], v[1], v[2], v[3]),
        _ => return None,
    })
}

fn cmyk(c: f32, m: f32, y: f32, k: f32) -> Rgba {
    Rgba::rgb((1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k))
}

struct Interp<'a> {
    pdf: &'a lopdf::Document,
    doc: Document,
    /// Page user space (points, y up) → document pixels.
    page: Affine,
    depth: u32,
}

impl Interp<'_> {
    fn lookup_resource<'b>(&'b self, resources: &[&'b lopdf::Dictionary], category: &[u8], name: &[u8]) -> Option<&'b lopdf::Object> {
        for r in resources {
            if let Ok(cat) = r.get(category) {
                let cat = self.pdf.dereference(cat).ok()?.1;
                if let Ok(d) = cat.as_dict() {
                    if let Ok(o) = d.get(name) {
                        return self.pdf.dereference(o).ok().map(|x| x.1);
                    }
                }
            }
        }
        None
    }

    fn emit(&mut self, path: &BezPath, gs: &GState, fill: bool, stroke: bool, even_odd: bool) {
        if path.elements().is_empty() {
            return;
        }
        let xf = self.page * gs.ctm;
        let p = xf * path.clone();
        let mut shape = Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&p)));
        shape.fill = if fill { Some(Paint::Solid(gs.fill.with_alpha(gs.fill_alpha))) } else { None };
        shape.fill_rule = if even_odd { FillRule::EvenOdd } else { FillRule::NonZero };
        if stroke {
            let scale = xf.determinant().abs().sqrt();
            shape.stroke = Some(Stroke {
                color: gs.stroke.with_alpha(gs.stroke_alpha),
                width: (gs.width.max(0.0) * scale).max(0.25),
                join: gs.join,
                cap: gs.cap,
                ..Default::default()
            });
        }
        self.doc.add_shape(shape);
    }

    fn run(&mut self, content: &[u8], resources: &[&lopdf::Dictionary], base: GState) {
        if self.depth > 8 {
            return;
        }
        let Ok(ops) = lopdf::content::Content::decode(content) else { return };
        let mut gs = base;
        let mut stack: Vec<GState> = Vec::new();
        let mut path = BezPath::new();
        let mut cur = Point::ZERO;
        let mut start = Point::ZERO;
        for op in ops.operations {
            let a = &op.operands;
            let n = |i: usize| a.get(i).map(num).unwrap_or(0.0);
            match op.operator.as_str() {
                "q" => stack.push(gs.clone()),
                "Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                "cm" => gs.ctm = gs.ctm * Affine::new([n(0), n(1), n(2), n(3), n(4), n(5)]),
                "w" => gs.width = n(0),
                "j" => {
                    gs.join = match n(0) as i32 {
                        1 => LineJoin::Round,
                        2 => LineJoin::Bevel,
                        _ => LineJoin::Miter,
                    }
                }
                "J" => {
                    gs.cap = match n(0) as i32 {
                        1 => LineCap::Round,
                        2 => LineCap::Square,
                        _ => LineCap::Butt,
                    }
                }
                "m" => {
                    cur = Point::new(n(0), n(1));
                    start = cur;
                    path.move_to(cur);
                }
                // Malformed streams may draw before any `m`: start there.
                "l" | "c" | "v" | "y" if path.elements().is_empty() => {
                    path.move_to(cur);
                    start = cur;
                    let p = match op.operator.as_str() {
                        "l" => Point::new(n(0), n(1)),
                        "c" => Point::new(n(4), n(5)),
                        _ => Point::new(n(2), n(3)),
                    };
                    path.line_to(p);
                    cur = p;
                }
                "l" => {
                    cur = Point::new(n(0), n(1));
                    path.line_to(cur);
                }
                "c" => {
                    cur = Point::new(n(4), n(5));
                    path.curve_to(Point::new(n(0), n(1)), Point::new(n(2), n(3)), cur);
                }
                "v" => {
                    let c0 = cur;
                    cur = Point::new(n(2), n(3));
                    path.curve_to(c0, Point::new(n(0), n(1)), cur);
                }
                "y" => {
                    cur = Point::new(n(2), n(3));
                    path.curve_to(Point::new(n(0), n(1)), cur, cur);
                }
                "h" => {
                    if !path.elements().is_empty() {
                        path.close_path();
                    }
                    cur = start;
                }
                "re" => {
                    let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                    path.move_to((x, y));
                    path.line_to((x + w, y));
                    path.line_to((x + w, y + h));
                    path.line_to((x, y + h));
                    path.close_path();
                    cur = Point::new(x, y);
                    start = cur;
                }
                "f" | "F" => {
                    self.emit(&path, &gs, true, false, false);
                    path = BezPath::new();
                }
                "f*" => {
                    self.emit(&path, &gs, true, false, true);
                    path = BezPath::new();
                }
                "S" => {
                    self.emit(&path, &gs, false, true, false);
                    path = BezPath::new();
                }
                "s" => {
                    close(&mut path);
                    self.emit(&path, &gs, false, true, false);
                    path = BezPath::new();
                }
                "B" | "b" => {
                    if op.operator == "b" {
                        close(&mut path);
                    }
                    self.emit(&path, &gs, true, true, false);
                    path = BezPath::new();
                }
                "B*" | "b*" => {
                    if op.operator == "b*" {
                        close(&mut path);
                    }
                    self.emit(&path, &gs, true, true, true);
                    path = BezPath::new();
                }
                "n" => path = BezPath::new(),
                "g" | "rg" | "k" | "sc" | "scn" => {
                    if let Some(c) = color_from(a) {
                        gs.fill = c;
                    }
                }
                "G" | "RG" | "K" | "SC" | "SCN" => {
                    if let Some(c) = color_from(a) {
                        gs.stroke = c;
                    }
                }
                "gs" => {
                    if let Some(lopdf::Object::Name(name)) = a.first() {
                        if let Some(o) = self.lookup_resource(resources, b"ExtGState", name) {
                            if let Ok(d) = o.as_dict() {
                                if let Ok(v) = d.get(b"ca") {
                                    gs.fill_alpha = num(v) as f32;
                                }
                                if let Ok(v) = d.get(b"CA") {
                                    gs.stroke_alpha = num(v) as f32;
                                }
                                if let Ok(v) = d.get(b"LW") {
                                    gs.width = num(v);
                                }
                            }
                        }
                    }
                }
                "Do" => {
                    if let Some(lopdf::Object::Name(name)) = a.first() {
                        let Some(lopdf::Object::Stream(st)) = self.lookup_resource(resources, b"XObject", name).cloned() else {
                            continue;
                        };
                        let is_form = st.dict.get(b"Subtype").ok().and_then(|s| s.as_name().ok()) == Some(b"Form".as_slice());
                        if !is_form {
                            continue;
                        }
                        let mut inner = gs.clone();
                        if let Ok(lopdf::Object::Array(m)) = st.dict.get(b"Matrix") {
                            if m.len() == 6 {
                                let v: Vec<f64> = m.iter().map(num).collect();
                                inner.ctm = inner.ctm * Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]);
                            }
                        }
                        let data = st.decompressed_content().unwrap_or_else(|_| st.content.clone());
                        let own_res = st
                            .dict
                            .get(b"Resources")
                            .ok()
                            .and_then(|r| self.pdf.dereference(r).ok())
                            .and_then(|(_, r)| r.as_dict().ok())
                            .cloned();
                        let mut res: Vec<&lopdf::Dictionary> = Vec::new();
                        if let Some(r) = &own_res {
                            res.push(r);
                        }
                        res.extend_from_slice(resources);
                        self.depth += 1;
                        self.run(&data, &res, inner);
                        self.depth -= 1;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Gap between imported pages, laid out left to right as artboards.
const PAGE_GAP: f64 = 100.0;

/// Open a PDF: every page becomes an artboard, side by side.
pub fn import(path: &Path) -> Result<Document, String> {
    import_pdf(lopdf::Document::load(path).map_err(|e| e.to_string())?)
}

/// `import` from bytes.
#[cfg(test)]
pub fn import_bytes(data: &[u8]) -> Result<Document, String> {
    import_pdf(lopdf::Document::load_mem(data).map_err(|e| e.to_string())?)
}

fn import_pdf(pdf: lopdf::Document) -> Result<Document, String> {
    let pages = pdf.get_pages();
    if pages.is_empty() {
        return Err("PDF has no pages".into());
    }
    let mut doc = Document::default();
    // PDF user space is in points: scale to pixels at the document's
    // resolution so physical sizes and pixel sizes both match the source.
    doc.unit = crate::units::Unit::Mm;
    let k = doc.dpi / 72.0;
    let mut it = Interp { pdf: &pdf, doc, page: Affine::IDENTITY, depth: 0 };
    let mut x = 0.0;
    let many = pages.len() > 1;
    for (i, (_, &page_id)) in pages.iter().enumerate() {
        let media = media_box(&pdf, page_id);
        let (w, h) = ((media[2] - media[0]).abs().max(1.0) * k, (media[3] - media[1]).abs().max(1.0) * k);
        if i == 0 {
            it.doc.set_size(w, h);
            if many {
                it.doc.primary_mut().name = "Page 1".into();
            }
        } else {
            it.doc.add_artboard(kurbo::Rect::new(x, 0.0, x + w, h), Some(format!("Page {}", i + 1)));
        }
        let content = pdf.get_page_content(page_id);
        let (inline, ids) = pdf.get_page_resources(page_id).map_err(|e| e.to_string())?;
        let mut res: Vec<&lopdf::Dictionary> = Vec::new();
        if let Some(d) = inline {
            res.push(d);
        }
        for id in ids {
            if let Ok(d) = pdf.get_object(id).and_then(|o| o.as_dict()) {
                res.push(d);
            }
        }
        it.page = Affine::translate((x, 0.0)) * Affine::scale(k) * Affine::new([1.0, 0.0, 0.0, -1.0, -media[0], media[3]]);
        let base = GState {
            ctm: Affine::IDENTITY,
            fill: Rgba::BLACK,
            stroke: Rgba::BLACK,
            width: 1.0,
            join: LineJoin::Miter,
            cap: LineCap::Butt,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
        };
        it.run(&content, &res, base);
        x += w + PAGE_GAP;
    }
    Ok(it.doc)
}

/// A page's MediaBox (inherited from its parents), default US Letter.
fn media_box(pdf: &lopdf::Document, page_id: lopdf::ObjectId) -> [f64; 4] {
    let mut media = [0.0, 0.0, 612.0, 792.0];
    let mut node = pdf.get_object(page_id).and_then(|o| o.as_dict()).ok();
    while let Some(d) = node {
        if let Ok(lopdf::Object::Array(a)) = d.get(b"MediaBox").and_then(|o| pdf.dereference(o).map(|x| x.1)) {
            if a.len() == 4 {
                for (i, v) in a.iter().enumerate() {
                    media[i] = num(v);
                }
            }
            break;
        }
        node = d.get(b"Parent").ok().and_then(|p| pdf.dereference(p).ok()).and_then(|(_, p)| p.as_dict().ok());
    }
    media
}

/// Close the current subpath (a no-op on an empty path from a malformed stream).
fn close(path: &mut BezPath) {
    if !path.elements().is_empty() {
        path.close_path();
    }
}

#[cfg(test)]
pub fn stamp_iso(secs: u64) -> String {
    Stamp::at(secs).iso()
}
