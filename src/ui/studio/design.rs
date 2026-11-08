//! Design tab: one scrolling inspector for everything about the current tool
//! and selection, top to bottom: tool options, the object header, transform,
//! shape parameters, text, layer, fill, stroke and effects. With nothing
//! selected it shows the artboard. Colours open inline under their row.

use egui::{Color32, CornerRadius, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use egui_phosphor::regular as icon;

use super::{appearance, color, layout_ui, shape_icon, stroke, transform, typography};
use crate::app::{App, BuilderMode, Tool};
use crate::commands::Cmd;
use crate::document::ShapeKind;
use crate::ui::theme::{self, ACCENT, TEXT_DIM, TEXT_FAINT};
use crate::ui::toolbar::cmd_button;
use crate::ui::widgets::{self, Num, WellPaint};

pub fn show(app: &mut App, ui: &mut Ui) {
    if app.selection.is_empty() {
        tool_section(app, ui);
        artboard_list(app, ui);
        widgets::section_icon(ui, "artboard", icon::FRAME_CORNERS, "Artboard", |_| {}, |ui| transform::artboard_ui(app, ui));
        color::background_section(app, ui);
        layout_ui::export_section(app, ui);
        return;
    }
    object_header(app, ui);
    component_section(app, ui);
    tool_section(app, ui);
    transform::transform_section(app, ui);
    layout_ui::auto_layout_section(app, ui);
    live_section(app, ui);
    layout_ui::constraints_section(app, ui);
    boolean_section(app, ui);
    appearance::shape_section(app, ui);
    typography::text_section(app, ui);
    appearance::layer_section(app, ui);
    color::fill_section(app, ui);
    stroke::stroke_section(app, ui);
    appearance::effects_section(app, ui);
    layout_ui::export_section(app, ui);
}

/// Live warp or blend of the selected group: its settings, Expand and
/// Release.
fn live_section(app: &mut App, ui: &mut Ui) {
    use crate::paint_ops::{LiveEffect, WarpStyle};
    let [id] = app.selection[..] else { return };
    let Some(live) = app.doc.get(id).and_then(|s| s.live.clone()) else { return };
    let (title, glyph) = match &live {
        LiveEffect::Warp(_) => ("Warp", icon::FLAG),
        LiveEffect::Blend { .. } => ("Blend", icon::CIRCLES_THREE),
    };
    let mut action = None;
    widgets::section_icon(ui, "live_effect", glyph, title, |_| {}, |ui| {
        match live {
            LiveEffect::Warp(w) => {
                let mut p = w.params();
                widgets::prop_row(ui, "Style", |ui| {
                    widgets::dropdown(ui, "live_warp_style", p.style.label(), ui.available_width(), |ui| {
                        for st in WarpStyle::ALL {
                            if ui.selectable_label(st == p.style, st.label()).clicked() {
                                p.style = st;
                            }
                        }
                    });
                });
                let pct = |ui: &mut Ui, label: &str, v: &mut f64| {
                    let mut f = (*v * 100.0) as f32;
                    if widgets::slider_row(ui, label, &mut f, -100.0..=100.0, "%") {
                        *v = (f / 100.0) as f64;
                    }
                };
                pct(ui, "Bend", &mut p.bend);
                pct(ui, "Horizontal", &mut p.hdist);
                pct(ui, "Vertical", &mut p.vdist);
                if p != w.params() {
                    app.checkpoint("live-warp");
                    app.set_warp_params(id, p);
                }
            }
            LiveEffect::Blend { steps } => {
                let mut f = steps as f32;
                if widgets::slider_row(ui, "Steps", &mut f, 1.0..=50.0, "") {
                    app.set_blend_steps(id, f.round() as u32);
                }
                ui.label(RichText::new("Edit the end shapes inside the group; the steps follow.").size(theme::FONT_SM).color(theme::TEXT_FAINT));
            }
        }
        ui.horizontal(|ui| {
            if widgets::tip(widgets::flat_button(ui, Some(icon::ARROWS_OUT), "Expand", true), "Make the result plain shapes (no longer editable)", None).clicked() {
                action = Some(true);
            }
            if widgets::tip(widgets::flat_button(ui, Some(icon::ARROW_COUNTER_CLOCKWISE), "Release", true), "Remove the effect and get the original shapes back", None).clicked() {
                action = Some(false);
            }
        });
    });
    match action {
        Some(true) => app.expand_live(id),
        Some(false) => app.release_live(id),
        None => {}
    }
}

/// Main component: instance count and "new instance". Instance (or a layer
/// inside one): its main, overrides, reset and detach.
fn component_section(app: &mut App, ui: &mut Ui) {
    let id = app.selection[0];
    let inst = {
        let mut cur = Some(id);
        let mut found = None;
        while let Some(c) = cur {
            if app.instance_link(c).is_some() {
                found = Some(c);
                break;
            }
            cur = app.doc.parent_of(c);
        }
        found
    };
    let is_main = app.selection.len() == 1 && app.is_main(id);
    let mains = app.selection.iter().filter(|s| app.is_main(**s)).count();
    if mains >= 2 && mains == app.selection.len() {
        widgets::section_icon(ui, "component", icon::DIAMONDS_FOUR, "Component", |_| {}, |ui| {
            ui.label(RichText::new(format!("{mains} main components")).size(theme::FONT_SM).color(TEXT_DIM));
            ui.horizontal(|ui| {
                if widgets::tip_long(widgets::flat_button(ui, Some(icon::STACK), "Combine as variants", true), "Combine as variants", None, "Make them variants of one component; instances can then switch between them").clicked() {
                    app.combine_as_variants();
                }
            });
        });
        return;
    }
    if !is_main && inst.is_none() {
        return;
    }
    let dim = |t: String| RichText::new(t).size(theme::FONT_SM).color(TEXT_DIM);
    widgets::section_icon(ui, "component", icon::DIAMONDS_FOUR, "Component", |_| {}, |ui| {
        if is_main {
            let n = app.instances_of(id).len();
            ui.label(dim(match n {
                0 => "Main component · no instances yet".into(),
                1 => "Main component · 1 instance".into(),
                n => format!("Main component · {n} instances"),
            }));
            ui.horizontal(|ui| {
                if widgets::tip(widgets::flat_button(ui, Some(icon::PLUS), "New instance", true), "Place an instance next to the main component", None).clicked() {
                    let b = app.selection_bounds();
                    if let Some(b) = b {
                        let at = kurbo::Point::new(b.x1 + 40.0 + b.width() / 2.0, b.center().y);
                        app.insert_instance(id, at);
                    }
                }
            });
            main_variant_ui(app, ui, id);
            ui.label(RichText::new("Edits here update every instance.").size(theme::FONT_SM).color(TEXT_FAINT));
            return;
        }
        let Some(inst) = inst else { return };
        let main = app.instance_link(inst).map(|l| l.main).unwrap_or(0);
        let main_name = app.doc.get(main).map(|m| m.name.clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new(icon::DIAMOND).size(13.0).color(theme::COMPONENT));
            ui.label(dim("Instance of".into()));
            let r = ui.add(egui::Label::new(RichText::new(&main_name).size(theme::FONT_SM).color(theme::COMPONENT)).sense(Sense::click()).truncate());
            if widgets::tip(r.on_hover_cursor(egui::CursorIcon::PointingHand), "Go to the main component", None).clicked() {
                let ctx = ui.ctx().clone();
                app.run_cmd(Cmd::GoToMain, &ctx);
            }
        });
        let o = app.override_count(inst);
        if o > 0 {
            ui.label(dim(format!("{o} overridden layer{}", if o == 1 { "" } else { "s" })));
        }
        instance_props_ui(app, ui, inst);
        if id != inst {
            ui.label(RichText::new("Restyle this layer here; edit its shape on the main component.").size(theme::FONT_SM).color(TEXT_FAINT));
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let ctx = ui.ctx().clone();
            if widgets::flat_button(ui, Some(icon::ARROW_SQUARE_OUT), "Main", true).on_hover_text("Go to the main component").clicked() {
                app.run_cmd(Cmd::GoToMain, &ctx);
            }
            if widgets::flat_button(ui, Some(icon::ARROW_COUNTER_CLOCKWISE), "Reset", o > 0).on_hover_text("Drop every override").clicked() {
                app.run_cmd(Cmd::ResetInstance, &ctx);
            }
            if widgets::tip(widgets::flat_button(ui, Some(icon::LINK_BREAK), "Detach", true), "Make it a plain group", Cmd::DetachInstance.shortcut_text().as_deref()).clicked() {
                app.run_cmd(Cmd::DetachInstance, &ctx);
            }
        });
    });
}

/// Pencil / brush colour: a swatch that opens a picker.
fn colour_row(app: &mut App, ui: &mut Ui) {
    widgets::prop_row(ui, "Colour", |ui| {
        let c = app.brush.color;
        let (rect, r) = ui.allocate_exact_size(egui::vec2(44.0, 20.0), Sense::click());
        ui.painter().rect_filled(rect, 4.0, egui::Color32::from_rgba_unmultiplied((c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8, (c.a * 255.0) as u8));
        ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
        let r = widgets::tip(r.on_hover_cursor(egui::CursorIcon::PointingHand), "Stroke colour", None);
        ui.label(RichText::new(c.to_hex()).size(theme::FONT_SM).color(TEXT_DIM));
        egui::Popup::from_toggle_button_response(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_width(240.0);
            let mut col = app.brush.color;
            if picker(app, ui, &mut col, egui::Id::new("brush_colour")) {
                app.brush.color = col;
            }
        });
    });
}

/// Stabilizer for freehand tools: on/off, rope or window, length.
fn stabilizer_row(app: &mut App, ui: &mut Ui) {
    use crate::freehand::StabMode;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        widgets::tip_long(ui.checkbox(&mut app.stabilizer, "Stabilizer"), "Stabilizer", None, "Steadies the line: Rope trails the pointer on a string so small wobbles vanish; Window averages the recent pointer positions");
        ui.add_enabled_ui(app.stabilizer, |ui| {
            let modes = [(StabMode::Rope, "Rope: the line trails the pointer on a string"), (StabMode::Window, "Window: the line averages the recent pointer positions")];
            widgets::segmented_glyphs(ui, &mut app.stab_mode, &modes, vec2(30.0, 20.0), paint_stab_glyph);
            let mut l = app.stab_len as f64;
            let w = ui.available_width().clamp(52.0, 72.0);
            if widgets::num_field(ui, "", &mut l, widgets::Num::px().range(2.0, 300.0), w).changed() {
                app.stab_len = l as f32;
            }
        });
    });
}

/// A stabilizer mode's glyph: Rope, the pen dot pulled along on a slack
/// string by the pointer ring; Window, a jittery trace with the averaged
/// line through it.
fn paint_stab_glyph(p: &egui::Painter, r: egui::Rect, mode: crate::freehand::StabMode, col: Color32) {
    use egui::pos2;
    let c = r.center();
    match mode {
        crate::freehand::StabMode::Rope => {
            let (a, b) = (pos2(c.x - 8.0, c.y + 3.5), pos2(c.x + 8.0, c.y - 3.5));
            let sag = pos2(c.x - 1.0, c.y + 6.0);
            let pts: Vec<egui::Pos2> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    let u = 1.0 - t;
                    pos2(u * u * a.x + 2.0 * u * t * sag.x + t * t * b.x, u * u * a.y + 2.0 * u * t * sag.y + t * t * b.y)
                })
                .collect();
            p.add(egui::Shape::dashed_line(&pts, Stroke::new(1.2, col), 2.0, 1.5));
            p.circle_filled(a, 2.6, col);
            p.circle_stroke(b, 3.0, Stroke::new(1.3, col));
        }
        crate::freehand::StabMode::Window => {
            let xs = [-9.0, -6.0, -3.0, 0.0, 3.0, 6.0, 9.0];
            let jitter = [2.5, -3.0, 3.5, -2.5, 3.0, -3.5, 2.0];
            let rough: Vec<egui::Pos2> = xs.iter().zip(jitter).map(|(x, j)| pos2(c.x + x, c.y + j)).collect();
            p.add(egui::Shape::line(rough, Stroke::new(1.0, col.gamma_multiply(0.45))));
            p.line_segment([pos2(c.x - 9.5, c.y), pos2(c.x + 9.5, c.y)], Stroke::new(1.8, col));
        }
    }
}

/// Sample stroke of brush `st` (in the UI's text colour), cached per
/// brush and variation. Every shape in it has its own id in a block
/// reserved for the brush, so the render cache keeps them apart.
fn brush_sample(st: crate::freehand::BrushStyle, vary: f32) -> Option<crate::document::Shape> {
    use crate::document::Shape;
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<HashMap<(crate::freehand::BrushStyle, u32), Option<Shape>>> = RefCell::new(HashMap::new());
    }
    let key = (st, (vary * 1000.0) as u32);
    if let Some(s) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return s;
    }
    use kurbo::Point;
    // A gentle S-curve, pressed hardest in the middle.
    let n = 60;
    let pts: Vec<Point> = (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            Point::new(10.0 + t * 200.0, 20.0 - 8.0 * (t * std::f64::consts::TAU).sin())
        })
        .collect();
    let times: Vec<f64> = (0..n).map(|i| i as f64 * 0.012).collect();
    let pressure: Vec<Option<f32>> = (0..n).map(|i| Some((0.3 + 0.7 * (std::f64::consts::PI * i as f64 / (n - 1) as f64).sin()) as f32)).collect();
    let ink = theme::TEXT;
    let opts = crate::freehand::BrushOpts {
        width: 12.0,
        vary: vary.max(0.6),
        smooth: 0.5,
        style: st,
        source: crate::freehand::WidthSource::Pressure,
        color: crate::document::Rgba::rgb(ink.r() as f32 / 255.0, ink.g() as f32 / 255.0, ink.b() as f32 / 255.0),
    };
    let out = crate::freehand::brush_shape(&pts, &times, &pressure, 0.5, &opts).map(|stroke| {
        // A fixed frame keeps every sample at the same scale.
        let mut frame = Shape::new(ShapeKind::Rect { w: 220.0, h: 40.0, radii: [0.0; 4] });
        frame.fill = None;
        frame.stroke = None;
        let mut g = Shape::new(ShapeKind::Group { children: vec![frame, stroke] });
        let index = crate::brushes::CATEGORIES.iter().flat_map(|c| c.1.iter()).position(|b| *b == st).unwrap_or(0) as u64;
        let mut next = u64::MAX - (index + 1) * 10_000;
        let rev = 1 + key.1 as u64;
        fn stamp(s: &mut Shape, next: &mut u64, rev: u64) {
            s.id = *next;
            s.rev = rev;
            *next += 1;
            if let Some(c) = s.kind.children_mut() {
                c.iter_mut().for_each(|c| stamp(c, next, rev));
            }
        }
        stamp(&mut g, &mut next, rev);
        g
    });
    CACHE.with(|c| c.borrow_mut().insert(key, out.clone()));
    out
}

fn paint_brush_sample(app: &mut App, ui: &Ui, rect: egui::Rect, st: crate::freehand::BrushStyle) {
    if let Some(g) = brush_sample(st, app.brush.vary) {
        let painter = ui.painter().clone();
        app.cache.paint_thumbnail(&painter, &g, &app.doc, &mut app.fonts, rect, 1.0);
    }
}

/// The brush: a dropdown of the library by category, each brush shown
/// with its name and a sample stroke.
fn brush_picker(app: &mut App, ui: &mut Ui) {
    use crate::brushes::CATEGORIES;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let bg = if resp.hovered() || open { theme::BG_HOVER } else { theme::BG_INPUT };
    ui.painter().rect_filled(rect, CornerRadius::same(4), bg);
    let current = app.brush.style;
    let name = ui.painter().layout(current.label().to_string(), egui::FontId::proportional(theme::FONT_MD), theme::TEXT, 84.0);
    ui.painter().galley(egui::pos2(rect.left() + 10.0, rect.center().y - name.size().y / 2.0), name, theme::TEXT);
    ui.painter().text(egui::pos2(rect.right() - 10.0, rect.center().y), egui::Align2::CENTER_CENTER, if open { icon::CARET_UP } else { icon::CARET_DOWN }, egui::FontId::proportional(12.0), TEXT_DIM);
    let sample = egui::Rect::from_min_max(egui::pos2(rect.left() + 98.0, rect.top() + 3.0), egui::pos2(rect.right() - 22.0, rect.bottom() - 3.0));
    paint_brush_sample(app, ui, sample, current);
    let resp = widgets::tip(resp.on_hover_cursor(egui::CursorIcon::PointingHand), "Brush", None);
    let mut chosen = None;
    egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(w).show(|ui| {
        ui.set_width(w - 12.0);
        egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
            for (cat, brushes) in CATEGORIES {
                ui.add_space(4.0);
                ui.label(RichText::new(cat).size(theme::FONT_SM).strong().color(TEXT_DIM));
                for &st in brushes {
                    let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                    let sel = st == current;
                    let fill = if sel { theme::ACCENT_DIM } else if row.hovered() { theme::BG_HOVER } else { egui::Color32::TRANSPARENT };
                    ui.painter().rect_filled(r, CornerRadius::same(4), fill);
                    if sel {
                        ui.painter().rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, ACCENT), StrokeKind::Inside);
                    }
                    let sample = egui::Rect::from_min_max(egui::pos2(r.left() + 92.0, r.top() + 3.0), egui::pos2(r.right() - 6.0, r.bottom() - 3.0));
                    paint_brush_sample(app, ui, sample, st);
                    let name = ui.painter().layout(st.label().to_string(), egui::FontId::proportional(theme::FONT_SM), if sel { egui::Color32::WHITE } else { theme::TEXT }, 82.0);
                    ui.painter().galley(egui::pos2(r.left() + 8.0, r.center().y - name.size().y / 2.0), name, theme::TEXT);
                    if row.clicked() {
                        chosen = Some(st);
                        ui.close();
                    }
                }
            }
        });
    });
    if let Some(st) = chosen {
        app.brush.style = st;
    }
    ui.add_space(4.0);
}

/// Variant set and property values of a main component.
fn main_variant_ui(app: &mut App, ui: &mut Ui, id: u64) {
    let props = crate::variants::props_of(app.doc.get(id).unwrap());
    let set = match app.doc.get(id).and_then(|s| s.component.as_ref()) {
        Some(crate::document::Component::Main { set, .. }) => set.clone(),
        _ => None,
    };
    if let Some(set) = &set {
        let n = app.variants_of(id).len();
        ui.label(RichText::new(format!("Variant of {set} · {n} in the set")).size(theme::FONT_SM).color(TEXT_DIM));
    }
    let mut change: Option<(String, String, String)> = None;
    let mut remove = None;
    for (k, v) in &props {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = ((ui.available_width() - 24.0) / 2.0).max(50.0);
            let (mut nk, mut nv) = (k.clone(), v.clone());
            let r1 = ui.add(egui::TextEdit::singleline(&mut nk).hint_text("Property").desired_width(w));
            let r2 = ui.add(egui::TextEdit::singleline(&mut nv).hint_text("Value").desired_width(w));
            if (r1.lost_focus() && nk != *k) || (r2.lost_focus() && nv != *v) {
                change = Some((k.clone(), nk.trim().to_string(), nv.trim().to_string()));
            }
            if widgets::tip(widgets::mini_icon(ui, icon::MINUS, TEXT_DIM), "Remove the property", None).clicked() {
                remove = Some(k.clone());
            }
        });
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if widgets::tip_long(widgets::flat_button(ui, Some(icon::PLUS), "Property", true), "Property", None, "Add a property such as State or Size; each variant gets its own value").clicked() {
            let mut n = 1;
            let name = loop {
                let k = if n == 1 { "Property".to_string() } else { format!("Property {n}") };
                if !props.contains_key(&k) {
                    break k;
                }
                n += 1;
            };
            let value = app.doc.get(id).map(|s| s.name.clone()).unwrap_or_default();
            app.set_variant_prop(id, None, &name, &value);
        }
        if widgets::tip_long(widgets::flat_button(ui, Some(icon::COPY), "Add variant", true), "Add variant", None, "A copy beside it in the same set, to restyle as another state").clicked() {
            app.add_variant(id);
        }
    });
    if let Some((old, k, v)) = change {
        app.set_variant_prop(id, Some(&old), &k, &v);
    }
    if let Some(k) = remove {
        app.checkpoint("variant prop");
        if let Some(Some(crate::document::Component::Main { props, .. })) = app.doc.get_mut(id).map(|s| s.component.as_mut()) {
            props.remove(&k);
        }
    }
}

/// An instance's properties: a dropdown per variant property, its text
/// layers and the visibility of its top-level layers.
fn instance_props_ui(app: &mut App, ui: &mut Ui, inst: u64) {
    let Some(main) = app.instance_link(inst).map(|l| l.main) else { return };
    let options = app.variant_options(main);
    let current = app.doc.get(main).map(crate::variants::props_of).unwrap_or_default();
    let texts = app.instance_texts(inst);
    let layers: Vec<(u64, String, bool)> = app.doc.get(inst).and_then(|s| s.kind.children()).map(|c| c.iter().map(|s| (s.id, s.name.clone(), s.visible)).collect()).unwrap_or_default();
    let mut switch = None;
    for (k, values) in options.iter().filter(|(_, v)| v.len() > 1 || current.len() > 0) {
        widgets::prop_row(ui, k, |ui| {
            let cur = current.get(k).cloned().unwrap_or_default();
            let w = (ui.available_width() - 4.0).max(100.0);
            widgets::dropdown(ui, ("variant", inst, k.as_str()), cur.clone(), w, |ui| {
                for v in values {
                    if ui.selectable_label(*v == cur, v).clicked() && *v != cur {
                        switch = Some((k.clone(), v.clone()));
                    }
                }
            });
        });
    }
    for (lid, name, text) in texts.iter().take(6) {
        widgets::prop_row(ui, name, |ui| {
            let mut t = text.clone();
            let w = (ui.available_width() - 4.0).max(100.0);
            let r = ui.add(egui::TextEdit::singleline(&mut t).desired_width(w));
            if r.changed() {
                app.set_instance_text(*lid, &t);
            }
        });
    }
    if layers.len() > 1 {
        widgets::prop_row(ui, "Layers", |ui| {
          ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for (lid, name, vis) in &layers {
                let r = ui.selectable_label(*vis, RichText::new(name).size(theme::FONT_SM));
                if widgets::tip(r, if *vis { "Hide this layer in the instance" } else { "Show this layer in the instance" }, None).clicked() {
                    app.checkpoint("instance layer");
                    if let Some(s) = app.doc.get_mut(*lid) {
                        s.visible = !vis;
                    }
                }
            }
          });
        });
    }
    if let Some((k, v)) = switch {
        app.set_instance_variant(inst, &k, &v);
    }
}

/// Combine two or more selected shapes (live booleans), or hand them to the
/// shape builder.
fn boolean_section(app: &mut App, ui: &mut Ui) {
    if app.selection.len() < 2 {
        return;
    }
    widgets::section_icon(ui, "boolean", icon::UNITE, "Boolean", |_| {}, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for c in crate::ui::menubar::BOOLEAN {
                cmd_button(ui, app, c);
            }
            ui.add_space(6.0);
            cmd_button(ui, app, Cmd::Flatten);
            cmd_button(ui, app, Cmd::Tool(Tool::ShapeBuilder));
        });
    });
}

/// Every artboard: click to select, double-click to zoom to it; "+" adds
/// one, hover icons duplicate or delete.
fn artboard_list(app: &mut App, ui: &mut Ui) {
    let mut add = false;
    widgets::section_icon(
        ui,
        "artboards", icon::SQUARES_FOUR, "Artboards",
        |ui| {
            add = widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), "New artboard", None).clicked();
        },
        |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let active = app.active().id;
            let list: Vec<(u64, String, f64, f64)> = app.doc.artboards.iter().map(|a| (a.id, a.name.clone(), a.w, a.h)).collect();
            let many = list.len() > 1;
            let mut action: Option<(u64, u8)> = None;
            for (id, name, w, h) in list {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_H + 2.0), Sense::click());
                let sel = id == active;
                let bg = if sel && app.artboard_selected {
                    theme::ACCENT_DIM
                } else if resp.hovered() {
                    theme::BG_HOVER
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
                let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
                row.spacing_mut().item_spacing.x = 2.0;
                if resp.hovered() || row.rect_contains_pointer(rect) {
                    if many && widgets::tip(widgets::mini_icon(&mut row, icon::TRASH, TEXT_DIM), "Delete artboard (artwork stays)", None).clicked() {
                        action = Some((id, 2));
                    }
                    if widgets::tip(widgets::mini_icon(&mut row, icon::COPY, TEXT_DIM), "Duplicate with artwork", None).clicked() {
                        action = Some((id, 1));
                    }
                } else {
                    let c = if sel && app.artboard_selected { theme::TEXT } else { TEXT_FAINT };
                    row.label(RichText::new(format!("{} × {}", crate::units::fmt_value(w), crate::units::fmt(h))).size(theme::FONT_SM).color(c));
                }
                row.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let c = if sel { Color32::WHITE } else { TEXT_DIM };
                    ui.label(RichText::new(icon::FRAME_CORNERS).size(13.0).color(c));
                    ui.add(egui::Label::new(RichText::new(name).size(theme::FONT_MD).color(if sel { Color32::WHITE } else { theme::TEXT })).truncate().selectable(false));
                });
                if resp.double_clicked() {
                    action = Some((id, 3));
                } else if resp.clicked() && action.is_none() {
                    action = Some((id, 0));
                }
            }
            match action {
                Some((id, 0)) => app.activate_artboard(id, true),
                Some((id, 1)) => {
                    app.activate_artboard(id, true);
                    app.duplicate_artboard();
                }
                Some((id, 2)) => {
                    app.activate_artboard(id, false);
                    app.delete_artboard();
                }
                Some((id, _)) => {
                    app.activate_artboard(id, true);
                    app.zoom_to_artboard(id);
                }
                None => {}
            }
        },
    );
    if add {
        app.new_artboard();
    }
}

/// Options of the active tool (defaults for new objects, node commands…).
fn tool_section(app: &mut App, ui: &mut Ui) {
    let selected_kind = (app.selection.len() == 1).then(|| app.doc.get(app.selection[0]).map(|s| s.kind.clone())).flatten();
    match app.tool {
        // A selected star/polygon is edited in its own section below.
        Tool::Star if !matches!(selected_kind, Some(ShapeKind::Star { .. })) => {
            widgets::section_icon(ui, "tool_star", icon::STAR, "New stars", |_| {}, |ui| {
                let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
                ui.horizontal(|ui| {
                    let mut pts = app.star_points as f64;
                    if widgets::num_field(ui, "Points", &mut pts, Num::default().range(3.0, 64.0).speed(0.1).decimals(0), fw).changed() {
                        app.star_points = pts as u32;
                    }
                    let mut inner = app.star_inner * 100.0;
                    if widgets::num_field(ui, "Inner", &mut inner, Num::default().range(5.0, 100.0).suffix("%").decimals(0), fw).changed() {
                        app.star_inner = inner / 100.0;
                    }
                });
            });
        }
        Tool::Polygon if !matches!(selected_kind, Some(ShapeKind::Polygon { .. })) => {
            widgets::section_icon(ui, "tool_polygon", icon::POLYGON, "New polygons", |_| {}, |ui| {
                let mut sides = app.polygon_sides as f64;
                widgets::prop_row(ui, "Sides", |ui| {
                    if widgets::num_field(ui, "", &mut sides, Num::default().range(3.0, 64.0).speed(0.1).decimals(0), ui.available_width()).changed() {
                        app.polygon_sides = sides as u32;
                    }
                });
            });
        }
        Tool::Prim(k) if !matches!(&selected_kind, Some(ShapeKind::Prim { prim, .. }) if prim.kind == k) => {
            widgets::section_icon(ui, "tool_prim", k.icon(), &format!("New {}", k.name().to_lowercase()), |_| {}, |ui| {
                let mut prim = app.prim_params.get(&k).cloned().unwrap_or_else(|| k.default_prim());
                let before = prim.clone();
                appearance::prim_fields(ui, &mut prim);
                if prim != before {
                    app.prim_params.insert(k, prim);
                }
            });
        }
        Tool::Fill => {
            widgets::section_icon(ui, "tool_fill", icon::PAINT_BUCKET, "Fill", |_| {}, |ui| {
                let has_grad = app.selection.len() == 1 && matches!(app.doc.get(app.selection[0]).and_then(|s| s.fill.as_ref()), Some(crate::document::Paint::Gradient(_)));
                if has_grad {
                    super::edit_selection(app, ui, |app, ui, p| {
                        if let Some(crate::document::Paint::Gradient(g)) = &mut p.fill {
                            super::gradient::editor(app, ui, g);
                        }
                    });
                } else {
                    let msg = if app.selection.is_empty() { "Drag across a shape to lay a gradient on it." } else { "Drag across the selection to lay a gradient." };
                    ui.label(RichText::new(msg).size(theme::FONT_SM).color(TEXT_FAINT));
                }
            });
        }
        Tool::Text | Tool::FrameText if !matches!(selected_kind, Some(ShapeKind::Text(_))) => {
            widgets::section_icon(ui, "tool_text", icon::TEXT_T, "New text", |_| {}, |ui| typography::text_defaults(app, ui));
        }
        Tool::Knife => {
            widgets::section_icon(ui, "tool_knife", icon::KNIFE, "Knife", |_| {}, |ui| {
                let mut v = app.knife_smooth * 100.0;
                if widgets::slider_row(ui, "Smoothing", &mut v, 0.0..=100.0, "%") {
                    app.knife_smooth = (v / 100.0).clamp(0.0, 1.0);
                }
                stabilizer_row(app, ui);
                ui.label(RichText::new("Freehand cuts are smoothed; Shift cuts straight.").size(theme::FONT_SM).color(TEXT_FAINT));
            });
        }
        Tool::Pencil | Tool::Brush => {
            let brush = app.tool == Tool::Brush;
            let (key, ic, title) = if brush { ("tool_brush", icon::PAINT_BRUSH, "Brush") } else { ("tool_pencil", icon::PENCIL_SIMPLE, "Pencil") };
            widgets::section_icon(ui, key, ic, title, |_| {}, |ui| {
                use crate::freehand::WidthSource;
                if brush {
                    brush_picker(app, ui);
                }
                let b = &mut app.brush;
                if brush {
                    let mut w = b.width as f32;
                    if widgets::slider_row(ui, "Width", &mut w, 1.0..=100.0, " px") {
                        b.width = w.clamp(0.5, 500.0) as f64;
                    }
                    if b.style.varies() {
                        widgets::prop_row(ui, "Width from", |ui| {
                            let w = (ui.available_width() - 4.0).max(100.0);
                            widgets::dropdown(ui, "brush_source", b.source.label(), w, |ui| {
                                for src in WidthSource::ALL {
                                    ui.selectable_value(&mut b.source, src, src.label());
                                }
                            });
                        });
                        if b.source != WidthSource::Fixed {
                            let mut v = b.vary * 100.0;
                            if widgets::slider_row(ui, "Variation", &mut v, 0.0..=100.0, "%") {
                                b.vary = (v / 100.0).clamp(0.0, 1.0);
                            }
                        }
                    }
                }
                let mut v = b.smooth * 100.0;
                if widgets::slider_row(ui, "Smoothing", &mut v, 0.0..=100.0, "%") {
                    b.smooth = (v / 100.0).clamp(0.0, 1.0);
                }
                colour_row(app, ui);
                stabilizer_row(app, ui);
                let hint = if !brush {
                    "End near the start to close the path.".to_string()
                } else if app.pen_pressure.is_some() || app.tablet_seen {
                    "Pen pressure is available.".to_string()
                } else if app.settings.tablet {
                    "No pen pressure yet: draw with a tablet pen to use it.".to_string()
                } else {
                    "Pen pressure is off (Preferences › Tablet).".to_string()
                };
                ui.label(RichText::new(hint).size(theme::FONT_SM).color(TEXT_FAINT));
            });
        }
        Tool::Node => {
            widgets::section_icon(ui, "tool_node", icon::NAVIGATION_ARROW, "Nodes", |_| {}, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
                    for c in [Cmd::NodesSmooth, Cmd::NodesSharp, Cmd::BreakAtNode, Cmd::JoinNodes, Cmd::ToggleClosed, Cmd::DeleteNodes] {
                        cmd_button(ui, app, c);
                    }
                    ui.add_space(4.0);
                    for c in [Cmd::ReversePath, Cmd::SimplifyPath] {
                        cmd_button(ui, app, c);
                    }
                });
                let n = app.node_sel.len();
                let text = if n == 0 { "Click or drag to select nodes".to_string() } else { format!("{n} node{} selected", if n == 1 { "" } else { "s" }) };
                ui.label(RichText::new(text).size(theme::FONT_SM).color(TEXT_FAINT));
            });
        }
        Tool::Corner => {
            widgets::section_icon(ui, "tool_corner", icon::CORNERS_OUT, "Corners", |_| {}, |ui| {
                let has = !app.selection.is_empty();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let w = (ui.available_width() - 120.0).max(70.0);
                    widgets::num_field(ui, "Radius", &mut app.corner_radius_ui, Num::px().range(0.0, 10000.0).speed(0.5), w);
                    if widgets::flat_button(ui, None, "Apply", has).on_hover_text("Round all corners of the selection (or only the selected nodes)").clicked() {
                        let r = app.corner_radius_ui;
                        app.set_corner_radius_all(r);
                    }
                    if widgets::flat_button(ui, None, "Remove", has).clicked() {
                        app.set_corner_radius_all(0.0);
                    }
                });
            });
        }
        Tool::ShapeBuilder => {
            widgets::section_icon(ui, "tool_builder", icon::UNITE, "Shape builder", |_| {}, |ui| {
                widgets::segmented_width(ui, &mut app.builder_mode, &[(BuilderMode::Add, "Add"), (BuilderMode::Subtract, "Subtract"), (BuilderMode::New, "New")], ui.available_width());
                let (text, color) = match &app.builder {
                    Some(b) => (format!("{} regions · click or drag across them", b.regions.len()), TEXT_FAINT),
                    None => ("Select two or more overlapping shapes".to_string(), ACCENT),
                };
                ui.label(RichText::new(text).size(theme::FONT_SM).color(color));
            });
        }
        _ => {}
    }
}

/// Name and type of the selection with its conversion actions.
fn object_header(app: &mut App, ui: &mut Ui) {
    let Some(first) = app.doc.get(app.selection[0]).cloned() else { return };
    let n = app.selection.len();
    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for c in [Cmd::ExpandOutlines, Cmd::ExpandStroke, Cmd::ConvertToCurves] {
                let hide = c == Cmd::ConvertToCurves && n == 1 && matches!(first.kind, ShapeKind::Path(_));
                if c.enabled(app) && !hide {
                    cmd_button(ui, app, c);
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (glyph, name, kind) = if n == 1 {
                    let kind = match &first.kind {
                        ShapeKind::Group { children } => {
                            let bg = first.layout.as_ref().and_then(|l| l.background).is_some_and(|b| children.iter().any(|c| c.id == b));
                            let what = match (&first.layout, &first.live) {
                                (Some(_), _) => "Auto layout",
                                (_, Some(crate::paint_ops::LiveEffect::Warp(_))) => "Warp",
                                (_, Some(crate::paint_ops::LiveEffect::Blend { .. })) => "Blend",
                                _ => "Group",
                            };
                            format!("{what} · {}", children.len() - bg as usize)
                        }
                        k => k.type_name().to_string(),
                    };
                    let kind = match &first.component {
                        Some(crate::document::Component::Main { .. }) => "Component".to_string(),
                        Some(crate::document::Component::Instance(_)) => "Instance".to_string(),
                        None => kind,
                    };
                    (shape_icon(&first), first.name.clone(), kind)
                } else {
                    ((icon::SELECTION_ALL, false), format!("{n} objects"), String::new())
                };
                let (glyph, comp) = glyph;
                ui.label(RichText::new(glyph).size(14.0).color(if comp { theme::COMPONENT } else { TEXT_DIM }));
                ui.add(egui::Label::new(RichText::new(name).size(theme::FONT_LG).strong()).truncate().selectable(false));
                if !kind.is_empty() && kind != first.name {
                    ui.label(RichText::new(kind).size(theme::FONT_SM).color(TEXT_FAINT));
                }
            });
        });
    });
    ui.add_space(2.0);
}

/// "+" / "−" in a section header (add or remove the paint).
pub fn header_actions(ui: &mut Ui, has: bool, add: &mut bool, remove: &mut bool, what: &str) {
    if has {
        *remove = widgets::tip(widgets::mini_icon(ui, icon::MINUS, TEXT_DIM), &format!("Remove {what}"), None).clicked();
    } else {
        *add = widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), &format!("Add {what}"), None).clicked();
    }
}

/// A paint row: swatch and label on the left, `trailing` on the right (added
/// right to left). Clicking the swatch or label toggles the inline editor
/// `key`; returns whether it is open.
pub fn paint_row(app: &mut App, ui: &mut Ui, key: &str, paint: WellPaint, label: &str, trailing: impl FnOnce(&mut Ui)) -> bool {
    let mut open = app.ui.paint_open.as_deref() == Some(key);
    let mut toggle = false;
    ui.horizontal(|ui| {
        ui.set_min_height(theme::ROW_H + 2.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            trailing(ui);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let r = widgets::mini_well(ui, paint, false);
                if open {
                    ui.painter().rect_stroke(r.rect.expand(2.0), CornerRadius::same(4), Stroke::new(1.5, ACCENT), StrokeKind::Outside);
                }
                let text = RichText::new(label).size(theme::FONT_MD);
                let text = if label.starts_with('#') { text.monospace() } else { text };
                let l = ui.add(egui::Label::new(text).truncate().selectable(false).sense(Sense::click()));
                toggle = r.clicked() || l.clicked();
                widgets::tip(r, if open { "Close colour editor" } else { "Edit colour" }, None);
            });
        });
    });
    if toggle {
        open = !open;
        if open {
            widgets::request_reveal(ui.ctx(), "paint");
        }
        app.ui.paint_open = open.then(|| key.to_string());
        if !open {
            app.ui.gradient_edit = None;
        }
    }
    open
}

/// Inset panel for an inline editor under its row.
pub fn editor_frame(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let r = egui::Frame::new().fill(Color32::from_black_alpha(40)).corner_radius(theme::radius()).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
    // Just opened: scroll down to show it (with the row that opened it).
    widgets::reveal(ui, "paint", r.response.rect.with_min_y(r.response.rect.top() - theme::ROW_H - 8.0));
    ui.add_space(4.0);
}

/// Colour picker with the document's colour keys and its colours below it.
/// Returns true when the colour changed.
pub fn picker(app: &mut App, ui: &mut Ui, color: &mut crate::document::Rgba, id: egui::Id) -> bool {
    picker_linked(app, ui, color, None, id, None)
}

/// Picker for a colour that can be linked to a colour key (fills and
/// strokes): choosing a key links it, editing the colour unlinks it.
pub fn picker_linked(app: &mut App, ui: &mut Ui, color: &mut crate::document::Rgba, mut link: Option<&mut Option<u64>>, id: egui::Id, eyedropper: Option<(&str, &mut bool)>) -> bool {
    let before = *color;
    let tools = eyedropper.map(|(tip, pick)| crate::color::PickerTools { tip, last: app.settings.last_picked, pick });
    if crate::color::color_picker_with(ui, color, id, tools) {
        if let Some(l) = link.as_deref_mut() {
            *l = None;
        }
    }
    ui.add_space(4.0);
    if let Some(k) = key_picker(app, ui, *color, link.as_deref().copied().flatten(), link.is_some(), id) {
        match k {
            KeyPick::Use(kid) => {
                if let Some(key) = app.doc.color_key(kid) {
                    *color = key.color;
                    if let Some(l) = link.as_deref_mut() {
                        *l = Some(kid);
                    }
                }
            }
            KeyPick::Unlink => {
                if let Some(l) = link.as_deref_mut() {
                    *l = None;
                }
            }
            KeyPick::Save => {
                app.checkpoint("color key");
                let kid = app.doc.add_color_key("", *color);
                if let Some(l) = link.as_deref_mut() {
                    *l = Some(kid);
                }
                app.notify("Saved as a colour key · rename it in the Colours tab");
            }
        }
    }
    let used = document_colors(app);
    if !used.is_empty() {
        ui.label(RichText::new("In this document").size(theme::FONT_SM).color(TEXT_FAINT));
        if let Some((i, _)) = chips(ui, &used, false) {
            *color = used[i];
            if let Some(l) = link.as_deref_mut() {
                *l = None;
            }
        }
    }
    *color != before
}

enum KeyPick {
    Use(u64),
    Unlink,
    Save,
}

/// The colour keys section of a picker: header with save, a search field
/// and the keys as named chips. `linked` is the key the colour follows.
fn key_picker(app: &mut App, ui: &mut Ui, color: crate::document::Rgba, linked: Option<u64>, linkable: bool, id: egui::Id) -> Option<KeyPick> {
    let mut out = None;
    let query_id = id.with("key_query");
    let mut query: String = ui.data(|d| d.get_temp(query_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::SWATCHES).size(12.0).color(TEXT_DIM));
        ui.label(RichText::new("Colour keys").size(theme::FONT_SM).color(TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), if linkable { "Save this colour as a key and link it" } else { "Save this colour as a key" }, None);
            if r.clicked() {
                out = Some(KeyPick::Save);
            }
        });
    });
    if let Some(k) = linked.and_then(|k| app.doc.color_key(k)).cloned() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(icon::LINK).size(12.0).color(ACCENT));
            ui.add(egui::Label::new(RichText::new(&k.name).size(theme::FONT_SM).color(theme::TEXT)).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::tip(widgets::mini_icon(ui, icon::LINK_BREAK, TEXT_DIM), "Unlink from the key (keeps the colour)", None).clicked() {
                    out = Some(KeyPick::Unlink);
                }
            });
        });
    }
    let keys = app.doc.colors.clone();
    if keys.is_empty() {
        ui.label(RichText::new("No keys yet: + saves this colour").size(theme::FONT_SM).color(TEXT_FAINT));
        return out;
    }
    if keys.len() > 8 || !query.is_empty() {
        let r = widgets::text_field(ui, &mut query, &format!("{}  Search keys", icon::MAGNIFYING_GLASS), f32::INFINITY);
        if r.changed() {
            ui.data_mut(|d| d.insert_temp(query_id, query.clone()));
        }
    }
    let q = query.trim().to_lowercase();
    let shown: Vec<&crate::document::ColorKey> = keys.iter().filter(|k| q.is_empty() || k.name.to_lowercase().contains(&q) || k.color.to_hex().to_lowercase().contains(&q)).collect();
    if shown.is_empty() {
        ui.label(RichText::new("No matching keys").size(theme::FONT_SM).color(TEXT_FAINT));
    }
    let size = 18.0;
    let gap = 4.0;
    let per_row = ((ui.available_width() + gap) / (size + gap)).floor().max(1.0) as usize;
    for row in shown.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for k in row {
                let resp = crate::color::swatch(ui, k.color, egui::Vec2::splat(size), false);
                let on = linked == Some(k.id) || (!linkable && k.color == color);
                if on {
                    ui.painter().rect_stroke(resp.rect.expand(1.5), CornerRadius::same(4), Stroke::new(1.5, ACCENT), StrokeKind::Outside);
                }
                if resp.on_hover_text(format!("{}  ·  {}", k.name, k.color.to_hex())).clicked() {
                    out = Some(KeyPick::Use(k.id));
                }
            }
        });
    }
    out
}

/// Solid fill and stroke colours used in the document (up to 32).
pub fn document_colors(app: &App) -> Vec<crate::document::Rgba> {
    let mut out: Vec<crate::document::Rgba> = Vec::new();
    fn walk<S: std::borrow::Borrow<crate::document::Shape>>(list: &[S], out: &mut Vec<crate::document::Rgba>) {
        for s in list {
            let s = s.borrow();
            let mut push = |c: crate::document::Rgba| {
                if out.len() < 32 && !out.iter().any(|d| d.to_hex() == c.to_hex() && (d.a - c.a).abs() < 0.01) {
                    out.push(c);
                }
            };
            if let Some(c) = s.fill.as_ref().and_then(|f| f.solid()) {
                push(c);
            }
            if let Some(st) = &s.stroke {
                push(st.color);
            }
            if let Some(c) = s.kind.children() {
                walk(c, out);
            }
        }
    }
    walk(app.doc.shapes(), &mut out);
    out
}

/// Small colour chips in a wrapping grid: (index, delete requested).
fn chips(ui: &mut Ui, colors: &[crate::document::Rgba], deletable: bool) -> Option<(usize, bool)> {
    let size = 16.0;
    let gap = 4.0;
    let per_row = ((ui.available_width() + gap) / (size + gap)).floor().max(1.0) as usize;
    let mut out = None;
    for (r, row) in colors.chunks(per_row).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (j, c) in row.iter().enumerate() {
                let i = r * per_row + j;
                let resp = crate::color::swatch(ui, *c, egui::Vec2::splat(size), false);
                let resp = resp.on_hover_text(if deletable { format!("{}  · right-click to delete", c.to_hex()) } else { c.to_hex() });
                if resp.clicked() {
                    out = Some((i, false));
                }
                if deletable && resp.secondary_clicked() {
                    out = Some((i, true));
                }
            }
        });
    }
    out
}
