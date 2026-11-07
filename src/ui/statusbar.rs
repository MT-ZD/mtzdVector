//! Status bar: tool hint with live modifier-key hints, cursor position,
//! selection size, object count and toasts.

use egui::{Align2, Color32, CornerRadius, RichText, Sense, Stroke, Ui, vec2};

use super::theme::{self, ACCENT, TEXT_DIM, TEXT_FAINT};
use crate::app::{App, Tool};

/// Modifier hints for the current tool: (key, meaning).
fn modifier_hints(t: Tool) -> &'static [(&'static str, &'static str)] {
    match t {
        Tool::Select => &[("Shift", "constrain · add to selection"), ("Alt", "duplicate · scale from center · measure"), ("Ctrl", "no snapping")],
        Tool::Node => &[("Shift", "add to selection"), ("Alt", "break handle symmetry"), ("Ctrl", "bend segment · pull handles from a node")],
        Tool::Fill => &[("Shift", "45° angles"), ("Double-click", "add stop"), ("Del", "remove stop")],
        Tool::Rect | Tool::Ellipse | Tool::Star | Tool::Polygon | Tool::Prim(_) => &[("Shift", "keep proportions · add to selection"), ("Alt", "draw from center"), ("Ctrl", "no snapping")],
        Tool::Corner => &[("Shift", "add to selection"), ("Alt", "single rectangle corner")],
        Tool::ShapeBuilder => &[("Alt", "subtract regions"), ("Shift", "add shapes")],
        Tool::Knife => &[("Shift", "straight cut")],
        Tool::Pencil | Tool::Brush => &[],
        Tool::Eyedropper => &[("Shift", "apply to stroke")],
        Tool::Zoom => &[("Alt", "zoom out")],
        Tool::Hand => &[("Ctrl", "scroll to zoom")],
        Tool::Text | Tool::FrameText => &[("Shift", "square frame")],
        Tool::Pen => &[("Shift", "45° angles"), ("Alt", "break handles while dragging"), ("Ctrl", "no snapping")],
        Tool::Artboard => &[("Shift", "square · constrain move"), ("Alt", "clone artboard")],
    }
}

fn key_chip(ui: &mut Ui, key: &str, text: &str, held: bool) {
    let font = egui::FontId::proportional(10.5);
    let g = ui.painter().layout_no_wrap(key.to_string(), font.clone(), Color32::WHITE);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 10.0, 16.0), Sense::hover());
    let (bg, fg) = if held { (ACCENT, Color32::WHITE) } else { (theme::BG_HOVER, TEXT_DIM) };
    ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, key, font, fg);
    ui.label(RichText::new(text).size(theme::FONT_SM).color(if held { theme::TEXT } else { TEXT_DIM }));
    ui.add_space(6.0);
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let frame = egui::Frame::new().fill(theme::BG_HEADER).inner_margin(egui::Margin::symmetric(10, 0));
    egui::Panel::bottom("status").exact_size(theme::STATUSBAR_H).frame(frame).show(ui, |ui| {
        let full = ui.max_rect();
        ui.painter().hline(full.x_range(), full.top() + 0.5, Stroke::new(1.0, theme::SEPARATOR));
        // Right side first: the tool hints get whatever room is left (and are
        // clipped rather than drawn under a long status message).
        let inner = full.shrink2(egui::vec2(10.0, 0.0));
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
        {
            let ui = &mut right;
            ui.spacing_mut().item_spacing.x = 14.0;
            let mono = |s: String| RichText::new(s).size(theme::FONT_SM).color(TEXT_DIM);
            unit_menu(app, ui);
            ui.label(mono(format!("{} selected · {} objects", app.selection.len(), app.doc.shapes().len())));
            let f = crate::units::fmt_value;
            if let Some(b) = app.selection_bounds() {
                ui.label(mono(format!("W {}  H {}", f(b.width()), f(b.height()))));
            }
            match app.cursor_doc {
                // Relative to the active artboard, like the rulers.
                Some(p) => ui.label(mono(format!("X {}  Y {}", f(p.x - app.active().x), f(p.y - app.active().y)))),
                None => ui.label(RichText::new("X –  Y –").size(theme::FONT_SM).color(TEXT_FAINT)),
            };
            if app.fonts.downloads_in_flight > 0 {
                ui.spinner();
            }
            let msg = app
                .toast
                .as_ref()
                .filter(|(_, t)| app.now - t < 4.0)
                .map(|(m, _)| m.clone())
                .or_else(|| app.fonts.status.clone().filter(|_| app.fonts.downloads_in_flight > 0));
            if let Some(m) = msg {
                ui.label(RichText::new(m).size(theme::FONT_SM).color(ACCENT));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            }
        }
        let left_rect = inner.with_max_x((right.min_rect().left() - 16.0).max(inner.left()));
        let mut left = ui.new_child(egui::UiBuilder::new().max_rect(left_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        left.set_clip_rect(left_rect.intersect(ui.clip_rect()));
        {
            let ui = &mut left;
            ui.spacing_mut().item_spacing.x = 4.0;
            let m = ui.input(|i| i.modifiers);
            let hints = modifier_hints(app.tool);
            let any_held = hints.iter().any(|(k, _)| held(k, m));
            if !any_held {
                ui.label(RichText::new(app.tool.icon()).size(13.0).color(TEXT_DIM));
                let hint = app.tool.hint().split(" · ").take(3).collect::<Vec<_>>().join(" · ");
                ui.add(egui::Label::new(RichText::new(hint).size(theme::FONT_SM).color(TEXT_DIM)).wrap_mode(egui::TextWrapMode::Extend));
                ui.add_space(10.0);
            }
            for (k, t) in hints {
                key_chip(ui, k, t, held(k, m));
            }
        }
    });
}

fn held(key: &str, m: egui::Modifiers) -> bool {
    match key {
        "Shift" => m.shift,
        "Alt" => m.alt,
        "Ctrl" => m.command || m.ctrl,
        _ => false,
    }
}

/// Document unit switcher.
pub fn unit_menu(app: &mut App, ui: &mut Ui) {
    let cur = app.doc.unit;
    let r = ui.add(egui::Button::new(RichText::new(format!("{}  {}", egui_phosphor::regular::RULER, cur.suffix())).size(theme::FONT_SM)).frame(false));
    let r = super::widgets::tip(r, "Document units", None);
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(150.0);
        for u in crate::units::Unit::ALL {
            if ui.add(egui::Button::new(format!("{}  ({})", u.label(), u.suffix())).selected(u == cur)).clicked() {
                app.set_unit(u);
                ui.close();
            }
        }
    });
}
