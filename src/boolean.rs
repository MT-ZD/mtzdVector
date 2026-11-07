//! Boolean operations (via i_overlay) and Shape Builder region arrangement.

use i_overlay::core::fill_rule::FillRule as IFill;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use kurbo::{BezPath, Point, Rect};

use crate::document::FillRule;
use crate::geom;

/// A set of contours (outer boundaries and holes), even-odd filled.
pub type Polys = Vec<Vec<[f64; 2]>>;

pub const FLATTEN_TOL: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BoolOp {
    Union,
    Subtract,
    Intersect,
    Exclude,
}

fn ifill(rule: FillRule) -> IFill {
    match rule {
        FillRule::NonZero => IFill::NonZero,
        FillRule::EvenOdd => IFill::EvenOdd,
    }
}

/// Normalize a path into non-overlapping even-odd contours.
pub fn path_to_polys(path: &BezPath, rule: FillRule) -> Polys {
    let polys = geom::flatten_polys(path, FLATTEN_TOL);
    if polys.is_empty() {
        return polys;
    }
    let empty: Polys = Vec::new();
    flatten_shapes(polys.overlay(&empty, OverlayRule::Subject, ifill(rule)))
}

fn flatten_shapes(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> Polys {
    shapes.into_iter().flatten().filter(|c| c.len() >= 3).collect()
}

pub fn op(a: &Polys, b: &Polys, op: BoolOp) -> Polys {
    if a.is_empty() {
        return match op {
            BoolOp::Union | BoolOp::Exclude => b.clone(),
            _ => Vec::new(),
        };
    }
    if b.is_empty() {
        return match op {
            BoolOp::Intersect => Vec::new(),
            _ => a.clone(),
        };
    }
    let rule = match op {
        BoolOp::Union => OverlayRule::Union,
        BoolOp::Subtract => OverlayRule::Difference,
        BoolOp::Intersect => OverlayRule::Intersect,
        BoolOp::Exclude => OverlayRule::Xor,
    };
    flatten_shapes(a.overlay(b, rule, IFill::EvenOdd))
}

/// Split polys into connected pieces (each outer contour plus its holes).
pub fn split_pieces(p: &Polys) -> Vec<Polys> {
    if p.is_empty() {
        return Vec::new();
    }
    let empty: Polys = Vec::new();
    p.overlay(&empty, OverlayRule::Subject, IFill::EvenOdd)
        .into_iter()
        .map(|s| s.into_iter().filter(|c| c.len() >= 3).collect::<Polys>())
        .filter(|s: &Polys| !s.is_empty())
        .collect()
}

pub fn area(p: &Polys) -> f64 {
    // Even-odd: outer contours and holes have opposite orientation in i_overlay output.
    p.iter().map(|c| geom::poly_area(c)).sum::<f64>().abs()
}

pub fn bounds(p: &Polys) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for c in p {
        for pt in c {
            let q = Rect::new(pt[0], pt[1], pt[0], pt[1]);
            r = geom::union_rect(r, q);
        }
    }
    r
}

/// One cell of the shape arrangement.
#[derive(Clone)]
pub struct Region {
    pub polys: Polys,
    /// Index (in the input list) of the top-most shape covering this region.
    pub owner: usize,
    pub bounds: Rect,
    pub path: BezPath,
}

/// Compute all distinct regions formed by overlapping the given shapes
/// (bottom-most first).
pub fn arrangement(shapes: &[Polys]) -> Vec<Region> {
    let mut cells: Vec<(Polys, usize)> = Vec::new();
    let mut covered: Polys = Vec::new();
    for (k, s) in shapes.iter().enumerate() {
        if s.is_empty() {
            continue;
        }
        let mut next = Vec::with_capacity(cells.len() * 2);
        for (cell, owner) in cells.drain(..) {
            let inter = op(&cell, s, BoolOp::Intersect);
            let diff = op(&cell, s, BoolOp::Subtract);
            if !inter.is_empty() {
                next.push((inter, k));
            }
            if !diff.is_empty() {
                next.push((diff, owner));
            }
        }
        let fresh = op(s, &covered, BoolOp::Subtract);
        if !fresh.is_empty() {
            next.push((fresh, k));
        }
        covered = op(&covered, s, BoolOp::Union);
        cells = next;
    }
    let mut out = Vec::new();
    for (cell, owner) in cells {
        for piece in split_pieces(&cell) {
            if area(&piece) < 0.05 {
                continue;
            }
            let Some(b) = bounds(&piece) else { continue };
            let path = geom::polys_to_bez(&piece);
            out.push(Region { polys: piece, owner, bounds: b, path });
        }
    }
    out
}

pub fn region_at(regions: &[Region], p: Point) -> Option<usize> {
    // Prefer the smallest region containing the point.
    let mut best: Option<(usize, f64)> = None;
    for (i, r) in regions.iter().enumerate() {
        if r.bounds.contains(p) && geom::point_in_polys(&r.polys, p) {
            let a = r.bounds.area();
            if best.map(|b| a < b.1).unwrap_or(true) {
                best = Some((i, a));
            }
        }
    }
    best.map(|b| b.0)
}

pub fn union_all(parts: &[&Polys]) -> Polys {
    let mut acc: Polys = Vec::new();
    for p in parts {
        acc = op(&acc, p, BoolOp::Union);
    }
    acc
}
