//! Rendering: shapes are tessellated with lyon into triangle meshes that are
//! cached per shape revision and zoom bucket, then submitted to the GPU via
//! egui's wgpu renderer every frame (only a cheap affine pass on the CPU).

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, Mesh, Pos2, TextureHandle, TextureId, epaint::Vertex};
use kurbo::{Affine, BezPath, PathEl, Point, Rect};
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, LineCap, LineJoin, StrokeOptions,
    StrokeTessellator, StrokeVertex, VertexBuffers, math::point, path::Path as LPath,
};

use crate::document::{self, Document, FillRule, ImageMode, Paint, Shape, ShapeKind};
use crate::fonts::{FontManager, TextLayout};
use crate::geom;

#[derive(Clone, Copy, Debug)]
pub struct View {
    pub zoom: f32,
    /// Screen position of the document origin.
    pub origin: egui::Vec2,
}

impl View {
    pub fn to_screen(&self, p: Point) -> Pos2 {
        Pos2::new(p.x as f32 * self.zoom + self.origin.x, p.y as f32 * self.zoom + self.origin.y)
    }
    pub fn to_doc(&self, p: Pos2) -> Point {
        Point::new(((p.x - self.origin.x) / self.zoom) as f64, ((p.y - self.origin.y) / self.zoom) as f64)
    }
    pub fn rect_to_screen(&self, r: Rect) -> egui::Rect {
        egui::Rect::from_two_pos(self.to_screen(Point::new(r.x0, r.y0)), self.to_screen(Point::new(r.x1, r.y1)))
    }
    pub fn affine(&self) -> Affine {
        Affine::new([self.zoom as f64, 0.0, 0.0, self.zoom as f64, self.origin.x as f64, self.origin.y as f64])
    }
}

pub fn to_lyon(path: &BezPath) -> LPath {
    let mut b = LPath::builder();
    let mut open = false;
    let p = |q: Point| point(q.x as f32, q.y as f32);
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(a) => {
                if open {
                    b.end(false);
                }
                b.begin(p(a));
                open = true;
            }
            PathEl::LineTo(a) => {
                if open {
                    b.line_to(p(a));
                }
            }
            PathEl::QuadTo(a, c) => {
                if open {
                    b.quadratic_bezier_to(p(a), p(c));
                }
            }
            PathEl::CurveTo(a, c, d) => {
                if open {
                    b.cubic_bezier_to(p(a), p(c), p(d));
                }
            }
            PathEl::ClosePath => {
                if open {
                    b.end(true);
                    open = false;
                }
            }
        }
    }
    if open {
        b.end(false);
    }
    b.build()
}

pub fn tessellate_fill_raw(path: &BezPath, tol: f64, rule: FillRule) -> (Vec<[f32; 2]>, Vec<u32>) {
    let lp = to_lyon(path);
    let mut geo: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let opts = FillOptions::tolerance(tol as f32).with_fill_rule(match rule {
        FillRule::NonZero => lyon_tessellation::FillRule::NonZero,
        FillRule::EvenOdd => lyon_tessellation::FillRule::EvenOdd,
    });
    let _ = FillTessellator::new().tessellate_path(
        &lp,
        &opts,
        &mut BuffersBuilder::new(&mut geo, |v: FillVertex| v.position().to_array()),
    );
    (geo.vertices, geo.indices)
}

pub fn tessellate_stroke_raw(path: &BezPath, tol: f64, s: &document::Stroke) -> (Vec<[f32; 2]>, Vec<u32>) {
    let lp = to_lyon(path);
    let mut geo: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let opts = StrokeOptions::tolerance(tol as f32)
        .with_line_width(s.width as f32)
        .with_miter_limit(s.miter_limit.max(1.0) as f32)
        .with_line_join(match s.join {
            document::LineJoin::Miter => LineJoin::Miter,
            document::LineJoin::Round => LineJoin::Round,
            document::LineJoin::Bevel => LineJoin::Bevel,
        })
        .with_line_cap(match s.cap {
            document::LineCap::Butt => LineCap::Butt,
            document::LineCap::Round => LineCap::Round,
            document::LineCap::Square => LineCap::Square,
        });
    let _ = StrokeTessellator::new().tessellate_path(
        &lp,
        &opts,
        &mut BuffersBuilder::new(&mut geo, |v: StrokeVertex| v.position().to_array()),
    );
    (geo.vertices, geo.indices)
}

pub struct MeshData {
    pub verts: Vec<[f32; 2]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub texture: Option<TexKey>,
    pub color: Color32,
    /// Gradient fills: per-pixel colour for the CPU rasterizer.
    pub grad: Option<Arc<GradSampler>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TexKey {
    /// (image id, tiled)
    Image(u64, bool),
    /// A gradient's colour ramp (see `Gradient::ramp_key`).
    Gradient(u64, document::GradientKind),
}

/// Evaluates a gradient fill at document points.
pub struct GradSampler {
    pub grad: document::Gradient,
    /// Document space → bounding-box units.
    pub to_unit: Affine,
    pub opacity: f32,
    /// Mesh gradients: the mesh raster (sampled per pixel).
    pub mesh: Option<Arc<Vec<document::Rgba>>>,
}

impl GradSampler {
    pub fn new(shape: &Shape, lb: Rect, grad: &document::Gradient, opacity: f32) -> Self {
        let (bw, bh) = (lb.width().max(1e-9), lb.height().max(1e-9));
        let unit = Affine::new([1.0 / bw, 0.0, 0.0, 1.0 / bh, -lb.x0 / bw, -lb.y0 / bh]);
        let mesh = grad.mesh.as_ref().filter(|_| grad.kind == document::GradientKind::Mesh).map(|m| m.raster(document::MeshGrid::RASTER));
        GradSampler { grad: grad.clone(), to_unit: unit * shape.transform.inverse(), opacity, mesh }
    }

    pub fn unit(&self, p: Point) -> [f64; 2] {
        let u = self.to_unit * p;
        [u.x, u.y]
    }

    /// Premultiplied colour at a document point.
    pub fn premul_at(&self, p: Point) -> [f32; 4] {
        let c = match &self.mesh {
            Some(img) => document::MeshGrid::sample(img, document::MeshGrid::RASTER, self.unit(p)),
            None => self.grad.color_at_unit(self.unit(p)),
        };
        let a = c.a * self.opacity;
        [c.r * a, c.g * a, c.b * a, a]
    }

    /// Texture coordinates for the ramp texture.
    pub fn uv(&self, p: Point) -> [f32; 2] {
        let g = &self.grad;
        let u = self.unit(p);
        match g.kind {
            document::GradientKind::Linear => {
                let t = g.t_at(u);
                [(t * (RAMP - 1) as f32 + 0.5) / RAMP as f32, 0.5]
            }
            document::GradientKind::Radial => {
                let r = ((g.end[0] - g.start[0]).powi(2) + (g.end[1] - g.start[1]).powi(2)).sqrt().max(1e-9);
                [(0.5 + 0.5 * (u[0] - g.start[0]) / r) as f32, (0.5 + 0.5 * (u[1] - g.start[1]) / r) as f32]
            }
            // Baked over the bounding box.
            document::GradientKind::Conic | document::GradientKind::Mesh => [u[0] as f32, u[1] as f32],
        }
    }
}

/// A conic or mesh gradient as an `n` × `n` image of the unit box
/// (straight alpha).
pub fn bake_gradient(g: &document::Gradient, n: usize) -> image::RgbaImage {
    let n = n.max(2);
    if let (document::GradientKind::Mesh, Some(m)) = (g.kind, &g.mesh) {
        let img = m.raster(n);
        return image::RgbaImage::from_fn(n as u32, n as u32, |x, y| {
            let c = img[y as usize * n + x as usize];
            image::Rgba([c.r, c.g, c.b, c.a].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
        });
    }
    image::RgbaImage::from_fn(n as u32, n as u32, |x, y| {
        let c = g.color_at_unit([(x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64]);
        image::Rgba([c.r, c.g, c.b, c.a].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
    })
}

/// Ramp texture size (linear: RAMP × 1, radial: RAMP × RAMP covering t ∈ [-1, 1]²).
const RAMP: usize = 256;

/// Fill of a rasterized layer: flat premultiplied colour or a gradient.
#[derive(Clone)]
enum Shade {
    Flat([f32; 4]),
    Grad(Arc<GradSampler>),
}

pub struct CachedShape {
    pub rev: u64,
    font_gen: u64,
    pub doc_path: Arc<BezPath>,
    pub bounds: Option<Rect>,
    pub text: Option<Arc<TextLayout>>,
    bucket: i32,
    pub fill: Option<MeshData>,
    pub stroke: Option<MeshData>,
    /// Stroke geometry (dashes, alignment, arrowheads), per revision.
    pub stroke_parts: Option<Arc<Vec<crate::strokes::StrokePart>>>,
    /// Visible fill geometry (the outline, intersected with the clip).
    pub fill_path: Arc<BezPath>,
    pub fill_rule: FillRule,
    /// Clip mask polygons in document space.
    pub clip: Option<Arc<crate::boolean::Polys>>,
    /// Outline-effect rings, outermost first (already clipped).
    pub rings: Arc<Vec<(BezPath, Paint)>>,
    /// Outermost extent of the outline effect (for hit testing).
    pub ring_area: Option<Arc<crate::boolean::Polys>>,
    pub ring_meshes: Vec<MeshData>,
    /// Text with per-range colours: those glyphs (document space, clipped);
    /// `fill_path` then holds only the glyphs in the shape's own fill.
    pub text_colors: Arc<Vec<(BezPath, document::Rgba)>>,
    /// Meshes painted over the fill: coloured text ranges.
    pub extra_meshes: Vec<MeshData>,
    /// Extra fills (over the fill) and strokes (under the stroke).
    pub fill_extra: Vec<MeshData>,
    pub stroke_extra: Vec<MeshData>,
}

impl CachedShape {
    /// Whether `p` is inside the clip mask (always true without a clip).
    /// Every mesh in paint order: rings, fills, coloured text, strokes.
    pub fn meshes(&self) -> impl Iterator<Item = &MeshData> {
        self.ring_meshes.iter().chain(self.fill.iter()).chain(self.fill_extra.iter()).chain(self.extra_meshes.iter()).chain(self.stroke_extra.iter()).chain(self.stroke.iter())
    }

    pub fn clip_contains(&self, p: Point) -> bool {
        self.clip.as_ref().map(|c| geom::point_in_polys(c, p)).unwrap_or(true)
    }

    pub fn ring_contains(&self, p: Point) -> bool {
        self.ring_area.as_ref().map(|c| geom::point_in_polys(c, p)).unwrap_or(false)
    }
}

/// Geometry derived from a shape's effects: clipped fill and stroke, outline rings.
struct Effects {
    fill_path: BezPath,
    fill_rule: FillRule,
    stroke_parts: Option<Vec<crate::strokes::StrokePart>>,
    clip: Option<crate::boolean::Polys>,
    rings: Vec<(BezPath, Paint)>,
    ring_area: Option<crate::boolean::Polys>,
}

/// Outline-effect rings of a shape (unclipped), outermost first.
pub fn outline_rings(shape: &Shape, path: &BezPath, fonts: &mut FontManager) -> Vec<(BezPath, Paint)> {
    rings_of(shape, path, None, fonts).0
}

/// What a group visibly covers: its descendants' fills and strokes.
pub fn group_area(shape: &Shape, fonts: &mut FontManager) -> crate::boolean::Polys {
    use crate::boolean::{self, BoolOp};
    let mut acc: crate::boolean::Polys = Vec::new();
    let add = |acc: &mut crate::boolean::Polys, p: crate::boolean::Polys| {
        *acc = if acc.is_empty() { p } else { boolean::op(acc, &p, BoolOp::Union) };
    };
    let Some(children) = shape.kind.children() else { return acc };
    for c in children.iter().filter(|c| c.visible) {
        if matches!(c.kind, ShapeKind::Group { .. }) {
            let a = group_area(c, fonts);
            add(&mut acc, a);
            continue;
        }
        let (path, _) = compute_outline(c, fonts);
        if c.fill.is_some() {
            add(&mut acc, boolean::path_to_polys(&path, c.fill_rule));
        }
        if let Some(st) = c.stroke.as_ref().filter(|s| s.width > 0.0) {
            add(&mut acc, crate::strokes::stroke_area(&path, st, c.fill_rule));
        }
    }
    acc
}

/// A group carries effects of its own (drawn around / over its children).
pub fn group_fx(shape: &Shape) -> bool {
    matches!(shape.kind, ShapeKind::Group { .. }) && (shape.shadow.is_some() || shape.inner_shadow.is_some() || shape.blur > 0.0 || shape.noise > 0.0 || !shape.outlines.is_empty() || !shape.blend.is_normal() || shape.mask.is_some())
}

/// A ring's bounding box in the shape's local space (the unit box of a
/// gradient ring).
pub fn ring_bounds(shape: &Shape, ring: &BezPath) -> Rect {
    geom::path_bounds(&(shape.transform.inverse() * ring.clone())).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0))
}

fn rings_of(shape: &Shape, path: &BezPath, clip: Option<&crate::boolean::Polys>, fonts: &mut FontManager) -> (Vec<(BezPath, Paint)>, Option<crate::boolean::Polys>) {
    use crate::boolean::{self, BoolOp};
    let mut rings = Vec::new();
    if shape.outlines.is_empty() {
        return (rings, None);
    }
    let stroke = shape.stroke.as_ref().filter(|s| s.width > 0.0);
    let (base, path, sr) = if matches!(shape.kind, ShapeKind::Group { .. }) {
        // Around everything the group shows (strokes included).
        let area = group_area(shape, fonts);
        let p = geom::polys_to_bez(&area);
        (area, p, 0.0)
    } else {
        let mut base = boolean::path_to_polys(path, shape.fill_rule);
        if let Some(st) = stroke {
            base = boolean::op(&base, &crate::strokes::stroke_area(path, st, shape.fill_rule), BoolOp::Union);
        }
        (base, path.clone(), stroke.map(|s| s.reach()).unwrap_or(0.0))
    };
    let path = &path;
    let round = document::Stroke { join: document::LineJoin::Round, cap: document::LineCap::Round, ..Default::default() };
    let mut inner = base.clone();
    let mut acc = 0.0;
    for step in &shape.outlines {
        acc += step.width.max(0.0);
        let grown = crate::strokes::outline(path, &round, 2.0 * (sr + acc));
        let outer = boolean::op(&base, &boolean::path_to_polys(&grown, FillRule::NonZero), BoolOp::Union);
        let mut ring = boolean::op(&outer, &inner, BoolOp::Subtract);
        if let Some(c) = clip {
            ring = boolean::op(&ring, c, BoolOp::Intersect);
        }
        rings.push((geom::polys_to_bez(&ring), step.paint()));
        inner = outer;
    }
    rings.reverse();
    let area = match clip {
        Some(c) => boolean::op(&inner, c, BoolOp::Intersect),
        None => inner,
    };
    (rings, Some(area))
}

/// Text with per-range colours: the fill keeps the glyphs in the shape's own
/// fill; the others come back by colour (document space, clipped).
fn split_text_colors(shape: &Shape, t: &TextLayout, fx: &mut Effects) -> Vec<(BezPath, document::Rgba)> {
    if t.colored.is_empty() {
        return Vec::new();
    }
    let clipped = |p: BezPath| match &fx.clip {
        Some(c) => geom::polys_to_bez(&crate::boolean::op(&crate::boolean::path_to_polys(&p, FillRule::NonZero), c, crate::boolean::BoolOp::Intersect)),
        None => p,
    };
    fx.fill_path = clipped(shape.transform * t.path.clone());
    fx.fill_rule = FillRule::NonZero;
    t.colored.iter().map(|(p, c)| (clipped(shape.transform * p.clone()), *c)).collect()
}

fn effects(shape: &Shape, path: &BezPath, fonts: &mut FontManager) -> Effects {
    use crate::boolean::{self, BoolOp};
    use crate::strokes::StrokePart;
    let stroke = shape.stroke.as_ref().filter(|s| s.width > 0.0);
    let mut stroke_parts = stroke.map(|s| crate::strokes::stroke_parts(path, s, shape.fill_rule));
    let clip = shape.clip.as_ref().map(|c| {
        let (cp, _) = compute_outline(c, fonts);
        boolean::path_to_polys(&cp, c.fill_rule)
    });
    let (rings, ring_area) = rings_of(shape, path, clip.as_ref(), fonts);
    let (fill_path, fill_rule) = match &clip {
        Some(c) => {
            let f = boolean::op(&boolean::path_to_polys(path, shape.fill_rule), c, BoolOp::Intersect);
            if let (Some(st), Some(parts)) = (stroke, &mut stroke_parts) {
                let area = boolean::op(&crate::strokes::stroke_area(path, st, shape.fill_rule), c, BoolOp::Intersect);
                *parts = vec![StrokePart::Fill(geom::polys_to_bez(&area), FillRule::EvenOdd)];
            }
            (geom::polys_to_bez(&f), FillRule::EvenOdd)
        }
        None => (path.clone(), shape.fill_rule),
    };
    Effects { fill_path, fill_rule, stroke_parts, clip, rings, ring_area }
}

#[derive(Default)]
pub struct GeoCache {
    map: HashMap<u64, CachedShape>,
    textures: HashMap<TexKey, TextureHandle>,
    shadows: HashMap<u64, ShadowTex>,
    blurs: HashMap<u64, ShadowTex>,
    inner: HashMap<u64, InnerTex>,
    noise: Option<TextureHandle>,
    /// Document state `prune` last ran for.
    pruned: Option<(u64, usize, usize)>,
    /// Top-level shapes' hit areas (bounds grown by their effects) for a
    /// document revision: hit-testing scans this flat list first.
    hit_boxes: Option<((u64, u64, usize), std::sync::Arc<Vec<Rect>>)>,
    /// Textures of shapes drawn through SVG (blend modes, masks).
    svg_tex: HashMap<u64, svg_paint::SvgTex>,
}

#[path = "render_svg.rs"]
pub mod svg_paint;

const NOISE_TILE: usize = 128;

/// Triangles with their fill.
type Layer<'a> = (&'a [[f32; 2]], &'a [u32], Shade);

fn mesh_shade(m: &MeshData) -> Shade {
    if let Some(g) = &m.grad {
        return Shade::Grad(g.clone());
    }
    // Image fills are approximated by a neutral grey in rasterized effects.
    let col = if m.texture.is_some() { Color32::from_gray(160).gamma_multiply(m.color.a() as f32 / 255.0) } else { m.color };
    Shade::Flat([col.r(), col.g(), col.b(), col.a()].map(|v| v as f32 / 255.0))
}

/// Paint layers bottom-up into premultiplied RGBA planes.
fn composite(layers: &[Layer], rect: Rect, scale: f64, w: usize, h: usize) -> [Vec<f32>; 4] {
    let mut planes = [vec![0.0f32; w * h], vec![0.0f32; w * h], vec![0.0f32; w * h], vec![0.0f32; w * h]];
    let mut mask = vec![0.0f32; w * h];
    for (verts, indices, shade) in layers {
        mask.iter_mut().for_each(|v| *v = 0.0);
        raster_triangles(&mut mask, w, h, rect, scale, verts, indices);
        for (i, k) in mask.iter().enumerate() {
            if *k > 0.0 {
                let rgba = match shade {
                    Shade::Flat(c) => *c,
                    Shade::Grad(g) => g.premul_at(Point::new(rect.x0 + ((i % w) as f64 + 0.5) / scale, rect.y0 + ((i / w) as f64 + 0.5) / scale)),
                };
                let inv = 1.0 - rgba[3];
                for ch in 0..4 {
                    planes[ch][i] = rgba[ch] + planes[ch][i] * inv;
                }
            }
        }
    }
    planes
}

/// Grain value for a pixel: (strength 0..1, lighten?) — same distribution as
/// the on-screen noise texture.
fn grain(x: usize, y: usize) -> (f32, bool) {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ 0xC2B2_AE3D;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    let v = (h >> 24) as i32;
    (((v - 128).unsigned_abs() * 2).min(255) as f32 / 255.0, v >= 128)
}

/// A rasterized shape: straight-alpha RGBA8 covering `rect` (document space).
pub struct Raster {
    pub rect: Rect,
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

/// Pixel scale for `rect` at `scale` px per document unit, capped in size.
fn raster_dims(rect: Rect, scale: f64, max_px: f64) -> (f64, usize, usize) {
    let scale = scale.min(max_px / rect.width().max(rect.height()).max(1e-6));
    (scale, ((rect.width() * scale).ceil() as usize).max(1), ((rect.height() * scale).ceil() as usize).max(1))
}

fn to_raster(planes: &[Vec<f32>; 4], rect: Rect, w: usize, h: usize) -> Raster {
    let mut rgba = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        let a = planes[3][i].clamp(0.0, 1.0);
        let un = |v: f32| if a > 1e-6 { (v / a).clamp(0.0, 1.0) } else { 0.0 };
        rgba.extend([un(planes[0][i]), un(planes[1][i]), un(planes[2][i]), a].map(|v| (v * 255.0).round() as u8));
    }
    Raster { rect, w, h, rgba }
}

/// Tessellated layers of a shape (rings, fill, stroke), with clip applied.
/// Fill of an outline ring for the CPU rasterizer.
fn ring_shade(shape: &Shape, ring: &BezPath, paint: &Paint, opacity: f32) -> Option<Shade> {
    match paint {
        Paint::Gradient(g) => Some(Shade::Grad(Arc::new(GradSampler::new(shape, ring_bounds(shape, ring), g, opacity)))),
        Paint::Solid(c) => {
            let a = c.a * opacity;
            Some(Shade::Flat([c.r * a, c.g * a, c.b * a, a]))
        }
        Paint::Image(_) => None,
    }
}

/// A layer drawn `k` times as opaque (group opacity).
fn scale_shade(sh: Shade, k: f32) -> Shade {
    match sh {
        Shade::Flat(c) => Shade::Flat(c.map(|v| v * k)),
        Shade::Grad(g) => Shade::Grad(Arc::new(GradSampler { grad: g.grad.clone(), to_unit: g.to_unit, opacity: g.opacity * k, mesh: g.mesh.clone() })),
    }
}

fn shape_layers(shape: &Shape, fonts: &mut FontManager, tol: f64) -> (Vec<(Vec<[f32; 2]>, Vec<u32>, Shade)>, Option<Rect>, usize) {
    if let ShapeKind::Group { children } = &shape.kind {
        // The group's rings, then its children (with the group's opacity).
        let (path, _) = compute_outline(shape, fonts);
        let (rings, _) = rings_of(shape, &path, None, fonts);
        let mut out = Vec::new();
        let mut bounds = None;
        for (p, paint) in &rings {
            if let Some(sh) = ring_shade(shape, p, paint, shape.opacity) {
                let (v, i) = tessellate_fill_raw(p, tol, FillRule::EvenOdd);
                out.push((v, i, sh));
            }
            bounds = geom::union_rect(bounds, geom::path_bounds(p).unwrap_or_default());
        }
        let n = out.len();
        for c in children.iter().filter(|c| c.visible) {
            let (layers, b, _) = shape_layers(c, fonts, tol);
            out.extend(layers.into_iter().map(|(v, i, sh)| (v, i, scale_shade(sh, shape.opacity))));
            if let Some(b) = b {
                bounds = geom::union_rect(bounds, b);
            }
        }
        return (out, bounds, n);
    }
    let (path, text) = compute_outline(shape, fonts);
    let mut fx = effects(shape, &path, fonts);
    let text_colors = text.as_ref().map(|t| split_text_colors(shape, t, &mut fx)).unwrap_or_default();
    let mut out = Vec::new();
    let pm = |c: document::Rgba, o: f32| {
        let a = c.a * o;
        [c.r * a, c.g * a, c.b * a, a]
    };
    for (p, paint) in &fx.rings {
        let (v, i) = tessellate_fill_raw(p, tol, FillRule::EvenOdd);
        let shade = match paint {
            Paint::Gradient(g) => Shade::Grad(Arc::new(GradSampler::new(shape, ring_bounds(shape, p), g, shape.opacity))),
            Paint::Solid(c) => Shade::Flat(pm(*c, shape.opacity)),
            Paint::Image(_) => continue,
        };
        out.push((v, i, shade));
    }
    let rings = out.len();
    for paint in shape.fill.iter().chain(shape.fills.iter()) {
        let (v, i) = tessellate_fill_raw(&fx.fill_path, tol, fx.fill_rule);
        let col = match paint {
            Paint::Solid(c) => Shade::Flat(pm(*c, shape.opacity)),
            Paint::Image(img) => Shade::Flat(pm(document::Rgba::rgb(0.63, 0.63, 0.63), img.opacity * shape.opacity)),
            Paint::Gradient(g) => {
                let lb = shape_local_bounds(shape, &path);
                Shade::Grad(Arc::new(GradSampler::new(shape, lb, g, shape.opacity)))
            }
        };
        out.push((v, i, col));
    }
    for (p, c) in &text_colors {
        let (v, i) = tessellate_fill_raw(p, tol, FillRule::NonZero);
        out.push((v, i, Shade::Flat(pm(*c, shape.opacity))));
    }
    for st in &shape.strokes {
        for part in crate::strokes::stroke_parts(&path, st, shape.fill_rule) {
            let (v, i) = match &part {
                crate::strokes::StrokePart::Line(p) => tessellate_stroke_raw(p, tol, st),
                crate::strokes::StrokePart::Fill(p, rule) => tessellate_fill_raw(p, tol, *rule),
            };
            out.push((v, i, Shade::Flat(pm(st.color, shape.opacity))));
        }
    }
    if let (Some(st), Some(parts)) = (&shape.stroke, &fx.stroke_parts) {
        for part in parts {
            let (v, i) = match part {
                crate::strokes::StrokePart::Line(p) => tessellate_stroke_raw(p, tol, st),
                crate::strokes::StrokePart::Fill(p, rule) => tessellate_fill_raw(p, tol, *rule),
            };
            out.push((v, i, Shade::Flat(pm(st.color, shape.opacity))));
        }
    }
    let mut bounds = match &text {
        Some(t) => Some(shape.transform.transform_rect_bbox(t.bounds)),
        None => geom::path_bounds(&path),
    };
    if let (Some(b), Some(c)) = (bounds, fx.clip.as_ref().and_then(|c| crate::boolean::bounds(c))) {
        bounds = Some(b.intersect(c));
    }
    (out, bounds, rings)
}

/// How strongly an inner shadow darkens each pixel of a shape whose
/// coverage is `cover` (0..1 per pixel, already scaled by colour alpha
/// and opacity: multiply by the shadow colour). The raster must extend
/// `sh.reach()` beyond the shape.
fn inner_shadow_mask(cover: &[f32], w: usize, h: usize, scale: f64, sh: &document::Shadow) -> Vec<f32> {
    // Blurred "outside", shifted by the offset, kept inside the shape.
    let mut out: Vec<f32> = cover.iter().map(|a| 1.0 - a.clamp(0.0, 1.0)).collect();
    gaussian_blur(&mut out, w, h, sh.sigma() * scale);
    let (dx, dy) = ((sh.dx * scale).round() as isize, (sh.dy * scale).round() as isize);
    let mut k = vec![0.0f32; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let i = y as usize * w + x as usize;
            let (sx, sy) = (x - dx, y - dy);
            let o = if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize { 1.0 } else { out[sy as usize * w + sx as usize] };
            k[i] = o.clamp(0.0, 1.0) * sh.color.a;
        }
    }
    k
}

/// Inner shadow composited over premultiplied planes, keeping their alpha.
fn apply_inner_shadow(planes: &mut [Vec<f32>; 4], w: usize, h: usize, scale: f64, sh: &document::Shadow, opacity: f32) {
    let k = inner_shadow_mask(&planes[3], w, h, scale, sh);
    let c = [sh.color.r, sh.color.g, sh.color.b];
    for i in 0..w * h {
        let a = k[i] * opacity;
        if a <= 0.0 {
            continue;
        }
        for ch in 0..3 {
            planes[ch][i] = planes[ch][i] * (1.0 - a) + c[ch] * a * planes[3][i];
        }
    }
}

/// How far the inner shadow needs to look outside the shape.
pub fn inner_reach(shape: &Shape) -> f64 {
    shape.inner_shadow.as_ref().map(|s| s.reach() + 1.0).unwrap_or(0.0)
}

/// Rasterize a shape with its noise and blur (and drop shadow underneath),
/// at `scale` pixels per document unit. Used where the effect has no vector
/// equivalent (PDF).
pub fn rasterize_shape(shape: &Shape, fonts: &mut FontManager, scale: f64, max_px: f64) -> Option<Raster> {
    let (layers, bounds, rings) = shape_layers(shape, fonts, (0.25 / scale).clamp(1e-3, 1.0));
    let b = bounds?;
    let sigma = shape.blur.max(0.0) / 2.0;
    let pad = effect_reach(shape) + sigma * 3.0 + shadow_reach(shape) + inner_reach(shape) + 2.0;
    let rect = b.inflate(pad, pad);
    let (scale, w, h) = raster_dims(rect, scale, max_px);
    let refs: Vec<Layer> = layers.iter().map(|(v, i, c)| (v.as_slice(), i.as_slice(), c.clone())).collect();
    let mut planes = composite(&refs[..rings], rect, scale, w, h);
    // Inner shadow and noise apply to fill and stroke, like on screen.
    let mut top = composite(&refs[rings..], rect, scale, w, h);
    if let Some(sh) = &shape.inner_shadow {
        apply_inner_shadow(&mut top, w, h, scale, sh, shape.opacity);
    }
    if shape.noise > 0.0 {
        let n = shape.noise.clamp(0.0, 1.0);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let a = top[3][i];
                if a <= 0.0 {
                    continue;
                }
                let (g, light) = grain(x, y);
                let k = g * n;
                for ch in 0..3 {
                    top[ch][i] = top[ch][i] * (1.0 - k) + if light { a * k } else { 0.0 };
                }
            }
        }
    }
    for i in 0..w * h {
        let inv = 1.0 - top[3][i];
        for ch in 0..4 {
            planes[ch][i] = top[ch][i] + planes[ch][i] * inv;
        }
    }
    if sigma > 0.0 {
        for pl in planes.iter_mut() {
            gaussian_blur(pl, w, h, sigma * scale);
        }
    }
    if let Some(sh) = &shape.shadow {
        // Shadow: blurred alpha of the result, offset, under the shape.
        let mut mask = planes[3].clone();
        gaussian_blur(&mut mask, w, h, sh.sigma() * scale);
        let (dx, dy) = ((sh.dx * scale).round() as isize, (sh.dy * scale).round() as isize);
        let c = sh.color;
        let ca = c.a * shape.opacity;
        for y in 0..h as isize {
            for x in 0..w as isize {
                let (sx, sy) = (x - dx, y - dy);
                if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize {
                    continue;
                }
                let m = mask[(sy as usize) * w + sx as usize] * ca;
                let i = y as usize * w + x as usize;
                let inv = 1.0 - planes[3][i];
                let sc = [c.r * m, c.g * m, c.b * m, m];
                for ch in 0..4 {
                    planes[ch][i] += sc[ch] * inv;
                }
            }
        }
    }
    Some(to_raster(&planes, rect, w, h))
}

/// Just the drop shadow of a shape as a raster (the shape stays vector).
pub fn rasterize_shadow(shape: &Shape, fonts: &mut FontManager, scale: f64, max_px: f64) -> Option<Raster> {
    let sh = shape.shadow.as_ref()?;
    let (layers, bounds, _) = shape_layers(shape, fonts, (0.25 / scale).clamp(1e-3, 1.0));
    let b = bounds?;
    let pad = effect_reach(shape) + sh.sigma() * 3.0 + 2.0;
    let rect = b.inflate(pad, pad);
    let (scale, w, h) = raster_dims(rect, scale, max_px);
    let mut mask = vec![0.0f32; w * h];
    for (v, i, _) in &layers {
        raster_triangles(&mut mask, w, h, rect, scale, v, i);
    }
    gaussian_blur(&mut mask, w, h, sh.sigma() * scale);
    let c = sh.color;
    let ca = c.a * shape.opacity;
    let planes = [mask.iter().map(|m| m * ca * c.r).collect(), mask.iter().map(|m| m * ca * c.g).collect(), mask.iter().map(|m| m * ca * c.b).collect(), mask.iter().map(|m| m * ca).collect()];
    Some(to_raster(&planes, rect + kurbo::Vec2::new(sh.dx, sh.dy), w, h))
}

/// The mesh again, textured with screen-space grain.
fn noise_mesh(m: &MeshData, view: &View, tex: TextureId, alpha: u8) -> Mesh {
    let mut mesh = Mesh::with_texture(tex);
    let color = Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha);
    for v in &m.verts {
        let p = view.to_screen(Point::new(v[0] as f64, v[1] as f64));
        let uv = Pos2::new(p.x / NOISE_TILE as f32, p.y / NOISE_TILE as f32);
        mesh.vertices.push(Vertex { pos: p, uv, color });
    }
    mesh.indices = m.indices.clone();
    mesh
}

/// Inner shadow strength texture plus the geometry that clips it (the
/// shape's visible area, drawn textured so its edges stay crisp).
struct InnerTex {
    rev: u64,
    bucket: i32,
    rect: Rect,
    tex: TextureHandle,
    verts: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

/// Blurred alpha mask of a shape, rasterized at a zoom bucket.
struct ShadowTex {
    rev: u64,
    bucket: i32,
    /// Document-space rectangle the texture covers (before the offset).
    rect: Rect,
    tex: Option<TextureHandle>,
}

/// How far stroke and outline rings extend beyond the outline.
pub fn effect_reach(shape: &Shape) -> f64 {
    let own = shape.stroke.as_ref().map(|s| s.reach()).unwrap_or(0.0) + shape.outlines.iter().map(|o| o.width.max(0.0)).sum::<f64>();
    match &shape.kind {
        // Children's strokes and outlines reach past the group's outline.
        ShapeKind::Group { children } => own + children.iter().filter(|c| c.visible).map(effect_reach).fold(0.0, f64::max),
        _ => own,
    }
}

pub fn shadow_reach(shape: &Shape) -> f64 {
    shape.shadow.as_ref().map(|s| s.reach()).unwrap_or(0.0)
}

/// Largest shadow texture side in pixels.
const SHADOW_MAX_PX: f64 = 1024.0;

/// Rasterize triangles (document space) into an alpha mask.
fn raster_triangles(mask: &mut [f32], w: usize, h: usize, rect: Rect, scale: f64, verts: &[[f32; 2]], indices: &[u32]) {
    let to_px = |v: [f32; 2]| ((v[0] as f64 - rect.x0) * scale, (v[1] as f64 - rect.y0) * scale);
    for tri in indices.chunks_exact(3) {
        let a = to_px(verts[tri[0] as usize]);
        let b = to_px(verts[tri[1] as usize]);
        let c = to_px(verts[tri[2] as usize]);
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-12 {
            continue;
        }
        let x0 = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
        let x1 = (a.0.max(b.0).max(c.0).ceil() as usize).min(w);
        let y0 = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
        let y1 = (a.1.max(b.1).max(c.1).ceil() as usize).min(h);
        let edge = |p: (f64, f64), q: (f64, f64), x: f64, y: f64| (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0);
        for y in y0..y1 {
            let py = y as f64 + 0.5;
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let e0 = edge(a, b, px, py);
                let e1 = edge(b, c, px, py);
                let e2 = edge(c, a, px, py);
                let inside = if area > 0.0 { e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0 } else { e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0 };
                if inside {
                    mask[y * w + x] = 1.0;
                }
            }
        }
    }
}

/// Approximate a Gaussian blur with three box blurs (in place).
pub fn gaussian_blur(mask: &mut [f32], w: usize, h: usize, sigma: f64) {
    if sigma < 0.3 {
        return;
    }
    // Box sizes per "Fast Almost-Gaussian Filtering" (Kovesi).
    let n = 3.0;
    let wi = ((12.0 * sigma * sigma / n) + 1.0).sqrt().floor();
    let wl = if (wi as i64) % 2 == 0 { wi - 1.0 } else { wi };
    let wu = wl + 2.0;
    let m = ((12.0 * sigma * sigma - n * wl * wl - 4.0 * n * wl - 3.0 * n) / (-4.0 * wl - 4.0)).round();
    let mut tmp = vec![0.0f32; mask.len()];
    for i in 0..3 {
        let size = if (i as f64) < m { wl } else { wu };
        let r = ((size - 1.0) / 2.0).max(0.0) as usize;
        if r == 0 {
            continue;
        }
        box_pass(mask, &mut tmp, w, h, r, true);
        box_pass(&tmp, mask, w, h, r, false);
    }
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let norm = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut acc = 0.0f32;
        // Zero padding outside the mask.
        for i in 0..=r.min(len.saturating_sub(1)) {
            acc += src[idx(line, i)];
        }
        for i in 0..len {
            dst[idx(line, i)] = acc * norm;
            let add = i + r + 1;
            if add < len {
                acc += src[idx(line, add)];
            }
            if i >= r {
                acc -= src[idx(line, i - r)];
            }
        }
    }
}

pub fn zoom_bucket(zoom: f32) -> i32 {
    zoom.max(1e-4).log2().floor() as i32
}

/// Outline + optional text layout for a shape in document space.
pub fn compute_outline(shape: &Shape, fonts: &mut FontManager) -> (BezPath, Option<TextLayout>) {
    match &shape.kind {
        ShapeKind::Text(td) => {
            fonts.ensure_available(&td.family, td.weight, td.italic);
            let layout = fonts.layout(td);
            let mut all = layout.path.clone();
            for (p, _) in &layout.colored {
                all.extend(p.elements().iter().copied());
            }
            (shape.transform * all, Some(layout))
        }
        ShapeKind::Compound { op, children } => (compound_outline(*op, children, fonts), None),
        // A group's outline is all of its children's (for bounds and outlines).
        ShapeKind::Group { children } => {
            let mut p = BezPath::new();
            for c in children.iter().filter(|c| c.visible) {
                p.extend(compute_outline(c, fonts).0.iter());
            }
            (p, None)
        }
        kind => (shape.transform * geom::local_path(kind), None),
    }
}

/// Live boolean of a compound's children (document space).
pub fn compound_outline(op: crate::boolean::BoolOp, children: &[Shape], fonts: &mut FontManager) -> BezPath {
    let mut acc: Option<crate::boolean::Polys> = None;
    for c in children {
        let (p, _) = compute_outline(c, fonts);
        let polys = crate::boolean::path_to_polys(&p, c.fill_rule);
        acc = Some(match acc {
            None => polys,
            Some(a) => crate::boolean::op(&a, &polys, op),
        });
    }
    acc.map(|p| geom::polys_to_bez(&p)).unwrap_or_default()
}

/// Local-space bounds used for image fills.
fn local_bounds(shape: &Shape, cache: &CachedShape) -> Rect {
    if let Some((w, h)) = shape.kind.size() {
        return Rect::new(0.0, 0.0, w, h);
    }
    let inv = shape.transform.inverse();
    let local = inv * (*cache.doc_path).clone();
    geom::path_bounds(&local).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0))
}

impl GeoCache {
    pub fn get(&self, id: u64) -> Option<&CachedShape> {
        self.map.get(&id)
    }

    pub fn invalidate_all(&mut self) {
        self.map.clear();
    }

    /// Ensure outline geometry is up to date.
    /// Hit areas of the top-level shapes (same order): bounds grown by effect
    /// reach; groups cover everything (their children are tested).
    pub fn hit_boxes(&mut self, doc: &Document, fonts: &mut FontManager) -> std::sync::Arc<Vec<Rect>> {
        let key = (doc.revision(), fonts.generation, doc.shapes().len());
        if let Some((k, v)) = &self.hit_boxes {
            if *k == key {
                return v.clone();
            }
        }
        let all = Rect::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY);
        let none = Rect::new(f64::NAN, f64::NAN, f64::NAN, f64::NAN);
        let boxes: Vec<Rect> = doc
            .shapes()
            .iter()
            .map(|s| {
                if matches!(s.kind, ShapeKind::Group { .. }) {
                    return all;
                }
                let r = effect_reach(s);
                self.update(s, fonts).bounds.map(|b| b.inflate(r, r)).unwrap_or(none)
            })
            .collect();
        let v = std::sync::Arc::new(boxes);
        self.hit_boxes = Some((key, v.clone()));
        v
    }

    pub fn update(&mut self, shape: &Shape, fonts: &mut FontManager) -> &CachedShape {
        let font_gen = if matches!(shape.kind, ShapeKind::Text(_)) { fonts.generation } else { 0 };
        let stale = match self.map.get(&shape.id) {
            Some(c) => c.rev != shape.rev || c.font_gen != font_gen,
            None => true,
        };
        if stale {
            let (path, text) = compute_outline(shape, fonts);
            let mut bounds = match &text {
                Some(t) => Some(shape.transform.transform_rect_bbox(t.bounds)),
                None => geom::path_bounds(&path),
            };
            let mut fx = effects(shape, &path, fonts);
            let text_colors = text.as_ref().map(|t| split_text_colors(shape, t, &mut fx)).unwrap_or_default();
            if let Some(c) = &fx.clip {
                let cb = crate::boolean::bounds(c);
                bounds = match (bounds, cb) {
                    (Some(b), Some(cb)) => {
                        let i = b.intersect(cb);
                        Some(if i.width() < 0.0 || i.height() < 0.0 { Rect::from_origin_size(cb.origin(), (0.0, 0.0)) } else { i })
                    }
                    (b, _) => b,
                };
            }
            self.map.insert(
                shape.id,
                CachedShape {
                    rev: shape.rev,
                    font_gen,
                    doc_path: Arc::new(path),
                    bounds,
                    text: text.map(Arc::new),
                    bucket: i32::MIN,
                    fill: None,
                    stroke: None,
                    stroke_parts: fx.stroke_parts.map(Arc::new),
                    fill_path: Arc::new(fx.fill_path),
                    fill_rule: fx.fill_rule,
                    clip: fx.clip.map(Arc::new),
                    rings: Arc::new(fx.rings),
                    ring_area: fx.ring_area.map(Arc::new),
                    ring_meshes: Vec::new(),
                    text_colors: Arc::new(text_colors),
                    extra_meshes: Vec::new(),
                    fill_extra: Vec::new(),
                    stroke_extra: Vec::new(),
                },
            );
        }
        self.map.get(&shape.id).unwrap()
    }

    fn ensure_meshes(&mut self, shape: &Shape, doc: &Document, bucket: i32) {
        let Some(c) = self.map.get_mut(&shape.id) else { return };
        if c.bucket == bucket {
            return;
        }
        c.bucket = bucket;
        let tol = 0.2 / 2f64.powi(bucket);
        let tol = tol.clamp(1e-4, 5.0);
        c.fill = None;
        c.stroke = None;
        c.extra_meshes = c
            .text_colors
            .iter()
            .map(|(p, col)| {
                let (verts, indices) = tessellate_fill_raw(p, tol, FillRule::NonZero);
                MeshData { verts, uvs: Vec::new(), indices, texture: None, color: col.to_color32(shape.opacity), grad: None }
            })
            .collect();
        c.ring_meshes = c
            .rings
            .iter()
            .map(|(p, paint)| {
                let (verts, indices) = tessellate_fill_raw(p, tol, FillRule::EvenOdd);
                match paint {
                    Paint::Gradient(g) => {
                        let gs = GradSampler::new(shape, ring_bounds(shape, p), g, shape.opacity);
                        let uvs = verts.iter().map(|v| gs.uv(Point::new(v[0] as f64, v[1] as f64))).collect();
                        MeshData {
                            verts,
                            uvs,
                            indices,
                            texture: Some(TexKey::Gradient(g.ramp_key(), g.kind)),
                            color: Color32::from_white_alpha((shape.opacity * 255.0).round() as u8),
                            grad: Some(Arc::new(gs)),
                        }
                    }
                    other => {
                        let col = other.solid().unwrap_or(document::Rgba::BLACK);
                        MeshData { verts, uvs: Vec::new(), indices, texture: None, color: col.to_color32(shape.opacity), grad: None }
                    }
                }
            })
            .collect();
        let lb = local_bounds(shape, c);
        c.fill_extra.clear();
        if shape.fill.is_some() || !shape.fills.is_empty() {
            let (verts, indices) = tessellate_fill_raw(&c.fill_path, tol, c.fill_rule);
            c.fill = shape.fill.as_ref().and_then(|p| paint_mesh(shape, p, verts.clone(), indices.clone(), lb, doc));
            c.fill_extra = shape.fills.iter().filter_map(|p| paint_mesh(shape, p, verts.clone(), indices.clone(), lb, doc)).collect();
        }
        if let (Some(s), Some(parts)) = (&shape.stroke, &c.stroke_parts) {
            c.stroke = stroke_mesh(s, parts, tol, shape.opacity);
        }
        c.stroke_extra = shape
            .strokes
            .iter()
            .filter_map(|st| {
                let parts = crate::strokes::stroke_parts(&c.doc_path, st, shape.fill_rule);
                stroke_mesh(st, &parts, tol, shape.opacity)
            })
            .collect();
    }

    fn texture(&mut self, ctx: &egui::Context, doc: &Document, key: TexKey, shape: &Shape) -> Option<TextureId> {
        if let Some(t) = self.textures.get(&key) {
            return Some(t.id());
        }
        let (image, tiled) = match key {
            TexKey::Image(i, t) => (i, t),
            TexKey::Gradient(rk, _) => {
                // The fill's gradient, or an outline step's.
                let fills = shape.fill.iter().chain(shape.fills.iter()).filter_map(|p| match p {
                    Paint::Gradient(g) => Some(g.clone()),
                    _ => None,
                });
                let g = fills.chain(shape.outlines.iter().filter_map(|o| o.gradient.clone())).find(|g| g.ramp_key() == rk)?;
                let g = &g;
                let img = ramp_image(g);
                let handle = ctx.load_texture(format!("grad-{:x}", g.ramp_key()), img, egui::TextureOptions::LINEAR);
                let id = handle.id();
                self.textures.insert(key, handle);
                return Some(id);
            }
        };
        let asset = doc.images.get(&image)?;
        let rgba = asset.rgba.as_ref()?;
        let (w, h) = (asset.width as usize, asset.height as usize);
        let img = if tiled {
            egui::ColorImage::from_rgba_unmultiplied([w, h], rgba)
        } else {
            let (pw, ph) = (w + 2, h + 2);
            let mut padded = vec![0u8; pw * ph * 4];
            for y in 0..h {
                let src = &rgba[y * w * 4..(y + 1) * w * 4];
                let dst = ((y + 1) * pw + 1) * 4;
                padded[dst..dst + w * 4].copy_from_slice(src);
            }
            egui::ColorImage::from_rgba_unmultiplied([pw, ph], &padded)
        };
        let opts = egui::TextureOptions {
            wrap_mode: if tiled { egui::TextureWrapMode::Repeat } else { egui::TextureWrapMode::ClampToEdge },
            ..egui::TextureOptions::LINEAR
        };
        let handle = ctx.load_texture(format!("img-{image}-{tiled}"), img, opts);
        let id = handle.id();
        self.textures.insert(key, handle);
        Some(id)
    }

    /// Build (or reuse) the blurred shadow texture of a shape.
    fn shadow_texture(&mut self, ctx: &egui::Context, shape: &Shape, bucket: i32, fonts: &mut FontManager) -> Option<(TextureId, Rect)> {
        let sh = shape.shadow.as_ref()?;
        if let Some(t) = self.shadows.get(&shape.id) {
            if t.rev == shape.rev && t.bucket == bucket {
                return t.tex.as_ref().map(|x| (x.id(), t.rect));
            }
        }
        let c = self.map.get(&shape.id)?;
        let b = c.bounds?;
        let pad = effect_reach(shape) + sh.sigma() * 3.0 + 1.0;
        let rect = b.inflate(pad, pad);
        let zoom = 2f64.powi(bucket);
        let scale = zoom.min(SHADOW_MAX_PX / rect.width().max(rect.height()).max(1e-6));
        let (w, h) = (((rect.width() * scale).ceil() as usize).max(1), ((rect.height() * scale).ceil() as usize).max(1));
        let mut mask = vec![0.0f32; w * h];
        if matches!(shape.kind, ShapeKind::Group { .. }) {
            let (layers, _, _) = shape_layers(shape, fonts, (0.2 / zoom).clamp(1e-4, 5.0));
            for (v, i, _) in &layers {
                raster_triangles(&mut mask, w, h, rect, scale, v, i);
            }
        } else {
            for m in c.meshes() {
                raster_triangles(&mut mask, w, h, rect, scale, &m.verts, &m.indices);
            }
        }
        gaussian_blur(&mut mask, w, h, sh.sigma() * scale);
        let pixels: Vec<Color32> = mask.iter().map(|a| Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)).collect();
        let img = egui::ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels };
        let tex = ctx.load_texture(format!("shadow-{}", shape.id), img, egui::TextureOptions::LINEAR);
        let id = tex.id();
        self.shadows.insert(shape.id, ShadowTex { rev: shape.rev, bucket, rect, tex: Some(tex) });
        Some((id, rect))
    }

    fn paint_shadow(&mut self, painter: &egui::Painter, shape: &Shape, view: &View, bucket: i32, alpha: f32, fonts: &mut FontManager) {
        let Some(sh) = shape.shadow.clone() else { return };
        let color = sh.color.to_color32(shape.opacity).gamma_multiply(alpha);
        let off = kurbo::Vec2::new(sh.dx, sh.dy);
        let group = matches!(shape.kind, ShapeKind::Group { .. });
        if sh.sigma() * view.zoom as f64 <= 0.35 && !group {
            // Hard shadow: repaint the geometry offset.
            let Some(c) = self.map.get(&shape.id) else { return };
            for m in c.meshes() {
                let mut mesh = Mesh::default();
                for v in &m.verts {
                    let p = view.to_screen(Point::new(v[0] as f64, v[1] as f64) + off);
                    mesh.vertices.push(Vertex { pos: p, uv: egui::epaint::WHITE_UV, color });
                }
                mesh.indices = m.indices.clone();
                painter.add(mesh);
            }
            return;
        }
        let ctx = painter.ctx().clone();
        if let Some((tex, rect)) = self.shadow_texture(&ctx, shape, bucket, fonts) {
            let r = view.rect_to_screen(rect + off);
            let mut mesh = Mesh::with_texture(tex);
            mesh.add_rect_with_uv(r, egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), color);
            painter.add(mesh);
        }
    }

    /// Build (or reuse) the inner shadow of a shape: a texture of the
    /// shadow's strength (extending past the edges) and the area it covers.
    fn inner_texture(&mut self, ctx: &egui::Context, shape: &Shape, bucket: i32, fonts: &mut FontManager) -> Option<&InnerTex> {
        let sh = shape.inner_shadow.as_ref()?;
        let fresh = self.inner.get(&shape.id).is_some_and(|t| t.rev == shape.rev && t.bucket == bucket);
        if !fresh {
            let c = self.map.get(&shape.id)?;
            let b = c.bounds?;
            let zoom = 2f64.powi(bucket);
            let tol = (0.2 / zoom).clamp(1e-4, 5.0);
            // The covered area: the fill mesh itself when that's all there is
            // (identical edges), else the union of fill and stroke.
            let (verts, indices) = match (&c.fill, &c.stroke, &shape.kind) {
                (Some(f), None, k) if !matches!(k, ShapeKind::Group { .. }) => (f.verts.clone(), f.indices.clone()),
                _ => {
                    use crate::boolean::{self, BoolOp};
                    let mut area = if matches!(shape.kind, ShapeKind::Group { .. }) {
                        group_area(shape, fonts)
                    } else {
                        let path = c.doc_path.clone();
                        let mut a: boolean::Polys = Vec::new();
                        if shape.fill.is_some() {
                            a = boolean::path_to_polys(&path, shape.fill_rule);
                        }
                        if let Some(st) = shape.stroke.as_ref().filter(|s| s.width > 0.0) {
                            let sa = crate::strokes::stroke_area(&path, st, shape.fill_rule);
                            a = if a.is_empty() { sa } else { boolean::op(&a, &sa, BoolOp::Union) };
                        }
                        a
                    };
                    if let Some(clip) = &c.clip {
                        area = boolean::op(&area, clip, BoolOp::Intersect);
                    }
                    tessellate_fill_raw(&geom::polys_to_bez(&area), tol, FillRule::EvenOdd)
                }
            };
            let pad = effect_reach(shape) + inner_reach(shape);
            let rect = b.inflate(pad, pad);
            let scale = zoom.min(SHADOW_MAX_PX / rect.width().max(rect.height()).max(1e-6));
            let (w, h) = (((rect.width() * scale).ceil() as usize).max(1), ((rect.height() * scale).ceil() as usize).max(1));
            let mut cover = vec![0.0f32; w * h];
            raster_triangles(&mut cover, w, h, rect, scale, &verts, &indices);
            cover.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
            let k = inner_shadow_mask(&cover, w, h, scale, sh);
            let pixels: Vec<Color32> = k.iter().map(|a| Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
            let img = egui::ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels };
            let tex = ctx.load_texture(format!("inner-{}", shape.id), img, egui::TextureOptions::LINEAR);
            self.inner.insert(shape.id, InnerTex { rev: shape.rev, bucket, rect, tex, verts, indices });
        }
        self.inner.get(&shape.id)
    }

    fn paint_inner_shadow(&mut self, painter: &egui::Painter, shape: &Shape, view: &View, bucket: i32, alpha: f32, fonts: &mut FontManager) {
        let Some(sh) = shape.inner_shadow.clone() else { return };
        let ctx = painter.ctx().clone();
        let Some(t) = self.inner_texture(&ctx, shape, bucket, fonts) else { return };
        // Alpha already carries the shadow colour's alpha.
        let color = sh.color.with_alpha(1.0).to_color32(shape.opacity).gamma_multiply(alpha);
        let mut mesh = Mesh::with_texture(t.tex.id());
        let (r, z, o) = (t.rect, view.zoom, view.origin);
        mesh.vertices = t
            .verts
            .iter()
            .map(|v| Vertex {
                pos: Pos2::new(v[0] * z + o.x, v[1] * z + o.y),
                uv: Pos2::new(((v[0] as f64 - r.x0) / r.width()) as f32, ((v[1] as f64 - r.y0) / r.height()) as f32),
                color,
            })
            .collect();
        mesh.indices = t.indices.clone();
        painter.add(mesh);
    }

    pub fn prune(&mut self, doc: &Document) {
        let key = (doc.revision(), doc.shapes().len(), doc.images.len());
        if self.pruned == Some(key) {
            return;
        }
        self.pruned = Some(key);
        let ids: std::collections::HashSet<u64> = doc.all_ids().into_iter().collect();
        self.map.retain(|k, _| ids.contains(k));
        self.shadows.retain(|k, _| doc.get(*k).map(|s| s.shadow.is_some()).unwrap_or(false));
        self.blurs.retain(|k, _| doc.get(*k).map(|s| s.blur > 0.0).unwrap_or(false));
        self.inner.retain(|k, _| doc.get(*k).map(|s| s.inner_shadow.is_some()).unwrap_or(false));
        self.svg_tex.retain(|k, _| doc.get(*k).is_some_and(svg_paint::needs_svg));
        let ramps: std::collections::HashSet<u64> = ids
            .iter()
            .filter_map(|id| doc.get(*id))
            .flat_map(|s| {
                let fills: Vec<u64> = s
                    .fill
                    .iter()
                    .chain(s.fills.iter())
                    .filter_map(|p| match p {
                        Paint::Gradient(g) => Some(g.ramp_key()),
                        _ => None,
                    })
                    .collect();
                fills.into_iter().chain(s.outlines.iter().filter_map(|o| o.gradient.as_ref().map(|g| g.ramp_key())))
            })
            .collect();
        self.textures.retain(|k, _| match k {
            TexKey::Image(img, _) => doc.images.contains_key(img),
            TexKey::Gradient(r, _) => ramps.contains(r),
        });
    }

    /// Draw all visible shapes.
    pub fn paint_document(
        &mut self,
        painter: &egui::Painter,
        doc: &Document,
        fonts: &mut FontManager,
        view: &View,
        hidden: Option<u64>,
    ) {
        let bucket = zoom_bucket(view.zoom);
        let clipping = doc.artboards.iter().any(|a| a.clip);
        for shape in doc.shapes() {
            // Artwork of a clipping artboard is cut at its edges.
            let clip = if clipping { self.update(shape, fonts).bounds.and_then(|b| doc.member_of(b)).filter(|a| a.clip) } else { None };
            match clip {
                Some(a) => {
                    let p = painter.with_clip_rect(view.rect_to_screen(a.rect()).intersect(painter.clip_rect()));
                    self.paint_shape(&p, shape, doc, fonts, view, hidden, bucket, 1.0);
                }
                None => self.paint_shape(painter, shape, doc, fonts, view, hidden, bucket, 1.0),
            }
        }
    }

    /// Outline view: every visible shape as a hairline (booleans show their result).
    pub fn paint_outlines(&mut self, painter: &egui::Painter, doc: &Document, fonts: &mut FontManager, view: &View) {
        fn walk(cache: &mut GeoCache, painter: &egui::Painter, s: &Shape, fonts: &mut FontManager, view: &View) {
            if !s.visible {
                return;
            }
            if let ShapeKind::Group { children } = &s.kind {
                for c in children {
                    walk(cache, painter, c, fonts, view);
                }
                return;
            }
            let path = cache.update(s, fonts).doc_path.clone();
            outline(painter, &path, view, egui::Stroke::new(1.0, Color32::from_gray(0x22)));
        }
        for s in doc.shapes() {
            walk(self, painter, s, fonts, view);
        }
    }

    /// Small preview of a shape fitted into `rect` (Layers). Reuses the
    /// meshes of the canvas zoom level `canvas_zoom` so nothing is re-tessellated.
    pub fn paint_thumbnail(&mut self, painter: &egui::Painter, shape: &Shape, doc: &Document, fonts: &mut FontManager, rect: egui::Rect, canvas_zoom: f32) {
        let Some(b) = self.update(shape, fonts).bounds else { return };
        let pad = 2.0;
        let z = ((rect.width() - pad * 2.0) as f64 / b.width().max(1e-6)).min((rect.height() - pad * 2.0) as f64 / b.height().max(1e-6)) as f32;
        let c = b.center();
        let view = View { zoom: z, origin: egui::vec2(rect.center().x - c.x as f32 * z, rect.center().y - c.y as f32 * z) };
        let p = painter.with_clip_rect(rect);
        let mut s = shape.clone();
        s.visible = true;
        s.shadow = None;
        self.paint_shape(&p, &s, doc, fonts, &view, None, zoom_bucket(canvas_zoom), 1.0);
    }

    /// Paint one shape (groups recurse). `alpha` is the inherited group opacity.
    #[allow(clippy::too_many_arguments)]
    fn paint_shape(&mut self, painter: &egui::Painter, shape: &Shape, doc: &Document, fonts: &mut FontManager, view: &View, hidden: Option<u64>, bucket: i32, alpha: f32) {
        if !shape.visible || Some(shape.id) == hidden {
            return;
        }
        if svg_paint::needs_svg(shape) {
            self.paint_via_svg(painter, shape, doc, fonts, view, alpha);
            return;
        }
        if let ShapeKind::Group { children } = &shape.kind {
            if group_fx(shape) {
                self.paint_group_fx(painter, shape, doc, fonts, view, hidden, bucket, alpha);
                return;
            }
            for c in children {
                self.paint_shape(painter, c, doc, fonts, view, hidden, bucket, alpha * shape.opacity);
            }
            return;
        }
        let add = |mut m: Mesh| {
            if alpha < 1.0 {
                for v in &mut m.vertices {
                    v.color = v.color.gamma_multiply(alpha);
                }
            }
            painter.add(m);
        };
        let clip = painter.clip_rect();
        let ctx = painter.ctx().clone();
        {
            let bounds = self.update(shape, fonts).bounds;
            if let Some(b) = bounds {
                let pad = effect_reach(shape) + shadow_reach(shape) + shape.blur * 1.5;
                let sb = view.rect_to_screen(b.inflate(pad, pad));
                if !sb.intersects(clip) {
                    return;
                }
            }
            self.ensure_meshes(shape, doc, bucket);
            let tex = {
                let c = self.map.get(&shape.id).unwrap();
                c.fill.as_ref().and_then(|f| f.texture)
            };
            let tex_id = tex.and_then(|key| self.texture(&ctx, doc, key, shape));
            if shape.shadow.is_some() {
                self.paint_shadow(painter, shape, view, bucket, alpha, fonts);
            }
            if shape.blur > 0.0 && shape.blur * view.zoom as f64 > 0.3 {
                if let Some((tex, rect)) = self.blur_texture(&ctx, shape, bucket, tex_id, fonts) {
                    let mut mesh = Mesh::with_texture(tex);
                    mesh.add_rect_with_uv(view.rect_to_screen(rect), egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    add(mesh);
                }
                return;
            }
            let noise_tex = (shape.noise > 0.0).then(|| self.noise_texture(&ctx));
            let ring_keys: Vec<Option<TexKey>> = self.map.get(&shape.id).unwrap().ring_meshes.iter().map(|m| m.texture).collect();
            let ring_tex: Vec<Option<TextureId>> = ring_keys.into_iter().map(|k| k.and_then(|k| self.texture(&ctx, doc, k, shape))).collect();
            let c = self.map.get(&shape.id).unwrap();
            for (r, t) in c.ring_meshes.iter().zip(ring_tex) {
                if r.texture.is_none() || t.is_some() {
                    add(to_mesh(r, view, t));
                }
            }
            if let Some(f) = &c.fill {
                if f.texture.is_none() || tex_id.is_some() {
                    add(to_mesh(f, view, tex_id));
                }
            }
            for m in &c.extra_meshes {
                add(to_mesh(m, view, None));
            }
            if let Some(s) = &c.stroke {
                add(to_mesh(s, view, None));
            }
            if shape.inner_shadow.is_some() {
                self.paint_inner_shadow(painter, shape, view, bucket, alpha, fonts);
            }
            let c = self.map.get(&shape.id).unwrap();
            if let Some(nt) = noise_tex {
                let a = (shape.noise.clamp(0.0, 1.0) * shape.opacity * 255.0) as u8;
                for m in [&c.fill, &c.stroke].into_iter().flatten().chain(c.extra_meshes.iter()) {
                    add(noise_mesh(m, view, nt, a));
                }
            }
                }
    }

    /// A group with effects: shadow under, rings, children, noise over (or
    /// everything blurred together).
    fn paint_group_fx(&mut self, painter: &egui::Painter, shape: &Shape, doc: &Document, fonts: &mut FontManager, view: &View, hidden: Option<u64>, bucket: i32, alpha: f32) {
        let ShapeKind::Group { children } = &shape.kind else { return };
        let add = |mut m: Mesh| {
            if alpha < 1.0 {
                for v in &mut m.vertices {
                    v.color = v.color.gamma_multiply(alpha);
                }
            }
            painter.add(m);
        };
        let ctx = painter.ctx().clone();
        if let Some(b) = self.update(shape, fonts).bounds {
            let pad = effect_reach(shape) + shadow_reach(shape) + shape.blur * 1.5;
            if !view.rect_to_screen(b.inflate(pad, pad)).intersects(painter.clip_rect()) {
                return;
            }
        }
        self.ensure_meshes(shape, doc, bucket);
        if shape.shadow.is_some() {
            self.paint_shadow(painter, shape, view, bucket, alpha, fonts);
        }
        if shape.blur > 0.0 && shape.blur * view.zoom as f64 > 0.3 {
            if let Some((tex, rect)) = self.blur_texture(&ctx, shape, bucket, None, fonts) {
                let mut mesh = Mesh::with_texture(tex);
                mesh.add_rect_with_uv(view.rect_to_screen(rect), egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                add(mesh);
            }
            return;
        }
        let ring_keys: Vec<Option<TexKey>> = self.map.get(&shape.id).map(|c| c.ring_meshes.iter().map(|m| m.texture).collect()).unwrap_or_default();
        let ring_tex: Vec<Option<TextureId>> = ring_keys.into_iter().map(|k| k.and_then(|k| self.texture(&ctx, doc, k, shape))).collect();
        if let Some(c) = self.map.get(&shape.id) {
            for (r, t) in c.ring_meshes.iter().zip(ring_tex) {
                if r.texture.is_none() || t.is_some() {
                    add(to_mesh(r, view, t));
                }
            }
        }
        for c in children {
            self.paint_shape(painter, c, doc, fonts, view, hidden, bucket, alpha * shape.opacity);
        }
        if shape.inner_shadow.is_some() {
            self.paint_inner_shadow(painter, shape, view, bucket, alpha, fonts);
        }
        if shape.noise > 0.0 {
            let nt = self.noise_texture(&ctx);
            let a = (shape.noise.clamp(0.0, 1.0) * shape.opacity * 255.0) as u8;
            fn leaves(s: &Shape, out: &mut Vec<u64>) {
                if !s.visible {
                    return;
                }
                match &s.kind {
                    ShapeKind::Group { children } => children.iter().for_each(|c| leaves(c, out)),
                    _ => out.push(s.id),
                }
            }
            let mut ids = Vec::new();
            children.iter().for_each(|c| leaves(c, &mut ids));
            for id in ids {
                if let Some(c) = self.map.get(&id) {
                    for m in [&c.fill, &c.stroke].into_iter().flatten() {
                        add(noise_mesh(m, view, nt, a));
                    }
                }
            }
        }
    }

    fn noise_texture(&mut self, ctx: &egui::Context) -> TextureId {
        if let Some(t) = &self.noise {
            return t.id();
        }
        let n = NOISE_TILE;
        let mut seed = 0x9E37_79B9u32;
        let pixels = (0..n * n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let v = (seed >> 24) as u8;
                // Grain is a light/dark mix around mid-grey, composited with alpha.
                let a = ((v as i32 - 128).unsigned_abs() * 2).min(255) as u8;
                if v >= 128 { Color32::from_rgba_premultiplied(a, a, a, a) } else { Color32::from_rgba_premultiplied(0, 0, 0, a) }
            })
            .collect();
        let img = egui::ColorImage { size: [n, n], source_size: egui::vec2(n as f32, n as f32), pixels };
        let opts = egui::TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..egui::TextureOptions::NEAREST };
        let t = ctx.load_texture("noise", img, opts);
        let id = t.id();
        self.noise = Some(t);
        id
    }

    /// Colored raster of the shape (rings, fill, stroke), Gaussian blurred.
    fn blur_texture(&mut self, ctx: &egui::Context, shape: &Shape, bucket: i32, _img: Option<TextureId>, fonts: &mut FontManager) -> Option<(TextureId, Rect)> {
        if let Some(t) = self.blurs.get(&shape.id) {
            if t.rev == shape.rev && t.bucket == bucket {
                return t.tex.as_ref().map(|x| (x.id(), t.rect));
            }
        }
        let c = self.map.get(&shape.id)?;
        let b = c.bounds?;
        let sigma = shape.blur / 2.0;
        let pad = effect_reach(shape) + sigma * 3.0 + inner_reach(shape) + 1.0;
        let rect = b.inflate(pad, pad);
        let zoom = 2f64.powi(bucket);
        let scale = zoom.min(SHADOW_MAX_PX / rect.width().max(rect.height()).max(1e-6));
        let (w, h) = (((rect.width() * scale).ceil() as usize).max(1), ((rect.height() * scale).ceil() as usize).max(1));
        let owned;
        let (layers, rings): (Vec<Layer>, usize) = if matches!(shape.kind, ShapeKind::Group { .. }) {
            let n;
            (owned, _, n) = shape_layers(shape, fonts, (0.2 / zoom).clamp(1e-4, 5.0));
            (owned.iter().map(|(v, i, sh)| (v.as_slice(), i.as_slice(), sh.clone())).collect(), n)
        } else {
            (c.meshes().map(|m| (m.verts.as_slice(), m.indices.as_slice(), mesh_shade(m))).collect(), c.ring_meshes.len())
        };
        let mut planes = composite(&layers[..rings], rect, scale, w, h);
        let mut top = composite(&layers[rings..], rect, scale, w, h);
        if let Some(sh) = &shape.inner_shadow {
            apply_inner_shadow(&mut top, w, h, scale, sh, shape.opacity);
        }
        for i in 0..w * h {
            let inv = 1.0 - top[3][i];
            for ch in 0..4 {
                planes[ch][i] = top[ch][i] + planes[ch][i] * inv;
            }
        }
        for pl in planes.iter_mut() {
            gaussian_blur(pl, w, h, sigma * scale);
        }
        let px = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let pixels = (0..w * h).map(|i| Color32::from_rgba_premultiplied(px(planes[0][i]), px(planes[1][i]), px(planes[2][i]), px(planes[3][i]))).collect();
        let img = egui::ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels };
        let tex = ctx.load_texture(format!("blur-{}", shape.id), img, egui::TextureOptions::LINEAR);
        let id = tex.id();
        self.blurs.insert(shape.id, ShadowTex { rev: shape.rev, bucket, rect, tex: Some(tex) });
        Some((id, rect))
    }
}

/// Fill mesh for one paint over the tessellated fill area.
fn paint_mesh(shape: &Shape, paint: &Paint, verts: Vec<[f32; 2]>, indices: Vec<u32>, lb: Rect, doc: &Document) -> Option<MeshData> {
    match paint {
        Paint::Solid(col) => Some(MeshData { uvs: Vec::new(), verts, indices, texture: None, color: col.to_color32(shape.opacity), grad: None }),
        Paint::Gradient(g) => {
            let gs = GradSampler::new(shape, lb, g, shape.opacity);
            let uvs = verts.iter().map(|v| gs.uv(Point::new(v[0] as f64, v[1] as f64))).collect();
            Some(MeshData {
                verts,
                uvs,
                indices,
                texture: Some(TexKey::Gradient(g.ramp_key(), g.kind)),
                color: Color32::from_white_alpha((shape.opacity * 255.0).round() as u8),
                grad: Some(Arc::new(gs)),
            })
        }
        Paint::Image(img) => {
            let asset = doc.images.get(&img.image)?;
            let inv = shape.transform.inverse();
            let (iw, ih) = (asset.width.max(1) as f64, asset.height.max(1) as f64);
            let tiled = img.mode == ImageMode::Tile;
            let uvs = verts
                .iter()
                .map(|v| {
                    let lp = inv * Point::new(v[0] as f64, v[1] as f64);
                    let uv = image_uv(img.mode, img.scale as f64, lb, iw, ih, lp);
                    if tiled {
                        [uv.x as f32, uv.y as f32]
                    } else {
                        // Account for the 1px transparent padding.
                        [((uv.x * iw + 1.0) / (iw + 2.0)) as f32, ((uv.y * ih + 1.0) / (ih + 2.0)) as f32]
                    }
                })
                .collect();
            Some(MeshData { verts, uvs, indices, texture: Some(TexKey::Image(img.image, tiled)), color: Color32::from_white_alpha((img.opacity * shape.opacity * 255.0) as u8), grad: None })
        }
        // Drawn through the SVG renderer (see `needs_svg`).
    }
}

/// Mesh of a stroke's parts.
fn stroke_mesh(s: &document::Stroke, parts: &[crate::strokes::StrokePart], tol: f64, opacity: f32) -> Option<MeshData> {
    if s.width <= 0.0 || parts.is_empty() {
        return None;
    }
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for part in parts.iter() {
        let (v, i) = match part {
            crate::strokes::StrokePart::Line(p) => tessellate_stroke_raw(p, tol, s),
            crate::strokes::StrokePart::Fill(p, rule) => tessellate_fill_raw(p, tol, *rule),
        };
        let base = verts.len() as u32;
        verts.extend(v);
        indices.extend(i.into_iter().map(|k| k + base));
    }
    Some(MeshData { verts, uvs: Vec::new(), indices, texture: None, color: s.color.to_color32(opacity), grad: None })
}

/// Colour ramp texture of a gradient (straight alpha).
fn ramp_image(g: &document::Gradient) -> egui::ColorImage {
    let px = |c: document::Rgba| Color32::from_rgba_unmultiplied((c.r * 255.0).round() as u8, (c.g * 255.0).round() as u8, (c.b * 255.0).round() as u8, (c.a * 255.0).round() as u8);
    if g.is_baked() {
        let img = bake_gradient(g, RAMP);
        let pixels = img.pixels().map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])).collect();
        return egui::ColorImage { size: [RAMP, RAMP], source_size: egui::vec2(RAMP as f32, RAMP as f32), pixels };
    }
    match g.kind {
        document::GradientKind::Linear => {
            let pixels = (0..RAMP).map(|i| px(g.color_at(i as f32 / (RAMP - 1) as f32))).collect();
            egui::ColorImage { size: [RAMP, 1], source_size: egui::vec2(RAMP as f32, 1.0), pixels }
        }
        _ => {
            let ramp: Vec<Color32> = (0..=RAMP * 2).map(|i| px(g.color_at(i as f32 / RAMP as f32))).collect();
            let mut pixels = Vec::with_capacity(RAMP * RAMP);
            for y in 0..RAMP {
                for x in 0..RAMP {
                    let dx = (x as f32 + 0.5) / RAMP as f32 * 2.0 - 1.0;
                    let dy = (y as f32 + 0.5) / RAMP as f32 * 2.0 - 1.0;
                    let t = (dx * dx + dy * dy).sqrt();
                    pixels.push(ramp[((t * RAMP as f32) as usize).min(RAMP * 2)]);
                }
            }
            egui::ColorImage { size: [RAMP, RAMP], source_size: egui::vec2(RAMP as f32, RAMP as f32), pixels }
        }
    }
}

pub fn image_uv(mode: ImageMode, scale: f64, lb: Rect, iw: f64, ih: f64, lp: Point) -> Point {
    let (bw, bh) = (lb.width().max(1e-9), lb.height().max(1e-9));
    match mode {
        ImageMode::Stretch => Point::new((lp.x - lb.x0) / bw, (lp.y - lb.y0) / bh),
        ImageMode::Fill | ImageMode::Fit => {
            let s = if mode == ImageMode::Fill { (bw / iw).max(bh / ih) } else { (bw / iw).min(bh / ih) };
            let (dw, dh) = (iw * s, ih * s);
            let x0 = lb.x0 + (bw - dw) / 2.0;
            let y0 = lb.y0 + (bh - dh) / 2.0;
            Point::new((lp.x - x0) / dw, (lp.y - y0) / dh)
        }
        ImageMode::Tile => {
            let s = scale.max(0.01);
            Point::new((lp.x - lb.x0) / (iw * s), (lp.y - lb.y0) / (ih * s))
        }
    }
}

/// Placement rectangle (local coords) of an image fill, for exporters.
pub fn image_rect(mode: ImageMode, lb: Rect, iw: f64, ih: f64) -> Rect {
    let (bw, bh) = (lb.width(), lb.height());
    match mode {
        ImageMode::Stretch | ImageMode::Tile => lb,
        ImageMode::Fill | ImageMode::Fit => {
            let s = if mode == ImageMode::Fill { (bw / iw).max(bh / ih) } else { (bw / iw).min(bh / ih) };
            let (dw, dh) = (iw * s, ih * s);
            let x0 = lb.x0 + (bw - dw) / 2.0;
            let y0 = lb.y0 + (bh - dh) / 2.0;
            Rect::new(x0, y0, x0 + dw, y0 + dh)
        }
    }
}

/// Maps a shape's gradient units (its bounding box, 0..1) to document space.
pub fn unit_frame(shape: &Shape, fonts: &mut FontManager) -> Affine {
    let (path, _) = compute_outline(shape, fonts);
    let lb = shape_local_bounds(shape, &path);
    shape.transform * Affine::new([lb.width().max(1e-9), 0.0, 0.0, lb.height().max(1e-9), lb.x0, lb.y0])
}

/// After replacing `old`'s geometry with `new`'s, keep a gradient fill where
/// it was on the canvas (its units follow the bounding box otherwise).
pub fn keep_gradient(old: &Shape, new: &mut Shape, fonts: &mut FontManager) {
    if !matches!(new.fill, Some(Paint::Gradient(_))) {
        return;
    }
    let m = unit_frame(new, fonts).inverse() * unit_frame(old, fonts);
    if let Some(Paint::Gradient(g)) = &mut new.fill {
        for p in [&mut g.start, &mut g.end] {
            let q = m * Point::new(p[0], p[1]);
            *p = [q.x, q.y];
        }
    }
}

/// Local bounds helper for exporters.
pub fn shape_local_bounds(shape: &Shape, doc_path: &BezPath) -> Rect {
    if let Some((w, h)) = shape.kind.size() {
        return Rect::new(0.0, 0.0, w, h);
    }
    let local = shape.transform.inverse() * doc_path.clone();
    geom::path_bounds(&local).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0))
}

fn to_mesh(m: &MeshData, view: &View, tex: Option<TextureId>) -> Mesh {
    let mut mesh = match tex {
        Some(t) => Mesh::with_texture(t),
        None => Mesh::default(),
    };
    mesh.vertices.reserve(m.verts.len());
    let z = view.zoom;
    let o = view.origin;
    for (i, v) in m.verts.iter().enumerate() {
        let uv = if tex.is_some() {
            let u = m.uvs[i];
            Pos2::new(u[0], u[1])
        } else {
            egui::epaint::WHITE_UV
        };
        mesh.vertices.push(Vertex { pos: Pos2::new(v[0] * z + o.x, v[1] * z + o.y), uv, color: m.color });
    }
    mesh.indices = m.indices.clone();
    mesh
}

/// Paint a raw mesh (e.g. a font preview) at a screen offset and scale.
pub fn paint_raw_mesh(painter: &egui::Painter, verts: &[[f32; 2]], indices: &[u32], offset: Pos2, scale: f32, color: Color32) {
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(verts.len());
    for v in verts {
        mesh.vertices.push(Vertex {
            pos: Pos2::new(offset.x + v[0] * scale, offset.y + v[1] * scale),
            uv: egui::epaint::WHITE_UV,
            color,
        });
    }
    mesh.indices = indices.to_vec();
    painter.add(mesh);
}

/// Stroke a document-space path as a thin screen-space outline.
pub fn outline(painter: &egui::Painter, path: &BezPath, view: &View, stroke: egui::Stroke) {
    let screen = view.affine() * path.clone();
    let mut pts: Vec<Pos2> = Vec::new();
    let flush = |pts: &mut Vec<Pos2>, closed: bool| {
        if pts.len() > 1 {
            let shape = if closed {
                egui::Shape::closed_line(std::mem::take(pts), stroke)
            } else {
                egui::Shape::line(std::mem::take(pts), stroke)
            };
            painter.add(shape);
        }
        pts.clear();
    };
    kurbo::flatten(screen.iter(), 0.25, |el| match el {
        PathEl::MoveTo(p) => {
            flush(&mut pts, false);
            pts.push(Pos2::new(p.x as f32, p.y as f32));
        }
        PathEl::LineTo(p) => pts.push(Pos2::new(p.x as f32, p.y as f32)),
        PathEl::ClosePath => flush(&mut pts, true),
        _ => {}
    });
    flush(&mut pts, false);
}
