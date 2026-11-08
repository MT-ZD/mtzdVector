//! EPS export (hand-written PostScript) and import (via Ghostscript → PDF).

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use kurbo::{BezPath, PathEl};

use super::{num, prepare};
use crate::document::{Document, FillRule, ImageMode, LineCap, LineJoin, Paint};
use crate::fonts::FontManager;
use crate::render;

fn ps_path(out: &mut String, path: &BezPath) {
    out.push_str("newpath\n");
    super::for_each_el(path, |el, last| {
        match *el {
            PathEl::MoveTo(p) => writeln!(out, "{} {} m", num(p.x), num(p.y)),
            PathEl::LineTo(p) => writeln!(out, "{} {} l", num(p.x), num(p.y)),
            PathEl::QuadTo(q, p) => {
                let p0 = last.unwrap_or(q);
                let c1 = p0 + (q - p0) * (2.0 / 3.0);
                let c2 = p + (q - p) * (2.0 / 3.0);
                writeln!(out, "{} {} {} {} {} {} c", num(c1.x), num(c1.y), num(c2.x), num(c2.y), num(p.x), num(p.y))
            }
            PathEl::CurveTo(a, b, p) => {
                writeln!(out, "{} {} {} {} {} {} c", num(a.x), num(a.y), num(b.x), num(b.y), num(p.x), num(p.y))
            }
            PathEl::ClosePath => writeln!(out, "h"),
        }
        .unwrap();
    });
}

/// EPS of the first artboard.
pub fn export(doc: &Document, fonts: &mut FontManager) -> String {
    let doc = &*doc.at_origin();
    let items = prepare(doc, fonts);
    let k = 72.0 / doc.dpi.max(1.0);
    let (w, h) = (doc.primary().w, doc.primary().h);
    let mut o = String::new();
    writeln!(o, "%!PS-Adobe-3.0 EPSF-3.0").unwrap();
    writeln!(o, "%%BoundingBox: 0 0 {} {}", (w * k).ceil() as i64, (h * k).ceil() as i64).unwrap();
    writeln!(o, "%%HiResBoundingBox: 0 0 {} {}", num(w * k), num(h * k)).unwrap();
    writeln!(o, "%%Creator: mtzd-vector").unwrap();
    // Gradients use smooth shading (`shfill`), a Level 3 feature.
    let level = if items.iter().any(|i| matches!(i.shape.fill, Some(Paint::Gradient(_)))) { 3 } else { 2 };
    writeln!(o, "%%LanguageLevel: {level}").unwrap();
    writeln!(o, "%%Pages: 1").unwrap();
    writeln!(o, "%%EndComments").unwrap();
    writeln!(o, "%%BeginProlog").unwrap();
    writeln!(o, "/m {{moveto}} bind def /l {{lineto}} bind def /c {{curveto}} bind def /h {{closepath}} bind def").unwrap();
    writeln!(o, "%%EndProlog").unwrap();
    writeln!(o, "%%Page: 1 1").unwrap();
    writeln!(o, "gsave").unwrap();
    writeln!(o, "0 {} translate {} {} scale", num(h * k), num(k), num(-k)).unwrap();
    if doc.primary().background.a > 0.0 {
        let b = doc.primary().background;
        writeln!(o, "{} {} {} setrgbcolor 0 0 {} {} rectfill", b.r, b.g, b.b, num(w), num(h)).unwrap();
    }
    for item in &items {
        let s: &crate::document::Shape = &item.shape;
        // Group effects aren't expressible in EPS; the members draw below.
        if item.path.elements().is_empty() || item.span > 0 {
            continue;
        }
        let even_odd = s.fill_rule == FillRule::EvenOdd;
        match &s.fill {
            Some(Paint::Solid(c)) => {
                ps_path(&mut o, &item.path);
                writeln!(o, "{} {} {} setrgbcolor {}", c.r, c.g, c.b, if even_odd { "eofill" } else { "fill" }).unwrap();
            }
            Some(Paint::Gradient(g)) if g.is_baked() => {
                // No conic or mesh shadings here: the average colour.
                let c = g.average();
                ps_path(&mut o, &item.path);
                writeln!(o, "{} {} {} setrgbcolor {}", c.r, c.g, c.b, if even_odd { "eofill" } else { "fill" }).unwrap();
            }
            Some(Paint::Gradient(g)) => {
                // PostScript 3 smooth shading, in the shape's bounding-box units.
                o.push_str("gsave\n");
                ps_path(&mut o, &item.path);
                writeln!(o, "{}", if even_odd { "eoclip" } else { "clip" }).unwrap();
                let lb = render::shape_local_bounds(s, &item.path);
                let m = s.transform * kurbo::Affine::new([lb.width().max(1e-9), 0.0, 0.0, lb.height().max(1e-9), lb.x0, lb.y0]);
                let c = m.as_coeffs();
                writeln!(o, "[{} {} {} {} {} {}] concat", num(c[0]), num(c[1]), num(c[2]), num(c[3]), num(c[4]), num(c[5])).unwrap();
                let stops = g.padded_stops();
                // No transparency in PostScript: composite stops against white.
                let rgb = |c: crate::document::Rgba| {
                    let w = |v: f32| v * c.a + (1.0 - c.a);
                    format!("[{} {} {}]", w(c.r), w(c.g), w(c.b))
                };
                let funcs: Vec<String> = stops.windows(2).map(|w| format!("<< /FunctionType 2 /Domain [0 1] /C0 {} /C1 {} /N 1 >>", rgb(w[0].color), rgb(w[1].color))).collect();
                let bounds: Vec<String> = stops[1..stops.len() - 1].iter().map(|s| s.offset.to_string()).collect();
                let encode = vec!["0 1"; funcs.len()].join(" ");
                let [sx, sy] = g.start;
                let [ex, ey] = g.end;
                let (ty, coords) = match g.kind {
                    crate::document::GradientKind::Linear => (2, format!("{} {} {} {}", num(sx), num(sy), num(ex), num(ey))),
                    _ => (3, format!("{} {} 0 {} {} {}", num(sx), num(sy), num(sx), num(sy), num(((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt()))),
                };
                writeln!(
                    o,
                    "<< /ShadingType {ty} /ColorSpace /DeviceRGB /Coords [{coords}] /Extend [true true] /Function << /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{encode}] >> >> shfill",
                    funcs.join(" "),
                    bounds.join(" ")
                )
                .unwrap();
                o.push_str("grestore\n");
            }
            Some(Paint::Image(f)) => {
                if let Some(asset) = doc.images.get(&f.image) {
                    if let Some(rgba) = &asset.rgba {
                        o.push_str("gsave\n");
                        ps_path(&mut o, &item.path);
                        writeln!(o, "{}", if even_odd { "eoclip" } else { "clip" }).unwrap();
                        let c = s.transform.as_coeffs();
                        writeln!(o, "[{} {} {} {} {} {}] concat", num(c[0]), num(c[1]), num(c[2]), num(c[3]), num(c[4]), num(c[5])).unwrap();
                        let lb = render::shape_local_bounds(s, &item.path);
                        let (iw, ih) = (asset.width as f64, asset.height as f64);
                        let r = if f.mode == ImageMode::Tile { lb } else { render::image_rect(f.mode, lb, iw, ih) };
                        // Composite alpha against white (PostScript has no transparency).
                        let mut hex = String::with_capacity(rgba.len() * 2);
                        for (i, p) in rgba.chunks(4).enumerate() {
                            let a = p[3] as f32 / 255.0 * f.opacity;
                            for ch in &p[..3] {
                                let v = (*ch as f32 * a + 255.0 * (1.0 - a)).round() as u8;
                                write!(hex, "{v:02x}").unwrap();
                            }
                            if i % 12 == 11 {
                                hex.push('\n');
                            }
                        }
                        writeln!(o, "{} {} translate {} {} scale", num(r.x0), num(r.y0), num(r.width()), num(r.height())).unwrap();
                        writeln!(o, "/picstr {} string def", asset.width * 3).unwrap();
                        writeln!(
                            o,
                            "{} {} 8 [{} 0 0 {} 0 0] {{currentfile picstr readhexstring pop}} false 3 colorimage",
                            asset.width, asset.height, asset.width, asset.height
                        )
                        .unwrap();
                        o.push_str(&hex);
                        o.push_str("\ngrestore\n");
                    }
                }
            }
            None => {}
        }
        if let Some(st) = &s.stroke {
            if st.width > 0.0 {
              for part in crate::strokes::stroke_parts(&item.path, st, s.fill_rule) {
                let l = match part {
                    crate::strokes::StrokePart::Fill(f, rule) => {
                        ps_path(&mut o, &f);
                        let op = if rule == FillRule::EvenOdd { "eofill" } else { "fill" };
                        writeln!(o, "{} {} {} setrgbcolor {op}", st.color.r, st.color.g, st.color.b).unwrap();
                        continue;
                    }
                    crate::strokes::StrokePart::Line(l) => l,
                };
                ps_path(&mut o, &l);
                let join = match st.join {
                    LineJoin::Miter => 0,
                    LineJoin::Round => 1,
                    LineJoin::Bevel => 2,
                };
                let cap = match st.cap {
                    LineCap::Butt => 0,
                    LineCap::Round => 1,
                    LineCap::Square => 2,
                };
                writeln!(
                    o,
                    "{} {} {} setrgbcolor {} setlinewidth {} setlinejoin {} setlinecap stroke",
                    st.color.r,
                    st.color.g,
                    st.color.b,
                    num(st.width),
                    join,
                    cap
                )
                .unwrap();
              }
            }
        }
    }
    writeln!(o, "grestore").unwrap();
    writeln!(o, "showpage").unwrap();
    writeln!(o, "%%EOF").unwrap();
    o
}

/// Import EPS by converting it to PDF with Ghostscript, then parsing the PDF.
pub fn import(path: &Path) -> Result<Document, String> {
    // A private directory: a predictable name in /tmp could be pre-created
    // (or symlinked) by another user.
    let dir = private_temp_dir()?;
    let tmp = dir.join("import.pdf");
    let out = Command::new("gs")
        .args(["-q", "-dNOPAUSE", "-dBATCH", "-dSAFER", "-dEPSCrop", "-dNoOutputFonts", "-sDEVICE=pdfwrite"])
        .arg(format!("-sOutputFile={}", tmp.display()))
        .arg(path)
        .output()
        .map_err(|e| format!("EPS import needs Ghostscript (`gs`) installed: {e}"))?;
    if !out.status.success() {
        return Err(format!("Ghostscript failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let doc = super::pdf::import(&tmp);
    let _ = std::fs::remove_dir_all(&dir);
    doc
}

/// A new directory only this user can access.
pub(super) fn private_temp_dir() -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::temp_dir();
    for i in 0..100u32 {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = base.join(format!("mtzd-vector-{}-{nanos:x}-{i}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("could not create a temporary directory".into())
}
