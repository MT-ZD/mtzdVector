//! Canvas rendering through SVG + resvg, for what the mesh renderer can't
//! draw: blend modes (which need the artwork below), masks and pattern
//! fills. The shape (with its backdrop for blend modes) is exported to SVG
//! over the visible part of its bounds, rendered at screen resolution and
//! drawn as a texture, cached until anything in it changes.

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

use egui::{Color32, Mesh, Pos2, TextureHandle};
use kurbo::Rect;

use super::{GeoCache, View, effect_reach, shadow_reach};
use crate::document::{Artboard, Document, Paint, Shape, ShapeKind};
use crate::fonts::FontManager;

/// Largest texture side.
const MAX_PX: f64 = 2048.0;

pub struct SvgTex {
    key: u64,
    rect: Rect,
    tex: Option<TextureHandle>,
}

/// Whether the mesh renderer hands `shape` to the SVG renderer.
pub fn needs_svg(shape: &Shape) -> bool {
    !shape.blend.is_normal() || shape.mask.is_some()
}

/// The shapes painted up to and including `target`, in paint order: earlier
/// shapes whole, the groups containing `target` cut after it.
fn prefix_of<S: std::borrow::Borrow<Shape>>(list: &[S], target: u64, out: &mut Vec<Shape>) -> bool {
    for s in list {
        let s = s.borrow();
        if s.id == target {
            out.push(s.clone());
            return true;
        }
        if let ShapeKind::Group { children } = &s.kind {
            if contains(children, target) {
                let mut inner = Vec::new();
                prefix_of(children, target, &mut inner);
                let mut g = s.clone();
                g.kind = ShapeKind::Group { children: inner };
                out.push(g);
                return true;
            }
        }
        out.push(s.clone());
    }
    false
}

fn contains(list: &[Shape], id: u64) -> bool {
    list.iter().any(|s| s.id == id || s.kind.children().is_some_and(|c| contains(c, id)))
}

/// Revision hash of what's painted before `target` (for the cache key).
fn prefix_key<S: std::borrow::Borrow<Shape>>(list: &[S], target: u64, h: &mut impl Hasher) -> bool {
    for s in list {
        let s = s.borrow();
        if s.id == target {
            return true;
        }
        (s.id, s.rev, s.visible).hash(h);
        if let ShapeKind::Group { children } = &s.kind {
            if prefix_key(children, target, h) {
                return true;
            }
        }
    }
    false
}

/// Images used by `shapes` (their fills, recursively).
fn used_images(shapes: &[Shape], out: &mut BTreeSet<u64>) {
    for s in shapes {
        for p in s.fill.iter().chain(s.fills.iter()) {
            if let Paint::Image(f) = p {
                out.insert(f.image);
            }
        }
        if let Some(c) = s.kind.children() {
            used_images(c, out);
        }
        if let Some(m) = &s.mask {
            used_images(std::slice::from_ref(&m.shape), out);
        }
    }
}

/// A document holding `shapes` with one artboard over `region`.
pub fn mini_document(doc: &Document, shapes: Vec<Shape>, region: Rect) -> Document {
    let mut d = Document::default();
    let mut ids = BTreeSet::new();
    used_images(&shapes, &mut ids);
    d.images = ids.into_iter().filter_map(|i| doc.images.get(&i).map(|a| (i, a.clone()))).collect();
    d.dpi = doc.dpi;
    d.artboards = vec![Artboard { id: 1, name: String::new(), x: region.x0, y: region.y0, w: region.width().max(1e-3), h: region.height().max(1e-3), background: crate::document::Rgba::WHITE.with_alpha(0.0), clip: false, exports: Vec::new() }];
    for s in shapes {
        d.add_shape(s);
    }
    d
}

/// Render `d` (first artboard) at `scale` into premultiplied RGBA.
pub fn render_document(d: &Document, fonts: &mut FontManager, w: u32, h: u32) -> Option<Vec<u8>> {
    let d = d.at_origin();
    let items = crate::io::prepare(&d, fonts);
    let svg = crate::io::svg::export_items(&d, &items);
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    let sx = w as f32 / tree.size().width();
    let sy = h as f32 / tree.size().height();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
    Some(pixmap.take())
}

impl GeoCache {
    /// Paint `shape` through SVG. With a blend mode, the texture holds what's
    /// below it too (the backdrop it blends with), so it replaces that area.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_via_svg(&mut self, painter: &egui::Painter, shape: &Shape, doc: &Document, fonts: &mut FontManager, view: &View, alpha: f32) {
        let Some(b) = self.update(shape, fonts).bounds else { return };
        let pad = effect_reach(shape) + shadow_reach(shape) + shape.blur * 1.5 + 2.0;
        let b = b.inflate(pad, pad);
        // The visible part (plus a margin, so small pans reuse the texture).
        let clip = painter.clip_rect();
        let (v0, v1) = (view.to_doc(clip.min), view.to_doc(clip.max));
        let vis = Rect::from_points(v0, v1);
        let vis = vis.inflate(vis.width() * 0.25, vis.height() * 0.25);
        let region = b.intersect(vis);
        if region.width() <= 0.0 || region.height() <= 0.0 {
            return;
        }
        let ppp = painter.ctx().pixels_per_point() as f64;
        let mut scale = 2f64.powi(super::zoom_bucket(view.zoom)) * ppp;
        // Snap the region to a grid in texture pixels.
        let step = 64.0 / scale;
        let region = Rect::new((region.x0 / step).floor() * step, (region.y0 / step).floor() * step, (region.x1 / step).ceil() * step, (region.y1 / step).ceil() * step).intersect(b);
        let longest = region.width().max(region.height()) * scale;
        if longest > MAX_PX {
            scale *= MAX_PX / longest;
        }
        let (w, h) = (((region.width() * scale).ceil() as u32).max(1), ((region.height() * scale).ceil() as u32).max(1));
        let backdrop = !shape.blend.is_normal();
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        (shape.id, shape.rev, w, h, backdrop).hash(&mut hs);
        for v in [region.x0, region.y0, region.x1, region.y1, scale] {
            v.to_bits().hash(&mut hs);
        }
        if backdrop {
            prefix_key(doc.shapes(), shape.id, &mut hs);
            for a in &doc.artboards {
                (a.x.to_bits(), a.y.to_bits(), a.w.to_bits(), a.h.to_bits(), a.background.to_hex()).hash(&mut hs);
            }
        }
        let key = hs.finish();
        let fresh = self.svg_tex.get(&shape.id).is_some_and(|t| t.key == key);
        if !fresh {
            let mut shapes = Vec::new();
            if backdrop {
                // Artboard backgrounds under the region, then everything below.
                for a in &doc.artboards {
                    if a.background.a > 0.0 && a.rect().intersect(region).area() > 0.0 {
                        let mut r = Shape::new(ShapeKind::Rect { w: a.w, h: a.h, radii: [0.0; 4] });
                        r.transform = kurbo::Affine::translate((a.x, a.y));
                        r.fill = Some(Paint::Solid(a.background));
                        shapes.push(r);
                    }
                }
                let mut prefix = Vec::new();
                prefix_of(doc.shapes(), shape.id, &mut prefix);
                // Only what can show in the region.
                let near = |s: &Shape| match self.map.get(&s.id).and_then(|c| c.bounds) {
                    Some(sb) => {
                        let r = effect_reach(s) + shadow_reach(s) + s.blur * 1.5 + 2.0;
                        sb.inflate(r, r).intersect(region).area() > 0.0
                    }
                    None => true,
                };
                let last = prefix.len().saturating_sub(1);
                shapes.extend(prefix.into_iter().enumerate().filter(|(i, s)| *i == last || near(s)).map(|(_, s)| s));
            } else {
                let mut s = shape.clone();
                s.visible = true;
                shapes.push(s);
            }
            let d = mini_document(doc, shapes, region);
            let tex = render_document(&d, fonts, w, h).map(|px| {
                let pixels = px.chunks_exact(4).map(|p| Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3])).collect();
                let img = egui::ColorImage { size: [w as usize, h as usize], source_size: egui::vec2(w as f32, h as f32), pixels };
                painter.ctx().load_texture(format!("svg-{}", shape.id), img, egui::TextureOptions::LINEAR)
            });
            self.svg_tex.insert(shape.id, SvgTex { key, rect: region, tex });
        }
        let Some(t) = self.svg_tex.get(&shape.id) else { return };
        let Some(tex) = &t.tex else { return };
        let mut mesh = Mesh::with_texture(tex.id());
        let a = if backdrop { 1.0 } else { alpha };
        mesh.add_rect_with_uv(view.rect_to_screen(t.rect), egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE.gamma_multiply(a));
        painter.add(mesh);
    }
}

/// Straight-alpha raster of one shape (with its mask and effects)
/// through SVG: for exporters (PDF) when their own drawing can't express it.
pub fn rasterize_shape_svg(shape: &Shape, doc: &Document, fonts: &mut FontManager, scale: f64, max_px: f64) -> Option<super::Raster> {
    let (path, text) = super::compute_outline(shape, fonts);
    let b = match text {
        Some(t) => shape.transform.transform_rect_bbox(t.bounds),
        None => crate::geom::path_bounds(&path)?,
    };
    let b = match &shape.mask {
        Some(m) => crate::geom::path_bounds(&super::compute_outline(&m.shape, fonts).0).map(|mb| b.intersect(mb.inflate(1.0, 1.0))).unwrap_or(b),
        None => b,
    };
    let pad = effect_reach(shape) + shadow_reach(shape) + shape.blur * 1.5 + 2.0;
    let rect = b.inflate(pad, pad);
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let s = scale.min(max_px / rect.width().max(rect.height()).max(1e-6));
    let (w, h) = (((rect.width() * s).ceil() as usize).max(1), ((rect.height() * s).ceil() as usize).max(1));
    let mut one = shape.clone();
    one.blend = crate::document::BlendMode::Normal;
    let d = mini_document(doc, vec![one], rect);
    let px = render_document(&d, fonts, w as u32, h as u32)?;
    let mut rgba = Vec::with_capacity(px.len());
    for p in px.chunks_exact(4) {
        let a = p[3] as f32 / 255.0;
        let un = |v: u8| if a > 0.0 { ((v as f32 / 255.0 / a).clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 };
        rgba.extend([un(p[0]), un(p[1]), un(p[2]), p[3]]);
    }
    Some(super::Raster { rect, w, h, rgba })
}
