//! Design panel sections: layer (opacity, fill rule, clip), shape
//! parameters (corners, points, sides) and effects.

use egui::{RichText, Ui};
use egui_phosphor::regular as icon;

use super::edit_selection;
use crate::app::App;
use crate::document::{FillRule, Shadow, Shape, ShapeKind};
use crate::ui::theme::{self, TEXT_DIM};
use crate::ui::widgets::{self, Num, WellPaint};

/// Layer appearance: opacity, fill rule, clip mask.
pub fn layer_section(app: &mut App, ui: &mut Ui) {
    edit_selection(app, ui, |app, ui, p| {
        widgets::section_icon(ui, "layer", icon::STACK, "Layer", |_| {}, |ui| {
            let mut o = p.opacity * 100.0;
            if widgets::slider_row(ui, &format!("{}  Opacity", icon::DROP_HALF), &mut o, 0.0..=100.0, "%") {
                p.opacity = o / 100.0;
            }
            widgets::prop_row(ui, &format!("{}  Blend", icon::CIRCLE_HALF), |ui| {
                use crate::document::BlendMode as B;
                let cur = p.blend;
                // Grouped as in other editors: darken, lighten, contrast, inversion, component.
                let groups: [&[B]; 6] = [
                    &[B::Normal],
                    &[B::Darken, B::Multiply, B::ColorBurn],
                    &[B::Lighten, B::Screen, B::ColorDodge],
                    &[B::Overlay, B::SoftLight, B::HardLight],
                    &[B::Difference, B::Exclusion],
                    &[B::Hue, B::Saturation, B::Color, B::Luminosity],
                ];
                widgets::dropdown(ui, "layer_blend", cur.label(), (ui.available_width() - 4.0).max(80.0), |ui| {
                    for (gi, g) in groups.iter().enumerate() {
                        if gi > 0 {
                            ui.separator();
                        }
                        for m in g.iter() {
                            if ui.selectable_label(*m == cur, m.label()).clicked() {
                                p.blend = *m;
                            }
                        }
                    }
                });
            });
            if matches!(p.kind, ShapeKind::Path(_)) {
                widgets::prop_row(ui, &format!("{}  Fill rule", icon::SELECTION_INVERSE), |ui| {
                    widgets::segmented(ui, &mut p.fill_rule, &[(FillRule::NonZero, "Non-zero"), (FillRule::EvenOdd, "Even-odd")]);
                });
            }
            if !matches!(p.kind, ShapeKind::Text(_)) {
                ui.horizontal(|ui| {
                    let mut on = p.text_wrap.is_some();
                    if ui.checkbox(&mut on, RichText::new("Wrap text around").size(theme::FONT_SM)).on_hover_text("Frame text below this shape flows around it").changed() {
                        p.text_wrap = on.then_some(8.0);
                    }
                    if let Some(off) = &mut p.text_wrap {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            widgets::num_field(ui, "", off, Num::px().range(0.0, 500.0), 64.0).on_hover_text("Gap between the shape and the text");
                        });
                    }
                });
            }
            let mut release_mask = false;
            if let Some(m) = &mut p.mask {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}  Masked by {}", icon::CIRCLE_HALF, m.shape.name)).size(theme::FONT_SM).color(TEXT_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        release_mask = widgets::flat_button(ui, None, "Release", true).clicked();
                    });
                });
                ui.horizontal(|ui| {
                    widgets::segmented(ui, &mut m.mode, &[(crate::document::MaskMode::Luminance, "Luminance"), (crate::document::MaskMode::Alpha, "Opacity")]);
                    ui.checkbox(&mut m.invert, RichText::new("Invert").size(theme::FONT_SM));
                });
            }
            if release_mask {
                app.pending_cmd = Some(crate::commands::Cmd::ReleaseMask);
            }
            if let Some(clip) = &p.clip {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}  Clipped by {}", icon::CROP, clip.name)).size(theme::FONT_SM).color(TEXT_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::flat_button(ui, None, "Release", true).clicked() {
                            app.pending_cmd = Some(crate::commands::Cmd::ReleaseClip);
                        }
                    });
                });
            }
        });
    });
}

/// Shape-specific parameters (corners, star points, polygon sides, …) of a
/// single selected shape. Changes also become the tool defaults.
pub fn shape_section(app: &mut App, ui: &mut Ui) {
    if app.selection.len() != 1 {
        return;
    }
    edit_selection(app, ui, |app, ui, p| {
        let before = p.kind.clone();
        shape_params(app, ui, p);
        if p.kind == before {
            return;
        }
        match &p.kind {
            ShapeKind::Polygon { sides, .. } => app.polygon_sides = *sides,
            ShapeKind::Prim { prim, .. } => {
                app.prim_params.insert(prim.kind, prim.clone());
            }
            ShapeKind::Star { points, inner, .. } => {
                app.star_points = *points;
                app.star_inner = *inner;
            }
            _ => {}
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Effect {
    Shadow,
    Blur,
    Noise,
    Outline,
    InnerShadow,
}

/// Effects list: drop and inner shadow, layer blur, noise and outline steps, each a
/// row that can be removed; "+" adds one.
pub fn effects_section(app: &mut App, ui: &mut Ui) {
    // Groups take effects of their own (around and over their children).
    edit_selection(app, ui, |app, ui, p| {
        let mut add: Option<Effect> = None;
        let has = [p.shadow.is_some(), p.inner_shadow.is_some(), p.blur > 0.0, p.noise > 0.0, false];
        widgets::section_icon(
            ui,
            "effects",
            icon::SPARKLE,
            "Effects",
            |ui| {
                let r = widgets::mini_icon(ui, icon::PLUS, TEXT_DIM);
                let r = widgets::tip(r, "Add effect: shadow, inner shadow, blur, noise or outline", None);
                egui::Popup::menu(&r).show(|ui| {
                    for (i, (e, g, name)) in [(Effect::Shadow, icon::DROP, "Drop shadow"), (Effect::InnerShadow, icon::SQUARE_HALF, "Inner shadow"), (Effect::Blur, icon::DROP_HALF_BOTTOM, "Layer blur"), (Effect::Noise, icon::DOTS_NINE, "Noise"), (Effect::Outline, icon::CIRCLES_THREE, "Outline")].into_iter().enumerate() {
                        if ui.add_enabled(!has[i], egui::Button::new(format!("{g}  {name}"))).clicked() {
                            add = Some(e);
                        }
                    }
                });
            },
            |ui| {
                effect_rows(app, ui, p);
            },
        );
        match add {
            Some(Effect::Shadow) => p.shadow = Some(Shadow::default()),
            Some(Effect::InnerShadow) => p.inner_shadow = Some(Shadow::inner()),
            Some(Effect::Blur) => p.blur = 4.0,
            Some(Effect::Noise) => p.noise = 0.15,
            Some(Effect::Outline) => {
                let palette = ["#FFFFFF", "#1E1E1E", "#FF5A5F", "#3D8BFF", "#FFC233"];
                let c = crate::document::Rgba::from_hex(palette[p.outlines.len() % palette.len()]).unwrap();
                p.outlines.push(crate::document::OutlineStep::new(c, 4.0));
            }
            None => {}
        }
    });
}

/// Title line of one effect: icon, name, trailing remove button.
fn effect_title(ui: &mut Ui, glyph: &str, name: &str, extra: impl FnOnce(&mut Ui)) -> bool {
    let mut remove = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(glyph).size(13.0).color(TEXT_DIM));
        ui.label(RichText::new(name).size(theme::FONT_MD));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            remove = widgets::tip(widgets::mini_icon(ui, icon::MINUS, TEXT_DIM), "Remove effect", None).clicked();
            extra(ui);
        });
    });
    remove
}

/// Colour row (with alpha) plus offset and blur fields; true = remove.
fn shadow_rows(app: &mut App, ui: &mut Ui, sh: &mut Shadow, key: &str, label: &str, picker_id: &str) -> bool {
    let mut remove = false;
    let open = super::design::paint_row(app, ui, key, WellPaint::Solid(sh.color), label, |ui| {
        remove = widgets::tip(widgets::mini_icon(ui, icon::MINUS, TEXT_DIM), "Remove effect", None).clicked();
        super::color::alpha_field(ui, &mut sh.color);
    });
    if open {
        super::design::editor_frame(ui, |ui| {
            super::design::picker(app, ui, &mut sh.color, egui::Id::new(picker_id));
        });
    }
    let fw = ((ui.available_width() - 8.0) / 3.0).max(50.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        widgets::num_field(ui, "X", &mut sh.dx, Num::px().range(-10000.0, 10000.0).speed(0.25), fw);
        widgets::num_field(ui, "Y", &mut sh.dy, Num::px().range(-10000.0, 10000.0).speed(0.25), fw);
        widgets::tip(widgets::num_field(ui, icon::DROP_HALF_BOTTOM, &mut sh.blur, Num::px().range(0.0, 1000.0).speed(0.25), fw), "Shadow blur", None);
    });
    ui.add_space(4.0);
    remove
}

fn effect_rows(app: &mut App, ui: &mut Ui, p: &mut Shape) {
    let id = p.id;
    if let Some(sh) = &mut p.shadow {
        if shadow_rows(app, ui, sh, &format!("shadow_{id}"), "Drop shadow", "shadow_picker") {
            p.shadow = None;
        }
    }
    if let Some(sh) = &mut p.inner_shadow {
        if shadow_rows(app, ui, sh, &format!("inner_shadow_{id}"), "Inner shadow", "inner_shadow_picker") {
            p.inner_shadow = None;
        }
    }
    if p.blur > 0.0 {
        let mut r = p.blur;
        let remove = effect_title(ui, icon::DROP_HALF_BOTTOM, "Layer blur", |ui| {
            widgets::num_field(ui, "", &mut r, Num::px().range(0.0, 1000.0).speed(0.25), 80.0);
        });
        p.blur = if remove { 0.0 } else { r.max(0.01) };
        ui.add_space(4.0);
    }
    if p.noise > 0.0 {
        let mut n = p.noise as f64 * 100.0;
        let remove = effect_title(ui, icon::DOTS_NINE, "Noise", |ui| {
            widgets::num_field(ui, "", &mut n, Num::default().range(1.0, 100.0).suffix("%").decimals(0), 80.0);
        });
        p.noise = if remove { 0.0 } else { (n / 100.0).max(0.01) as f32 };
        ui.add_space(4.0);
    }
    if !p.outlines.is_empty() {
        let mut add = false;
        let mut expand = false;
        let remove_all = effect_title(ui, icon::CIRCLES_THREE, "Outline", |ui| {
            add = widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), "Add outline step", None).clicked();
            expand = widgets::tip(widgets::mini_icon(ui, icon::PATH, TEXT_DIM), "Expand into shapes (bake the rings as paths)", None).clicked();
        });
        if expand {
            app.pending_cmd = Some(crate::commands::Cmd::ExpandOutlines);
        }
        let mut remove = None;
        for (i, step) in p.outlines.iter_mut().enumerate() {
            let key = format!("outline_{id}_{i}");
            let paint = step.paint();
            let label = match &step.gradient {
                Some(g) if g.kind == crate::document::GradientKind::Radial => "Radial".to_string(),
                Some(_) => "Linear".to_string(),
                None => step.color.with_alpha(1.0).to_hex(),
            };
            let open = super::design::paint_row(app, ui, &key, super::color::paint_of(Some(&paint)), &label, |ui| {
                if widgets::tip(widgets::mini_icon(ui, icon::X, TEXT_DIM), "Remove step", None).clicked() {
                    remove = Some(i);
                }
                widgets::tip(widgets::num_field(ui, icon::LINE_SEGMENT, &mut step.width, Num::px().range(0.0, 1000.0).speed(0.25), 76.0), "Outline width", None);
            });
            if open {
                super::design::editor_frame(ui, |ui| outline_paint(app, ui, step, i));
            }
        }
        if let Some(i) = remove {
            p.outlines.remove(i);
        }
        if add {
            let palette = ["#FFFFFF", "#1E1E1E", "#FF5A5F", "#3D8BFF", "#FFC233"];
            let c = crate::document::Rgba::from_hex(palette[p.outlines.len() % palette.len()]).unwrap();
            p.outlines.push(crate::document::OutlineStep::new(c, 4.0));
        }
        if remove_all {
            p.outlines.clear();
        }
    }
}

/// Paint editor of one outline step: solid colour or gradient.
fn outline_paint(app: &mut App, ui: &mut Ui, step: &mut crate::document::OutlineStep, i: usize) {
    use crate::document::{Gradient, GradientKind};
    let mut grad = step.gradient.is_some();
    let before = grad;
    widgets::segmented_width(ui, &mut grad, &[(false, "Solid"), (true, "Gradient")], ui.available_width());
    if grad != before {
        step.gradient = if grad {
            app.ui.gradient_stop = 0;
            Some(Gradient::new(GradientKind::Linear, step.color.with_alpha(1.0), step.color.with_alpha(0.0)))
        } else {
            None
        };
    }
    ui.add_space(6.0);
    match &mut step.gradient {
        Some(g) => {
            ui.push_id(("outline_grad", i), |ui| super::gradient::editor(app, ui, g));
            // The solid colour follows the first stop (used when switching back).
            if let Some(st) = g.stops.first() {
                step.color = st.color;
            }
        }
        None => {
            super::design::picker(app, ui, &mut step.color, egui::Id::new(("outline_picker", i)));
        }
    }
}

/// Fields for a primitive's parameters, two per row.
pub fn prim_fields(ui: &mut Ui, prim: &mut crate::prims::Prim) {
    let kind = prim.kind;
    if kind == crate::prims::PrimKind::Qr {
        let r = ui.add(egui::TextEdit::multiline(&mut prim.text).desired_rows(2).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 6)).hint_text("Text or URL"));
        widgets::tip(r, "Encoded text", None);
        ui.add_space(4.0);
    }
    let specs = kind.params();
    let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
    // Rows of two fields; a segmented choice takes a row of its own.
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let wide = spec.suffix == "ecc";
        match rows.last_mut() {
            Some(r) if !wide && r.len() == 1 && specs[r[0]].suffix != "ecc" => r.push(i),
            _ => rows.push(vec![i]),
        }
    }
    for row in rows {
        ui.horizontal(|ui| {
            for i in row {
                let spec = &specs[i];
                match spec.suffix {
                    "bool" => {
                        let mut on = prim.p[i] > 0.5;
                        let r = widgets::tip(widgets::icon_toggle(ui, &mut on, icon::ARROWS_LEFT_RIGHT), spec.name, None);
                        if r.changed() || r.clicked() {
                            prim.p[i] = if on { 1.0 } else { 0.0 };
                        }
                        ui.label(RichText::new(spec.name).size(theme::FONT_SM).color(TEXT_DIM));
                    }
                    "ecc" => {
                        let mut v = prim.p[i].round() as u8;
                        widgets::tip(ui.label(RichText::new(spec.name).size(theme::FONT_SM).color(TEXT_DIM)), "Error correction: higher survives more damage but makes a denser code", None);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                widgets::segmented(ui, &mut v, &[(0, "L"), (1, "M"), (2, "Q"), (3, "H")]);
                            });
                        });
                        prim.p[i] = v as f64;
                    }
                    _ => {
                        let k = if spec.percent { 100.0 } else { 1.0 };
                        let mut v = prim.p[i] * k;
                        let mut num = Num::default().range(spec.min * k, spec.max * k).suffix(spec.suffix);
                        // Integers step about one per 8 px of scrubbing.
                        num = if spec.int { num.decimals(0).speed(0.125) } else if spec.percent { num.decimals(0).speed(0.5) } else { num.decimals(1).speed(0.02) };
                        if widgets::num_field(ui, spec.name, &mut v, num, fw).changed() {
                            prim.p[i] = v / k;
                            *prim = prim.clone().clamped();
                        }
                    }
                }
            }
        });
    }
}

fn shape_params(app: &mut App, ui: &mut Ui, p: &mut Shape) {
    let id = p.id;
    match &mut p.kind {
        ShapeKind::Rect { radii, .. } => {
            widgets::section_icon(ui, "corners", icon::CORNERS_OUT, "Corners", |_| {}, |ui| {
                let uniform = radii.iter().all(|r| (*r - radii[0]).abs() < 1e-9);
                let key = ui.id().with("indep_corners");
                let mut indep = ui.data_mut(|d| *d.get_temp_mut_or(key, !uniform));
                ui.horizontal(|ui| {
                    if indep {
                        let fw = (ui.available_width() - 30.0) / 2.0 - 2.0;
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                widgets::num_field(ui, "↖", &mut radii[0], Num::px().range(0.0, 10000.0).speed(0.5), fw);
                                widgets::num_field(ui, "↗", &mut radii[1], Num::px().range(0.0, 10000.0).speed(0.5), fw);
                            });
                            ui.horizontal(|ui| {
                                widgets::num_field(ui, "↙", &mut radii[3], Num::px().range(0.0, 10000.0).speed(0.5), fw);
                                widgets::num_field(ui, "↘", &mut radii[2], Num::px().range(0.0, 10000.0).speed(0.5), fw);
                            });
                        });
                    } else {
                        let mut r = radii[0];
                        if widgets::num_field(ui, "Radius", &mut r, Num::px().range(0.0, 10000.0).speed(0.5), ui.available_width() - 30.0).changed() {
                            *radii = [r.max(0.0); 4];
                        }
                    }
                    let mut on = indep;
                    widgets::tip(widgets::icon_toggle(ui, &mut on, icon::CORNERS_OUT), "Independent corners", None);
                    indep = on;
                });
                ui.data_mut(|d| d.insert_temp(key, indep));
            });
        }
        ShapeKind::Star { points, inner, corner, .. } => {
            widgets::section_icon(ui, "star", icon::STAR, "Star", |_| {}, |ui| {
                let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
                ui.horizontal(|ui| {
                    let mut pts = *points as f64;
                    if widgets::num_field(ui, "Points", &mut pts, Num::default().range(3.0, 64.0).speed(0.1).decimals(0), fw).changed() {
                        *points = pts as u32;
                    }
                    let mut inr = *inner as f64 * 100.0;
                    if widgets::tip(widgets::num_field(ui, "Inner", &mut inr, Num::default().range(5.0, 100.0).suffix("%").decimals(0), fw), "Inner radius", None).changed() {
                        *inner = (inr / 100.0) as _;
                    }
                });
                ui.horizontal(|ui| {
                    widgets::tip(widgets::num_field(ui, "Corner", corner, Num::px().range(0.0, 10000.0).speed(0.5), fw), "Corner radius", None);
                });
            });
        }
        ShapeKind::Polygon { sides, corner, .. } => {
            widgets::section_icon(ui, "polygon", icon::POLYGON, "Polygon", |_| {}, |ui| {
                let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
                ui.horizontal(|ui| {
                    let mut s = *sides as f64;
                    if widgets::num_field(ui, "Sides", &mut s, Num::default().range(3.0, 64.0).speed(0.1).decimals(0), fw).changed() {
                        *sides = s as u32;
                    }
                    widgets::tip(widgets::num_field(ui, "Corner", corner, Num::px().range(0.0, 10000.0).speed(0.5), fw), "Corner radius", None);
                });
            });
        }
        ShapeKind::Prim { prim, .. } => {
            let kind = prim.kind;
            widgets::section_icon(ui, "prim", kind.icon(), kind.name(), |_| {}, |ui| {
                prim_fields(ui, prim);
            });
        }
        ShapeKind::Path(pd) => {
            widgets::section_icon(ui, "path", icon::BEZIER_CURVE, "Path", |_| {}, |ui| {
                ui.label(RichText::new(format!("{} nodes · {} subpaths", pd.node_count(), pd.subpaths.len())).size(theme::FONT_SM).color(TEXT_DIM));
                let sel: Vec<(usize, usize)> = app.node_sel.iter().filter(|k| k.0 == id).map(|k| (k.1, k.2)).collect();
                let mut r = sel
                    .first()
                    .and_then(|(s, i)| pd.subpaths.get(*s).and_then(|sp| sp.nodes.get(*i)))
                    .map(|n| n.corner)
                    .unwrap_or_else(|| pd.subpaths.iter().flat_map(|s| s.nodes.iter()).map(|n| n.corner).fold(0.0, f64::max));
                widgets::prop_row(ui, if sel.is_empty() { "Round all" } else { "Round sel." }, |ui| {
                    if widgets::num_field(ui, "", &mut r, Num::px().range(0.0, 10000.0).speed(0.5), 110.0).changed() {
                        for (si, sp) in pd.subpaths.iter_mut().enumerate() {
                            for (ni, node) in sp.nodes.iter_mut().enumerate() {
                                if sel.is_empty() || sel.contains(&(si, ni)) {
                                    node.corner = r;
                                }
                            }
                        }
                    }
                });
            });
        }
        ShapeKind::Compound { op, children } => {
            widgets::section_icon(ui, "compound", icon::UNITE, "Boolean", |_| {}, |ui| {
                ui.label(RichText::new(format!("{} shapes, combined live", children.len())).size(theme::FONT_SM).color(TEXT_DIM));
                let before = *op;
                widgets::prop_row(ui, "Operation", |ui| {
                    use crate::boolean::BoolOp;
                    widgets::segmented(ui, op, &[(BoolOp::Union, "Add"), (BoolOp::Subtract, "Subtract"), (BoolOp::Intersect, "Intersect"), (BoolOp::Exclude, "Xor")]);
                });
                if *op != before && p.name == crate::ops::op_name(before) {
                    p.name = crate::ops::op_name(*op).into();
                }
                if widgets::flat_button(ui, Some(icon::BEZIER_CURVE), "Flatten", true).clicked() {
                    app.pending_cmd = Some(crate::commands::Cmd::Flatten);
                }
            });
        }
        ShapeKind::Ellipse { .. } | ShapeKind::Text(_) | ShapeKind::Group { .. } => {}
    }
}
