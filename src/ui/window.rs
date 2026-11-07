//! Client-side window frame: the menu bar doubles as the title bar (the OS
//! title bar is turned off). Drag it to move the window, double-click to
//! maximize; window buttons on the right; the window edges resize it.

use egui::{Align2, Color32, CursorIcon, Id, Rect, ResizeDirection, Sense, Stroke, Ui, ViewportCommand, pos2, vec2};
use egui_phosphor::regular as icon;

use super::theme::{self, TEXT, TEXT_DIM};
use crate::app::App;
use crate::commands::Cmd;

/// Width of the invisible resize band along the window edges.
const EDGE: f32 = 5.0;
/// Corner grab size (both directions).
const CORNER: f32 = 12.0;

fn maximized(ui: &Ui) -> bool {
    ui.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false))
}

/// Register the title bar's background as the window drag area. Call before
/// adding the bar's widgets so they take precedence.
pub fn drag_area(ui: &mut Ui, rect: Rect) {
    let r = ui.interact(rect, Id::new("titlebar_drag"), Sense::click_and_drag());
    if r.double_clicked() {
        let m = maximized(ui);
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!m));
    } else if r.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Minimize, maximize/restore and close, right to left.
pub fn buttons(ui: &mut Ui, app: &mut App) {
    let h = ui.max_rect().height();
    let m = maximized(ui);
    let button = |ui: &mut Ui, glyph: &str, tip: &str, danger: bool| {
        let (rect, resp) = ui.allocate_exact_size(vec2(42.0, h), Sense::click());
        let (bg, fg) = match (resp.hovered(), danger) {
            (true, true) => (Color32::from_rgb(0xD9, 0x3A, 0x3A), Color32::WHITE),
            (true, false) => (theme::BG_HOVER, TEXT),
            _ => (Color32::TRANSPARENT, TEXT_DIM),
        };
        ui.painter().rect_filled(rect, 0.0, bg);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, egui::FontId::proportional(14.0), fg);
        super::widgets::tip(resp, tip, None).clicked()
    };
    if button(ui, icon::X, "Close", true) {
        let ctx = ui.ctx().clone();
        app.run_cmd(Cmd::Quit, &ctx);
    }
    if button(ui, if m { icon::COPY_SIMPLE } else { icon::SQUARE }, if m { "Restore" } else { "Maximize" }, false) {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!m));
    }
    if button(ui, icon::MINUS, "Minimize", false) {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// While a modal (welcome screen, dialogs) covers the app, keep the title
/// bar usable: a top-level copy of the drag area and window buttons.
pub fn modal_overlay(app: &mut App, ctx: &egui::Context) {
    let modal = app.ui.welcome_open || ctx.memory(|m| m.top_modal_layer()).is_some();
    if !modal {
        return;
    }
    let screen = ctx.content_rect();
    let bar = Rect::from_min_size(screen.min, vec2(screen.width(), theme::MENUBAR_H));
    egui::Area::new(Id::new("modal_titlebar")).order(egui::Order::Tooltip).fixed_pos(bar.min).interactable(true).show(ctx, |ui| {
        ui.set_min_size(bar.size());
        let drag = Rect::from_min_max(bar.min, pos2(bar.right() - 3.0 * 42.0, bar.bottom()));
        let r = ui.interact(drag, Id::new("modal_titlebar_drag"), Sense::click_and_drag());
        if r.double_clicked() {
            let m = maximized(ui);
            ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!m));
        } else if r.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
        child.spacing_mut().item_spacing.x = 0.0;
        buttons(&mut child, app);
    });
}

/// Resize bands along the window edges, and a hairline border, on top of
/// everything (not while maximized).
pub fn frame(ui: &mut Ui) {
    if maximized(ui) {
        return;
    }
    let screen = ui.ctx().content_rect();
    let layer = egui::LayerId::new(egui::Order::Foreground, Id::new("window_frame"));
    ui.ctx().layer_painter(layer).rect_stroke(screen, 0.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    let (l, r, t, b) = (screen.left(), screen.right(), screen.top(), screen.bottom());
    // Later bands win where they overlap: corners go last.
    let bands = [
        (Rect::from_min_max(pos2(l, t), pos2(r, t + EDGE)), ResizeDirection::North, CursorIcon::ResizeNorth),
        (Rect::from_min_max(pos2(l, b - EDGE), pos2(r, b)), ResizeDirection::South, CursorIcon::ResizeSouth),
        (Rect::from_min_max(pos2(l, t), pos2(l + EDGE, b)), ResizeDirection::West, CursorIcon::ResizeWest),
        (Rect::from_min_max(pos2(r - EDGE, t), pos2(r, b)), ResizeDirection::East, CursorIcon::ResizeEast),
        (Rect::from_min_max(pos2(l, t), pos2(l + CORNER, t + CORNER)), ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest),
        (Rect::from_min_max(pos2(r - CORNER, t), pos2(r, t + CORNER)), ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast),
        (Rect::from_min_max(pos2(l, b - CORNER), pos2(l + CORNER, b)), ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest),
        (Rect::from_min_max(pos2(r - CORNER, b - CORNER), pos2(r, b)), ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast),
    ];
    egui::Area::new(Id::new("window_resize")).order(egui::Order::Tooltip).fixed_pos(screen.min).interactable(true).show(ui.ctx(), |ui| {
        ui.set_clip_rect(screen);
        for (i, (rect, dir, cursor)) in bands.into_iter().enumerate() {
            let resp = ui.interact(rect, Id::new(("resize_band", i)), Sense::drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(cursor);
            }
            if resp.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
                ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
            }
        }
    });
}
