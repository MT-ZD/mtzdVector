//! DXF export (R12, the most widely read flavour): every outline of the
//! first artboard as a polyline in millimetres, one DXF layer per top-level
//! layer, coloured with the nearest AutoCAD colour index. For laser and
//! plotter work and CAD hand-off; fills, gradients and effects don't carry
//! over (text is outlined).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use kurbo::PathEl;

use crate::document::{Document, Rgba, Shape};
use crate::fonts::FontManager;

/// The 255 AutoCAD colour index entries (approximate RGB).
fn aci_palette() -> Vec<(u8, [f32; 3])> {
    let mut v: Vec<(u8, [f32; 3])> = vec![
        (1, [1.0, 0.0, 0.0]),
        (2, [1.0, 1.0, 0.0]),
        (3, [0.0, 1.0, 0.0]),
        (4, [0.0, 1.0, 1.0]),
        (5, [0.0, 0.0, 1.0]),
        (6, [1.0, 0.0, 1.0]),
        (7, [0.0, 0.0, 0.0]),
        (8, [0.5, 0.5, 0.5]),
        (9, [0.75, 0.75, 0.75]),
    ];
    // 10..249: 24 hues × 5 brightness levels × full / half saturation.
    let levels = [1.0, 0.65, 0.5, 0.3, 0.15];
    for i in 10..250u32 {
        let hue = ((i / 10 - 1) * 15) as f32;
        let k = (i % 10) as usize;
        let (v_, sat) = (levels[k / 2], if k % 2 == 0 { 1.0 } else { 0.5 });
        let c = v_ * sat;
        let h = hue / 60.0;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v_ - c;
        v.push((i as u8, [r + m, g + m, b + m]));
    }
    for (i, g) in [(250u8, 0.2), (251, 0.31), (252, 0.41), (253, 0.51), (254, 0.71), (255, 1.0)] {
        v.push((i, [g, g, g]));
    }
    v
}

/// Nearest colour index (7 is black/white by the viewer's background).
pub fn aci(c: Rgba) -> u8 {
    let pal = aci_palette();
    pal.iter()
        .min_by(|a, b| {
            let d = |p: &[f32; 3]| (p[0] - c.r).powi(2) + (p[1] - c.g).powi(2) + (p[2] - c.b).powi(2);
            d(&a.1).total_cmp(&d(&b.1))
        })
        .map(|p| p.0)
        .unwrap_or(7)
}

fn layer_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_uppercase() } else { '_' }).collect();
    if n.is_empty() { "0".into() } else { n.chars().take(31).collect() }
}

/// Top-level layer name of every shape id.
fn top_names(list: &[std::sync::Arc<Shape>]) -> BTreeMap<u64, String> {
    fn walk(s: &Shape, top: &str, out: &mut BTreeMap<u64, String>) {
        out.insert(s.id, top.to_string());
        for c in s.kind.children().into_iter().flatten() {
            walk(c, top, out);
        }
    }
    let mut out = BTreeMap::new();
    for s in list {
        walk(s, &layer_name(&s.name), &mut out);
    }
    out
}

fn pair(out: &mut String, code: i32, value: impl std::fmt::Display) {
    writeln!(out, "{code:>3}\n{value}").unwrap();
}

fn mm(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.into() }
}

/// DXF of the first artboard (at the origin).
pub fn export(doc: &Document, fonts: &mut FontManager) -> String {
    let doc = &*doc.at_origin();
    let items = super::prepare(doc, fonts);
    let k = 25.4 / doc.dpi.max(1.0);
    let h = doc.primary().h;
    let names = top_names(doc.shapes());
    // Polylines: (layer, colour, closed, points in mm).
    let mut polys: Vec<(String, u8, bool, Vec<(f64, f64)>)> = Vec::new();
    for it in &items {
        if it.span > 0 {
            continue;
        }
        let s: &Shape = &it.shape;
        let col = s.stroke.as_ref().filter(|st| st.width > 0.0).map(|st| st.color).or_else(|| s.fill.as_ref().and_then(|f| f.solid())).unwrap_or(Rgba::BLACK);
        let layer = names.get(&s.id).cloned().unwrap_or_else(|| "0".into());
        let mut cur: Vec<(f64, f64)> = Vec::new();
        let flush = |cur: &mut Vec<(f64, f64)>, closed: bool, polys: &mut Vec<(String, u8, bool, Vec<(f64, f64)>)>| {
            if cur.len() >= 2 {
                if closed && cur.len() > 2 && cur.first() == cur.last() {
                    cur.pop();
                }
                polys.push((layer.clone(), aci(col), closed, std::mem::take(cur)));
            }
            cur.clear();
        };
        kurbo::flatten(it.path.iter(), 0.05, |el| match el {
            PathEl::MoveTo(p) => {
                flush(&mut cur, false, &mut polys);
                cur.push((p.x * k, (h - p.y) * k));
            }
            PathEl::LineTo(p) => cur.push((p.x * k, (h - p.y) * k)),
            PathEl::ClosePath => flush(&mut cur, true, &mut polys),
            _ => {}
        });
        flush(&mut cur, false, &mut polys);
    }
    let mut layers: Vec<String> = polys.iter().map(|p| p.0.clone()).collect();
    layers.sort();
    layers.dedup();
    let mut out = String::new();
    // Header: R12, millimetres, extents.
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "HEADER");
    pair(&mut out, 9, "$ACADVER");
    pair(&mut out, 1, "AC1009");
    pair(&mut out, 9, "$INSUNITS");
    pair(&mut out, 70, 4);
    pair(&mut out, 9, "$MEASUREMENT");
    pair(&mut out, 70, 1);
    pair(&mut out, 9, "$EXTMIN");
    pair(&mut out, 10, 0);
    pair(&mut out, 20, 0);
    pair(&mut out, 9, "$EXTMAX");
    pair(&mut out, 10, mm(doc.primary().w * k));
    pair(&mut out, 20, mm(h * k));
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "TABLES");
    pair(&mut out, 0, "TABLE");
    pair(&mut out, 2, "LAYER");
    pair(&mut out, 70, layers.len() + 1);
    for name in std::iter::once("0".to_string()).chain(layers.iter().filter(|l| *l != "0").cloned()) {
        pair(&mut out, 0, "LAYER");
        pair(&mut out, 2, name);
        pair(&mut out, 70, 0);
        pair(&mut out, 62, 7);
        pair(&mut out, 6, "CONTINUOUS");
    }
    pair(&mut out, 0, "ENDTAB");
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "ENTITIES");
    for (layer, color, closed, pts) in &polys {
        pair(&mut out, 0, "POLYLINE");
        pair(&mut out, 8, layer);
        pair(&mut out, 62, color);
        pair(&mut out, 66, 1);
        pair(&mut out, 70, if *closed { 1 } else { 0 });
        for (x, y) in pts {
            pair(&mut out, 0, "VERTEX");
            pair(&mut out, 8, layer);
            pair(&mut out, 10, mm(*x));
            pair(&mut out, 20, mm(*y));
        }
        pair(&mut out, 0, "SEQEND");
        pair(&mut out, 8, layer);
    }
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "EOF");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Paint, ShapeKind};

    #[test]
    fn rectangle_becomes_closed_polyline_in_mm() {
        let mut d = Document::default();
        d.artboards[0].w = 96.0;
        d.artboards[0].h = 96.0;
        let mut s = Shape::new(ShapeKind::Rect { w: 48.0, h: 24.0, radii: [0.0; 4] });
        s.name = "Cut line".into();
        s.fill = Some(Paint::Solid(Rgba::from_hex("#ff0000").unwrap()));
        d.add_shape(s);
        let mut f = FontManager::new(egui::Context::default());
        let dxf = export(&d, &mut f);
        assert!(dxf.contains("AC1009") && dxf.trim_end().ends_with("EOF"));
        assert!(dxf.contains("CUT_LINE"));
        assert_eq!(dxf.matches("\nPOLYLINE\n").count(), 1);
        // 48 px at 96 dpi = 12.7 mm; top edge at y = (96 - 0) px = 25.4 mm.
        assert!(dxf.contains("\n 10\n12.7\n"), "{dxf}");
        assert!(dxf.contains("\n 20\n25.4\n"));
        // Red → index 1.
        assert!(dxf.contains(" 62\n1\n"));
        assert_eq!(aci(Rgba::from_hex("#00ff00").unwrap()), 3);
    }
}
