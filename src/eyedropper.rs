//! Eyedropper tool (I): click a shape (or the artboard) to copy its colour
//! onto the selection's fill — or its stroke with Shift, or when the colour
//! panel targets the stroke. Returns to the previous tool afterwards.

use egui::{Color32, Stroke, pos2, vec2};
use kurbo::{Point, Shape as _};

use crate::app::{App, Tool};
use crate::document::{Paint, Rgba, ShapeKind, Stroke as DStroke};
use crate::ui::widgets::WellTarget;

impl App {
    /// The colour visible at `p`: every shape covering it (fill, stroke,
    /// outline rings, image pixels, gradients), blended bottom to top with
    /// its opacity over the artboard background, like on screen.
    pub fn sample_color(&mut self, p: Point) -> Option<Rgba> {
        let mut layers = Vec::new(); // top-most first
        for top in self.hit_shapes(p, false) {
            let mut id = top;
            while let Some(c) = self.hit_child(id, p) {
                id = c;
            }
            if let Some((c, opacity)) = self.sample_shape(id, p) {
                layers.push(Rgba { a: c.a * opacity, ..c });
                if layers.last().map(|c| c.a >= 0.999).unwrap_or(false) {
                    break; // opaque: nothing below shows
                }
            }
        }
        let opaque_top = layers.last().map(|c| c.a >= 0.999).unwrap_or(false);
        let base = if opaque_top { None } else { self.doc.artboard_at(p).map(|a| a.background) };
        if layers.is_empty() {
            return base;
        }
        // Premultiplied "over", bottom to top.
        let mut acc = base.map(|c| [c.r * c.a, c.g * c.a, c.b * c.a, c.a]).unwrap_or([0.0; 4]);
        for c in layers.iter().rev() {
            let a = c.a.clamp(0.0, 1.0);
            acc = [c.r * a + acc[0] * (1.0 - a), c.g * a + acc[1] * (1.0 - a), c.b * a + acc[2] * (1.0 - a), a + acc[3] * (1.0 - a)];
        }
        let a = acc[3];
        if a <= 1e-6 {
            return base;
        }
        Some(Rgba { r: (acc[0] / a).clamp(0.0, 1.0), g: (acc[1] / a).clamp(0.0, 1.0), b: (acc[2] / a).clamp(0.0, 1.0), a })
    }

    /// The colour a shape paints at `p` and the opacity it is drawn with
    /// (its own and its groups'), or None where it doesn't cover `p`.
    fn sample_shape(&mut self, mut id: u64, p: Point) -> Option<(Rgba, f32)> {
        // A boolean's children are painted with the boolean's style.
        while let Some(parent) = self.doc.parent_of(id) {
            if matches!(self.doc.get(parent).map(|s| &s.kind), Some(ShapeKind::Compound { .. })) {
                id = parent;
            } else {
                break;
            }
        }
        let s = self.doc.get(id)?.clone();
        let mut opacity = s.opacity;
        let mut up = self.doc.parent_of(id);
        while let Some(g) = up {
            opacity *= self.doc.get(g).map(|g| g.opacity).unwrap_or(1.0);
            up = self.doc.parent_of(g);
        }
        let c = self.cache.update(&s, &mut self.fonts);
        let path = c.doc_path.clone();
        let w = path.winding(p);
        let in_fill = match (&s.kind, s.fill_rule) {
            (ShapeKind::Text(_), _) | (_, crate::document::FillRule::NonZero) => w != 0,
            (_, crate::document::FillRule::EvenOdd) => w % 2 != 0,
        };
        // Stroke on top of the fill.
        if let Some(st) = s.stroke.as_ref().filter(|st| st.width > 0.0) {
            let d = crate::geom::distance_to_path(&path, p);
            let on = match st.align {
                crate::document::StrokeAlign::Center => d <= st.width / 2.0,
                crate::document::StrokeAlign::Inside => in_fill && d <= st.width,
                crate::document::StrokeAlign::Outside => !in_fill && d <= st.width,
            };
            if on {
                return Some((st.color, opacity));
            }
        }
        if in_fill {
            match &s.fill {
                Some(Paint::Image(img)) => {
                    // Transparent pixels (or outside a Fit image) show what is below.
                    let img = img.clone();
                    return self.image_pixel(&s, &img, p).filter(|c| c.a > 0.02).map(|c| (c, opacity));
                }
                Some(Paint::Gradient(g)) => {
                    let gs = crate::render::GradSampler::new(&s, crate::render::shape_local_bounds(&s, &path), g, 1.0);
                    return Some((g.color_at(g.t_at(gs.unit(p))), opacity));
                }
                Some(f) => return f.representative().map(|c| (c, opacity)),
                None => {}
            }
        }
        // Outline rings around the shape.
        let rings = self.cache.get(id).map(|c| c.rings.clone()).unwrap_or_default();
        for (rp, paint) in rings.iter() {
            if rp.winding(p) % 2 != 0 {
                return match paint {
                    Paint::Gradient(g) => {
                        let gs = crate::render::GradSampler::new(&s, crate::render::ring_bounds(&s, rp), g, 1.0);
                        Some((g.color_at(g.t_at(gs.unit(p))), opacity))
                    }
                    other => other.representative().map(|c| (c, opacity)),
                };
            }
        }
        None
    }

    /// The image-fill pixel under doc point `p` (nearest pixel).
    fn image_pixel(&mut self, s: &crate::document::Shape, img: &crate::document::ImageFill, p: Point) -> Option<Rgba> {
        let asset = self.doc.images.get(&img.image)?;
        let rgba = asset.rgba.as_ref()?;
        let (w, h) = (asset.width as usize, asset.height as usize);
        if w == 0 || h == 0 {
            return None;
        }
        let path = &self.cache.get(s.id)?.doc_path;
        let lb = crate::render::shape_local_bounds(s, path);
        let lp = s.transform.inverse() * p;
        let mut uv = crate::render::image_uv(img.mode, img.scale as f64, lb, w as f64, h as f64, lp);
        if img.mode == crate::document::ImageMode::Tile {
            uv = Point::new(uv.x.rem_euclid(1.0), uv.y.rem_euclid(1.0));
        }
        if !(0.0..=1.0).contains(&uv.x) || !(0.0..=1.0).contains(&uv.y) {
            return None;
        }
        let x = ((uv.x * w as f64) as usize).min(w - 1);
        let y = ((uv.y * h as f64) as usize).min(h - 1);
        let px = rgba.get((y * w + x) * 4..(y * w + x) * 4 + 4)?;
        Some(Rgba {
            r: px[0] as f32 / 255.0,
            g: px[1] as f32 / 255.0,
            b: px[2] as f32 / 255.0,
            a: px[3] as f32 / 255.0 * img.opacity,
        })
    }

    /// Apply a picked colour to the selection (fill, or stroke).
    pub fn apply_picked(&mut self, c: Rgba, to_stroke: bool) {
        let targets = self.style_targets();
        if targets.is_empty() {
            self.egui_ctx.copy_text(c.to_hex());
            self.notify(format!("Copied {}", c.to_hex()));
            return;
        }
        self.checkpoint("");
        for id in targets {
            if let Some(s) = self.doc.get_mut(id) {
                if to_stroke {
                    match &mut s.stroke {
                        Some(st) => st.color = c,
                        None => s.stroke = Some(DStroke { color: c, width: 1.0, ..Default::default() }),
                    }
                } else {
                    s.fill = Some(Paint::Solid(c));
                }
            }
        }
    }

    pub fn eyedropper_click(&mut self, p: Point, shift: bool) {
        let Some(c) = self.sample_color(p) else { return };
        let to_stroke = shift || self.ui.color_target == WellTarget::Stroke;
        self.apply_picked(c, to_stroke);
        let back = self.prev_tool.filter(|t| *t != Tool::Eyedropper).unwrap_or(Tool::Select);
        self.set_tool(back);
    }

    /// Loupe next to the cursor with the colour under it.
    pub fn paint_eyedropper(&mut self, painter: &egui::Painter) {
        let Some(p) = self.cursor_doc else { return };
        let Some(c) = self.sample_color(p) else { return };
        let sp = self.view.to_screen(p);
        let col = Color32::from_rgba_unmultiplied((c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8, 255);
        let center = sp + vec2(22.0, -22.0);
        painter.circle_filled(center, 13.0, Color32::from_black_alpha(90));
        painter.circle_filled(center, 11.0, col);
        painter.circle_stroke(center, 11.0, Stroke::new(1.5, Color32::WHITE));
        crate::snap::label(painter, pos2(center.x, center.y + 22.0), &c.to_hex(), Color32::from_rgb(0x2B, 0x2B, 0x2B));
    }
}
