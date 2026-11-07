//! Fill and artboard-background sections of the Design panel, and the
//! paint editor (solid / gradient / image) they open inline.

use egui::{RichText, Ui};
use egui_phosphor::regular as icon;

use super::{design, edit_style};
use crate::app::App;
use crate::document::{Gradient, GradientKind, ImageFill, ImageMode, Paint, Rgba};
use crate::ui::theme::{self, TEXT_DIM};
use crate::ui::widgets::{self, Num, WellPaint, WellTarget};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    None,
    Solid,
    Gradient,
    Image,
}

pub fn paint_of(p: Option<&Paint>) -> WellPaint {
    match p {
        None => WellPaint::None,
        Some(Paint::Solid(c)) => WellPaint::Solid(*c),
        Some(Paint::Image(_)) => WellPaint::Image,
        Some(Paint::Gradient(g)) => WellPaint::Gradient(g.stops.first().map(|s| s.color).unwrap_or(Rgba::BLACK), g.stops.last().map(|s| s.color).unwrap_or(Rgba::WHITE)),
    }
}

fn paint_label(p: &Paint) -> String {
    match p {
        Paint::Solid(c) => c.with_alpha(1.0).to_hex(),
        Paint::Image(_) => "Image".into(),
        Paint::Gradient(g) => match g.kind {
            GradientKind::Linear => "Linear".into(),
            GradientKind::Radial => "Radial".into(),
            GradientKind::Conic => "Conic".into(),
            GradientKind::Mesh => "Mesh".into(),
        },
    }
}

/// Name of a linked colour key, for paint rows.
pub fn key_label(app: &App, key: Option<u64>) -> Option<String> {
    key.and_then(|k| app.doc.color_key(k)).map(|k| k.name.clone())
}

/// Alpha of a colour as a percent field (trailing in paint rows).
pub fn alpha_field(ui: &mut Ui, c: &mut Rgba) {
    let mut a = c.a as f64 * 100.0;
    if widgets::tip(widgets::num_field(ui, "", &mut a, Num::default().range(0.0, 100.0).suffix("%").decimals(0), 56.0), "Opacity", None).changed() {
        c.a = (a / 100.0) as f32;
    }
}

/// Fill section: a paint row per fill (top-most first); the swatch opens
/// the paint editor inline. "+" adds a fill over the others.
pub fn fill_section(app: &mut App, ui: &mut Ui) {
    let mut pick = false;
    edit_style(app, ui, |app, ui, p| {
        let has = p.fill.is_some();
        let mut add = false;
        let mut remove: Option<usize> = None;
        widgets::section_icon(
            ui,
            "fill",
            icon::PAINT_BUCKET,
            "Fill",
            |ui| {
                add = widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), if has { "Add a fill over this one" } else { "Add fill" }, None).clicked();
                pick = design::eyedropper_button(ui, has, "Pick the fill colour from the canvas");
            },
            |ui| {
                if p.fill.is_none() && p.fills.is_empty() {
                    return;
                }
                let layers = 1 + p.fills.len();
                for k in (0..layers).rev() {
                    // k = 0: the main fill (follows colour keys, has canvas handles).
                    let mut slot = if k == 0 { p.fill.take() } else { Some(p.fills[k - 1].clone()) };
                    let Some(fill) = slot.as_mut() else {
                        continue;
                    };
                    let key = if k == 0 { "fill".to_string() } else { format!("fill{k}") };
                    let label = if k == 0 { key_label(app, p.fill_key).unwrap_or_else(|| paint_label(fill)) } else { paint_label(fill) };
                    let mut del = false;
                    let open = design::paint_row(app, ui, &key, paint_of(Some(fill)), &label, |ui| {
                        if layers > 1 || k == 0 {
                            del = widgets::tip(widgets::mini_icon(ui, icon::MINUS, TEXT_DIM), "Remove fill", None).clicked();
                        }
                        if let Paint::Solid(c) = fill {
                            alpha_field(ui, c);
                        }
                    });
                    if del {
                        remove = Some(k);
                    }
                    if open {
                        app.ui.color_target = WellTarget::Fill;
                        if k == 0 && matches!(fill, Paint::Gradient(_)) && app.ui.gradient_edit.is_none() {
                            app.ui.gradient_edit = app.selection.first().copied();
                        }
                        let fill_key = if k == 0 { Some(&mut p.fill_key) } else { None };
                        design::editor_frame(ui, |ui| paint_body(app, ui, &mut slot, fill_key, k));
                    }
                    if k == 0 {
                        p.fill = slot;
                    } else if let Some(f) = slot {
                        p.fills[k - 1] = f;
                    }
                }
            },
        );
        if add {
            if p.fill.is_none() {
                p.fill = Some(Paint::Solid(Rgba::from_hex("#D9D9D9").unwrap()));
            } else {
                p.fills.push(Paint::Solid(Rgba::from_hex("#000000").unwrap().with_alpha(0.25)));
                app.ui.paint_open = Some(format!("fill{}", p.fills.len()));
            }
        }
        if let Some(k) = remove {
            if k == 0 {
                p.fill = if p.fills.is_empty() { None } else { Some(p.fills.remove(0)) };
                p.fill_key = None;
            } else if k <= p.fills.len() {
                p.fills.remove(k - 1);
            }
            app.ui.paint_open = None;
        }
    });
    if pick {
        app.ui.color_target = WellTarget::Fill;
        app.set_tool(crate::app::Tool::Eyedropper);
    }
}

/// Artboard background colour (no selection).
pub fn background_section(app: &mut App, ui: &mut Ui) {
    widgets::section_icon(ui, "background", icon::PALETTE, "Background", |_| {}, |ui| {
        let mut c = app.active().background;
        let label = if c.a <= 0.0 { "Transparent".to_string() } else { c.with_alpha(1.0).to_hex() };
        let open = design::paint_row(app, ui, "background", WellPaint::Solid(c), &label, |ui| {
            alpha_field(ui, &mut c);
            let clear = c.a <= 0.0;
            let r = widgets::icon_button_sized(ui, icon::CHECKERBOARD, true, clear, egui::vec2(26.0, 24.0), 14.0);
            if widgets::tip(r, if clear { "Make opaque" } else { "Transparent background" }, None).clicked() {
                c.a = if clear { 1.0 } else { 0.0 };
            }
        });
        if open {
            design::editor_frame(ui, |ui| {
                design::picker(app, ui, &mut c, egui::Id::new("bg_picker"));
            });
        }
        if c != app.active().background {
            app.checkpoint("artboard-bg");
            app.active_mut().background = c;
        }
    });
}

/// Editor for one fill: kind, then the colour picker, gradient editor,
/// or image settings. `k` is the fill's index (0 = main fill).
fn paint_body(app: &mut App, ui: &mut Ui, slot: &mut Option<Paint>, key: Option<&mut Option<u64>>, k: usize) {
    let mut kind = match slot {
        None => Kind::None,
        Some(Paint::Solid(_)) => Kind::Solid,
        Some(Paint::Image(_)) => Kind::Image,
        Some(Paint::Gradient(_)) => Kind::Gradient,
    };
    let before = kind;
    widgets::segmented_width(ui, &mut kind, &[(Kind::Solid, "Solid"), (Kind::Gradient, "Gradient"), (Kind::Image, "Image")], ui.available_width());
    if kind != before {
        // Keep the current colour where possible.
        let current = slot.as_ref().and_then(|f| f.representative()).unwrap_or(Rgba::from_hex("#D9D9D9").unwrap());
        match kind {
            Kind::None => *slot = None,
            Kind::Solid => *slot = Some(Paint::Solid(current)),
            Kind::Gradient => {
                // Like Figma: the colour fading out.
                *slot = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, current.with_alpha(1.0), current.with_alpha(0.0))));
                app.ui.gradient_stop = 0;
            }
            Kind::Image => {
                if let Some(p) = app.pick_image(slot.as_ref()) {
                    *slot = Some(p);
                }
            }
        }
    }
    ui.add_space(6.0);
    let picker_id = egui::Id::new(("fill_picker", k));
    match slot.as_mut() {
        None => {}
        Some(Paint::Solid(c)) => match key {
            Some(key) => {
                design::picker_linked(app, ui, c, Some(key), picker_id);
            }
            None => {
                design::picker(app, ui, c, picker_id);
            }
        },
        Some(Paint::Gradient(g)) => {
            let r = ui.scope(|ui| super::gradient::editor(app, ui, g)).response;
            let area = r.rect.intersect(ui.clip_rect());
            let pressed = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().map(|p| area.contains(p)).unwrap_or(false));
            if k == 0 && (pressed || kind != before) {
                app.ui.gradient_edit = app.selection.first().copied();
            }
        }
        Some(Paint::Image(f)) => {
            if let Some(asset) = app.doc.images.get(&f.image) {
                ui.label(RichText::new(format!("{} {} × {} px", icon::IMAGE, asset.width, asset.height)).size(theme::FONT_SM).color(TEXT_DIM));
            }
            widgets::prop_row(ui, "Mode", |ui| {
                widgets::dropdown(ui, ("img_mode", k), f.mode.label(), 120.0, |ui| {
                    for m in ImageMode::ALL {
                        ui.selectable_value(&mut f.mode, m, m.label());
                    }
                });
            });
            if f.mode == ImageMode::Tile {
                widgets::prop_row(ui, "Scale", |ui| {
                    let mut s = f.scale as f64 * 100.0;
                    if widgets::num_field(ui, "", &mut s, Num::default().range(1.0, 2000.0).suffix("%").decimals(0), 90.0).changed() {
                        f.scale = (s / 100.0) as _;
                    }
                });
            }
            let mut o = f.opacity * 100.0;
            if widgets::slider_row(ui, "Opacity", &mut o, 0.0..=100.0, "%") {
                f.opacity = o / 100.0;
            }
            let mut replace = false;
            if widgets::flat_button(ui, Some(icon::IMAGE), "Replace image…", true).clicked() {
                replace = true;
            }
            if replace {
                if let Some(p) = app.pick_image(slot.as_ref()) {
                    *slot = Some(p);
                }
            }
        }
    }
}

impl App {
    /// Ask for an image; a fill using it (keeping the mode of `current`).
    pub fn pick_image(&mut self, current: Option<&Paint>) -> Option<Paint> {
        let path = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"]).pick_file()?;
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(crate::document::ImageAsset::from_encoded) {
            Ok(asset) => {
                let img = self.doc.add_image(asset);
                let (mode, scale, opacity) = match current {
                    Some(Paint::Image(f)) => (f.mode, f.scale, f.opacity),
                    _ => (ImageMode::Fill, 1.0, 1.0),
                };
                Some(Paint::Image(ImageFill { image: img, mode, scale, opacity }))
            }
            Err(e) => {
                self.notify(format!("Could not load image: {e}"));
                None
            }
        }
    }

}
