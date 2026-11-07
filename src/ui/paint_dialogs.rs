//! Dialogs for the drawing operations: blend steps, envelope warp (live
//! preview on the canvas) and image trace.

use egui::RichText;
use egui_phosphor::regular as icon;

use super::dialogs::{footer, modal, modal_title};
use super::theme;
use super::widgets;
use crate::app::App;
use crate::paint_ops::WarpStyle;

pub fn show(app: &mut App, ctx: &egui::Context) {
    blend_dialog(app, ctx);
    warp_dialog(app, ctx);
    trace_dialog(app, ctx);
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(theme::FONT_SM).color(theme::TEXT_FAINT));
}

fn blend_dialog(app: &mut App, ctx: &egui::Context) {
    if !app.ui.blend.open {
        return;
    }
    let mut action = None;
    // A window, not a modal: the canvas shows the blend as it changes.
    egui::Window::new(format!("{}  Blend Shapes", icon::CIRCLES_THREE))
        .collapsible(false)
        .resizable(false)
        .default_width(300.0)
        .anchor(egui::Align2::RIGHT_TOP, [-330.0, 90.0])
        .show(ctx, |ui| {
            let mut steps = app.ui.blend.steps as f32;
            if widgets::slider_row(ui, "Steps", &mut steps, 1.0..=50.0, "") {
                app.ui.blend.steps = steps.round().clamp(1.0, 500.0) as u32;
            }
            hint(ui, "Shapes morphing between the two selected ones, in their colours, stroke widths and opacity. The blend stays live: edit either end shape or the steps in the Design panel, or expand it.");
            action = footer(ui, &[], &["Cancel"], "Apply");
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                action = Some("Apply".into());
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some("Cancel".into());
            }
        });
    match action.as_deref() {
        Some("Apply") => app.end_blend(true),
        Some(_) => app.end_blend(false),
        None => app.blend_preview(),
    }
}

fn warp_dialog(app: &mut App, ctx: &egui::Context) {
    if !app.ui.warp.open {
        return;
    }
    let mut action = None;
    // A window, not a modal: the canvas shows the preview.
    egui::Window::new(format!("{}  Envelope Warp", icon::FLAG))
        .collapsible(false)
        .resizable(false)
        .default_width(300.0)
        .anchor(egui::Align2::RIGHT_TOP, [-330.0, 90.0])
        .show(ctx, |ui| {
            let st = &mut app.ui.warp;
            widgets::prop_row(ui, "Style", |ui| {
                widgets::dropdown(ui, "warp_style", st.style.label(), 160.0, |ui| {
                    for s in WarpStyle::ALL {
                        if ui.selectable_label(s == st.style, s.label()).clicked() {
                            st.style = s;
                        }
                    }
                });
            });
            let pct = |ui: &mut egui::Ui, label: &str, v: &mut f64| {
                let mut f = (*v * 100.0) as f32;
                if widgets::slider_row(ui, label, &mut f, -100.0..=100.0, "%") {
                    *v = (f / 100.0) as f64;
                }
            };
            pct(ui, "Bend", &mut st.bend);
            pct(ui, "Horizontal", &mut st.hdist);
            pct(ui, "Vertical", &mut st.vdist);
            hint(ui, "Horizontal and Vertical distort like perspective. The warp stays live: change it in the Design panel, or expand it into plain paths.");
            action = footer(ui, &[], &["Cancel"], "Apply");
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                action = Some("Apply".into());
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some("Cancel".into());
            }
        });
    match action.as_deref() {
        Some("Apply") => app.end_warp(true),
        Some(_) => app.end_warp(false),
        None => app.warp_preview(),
    }
}

fn trace_dialog(app: &mut App, ctx: &egui::Context) {
    if !app.ui.trace.open {
        return;
    }
    let mut action = None;
    let r = modal("trace_dialog").show(ctx, |ui| {
        ui.set_width(340.0);
        modal_title(ui, icon::SCAN, "Trace Image");
        let st = &mut app.ui.trace;
        widgets::prop_row(ui, "Preset", |ui| {
            let mut c = st.colours;
            if widgets::segmented(ui, &mut c, &[(2, "Black & White"), (6, "Poster"), (16, "Photo")]) {
                st.colours = c;
            }
        });
        let mut c = st.colours as f32;
        if widgets::slider_row(ui, "Colours", &mut c, 2.0..=32.0, "") {
            st.colours = c.round() as u32;
        }
        let mut d = st.detail * 100.0;
        if widgets::slider_row(ui, "Detail", &mut d, 0.0..=100.0, "%") {
            st.detail = d / 100.0;
        }
        let mut s = st.smooth * 100.0;
        if widgets::slider_row(ui, "Smoothing", &mut s, 0.0..=100.0, "%") {
            st.smooth = s / 100.0;
        }
        let mut sp = st.speckle as f32;
        if widgets::slider_row(ui, "Specks", &mut sp, 0.0..=200.0, " px") {
            st.speckle = sp as f64;
        }
        hint(ui, "Stacks one filled shape per colour above the selection (lightest at the bottom). Regions smaller than Specks are dropped.");
        action = footer(ui, &[], &["Cancel"], "Trace");
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            action = Some("Trace".into());
        }
    });
    if r.should_close() && action.is_none() {
        action = Some("Cancel".into());
    }
    match action.as_deref() {
        Some("Trace") => {
            app.ui.trace.open = false;
            app.trace_selection();
        }
        Some(_) => app.ui.trace.open = false,
        None => {}
    }
}
