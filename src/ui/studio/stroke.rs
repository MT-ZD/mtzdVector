//! Stroke section of the Design panel: colour, position, weight and the
//! advanced options (dash pattern, join/cap, arrowheads).

use egui::{RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use super::{design, edit_style};
use crate::app::App;
use crate::document::{Arrowhead, LineCap, LineJoin, ShapeKind, Stroke as DStroke, StrokeAlign};
use crate::ui::theme::{self, TEXT_DIM};
use crate::ui::widgets::{self, Num, WellPaint, WellTarget};

/// Dash presets in multiples of the stroke width.
const DASHES: [(&str, &[f64]); 6] = [
    ("Solid", &[]),
    ("Dash", &[4.0, 2.0]),
    ("Long dash", &[8.0, 3.0]),
    ("Dot", &[0.0, 2.0]),
    ("Dash dot", &[4.0, 2.0, 0.0, 2.0]),
    ("Fine", &[1.0, 1.0]),
];

fn preset_name(dash: &[f64]) -> &'static str {
    DASHES.iter().find(|(_, d)| d.len() == dash.len() && d.iter().zip(dash).all(|(a, b)| (a - b).abs() < 1e-6)).map(|(n, _)| *n).unwrap_or("Custom")
}

/// A small line sample of a dash pattern.
/// Premultiplied colour → opaque (overlapping parts must not add up).
fn opaque(c: egui::Color32) -> egui::Color32 {
    egui::Color32::from_rgb(c.r(), c.g(), c.b())
}

/// Segment size of the join / cap pickers.
const GLYPH: egui::Vec2 = egui::vec2(40.0, 22.0);

/// A thick corner showing the join's outer shape, with the path's centre
/// line drawn through it.
fn join_glyph(p: &egui::Painter, r: egui::Rect, j: LineJoin, c: egui::Color32) {
    let hw = 4.5;
    let (x0, y0) = (r.center().x - 4.0, r.center().y - 2.0);
    let (x1, y1) = (r.right() + 1.0, r.bottom() + 1.0);
    let fill = opaque(c.gamma_multiply(0.85));
    let p = p.with_clip_rect(r.shrink(1.0));
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(x0 - hw, y0), egui::pos2(x0 + hw, y1)), 0.0, fill);
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, y0 - hw), egui::pos2(x1, y0 + hw)), 0.0, fill);
    match j {
        LineJoin::Miter => {
            p.rect_filled(egui::Rect::from_min_max(egui::pos2(x0 - hw, y0 - hw), egui::pos2(x0, y0)), 0.0, fill);
        }
        LineJoin::Round => {
            p.circle_filled(egui::pos2(x0, y0), hw, fill);
        }
        LineJoin::Bevel => {
            p.add(egui::Shape::convex_polygon(vec![egui::pos2(x0 - hw, y0), egui::pos2(x0, y0 - hw), egui::pos2(x0, y0)], fill, egui::Stroke::NONE));
        }
    }
    let line = egui::Stroke::new(1.0, theme::BG_INPUT);
    p.line_segment([egui::pos2(x0, y1), egui::pos2(x0, y0)], line);
    p.line_segment([egui::pos2(x0, y0), egui::pos2(x1, y0)], line);
}

/// A thick line end: the cap's shape past the path's end point (the thin
/// vertical mark).
fn cap_glyph(p: &egui::Painter, r: egui::Rect, cap: LineCap, c: egui::Color32) {
    let hw = 4.5;
    let cy = r.center().y;
    let (x0, xe) = (r.left() - 2.0, r.center().x + 2.0);
    let fill = opaque(c.gamma_multiply(0.85));
    let p = p.with_clip_rect(r.shrink(1.0));
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, cy - hw), egui::pos2(xe, cy + hw)), 0.0, fill);
    match cap {
        LineCap::Butt => {}
        LineCap::Round => {
            p.circle_filled(egui::pos2(xe, cy), hw, fill);
        }
        LineCap::Square => {
            p.rect_filled(egui::Rect::from_min_max(egui::pos2(xe, cy - hw), egui::pos2(xe + hw, cy + hw)), 0.0, fill);
        }
    }
    p.line_segment([egui::pos2(x0, cy), egui::pos2(xe, cy)], egui::Stroke::new(1.0, theme::BG_INPUT));
    p.line_segment([egui::pos2(xe, cy - hw - 2.5), egui::pos2(xe, cy + hw + 2.5)], egui::Stroke::new(1.0, c));
}

fn dash_sample(ui: &mut Ui, dash: &[f64], cap: LineCap, w: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    let lw = 2.5f32;
    let color = theme::TEXT;
    let round = cap == LineCap::Round || dash.iter().step_by(2).any(|d| *d < 0.01);
    if dash.iter().sum::<f64>() < 1e-6 {
        p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(lw, color));
        return;
    }
    let mut x = rect.left();
    let mut i = 0;
    while x < rect.right() {
        let len = dash[i % dash.len()] as f32 * lw;
        if i % 2 == 0 {
            let x1 = (x + len).min(rect.right());
            if len < 0.01 || round {
                p.circle_filled(pos2(x + len / 2.0, y), lw / 2.0, color);
                if len > 0.01 {
                    p.line_segment([pos2(x, y), pos2(x1, y)], Stroke::new(lw, color));
                }
            } else {
                p.line_segment([pos2(x, y), pos2(x1, y)], Stroke::new(lw, color));
            }
        }
        x += len.max(0.5) + if i % 2 == 0 && round { lw } else { 0.0 };
        i += 1;
    }
}

fn arrow_glyph(a: Arrowhead) -> &'static str {
    match a {
        Arrowhead::None => icon::MINUS,
        Arrowhead::Arrow => icon::CARET_RIGHT,
        Arrowhead::Triangle => icon::TRIANGLE,
        Arrowhead::Circle => icon::CIRCLE,
        Arrowhead::Square => icon::SQUARE,
        Arrowhead::Bar => icon::LINE_VERTICAL,
    }
}

fn arrow_dropdown(ui: &mut Ui, id: &str, value: &mut Arrowhead, w: f32) {
    let cur = *value;
    widgets::dropdown(ui, id, format!("{}  {}", arrow_glyph(cur), cur.label()), w, |ui| {
        for a in Arrowhead::ALL {
            if ui.selectable_label(a == cur, format!("{}  {}", arrow_glyph(a), a.label())).clicked() {
                *value = a;
            }
        }
    });
}

/// Stroke section: paint row, position and weight; the sliders toggle shows
/// dashes, joins, caps and arrowheads.
pub fn stroke_section(app: &mut App, ui: &mut Ui) {
    let mut pick = false;
    edit_style(app, ui, |app, ui, p| {
        let has = p.stroke.is_some();
        let (mut add, mut remove) = (false, false);
        let mut add_extra = false;
        let adv_key = egui::Id::new("stroke_advanced");
        let mut advanced = ui.data_mut(|d| *d.get_temp_mut_or(adv_key, false));
        let shown = advanced;
        let closed = match &p.kind {
            ShapeKind::Path(pd) => pd.subpaths.iter().all(|s| s.closed),
            _ => true,
        };
        widgets::section_icon(
            ui,
            "stroke",
            icon::LINE_SEGMENT,
            "Stroke",
            |ui| {
                design::header_actions(ui, has, &mut add, &mut remove, "stroke");
                if has && widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), "Add a stroke under this one", None).clicked() {
                    add_extra = true;
                }
                if has {
                    let r = widgets::mini_icon(ui, icon::SLIDERS_HORIZONTAL, if advanced { theme::ACCENT } else { TEXT_DIM });
                    if widgets::tip(r, if advanced { "Hide stroke options" } else { "Dashes, joins, caps and arrowheads" }, None).clicked() {
                        advanced = !advanced;
                        if advanced {
                            widgets::request_reveal(ui.ctx(), "stroke_options");
                        }
                    }
                }
                pick = design::eyedropper_button(ui, has, "Pick the stroke colour from the canvas");
            },
            |ui| {
                let Some(st) = &mut p.stroke else { return };
                let hex = super::color::key_label(app, p.stroke_key).unwrap_or_else(|| st.color.with_alpha(1.0).to_hex());
                let open = design::paint_row(app, ui, "stroke", WellPaint::Solid(st.color), &hex, |ui| super::color::alpha_field(ui, &mut st.color));
                if open {
                    app.ui.color_target = WellTarget::Stroke;
                    design::editor_frame(ui, |ui| {
                        design::picker_linked(app, ui, &mut st.color, Some(&mut p.stroke_key), egui::Id::new("stroke_picker"));
                    });
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    widgets::segmented_width(ui, &mut st.align, &[(StrokeAlign::Inside, "Inside"), (StrokeAlign::Center, "Center"), (StrokeAlign::Outside, "Outside")], (ui.available_width() - 78.0).max(130.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::tip(widgets::num_field(ui, icon::LINE_SEGMENT, &mut st.width, Num::px().range(0.0, 1000.0).speed(0.1), ui.available_width()), "Stroke width", None);
                    });
                });
                if !closed && st.align != StrokeAlign::Center {
                    ui.label(RichText::new("Open paths are stroked centered").size(theme::FONT_SM).color(theme::TEXT_FAINT));
                }
                if shown {
                    ui.add_space(2.0);
                    let r = ui.scope(|ui| stroke_options(ui, st, closed)).response.rect;
                    widgets::reveal(ui, "stroke_options", r);
                }
                extra_strokes(app, ui, &mut p.strokes);
            },
        );
        if add_extra {
            let base = p.stroke.clone().unwrap_or_default();
            p.strokes.insert(0, DStroke { width: base.width * 3.0 + 2.0, color: crate::document::Rgba::BLACK.with_alpha(0.35), dash: Vec::new(), start_arrow: Arrowhead::None, end_arrow: Arrowhead::None, ..base });
        }
        ui.data_mut(|d| d.insert_temp(adv_key, advanced));
        if add {
            p.stroke = Some(DStroke { width: 2.0, ..Default::default() });
        }
        if remove {
            p.stroke = None;
            if app.ui.paint_open.as_deref() == Some("stroke") {
                app.ui.paint_open = None;
            }
        }
    });
    if pick {
        app.ui.color_target = WellTarget::Stroke;
        app.set_tool(crate::app::Tool::Eyedropper);
    }
}

/// Strokes under the main one (the first lowest): colour, width and
/// position each, top-most first.
fn extra_strokes(app: &mut App, ui: &mut Ui, strokes: &mut Vec<DStroke>) {
    let mut remove = None;
    for k in (0..strokes.len()).rev() {
        let st = &mut strokes[k];
        let key = format!("stroke{}", k + 1);
        let hex = st.color.with_alpha(1.0).to_hex();
        let mut del = false;
        let open = design::paint_row(app, ui, &key, WellPaint::Solid(st.color), &hex, |ui| {
            del = widgets::tip(widgets::mini_icon(ui, icon::MINUS, TEXT_DIM), "Remove stroke", None).clicked();
            widgets::tip(widgets::num_field(ui, icon::LINE_SEGMENT, &mut st.width, Num::px().range(0.0, 1000.0).speed(0.1), 70.0), "Stroke width", None);
        });
        if del {
            remove = Some(k);
        }
        if open {
            design::editor_frame(ui, |ui| {
                widgets::segmented_width(ui, &mut st.align, &[(StrokeAlign::Inside, "Inside"), (StrokeAlign::Center, "Center"), (StrokeAlign::Outside, "Outside")], ui.available_width());
                ui.add_space(4.0);
                design::picker(app, ui, &mut st.color, egui::Id::new(("extra_stroke_picker", k)));
            });
        }
    }
    if let Some(k) = remove {
        strokes.remove(k);
        app.ui.paint_open = None;
    }
}

/// A small sample of a width profile.
fn profile_sample(ui: &mut Ui, profile: &[crate::document::WidthPoint], w: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    let n = 24;
    let half = |i: usize| crate::document::profile_at(profile, i as f64 / n as f64) as f32 * 5.0;
    let x = |i: usize| rect.left() + rect.width() * i as f32 / n as f32;
    let mut pts: Vec<egui::Pos2> = (0..=n).map(|i| pos2(x(i), rect.center().y - half(i))).collect();
    pts.extend((0..=n).rev().map(|i| pos2(x(i), rect.center().y + half(i))));
    ui.painter().add(egui::Shape::Path(egui::epaint::PathShape::closed_line(pts, Stroke::new(1.0, theme::TEXT))));
}

fn stroke_options(ui: &mut Ui, st: &mut DStroke, closed: bool) {
    let fw = (ui.available_width() - 68.0).max(80.0);
    widgets::prop_row(ui, "Width", |ui| {
        let current = crate::document::WidthPreset::of(&st.profile).map(|p| p.label()).unwrap_or("Custom (brush)");
        let mut pick = None;
        widgets::dropdown(ui, "width_profile", current, fw - 8.0, |ui| {
            for p in crate::document::WidthPreset::ALL {
                ui.horizontal(|ui| {
                    if ui.selectable_label(p.profile() == st.profile, p.label()).clicked() {
                        pick = Some(p);
                    }
                    profile_sample(ui, &p.profile(), 60.0);
                });
            }
        });
        if let Some(p) = pick {
            st.profile = p.profile();
        }
    });
    widgets::prop_row(ui, "Dash", |ui| {
        let name = preset_name(&st.dash);
        let mut pick: Option<&[f64]> = None;
        let cap = st.cap;
        widgets::dropdown(ui, "dash_preset", name, fw - 8.0, |ui| {
            for (n, d) in DASHES {
                ui.horizontal(|ui| {
                    if ui.selectable_label(n == name, n).clicked() {
                        pick = Some(d);
                    }
                    dash_sample(ui, d, cap, 70.0);
                });
            }
        });
        if let Some(d) = pick {
            st.dash = d.to_vec();
            if d.iter().step_by(2).any(|v| *v < 0.01) {
                st.cap = LineCap::Round;
            }
        }
    });
    if st.is_dashed() {
        if st.dash.len() < 2 {
            st.dash.push(st.dash.first().copied().unwrap_or(2.0));
        }
        let half = ((fw - 4.0) / 2.0).max(60.0);
        widgets::prop_row(ui, "Dash / gap", |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            widgets::num_field(ui, "", &mut st.dash[0], Num::default().range(0.0, 100.0).speed(0.05).suffix("×"), half);
            widgets::num_field(ui, "", &mut st.dash[1], Num::default().range(0.0, 100.0).speed(0.05).suffix("×"), half);
        });
        if st.dash.len() >= 4 {
            widgets::prop_row(ui, "Dash 2 / gap", |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                widgets::num_field(ui, "", &mut st.dash[2], Num::default().range(0.0, 100.0).speed(0.05).suffix("×"), half);
                widgets::num_field(ui, "", &mut st.dash[3], Num::default().range(0.0, 100.0).speed(0.05).suffix("×"), half);
            });
        }
        widgets::prop_row(ui, "Phase", |ui| {
            widgets::num_field(ui, "", &mut st.dash_offset, Num::default().range(-1000.0, 1000.0).speed(0.05).suffix("×"), half);
            ui.label(RichText::new("× width").size(theme::FONT_SM).color(theme::TEXT_FAINT));
        });
        let (d, cap) = (st.dash.clone(), st.cap);
        widgets::prop_row(ui, "", |ui| dash_sample(ui, &d, cap, fw - 8.0));
    }
    widgets::prop_row(ui, "Join", |ui| {
        widgets::segmented_glyphs(ui, &mut st.join, &[(LineJoin::Miter, "Miter join"), (LineJoin::Round, "Round join"), (LineJoin::Bevel, "Bevel join")], GLYPH, join_glyph);
    });
    if st.join == LineJoin::Miter {
        widgets::prop_row(ui, "Miter limit", |ui| {
            widgets::num_field(ui, "", &mut st.miter_limit, Num::default().range(1.0, 100.0).speed(0.05).decimals(1), fw);
        });
    }
    widgets::prop_row(ui, "Cap", |ui| {
        widgets::segmented_glyphs(ui, &mut st.cap, &[(LineCap::Butt, "Butt cap"), (LineCap::Round, "Round cap"), (LineCap::Square, "Square cap")], GLYPH, cap_glyph);
    });
    let half = ((fw - 30.0) / 2.0).max(70.0);
    widgets::prop_row(ui, "Arrows", |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        arrow_dropdown(ui, "arrow_start", &mut st.start_arrow, half - 12.0);
        arrow_dropdown(ui, "arrow_end", &mut st.end_arrow, half - 12.0);
        if widgets::tip(widgets::mini_icon(ui, icon::ARROWS_LEFT_RIGHT, TEXT_DIM), "Swap start and end", None).clicked() {
            std::mem::swap(&mut st.start_arrow, &mut st.end_arrow);
        }
    });
    if closed && st.has_arrows() {
        widgets::prop_row(ui, "", |ui| {
            ui.label(RichText::new("Arrowheads apply to open paths").size(theme::FONT_SM).color(theme::TEXT_FAINT));
        });
    }
    if st.has_arrows() {
        let mut pct = st.arrow_scale * 100.0;
        widgets::prop_row(ui, "Arrow size", |ui| {
            if widgets::num_field(ui, "", &mut pct, Num::default().range(10.0, 1000.0).suffix("%").decimals(0), fw).changed() {
                st.arrow_scale = pct / 100.0;
            }
        });
    }
}
