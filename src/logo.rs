//! The app logo (logo.svg) and word-mark (word-mark.svg), built in and
//! rasterized at the exact pixel size they are shown at.

use egui::{Color32, TextureHandle, Ui, vec2};

const LOGO: &[u8] = include_bytes!("../logo.svg");
const WORD_MARK: &[u8] = include_bytes!("../word-mark.svg");

/// Parsed artwork and the region to show (document units).
struct Art {
    tree: usvg::Tree,
    view: usvg::Rect,
}

fn logo_art() -> Option<Art> {
    let tree = usvg::Tree::from_data(LOGO, &usvg::Options::default()).ok()?;
    let s = tree.size();
    Some(Art { view: usvg::Rect::from_xywh(0.0, 0.0, s.width(), s.height())?, tree })
}

/// The word-mark without its exported artboard background, cropped to the
/// letters.
fn word_mark_art() -> Option<Art> {
    let src = std::str::from_utf8(WORD_MARK).ok()?;
    let src: String = src.lines().filter(|l| !l.trim_start().starts_with("<rect")).collect::<Vec<_>>().join("\n");
    let tree = usvg::Tree::from_data(src.as_bytes(), &usvg::Options::default()).ok()?;
    let b = tree.root().abs_bounding_box();
    Some(Art { view: b, tree })
}

/// Premultiplied RGBA of `art` scaled to `h_px` pixels tall.
fn rasterize(art: &Art, h_px: u32) -> Option<(Vec<u8>, [usize; 2])> {
    let k = h_px.clamp(1, 1024) as f32 / art.view.height().max(1e-3);
    let (w, h) = ((art.view.width() * k).ceil() as u32, (art.view.height() * k).ceil() as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    let xf = resvg::tiny_skia::Transform::from_row(k, 0.0, 0.0, k, -art.view.x() * k, -art.view.y() * k);
    resvg::render(&art.tree, xf, &mut pixmap.as_mut());
    Some((pixmap.take(), [w.max(1) as usize, h.max(1) as usize]))
}

/// The logo as a `px`×`px` PNG (straight alpha), for desktop icons.
pub fn png(px: u32) -> Option<Vec<u8>> {
    let d = icon_data_px(px)?;
    let img = image::RgbaImage::from_raw(d.width, d.height, d.rgba)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Window / taskbar icon.
pub fn icon_data() -> Option<egui::IconData> {
    icon_data_px(256)
}

fn icon_data_px(px: u32) -> Option<egui::IconData> {
    let (mut rgba, [w, h]) = rasterize(&logo_art()?, px)?;
    // IconData is straight alpha.
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(egui::IconData { rgba, width: w as u32, height: h as u32 })
}

/// Texture of `art` for `height` points at the current pixel density (cached).
fn texture(ctx: &egui::Context, name: &str, art: fn() -> Option<Art>, height: f32) -> Option<(TextureHandle, egui::Vec2)> {
    let ppp = ctx.pixels_per_point();
    let px = (height * ppp).round() as u32;
    let id = egui::Id::new(("app_art", name, px));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        let s = t.size_vec2() / ppp;
        return Some((t, s));
    }
    let (rgba, size) = rasterize(&art()?, px)?;
    let img = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
    let tex = ctx.load_texture(format!("app-{name}-{px}"), img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    let s = tex.size_vec2() / ppp;
    Some((tex, s))
}

fn paint(ui: &mut Ui, name: &str, art: fn() -> Option<Art>, height: f32, tint: Color32) -> egui::Response {
    let Some((t, size)) = texture(ui.ctx(), name, art, height) else {
        return ui.allocate_response(vec2(height, height), egui::Sense::hover());
    };
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    // Snap to pixels so the raster maps 1:1.
    use egui::emath::GuiRounding;
    let rect = egui::Rect::from_min_size(rect.min.round_to_pixels(ui.pixels_per_point()), rect.size());
    ui.painter().image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
    resp
}

/// The logo as a `size`-point square widget.
pub fn show(ui: &mut Ui, size: f32) -> egui::Response {
    paint(ui, "logo", logo_art, size, Color32::WHITE)
}

/// The "mtzdVector" word-mark, `height` points tall, tinted like text.
pub fn word_mark(ui: &mut Ui, height: f32) -> egui::Response {
    paint(ui, "word_mark", word_mark_art, height, crate::ui::theme::TEXT)
}
