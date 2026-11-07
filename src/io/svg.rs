//! SVG import (via usvg) and export.

use std::fmt::Write as _;
use std::sync::Arc;

use base64::Engine;
use kurbo::{Affine, BezPath, PathEl, Point};

use super::{Item, coeffs, num};
use crate::document::{
    Document, FillRule, ImageAsset, ImageFill, ImageMode, LineCap, LineJoin, Paint, Rgba, Shape, ShapeKind, Stroke,
};
use crate::fonts::FontManager;
use crate::{geom, render, strokes};

pub fn path_d(path: &BezPath) -> String {
    let mut s = String::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => write!(s, "M{} {}", num(p.x), num(p.y)),
            PathEl::LineTo(p) => write!(s, "L{} {}", num(p.x), num(p.y)),
            PathEl::QuadTo(a, p) => write!(s, "Q{} {} {} {}", num(a.x), num(a.y), num(p.x), num(p.y)),
            PathEl::CurveTo(a, b, p) => write!(
                s,
                "C{} {} {} {} {} {}",
                num(a.x),
                num(a.y),
                num(b.x),
                num(b.y),
                num(p.x),
                num(p.y)
            ),
            PathEl::ClosePath => write!(s, "Z"),
        }
        .unwrap();
    }
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Width/height attribute in the document unit (plain px stays unitless).
fn physical(doc: &Document, px: f64) -> String {
    match doc.unit {
        crate::units::Unit::Px => num(px),
        u => format!("{}{}", num(px * u.per_px(doc.dpi)), u.suffix()),
    }
}

/// Native stroke attributes (alignment and arrowheads handled by the caller).
fn stroke_attrs(s: &Stroke) -> String {
    let mut a = stroke_attrs_solid(s);
    if s.is_dashed() {
        let d: Vec<String> = s.dash_px().iter().map(|v| num(*v)).collect();
        write!(a, r#" stroke-dasharray="{}""#, d.join(" ")).unwrap();
        if s.dash_offset != 0.0 {
            write!(a, r#" stroke-dashoffset="{}""#, num(s.dash_offset * s.width)).unwrap();
        }
    }
    a
}

fn stroke_attrs_solid(s: &Stroke) -> String {
    let mut a = format!(r#" stroke="{}" stroke-width="{}""#, s.color.to_hex(), num(s.width));
    if s.join == LineJoin::Miter && (s.miter_limit - 4.0).abs() > 1e-9 {
        write!(a, r#" stroke-miterlimit="{}""#, num(s.miter_limit)).unwrap();
    }
    if s.color.a < 1.0 {
        write!(a, r#" stroke-opacity="{}""#, num(s.color.a as f64)).unwrap();
    }
    match s.join {
        LineJoin::Miter => {}
        LineJoin::Round => a.push_str(r#" stroke-linejoin="round""#),
        LineJoin::Bevel => a.push_str(r#" stroke-linejoin="bevel""#),
    }
    match s.cap {
        LineCap::Butt => {}
        LineCap::Round => a.push_str(r#" stroke-linecap="round""#),
        LineCap::Square => a.push_str(r#" stroke-linecap="square""#),
    }
    a
}

/// Stroke markup for a path: native attributes when SVG can express it,
/// otherwise separate elements built from the stroke geometry.
enum StrokeSvg {
    Attrs(String),
    Elements(String),
}

fn stroke_svg(s: &crate::document::Shape, path: &kurbo::BezPath, d: &str, indent: &str) -> StrokeSvg {
    let Some(st) = s.stroke.as_ref().filter(|st| st.width > 0.0) else { return StrokeSvg::Attrs(String::new()) };
    if strokes::is_plain(st, path) {
        return StrokeSvg::Attrs(stroke_attrs(st));
    }
    let _ = d;
    let mut out = String::new();
    let opacity = if st.color.a < 1.0 { format!(r#" fill-opacity="{}""#, num(st.color.a as f64)) } else { String::new() };
    for part in strokes::stroke_parts(path, st, s.fill_rule) {
        match part {
            strokes::StrokePart::Line(l) => {
                writeln!(out, r#"{indent}<path d="{}" fill="none"{}/>"#, path_d(&l), stroke_attrs_solid(st)).unwrap();
            }
            strokes::StrokePart::Fill(f, rule) => {
                let r = if rule == FillRule::EvenOdd { r#" fill-rule="evenodd""# } else { "" };
                writeln!(out, r#"{indent}<path d="{}" fill="{}"{opacity}{r}/>"#, path_d(&f), st.color.to_hex()).unwrap();
            }
        }
    }
    StrokeSvg::Elements(out)
}

/// `<filter>` for a drop shadow covering `bounds` (document space).
fn shadow_filter(id: &str, sh: &crate::document::Shadow, bounds: kurbo::Rect) -> String {
    let r = bounds.inflate(sh.sigma() * 3.0 + 2.0, sh.sigma() * 3.0 + 2.0);
    let r = r.union(r + kurbo::Vec2::new(sh.dx, sh.dy));
    format!(
        r#"    <filter id="{id}" filterUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" color-interpolation-filters="sRGB"><feDropShadow dx="{}" dy="{}" stdDeviation="{}" flood-color="{}" flood-opacity="{}"/></filter>"#,
        num(r.x0),
        num(r.y0),
        num(r.width()),
        num(r.height()),
        num(sh.dx),
        num(sh.dy),
        num(sh.sigma()),
        sh.color.to_hex(),
        num(sh.color.a as f64)
    )
}

/// Drop and/or inner shadow as one `<filter id="shadow{i}">`: the drop
/// shadow under the graphic, the inner shadow over it (clipped to it).
fn shadows_filter(id: &str, drop: Option<&crate::document::Shadow>, inner: Option<&crate::document::Shadow>, bounds: kurbo::Rect) -> String {
    let (drop, inner) = match (drop, inner) {
        (Some(d), None) => return shadow_filter(id, d, bounds),
        (d, Some(n)) => (d, n),
        (None, None) => return String::new(),
    };
    let mut r = bounds.inflate(inner.reach() + 2.0, inner.reach() + 2.0);
    if let Some(d) = drop {
        let dr = bounds.inflate(d.sigma() * 3.0 + 2.0, d.sigma() * 3.0 + 2.0);
        r = r.union(dr).union(dr + kurbo::Vec2::new(d.dx, d.dy));
    }
    let mut f = format!(r#"    <filter id="{id}" {} color-interpolation-filters="sRGB">"#, region(r));
    let mut merge = String::new();
    if let Some(d) = drop {
        write!(
            f,
            r#"<feGaussianBlur in="SourceAlpha" stdDeviation="{}"/><feOffset dx="{}" dy="{}" result="dropOff"/><feFlood flood-color="{}" flood-opacity="{}"/><feComposite in2="dropOff" operator="in" result="drop"/>"#,
            num(d.sigma()),
            num(d.dx),
            num(d.dy),
            d.color.to_hex(),
            num(d.color.a as f64)
        )
        .unwrap();
        merge.push_str(r#"<feMergeNode in="drop"/>"#);
    }
    // Inner: the blurred, offset outside of the shape, kept inside it.
    write!(
        f,
        r#"<feComponentTransfer in="SourceAlpha" result="outside"><feFuncA type="table" tableValues="1 0"/></feComponentTransfer><feGaussianBlur in="outside" stdDeviation="{}"/><feOffset dx="{}" dy="{}" result="innerOff"/><feFlood flood-color="{}" flood-opacity="{}"/><feComposite in2="innerOff" operator="in"/><feComposite in2="SourceAlpha" operator="in" result="inner"/>"#,
        num(inner.sigma()),
        num(inner.dx),
        num(inner.dy),
        inner.color.to_hex(),
        num(inner.color.a as f64)
    )
    .unwrap();
    write!(f, r#"<feMerge>{merge}<feMergeNode in="SourceGraphic"/><feMergeNode in="inner"/></feMerge></filter>"#).unwrap();
    f
}

fn region(r: kurbo::Rect) -> String {
    format!(r#"filterUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}""#, num(r.x0), num(r.y0), num(r.width()), num(r.height()))
}

/// Wrap the markup written for `item` since `start` with its noise filter,
/// outline rings, clip mask and layer blur.
fn wrap_effects(i: usize, item: &Item, bounds: Option<kurbo::Rect>, start: usize, body: &mut String, defs: &mut String) {
    let s: &Shape = &item.shape;
    if s.noise <= 0.0 && s.blur <= 0.0 && item.clip.is_none() && item.rings.is_empty() {
        return;
    }
    let mut inner = body.split_off(start);
    let b = bounds.unwrap_or(kurbo::Rect::new(0.0, 0.0, 1.0, 1.0));
    if s.noise > 0.0 {
        writeln!(
            defs,
            r#"    <filter id="noise{i}" {} color-interpolation-filters="sRGB"><feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="3" stitchTiles="stitch" result="t"/><feColorMatrix in="t" type="saturate" values="0" result="g"/><feComponentTransfer in="g" result="n"><feFuncA type="linear" slope="0" intercept="{}"/></feComponentTransfer><feComposite in="n" in2="SourceGraphic" operator="in" result="m"/><feBlend in="m" in2="SourceGraphic" mode="overlay"/></filter>"#,
            region(b),
            num(s.noise.clamp(0.0, 1.0) as f64)
        )
        .unwrap();
        inner = format!("  <g filter=\"url(#noise{i})\">\n{inner}  </g>\n");
    }
    let mut attrs = String::new();
    if let Some((cp, rule)) = &item.clip {
        let r = if *rule == FillRule::EvenOdd { r#" clip-rule="evenodd""# } else { "" };
        writeln!(defs, r#"    <clipPath id="mask{i}"><path d="{}"{r}/></clipPath>"#, path_d(cp)).unwrap();
        write!(attrs, r#" clip-path="url(#mask{i})""#).unwrap();
    }
    if s.blur > 0.0 {
        writeln!(defs, r#"    <filter id="blur{i}" {}><feGaussianBlur stdDeviation="{}"/></filter>"#, region(b), num(s.blur / 2.0)).unwrap();
        write!(attrs, r#" filter="url(#blur{i})""#).unwrap();
    }
    let mut out = format!("  <g{attrs}>\n");
    let op = if s.opacity < 1.0 { format!(r#" opacity="{}""#, num(s.opacity as f64)) } else { String::new() };
    for (k, (p, paint)) in item.rings.iter().enumerate() {
        let fill = match paint {
            Paint::Gradient(g) => {
                let id = format!("ring{i}_{k}");
                writeln!(defs, "{}", gradient_def(&id, g, s.transform, render::ring_bounds(s, p))).unwrap();
                format!(r#"url(#{id})""#)
            }
            other => {
                let c = other.solid().unwrap_or(crate::document::Rgba::BLACK);
                let a = if c.a < 1.0 { format!(r#" fill-opacity="{}""#, num(c.a as f64)) } else { String::new() };
                format!(r#"{}"{a}"#, c.to_hex())
            }
        };
        writeln!(out, r#"    <path d="{}" fill="{fill} fill-rule="evenodd"{op}/>"#, path_d(p)).unwrap();
    }
    out.push_str(&inner);
    out.push_str("  </g>\n");
    body.push_str(&out);
}

/// Wrap a group's members (written since `start`) with the group's
/// effects, and its drop shadow outermost.
fn close_group(i: usize, item: &Item, start: usize, body: &mut String, defs: &mut String) {
    let s: &Shape = &item.shape;
    let bounds = crate::geom::path_bounds(&item.path).map(|b| {
        let r = render::effect_reach(s) + s.blur * 1.5 + 2.0;
        b.inflate(r, r)
    });
    // Inner shadow over the members (not the rings), drop shadow outermost.
    let id = layer_id(&s.name);
    let wrap = |filter: String, fid: String, named: bool, body: &mut String, defs: &mut String| {
        writeln!(defs, "{filter}").unwrap();
        let inner = body.split_off(start);
        let name = if named && !id.is_empty() { format!(r#" id="{id}""#) } else { String::new() };
        writeln!(body, r#"  <g{name} filter="url(#{fid})">"#).unwrap();
        body.push_str(&inner);
        body.push_str("  </g>\n");
    };
    if let (Some(sh), Some(b)) = (&s.inner_shadow, bounds) {
        wrap(shadows_filter(&format!("inner{i}"), None, Some(sh), b), format!("inner{i}"), false, body, defs);
    }
    wrap_effects(i, item, bounds, start, body, defs);
    if let (Some(sh), Some(b)) = (&s.shadow, bounds) {
        wrap(shadow_filter(&format!("shadow{i}"), sh, b), format!("shadow{i}"), true, body, defs);
    }
}

/// Real text: a `<text>` with one `<tspan>` per glyph run, positioned as
/// laid out (per character where glyphs map one-to-one), plus underline
/// and strike bars. Local coordinates; the caller wraps it in the shape's
/// transform.
fn text_svg(t: &crate::text::TextLayout, fill_attr: &str, base: Option<Rgba>, indent: &str) -> String {
    let mut out = String::new();
    writeln!(out, r#"{indent}<text xml:space="preserve"{fill_attr}>"#).unwrap();
    for run in &t.runs {
        let face = t.faces.get(run.face);
        let st = &run.style;
        let mut fam = Vec::new();
        if let Some(f) = face {
            fam.push(f.family.clone());
        }
        if !fam.contains(&st.family) {
            fam.push(st.family.clone());
        }
        let family = fam.iter().map(|f| format!("'{}'", esc(&f.replace('\'', "")))).collect::<Vec<_>>().join(", ");
        let mut attrs = format!(r#" font-family="{family}" font-size="{}""#, num(st.size));
        if st.weight != 400 {
            write!(attrs, r#" font-weight="{}""#, st.weight).unwrap();
        }
        if st.italic {
            attrs.push_str(r#" font-style="italic""#);
        }
        if let Some(c) = st.fill.filter(|c| Some(*c) != base) {
            write!(attrs, r#" fill="{}""#, c.to_hex()).unwrap();
            if c.a < 1.0 {
                write!(attrs, r#" fill-opacity="{}""#, num(c.a as f64)).unwrap();
            }
        }
        let mut css = Vec::new();
        if !st.features.is_empty() {
            css.push(format!("font-feature-settings:{}", st.features.iter().map(|f| format!("'{}' {}", f.tag, f.value)).collect::<Vec<_>>().join(",")));
        }
        if !st.variations.is_empty() {
            css.push(format!("font-variation-settings:{}", st.variations.iter().map(|v| format!("'{}' {}", v.tag, num(v.value as f64))).collect::<Vec<_>>().join(",")));
        }
        if !css.is_empty() {
            write!(attrs, r#" style="{}""#, esc(&css.join(";"))).unwrap();
        }
        let chars = run.text.chars().count();
        let simple = !run.rtl && chars == run.glyphs.len() && run.glyphs.iter().all(|g| g.text.chars().count() == 1);
        let y = run.glyphs.first().map(|g| g.y).unwrap_or(run.origin.y);
        if simple {
            let xs: Vec<String> = run.glyphs.iter().map(|g| num(g.x)).collect();
            writeln!(out, r#"{indent}  <tspan x="{}" y="{}"{attrs}>{}</tspan>"#, xs.join(" "), num(y), esc(&run.text)).unwrap();
        } else {
            writeln!(out, r#"{indent}  <tspan x="{}" y="{}" textLength="{}"{attrs}>{}</tspan>"#, num(run.origin.x), num(y), num(run.width.max(0.0)), esc(&run.text)).unwrap();
        }
    }
    writeln!(out, "{indent}</text>").unwrap();
    // Decorations, grouped by colour.
    let mut by_color: Vec<(Option<Rgba>, kurbo::BezPath)> = Vec::new();
    for (r, c) in &t.decorations {
        let c = c.filter(|c| Some(*c) != base);
        let i = match by_color.iter().position(|(k, _)| *k == c) {
            Some(i) => i,
            None => {
                by_color.push((c, kurbo::BezPath::new()));
                by_color.len() - 1
            }
        };
        by_color[i].1.extend(kurbo::Shape::to_path(r, 0.1).elements().iter().copied());
    }
    for (c, p) in by_color {
        let fill = match c {
            Some(c) => format!(r#" fill="{}""#, c.to_hex()),
            None => String::new(),
        };
        writeln!(out, r#"{indent}<path d="{}"{fill}/>"#, path_d(&p)).unwrap();
    }
    out
}

/// SVG export settings.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgOptions {
    /// Text as `<text>` (else outlines).
    pub real_text: bool,
    /// Decimal places of coordinates.
    pub precision: usize,
    /// Layer names as element ids (made unique).
    pub ids: bool,
    /// No indentation or line breaks.
    pub minify: bool,
    /// No width/height: scales to its container (viewBox only).
    pub responsive: bool,
}

impl Default for SvgOptions {
    fn default() -> Self {
        SvgOptions { real_text: true, precision: 3, ids: true, minify: false, responsive: false }
    }
}

thread_local! {
    /// Layer ids of the export in progress: on, and those used so far.
    static IDS: std::cell::RefCell<(bool, std::collections::HashSet<String>)> = std::cell::RefCell::new((true, Default::default()));
}

/// A unique, valid id from a layer name ("" when ids are off).
fn layer_id(name: &str) -> String {
    IDS.with(|c| {
        let mut c = c.borrow_mut();
        if !c.0 {
            return String::new();
        }
        let mut base: String = name.chars().map(|ch| if ch.is_alphanumeric() || ch == '-' || ch == '_' || ch == '.' { ch } else { '_' }).collect();
        if base.is_empty() || !base.starts_with(|ch: char| ch.is_alphabetic() || ch == '_') {
            base.insert(0, '_');
        }
        let mut id = base.clone();
        let mut n = 2;
        while c.1.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        c.1.insert(id.clone());
        esc(&id)
    })
}

/// SVG of the first artboard with `opts`.
pub fn export_svg(doc: &Document, fonts: &mut FontManager, opts: &SvgOptions) -> String {
    let prev = crate::io::PRECISION.with(|p| p.replace(opts.precision.clamp(0, 8)));
    IDS.with(|c| *c.borrow_mut() = (opts.ids, Default::default()));
    let mut out = export_with(doc, fonts, opts.real_text);
    crate::io::PRECISION.with(|p| p.set(prev));
    IDS.with(|c| *c.borrow_mut() = (true, Default::default()));
    if opts.responsive {
        if let (Some(a), Some(b)) = (out.find(" width=\""), out.find(" viewBox=")) {
            if a < b {
                out.replace_range(a..b, "");
            }
        }
    }
    if opts.minify {
        out = minify(&out);
    }
    out
}

/// Drop indentation and line breaks between tags (kept inside `<text>`,
/// where whitespace is content).
fn minify(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut in_text = 0usize;
    for line in svg.lines() {
        let t = line.trim_start();
        if in_text > 0 {
            out.push_str(line);
            out.push('\n');
        } else {
            out.push_str(t);
        }
        in_text += t.matches("<text").count();
        in_text = in_text.saturating_sub(t.matches("</text>").count());
    }
    out.push('\n');
    out
}

/// SVG of the first artboard, text as outlines.
pub fn export(doc: &Document, fonts: &mut FontManager) -> String {
    export_with(doc, fonts, false)
}

/// SVG of the first artboard; `real_text` keeps text as `<text>`.
pub fn export_with(doc: &Document, fonts: &mut FontManager, real_text: bool) -> String {
    IDS.with(|c| c.borrow_mut().1.clear());
    let doc = &*doc.at_origin();
    let items = crate::io::prepare_with(doc, fonts, real_text);
    export_items(doc, &items)
}

/// `items` must come from `doc` with its first artboard at the origin.
pub fn export_items(doc: &Document, items: &[Item]) -> String {
    // Layer ids are unique per file (left over ids made every render slower).
    IDS.with(|c| c.borrow_mut().1.clear());
    let (dw, dh, bg) = (doc.primary().w, doc.primary().h, doc.primary().background);
    let mut out = String::new();
    let mut defs = String::new();
    let mut body = String::new();
    writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{pw}" height="{ph}" viewBox="0 0 {w} {h}">"#,
        pw = physical(doc, dw),
        ph = physical(doc, dh),
        w = num(dw),
        h = num(dh)
    )
    .unwrap();
    if bg.a > 0.0 {
        writeln!(
            body,
            r#"  <rect width="{}" height="{}" fill="{}"{}/>"#,
            num(dw),
            num(dh),
            bg.to_hex(),
            if bg.a < 1.0 { format!(r#" fill-opacity="{}""#, num(bg.a as f64)) } else { String::new() }
        )
        .unwrap();
    }
    let (d, b) = items_markup(doc, items);
    defs.push_str(&d);
    body.push_str(&b);
    if !defs.is_empty() {
        writeln!(out, "  <defs>\n{defs}  </defs>").unwrap();
    }
    out.push_str(&body);
    out.push_str("</svg>\n");
    out
}

/// Prefix every id (and reference to one) in generated markup.
fn prefix_ids(s: &str, p: &str) -> String {
    s.replace("id=\"", &format!("id=\"{p}")).replace("url(#", &format!("url(#{p}")).replace("href=\"#", &format!("href=\"#{p}"))
}

/// Wrap the markup written for item `i` since `start` with its blend mode
/// and mask.
fn wrap_blend_mask(doc: &Document, i: usize, item: &Item, start: usize, body: &mut String, defs: &mut String) {
    let s: &Shape = &item.shape;
    if s.blend.is_normal() && item.mask.is_none() {
        return;
    }
    let mut attrs = String::new();
    if !s.blend.is_normal() {
        write!(attrs, r#" style="mix-blend-mode:{}""#, s.blend.css()).unwrap();
    }
    if let Some((mitems, mode, invert)) = &item.mask {
        let p = format!("mk{i}_");
        let (md, mb) = items_markup(doc, mitems);
        defs.push_str(&prefix_ids(&md, &p));
        let mb = prefix_ids(&mb, &p);
        // Generous region: the shape, its mask and effects.
        let mut r = crate::geom::path_bounds(&item.path).unwrap_or_default();
        for it in mitems {
            if let Some(b) = crate::geom::path_bounds(&it.path) {
                r = r.union(b);
            }
        }
        let reach = render::effect_reach(s) + render::shadow_reach(s) + s.blur * 1.5 + 4.0;
        let r = r.inflate(reach, reach);
        let reg = format!(r#"x="{}" y="{}" width="{}" height="{}""#, num(r.x0), num(r.y0), num(r.width()), num(r.height()));
        let ty = if *mode == crate::document::MaskMode::Alpha { r#" mask-type="alpha" style="mask-type:alpha""# } else { "" };
        let content = if *invert {
            let f = if *mode == crate::document::MaskMode::Alpha {
                r#"<feComponentTransfer><feFuncA type="table" tableValues="1 0"/></feComponentTransfer>"#
            } else {
                r#"<feColorMatrix type="matrix" values="-1 0 0 0 1 0 -1 0 0 1 0 0 -1 0 1 0 0 0 1 0"/>"#
            };
            writeln!(defs, r#"    <filter id="mkinv{i}" filterUnits="userSpaceOnUse" {reg} color-interpolation-filters="sRGB">{f}</filter>"#).unwrap();
            let bg = if *mode == crate::document::MaskMode::Alpha { String::new() } else { format!(r##"<rect {reg} fill="#000000"/>"##) };
            format!("<g filter=\"url(#mkinv{i})\">{bg}\n{mb}</g>")
        } else {
            mb
        };
        writeln!(defs, r#"    <mask id="mask_{i}" maskUnits="userSpaceOnUse" maskContentUnits="userSpaceOnUse" {reg}{ty}>{content}</mask>"#).unwrap();
        write!(attrs, r#" mask="url(#mask_{i})""#).unwrap();
    }
    let inner = body.split_off(start);
    body.push_str(&format!("  <g{attrs}>\n{inner}  </g>\n"));
}

/// Defs and body markup of export items.
fn items_markup(doc: &Document, items: &[Item]) -> (String, String) {
    let mut defs = String::new();
    let mut body = String::new();
    let b64 = base64::engine::general_purpose::STANDARD;
    // Open effect groups: (item index, body offset where their members start).
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        while open.last().is_some_and(|&(g, _)| i > g + items[g].span) {
            let (g, start) = open.pop().unwrap();
            close_group(g, &items[g], start, &mut body, &mut defs);
            wrap_blend_mask(doc, g, &items[g], start, &mut body, &mut defs);
        }
        if item.span > 0 {
            open.push((i, body.len()));
            continue;
        }
        let s: &Shape = &item.shape;
        let d = path_d(&item.path);
        if d.is_empty() {
            continue;
        }
        let start = body.len();
        let fx_bounds = crate::geom::path_bounds(&item.path).map(|b| {
            let r = crate::render::effect_reach(s) + s.blur * 1.5 + 2.0;
            b.inflate(r, r)
        });
        let rule = if s.fill_rule == FillRule::EvenOdd { r#" fill-rule="evenodd""# } else { "" };
        let opacity = if s.opacity < 1.0 { format!(r#" opacity="{}""#, num(s.opacity as f64)) } else { String::new() };
        let id = match layer_id(&s.name) {
            n if n.is_empty() => String::new(),
            n => format!(r#" id="{n}""#),
        };
        let filter = match (s.shadow.is_some() || s.inner_shadow.is_some(), crate::geom::path_bounds(&item.path)) {
            (true, Some(b)) => {
                let reach = s.stroke.as_ref().map(|st| st.reach()).unwrap_or(0.0);
                writeln!(defs, "{}", shadows_filter(&format!("shadow{i}"), s.shadow.as_ref(), s.inner_shadow.as_ref(), b.inflate(reach, reach))).unwrap();
                format!(r#" filter="url(#shadow{i})""#)
            }
            _ => String::new(),
        };
        match &s.fill {
            Some(Paint::Image(img)) if doc.images.contains_key(&img.image) => {
                let asset = &doc.images[&img.image];
                let href = format!("data:image/png;base64,{}", b64.encode(asset.png_bytes()));
                let lb = render::shape_local_bounds(s, &item.path);
                let (iw, ih) = (asset.width as f64, asset.height as f64);
                body.push_str(&format!("  <g{id}{opacity}{filter}>\n"));
                if img.mode == ImageMode::Tile {
                    let sc = img.scale.max(0.01) as f64;
                    writeln!(
                        defs,
                        r#"    <pattern id="pat{i}" patternUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" patternTransform="matrix({})"><image width="{}" height="{}" preserveAspectRatio="none" opacity="{}" xlink:href="{href}"/></pattern>"#,
                        num(lb.x0),
                        num(lb.y0),
                        num(iw * sc),
                        num(ih * sc),
                        coeffs(s.transform),
                        num(iw * sc),
                        num(ih * sc),
                        num(img.opacity as f64)
                    )
                    .unwrap();
                    writeln!(body, r#"    <path d="{d}" fill="url(#pat{i})"{rule}/>"#).unwrap();
                } else {
                    let r = render::image_rect(img.mode, lb, iw, ih);
                    writeln!(defs, r#"    <clipPath id="clip{i}"><path d="{d}"{}/></clipPath>"#, rule.replace("fill-rule", "clip-rule")).unwrap();
                    writeln!(
                        body,
                        r#"    <g clip-path="url(#clip{i})"><image transform="matrix({})" x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none" opacity="{}" xlink:href="{href}"/></g>"#,
                        coeffs(s.transform),
                        num(r.x0),
                        num(r.y0),
                        num(r.width()),
                        num(r.height()),
                        num(img.opacity as f64)
                    )
                    .unwrap();
                }
                match stroke_svg(s, &item.path, &d, "    ") {
                    StrokeSvg::Attrs(a) if !a.is_empty() => writeln!(body, r#"    <path d="{d}" fill="none"{a}/>"#).unwrap(),
                    StrokeSvg::Attrs(_) => {}
                    StrokeSvg::Elements(e) => body.push_str(&e),
                }
                body.push_str("  </g>\n");
            }
            fill => {
                let fill_attr = match fill {
                    Some(Paint::Gradient(g)) => {
                        let lb = render::shape_local_bounds(s, &item.path);
                        writeln!(defs, "{}", gradient_def(&format!("grad{i}"), g, s.transform, lb)).unwrap();
                        format!(r#" fill="url(#grad{i})""#)
                    }
                    _ => match fill.as_ref().and_then(|f| f.solid()) {
                    Some(c) => {
                        let mut a = format!(r#" fill="{}""#, c.to_hex());
                        if c.a < 1.0 {
                            write!(a, r#" fill-opacity="{}""#, num(c.a as f64)).unwrap();
                        }
                        a
                    }
                    None => r#" fill="none""#.to_string(),
                    },
                };
                if let Some(t) = &item.text {
                    // Real text in the shape's frame; the stroke (in document
                    // space) is drawn from the outlines over it.
                    writeln!(body, r#"  <g{id}{opacity}{filter}>"#).unwrap();
                    if fill.is_some() {
                        writeln!(body, r#"    <g transform="matrix({})">"#, coeffs(s.transform)).unwrap();
                        body.push_str(&text_svg(t, &fill_attr, fill.as_ref().and_then(|f| f.solid()), "      "));
                        body.push_str("    </g>\n");
                    }
                    match stroke_svg(s, &item.path, &d, "    ") {
                        StrokeSvg::Attrs(a) if !a.is_empty() => writeln!(body, r#"    <path d="{d}" fill="none"{a}/>"#).unwrap(),
                        StrokeSvg::Attrs(_) => {}
                        StrokeSvg::Elements(e) => body.push_str(&e),
                    }
                    body.push_str("  </g>\n");
                    wrap_effects(i, item, fx_bounds, start, &mut body, &mut defs);
                    wrap_blend_mask(doc, i, item, start, &mut body, &mut defs);
                    continue;
                }
                match stroke_svg(s, &item.path, &d, "    ") {
                    StrokeSvg::Attrs(stroke) => {
                        writeln!(body, r#"  <path{id} d="{d}"{fill_attr}{rule}{stroke}{opacity}{filter}/>"#).unwrap();
                    }
                    StrokeSvg::Elements(e) => {
                        writeln!(body, r#"  <g{id}{opacity}{filter}>"#).unwrap();
                        if fill.is_some() {
                            writeln!(body, r#"    <path d="{d}"{fill_attr}{rule}/>"#).unwrap();
                        }
                        body.push_str(&e);
                        body.push_str("  </g>\n");
                    }
                }
            }
        }
        wrap_effects(i, item, fx_bounds, start, &mut body, &mut defs);
        wrap_blend_mask(doc, i, item, start, &mut body, &mut defs);
    }
    while let Some((g, start)) = open.pop() {
        close_group(g, &items[g], start, &mut body, &mut defs);
        wrap_blend_mask(doc, g, &items[g], start, &mut body, &mut defs);
    }
    (defs, body)
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

fn color_of(paint: &usvg::Paint, opacity: f32) -> Rgba {
    let c = match paint {
        usvg::Paint::Color(c) => *c,
        usvg::Paint::LinearGradient(g) => g.stops().first().map(|s| s.color()).unwrap_or(usvg::Color::black()),
        usvg::Paint::RadialGradient(g) => g.stops().first().map(|s| s.color()).unwrap_or(usvg::Color::black()),
        usvg::Paint::Pattern(_) => usvg::Color::new_rgb(128, 128, 128),
    };
    Rgba { r: c.red as f32 / 255.0, g: c.green as f32 / 255.0, b: c.blue as f32 / 255.0, a: opacity }
}

/// An SVG gradient as our gradient in the path's bounding-box units
/// (`t`: the path's transform to document space, `path`: in document space).
fn gradient_of(paint: &usvg::Paint, opacity: f32, t: Affine, path: &BezPath) -> Option<Paint> {
    use crate::document::{Gradient, GradientKind, GradientStop};
    let b = geom::path_bounds(path)?;
    let unit = Affine::new([b.width().max(1e-9), 0.0, 0.0, b.height().max(1e-9), b.x0, b.y0]).inverse();
    let stops_of = |st: &[usvg::Stop]| -> Vec<GradientStop> {
        st.iter()
            .map(|s| {
                let c = s.color();
                GradientStop::new(s.offset().get(), Rgba { r: c.red as f32 / 255.0, g: c.green as f32 / 255.0, b: c.blue as f32 / 255.0, a: s.opacity().get() * opacity })
            })
            .collect()
    };
    let pt = |m: Affine, x: f32, y: f32| {
        let q = m * kurbo::Point::new(x as f64, y as f64);
        [q.x, q.y]
    };
    match paint {
        usvg::Paint::LinearGradient(g) => {
            let m = unit * t * affine_of(g.transform());
            Some(Paint::Gradient(Gradient { kind: GradientKind::Linear, stops: stops_of(g.stops()), start: pt(m, g.x1(), g.y1()), end: pt(m, g.x2(), g.y2()), mesh: None }))
        }
        usvg::Paint::RadialGradient(g) => {
            let m = unit * t * affine_of(g.transform());
            let r = g.r().get();
            Some(Paint::Gradient(Gradient { kind: GradientKind::Radial, stops: stops_of(g.stops()), start: pt(m, g.cx(), g.cy()), end: pt(m, g.cx() + r, g.cy()), mesh: None }))
        }
        _ => None,
    }
}

fn affine_of(t: usvg::Transform) -> Affine {
    Affine::new([t.sx as f64, t.ky as f64, t.kx as f64, t.sy as f64, t.tx as f64, t.ty as f64])
}

fn bez_of(p: &usvg::tiny_skia_path::Path) -> BezPath {
    use usvg::tiny_skia_path::PathSegment as S;
    let mut b = BezPath::new();
    let pt = |p: usvg::tiny_skia_path::Point| Point::new(p.x as f64, p.y as f64);
    for seg in p.segments() {
        match seg {
            S::MoveTo(a) => b.move_to(pt(a)),
            S::LineTo(a) => b.line_to(pt(a)),
            S::QuadTo(a, c) => b.quad_to(pt(a), pt(c)),
            S::CubicTo(a, c, d) => b.curve_to(pt(a), pt(c), pt(d)),
            S::Close => b.close_path(),
        }
    }
    b
}

/// A drop shadow declared through `feDropShadow` on a group.
fn group_shadow(g: &usvg::Group) -> Option<crate::document::Shadow> {
    for f in g.filters() {
        for p in f.primitives() {
            if let usvg::filter::Kind::DropShadow(ds) = p.kind() {
                let c = ds.color();
                let color = Rgba { r: c.red as f32 / 255.0, g: c.green as f32 / 255.0, b: c.blue as f32 / 255.0, a: ds.opacity().get() };
                return Some(crate::document::Shadow { color, dx: ds.dx() as f64, dy: ds.dy() as f64, blur: ds.std_dev_x().get() as f64 * 2.0 });
            }
        }
    }
    None
}

fn blend_of(b: usvg::BlendMode) -> crate::document::BlendMode {
    use crate::document::BlendMode as B;
    match b {
        usvg::BlendMode::Normal | usvg::BlendMode::PlusLighter => B::Normal,
        usvg::BlendMode::Multiply => B::Multiply,
        usvg::BlendMode::Screen => B::Screen,
        usvg::BlendMode::Overlay => B::Overlay,
        usvg::BlendMode::Darken => B::Darken,
        usvg::BlendMode::Lighten => B::Lighten,
        usvg::BlendMode::ColorDodge => B::ColorDodge,
        usvg::BlendMode::ColorBurn => B::ColorBurn,
        usvg::BlendMode::HardLight => B::HardLight,
        usvg::BlendMode::SoftLight => B::SoftLight,
        usvg::BlendMode::Difference => B::Difference,
        usvg::BlendMode::Exclusion => B::Exclusion,
        usvg::BlendMode::Hue => B::Hue,
        usvg::BlendMode::Saturation => B::Saturation,
        usvg::BlendMode::Color => B::Color,
        usvg::BlendMode::Luminosity => B::Luminosity,
    }
}

fn walk(group: &usvg::Group, opacity: f32, doc: &mut Document) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => {
                let before = doc.shapes().len();
                walk(g, opacity * g.opacity().get(), doc);
                if let Some(sh) = group_shadow(g) {
                    let ids: Vec<u64> = doc.shapes()[before..].iter().map(|s| s.id).collect();
                    for id in ids {
                        if let Some(s) = doc.get_mut(id) {
                            s.shadow = Some(sh.clone());
                        }
                    }
                }
                // Blend mode and mask: on the one shape, or a group of them.
                let blend = blend_of(g.blend_mode());
                let mask = g.mask().and_then(|m| {
                    let mut md = Document::default();
                    walk(m.root(), 1.0, &mut md);
                    let mut shapes: Vec<Shape> = md.shapes().iter().map(|s| Shape::clone(s)).collect();
                    let shape = match shapes.len() {
                        0 => return None,
                        1 => shapes.pop().unwrap(),
                        _ => Shape::new(ShapeKind::Group { children: shapes }),
                    };
                    let mode = if m.kind() == usvg::MaskType::Alpha { crate::document::MaskMode::Alpha } else { crate::document::MaskMode::Luminance };
                    Some(Box::new(crate::document::Mask { shape, mode, invert: false }))
                });
                if !blend.is_normal() || mask.is_some() {
                    let ids: Vec<u64> = doc.shapes()[before..].iter().map(|s| s.id).collect();
                    let target = if ids.len() == 1 {
                        ids[0]
                    } else if ids.is_empty() {
                        continue;
                    } else {
                        let children: Vec<Shape> = ids.iter().filter_map(|id| doc.get(*id).cloned()).collect();
                        for id in &ids {
                            doc.remove(*id);
                        }
                        doc.add_shape(Shape::new(ShapeKind::Group { children }))
                    };
                    if let Some(s) = doc.get_mut(target) {
                        s.blend = blend;
                        if mask.is_some() {
                            s.mask = mask;
                        }
                    }
                }
            }
            usvg::Node::Path(p) => {
                if !p.is_visible() {
                    continue;
                }
                let t = affine_of(p.abs_transform());
                let path = t * bez_of(p.data());
                let mut shape = Shape::new(ShapeKind::Path(geom::bez_to_pathdata(&path)));
                if !p.id().is_empty() {
                    shape.name = p.id().to_string();
                }
                shape.opacity = opacity;
                shape.fill = p.fill().map(|f| gradient_of(f.paint(), f.opacity().get(), t, &path).unwrap_or_else(|| Paint::Solid(color_of(f.paint(), f.opacity().get()))));
                if let Some(f) = p.fill() {
                    shape.fill_rule = match f.rule() {
                        usvg::FillRule::EvenOdd => FillRule::EvenOdd,
                        usvg::FillRule::NonZero => FillRule::NonZero,
                    };
                }
                let scale = t.determinant().abs().sqrt();
                shape.stroke = p.stroke().map(|s| {
                    let width = s.width().get() as f64 * scale;
                    let rel = |v: f32| v as f64 * scale / width.max(1e-6);
                    Stroke {
                    color: color_of(s.paint(), s.opacity().get()),
                    width,
                    miter_limit: s.miterlimit().get() as f64,
                    dash: s.dasharray().map(|d| d.iter().map(|v| rel(*v)).collect()).unwrap_or_default(),
                    dash_offset: rel(s.dashoffset()),
                    join: match s.linejoin() {
                        usvg::LineJoin::Round => LineJoin::Round,
                        usvg::LineJoin::Bevel => LineJoin::Bevel,
                        _ => LineJoin::Miter,
                    },
                    cap: match s.linecap() {
                        usvg::LineCap::Round => LineCap::Round,
                        usvg::LineCap::Square => LineCap::Square,
                        usvg::LineCap::Butt => LineCap::Butt,
                    },
                    ..Default::default()
                }});
                doc.add_shape(shape);
            }
            usvg::Node::Image(img) => {
                if !img.is_visible() {
                    continue;
                }
                let data: Option<Arc<Vec<u8>>> = match img.kind() {
                    usvg::ImageKind::PNG(d) | usvg::ImageKind::JPEG(d) | usvg::ImageKind::GIF(d) | usvg::ImageKind::WEBP(d) => {
                        Some(d.clone())
                    }
                    usvg::ImageKind::SVG(tree) => {
                        walk(tree.root(), opacity, doc);
                        None
                    }
                };
                if let Some(d) = data {
                    if let Ok(asset) = ImageAsset::from_encoded(d.as_ref().clone()) {
                        let id = doc.add_image(asset);
                        let size = img.size();
                        let mut shape =
                            Shape::new(ShapeKind::Rect { w: size.width() as f64, h: size.height() as f64, radii: [0.0; 4] });
                        shape.name = "Image".into();
                        shape.transform = affine_of(img.abs_transform());
                        shape.opacity = opacity;
                        shape.fill =
                            Some(Paint::Image(ImageFill { image: id, mode: ImageMode::Stretch, scale: 1.0, opacity: 1.0 }));
                        doc.add_shape(shape);
                    }
                }
            }
            usvg::Node::Text(t) => walk(t.flattened(), opacity, doc),
        }
    }
}

pub fn import(data: &[u8], fonts: &mut FontManager) -> Result<Document, String> {
    let opt = usvg::Options { fontdb: Arc::new(fonts.db.clone()), ..Default::default() };
    let tree = usvg::Tree::from_data(data, &opt).map_err(|e| e.to_string())?;
    let mut doc = Document::default();
    doc.set_size(tree.size().width() as f64, tree.size().height() as f64);
    walk(tree.root(), 1.0, &mut doc);
    Ok(doc)
}

/// Import SVG content into an existing document (paste / place).
pub fn import_into(data: &[u8], fonts: &mut FontManager, target: &mut Document) -> Result<Vec<u64>, String> {
    let src = import(data, fonts)?;
    let mut ids = Vec::new();
    let mut map = std::collections::HashMap::new();
    for (k, img) in src.images.iter() {
        map.insert(*k, target.add_image(img.clone()));
    }
    for s in src.shapes() {
        let mut s = crate::document::Shape::clone(s);
        s.id = 0;
        if let Some(Paint::Image(f)) = &mut s.fill {
            f.image = map.get(&f.image).copied().unwrap_or(f.image);
        }
        ids.push(target.add_shape(s));
    }
    Ok(ids)
}

/// `<linearGradient>` / `<radialGradient>` for a gradient in the shape's
/// bounding-box units: user-space coordinates via a gradientTransform that
/// maps the unit box onto the shape.
pub fn gradient_def(id: &str, g: &crate::document::Gradient, transform: kurbo::Affine, lb: kurbo::Rect) -> String {
    let m = transform * kurbo::Affine::new([lb.width().max(1e-9), 0.0, 0.0, lb.height().max(1e-9), lb.x0, lb.y0]);
    let mut stops = String::new();
    for st in &g.linear_stops() {
        let op = if st.color.a < 1.0 { format!(r#" stop-opacity="{}""#, num(st.color.a as f64)) } else { String::new() };
        write!(stops, r#"<stop offset="{}" stop-color="{}"{op}/>"#, num(st.offset as f64), st.color.to_hex()).unwrap();
    }
    let [sx, sy] = g.start;
    let [ex, ey] = g.end;
    match g.kind {
        crate::document::GradientKind::Conic | crate::document::GradientKind::Mesh => {
            // No SVG equivalent: the gradient as an image over the unit box.
            let img = render::bake_gradient(g, 256);
            let mut png = Vec::new();
            let _ = image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
            format!(
                r#"    <pattern id="{id}" patternUnits="userSpaceOnUse" patternTransform="matrix({})" width="1" height="1"><image width="1" height="1" preserveAspectRatio="none" xlink:href="data:image/png;base64,{b64}"/></pattern>"#,
                coeffs(m)
            )
        }
        crate::document::GradientKind::Linear => format!(
            r#"    <linearGradient id="{id}" gradientUnits="userSpaceOnUse" gradientTransform="matrix({})" x1="{}" y1="{}" x2="{}" y2="{}">{stops}</linearGradient>"#,
            coeffs(m),
            num(sx),
            num(sy),
            num(ex),
            num(ey)
        ),
        crate::document::GradientKind::Radial => format!(
            r#"    <radialGradient id="{id}" gradientUnits="userSpaceOnUse" gradientTransform="matrix({})" cx="{}" cy="{}" r="{}">{stops}</radialGradient>"#,
            coeffs(m),
            num(sx),
            num(sy),
            num(((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt())
        ),
    }
}
