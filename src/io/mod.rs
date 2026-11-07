//! File import / export.

pub mod dxf;
pub mod eps;
pub mod export;
pub mod native;
pub mod pdf;
pub mod pdf_text;
pub mod svg;

use std::path::Path;

use kurbo::{Affine, BezPath, PathEl};

use crate::document::{Document, Shape};
use crate::fonts::FontManager;
use crate::render;

/// A shape together with its document-space outline, ready for export.
pub struct Item<'a> {
    /// Borrowed, or owned for group members (with the group opacity applied).
    pub shape: std::borrow::Cow<'a, Shape>,
    pub path: BezPath,
    /// Clip mask outline (document space) and its fill rule.
    pub clip: Option<(BezPath, crate::document::FillRule)>,
    /// Outline-effect rings, outermost first (not clipped).
    pub rings: Vec<(BezPath, crate::document::Paint)>,
    /// For a group with effects: how many following items are its
    /// descendants (its `path` is their combined outline). Zero otherwise.
    pub span: usize,
    /// Text exported as real text (glyph runs): `path` still holds the
    /// outlines, for effects, strokes and bounds.
    pub text: Option<crate::text::TextLayout>,
    /// Luminance / alpha mask: the mask shape's own items, mode, inverted.
    pub mask: Option<(Vec<Item<'a>>, crate::document::MaskMode, bool)>,
}

/// Export items of a shape's mask.
fn mask_items<'a>(s: &Shape, doc: &Document, fonts: &mut FontManager) -> Option<(Vec<Item<'a>>, crate::document::MaskMode, bool)> {
    let m = s.mask.as_ref()?;
    let mut items = Vec::new();
    let mut ms = m.shape.clone();
    ms.visible = true;
    push_item(std::borrow::Cow::Owned(ms), 1.0, doc, fonts, false, &mut items);
    Some((items, m.mode, m.invert))
}

/// Export items with text as outlines.
pub fn prepare<'a>(doc: &'a Document, fonts: &mut FontManager) -> Vec<Item<'a>> {
    prepare_with(doc, fonts, false)
}

/// Export items; with `real_text`, text that formats can show as text
/// (solid or no fill, no path text, no synthesized glyphs) keeps its runs.
pub fn prepare_with<'a>(doc: &'a Document, fonts: &mut FontManager, real_text: bool) -> Vec<Item<'a>> {
    let mut out = Vec::new();
    for s in doc.shapes() {
        push_item(std::borrow::Cow::Borrowed(s), 1.0, doc, fonts, real_text, &mut out);
    }
    out
}

fn real_text_ok(s: &Shape, t: &crate::text::TextLayout) -> bool {
    matches!(s.fill, None | Some(crate::document::Paint::Solid(_))) && t.on_path.is_none() && !t.runs.is_empty() && !t.runs.iter().any(|r| r.needs_outline)
}

/// Export items for a shape; groups expand to their visible children.
fn push_item<'a>(s: std::borrow::Cow<'a, Shape>, alpha: f32, doc: &Document, fonts: &mut FontManager, real_text: bool, out: &mut Vec<Item<'a>>) {
    if !s.visible {
        return;
    }
    // Several fills / strokes: one plain shape per paint.
    if crate::appearance::needs_expand(&s) {
        if let Some(g) = crate::appearance::expand(&s) {
            push_item(std::borrow::Cow::Owned(g), alpha, doc, fonts, real_text, out);
            return;
        }
    }
    if let crate::document::ShapeKind::Group { children } = &s.kind {
        let a = alpha * s.opacity;
        let fx = render::group_fx(&s);
        let at = out.len();
        if fx {
            let mut g = s.clone().into_owned();
            g.opacity = a;
            let rings = if g.outlines.is_empty() { Vec::new() } else { render::outline_rings(&g, &BezPath::new(), fonts) };
            let mask = mask_items(&g, doc, fonts);
            out.push(Item { shape: std::borrow::Cow::Owned(g), path: BezPath::new(), clip: None, rings, span: 0, text: None, mask });
        }
        for c in children {
            push_item(std::borrow::Cow::Owned(c.clone()), a, doc, fonts, real_text, out);
        }
        if fx {
            let mut path = BezPath::new();
            for it in &out[at + 1..] {
                path.extend(it.path.elements().iter().copied());
            }
            out[at].span = out.len() - at - 1;
            out[at].path = path;
        }
        return;
    }
    let s = if alpha < 1.0 {
        let mut o = s.into_owned();
        o.opacity *= alpha;
        std::borrow::Cow::Owned(o)
    } else {
        s
    };
    let (path, text) = render::compute_outline(&s, fonts);
    let clip = s.clip.as_ref().map(|c| (render::compute_outline(c, fonts).0, c.fill_rule));
    let rings = if s.outlines.is_empty() { Vec::new() } else { render::outline_rings(&s, &path, fonts) };
    // `path` holds every glyph, whatever its colour.
    if text.as_ref().is_some_and(|t| real_text && real_text_ok(&s, t)) {
        let mask = mask_items(&s, doc, fonts);
        out.push(Item { shape: s, path, clip, rings, span: 0, text, mask });
        return;
    }
    match text.filter(|t| !t.colored.is_empty()) {
        // Coloured text ranges: one item per colour (the first keeps the
        // shape's own fill and its effects).
        Some(t) => {
            let pieces: Vec<(BezPath, crate::document::Rgba)> = t.colored.iter().map(|(p, c)| (s.transform * p.clone(), *c)).collect();
            let main = s.transform * t.path.clone();
            let base = s.clone().into_owned();
            let mask = mask_items(&s, doc, fonts);
            out.push(Item { shape: s, path: main, clip: clip.clone(), rings, span: 0, text: None, mask });
            for (p, c) in pieces {
                let mut piece = base.clone();
                piece.fill = Some(crate::document::Paint::Solid(c));
                piece.fill_key = None;
                piece.outlines.clear();
                let mask = mask_items(&piece, doc, fonts);
                out.push(Item { shape: std::borrow::Cow::Owned(piece), path: p, clip: clip.clone(), rings: Vec::new(), span: 0, text: None, mask });
            }
        }
        None => {
            let mask = mask_items(&s, doc, fonts);
            out.push(Item { shape: s, path, clip, rings, span: 0, text: None, mask });
        }
    }
}

// Compact number formatting for text-based formats.
thread_local! {
    /// Decimal places of [`num`] (SVG export option; 3 otherwise).
    pub static PRECISION: std::cell::Cell<usize> = const { std::cell::Cell::new(3) };
}

pub fn num(v: f64) -> String {
    let s = format!("{:.*}", PRECISION.with(|p| p.get()), v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.into() }
}

pub fn coeffs(a: Affine) -> String {
    a.as_coeffs().iter().map(|v| num(*v)).collect::<Vec<_>>().join(" ")
}

/// Iterate path elements as (op, points) — helper for PS/PDF writers.
pub fn for_each_el(path: &BezPath, mut f: impl FnMut(&PathEl, Option<kurbo::Point>)) {
    let mut last = None;
    for el in path.elements() {
        f(el, last);
        last = match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(p),
            PathEl::QuadTo(_, p) => Some(p),
            PathEl::CurveTo(_, _, p) => Some(p),
            PathEl::ClosePath => last,
        };
    }
}

/// Save without a preview or backup (autosave, tests).
pub fn save_native(doc: &Document, path: &Path) -> Result<(), String> {
    native::save(doc, path, None, native::SaveOptions::default())
}

pub fn load_native(path: &Path) -> Result<Document, String> {
    native::load(path)
}

pub fn parse_native(data: &[u8]) -> Result<Document, String> {
    native::decode(data)
}

pub enum Format {
    Native,
    Svg,
    Pdf,
    Eps,
}

pub fn format_of(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mvec" => Format::Native,
        "svg" | "svgz" => Format::Svg,
        "pdf" | "ai" => Format::Pdf,
        "eps" | "ps" => Format::Eps,
        _ => return None,
    })
}

/// Illustrator files from before CS (and some saved without PDF
/// compatibility) are PostScript rather than PDF.
fn is_postscript(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"%!PS"
}

/// Open any supported file into a new document.
pub fn open(path: &Path, fonts: &mut FontManager) -> Result<Document, String> {
    match format_of(path).ok_or("Unsupported file type")? {
        Format::Native => load_native(path),
        Format::Svg => svg::import(&std::fs::read(path).map_err(|e| e.to_string())?, fonts),
        Format::Pdf if is_postscript(path) => eps::import(path),
        Format::Pdf => pdf::import(path),
        Format::Eps => eps::import(path),
    }
}

