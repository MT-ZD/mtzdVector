//! Reusable widget kit: every panel is built from these so spacing, hover
//! states and tooltips stay consistent.

use egui::{Align2, Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use egui_phosphor::regular as icon;

use super::theme::{self, ACCENT, BG_ACTIVE, BG_HOVER, BG_INPUT, BORDER, TEXT, TEXT_DIM, TEXT_FAINT};
use crate::document::Rgba;

// ---------------------------------------------------------------------------
// Tooltips
// ---------------------------------------------------------------------------

/// Tooltip with a title and a dimmed shortcut, e.g. "Union   Ctrl+G".
pub fn tip(resp: Response, label: &str, shortcut: Option<&str>) -> Response {
    if label.is_empty() {
        return resp;
    }
    resp.on_hover_ui(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).size(theme::FONT_MD).color(TEXT));
            if let Some(s) = shortcut.filter(|s| !s.is_empty()) {
                ui.add_space(6.0);
                ui.label(RichText::new(s).size(theme::FONT_SM).color(TEXT_DIM));
            }
        });
    })
}

/// Tooltip with a title, optional shortcut and a short dim description.
pub fn tip_long(resp: Response, label: &str, shortcut: Option<&str>, body: &str) -> Response {
    resp.on_hover_ui(|ui| {
        ui.set_max_width(260.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).size(theme::FONT_MD).strong().color(TEXT));
            if let Some(s) = shortcut.filter(|s| !s.is_empty()) {
                ui.add_space(6.0);
                ui.label(RichText::new(s).size(theme::FONT_SM).color(TEXT_DIM));
            }
        });
        if !body.is_empty() {
            ui.label(RichText::new(body).size(theme::FONT_SM).color(TEXT_DIM));
        }
    })
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

fn paint_icon_button(ui: &Ui, rect: Rect, resp: &Response, glyph: &str, selected: bool, enabled: bool, size: f32) {
    let p = ui.painter();
    let bg = if selected {
        ACCENT_BG
    } else if !enabled {
        Color32::TRANSPARENT
    } else if resp.is_pointer_button_down_on() {
        BG_ACTIVE
    } else if resp.hovered() {
        BG_HOVER
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, theme::radius(), bg);
    if selected {
        p.rect_stroke(rect, theme::radius(), Stroke::new(1.0, ACCENT.gamma_multiply(0.6)), StrokeKind::Inside);
    }
    let fg = if !enabled {
        TEXT_FAINT
    } else if selected {
        Color32::WHITE
    } else if resp.hovered() {
        Color32::WHITE
    } else {
        TEXT
    };
    p.text(rect.center(), Align2::CENTER_CENTER, glyph, egui::FontId::proportional(size), fg);
}

const ACCENT_BG: Color32 = Color32::from_rgb(0x2A, 0x4A, 0x78);

/// Large square tool button for the Tools column.
pub fn tool_button(ui: &mut Ui, glyph: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        paint_icon_button(ui, rect, &resp, glyph, selected, true, 18.0);
    }
    resp.on_hover_cursor(CursorIcon::Default)
}

pub fn icon_button_sized(ui: &mut Ui, glyph: &str, enabled: bool, selected: bool, size: Vec2, glyph_size: f32) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        paint_icon_button(ui, rect, &resp, glyph, selected, enabled, glyph_size);
    }
    resp
}

/// Small icon button used inline in rows (eye, lock, add, remove…).
pub fn mini_icon(ui: &mut Ui, glyph: &str, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(3), BG_ACTIVE);
        }
        let fg = if resp.hovered() { Color32::WHITE } else { color };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, egui::FontId::proportional(13.5), fg);
    }
    resp
}

/// Icon toggle: highlighted while `on`.
pub fn icon_toggle(ui: &mut Ui, on: &mut bool, glyph: &str) -> Response {
    let mut resp = icon_button_sized(ui, glyph, true, *on, vec2(26.0, 26.0), theme::ICON);
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// Text button with an optional leading icon, flat until hovered.
pub fn flat_button(ui: &mut Ui, glyph: Option<&str>, label: &str, enabled: bool) -> Response {
    let text = match glyph {
        Some(g) => format!("{g}  {label}"),
        None => label.to_string(),
    };
    let b = egui::Button::new(RichText::new(text).size(theme::FONT_MD)).fill(Color32::TRANSPARENT).min_size(vec2(0.0, 24.0));
    ui.add_enabled(enabled, b)
}

/// Solid accent button (primary action).
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let b = egui::Button::new(RichText::new(label).size(theme::FONT_MD).color(Color32::WHITE))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .corner_radius(theme::radius())
        .min_size(vec2(0.0, 26.0));
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(12.0, 4.0);
        ui.add(b)
    })
    .inner
}

/// Single-line text input with the same height and inner padding as the
/// number fields.
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(width)
            .margin(egui::Margin::symmetric(8, 5))
            .min_size(vec2(0.0, theme::ROW_H))
            .vertical_align(egui::Align::Center)
            .font(egui::TextStyle::Body),
    )
}

/// Thin vertical separator for toolbars.
pub fn vsep(ui: &mut Ui) {
    let h = ui.available_height().min(20.0);
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, h), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, BORDER));
}

// ---------------------------------------------------------------------------
// Panels
// ---------------------------------------------------------------------------

/// Tab strip for a docked panel group. Returns true when the collapse
/// chevron was toggled.
pub fn tab_group_header(ui: &mut Ui, tabs: &[&str], active: &mut usize, collapsed: &mut bool) -> Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, theme::TAB_H), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, theme::BG_HEADER);
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme::SEPARATOR));
    let mut x = rect.left() + 4.0;
    let font = egui::FontId::proportional(theme::FONT_MD);
    for (i, t) in tabs.iter().enumerate() {
        let galley = p.layout_no_wrap(t.to_string(), font.clone(), TEXT);
        let tw = galley.size().x + 18.0;
        let tr = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(tw, rect.height() - 3.0));
        let tresp = ui.interact(tr, ui.id().with(("tab", i, *t)), Sense::click());
        let sel = *active == i && !*collapsed;
        if sel {
            ui.painter().rect_filled(tr, CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, theme::BG_PANEL);
        } else if tresp.hovered() {
            ui.painter().rect_filled(tr.shrink2(vec2(0.0, 2.0)), theme::radius(), BG_HOVER);
        }
        let color = if sel { Color32::WHITE } else if tresp.hovered() { TEXT } else { TEXT_DIM };
        ui.painter().text(tr.center(), Align2::CENTER_CENTER, *t, font.clone(), color);
        if tresp.clicked() {
            if *active == i {
                *collapsed = !*collapsed;
            } else {
                *active = i;
                *collapsed = false;
            }
        }
        x += tw + 2.0;
    }
    // Collapse chevron.
    let cr = Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(20.0, 20.0));
    let cresp = ui.interact(cr, ui.id().with(("collapse", tabs.first().copied().unwrap_or(""))), Sense::click());
    if cresp.hovered() {
        ui.painter().rect_filled(cr, theme::radius(), BG_HOVER);
    }
    ui.painter().text(cr.center(), Align2::CENTER_CENTER, if *collapsed { icon::CARET_RIGHT } else { icon::CARET_DOWN }, egui::FontId::proportional(12.0), TEXT_DIM);
    if cresp.clicked() || resp.double_clicked() {
        *collapsed = !*collapsed;
    }
    tip(cresp, if *collapsed { "Expand panel" } else { "Collapse panel" }, None);
    resp
}


/// [`section`] with an icon before the title (Design panel).
pub fn section_icon<R>(ui: &mut Ui, id: &str, glyph: &str, title: &str, trailing: impl FnOnce(&mut Ui), body: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    // Figma UI3 rhythm: a full-width divider, a 36 px header in sentence
    // case, 8 px between rows and fields, 12 px below the body.
    let id = ui.id().with(("section", id));
    let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
    let p = ui.painter();
    let clip = ui.clip_rect();
    p.hline(clip.left()..=clip.right(), rect.top() + 0.5, Stroke::new(1.0, theme::SEPARATOR));
    let hot = resp.hovered();
    let tx = if glyph.is_empty() {
        0.0
    } else {
        p.text(rect.left_center(), Align2::LEFT_CENTER, glyph, egui::FontId::proportional(13.0), if hot { TEXT } else { theme::ACCENT.gamma_multiply(0.9) });
        20.0
    };
    // Capitalize the first letter only (keeps acronyms like "QR").
    let mut title = title.to_string();
    if let Some(f) = title.get(0..1) {
        title = f.to_uppercase() + &title[1..];
    }
    p.text(rect.left_center() + vec2(tx, 0.0), Align2::LEFT_CENTER, &title, egui::FontId::proportional(theme::FONT_MD), if open || hot { TEXT } else { TEXT_DIM });
    if hot || !open {
        let tw = p.layout_no_wrap(title.clone(), egui::FontId::proportional(theme::FONT_MD), TEXT).size().x;
        p.text(rect.left_center() + vec2(tx + tw + 6.0, 1.0), Align2::LEFT_CENTER, if open { icon::CARET_DOWN } else { icon::CARET_RIGHT }, egui::FontId::proportional(10.0), TEXT_DIM);
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
    trailing(&mut child);
    let mut expanded = false;
    if resp.clicked() {
        open = !open;
        expanded = open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    if open {
        let r = ui.scope(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            body(ui)
        });
        if expanded {
            scroll_into_view(ui, rect.union(r.response.rect));
        }
        ui.add_space(12.0);
        Some(r.inner)
    } else {
        ui.add_space(2.0);
        None
    }
}

/// Scroll the enclosing scroll area so `rect` shows: the least scrolling
/// that reveals all of it, or its top when it is taller than the view.
pub fn scroll_into_view(ui: &Ui, rect: egui::Rect) {
    let view = ui.clip_rect();
    if view.contains_rect(rect) {
        return;
    }
    let align = (rect.height() > view.height()).then_some(egui::Align::TOP);
    ui.scroll_to_rect(rect, align);
}

/// Ask the expanding editor `key` to scroll into view when it is drawn
/// (it usually appears this frame, below the control that opened it).
pub fn request_reveal(ctx: &egui::Context, key: &str) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(("reveal", key)), true));
}

/// Scroll `rect` into view if `key` asked for it (once).
pub fn reveal(ui: &Ui, key: &str, rect: egui::Rect) {
    let id = egui::Id::new(("reveal", key));
    if ui.data_mut(|d| d.remove_temp::<bool>(id)).unwrap_or(false) {
        scroll_into_view(ui, rect);
    }
}

/// Label + content row with a fixed label column, used for property grids.
pub fn prop_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SP1;
        let (rect, _) = ui.allocate_exact_size(vec2(64.0, theme::ROW_H), Sense::hover());
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, label, egui::FontId::proportional(theme::FONT_SM), TEXT_DIM);
        add(ui)
    })
    .inner
}

/// Centered empty-state message.
pub fn empty_state(ui: &mut Ui, glyph: &str, title: &str, body: &str) {
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(glyph).size(26.0).color(TEXT_FAINT));
        ui.add_space(2.0);
        ui.label(RichText::new(title).size(theme::FONT_MD).color(TEXT_DIM));
        if !body.is_empty() {
            ui.label(RichText::new(body).size(theme::FONT_SM).color(TEXT_FAINT));
        }
    });
    ui.add_space(18.0);
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct Num {
    pub speed: f64,
    pub min: f64,
    pub max: f64,
    pub decimals: usize,
    pub suffix: &'static str,
    /// A document length: shown in the document unit (value stays px).
    pub length: bool,
}

impl Default for Num {
    fn default() -> Self {
        Num { speed: 1.0, min: f64::NEG_INFINITY, max: f64::INFINITY, decimals: 2, suffix: "", length: false }
    }
}

impl Num {
    /// Length in document units (converted from/to px).
    pub fn px() -> Self {
        Num { length: true, ..Default::default() }
    }
    /// Raw pixels regardless of the document unit.
    pub fn raw_px() -> Self {
        Num { suffix: " px", ..Default::default() }
    }
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }
    pub fn speed(mut self, s: f64) -> Self {
        self.speed = s;
        self
    }
    pub fn suffix(mut self, s: &'static str) -> Self {
        self.suffix = s;
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = d;
        self
    }
}

/// Numeric field with an Affinity-style drag-to-scrub label (drag the label
/// left/right to change the value; Shift = ×10, Alt = ×0.1), an input-styled
/// value box and a unit suffix.
pub fn num_field(ui: &mut Ui, label: &str, value: &mut f64, opts: Num, width: f32) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let total = vec2(width, theme::ROW_H);
        let (rect, base) = ui.allocate_exact_size(total, Sense::hover());
        let label_w = if label.is_empty() { 0.0 } else { (label.chars().count() as f32 * 7.0 + 10.0).max(18.0) };
        let lrect = Rect::from_min_size(rect.min, vec2(label_w, rect.height()));
        let frect = Rect::from_min_max(pos2(lrect.right(), rect.top()), rect.max);
        let p = ui.painter();
        p.rect_filled(rect, theme::radius(), BG_INPUT);

        let mut resp = ui.interact(lrect, base.id.with("scrub"), Sense::drag());
        if !label.is_empty() {
            let hovered = resp.hovered() || resp.dragged();
            ui.painter().text(lrect.center(), Align2::CENTER_CENTER, label, egui::FontId::proportional(theme::FONT_SM), if hovered { ACCENT } else { TEXT_DIM });
            if hovered {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
        }
        let mut changed = false;
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt(base.id).max_rect(frect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.style_mut().visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
        child.style_mut().visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        child.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::NONE;
        child.style_mut().visuals.widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
        child.style_mut().visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER);
        let (unit, dpi) = crate::units::current();
        let k = if opts.length { unit.per_px(dpi) } else { 1.0 };
        let mut shown = *value * k;
        let (suffix, decimals) = if opts.length && unit != crate::units::Unit::Px {
            (match unit {
                crate::units::Unit::Mm => " mm",
                crate::units::Unit::Cm => " cm",
                crate::units::Unit::In => " in",
                _ => " pt",
            }, unit.decimals())
        } else if opts.length {
            (" px", opts.decimals)
        } else {
            (opts.suffix, opts.decimals)
        };
        // Dragging is handled below (endless scrubbing); the value box itself
        // only edits on click.
        let dv = egui::DragValue::new(&mut shown)
            .speed(0.0)
            .range(opts.min * k..=opts.max * k)
            .max_decimals(decimals)
            .suffix(suffix)
            .custom_parser(move |t| crate::units::eval_expr(t, opts.length.then_some((unit, dpi))))
            .update_while_editing(false);
        let r = child.add_sized(frect.size(), dv);
        if r.changed() && !r.dragged() {
            *value = (shown / k).clamp(opts.min, opts.max);
        }
        let dx = scrub(ui, &resp, &r);
        // Accumulate unrounded: callers may round (integer fields) every
        // frame, which would otherwise swallow small motions.
        let acc_key = base.id.with("scrub_acc");
        if resp.drag_started() || r.drag_started() {
            ui.data_mut(|d| d.insert_temp(acc_key, *value));
        }
        if dx != 0.0 {
            let m = ui.input(|i| i.modifiers);
            let f = if m.shift { 10.0 } else if m.alt { 0.1 } else { 1.0 };
            let acc = ui.data(|d| d.get_temp::<f64>(acc_key)).unwrap_or(*value);
            let acc = (acc + dx * opts.speed * f).clamp(opts.min, opts.max);
            ui.data_mut(|d| d.insert_temp(acc_key, acc));
            let rounded = if opts.decimals == 0 { acc.round() } else { acc };
            if rounded != *value {
                *value = rounded;
                changed = true;
            }
        }
        let focused = r.has_focus();
        if focused {
            ui.painter().rect_stroke(rect, theme::radius(), Stroke::new(1.0, ACCENT), StrokeKind::Inside);
        } else if r.hovered() || resp.hovered() {
            ui.painter().rect_stroke(rect, theme::radius(), Stroke::new(1.0, BORDER), StrokeKind::Inside);
        }
        if r.changed() {
            changed = true;
        }
        resp = resp.union(r);
        if changed {
            resp.mark_changed();
        }
        resp
    })
    .inner
}

/// Endless horizontal scrubbing (like Blender): while either part of a number
/// field is dragged the pointer is locked and hidden, and raw mouse motion
/// drives the value, so the drag never runs into the screen edge. Where the
/// pointer can't be locked it wraps around the window edges instead.
/// Returns the horizontal motion in points for this frame.
fn scrub(ui: &Ui, a: &Response, b: &Response) -> f64 {
    let started = a.drag_started() || b.drag_started();
    let stopped = a.drag_stopped() || b.drag_stopped();
    let dragged = a.dragged() || b.dragged();
    let key = a.id.with("scrub_state");
    let ctx = ui.ctx();
    if started {
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::Locked));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(false));
        ctx.data_mut(|d| d.insert_temp(key, (false, false)));
    }
    if stopped {
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
        ctx.data_mut(|d| d.remove::<(bool, bool)>(key));
    }
    if !dragged {
        return 0.0;
    }
    ctx.set_cursor_icon(CursorIcon::None);
    // (raw motion seen, skip the next pointer delta after a warp)
    let (mut raw_seen, skip) = ctx.data(|d| d.get_temp::<(bool, bool)>(key)).unwrap_or((false, false));
    let ppp = ctx.pixels_per_point();
    let raw: f32 = ui.input(|i| i.events.iter().map(|e| if let egui::Event::MouseMoved(d) = e { d.x } else { 0.0 }).sum());
    let mut dx = 0.0;
    let mut warped = false;
    if raw != 0.0 {
        raw_seen = true;
        dx = raw / ppp;
    } else if !raw_seen {
        if !skip {
            dx = a.drag_delta().x + b.drag_delta().x;
        }
        // No raw motion (pointer lock unsupported): wrap at the window edges.
        if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
            let screen = ctx.content_rect();
            let x = if p.x <= screen.left() + 2.0 {
                Some(screen.right() - 4.0)
            } else if p.x >= screen.right() - 2.0 {
                Some(screen.left() + 4.0)
            } else {
                None
            };
            if let Some(x) = x {
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(pos2(x, p.y)));
                warped = true;
            }
        }
    }
    ctx.data_mut(|d| d.insert_temp(key, (raw_seen, warped)));
    dx as f64
}

/// Segmented control.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    segmented_sized(ui, value, options, 0.0)
}

/// Segmented control; `seg_w` > 0 forces equal segment widths.
pub fn segmented_sized<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)], seg_w: f32) -> bool {
    let mut changed = false;
    egui::Frame::new().fill(BG_INPUT).corner_radius(theme::radius()).inner_margin(egui::Margin::same(2)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            for (v, label) in options {
                let sel = *value == *v;
                let text = RichText::new(*label).size(theme::FONT_SM).color(if sel { Color32::WHITE } else { TEXT_DIM });
                let b = egui::Button::new(text)
                    .fill(if sel { BG_ACTIVE } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::same(3))
                    .min_size(vec2(seg_w.max(28.0), 18.0));
                if ui.add(b).clicked() && !sel {
                    *value = *v;
                    changed = true;
                }
            }
        });
    });
    changed
}

/// Segmented control with painted glyphs: `paint(painter, rect, value,
/// colour)` draws each option; `options` pairs values with tooltips.
pub fn segmented_glyphs<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)], size: egui::Vec2, paint: impl Fn(&egui::Painter, egui::Rect, T, Color32)) -> bool {
    let mut changed = false;
    egui::Frame::new().fill(BG_INPUT).corner_radius(theme::radius()).inner_margin(egui::Margin::same(2)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            for (v, label) in options {
                let sel = *value == *v;
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                let bg = if sel {
                    BG_ACTIVE
                } else if resp.hovered() {
                    theme::BG_HOVER
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
                paint(ui.painter(), rect, *v, if sel { Color32::WHITE } else { TEXT_DIM });
                if tip(resp, label, None).clicked() && !sel {
                    *value = *v;
                    changed = true;
                }
            }
        });
    });
    changed
}

/// Segmented control spanning `width` (equal segments).
pub fn segmented_width<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)], width: f32) -> bool {
    let n = options.len().max(1) as f32;
    segmented_sized(ui, value, options, ((width - 4.0 - (n - 1.0) * 2.0) / n).floor())
}

/// Styled combo box.
pub fn dropdown<R>(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, selected: impl Into<egui::WidgetText>, width: f32, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    egui::ComboBox::from_id_salt(id).selected_text(selected).width(width).height(320.0).show_ui(ui, add).inner
}

/// Label + slider + value, compact.
pub fn slider_row(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str) -> bool {
    prop_row(ui, label, |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 56.0).max(40.0);
        ui.add(egui::Slider::new(value, range).suffix(suffix).max_decimals(0)).changed()
    })
}

// ---------------------------------------------------------------------------
// Color wells
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WellTarget {
    #[default]
    Fill,
    Stroke,
}

/// Paint of a well: none, a solid color, a gradient (first, last colour) or an image fill.
#[derive(Clone, Copy)]
pub enum WellPaint {
    None,
    Solid(Rgba),
    Gradient(Rgba, Rgba),
    Image,
}

/// Horizontal strip of a gradient's colours (over a checkerboard if any stop is transparent).
pub fn ramp_strip(p: &egui::Painter, r: Rect, g: &crate::document::Gradient) {
    if g.has_alpha() {
        crate::color::checkerboard(&p.with_clip_rect(r), r, 5.0);
    }
    let mut stops = g.padded_stops();
    stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    let mut mesh = egui::Mesh::default();
    for w in stops.windows(2) {
        let x0 = r.left() + r.width() * w[0].offset.clamp(0.0, 1.0);
        let x1 = r.left() + r.width() * w[1].offset.clamp(0.0, 1.0);
        let (a, b) = (w[0].color.to_color32(1.0), w[1].color.to_color32(1.0));
        let i = mesh.vertices.len() as u32;
        for (pos, c) in [(egui::pos2(x0, r.top()), a), (egui::pos2(x1, r.top()), b), (egui::pos2(x1, r.bottom()), b), (egui::pos2(x0, r.bottom()), a)] {
            mesh.colored_vertex(pos, c);
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    p.add(mesh);
}

fn paint_well(p: &egui::Painter, r: Rect, paint: WellPaint, ring: bool) {
    match paint {
        WellPaint::None => {
            p.rect_filled(r, CornerRadius::same(3), Color32::WHITE);
            p.line_segment([r.left_bottom() + vec2(2.0, -2.0), r.right_top() + vec2(-2.0, 2.0)], Stroke::new(2.0, theme::DANGER));
        }
        WellPaint::Solid(c) => {
            if c.a < 1.0 {
                crate::color::checkerboard(&p.with_clip_rect(r), r, 5.0);
            }
            p.rect_filled(r, CornerRadius::same(3), c.to_color32(1.0));
        }
        WellPaint::Gradient(a, b) => {
            ramp_strip(p, r, &crate::document::Gradient::new(crate::document::GradientKind::Linear, a, b));
        }
        WellPaint::Image => {
            p.rect_filled(r, CornerRadius::same(3), Color32::from_gray(0x55));
            p.text(r.center(), Align2::CENTER_CENTER, icon::IMAGE, egui::FontId::proportional(14.0), Color32::WHITE);
        }
    }
    if ring {
        p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
    }
}

/// Compact inline well used in toolbars (single color chip).
pub fn mini_well(ui: &mut Ui, paint: WellPaint, is_stroke: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
    let p = ui.painter();
    paint_well(p, rect, paint, true);
    if is_stroke {
        let inner = rect.shrink(6.0);
        p.rect_filled(inner, CornerRadius::same(1), theme::BG_PANEL);
    }
    if resp.hovered() {
        p.rect_stroke(rect.expand(1.5), CornerRadius::same(4), Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
    }
    resp
}

// ---------------------------------------------------------------------------
// Anchor selector
// ---------------------------------------------------------------------------

/// 3×3 anchor selector (reference point for X/Y and W/H edits).
pub fn anchor_selector(ui: &mut Ui, anchor: &mut (u8, u8)) -> Response {
    let cell = 10.0;
    let size = vec2(cell * 3.0 + 8.0, cell * 3.0 + 8.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, theme::radius(), BG_INPUT);
    let origin = rect.min + vec2(4.0 + cell / 2.0, 4.0 + cell / 2.0);
    for y in 0..3u8 {
        for x in 0..3u8 {
            let c: Pos2 = origin + vec2(x as f32 * cell, y as f32 * cell);
            let sel = *anchor == (x, y);
            if sel {
                p.circle_filled(c, 3.2, ACCENT);
            } else {
                p.circle_stroke(c, 2.2, Stroke::new(1.0, TEXT_DIM));
            }
        }
    }
    let mut resp = resp;
    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let lx = ((pos.x - rect.left() - 4.0) / cell).floor().clamp(0.0, 2.0) as u8;
            let ly = ((pos.y - rect.top() - 4.0) / cell).floor().clamp(0.0, 2.0) as u8;
            *anchor = (lx, ly);
            resp.mark_changed();
        }
    }
    tip(resp, "Transform origin", None)
}

/// Popover id helper.
pub fn popup_id(name: &str) -> Id {
    Id::new(("mtzd-popup", name))
}
