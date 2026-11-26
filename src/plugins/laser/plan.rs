//! Document → laser job. Shapes go to colour layers (LightBurn style): a
//! stroke to its stroke colour's layer, a fill to its fill colour's. A Line
//! layer follows the paths' centrelines; a Fill layer hatches the union of
//! their areas. Inner contours are cut before the outlines around them, and
//! paths are visited nearest first.

use kurbo::{Affine, BezPath, Point, Rect, Shape as _};

use super::settings::{Area, Corner, Dither, DocSettings, FillStyle, LayerSettings, MachineSettings, Mode, Placement};
use crate::boolean::{self, BoolOp, Polys};
use crate::document::{Document, Paint, Rgba, Shape};
use crate::fonts::FontManager;

/// Curve flattening tolerance, mm.
const TOL_MM: f64 = 0.01;
/// Containment checks get quadratic; skip them above this many paths.
const MAX_NEST: usize = 4000;
const MAX_HATCH_LINES: usize = 40_000;
const MAX_RINGS: usize = 4_000;

/// The geometry of one colour, as found in the document (document px).
#[derive(Clone, Debug, Default)]
pub struct Source {
    pub key: String,
    pub rgb: [u8; 3],
    /// What the colour came from first (sets the default mode).
    pub from_stroke: bool,
    /// Subpaths: (points, closed).
    pub lines: Vec<(Vec<Point>, bool)>,
    /// Filled areas (each shape's, even-odd contours).
    pub areas: Vec<Polys>,
    /// Pictures (shapes with an image fill) and their outlines' bounds,
    /// for the Image layer.
    pub images: Vec<(Shape, Rect)>,
}

/// The layer all pictures go to.
pub const IMAGE_KEY: &str = "Image";

/// A colour layer of the plan; paths in document px, in cut order.
#[derive(Clone, Debug)]
pub struct PlanLayer {
    pub key: String,
    pub rgb: [u8; 3],
    pub settings: LayerSettings,
    pub paths: Vec<Vec<Point>>,
    /// How often the paths are run (1 when the passes are already in
    /// `paths`, e.g. a fill turning its angle each pass).
    pub passes: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub layers: Vec<PlanLayer>,
    /// The artboard's top-left (document px): the job's (0, 0).
    pub origin: Point,
    /// mm per document px.
    pub mm_per_px: f64,
    /// The artboard (document px).
    pub frame: Option<Rect>,
    pub warnings: Vec<String>,
}

fn rgb_of(c: Rgba) -> [u8; 3] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(c.r), q(c.g), q(c.b)]
}

fn paint_color(p: &Paint) -> Option<Rgba> {
    match p {
        Paint::Solid(c) => Some(*c),
        Paint::Gradient(g) => g.stops.first().map(|s| s.color),
        Paint::Image(_) => None,
    }
}

/// The document as cut: everything, or only the selection (`ids`).
pub fn scope(doc: &Document, area: Area, ids: &[u64]) -> Document {
    let mut d = doc.clone();
    if area == Area::Selection {
        let mut keep: Vec<Shape> = ids.iter().filter_map(|id| doc.get(*id).cloned()).collect();
        // Drop shapes whose container is selected too.
        let all: Vec<u64> = keep.iter().map(|s| s.id).collect();
        keep.retain(|s| {
            let mut p = doc.parent_of(s.id);
            while let Some(x) = p {
                if all.contains(&x) {
                    return false;
                }
                p = doc.parent_of(x);
            }
            true
        });
        keep.sort_by_key(|s| doc.order_key(s.id));
        let tops: Vec<u64> = d.shapes().iter().map(|s| s.id).collect();
        for id in tops {
            d.remove(id);
        }
        for s in keep {
            d.add_shape(s);
        }
    }
    d
}

/// Colour sources of a document's visible shapes (text as outlines); with
/// `artboard`, only of the shapes on it.
pub fn collect(doc: &Document, fonts: &mut FontManager, artboard: Option<u64>, warnings: &mut Vec<String>) -> Vec<Source> {
    let tol = TOL_MM * doc.dpi / 25.4;
    let mut out: Vec<Source> = Vec::new();
    let find = |rgb: [u8; 3], from_stroke: bool, out: &mut Vec<Source>| -> usize {
        let key = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
        if let Some(i) = out.iter().position(|s| s.key == key) {
            return i;
        }
        out.push(Source { key, rgb, from_stroke, ..Default::default() });
        out.len() - 1
    };
    let mut clipped = false;
    for it in crate::io::prepare(doc, fonts) {
        if it.span > 0 {
            continue;
        }
        if let Some(ab) = artboard
            && doc.member_of(it.path.bounding_box()).map(|a| a.id) != Some(ab)
        {
            continue;
        }
        let s: &Shape = &it.shape;
        if matches!(s.fill, Some(Paint::Image(_))) {
            let i = match out.iter().position(|s| s.key == IMAGE_KEY) {
                Some(i) => i,
                None => {
                    out.push(Source { key: IMAGE_KEY.into(), rgb: [128, 128, 128], ..Default::default() });
                    out.len() - 1
                }
            };
            out[i].images.push((s.clone(), it.path.bounding_box()));
        }
        if let Some(st) = s.stroke.as_ref().filter(|st| st.width > 0.0) {
            let i = find(rgb_of(st.color), true, &mut out);
            out[i].lines.extend(crate::geom::flatten_subpaths(&it.path, tol));
            clipped |= it.clip.is_some();
            // Its closed outlines are its area, for a Fill layer of that colour.
            if s.fill.is_none() {
                out[i].areas.push(boolean::path_to_polys(&it.path, s.fill_rule));
            }
        }
        if let Some(c) = s.fill.as_ref().and_then(paint_color) {
            let i = find(rgb_of(c), false, &mut out);
            let mut polys = boolean::path_to_polys(&it.path, s.fill_rule);
            if let Some((clip, rule)) = &it.clip {
                polys = boolean::op(&polys, &boolean::path_to_polys(clip, *rule), BoolOp::Intersect);
            }
            out[i].areas.push(polys);
            if s.stroke.as_ref().is_none_or(|st| st.width <= 0.0) {
                out[i].lines.extend(crate::geom::flatten_subpaths(&it.path, tol));
            }
        }
    }
    if clipped {
        warnings.push("Clipped strokes are cut whole (clipping applies to fills only)".into());
    }
    out
}

/// The settings a colour uses (saved ones, else a default for how it's used).
pub fn layer_settings(doc: &DocSettings, src: &Source) -> LayerSettings {
    let image = src.key == IMAGE_KEY;
    match doc.layers.get(&src.key) {
        // Pictures are always engraved as images; colours never are.
        Some(s) if image => LayerSettings { mode: Mode::Image, ..s.clone() },
        Some(s) if s.mode == Mode::Image => LayerSettings { mode: Mode::Fill, ..s.clone() },
        Some(s) => s.clone(),
        None if image => LayerSettings::image(),
        None if src.from_stroke => LayerSettings::line(),
        None => LayerSettings::fill(),
    }
}

pub fn plan(doc: &Document, fonts: &mut FontManager, settings: &DocSettings, area: Area, artboard: u64, ids: &[u64]) -> Plan {
    let scoped = scope(doc, area, ids);
    let mut warnings = Vec::new();
    let sources = collect(&scoped, fonts, (area == Area::Artboard).then_some(artboard), &mut warnings);
    let mm_per_px = 25.4 / doc.dpi.max(1.0);
    let origin = doc.artboard(artboard).map(|a| Point::new(a.x, a.y)).unwrap_or_default();
    let mut layers = Vec::new();
    let mut at = origin;
    // Engrave first, then cut: a part cut free may shift.
    let mut sources: Vec<(LayerSettings, &Source)> = sources.iter().map(|s| (layer_settings(settings, s), s)).collect();
    sources.sort_by_key(|(st, _)| match st.mode {
        Mode::Image => 0,
        Mode::Fill => 1,
        Mode::Line => 2,
    });
    for (st, src) in sources {
        let paths = match st.mode {
            _ if !st.output => Vec::new(),
            Mode::Line => order(&src.lines, &mut at),
            Mode::Fill => {
                let parts: Vec<&Polys> = src.areas.iter().collect();
                let area = boolean::union_all(&parts);
                let lines = fill(&area, &st, 1.0 / mm_per_px, &mut at, &mut warnings);
                if let Some(l) = lines.last().and_then(|p| p.last()) {
                    at = *l;
                }
                lines
            }
            Mode::Image => {
                let lines = raster(&scoped, fonts, &src.images, &st, 1.0 / mm_per_px, &mut warnings);
                if let Some(l) = lines.last().and_then(|p| p.last()) {
                    at = *l;
                }
                lines
            }
        };
        let baked = st.mode == Mode::Fill && st.fill_style != FillStyle::Offset && st.angle_step != 0.0 && st.passes > 1;
        let passes = if baked { 1 } else { st.passes };
        layers.push(PlanLayer { key: src.key.clone(), rgb: src.rgb, settings: st, paths, passes });
    }
    Plan { layers, origin, mm_per_px, frame: doc.artboard(artboard).map(|a| a.rect()), warnings }
}

/// Image brightness 0..1 (white = 1) at each pixel, over white, with the
/// layer's brightness / contrast / negative applied.
fn tones(rgba: &[u8], st: &LayerSettings) -> Vec<f32> {
    let b = (st.brightness / 100.0) as f32;
    let c = (st.contrast / 100.0).clamp(-0.99, 0.99) as f32;
    let k = (1.0 + c) / (1.0 - c);
    rgba.as_chunks::<4>().0.iter()
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            let l = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0;
            let mut v = 1.0 - a * (1.0 - l);
            if st.invert && a > 0.0 {
                v = 1.0 - v;
            }
            ((v - 0.5) * k + 0.5 + b).clamp(0.0, 1.0)
        })
        .collect()
}

/// Which pixels burn (true), `w` × `h`, from brightness.
pub fn dither(v: &[f32], w: usize, h: usize, st: &LayerSettings) -> Vec<bool> {
    let mut out = vec![false; w * h];
    match st.dither {
        Dither::Threshold => {
            let t = (st.threshold / 100.0) as f32;
            for (o, x) in out.iter_mut().zip(v) {
                *o = *x < t;
            }
        }
        Dither::Ordered => {
            const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
            for y in 0..h {
                for x in 0..w {
                    let t = (BAYER[y % 4][x % 4] as f32 + 0.5) / 16.0;
                    out[y * w + x] = v[y * w + x] < t;
                }
            }
        }
        Dither::FloydSteinberg | Dither::Jarvis => {
            let taps: &[(isize, isize, f32)] = if st.dither == Dither::FloydSteinberg {
                &[(1, 0, 7.0), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)]
            } else {
                &[(1, 0, 7.0), (2, 0, 5.0), (-2, 1, 3.0), (-1, 1, 5.0), (0, 1, 7.0), (1, 1, 5.0), (2, 1, 3.0), (-2, 2, 1.0), (-1, 2, 3.0), (0, 2, 5.0), (1, 2, 3.0), (2, 2, 1.0)]
            };
            let total: f32 = taps.iter().map(|t| t.2).sum();
            let mut e = v.to_vec();
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let on = e[i] < 0.5;
                    out[i] = on;
                    let err = e[i] - if on { 0.0 } else { 1.0 };
                    for (dx, dy, k) in taps {
                        let (nx, ny) = (x as isize + dx, y as isize + dy);
                        if nx >= 0 && (nx as usize) < w && (ny as usize) < h {
                            e[ny as usize * w + nx as usize] += err * k / total;
                        }
                    }
                }
            }
        }
    }
    out
}

/// An Image layer's toolpaths: each picture rendered at the line interval
/// (as it looks: rotated, cropped, masked), dithered, and every run of
/// burning pixels on a row is one cut (document px).
pub fn raster(doc: &Document, fonts: &mut FontManager, images: &[(Shape, Rect)], st: &LayerSettings, px_per_mm: f64, warnings: &mut Vec<String>) -> Vec<Vec<Point>> {
    let interval = st.interval.max(0.02) * px_per_mm;
    let mut out = Vec::new();
    let mut flip = false;
    for (s, bounds) in images {
        let Some(r) = crate::render::svg_paint::rasterize_shape_svg(s, doc, fonts, 1.0 / interval, 20_000.0) else { continue };
        if (r.rect.width() / r.w as f64) > interval * 1.5 {
            warnings.push("An image is too large for this line interval: engraved at a coarser one".into());
        }
        let (pw, ph) = (r.rect.width() / r.w as f64, r.rect.height() / r.h as f64);
        let dots = dither(&tones(&r.rgba, st), r.w, r.h, st);
        for y in 0..r.h {
            let row = &dots[y * r.w..(y + 1) * r.w];
            let cy = r.rect.y0 + (y as f64 + 0.5) * ph;
            if cy < bounds.y0 || cy > bounds.y1 {
                continue;
            }
            let mut runs: Vec<(f64, f64)> = Vec::new();
            let mut x = 0;
            while x < r.w {
                if row[x] {
                    let start = x;
                    while x < r.w && row[x] {
                        x += 1;
                    }
                    // Pixel edges, kept inside the picture (the raster
                    // has a margin and its grid doesn't meet the edges).
                    let a = (r.rect.x0 + start as f64 * pw).max(bounds.x0);
                    let b = (r.rect.x0 + x as f64 * pw).min(bounds.x1);
                    if b > a {
                        runs.push((a, b));
                    }
                } else {
                    x += 1;
                }
            }
            if runs.is_empty() {
                continue;
            }
            if flip {
                runs.reverse();
                for r in &mut runs {
                    *r = (r.1, r.0);
                }
            }
            if st.bidirectional {
                flip = !flip;
            }
            out.extend(runs.into_iter().map(|(a, b)| vec![Point::new(a, cy), Point::new(b, cy)]));
        }
    }
    out
}

/// A Fill layer's toolpaths over `area` (document px; `px_per_mm` for
/// the settings in mm): per pass (when the angle turns each pass), per
/// shape (one by one, nearest first) or all at once.
pub fn fill(area: &Polys, st: &LayerSettings, px_per_mm: f64, at: &mut Point, warnings: &mut Vec<String>) -> Vec<Vec<Point>> {
    let interval = st.interval.max(0.01) * px_per_mm;
    let pieces: Vec<Polys> = if st.one_by_one {
        // Nearest shape next.
        let mut rest = boolean::split_pieces(area);
        let mut out = Vec::new();
        let mut from = *at;
        while !rest.is_empty() {
            let centre = |p: &Polys| boolean::bounds(p).map(|b| b.center()).unwrap_or_default();
            let i = (0..rest.len()).min_by(|a, b| centre(&rest[*a]).distance(from).total_cmp(&centre(&rest[*b]).distance(from))).unwrap();
            let p = rest.swap_remove(i);
            from = centre(&p);
            out.push(p);
        }
        out
    } else {
        vec![area.clone()]
    };
    let turns = if st.fill_style != FillStyle::Offset && st.angle_step != 0.0 { st.passes.max(1) } else { 1 };
    let mut out = Vec::new();
    for pass in 0..turns {
        let angle = st.angle + st.angle_step * pass as f64;
        for piece in &pieces {
            match st.fill_style {
                FillStyle::Scan => out.extend(hatch(piece, interval, angle, st.bidirectional, warnings)),
                FillStyle::CrossHatch => {
                    out.extend(hatch(piece, interval, angle, st.bidirectional, warnings));
                    out.extend(hatch(piece, interval, angle + 90.0, st.bidirectional, warnings));
                }
                FillStyle::Offset => out.extend(rings(piece, interval, warnings)),
            }
        }
    }
    if let Some(p) = out.last().and_then(|p| p.last()) {
        *at = *p;
    }
    out
}

/// Offset fill: the outline, then rings `interval` apart going inwards,
/// each a closed path.
pub fn rings(area: &Polys, interval: f64, warnings: &mut Vec<String>) -> Vec<Vec<Point>> {
    // Each ring is the outline itself offset inwards by (k + ½) intervals.
    // Offsetting the previous ring instead would pile up round-join points
    // ring after ring, and get slower and slower (minutes for a fine fill).
    let mut out = Vec::new();
    for k in 0.. {
        if k >= MAX_RINGS {
            warnings.push(format!("Offset fill limited to {MAX_RINGS} rings (interval too fine for this area)"));
            break;
        }
        let cur = boolean::offset_polys(area, -interval * (k as f64 + 0.5));
        if cur.is_empty() || boolean::area(&cur) < interval * interval * 0.25 {
            break;
        }
        for c in &cur {
            let mut p: Vec<Point> = c.iter().map(|q| Point::new(q[0], q[1])).collect();
            if p.len() >= 3 {
                p.push(p[0]);
                out.push(p);
            }
        }
    }
    out
}

fn area_of(p: &[Point]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].x * p[(i + 1) % n].y - p[(i + 1) % n].x * p[i].y).sum::<f64>() * 0.5
}

fn inside(poly: &[Point], q: Point) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Cut order: a path waits until every closed path inside it is done
/// (parts don't drop out before their holes are cut); among the ready ones
/// the nearest goes next. Closed paths start at their vertex nearest the
/// head, open ones from their nearer end. Closed paths come back to their
/// start.
pub fn order(paths: &[(Vec<Point>, bool)], at: &mut Point) -> Vec<Vec<Point>> {
    let n = paths.len();
    // waits[i]: closed paths inside path i not cut yet; holders[j]: paths
    // around path j.
    let mut waits = vec![0usize; n];
    let mut holders: Vec<Vec<usize>> = vec![Vec::new(); n];
    if n <= MAX_NEST {
        let areas: Vec<f64> = paths.iter().map(|(p, c)| if *c { area_of(p).abs() } else { 0.0 }).collect();
        let boxes: Vec<Rect> = paths.iter().map(|(p, _)| p.iter().fold(Rect::from_points(p[0], p[0]), |r, q| r.union_pt(*q))).collect();
        for i in 0..n {
            if !paths[i].1 {
                continue;
            }
            for j in 0..n {
                if i == j || !paths[j].1 || areas[j] >= areas[i] || !boxes[i].contains_rect(boxes[j]) {
                    continue;
                }
                if inside(&paths[i].0, paths[j].0[0]) {
                    waits[i] += 1;
                    holders[j].push(i);
                }
            }
        }
    }
    let mut done = vec![false; n];
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        // Nearest ready path, and where to start it.
        let mut best: Option<(f64, usize, usize, bool)> = None;
        for i in (0..n).filter(|i| !done[*i] && waits[*i] == 0) {
            let (p, closed) = &paths[i];
            let cands: Vec<(usize, bool)> = if *closed { (0..p.len()).map(|k| (k, false)).collect() } else { vec![(0, false), (p.len() - 1, true)] };
            for (k, rev) in cands {
                let d = p[k].distance_squared(*at);
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, i, k, rev));
                }
            }
        }
        let Some((_, i, k, rev)) = best else { break };
        done[i] = true;
        for h in &holders[i] {
            waits[*h] -= 1;
        }
        let (p, closed) = &paths[i];
        let mut q: Vec<Point> = if *closed {
            let mut q: Vec<Point> = p[k..].iter().chain(p[..k].iter()).copied().collect();
            q.push(p[k]);
            q
        } else if rev {
            p.iter().rev().copied().collect()
        } else {
            p.clone()
        };
        q.dedup();
        if q.len() >= 2 {
            *at = *q.last().unwrap();
            out.push(q);
        }
    }
    out
}

/// Scanlines across `area` (even-odd), `interval` apart (document px), at
/// `angle_deg`; each crossing is a cut from one edge to the other.
/// Bidirectional lines alternate direction (less travel).
pub fn hatch(area: &Polys, interval: f64, angle_deg: f64, bidirectional: bool, warnings: &mut Vec<String>) -> Vec<Vec<Point>> {
    if area.is_empty() || interval <= 0.0 {
        return Vec::new();
    }
    let rot = Affine::rotate(-angle_deg.to_radians());
    let back = rot.inverse();
    let polys: Vec<Vec<Point>> = area.iter().map(|p| p.iter().map(|q| rot * Point::new(q[0], q[1])).collect()).collect();
    let (y0, y1) = polys.iter().flatten().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
    let mut count = ((y1 - y0) / interval).floor() as usize + 1;
    if count > MAX_HATCH_LINES {
        warnings.push(format!("Fill interval too fine for this area: limited to {MAX_HATCH_LINES} lines"));
        count = MAX_HATCH_LINES;
    }
    let step = if count > 1 { interval.max((y1 - y0) / count as f64) } else { interval };
    let mut out = Vec::new();
    let mut flip = false;
    let mut y = y0 + step * 0.5;
    while y < y1 {
        let mut xs: Vec<f64> = Vec::new();
        for p in &polys {
            let n = p.len();
            for i in 0..n {
                let (a, b) = (p[i], p[(i + 1) % n]);
                if (a.y > y) != (b.y > y) {
                    xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        let mut segs: Vec<(f64, f64)> = xs.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).filter(|(a, b)| b - a > 1e-9).collect();
        if flip {
            segs.reverse();
            for s in &mut segs {
                *s = (s.1, s.0);
            }
        }
        if !segs.is_empty() && bidirectional {
            flip = !flip;
        }
        out.extend(segs.into_iter().map(|(a, b)| vec![back * Point::new(a, y), back * Point::new(b, y)]));
        y += step;
    }
    out
}

/// Where the job starts, document px: its anchor point (the point placed
/// at the head / user origin), or for absolute placement the artboard
/// corner where the machine's home is. None when there's nothing to cut.
pub fn start_point(plan: &Plan, m: &MachineSettings) -> Option<Point> {
    if m.placement == Placement::Absolute {
        let f = plan.frame?;
        return Some(match m.origin {
            Corner::TopLeft => Point::new(f.x0, f.y0),
            Corner::TopRight => Point::new(f.x0 + m.bed_w / plan.mm_per_px, f.y0),
            Corner::BottomLeft => Point::new(f.x0, f.y0 + m.bed_h / plan.mm_per_px),
            Corner::BottomRight => Point::new(f.x0 + m.bed_w / plan.mm_per_px, f.y0 + m.bed_h / plan.mm_per_px),
        });
    }
    let b = plan_bounds(plan)?;
    Some(Point::new(b.x0 + b.width() * m.anchor.0 as f64 / 2.0, b.y0 + b.height() * m.anchor.1 as f64 / 2.0))
}

/// Bounds of all the plan's paths (document px).
pub fn plan_bounds(plan: &Plan) -> Option<Rect> {
    let mut pts = plan.layers.iter().flat_map(|l| l.paths.iter().flatten());
    let first = *pts.next()?;
    Some(pts.fold(Rect::from_points(first, first), |r, p| r.union_pt(*p)))
}

/// Bed mm (x right, y down from the bed's top-left) → machine µm (from the
/// home corner). Mirroring is its own inverse.
pub fn to_machine(m: &MachineSettings, p: (f64, f64)) -> (f64, f64) {
    let (x, y) = p;
    match m.origin {
        Corner::TopLeft => (x, y),
        Corner::TopRight => (m.bed_w - x, y),
        Corner::BottomLeft => (x, m.bed_h - y),
        Corner::BottomRight => (m.bed_w - x, m.bed_h - y),
    }
}

/// The job's paths in bed mm (before mirroring), placed per the settings.
/// `head`: the head's position in machine µm (for `CurrentPosition`);
/// `UserOrigin` uses the saved origin instead.
/// Paths in bed mm, with their layer's index.
pub type Placed = Vec<(usize, Vec<(f64, f64)>)>;

pub fn placed(plan: &Plan, m: &MachineSettings, head: Option<mtzd_ruida::Pt>) -> Result<Placed, String> {
    let k = plan.mm_per_px;
    let mut paths: Placed = plan
        .layers
        .iter()
        .enumerate()
        .flat_map(|(i, l)| l.paths.iter().map(move |p| (i, p.iter().map(|q| ((q.x - plan.origin.x) * k, (q.y - plan.origin.y) * k)).collect())))
        .collect();
    if m.placement != Placement::Absolute {
        let at = match m.placement {
            Placement::UserOrigin => m.user_origin.ok_or("Set the user origin first (Move tab › Set origin here)")?,
            _ => {
                let head = head.ok_or("The head position isn't known yet (connect first)")?;
                to_machine(m, (head.0 as f64 / 1000.0, head.1 as f64 / 1000.0))
            }
        };
        let (lo, hi) = bounds_mm(paths.iter().flat_map(|(_, p)| p.iter())).ok_or("Nothing to cut")?;
        let fx = m.anchor.0 as f64 / 2.0;
        let fy = m.anchor.1 as f64 / 2.0;
        let anchor = (lo.0 + (hi.0 - lo.0) * fx, lo.1 + (hi.1 - lo.1) * fy);
        let d = (at.0 - anchor.0, at.1 - anchor.1);
        for (_, p) in &mut paths {
            for q in p.iter_mut() {
                *q = (q.0 + d.0, q.1 + d.1);
            }
        }
    }
    Ok(paths)
}

pub fn bounds_mm<'a>(pts: impl Iterator<Item = &'a (f64, f64)>) -> Option<((f64, f64), (f64, f64))> {
    pts.fold(None, |b, p| match b {
        None => Some((*p, *p)),
        Some((lo, hi)) => Some(((lo.0.min(p.0), lo.1.min(p.1)), (hi.0.max(p.0), hi.1.max(p.1)))),
    })
}

/// The job for the controller (machine µm, power capped). Fails when the
/// job leaves the bed.
pub fn to_job(plan: &Plan, m: &MachineSettings, head: Option<mtzd_ruida::Pt>) -> Result<mtzd_ruida::Job, String> {
    let paths = placed(plan, m, head)?;
    if let Some((lo, hi)) = bounds_mm(paths.iter().flat_map(|(_, p)| p.iter())) {
        let e = 1e-6;
        if lo.0 < -e || lo.1 < -e || hi.0 > m.bed_w + e || hi.1 > m.bed_h + e {
            return Err(format!("The job ({:.1}, {:.1}) – ({:.1}, {:.1}) mm leaves the {:.0} × {:.0} mm bed", lo.0, lo.1, hi.0, hi.1, m.bed_w, m.bed_h));
        }
    }
    let cap = m.power_cap.clamp(0.0, 100.0);
    let um = |v: f64| (v * 1000.0).round() as i64;
    let mut layers: Vec<mtzd_ruida::JobLayer> = Vec::new();
    for (i, l) in plan.layers.iter().enumerate() {
        let lp: Vec<Vec<mtzd_ruida::Pt>> = paths
            .iter()
            .filter(|(j, _)| *j == i)
            .map(|(_, p)| {
                let mut q: Vec<mtzd_ruida::Pt> = p.iter().map(|q| to_machine(m, *q)).map(|(x, y)| (um(x), um(y))).collect();
                q.dedup();
                q
            })
            .filter(|p| p.len() >= 2)
            .collect();
        if lp.is_empty() {
            continue;
        }
        let s = &l.settings;
        layers.push(mtzd_ruida::JobLayer {
            rgb: l.rgb,
            speed: s.speed.clamp(0.5, 1000.0),
            min_power: s.min_power.clamp(0.0, cap),
            max_power: s.max_power.clamp(0.0, cap),
            air: s.air,
            passes: l.passes.clamp(1, 50),
            paths: lp,
        });
    }
    Ok(mtzd_ruida::Job { layers })
}

/// The frame of a job: its bounding box corners (machine µm), closed.
pub fn frame(job: &mtzd_ruida::Job) -> Option<Vec<mtzd_ruida::Pt>> {
    let ((x0, y0), (x1, y1)) = job.bounds()?;
    Some(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)])
}

/// A path in document px as a BezPath (for the overlay).
pub fn polyline(p: &[Point]) -> BezPath {
    let mut b = BezPath::new();
    if let Some(f) = p.first() {
        b.move_to(*f);
        for q in &p[1..] {
            b.line_to(*q);
        }
    }
    b
}
