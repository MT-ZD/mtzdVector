//! Document model: shapes, paints, images and undo history.

use std::collections::BTreeMap;
use std::sync::Arc;

use kurbo::{Affine, Point};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Colors & paints
// ---------------------------------------------------------------------------

/// Straight-alpha sRGB color, all channels in 0..=1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Rgba = Rgba::rgb(1.0, 1.0, 1.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Rgba { r, g, b, a: 1.0 }
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        let h = hex.trim().trim_start_matches('#');
        let p = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| v as f32 / 255.0);
        match h.len() {
            3 => {
                let c: Vec<char> = h.chars().collect();
                Some(Rgba::rgb(
                    p(&format!("{0}{0}", c[0]))?,
                    p(&format!("{0}{0}", c[1]))?,
                    p(&format!("{0}{0}", c[2]))?,
                ))
            }
            6 => Some(Rgba::rgb(p(&h[0..2])?, p(&h[2..4])?, p(&h[4..6])?)),
            8 => Some(Rgba {
                r: p(&h[0..2])?,
                g: p(&h[2..4])?,
                b: p(&h[4..6])?,
                a: p(&h[6..8])?,
            }),
            _ => None,
        }
    }

    pub fn to_hex(&self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", c(self.r), c(self.g), c(self.b))
    }

    pub fn to_color32(&self, opacity: f32) -> egui::Color32 {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        egui::Color32::from_rgba_unmultiplied(
            c(self.r),
            c(self.g),
            c(self.b),
            c(self.a * opacity),
        )
    }

    pub fn lerp(self, o: Rgba, t: f32) -> Rgba {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Rgba { r: m(self.r, o.r), g: m(self.g, o.g), b: m(self.b, o.b), a: m(self.a, o.a) }
    }

    pub fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageMode {
    /// Stretch the image over the shape bounds.
    Stretch,
    /// Scale to cover the bounds, keeping aspect ratio (cropping).
    Fill,
    /// Scale to fit inside the bounds, keeping aspect ratio.
    Fit,
    /// Repeat the image at its natural size times `scale`.
    Tile,
}

impl ImageMode {
    pub const ALL: [ImageMode; 4] = [ImageMode::Fill, ImageMode::Fit, ImageMode::Stretch, ImageMode::Tile];
    pub fn label(self) -> &'static str {
        match self {
            ImageMode::Stretch => "Stretch",
            ImageMode::Fill => "Fill",
            ImageMode::Fit => "Fit",
            ImageMode::Tile => "Tile",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageFill {
    pub image: u64,
    pub mode: ImageMode,
    pub scale: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Paint {
    Solid(Rgba),
    Image(ImageFill),
    Gradient(Gradient),
}

/// How a shape's colours combine with what is below it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {

    pub fn is_normal(&self) -> bool {
        *self == BlendMode::Normal
    }

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::ColorDodge => "Colour Dodge",
            BlendMode::ColorBurn => "Colour Burn",
            BlendMode::HardLight => "Hard Light",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::Hue => "Hue",
            BlendMode::Saturation => "Saturation",
            BlendMode::Color => "Colour",
            BlendMode::Luminosity => "Luminosity",
        }
    }

    /// CSS / SVG `mix-blend-mode` keyword.
    pub fn css(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::Darken => "darken",
            BlendMode::Lighten => "lighten",
            BlendMode::ColorDodge => "color-dodge",
            BlendMode::ColorBurn => "color-burn",
            BlendMode::HardLight => "hard-light",
            BlendMode::SoftLight => "soft-light",
            BlendMode::Difference => "difference",
            BlendMode::Exclusion => "exclusion",
            BlendMode::Hue => "hue",
            BlendMode::Saturation => "saturation",
            BlendMode::Color => "color",
            BlendMode::Luminosity => "luminosity",
        }
    }

    /// PDF blend mode name (`/BM`).
    pub fn pdf(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::ColorDodge => "ColorDodge",
            BlendMode::ColorBurn => "ColorBurn",
            BlendMode::HardLight => "HardLight",
            BlendMode::SoftLight => "SoftLight",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::Hue => "Hue",
            BlendMode::Saturation => "Saturation",
            BlendMode::Color => "Color",
            BlendMode::Luminosity => "Luminosity",
        }
    }
}

/// How a mask's artwork controls visibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskMode {
    /// Light areas show the shape, dark areas hide it.
    #[default]
    Luminance,
    /// The mask's opacity (coverage) shows the shape.
    Alpha,
}

/// Luminance or opacity mask (document space, like a clip shape).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    pub shape: Shape,
    #[serde(default)]
    pub mode: MaskMode,
    #[serde(default)]
    pub invert: bool,
}

/// Width of a variable-width stroke at `t` (0..1 along the path), as a
/// multiple of the stroke width.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidthPoint {
    pub t: f64,
    pub w: f64,
}

/// Preset width profiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidthPreset {
    Uniform,
    Taper,
    TaperEnd,
    TaperStart,
    Bulge,
    Pinch,
}

impl WidthPreset {
    pub const ALL: [WidthPreset; 6] = [WidthPreset::Uniform, WidthPreset::Taper, WidthPreset::TaperEnd, WidthPreset::TaperStart, WidthPreset::Bulge, WidthPreset::Pinch];

    pub fn label(self) -> &'static str {
        match self {
            WidthPreset::Uniform => "Uniform",
            WidthPreset::Taper => "Taper both ends",
            WidthPreset::TaperEnd => "Taper end",
            WidthPreset::TaperStart => "Taper start",
            WidthPreset::Bulge => "Bulge",
            WidthPreset::Pinch => "Pinch",
        }
    }

    pub fn profile(self) -> Vec<WidthPoint> {
        let p = |t: f64, w: f64| WidthPoint { t, w };
        match self {
            WidthPreset::Uniform => Vec::new(),
            WidthPreset::Taper => vec![p(0.0, 0.05), p(0.5, 1.0), p(1.0, 0.05)],
            WidthPreset::TaperEnd => vec![p(0.0, 1.0), p(1.0, 0.05)],
            WidthPreset::TaperStart => vec![p(0.0, 0.05), p(1.0, 1.0)],
            WidthPreset::Bulge => vec![p(0.0, 0.3), p(0.5, 1.0), p(1.0, 0.3)],
            WidthPreset::Pinch => vec![p(0.0, 1.0), p(0.5, 0.2), p(1.0, 1.0)],
        }
    }

    pub fn of(profile: &[WidthPoint]) -> Option<WidthPreset> {
        WidthPreset::ALL.into_iter().find(|p| p.profile() == profile)
    }
}

/// Width multiplier at `t` (linear between points, smoothed).
pub fn profile_at(profile: &[WidthPoint], t: f64) -> f64 {
    let Some(first) = profile.first() else { return 1.0 };
    if t <= first.t {
        return first.w;
    }
    for w in profile.windows(2) {
        if t <= w[1].t {
            let k = if w[1].t - w[0].t > 1e-9 { (t - w[0].t) / (w[1].t - w[0].t) } else { 1.0 };
            let k = k * k * (3.0 - 2.0 * k);
            return w[0].w + (w[1].w - w[0].w) * k;
        }
    }
    profile.last().unwrap().w
}

/// Grid of colours for a mesh gradient (bounding-box units, row-major,
/// `cols` × `rows` points). Each cell blends its four corner colours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshGrid {
    pub cols: usize,
    pub rows: usize,
    pub points: Vec<[f64; 2]>,
    pub colors: Vec<Rgba>,
}

impl MeshGrid {
    /// Regular grid; colours from `f(column, row)` in 0..1.
    pub fn new(cols: usize, rows: usize, f: impl Fn(f64, f64) -> Rgba) -> Self {
        let (cols, rows) = (cols.max(2), rows.max(2));
        let mut points = Vec::new();
        let mut colors = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                let (u, v) = (c as f64 / (cols - 1) as f64, r as f64 / (rows - 1) as f64);
                points.push([u, v]);
                colors.push(f(u, v));
            }
        }
        MeshGrid { cols, rows, points, colors }
    }

    fn at(&self, c: usize, r: usize) -> ([f64; 2], Rgba) {
        let i = r * self.cols + c;
        (self.points.get(i).copied().unwrap_or([0.0, 0.0]), self.colors.get(i).copied().unwrap_or(Rgba::BLACK))
    }

    fn point(&self, c: usize, r: usize) -> [f64; 2] {
        self.at(c.min(self.cols - 1), r.min(self.rows - 1)).0
    }

    /// Position at grid coordinates (`gs` columns, `gt` rows): bilinear in
    /// the cell (cells have straight edges, as drawn on the canvas).
    pub fn pos_at(&self, gs: f64, gt: f64) -> [f64; 2] {
        let (c, r) = ((gs.floor().max(0.0) as usize).min(self.cols.saturating_sub(2)), (gt.floor().max(0.0) as usize).min(self.rows.saturating_sub(2)));
        let (s, t) = (gs - c as f64, gt - r as f64);
        let (a, b, d, e) = (self.point(c, r), self.point(c + 1, r), self.point(c, r + 1), self.point(c + 1, r + 1));
        let lerp = |x: [f64; 2], y: [f64; 2], k: f64| [x[0] + (y[0] - x[0]) * k, x[1] + (y[1] - x[1]) * k];
        lerp(lerp(a, b, s), lerp(d, e, s), t)
    }

    /// Colour at grid coordinates: Catmull-Rom across the whole grid, so the
    /// colour flows smoothly over cell borders.
    pub fn color_at_grid(&self, gs: f64, gt: f64) -> Rgba {
        fn cr(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
            let t2 = t * t;
            0.5 * ((2.0 * p1) + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t2 * t)
        }
        let (cols, rows) = (self.cols as i64, self.rows as i64);
        let (c, r) = (gs.floor().clamp(0.0, (cols - 2).max(0) as f64) as i64, gt.floor().clamp(0.0, (rows - 2).max(0) as f64) as i64);
        let (s, t) = ((gs - c as f64).clamp(0.0, 1.0) as f32, (gt - r as f64).clamp(0.0, 1.0) as f32);
        let col = |cc: i64, rr: i64| self.at(cc.clamp(0, cols - 1) as usize, rr.clamp(0, rows - 1) as usize).1;
        let mut out = [0.0f32; 4];
        for (k, o) in out.iter_mut().enumerate() {
            let ch = |c: Rgba| [c.r, c.g, c.b, c.a][k];
            let row = |rr: i64| cr(ch(col(c - 1, rr)), ch(col(c, rr)), ch(col(c + 1, rr)), ch(col(c + 2, rr)), s);
            *o = cr(row(r - 1), row(r), row(r + 1), row(r + 2), t).clamp(0.0, 1.0);
        }
        Rgba { r: out[0], g: out[1], b: out[2], a: out[3] }
    }

    /// The mesh drawn into an `n` × `n` image of the unit box (row-major):
    /// each cell is cut into small quads painted with interpolated colours,
    /// later cells over earlier ones where the mesh folds; pixels outside
    /// the mesh take the nearest painted colour. Cached by content.
    pub fn raster(&self, n: usize) -> std::sync::Arc<Vec<Rgba>> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (n, self.cols, self.rows).hash(&mut h);
        for p in &self.points {
            p[0].to_bits().hash(&mut h);
            p[1].to_bits().hash(&mut h);
        }
        for c in &self.colors {
            [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut h);
        }
        let key = h.finish();
        thread_local! {
            static CACHE: std::cell::RefCell<Vec<(u64, std::sync::Arc<Vec<Rgba>>)>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        if let Some(hit) = CACHE.with(|c| c.borrow().iter().find(|e| e.0 == key).map(|e| e.1.clone())) {
            return hit;
        }
        let img = std::sync::Arc::new(self.rasterize(n));
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            c.push((key, img.clone()));
            if c.len() > 8 {
                c.remove(0);
            }
        });
        img
    }

    fn rasterize(&self, n: usize) -> Vec<Rgba> {
        let n = n.max(2);
        let mut px = vec![Rgba::BLACK.with_alpha(0.0); n * n];
        let mut hit = vec![false; n * n];
        if self.cols < 2 || self.rows < 2 {
            return vec![self.colors.first().copied().unwrap_or(Rgba::BLACK); n * n];
        }
        const SUB: usize = 12;
        let nf = n as f64;
        for r in 0..self.rows - 1 {
            for c in 0..self.cols - 1 {
                // Vertices of the sub-grid (pixel space) with their colours.
                let mut vs = Vec::with_capacity((SUB + 1) * (SUB + 1));
                for j in 0..=SUB {
                    for i in 0..=SUB {
                        let (gs, gt) = (c as f64 + i as f64 / SUB as f64, r as f64 + j as f64 / SUB as f64);
                        let p = self.pos_at(gs, gt);
                        vs.push(([p[0] * nf - 0.5, p[1] * nf - 0.5], self.color_at_grid(gs, gt)));
                    }
                }
                let v = |i: usize, j: usize| vs[j * (SUB + 1) + i];
                for j in 0..SUB {
                    for i in 0..SUB {
                        let (a, b, d, e) = (v(i, j), v(i + 1, j), v(i, j + 1), v(i + 1, j + 1));
                        raster_tri(&mut px, &mut hit, n, a, b, e);
                        raster_tri(&mut px, &mut hit, n, a, e, d);
                    }
                }
            }
        }
        // Outside the mesh: spread the nearest painted colour outwards.
        let mut queue: std::collections::VecDeque<usize> = (0..n * n).filter(|i| hit[*i]).collect();
        while let Some(i) = queue.pop_front() {
            let (x, y) = (i % n, i / n);
            for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= n as i64 || ny >= n as i64 {
                    continue;
                }
                let k = ny as usize * n + nx as usize;
                if !hit[k] {
                    hit[k] = true;
                    px[k] = px[i];
                    queue.push_back(k);
                }
            }
        }
        px
    }

    /// Colour at a point in bounding-box units (sampled from the raster).
    /// For many lookups, take `raster` once and use `sample`.
    pub fn color_at(&self, u: [f64; 2]) -> Rgba {
        Self::sample(&self.raster(Self::RASTER), Self::RASTER, u)
    }

    /// Raster size used for previews and sampling.
    pub const RASTER: usize = 256;

    /// Bilinear sample of an `n` × `n` raster at unit-box point `u`.
    pub fn sample(img: &[Rgba], n: usize, u: [f64; 2]) -> Rgba {
        let fx = (u[0] * n as f64 - 0.5).clamp(0.0, (n - 1) as f64);
        let fy = (u[1] * n as f64 - 0.5).clamp(0.0, (n - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let at = |x: usize, y: usize| img[y * n + x];
        at(x0, y0).lerp(at(x1, y0), tx).lerp(at(x0, y1).lerp(at(x1, y1), tx), ty)
    }

    /// Grid coordinates of a unit-box point: the cell containing it (the
    /// last one where the mesh folds), else None.
    pub fn grid_at(&self, u: [f64; 2]) -> Option<(f64, f64)> {
        let mut found = None;
        for r in 0..self.rows.saturating_sub(1) {
            for c in 0..self.cols.saturating_sub(1) {
                let (s, t) = inv_bilinear(u, self.point(c, r), self.point(c + 1, r), self.point(c + 1, r + 1), self.point(c, r + 1));
                if (-1e-6..=1.0 + 1e-6).contains(&s) && (-1e-6..=1.0 + 1e-6).contains(&t) {
                    found = Some((c as f64 + s.clamp(0.0, 1.0), r as f64 + t.clamp(0.0, 1.0)));
                }
            }
        }
        found
    }

    /// Add a column of points through grid column `gs` (between two
    /// existing ones), keeping the look.
    pub fn insert_column(&mut self, gs: f64) -> bool {
        let c = gs.floor() as usize;
        if gs - c as f64 <= 1e-3 || c + 1 >= self.cols {
            return false;
        }
        let mut points = Vec::with_capacity((self.cols + 1) * self.rows);
        let mut colors = Vec::with_capacity(points.capacity());
        for r in 0..self.rows {
            for cc in 0..self.cols {
                points.push(self.point(cc, r));
                colors.push(self.at(cc, r).1);
                if cc == c {
                    points.push(self.pos_at(gs, r as f64));
                    colors.push(self.color_at_grid(gs, r as f64));
                }
            }
        }
        self.points = points;
        self.colors = colors;
        self.cols += 1;
        true
    }

    /// Add a row of points through grid row `gt`.
    pub fn insert_row(&mut self, gt: f64) -> bool {
        let r = gt.floor() as usize;
        if gt - r as f64 <= 1e-3 || r + 1 >= self.rows {
            return false;
        }
        let at = (r + 1) * self.cols;
        let new_pts: Vec<[f64; 2]> = (0..self.cols).map(|c| self.pos_at(c as f64, gt)).collect();
        let new_cols: Vec<Rgba> = (0..self.cols).map(|c| self.color_at_grid(c as f64, gt)).collect();
        self.points.splice(at..at, new_pts);
        self.colors.splice(at..at, new_cols);
        self.rows += 1;
        true
    }
}

/// Gouraud-shade triangle (pixel-space vertices with colours) into `px`.
fn raster_tri(px: &mut [Rgba], hit: &mut [bool], n: usize, a: ([f64; 2], Rgba), b: ([f64; 2], Rgba), c: ([f64; 2], Rgba)) {
    let (pa, pb, pc) = (a.0, b.0, c.0);
    let area = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0]);
    if area.abs() < 1e-12 {
        return;
    }
    let x0 = pa[0].min(pb[0]).min(pc[0]).floor().max(0.0) as i64;
    let x1 = pa[0].max(pb[0]).max(pc[0]).ceil().min(n as f64 - 1.0) as i64;
    let y0 = pa[1].min(pb[1]).min(pc[1]).floor().max(0.0) as i64;
    let y1 = pa[1].max(pb[1]).max(pc[1]).ceil().min(n as f64 - 1.0) as i64;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (fx, fy) = (x as f64, y as f64);
            let w0 = ((pb[0] - fx) * (pc[1] - fy) - (pb[1] - fy) * (pc[0] - fx)) / area;
            let w1 = ((pc[0] - fx) * (pa[1] - fy) - (pc[1] - fy) * (pa[0] - fx)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < -1e-9 || w1 < -1e-9 || w2 < -1e-9 {
                continue;
            }
            let (w0, w1, w2) = (w0 as f32, w1 as f32, w2 as f32);
            let mix = |k: fn(&Rgba) -> f32| k(&a.1) * w0 + k(&b.1) * w1 + k(&c.1) * w2;
            let i = y as usize * n + x as usize;
            px[i] = Rgba { r: mix(|c| c.r), g: mix(|c| c.g), b: mix(|c| c.b), a: mix(|c| c.a) };
            hit[i] = true;
        }
    }
}

/// (s, t) of point `p` in the quad a (0,0), b (1,0), c (1,1), d (0,1).
fn inv_bilinear(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> (f64, f64) {
    let sub = |x: [f64; 2], y: [f64; 2]| [x[0] - y[0], x[1] - y[1]];
    let cross = |x: [f64; 2], y: [f64; 2]| x[0] * y[1] - x[1] * y[0];
    let e = sub(b, a);
    let f = sub(d, a);
    let g = [a[0] - b[0] + c[0] - d[0], a[1] - b[1] + c[1] - d[1]];
    let h = sub(p, a);
    let k2 = cross(g, f);
    let k1 = cross(e, f) + cross(h, g);
    let k0 = cross(h, e);
    let v = if k2.abs() < 1e-12 {
        if k1.abs() < 1e-12 { 0.0 } else { -k0 / k1 }
    } else {
        let disc = k1 * k1 - 4.0 * k0 * k2;
        if disc < 0.0 {
            return (-1.0, -1.0);
        }
        let w = disc.sqrt();
        let v1 = (-k1 - w) / (2.0 * k2);
        let v2 = (-k1 + w) / (2.0 * k2);
        if (0.0..=1.0).contains(&v1) { v1 } else { v2 }
    };
    let den_x = e[0] + g[0] * v;
    let den_y = e[1] + g[1] * v;
    let u = if den_x.abs() > den_y.abs() { (h[0] - f[0] * v) / den_x } else if den_y.abs() > 1e-12 { (h[1] - f[1] * v) / den_y } else { 0.0 };
    (u, v)
}

impl Paint {
    pub fn solid(&self) -> Option<Rgba> {
        match self {
            Paint::Solid(c) => Some(*c),
            _ => None,
        }
    }

    /// A single colour standing in for the paint (gradients: the average).
    pub fn representative(&self) -> Option<Rgba> {
        match self {
            Paint::Solid(c) => Some(*c),
            Paint::Gradient(g) => Some(g.average()),
            Paint::Image(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GradientKind {
    Linear,
    Radial,
    /// Colours sweep around `start`, beginning in the direction of `end`.
    Conic,
    /// Colours at the points of a grid (`Gradient::mesh`).
    Mesh,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// Position along the gradient, 0..=1.
    pub offset: f32,
    pub color: Rgba,
    /// Where the blend towards the next stop is half way, as a fraction of
    /// the gap (0.5 = even blend), like Affinity's midpoint ticks.
    #[serde(default = "half")]
    pub mid: f32,
}

fn half() -> f32 {
    0.5
}

impl GradientStop {
    pub fn new(offset: f32, color: Rgba) -> Self {
        GradientStop { offset, color, mid: 0.5 }
    }
}

/// Bias a 0..1 blend so it reaches 0.5 at `mid`.
pub fn mid_bias(k: f32, mid: f32) -> f32 {
    let m = mid.clamp(0.02, 0.98);
    if (m - 0.5).abs() < 1e-4 {
        k
    } else if k < m {
        0.5 * k / m
    } else {
        0.5 + 0.5 * (k - m) / (1.0 - m)
    }
}

/// A linear or radial gradient. `start`/`end` are in the shape's local
/// bounding-box units (0..1 spans the box): a linear gradient runs from
/// `start` to `end`; a radial one is centred on `start` with `end` on its
/// edge (an ellipse on non-square shapes, like SVG objectBoundingBox).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub kind: GradientKind,
    /// Sorted by offset.
    pub stops: Vec<GradientStop>,
    pub start: [f64; 2],
    pub end: [f64; 2],
    /// Mesh gradients: the colour grid (the stops seed it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshGrid>,
}

impl Gradient {
    /// Default: left to right from `a` to `b` (radial: centre outwards;
    /// conic: around the centre, starting to the right; mesh: a 3 × 3 grid
    /// from `a` at the top left to `b` at the bottom right).
    pub fn new(kind: GradientKind, a: Rgba, b: Rgba) -> Self {
        let (start, end) = match kind {
            GradientKind::Linear => ([0.0, 0.5], [1.0, 0.5]),
            GradientKind::Radial | GradientKind::Conic | GradientKind::Mesh => ([0.5, 0.5], [1.0, 0.5]),
        };
        let mesh = (kind == GradientKind::Mesh).then(|| MeshGrid::new(3, 3, |u, v| a.lerp(b, ((u + v) / 2.0) as f32)));
        Gradient { kind, stops: vec![GradientStop::new(0.0, a), GradientStop::new(1.0, b)], start, end, mesh }
    }

    /// Switch kind, keeping the colours (a mesh is seeded from the stops).
    pub fn set_kind(&mut self, kind: GradientKind) {
        if kind == self.kind {
            return;
        }
        if kind == GradientKind::Mesh && self.mesh.is_none() {
            let g = self.clone();
            self.mesh = Some(MeshGrid::new(3, 3, |u, v| g.color_at(((u + v) / 2.0) as f32)));
        }
        if self.kind == GradientKind::Linear && kind != GradientKind::Linear {
            (self.start, self.end) = ([0.5, 0.5], [1.0, 0.5]);
        } else if kind == GradientKind::Linear && self.kind != GradientKind::Linear {
            (self.start, self.end) = ([0.0, 0.5], [1.0, 0.5]);
        }
        self.kind = kind;
    }

    /// Colour at a point in bounding-box units.
    pub fn color_at_unit(&self, u: [f64; 2]) -> Rgba {
        match (&self.kind, &self.mesh) {
            (GradientKind::Mesh, Some(m)) => m.color_at(u),
            _ => self.color_at(self.t_at(u)),
        }
    }

    /// Kinds that SVG and PDF can't describe natively (baked to images).
    pub fn is_baked(&self) -> bool {
        matches!(self.kind, GradientKind::Conic | GradientKind::Mesh)
    }

    pub fn sort(&mut self) {
        self.stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    }

    /// Colour at gradient position `t` (clamped), interpolated in sRGB.
    pub fn color_at(&self, t: f32) -> Rgba {
        let st = &self.stops;
        let Some(first) = st.first() else { return Rgba::BLACK };
        let t = t.clamp(0.0, 1.0);
        if t <= first.offset {
            return first.color;
        }
        for w in st.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t <= b.offset {
                let k = if b.offset - a.offset > 1e-6 { (t - a.offset) / (b.offset - a.offset) } else { 1.0 };
                return a.color.lerp(b.color, mid_bias(k, a.mid));
            }
        }
        st.last().unwrap().color
    }

    /// Gradient position of a point in bounding-box units.
    pub fn t_at(&self, u: [f64; 2]) -> f32 {
        let d = [self.end[0] - self.start[0], self.end[1] - self.start[1]];
        let len2 = (d[0] * d[0] + d[1] * d[1]).max(1e-12);
        let v = [u[0] - self.start[0], u[1] - self.start[1]];
        match self.kind {
            GradientKind::Linear => ((v[0] * d[0] + v[1] * d[1]) / len2) as f32,
            GradientKind::Radial => ((v[0] * v[0] + v[1] * v[1]) / len2).sqrt() as f32,
            GradientKind::Conic | GradientKind::Mesh => {
                let a = v[1].atan2(v[0]) - d[1].atan2(d[0]);
                (a / std::f64::consts::TAU).rem_euclid(1.0) as f32
            }
        }
    }

    pub fn average(&self) -> Rgba {
        let n = 16;
        let mut acc = [0.0f32; 4];
        for i in 0..n {
            let c = self.color_at(i as f32 / (n - 1) as f32);
            for (k, v) in [c.r, c.g, c.b, c.a].into_iter().enumerate() {
                acc[k] += v / n as f32;
            }
        }
        Rgba { r: acc[0], g: acc[1], b: acc[2], a: acc[3] }
    }

    /// Stops padded to cover 0..=1 (at least two), for exporters whose
    /// shading functions need the full domain.
    pub fn padded_stops(&self) -> Vec<GradientStop> {
        let mut stops = self.linear_stops();
        let first = stops.first().copied().unwrap_or(GradientStop::new(0.0, Rgba::BLACK));
        let last = stops.last().copied().unwrap_or(first);
        if stops.is_empty() || first.offset > 0.0 {
            stops.insert(0, GradientStop { offset: 0.0, ..first });
        }
        if stops.len() == 1 || last.offset < 1.0 {
            stops.push(GradientStop { offset: 1.0, ..last });
        }
        stops
    }

    /// Stops with plain linear blends only: segments with a moved midpoint
    /// are sampled into extra stops (for SVG, PDF and EPS export).
    pub fn linear_stops(&self) -> Vec<GradientStop> {
        let mut out = Vec::with_capacity(self.stops.len());
        for (i, a) in self.stops.iter().enumerate() {
            out.push(GradientStop { mid: 0.5, ..*a });
            let Some(b) = self.stops.get(i + 1) else { continue };
            if (a.mid - 0.5).abs() < 1e-4 || b.offset - a.offset < 1e-6 {
                continue;
            }
            const N: usize = 12;
            for j in 1..N {
                let k = j as f32 / N as f32;
                out.push(GradientStop::new(a.offset + (b.offset - a.offset) * k, a.color.lerp(b.color, mid_bias(k, a.mid))));
            }
        }
        out
    }

    /// Whether any stop is (partly) transparent.
    pub fn has_alpha(&self) -> bool {
        self.stops.iter().any(|s| s.color.a < 0.999)
    }

    /// Key for caching the colour ramp (stops and kind, not geometry).
    pub fn ramp_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.kind.hash(&mut h);
        if self.kind == GradientKind::Conic {
            for v in [self.start, self.end].concat() {
                v.to_bits().hash(&mut h);
            }
        }
        if let Some(m) = &self.mesh {
            (m.cols, m.rows).hash(&mut h);
            for p in &m.points {
                p[0].to_bits().hash(&mut h);
                p[1].to_bits().hash(&mut h);
            }
            for c in &m.colors {
                for v in [c.r, c.g, c.b, c.a] {
                    v.to_bits().hash(&mut h);
                }
            }
        }
        for s in &self.stops {
            s.offset.to_bits().hash(&mut h);
            s.mid.to_bits().hash(&mut h);
            for v in [s.color.r, s.color.g, s.color.b, s.color.a] {
                v.to_bits().hash(&mut h);
            }
        }
        h.finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

/// Where the stroke sits relative to the path (closed paths only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeAlign {
    #[default]
    Center,
    Inside,
    Outside,
}

/// Decoration at the ends of open paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arrowhead {
    #[default]
    None,
    Arrow,
    Triangle,
    Circle,
    Square,
    Bar,
}

impl Arrowhead {
    pub const ALL: [Arrowhead; 6] = [Arrowhead::None, Arrowhead::Arrow, Arrowhead::Triangle, Arrowhead::Circle, Arrowhead::Square, Arrowhead::Bar];
    pub fn label(self) -> &'static str {
        match self {
            Arrowhead::None => "None",
            Arrowhead::Arrow => "Arrow",
            Arrowhead::Triangle => "Triangle",
            Arrowhead::Circle => "Circle",
            Arrowhead::Square => "Square",
            Arrowhead::Bar => "Bar",
        }
    }
}

fn default_miter() -> f64 {
    4.0
}

fn default_arrow_scale() -> f64 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub color: Rgba,
    pub width: f64,
    pub join: LineJoin,
    pub cap: LineCap,
    #[serde(default = "default_miter")]
    pub miter_limit: f64,
    #[serde(default)]
    pub align: StrokeAlign,
    /// Dash pattern as multiples of the stroke width (dash, gap, ...). Empty = solid.
    #[serde(default)]
    pub dash: Vec<f64>,
    /// Dash phase, in multiples of the stroke width.
    #[serde(default)]
    pub dash_offset: f64,
    #[serde(default)]
    pub start_arrow: Arrowhead,
    #[serde(default)]
    pub end_arrow: Arrowhead,
    /// Arrowhead size multiplier.
    #[serde(default = "default_arrow_scale")]
    pub arrow_scale: f64,
    /// Variable width along the path (empty = uniform).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile: Vec<WidthPoint>,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke {
            color: Rgba::BLACK,
            width: 1.0,
            join: LineJoin::Miter,
            cap: LineCap::Butt,
            miter_limit: 4.0,
            align: StrokeAlign::Center,
            dash: Vec::new(),
            dash_offset: 0.0,
            start_arrow: Arrowhead::None,
            end_arrow: Arrowhead::None,
            arrow_scale: 1.0,
            profile: Vec::new(),
        }
    }
}

impl Stroke {
    pub fn is_dashed(&self) -> bool {
        self.dash.iter().sum::<f64>() > 1e-6
    }

    pub fn has_arrows(&self) -> bool {
        self.start_arrow != Arrowhead::None || self.end_arrow != Arrowhead::None
    }

    /// Dash lengths in document units.
    pub fn dash_px(&self) -> Vec<f64> {
        self.dash.iter().map(|d| (d * self.width).max(1e-3)).collect()
    }

    /// Extra extent beyond the path (for culling and hit testing).
    pub fn reach(&self) -> f64 {
        let base = match self.align {
            StrokeAlign::Center => self.width / 2.0,
            StrokeAlign::Inside => 0.0,
            StrokeAlign::Outside => self.width,
        };
        let arrows = if self.has_arrows() { self.width * 3.5 * self.arrow_scale } else { 0.0 };
        let wide = self.profile.iter().map(|p| p.w).fold(1.0f64, f64::max);
        (base * wide).max(arrows)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// A Bézier anchor with optional absolute handle positions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub p: Point,
    pub hin: Option<Point>,
    pub hout: Option<Point>,
    /// Smooth nodes keep their handles collinear while editing.
    pub smooth: bool,
    /// Live corner rounding radius (only applied on sharp corners).
    #[serde(default)]
    pub corner: f64,
}

impl Node {
    pub fn corner(p: Point) -> Self {
        Node { p, hin: None, hout: None, smooth: false, corner: 0.0 }
    }

    pub fn transform(&mut self, a: Affine) {
        self.p = a * self.p;
        self.hin = self.hin.map(|h| a * h);
        self.hout = self.hout.map(|h| a * h);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubPath {
    pub nodes: Vec<Node>,
    pub closed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PathData {
    pub subpaths: Vec<SubPath>,
}

impl PathData {
    pub fn transform(&mut self, a: Affine) {
        for sp in &mut self.subpaths {
            for n in &mut sp.nodes {
                n.transform(a);
            }
        }
    }

    pub fn node_count(&self) -> usize {
        self.subpaths.iter().map(|s| s.nodes.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    /// Lines (except a paragraph's last) stretched to the full width.
    Justify,
}

/// How letters are displayed; the content keeps what was typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextCase {
    #[default]
    AsTyped,
    Upper,
    Lower,
    /// First letter of each word capitalised.
    Title,
    /// Lowercase letters as small capitals.
    SmallCaps,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextData {
    pub content: String,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub size: f64,
    /// Multiplier of the font size.
    pub line_height: f64,
    /// Extra spacing between glyphs in px.
    pub letter_spacing: f64,
    pub align: TextAlign,
    /// Frame (text box) size: text wraps to the width and is clipped to the
    /// height. `None` = artistic text that grows with its content.
    #[serde(default)]
    pub frame: Option<[f64; 2]>,
    /// Text on a path (local coordinates); glyphs follow its first subpath.
    #[serde(default)]
    pub path: Option<PathData>,
    /// Start offset along the path in px.
    #[serde(default)]
    pub path_offset: f64,
    #[serde(default, skip_serializing_if = "is_as_typed")]
    pub case: TextCase,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strike: bool,
    /// OpenType feature settings (on top of the font's defaults).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<FontFeature>,
    /// Variable-font axis positions (`wght` follows `weight` unless set).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variations: Vec<FontAxis>,
    /// Character-style overrides over byte ranges of `content` (sorted,
    /// non-overlapping).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<TextSpan>,
    /// Linked paragraph style (its attributes are copied into these fields).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_id: Option<u64>,
    /// Extra space after each paragraph, in px.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub paragraph_spacing: f64,
    /// Frame text: number of columns (0 and 1 = one) and the gap between them.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub columns: u32,
    #[serde(default = "default_column_gap")]
    pub column_gap: f64,
    /// Frame text: the frame the overflow continues in (a thread).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
    /// Hyphenate words at line ends (frame text).
    #[serde(default, skip_serializing_if = "is_false")]
    pub hyphenate: bool,
    /// Language (BCP 47, e.g. `en-us`, `pl`) for hyphenation and spelling;
    /// `None` = the app default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Derived: the story this frame continues (it is a later frame of a
    /// thread); its own content is unused. Set by the text-flow pass.
    #[serde(skip)]
    pub flow: Option<FlowIn>,
    /// Derived: outlines (local coordinates) of shapes the text wraps around.
    #[serde(skip)]
    pub exclusions: Option<std::sync::Arc<Vec<kurbo::BezPath>>>,
}

fn is_as_typed(c: &TextCase) -> bool {
    *c == TextCase::AsTyped
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn default_column_gap() -> f64 {
    16.0
}

impl Default for TextData {
    fn default() -> Self {
        TextData {
            content: String::new(),
            family: "Noto Sans".into(),
            weight: 400,
            italic: false,
            size: 32.0,
            line_height: 1.2,
            letter_spacing: 0.0,
            align: TextAlign::Left,
            frame: None,
            path: None,
            path_offset: 0.0,
            case: TextCase::AsTyped,
            underline: false,
            strike: false,
            features: Vec::new(),
            variations: Vec::new(),
            spans: Vec::new(),
            style_id: None,
            paragraph_spacing: 0.0,
            columns: 0,
            column_gap: default_column_gap(),
            next: None,
            hyphenate: false,
            lang: None,
            flow: None,
            exclusions: None,
        }
    }
}

/// An OpenType feature setting, e.g. `liga` = 0 turns standard ligatures off,
/// `ss01` = 1 turns a stylistic set on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontFeature {
    pub tag: String,
    pub value: u32,
}

/// A variable-font axis position, e.g. `wdth` = 75.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontAxis {
    pub tag: String,
    pub value: f32,
}

/// Character attributes; `None` inherits from the text object.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Text colour (None: the shape's fill).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<Rgba>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strike: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case: Option<TextCase>,
    /// Baseline shift in px (positive raises).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<FontFeature>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variations: Option<Vec<FontAxis>>,
}

impl CharStyle {
    pub fn is_empty(&self) -> bool {
        *self == CharStyle::default()
    }

    /// `other`'s set attributes on top of these.
    pub fn merge(&mut self, other: &CharStyle) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f.clone(); } )* };
        }
        take!(family, weight, italic, size, fill, letter_spacing, underline, strike, case, baseline, features, variations);
    }

    /// The attributes that differ between two resolved styles (as overrides).
    pub fn diff(before: &RunStyle, after: &RunStyle) -> CharStyle {
        let mut d = CharStyle::default();
        macro_rules! cmp {
            ($($f:ident),*) => { $( if before.$f != after.$f { d.$f = Some(after.$f.clone()); } )* };
        }
        cmp!(family, weight, italic, size, letter_spacing, underline, strike, case, baseline, features, variations);
        if before.fill != after.fill {
            d.fill = after.fill;
        }
        d
    }

    /// Drop the attributes `mask` sets (they now come from the object).
    pub fn clear(&mut self, mask: &CharStyle) {
        macro_rules! clr {
            ($($f:ident),*) => { $( if mask.$f.is_some() { self.$f = None; } )* };
        }
        clr!(family, weight, italic, size, fill, letter_spacing, underline, strike, case, baseline, features, variations);
    }
}

/// Character style over `start..end` (byte offsets into the content).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
    #[serde(flatten)]
    pub style: CharStyle,
    /// Linked character style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_id: Option<u64>,
}

/// Fully resolved attributes of a run of text.
#[derive(Clone, Debug, PartialEq)]
pub struct RunStyle {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub size: f64,
    pub fill: Option<Rgba>,
    pub letter_spacing: f64,
    pub underline: bool,
    pub strike: bool,
    pub case: TextCase,
    pub baseline: f64,
    pub features: Vec<FontFeature>,
    pub variations: Vec<FontAxis>,
}

impl RunStyle {
    fn apply(&mut self, c: &CharStyle) {
        macro_rules! set {
            ($($f:ident),*) => { $( if let Some(v) = &c.$f { self.$f = v.clone(); } )* };
        }
        set!(family, weight, italic, size, letter_spacing, underline, strike, case, baseline, features, variations);
        if c.fill.is_some() {
            self.fill = c.fill;
        }
    }

    /// The same attributes as a full override set.
    pub fn to_char_style(&self) -> CharStyle {
        CharStyle {
            family: Some(self.family.clone()),
            weight: Some(self.weight),
            italic: Some(self.italic),
            size: Some(self.size),
            fill: self.fill,
            letter_spacing: Some(self.letter_spacing),
            underline: Some(self.underline),
            strike: Some(self.strike),
            case: Some(self.case),
            baseline: Some(self.baseline),
            features: Some(self.features.clone()),
            variations: Some(self.variations.clone()),
        }
    }
}

/// The story a later frame of a thread shows, from byte `start`.
#[derive(Clone, Debug)]
pub struct FlowIn {
    pub head: u64,
    pub head_rev: u64,
    pub start: usize,
    pub story: std::sync::Arc<TextData>,
}

impl PartialEq for FlowIn {
    fn eq(&self, o: &Self) -> bool {
        self.head == o.head && self.head_rev == o.head_rev && self.start == o.start
    }
}

impl TextData {
    /// The object-level attributes.
    pub fn base_style(&self) -> RunStyle {
        RunStyle {
            family: self.family.clone(),
            weight: self.weight,
            italic: self.italic,
            size: self.size,
            fill: None,
            letter_spacing: self.letter_spacing,
            underline: self.underline,
            strike: self.strike,
            case: self.case,
            baseline: 0.0,
            features: self.features.clone(),
            variations: self.variations.clone(),
        }
    }

    /// Attributes of the character at byte `i` (the one before it at the end).
    pub fn style_at(&self, i: usize) -> RunStyle {
        let mut st = self.base_style();
        let i = if i >= self.content.len() && i > 0 { i - 1 } else { i };
        if let Some(sp) = self.spans.iter().find(|sp| sp.start <= i && i < sp.end) {
            st.apply(&sp.style);
        }
        st
    }

    /// Resolved style runs covering `range` (in order, no gaps).
    pub fn runs(&self, range: std::ops::Range<usize>) -> Vec<(std::ops::Range<usize>, RunStyle)> {
        let base = self.base_style();
        let mut out: Vec<(std::ops::Range<usize>, RunStyle)> = Vec::new();
        let mut at = range.start;
        let mut push = |r: std::ops::Range<usize>, st: RunStyle| {
            if r.start >= r.end {
                return;
            }
            match out.last_mut() {
                Some((pr, ps)) if *ps == st && pr.end == r.start => pr.end = r.end,
                _ => out.push((r, st)),
            }
        };
        for sp in &self.spans {
            if sp.end <= at || sp.start >= range.end {
                continue;
            }
            let (s, e) = (sp.start.max(at), sp.end.min(range.end));
            push(at..s, base.clone());
            let mut st = base.clone();
            st.apply(&sp.style);
            push(s..e, st);
            at = e;
        }
        push(at..range.end, base);
        out
    }

    /// Replace bytes `range` with `text`, keeping spans in place. Inserted
    /// text takes the style of the character before it.
    pub fn edit(&mut self, range: std::ops::Range<usize>, text: &str) {
        let (a, b) = (range.start.min(self.content.len()), range.end.min(self.content.len()));
        self.content.replace_range(a..b, text);
        let (removed, added) = (b - a, text.len());
        let shift = |x: usize| x - removed + added;
        for sp in &mut self.spans {
            let (s0, e0) = (sp.start, sp.end);
            // Typed text takes the style of the character before it (at the
            // very start: of the first character).
            sp.start = if s0 < a || (a == 0 && s0 == 0) {
                s0
            } else if s0 >= b {
                shift(s0)
            } else {
                a + added
            };
            sp.end = if e0 < a {
                e0
            } else if e0 == a {
                if s0 < a { a + added } else { e0 }
            } else if e0 > b {
                shift(e0)
            } else {
                a + added
            };
        }
        self.normalize_spans();
    }

    /// Set attributes over `range` (as overrides; `f` edits each piece).
    pub fn apply(&mut self, range: std::ops::Range<usize>, f: impl Fn(&mut CharStyle, &mut Option<u64>)) {
        let (a, b) = (range.start.min(self.content.len()), range.end.min(self.content.len()));
        if a >= b {
            return;
        }
        // Split spans at the edges, fill gaps, then edit the covered pieces.
        let mut pieces: Vec<TextSpan> = Vec::new();
        let mut at = 0;
        let len = self.content.len();
        for sp in std::mem::take(&mut self.spans) {
            if sp.start > at {
                pieces.push(TextSpan { start: at, end: sp.start, style: CharStyle::default(), style_id: None });
            }
            at = sp.end;
            pieces.push(sp);
        }
        if at < len {
            pieces.push(TextSpan { start: at, end: len, style: CharStyle::default(), style_id: None });
        }
        let mut out = Vec::new();
        for p in pieces {
            for (s, e) in [(p.start, p.end.min(a).max(p.start)), (p.start.max(a), p.end.min(b)), (p.start.max(b).min(p.end), p.end)] {
                if s >= e {
                    continue;
                }
                let mut q = TextSpan { start: s, end: e, style: p.style.clone(), style_id: p.style_id };
                if s >= a && e <= b {
                    f(&mut q.style, &mut q.style_id);
                }
                out.push(q);
            }
        }
        out.sort_by_key(|s| s.start);
        self.spans = out;
        self.normalize_spans();
    }

    /// Drop empty and attribute-free spans, merge equal neighbours, clamp
    /// to the content.
    pub fn normalize_spans(&mut self) {
        let len = self.content.len();
        let mut out: Vec<TextSpan> = Vec::new();
        let mut spans = std::mem::take(&mut self.spans);
        spans.sort_by_key(|s| s.start);
        for mut sp in spans {
            sp.end = sp.end.min(len);
            if let Some(prev) = out.last() {
                sp.start = sp.start.max(prev.end);
            }
            // Keep char boundaries.
            while sp.start < len && !self.content.is_char_boundary(sp.start) {
                sp.start += 1;
            }
            while sp.end < len && !self.content.is_char_boundary(sp.end) {
                sp.end += 1;
            }
            if sp.start >= sp.end || (sp.style.is_empty() && sp.style_id.is_none()) {
                continue;
            }
            match out.last_mut() {
                Some(p) if p.end == sp.start && p.style == sp.style && p.style_id == sp.style_id => p.end = sp.end,
                _ => out.push(sp),
            }
        }
        self.spans = out;
    }

    /// Set object attributes `mask` (from a whole-object edit): spans no
    /// longer override them.
    pub fn clear_overrides(&mut self, mask: &CharStyle) {
        for sp in &mut self.spans {
            sp.style.clear(mask);
        }
        self.normalize_spans();
    }

    /// The text this object lays out and where in it this frame starts.
    pub fn story(&self) -> (&TextData, usize) {
        match &self.flow {
            Some(f) => (&f.story, f.start),
            None => (self, 0),
        }
    }
}

/// A named paragraph or character style.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub id: u64,
    pub name: String,
    /// Paragraph styles apply to whole text objects; character styles to ranges.
    pub paragraph: bool,
    #[serde(default)]
    pub style: CharStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paragraph_spacing: Option<f64>,
}

impl TextStyle {
    /// Copy a paragraph style's attributes into a text object.
    pub fn apply_to(&self, td: &mut TextData) {
        let c = &self.style;
        if let Some(v) = &c.family {
            td.family = v.clone();
        }
        if let Some(v) = c.weight {
            td.weight = v;
        }
        if let Some(v) = c.italic {
            td.italic = v;
        }
        if let Some(v) = c.size {
            td.size = v;
        }
        if let Some(v) = c.letter_spacing {
            td.letter_spacing = v;
        }
        if let Some(v) = c.underline {
            td.underline = v;
        }
        if let Some(v) = c.strike {
            td.strike = v;
        }
        if let Some(v) = c.case {
            td.case = v;
        }
        if let Some(v) = &c.features {
            td.features = v.clone();
        }
        if let Some(v) = &c.variations {
            td.variations = v.clone();
        }
        if let Some(v) = self.line_height {
            td.line_height = v;
        }
        if let Some(v) = self.align {
            td.align = v;
        }
        if let Some(v) = self.paragraph_spacing {
            td.paragraph_spacing = v;
        }
    }

    /// A paragraph style capturing a text object's attributes.
    pub fn from_text(id: u64, name: String, td: &TextData) -> Self {
        let mut style = td.base_style().to_char_style();
        style.fill = None;
        style.baseline = None;
        TextStyle { id, name, paragraph: true, style, line_height: Some(td.line_height), align: Some(td.align), paragraph_spacing: Some(td.paragraph_spacing) }
    }
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ShapeKind {
    /// Rectangle at local (0,0)-(w,h). Radii: top-left, top-right, bottom-right, bottom-left.
    Rect { w: f64, h: f64, radii: [f64; 4] },
    /// Ellipse inscribed in local (0,0)-(w,h).
    Ellipse { w: f64, h: f64 },
    /// Star inscribed in local (0,0)-(w,h). `inner` is the inner radius ratio.
    Star { w: f64, h: f64, points: u32, inner: f64, corner: f64 },
    /// Regular polygon inscribed in local (0,0)-(w,h).
    Polygon { w: f64, h: f64, sides: u32, corner: f64 },
    /// Parametric primitive (triangle, arrow, donut, cog…) in local (0,0)-(w,h).
    Prim { w: f64, h: f64, prim: crate::prims::Prim },
    /// Free-form Bézier path (always kept in document coordinates).
    Path(PathData),
    Text(TextData),
    /// Non-destructive boolean: the outline is `op` folded over the children
    /// (document coordinates); the compound's own style is used.
    Compound { op: crate::boolean::BoolOp, children: Vec<Shape> },
    /// Plain group: children keep their own styles and are drawn in order.
    Group { children: Vec<Shape> },
}

impl ShapeKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            ShapeKind::Rect { .. } => "Rectangle",
            ShapeKind::Ellipse { .. } => "Ellipse",
            ShapeKind::Star { .. } => "Star",
            ShapeKind::Polygon { .. } => "Polygon",
            ShapeKind::Prim { prim, .. } => prim.kind.name(),
            ShapeKind::Path(_) => "Path",
            ShapeKind::Text(_) => "Text",
            ShapeKind::Compound { .. } => "Compound",
            ShapeKind::Group { .. } => "Group",
        }
    }

    /// Children of a container (boolean or group).
    pub fn children(&self) -> Option<&Vec<Shape>> {
        match self {
            ShapeKind::Compound { children, .. } | ShapeKind::Group { children } => Some(children),
            _ => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<Shape>> {
        match self {
            ShapeKind::Compound { children, .. } | ShapeKind::Group { children } => Some(children),
            _ => None,
        }
    }

    pub fn is_container(&self) -> bool {
        self.children().is_some()
    }

    pub fn size(&self) -> Option<(f64, f64)> {
        match *self {
            ShapeKind::Rect { w, h, .. }
            | ShapeKind::Ellipse { w, h }
            | ShapeKind::Star { w, h, .. }
            | ShapeKind::Polygon { w, h, .. }
            | ShapeKind::Prim { w, h, .. } => Some((w, h)),
            _ => None,
        }
    }

    pub fn set_size(&mut self, nw: f64, nh: f64) {
        match self {
            ShapeKind::Rect { w, h, .. }
            | ShapeKind::Ellipse { w, h }
            | ShapeKind::Star { w, h, .. }
            | ShapeKind::Polygon { w, h, .. }
            | ShapeKind::Prim { w, h, .. } => {
                *w = nw;
                *h = nh;
            }
            _ => {}
        }
    }
}

/// Soft drop shadow painted under a shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub color: Rgba,
    pub dx: f64,
    pub dy: f64,
    /// Blur radius in document units (Gaussian sigma = radius / 2).
    pub blur: f64,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow { color: Rgba::BLACK.with_alpha(0.35), dx: 4.0, dy: 6.0, blur: 12.0 }
    }
}

impl Shadow {
    /// Starting values for a new inner shadow.
    pub fn inner() -> Self {
        Shadow { color: Rgba::BLACK.with_alpha(0.25), dx: 0.0, dy: 4.0, blur: 8.0 }
    }

    pub fn sigma(&self) -> f64 {
        (self.blur / 2.0).max(0.0)
    }

    /// How far the shadow can extend beyond the shape.
    pub fn reach(&self) -> f64 {
        self.sigma() * 3.0 + self.dx.abs().max(self.dy.abs())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u64,
    /// Revision stamp; changes whenever the shape is mutated (cache key).
    #[serde(skip)]
    pub rev: u64,
    pub name: String,
    pub kind: ShapeKind,
    /// Local → document transform (identity for paths, which are baked).
    pub transform: Affine,
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
    pub opacity: f32,
    pub fill_rule: FillRule,
    pub visible: bool,
    pub locked: bool,
    #[serde(default)]
    pub shadow: Option<Shadow>,
    /// Shadow cast inwards, over the fill and stroke (clipped to them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_shadow: Option<Shadow>,
    /// Layer blur radius in document units (0 = off).
    #[serde(default)]
    pub blur: f64,
    /// Noise overlay strength 0..1 on fill and stroke.
    #[serde(default)]
    pub noise: f32,
    /// Outline rings drawn outside the shape, innermost first.
    #[serde(default)]
    pub outlines: Vec<OutlineStep>,
    /// Clip mask: only the part inside this shape is visible.
    #[serde(default)]
    pub clip: Option<Box<Shape>>,
    /// Main component or instance (groups only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<Component>,
    /// Colour key the solid fill follows (see [`ColorKey`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_key: Option<u64>,
    /// Colour key the stroke colour follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_key: Option<u64>,
    /// Frame text below this shape flows around it, keeping this distance (px).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_wrap: Option<f64>,
    #[serde(default, skip_serializing_if = "BlendMode::is_normal")]
    pub blend: BlendMode,
    /// Luminance or opacity mask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<Mask>>,
    /// More fills, painted over `fill` in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fills: Vec<Paint>,
    /// More strokes, painted under `stroke` in order (the first lowest).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<Stroke>,
    /// Groups: stack the members in a row or column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<crate::layout::AutoLayout>,
    /// How this member follows when its group is resized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<crate::layout::Constraints>,
    /// Export settings (slices): scale, format and file-name suffix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<ExportSlice>,
    /// Groups: a non-destructive warp or blend; the children are its
    /// result (see `paint_ops::LiveEffect`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<crate::paint_ops::LiveEffect>,
}

/// One export of a layer or artboard ("@2x PNG").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportSlice {
    pub scale: f64,
    pub format: crate::io::export::Format,
    /// Added to the file name ("@2x").
    #[serde(default)]
    pub suffix: String,
}

impl ExportSlice {
    pub fn new(scale: f64, format: crate::io::export::Format) -> Self {
        let suffix = if format.is_raster() && scale != 1.0 { format!("@{}x", crate::io::num(scale)) } else { String::new() };
        ExportSlice { scale, format, suffix }
    }
}

/// A named document colour, like Figma's colour variables: fills and
/// strokes linked to it change with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorKey {
    pub id: u64,
    pub name: String,
    pub color: Rgba,
    /// A spot colour: PDF export prints it as its own ink (a Separation
    /// named after the key) instead of mixing it from process colours.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spot: bool,
    /// Exact CMYK ink values for print (else converted from `color`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmyk: Option<[f32; 4]>,
}

/// A group's role in a component (like Figma's): the main component holds
/// the source artwork; instances mirror its children, keeping per-layer
/// style overrides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Component {
    /// `frame`: the main's placement, a translation to its top-left corner
    /// (follows moves of the whole main, so moving it doesn't move its
    /// instances; its size and rotation carry over to them).
    /// `set` groups variants of one component; `props` are this variant's
    /// property values ("State" = "Hover"). `library`: imported from a
    /// shared library (updated from it).
    Main {
        frame: Affine,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        set: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        props: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<LibraryLink>,
    },
    Instance(InstanceLink),
}

impl Component {
    pub fn main(frame: Affine) -> Self {
        Component::Main { frame, set: None, props: BTreeMap::new(), library: None }
    }
}

/// Where an imported main component comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryLink {
    /// The library document.
    pub path: std::path::PathBuf,
    /// The main's id in the library.
    pub source: u64,
    /// Library layer id → local layer id (stable across updates, so
    /// instance overrides survive them).
    #[serde(default)]
    pub ids: BTreeMap<u64, u64>,
    /// Content hash of the library version imported.
    #[serde(default)]
    pub key: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstanceLink {
    /// Shape id of the main component.
    pub main: u64,
    /// Main's frame → this instance's placement.
    pub xf: Affine,
    /// Main layer id → id of the mirrored layer in this instance.
    #[serde(default)]
    pub ids: BTreeMap<u64, u64>,
    /// Restyled layers, by main layer id.
    #[serde(default)]
    pub overrides: BTreeMap<u64, Override>,
}

/// Properties an instance's layer sets for itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Override {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Option<Paint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Option<Stroke>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Text content and style (the frame size follows the main).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextData>,
}

impl Override {
    pub fn is_empty(&self) -> bool {
        *self == Override::default()
    }

    pub fn apply(&self, s: &mut Shape) {
        if let Some(f) = &self.fill {
            s.fill = f.clone();
        }
        if let Some(st) = &self.stroke {
            s.stroke = st.clone();
        }
        if let Some(o) = self.opacity {
            s.opacity = o;
        }
        if let Some(v) = self.visible {
            s.visible = v;
        }
        if let Some(n) = &self.name {
            s.name = n.clone();
        }
        if let (Some(t), ShapeKind::Text(td)) = (&self.text, &mut s.kind) {
            let frame = td.frame;
            *td = t.clone();
            td.frame = frame;
        }
    }

    /// Record how `actual` differs from `expected` (the main's version).
    pub fn record(&mut self, expected: &Shape, actual: &Shape) {
        if expected.fill != actual.fill {
            self.fill = Some(actual.fill.clone());
        }
        if expected.stroke != actual.stroke {
            self.stroke = Some(actual.stroke.clone());
        }
        if expected.opacity != actual.opacity {
            self.opacity = Some(actual.opacity);
        }
        if expected.visible != actual.visible {
            self.visible = Some(actual.visible);
        }
        if expected.name != actual.name {
            self.name = Some(actual.name.clone());
        }
        if let (ShapeKind::Text(e), ShapeKind::Text(a)) = (&expected.kind, &actual.kind) {
            let mut e = e.clone();
            e.frame = a.frame;
            if e != *a {
                self.text = Some(a.clone());
            }
        }
    }
}

/// One ring of a multi-step outline effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutlineStep {
    pub color: Rgba,
    pub width: f64,
    /// Gradient paint (in the ring's own bounding-box units); `color` is
    /// used when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<Gradient>,
}

impl OutlineStep {
    pub fn new(color: Rgba, width: f64) -> Self {
        OutlineStep { color, width, gradient: None }
    }

    pub fn paint(&self) -> Paint {
        match &self.gradient {
            Some(g) => Paint::Gradient(g.clone()),
            None => Paint::Solid(self.color),
        }
    }
}

impl Shape {
    pub fn new(kind: ShapeKind) -> Self {
        Shape {
            id: 0,
            rev: 0,
            name: kind.type_name().to_string(),
            kind,
            transform: Affine::IDENTITY,
            fill: Some(Paint::Solid(Rgba::from_hex("#D9D9D9").unwrap())),
            stroke: None,
            opacity: 1.0,
            fill_rule: FillRule::NonZero,
            visible: true,
            locked: false,
            shadow: None,
            inner_shadow: None,
            blur: 0.0,
            noise: 0.0,
            outlines: Vec::new(),
            clip: None,
            component: None,
            fill_key: None,
            stroke_key: None,
            text_wrap: None,
            blend: BlendMode::Normal,
            mask: None,
            fills: Vec::new(),
            strokes: Vec::new(),
            layout: None,
            constraints: None,
            exports: Vec::new(),
            live: None,
        }
    }

    /// Apply a document-space affine transform to this shape. Paths are baked;
    /// primitives keep sizes in their parameters where possible so that corner
    /// radii and stroke widths stay crisp.
    pub fn apply_transform(&mut self, a: Affine) {
        if let Some(c) = &mut self.clip {
            c.apply_transform(a);
        }
        if let Some(m) = &mut self.mask {
            m.shape.apply_transform(a);
        }
        match &mut self.component {
            // The main's frame only follows its position: resizing or
            // rotating the main resizes / rotates its instances too.
            Some(Component::Main { frame, .. }) => {
                let o = Point::ZERO + frame.translation();
                *frame = Affine::translate(a * o - o) * *frame;
            }
            Some(Component::Instance(l)) => l.xf = a * l.xf,
            None => {}
        }
        match &mut self.kind {
            ShapeKind::Compound { children, .. } | ShapeKind::Group { children } => {
                let t = a * self.transform;
                for c in children {
                    c.apply_transform(t);
                }
                // Warping commutes with the transform: the envelope and its
                // sources follow, so the result stays in step.
                if let Some(crate::paint_ops::LiveEffect::Warp(w)) = &mut self.live {
                    w.frame = t * w.frame;
                    for c in &mut w.sources {
                        c.apply_transform(t);
                    }
                }
                self.transform = Affine::IDENTITY;
            }
            ShapeKind::Path(pd) => {
                // Paths bake the transform, which changes their bounding box:
                // keep a gradient's handles on the same points of the shape.
                let unit_box = |pd: &PathData, t: Affine| {
                    let b = crate::geom::path_bounds(&(t * crate::geom::pathdata_to_bez(pd)))?;
                    Some(Affine::new([b.width().max(1e-9), 0.0, 0.0, b.height().max(1e-9), b.x0, b.y0]))
                };
                let before = matches!(self.fill, Some(Paint::Gradient(_))).then(|| unit_box(pd, self.transform)).flatten();
                pd.transform(a * self.transform);
                if let (Some(l0), Some(l1), Some(Paint::Gradient(g))) = (before, unit_box(pd, Affine::IDENTITY), &mut self.fill) {
                    let m = l1.inverse() * a * l0;
                    for p in [&mut g.start, &mut g.end] {
                        let q = m * Point::new(p[0], p[1]);
                        *p = [q.x, q.y];
                    }
                }
                self.transform = Affine::IDENTITY;
            }
            ShapeKind::Text(td) => {
                let t = a * self.transform;
                match (td.frame.as_mut(), split_scale(t)) {
                    // Frame text: resizing changes the frame, not the glyphs.
                    (Some(f), Some((rt, sx, sy))) => {
                        f[0] = (f[0] * sx).max(1.0);
                        f[1] = (f[1] * sy).max(1.0);
                        self.transform = rt;
                    }
                    _ => self.transform = t,
                }
            }
            kind => {
                let t = a * self.transform;
                match (kind.size(), split_scale(t)) {
                    (Some((w, h)), Some((rt, sx, sy))) => {
                        kind.set_size(w * sx, h * sy);
                        if let ShapeKind::Rect { radii, .. } = kind {
                            let k = sx.min(sy);
                            if (k - 1.0).abs() > 1e-9 {
                                for r in radii.iter_mut() {
                                    *r *= k;
                                }
                            }
                        }
                        self.transform = rt;
                    }
                    _ => self.transform = t,
                }
            }
        }
    }
}

/// Set the revision of a shape and all of its descendants.
fn stamp_subtree(s: &mut Shape, rev: u64) {
    s.rev = rev;
    if let Some(children) = s.kind.children_mut() {
        for c in children {
            stamp_subtree(c, rev);
        }
    }
}

/// Split an affine without skew or mirroring into (translate·rotate, sx, sy).
pub fn split_scale(t: Affine) -> Option<(Affine, f64, f64)> {
    let [a, b, c, d, e, f] = t.as_coeffs();
    let sx = (a * a + b * b).sqrt();
    let sy = (c * c + d * d).sqrt();
    if sx < 1e-9 || sy < 1e-9 {
        return None;
    }
    let ortho = (a * c + b * d).abs() <= 1e-6 * sx * sy;
    let det = a * d - b * c;
    if !ortho || det <= 0.0 {
        return None;
    }
    let rot = b.atan2(a);
    // Snap tiny rotations so axis-aligned shapes stay exactly axis-aligned.
    let rt = if rot.abs() < 1e-9 { Affine::translate((e, f)) } else { Affine::translate((e, f)) * Affine::rotate(rot) };
    Some((rt, sx, sy))
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageAsset {
    pub width: u32,
    pub height: u32,
    /// Original encoded bytes (PNG/JPEG/...), used for export and saving.
    /// Containers store them as separate entries (empty in the JSON).
    #[serde(with = "b64", default, skip_serializing_if = "Vec::is_empty")]
    pub encoded: Vec<u8>,
    /// Decoded RGBA8 pixels (not serialized).
    #[serde(skip)]
    pub rgba: Option<Arc<Vec<u8>>>,
}

impl ImageAsset {
    pub fn from_encoded(encoded: Vec<u8>) -> Result<Self, String> {
        let img = image::load_from_memory(&encoded).map_err(|e| e.to_string())?;
        let rgba = img.to_rgba8();
        Ok(ImageAsset {
            width: rgba.width(),
            height: rgba.height(),
            encoded,
            rgba: Some(Arc::new(rgba.into_raw())),
        })
    }

    pub fn ensure_decoded(&mut self) {
        if self.rgba.is_none() {
            if let Ok(img) = image::load_from_memory(&self.encoded) {
                self.rgba = Some(Arc::new(img.to_rgba8().into_raw()));
            }
        }
    }

    /// Encoded bytes as PNG (re-encodes when the original is not PNG).
    pub fn png_bytes(&self) -> Vec<u8> {
        if self.encoded.starts_with(b"\x89PNG") {
            return self.encoded.clone();
        }
        let mut out = Vec::new();
        if let Some(rgba) = &self.rgba {
            if let Some(buf) = image::RgbaImage::from_raw(self.width, self.height, rgba.as_ref().clone()) {
                let _ = buf.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
            }
        }
        out
    }
}

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

fn default_dpi() -> f64 {
    96.0
}

/// A page on the canvas. Shapes are not owned by artboards: an artboard shows
/// (and exports) whatever lies inside its rectangle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artboard {
    pub id: u64,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub background: Rgba,
    /// Hide the parts of its artwork that fall outside the artboard.
    #[serde(default)]
    pub clip: bool,
    /// Export settings (slices).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<ExportSlice>,
}

impl Artboard {
    pub fn rect(&self) -> kurbo::Rect {
        kurbo::Rect::new(self.x, self.y, self.x + self.w, self.y + self.h)
    }

    pub fn set_rect(&mut self, r: kurbo::Rect) {
        let r = r.abs();
        (self.x, self.y, self.w, self.h) = (r.x0, r.y0, r.width().max(1.0), r.height().max(1.0));
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    /// File format version (see `io::native`); 0 = legacy JSON.
    #[serde(default)]
    pub format_version: u32,
    /// Pages, in creation order (at least one).
    #[serde(default)]
    pub artboards: Vec<Artboard>,
    // Single-page files from before artboards (read only, migrated on load).
    #[serde(default, rename = "width", skip_serializing)]
    legacy_width: Option<f64>,
    #[serde(default, rename = "height", skip_serializing)]
    legacy_height: Option<f64>,
    #[serde(default, rename = "background", skip_serializing)]
    legacy_background: Option<Rgba>,
    /// Display unit for lengths (geometry is stored in px).
    #[serde(default)]
    pub unit: crate::units::Unit,
    /// Pixels per inch, used to convert to physical units and for PDF/EPS size.
    #[serde(default = "default_dpi")]
    pub dpi: f64,
    /// Bottom-most first. Shared with undo snapshots (copy on write).
    shapes: Vec<std::sync::Arc<Shape>>,
    pub images: BTreeMap<u64, ImageAsset>,
    /// Ruler guides.
    #[serde(default)]
    pub guides: Vec<Guide>,
    /// Grid spacing in px (shown and snapped to when enabled).
    #[serde(default = "default_grid")]
    pub grid: f64,
    /// Named colour keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<ColorKey>,
    /// Named paragraph and character styles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_styles: Vec<TextStyle>,
    next_id: u64,
    #[serde(skip)]
    next_rev: u64,
    /// Id → index path, for O(1) lookups (rebuilt lazily).
    #[serde(skip)]
    index: IdIndex,
}

/// Lazily built map from shape id to its index path. Entries are checked
/// against the tree before use, so moves and edits can't return a wrong
/// shape; a miss rebuilds the map when the document changed since.
#[derive(Default)]
struct IdIndex(std::sync::Mutex<Option<(u64, std::sync::Arc<std::collections::HashMap<u64, Box<[u32]>>>)>>);

impl Clone for IdIndex {
    fn clone(&self) -> Self {
        IdIndex::default()
    }
}

impl std::fmt::Debug for IdIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdIndex")
    }
}

fn default_grid() -> f64 {
    10.0
}

/// A vertical (x = pos) or horizontal (y = pos) ruler guide.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub vertical: bool,
    pub pos: f64,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            format_version: crate::io::native::FORMAT_VERSION,
            artboards: vec![Artboard { id: 1, name: "Artboard 1".into(), x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, background: Rgba::WHITE, clip: false, exports: Vec::new() }],
            legacy_width: None,
            legacy_height: None,
            legacy_background: None,
            unit: crate::units::Unit::Px,
            dpi: 96.0,
            shapes: Vec::new(),
            images: BTreeMap::new(),
            guides: Vec::new(),
            grid: 10.0,
            colors: Vec::new(),
            text_styles: Vec::new(),
            next_id: 2,
            next_rev: 1,
            index: IdIndex::default(),
        }
    }
}

impl Document {
    pub fn shapes(&self) -> &[std::sync::Arc<Shape>] {
        &self.shapes
    }

    /// Changes whenever any shape changes (for caches).
    pub fn revision(&self) -> u64 {
        self.next_rev
    }

    pub fn fresh_rev(&mut self) -> u64 {
        self.next_rev += 1;
        self.next_rev
    }

    pub fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Re-stamp every shape after loading (revisions are not persisted), and
    /// migrate single-page files to one artboard.
    pub fn after_load(&mut self) {
        self.next_rev = 1;
        self.index = IdIndex::default();
        let max_ab = self.artboards.iter().map(|a| a.id).max().unwrap_or(0);
        let max_id = self.all_ids().into_iter().max().unwrap_or(0).max(max_ab);
        let max_img = self.images.keys().copied().max().unwrap_or(0);
        self.next_id = self.next_id.max(max_id).max(max_img) + 1;
        for i in 0..self.shapes.len() {
            let r = self.fresh_rev();
            std::sync::Arc::make_mut(&mut self.shapes[i]).rev = r;
        }
        for img in self.images.values_mut() {
            img.ensure_decoded();
        }
        if self.artboards.is_empty() {
            let id = self.fresh_id();
            let (w, h) = (self.legacy_width.take().unwrap_or(1920.0), self.legacy_height.take().unwrap_or(1080.0));
            let background = self.legacy_background.take().unwrap_or(Rgba::WHITE);
            self.artboards.push(Artboard { id, name: "Artboard 1".into(), x: 0.0, y: 0.0, w: w.max(1.0), h: h.max(1.0), background, clip: false, exports: Vec::new() });
        }
    }

    // --- Artboards -------------------------------------------------------

    /// The first artboard: the page of single-page formats and imports.
    pub fn primary(&self) -> &Artboard {
        &self.artboards[0]
    }

    pub fn primary_mut(&mut self) -> &mut Artboard {
        &mut self.artboards[0]
    }

    /// Resize the first artboard (imports, templates).
    pub fn set_size(&mut self, w: f64, h: f64) {
        let a = self.primary_mut();
        a.w = w.max(1.0);
        a.h = h.max(1.0);
    }

    pub fn artboard(&self, id: u64) -> Option<&Artboard> {
        self.artboards.iter().find(|a| a.id == id)
    }

    pub fn artboard_mut(&mut self, id: u64) -> Option<&mut Artboard> {
        self.artboards.iter_mut().find(|a| a.id == id)
    }

    /// Top-most (last created) artboard containing `p`.
    pub fn artboard_at(&self, p: Point) -> Option<&Artboard> {
        self.artboards.iter().rev().find(|a| a.rect().contains(p))
    }

    /// The artboard a shape with bounds `b` belongs to: the top-most one
    /// containing its center (None: loose on the canvas).
    pub fn member_of(&self, b: kurbo::Rect) -> Option<&Artboard> {
        self.artboard_at(b.center())
    }

    /// The artboard overlapping `r` the most.
    pub fn artboard_for(&self, r: kurbo::Rect) -> Option<&Artboard> {
        let area = |a: &Artboard| {
            let i = a.rect().intersect(r);
            if i.width() > 0.0 && i.height() > 0.0 { i.area() } else { 0.0 }
        };
        let best = self.artboards.iter().rev().max_by(|a, b| area(a).total_cmp(&area(b)))?;
        (area(best) > 0.0 || r.area() == 0.0 && best.rect().contains(r.origin())).then_some(best)
    }

    /// Union of all artboards.
    pub fn bounds(&self) -> kurbo::Rect {
        self.artboards.iter().map(|a| a.rect()).reduce(|a, b| a.union(b)).unwrap_or(kurbo::Rect::new(0.0, 0.0, 1.0, 1.0))
    }

    /// Add an artboard; returns its id.
    pub fn add_artboard(&mut self, r: kurbo::Rect, name: Option<String>) -> u64 {
        let id = self.fresh_id();
        let mut n = self.artboards.len() + 1;
        while self.artboards.iter().any(|a| a.name == format!("Artboard {n}")) {
            n += 1;
        }
        let mut a = Artboard { id, name: name.unwrap_or(format!("Artboard {n}")), x: 0.0, y: 0.0, w: 1.0, h: 1.0, background: Rgba::WHITE, clip: false, exports: Vec::new() };
        a.set_rect(r);
        self.artboards.push(a);
        id
    }

    /// A single-page copy for exporting artboard `id`: that artboard moved to
    /// the origin with the shapes (and guides) moved along.
    pub fn page(&self, id: u64) -> Document {
        let mut d = self.clone();
        let Some(ab) = self.artboard(id).cloned() else { return d };
        d.artboards = vec![Artboard { x: 0.0, y: 0.0, ..ab.clone() }];
        if ab.x != 0.0 || ab.y != 0.0 {
            let shift = Affine::translate((-ab.x, -ab.y));
            for s in &mut d.shapes {
                std::sync::Arc::make_mut(s).apply_transform(shift);
            }
            for g in &mut d.guides {
                g.pos -= if g.vertical { ab.x } else { ab.y };
            }
        }
        d
    }

    /// This document with its first artboard at the origin (exporters assume
    /// it); borrowed when it already is.
    pub fn at_origin(&self) -> std::borrow::Cow<'_, Document> {
        let a = self.primary();
        if a.x == 0.0 && a.y == 0.0 {
            std::borrow::Cow::Borrowed(self)
        } else {
            std::borrow::Cow::Owned(self.page(a.id))
        }
    }

    pub fn add_shape(&mut self, mut s: Shape) -> u64 {
        if s.id == 0 || self.get(s.id).is_some() {
            s.id = self.fresh_id();
        }
        self.reid_children(&mut s);
        let rev = self.fresh_rev();
        stamp_subtree(&mut s, rev);
        let id = s.id;
        self.shapes.push(std::sync::Arc::new(s));
        id
    }

    pub fn insert_shape(&mut self, index: usize, mut s: Shape) -> u64 {
        if s.id == 0 || self.get(s.id).is_some() {
            s.id = self.fresh_id();
        }
        self.reid_children(&mut s);
        let rev = self.fresh_rev();
        stamp_subtree(&mut s, rev);
        let id = s.id;
        let index = index.min(self.shapes.len());
        self.shapes.insert(index, std::sync::Arc::new(s));
        id
    }

    pub fn add_image(&mut self, asset: ImageAsset) -> u64 {
        let id = self.fresh_id();
        self.images.insert(id, asset);
        id
    }

    /// Top-level index (children of compounds have none).
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.path_to(id).filter(|p| p.len() == 1).map(|p| p[0])
    }

    /// Paint-order key: comparing keys orders shapes bottom to top.
    pub fn order_key(&self, id: u64) -> Vec<usize> {
        self.path_to(id).unwrap_or_default()
    }

    /// Replace shape `id` (at any depth) by `shapes`, in place. Returns the
    /// ids of the inserted shapes.
    pub fn replace_with(&mut self, id: u64, shapes: Vec<Shape>) -> Vec<u64> {
        let Some(path) = self.path_to(id) else { return Vec::new() };
        let at = *path.last().unwrap();
        // Take the old shape out first so its subtree's ids can be reused.
        if path.len() == 1 {
            self.shapes.remove(at);
        } else {
            self.at_path_mut(&path[..path.len() - 1]).kind.children_mut().unwrap().remove(at);
        }
        let mut prepared: Vec<Shape> = Vec::new();
        for mut s in shapes {
            if s.id == 0 || self.get(s.id).is_some() || prepared.iter().any(|p| p.id == s.id) {
                s.id = self.fresh_id();
            }
            self.reid_children_except(&mut s, &prepared);
            // Members keep ids they had elsewhere (a warp's results take
            // its sources'): a fresh revision keeps cached geometry from
            // standing in for them.
            let rev = self.fresh_rev();
            stamp_subtree(&mut s, rev);
            prepared.push(s);
        }
        let ids = prepared.iter().map(|s| s.id).collect();
        if path.len() == 1 {
            let at = at.min(self.shapes.len());
            self.shapes.splice(at..at, prepared.into_iter().map(std::sync::Arc::new));
        } else {
            let list = self.at_path_mut(&path[..path.len() - 1]).kind.children_mut().unwrap();
            let at = at.min(list.len());
            list.splice(at..at, prepared);
        }
        self.next_rev += 1;
        ids
    }

    /// Index path to a shape: top-level index, then child indices.
    fn path_to(&self, id: u64) -> Option<Vec<usize>> {
        let mut guard = self.index.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut fresh = false;
        loop {
            let built = match &*guard {
                Some((at, map)) if fresh || *at == self.next_rev || map.contains_key(&id) => map.clone(),
                _ => {
                    let map = std::sync::Arc::new(self.build_index());
                    *guard = Some((self.next_rev, map.clone()));
                    fresh = true;
                    map
                }
            };
            let Some(path) = built.get(&id) else {
                // Unknown and the map is current: not in the document.
                return None;
            };
            let path: Vec<usize> = path.iter().map(|i| *i as usize).collect();
            if self.resolve(&path).map(|s| s.id) == Some(id) {
                return Some(path);
            }
            if fresh {
                return None;
            }
            // Moved since the map was built.
            *guard = None;
        }
    }

    fn build_index(&self) -> std::collections::HashMap<u64, Box<[u32]>> {
        fn walk<S: std::borrow::Borrow<Shape>>(list: &[S], prefix: &mut Vec<u32>, out: &mut std::collections::HashMap<u64, Box<[u32]>>) {
            for (i, s) in list.iter().enumerate() {
                let s = s.borrow();
                prefix.push(i as u32);
                out.insert(s.id, prefix.clone().into_boxed_slice());
                if let Some(children) = s.kind.children() {
                    walk(children, prefix, out);
                }
                prefix.pop();
            }
        }
        let mut out = std::collections::HashMap::with_capacity(self.shapes.len());
        walk(&self.shapes, &mut Vec::new(), &mut out);
        out
    }

    fn resolve(&self, path: &[usize]) -> Option<&Shape> {
        let mut s: &Shape = self.shapes.get(*path.first()?)?;
        for &i in &path[1..] {
            s = s.kind.children()?.get(i)?;
        }
        Some(s)
    }

    /// Any shape, including children of compounds.
    pub fn get(&self, id: u64) -> Option<&Shape> {
        self.resolve(&self.path_to(id)?)
    }

    /// The compound directly containing `id`, if it is a child.
    pub fn parent_of(&self, id: u64) -> Option<u64> {
        let path = self.path_to(id)?;
        if path.len() < 2 {
            return None;
        }
        self.resolve(&path[..path.len() - 1]).map(|s| s.id)
    }

    /// Top-level shape that contains `id` (itself when top-level).
    pub fn root_of(&self, id: u64) -> Option<u64> {
        self.path_to(id).map(|p| self.shapes[p[0]].id)
    }

    /// Every shape id, depth first (compound before its children).
    pub fn all_ids(&self) -> Vec<u64> {
        fn walk<S: std::borrow::Borrow<Shape>>(list: &[S], out: &mut Vec<u64>) {
            for s in list {
                let s = s.borrow();
                out.push(s.id);
                if let Some(children) = s.kind.children() {
                    walk(children, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.shapes, &mut out);
        out
    }

    /// Mutable shape at an index path, bumping revisions along the way so
    /// compounds recompute when a child changes.
    fn at_path_mut(&mut self, path: &[usize]) -> &mut Shape {
        let rev = self.fresh_rev();
        let mut s = std::sync::Arc::make_mut(&mut self.shapes[path[0]]);
        s.rev = rev;
        for &i in &path[1..] {
            let Some(children) = s.kind.children_mut() else { unreachable!() };
            s = &mut children[i];
            s.rev = rev;
        }
        stamp_subtree(s, rev);
        s
    }

    /// Mutable access; bumps the revision so caches are invalidated.
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Shape> {
        let path = self.path_to(id)?;
        Some(self.at_path_mut(&path))
    }

    /// Replace a shape if it differs from the stored one. Returns true if changed.
    pub fn replace(&mut self, shape: Shape) -> bool {
        let Some(path) = self.path_to(shape.id) else { return false };
        if self.get(shape.id) == Some(&shape) {
            return false;
        }
        let slot = self.at_path_mut(&path);
        let rev = slot.rev;
        *slot = shape;
        stamp_subtree(slot, rev);
        true
    }

    /// Remove a shape (top-level or a compound child). A compound left
    /// without children is removed too.
    pub fn remove(&mut self, id: u64) -> Option<Shape> {
        let path = self.path_to(id)?;
        self.next_rev += 1;
        if path.len() == 1 {
            return Some(std::sync::Arc::unwrap_or_clone(self.shapes.remove(path[0])));
        }
        let parent = self.at_path_mut(&path[..path.len() - 1]);
        let Some(children) = parent.kind.children_mut() else { return None };
        let removed = children.remove(*path.last().unwrap());
        let (empty, pid) = (children.is_empty(), parent.id);
        if empty {
            self.remove(pid);
        }
        Some(removed)
    }

    /// Give nested children ids that are unique in this document.
    fn reid_children(&mut self, s: &mut Shape) {
        self.reid_children_except(s, &[]);
    }

    /// Like `reid_children`, also avoiding ids `taken` (shapes about to be
    /// inserted alongside).
    fn reid_children_except(&mut self, s: &mut Shape, taken: &[Shape]) {
        if s.kind.children().is_none_or(|c| c.is_empty()) {
            return;
        }
        let mut seen: std::collections::HashSet<u64> = self.all_ids().into_iter().collect();
        fn ids_of(list: &[Shape], out: &mut std::collections::HashSet<u64>) {
            for s in list {
                out.insert(s.id);
                if let Some(c) = s.kind.children() {
                    ids_of(c, out);
                }
            }
        }
        ids_of(taken, &mut seen);
        seen.insert(s.id);
        let mut remap = std::collections::HashMap::new();
        fn walk(doc: &mut Document, s: &mut Shape, seen: &mut std::collections::HashSet<u64>, remap: &mut std::collections::HashMap<u64, u64>) {
            if let Some(children) = s.kind.children_mut() {
                for c in children {
                    if c.id == 0 || seen.contains(&c.id) {
                        let new = doc.fresh_id();
                        remap.insert(c.id, new);
                        c.id = new;
                    }
                    seen.insert(c.id);
                    walk(doc, c, seen, remap);
                }
            }
        }
        walk(self, s, &mut seen, &mut remap);
        // Instances keep pointing at their (renumbered) layers.
        fn fix(s: &mut Shape, remap: &std::collections::HashMap<u64, u64>) {
            if let Some(Component::Instance(l)) = &mut s.component {
                for v in l.ids.values_mut() {
                    if let Some(n) = remap.get(v) {
                        *v = *n;
                    }
                }
            }
            if let Some(children) = s.kind.children_mut() {
                for c in children {
                    fix(c, remap);
                }
            }
        }
        if !remap.is_empty() {
            fix(s, &remap);
        }
    }

    /// Restack top-level `id` directly above (or below) top-level `anchor`.
    pub fn move_next_to(&mut self, id: u64, anchor: u64, above: bool) {
        if id == anchor {
            return;
        }
        let (Some(i), j) = (self.index_of(id), self.index_of(anchor)) else { return };
        let s = self.shapes.remove(i);
        match j {
            Some(j) => self.shapes.insert(if above { j + 1 } else { j } - usize::from(j > i), s),
            None => self.shapes.insert(i, s),
        }
        self.next_rev += 1;
    }

    /// Whether `id` is `ancestor` or inside it.
    pub fn is_within(&self, id: u64, ancestor: u64) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self.parent_of(c);
        }
        false
    }

    /// Move shape `id` (at any depth) into container `parent` at child
    /// `index` (0 = bottom), or to the top level when `parent` is None.
    /// Geometry is kept (child transforms are in document space).
    pub fn move_into(&mut self, id: u64, parent: Option<u64>, index: usize) -> bool {
        if parent.is_some_and(|p| self.is_within(p, id)) || parent.is_some_and(|p| self.get(p).and_then(|s| s.kind.children()).is_none()) {
            return false;
        }
        let Some(s) = self.remove_keep_parent(id) else { return false };
        match parent.and_then(|p| self.path_to(p)) {
            Some(path) => {
                let ps = self.at_path_mut(&path);
                let Some(children) = ps.kind.children_mut() else { unreachable!() };
                let index = index.min(children.len());
                children.insert(index, s);
            }
            None => {
                let index = index.min(self.shapes.len());
                self.shapes.insert(index, std::sync::Arc::new(s));
            }
        }
        self.next_rev += 1;
        true
    }

    /// Move shape `id` (at any depth) just above or below `anchor`, in
    /// `anchor`'s parent.
    pub fn move_beside(&mut self, id: u64, anchor: u64, above: bool) -> bool {
        if id == anchor || self.is_within(anchor, id) {
            return false;
        }
        let parent = self.parent_of(anchor);
        let Some(s) = self.remove_keep_parent(id) else { return false };
        let Some(path) = self.path_to(anchor) else {
            self.shapes.push(std::sync::Arc::new(s));
            return false;
        };
        let at = path[path.len() - 1] + usize::from(above);
        match parent {
            Some(_) => {
                let ps = self.at_path_mut(&path[..path.len() - 1]);
                let Some(children) = ps.kind.children_mut() else { unreachable!() };
                children.insert(at.min(children.len()), s);
            }
            None => self.shapes.insert(at.min(self.shapes.len()), std::sync::Arc::new(s)),
        }
        self.next_rev += 1;
        true
    }

    /// Like `remove`, but a group left empty is removed only when it has no
    /// other role (it stays when it is a component or auto layout).
    fn remove_keep_parent(&mut self, id: u64) -> Option<Shape> {
        let path = self.path_to(id)?;
        self.next_rev += 1;
        if path.len() == 1 {
            return Some(std::sync::Arc::unwrap_or_clone(self.shapes.remove(path[0])));
        }
        let parent = self.at_path_mut(&path[..path.len() - 1]);
        let children = parent.kind.children_mut()?;
        let removed = children.remove(*path.last().unwrap());
        let (empty, pid, keep) = (children.is_empty(), parent.id, parent.component.is_some() || parent.layout.is_some());
        if empty && !keep {
            self.remove(pid);
        }
        Some(removed)
    }

    pub fn move_to_index(&mut self, id: u64, index: usize) {
        if let Some(i) = self.index_of(id) {
            let s = self.shapes.remove(i);
            let index = index.min(self.shapes.len());
            self.shapes.insert(index, s);
            self.next_rev += 1;
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot { shapes: self.shapes.clone(), guides: self.guides.clone(), artboards: self.artboards.clone(), colors: self.colors.clone(), text_styles: self.text_styles.clone() }
    }

    pub fn restore(&mut self, s: Snapshot) {
        self.next_rev += 1;
        self.shapes = s.shapes;
        self.guides = s.guides;
        self.artboards = s.artboards;
        self.colors = s.colors;
        self.text_styles = s.text_styles;
    }

    // Colour keys ---------------------------------------------------------

    pub fn color_key(&self, id: u64) -> Option<&ColorKey> {
        self.colors.iter().find(|k| k.id == id)
    }

    /// Add a key; an empty name becomes "Color N".
    pub fn add_color_key(&mut self, name: &str, color: Rgba) -> u64 {
        let id = self.colors.iter().map(|k| k.id).max().unwrap_or(0) + 1;
        let name = if name.trim().is_empty() {
            let mut n = self.colors.len() + 1;
            while self.colors.iter().any(|k| k.name == format!("Colour {n}")) {
                n += 1;
            }
            format!("Colour {n}")
        } else {
            name.to_string()
        };
        self.colors.push(ColorKey { id, name, color, spot: false, cmyk: None });
        id
    }

    /// Ids of shapes linked to key `id`: (fill users, stroke users).
    pub fn key_users(&self, id: u64) -> (Vec<u64>, Vec<u64>) {
        let (mut f, mut s) = (Vec::new(), Vec::new());
        fn walk<S: std::borrow::Borrow<Shape>>(list: &[S], id: u64, f: &mut Vec<u64>, s: &mut Vec<u64>) {
            for sh in list {
                let sh = sh.borrow();
                if sh.fill_key == Some(id) {
                    f.push(sh.id);
                }
                if sh.stroke_key == Some(id) {
                    s.push(sh.id);
                }
                if let Some(c) = sh.kind.children() {
                    walk(c, id, f, s);
                }
            }
        }
        walk(&self.shapes, id, &mut f, &mut s);
        (f, s)
    }

    /// Change a key's colour and every fill and stroke linked to it. A link
    /// whose colour was changed some other way since is dropped instead.
    pub fn set_key_color(&mut self, id: u64, color: Rgba) {
        let Some(old) = self.color_key(id).map(|k| k.color) else { return };
        if let Some(k) = self.colors.iter_mut().find(|k| k.id == id) {
            k.color = color;
        }
        let (fills, strokes) = self.key_users(id);
        for sid in fills {
            let Some(mut sh) = self.get(sid).cloned() else { continue };
            match &mut sh.fill {
                Some(Paint::Solid(c)) if *c == old => *c = color,
                _ => sh.fill_key = None,
            }
            self.replace(sh);
        }
        for sid in strokes {
            let Some(mut sh) = self.get(sid).cloned() else { continue };
            match &mut sh.stroke {
                Some(st) if st.color == old => st.color = color,
                _ => sh.stroke_key = None,
            }
            self.replace(sh);
        }
    }

    /// Drop links of `s` (and its children) to keys this document doesn't
    /// have with the same colour (shapes pasted from another document).
    pub fn fix_key_links(&self, s: &mut Shape) {
        let fill = s.fill.as_ref().and_then(|f| f.solid());
        if s.fill_key.and_then(|k| self.color_key(k)).map(|k| Some(k.color) != fill).unwrap_or(true) {
            s.fill_key = None;
        }
        let stroke = s.stroke.as_ref().map(|st| st.color);
        if s.stroke_key.and_then(|k| self.color_key(k)).map(|k| Some(k.color) != stroke).unwrap_or(true) {
            s.stroke_key = None;
        }
        if let Some(ch) = s.kind.children_mut() {
            for c in ch {
                self.fix_key_links(c);
            }
        }
    }

    /// Remove a key; its users keep their colour, unlinked.
    pub fn remove_color_key(&mut self, id: u64) {
        let (fills, strokes) = self.key_users(id);
        for sid in fills {
            if let Some(mut sh) = self.get(sid).cloned() {
                sh.fill_key = None;
                self.replace(sh);
            }
        }
        for sid in strokes {
            if let Some(mut sh) = self.get(sid).cloned() {
                sh.stroke_key = None;
                self.replace(sh);
            }
        }
        self.colors.retain(|k| k.id != id);
    }

    /// Remove images that no shape references anymore.
    pub fn gc_images(&mut self) {
        let used: std::collections::HashSet<u64> = self
            .shapes
            .iter()
            .filter_map(|s| match &s.fill {
                Some(Paint::Image(f)) => Some(f.image),
                _ => None,
            })
            .collect();
        self.images.retain(|k, _| used.contains(k));
    }
}

// ---------------------------------------------------------------------------
// Undo history (snapshot based, with coalescing of continuous edits)
// ---------------------------------------------------------------------------

/// Undoable document state.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub shapes: Vec<std::sync::Arc<Shape>>,
    pub guides: Vec<Guide>,
    pub artboards: Vec<Artboard>,
    pub colors: Vec<ColorKey>,
    pub text_styles: Vec<TextStyle>,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_key: Option<String>,
    last_time: f64,
    /// Bumped on every change to the stacks (for caching derived views).
    pub generation: u64,
}

impl History {
    /// Record the current state before a mutation. Consecutive checkpoints with
    /// the same `key` within a short time window are coalesced.
    pub fn checkpoint(&mut self, doc: &Document, key: &str, now: f64) {
        let same = self.last_key.as_deref() == Some(key) && now - self.last_time < 1.0;
        self.last_time = now;
        if same && !key.is_empty() {
            return;
        }
        self.last_key = Some(key.to_string());
        self.generation += 1;
        self.undo.push(doc.snapshot());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn break_coalescing(&mut self) {
        self.last_key = None;
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        if let Some(s) = self.undo.pop() {
            self.redo.push(doc.snapshot());
            doc.restore(s);
            self.last_key = None;
            self.generation += 1;
            true
        } else {
            false
        }
    }

    /// Go back to the last checkpoint without keeping a redo step
    /// (a cancelled dialog).
    pub fn cancel(&mut self, doc: &mut Document) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        doc.restore(s);
        self.last_key = None;
        self.generation += 1;
        true
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        if let Some(s) = self.redo.pop() {
            self.undo.push(doc.snapshot());
            doc.restore(s);
            self.last_key = None;
            self.generation += 1;
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.last_key = None;
        self.generation += 1;
    }

    /// Past states (oldest first) and future states (next first).
    pub fn stacks(&self) -> (&[Snapshot], impl Iterator<Item = &Snapshot>) {
        (&self.undo, self.redo.iter().rev())
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
