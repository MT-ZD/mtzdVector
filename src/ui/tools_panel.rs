//! Vertical Tools column. Shape and text tools share one grouped button;
//! its flyout docks beside the column (corner triangle, right-click or
//! press-and-hold), Affinity style, as a two-column grid that stays open.

use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use kurbo::Shape as _;

use super::theme;
use super::widgets;
use crate::app::{App, Tool};
use crate::commands::Cmd;
use crate::document::ShapeKind;
use crate::prims::PrimKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Shapes,
    Text,
}

impl Group {
    pub fn tools(self) -> Vec<Tool> {
        match self {
            Group::Shapes => [Tool::Rect, Tool::Ellipse, Tool::Star, Tool::Polygon].into_iter().chain(PrimKind::ALL.into_iter().map(Tool::Prim)).collect(),
            Group::Text => vec![Tool::Text, Tool::FrameText],
        }
    }

    fn title(self) -> &'static str {
        match self {
            Group::Shapes => "Shapes",
            Group::Text => "Text",
        }
    }
}

fn tool_tip(r: egui::Response, t: Tool, extra: &str) -> egui::Response {
    let sc = Cmd::Tool(t).meta().keys.iter().map(crate::commands::format_shortcut).collect::<Vec<_>>().join(" / ");
    let body = if extra.is_empty() { t.hint().to_string() } else { format!("{}\n{extra}", t.hint()) };
    widgets::tip_long(r, t.name(), (!sc.is_empty()).then_some(sc.as_str()), &body)
}

fn group_sep(ui: &mut Ui) {
    ui.add_space(3.0);
    let r = ui.available_rect_before_wrap();
    ui.painter().hline(r.left() + 7.0..=r.right() - 7.0, r.top(), Stroke::new(1.0, theme::BORDER));
    ui.add_space(4.0);
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let frame = egui::Frame::new().fill(theme::BG_PANEL).inner_margin(egui::Margin::symmetric(5, 8));
    egui::Panel::left("tools").exact_size(theme::TOOLS_W).resizable(false).frame(frame).show(ui, |ui| {
        let full = ui.max_rect().expand2(vec2(5.0, 8.0));
        ui.painter().vline(full.right() - 0.5, full.y_range(), Stroke::new(1.0, theme::SEPARATOR));
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let simple = |ui: &mut Ui, app: &mut App, t: Tool| {
                let r = widgets::tool_button(ui, t.icon(), app.tool == t);
                if tool_tip(r, t, "").clicked() {
                    app.set_tool(t);
                }
            };
            simple(ui, app, Tool::Select);
            simple(ui, app, Tool::Node);
            simple(ui, app, Tool::Artboard);
            group_sep(ui);
            simple(ui, app, Tool::Pen);
            simple(ui, app, Tool::Pencil);
            simple(ui, app, Tool::Brush);
            simple(ui, app, Tool::Corner);
            simple(ui, app, Tool::Knife);
            group_sep(ui);
            let shown = if app.tool.is_shape() { app.tool } else { app.shape_tool };
            tool_group(app, ui, Group::Shapes, shown);
            let shown = if app.tool.is_text() { app.tool } else { app.text_tool };
            tool_group(app, ui, Group::Text, shown);
            group_sep(ui);
            simple(ui, app, Tool::ShapeBuilder);
            group_sep(ui);
            simple(ui, app, Tool::Fill);
            simple(ui, app, Tool::Eyedropper);
            simple(ui, app, Tool::Zoom);
            simple(ui, app, Tool::Hand);
        });
    });
}

fn tool_group(app: &mut App, ui: &mut Ui, group: Group, shown: Tool) {
    let tools = group.tools();
    let open = app.ui.tool_flyout == Some(group);
    let active = tools.contains(&app.tool);
    let r = tool_glyph_button(ui, shown, active);
    app.ui.flyout_anchor.retain(|(g, _)| *g != group);
    app.ui.flyout_anchor.push((group, r.rect));
    // Flyout indicator (accent while open).
    let rect = r.rect;
    let a = pos2(rect.right() - 3.0, rect.bottom() - 3.0);
    let col = if open { theme::ACCENT } else { theme::TEXT_DIM };
    ui.painter().add(egui::Shape::convex_polygon(vec![a, a - vec2(5.0, 0.0), a - vec2(0.0, 5.0)], col, Stroke::NONE));
    let held = r.is_pointer_button_down_on() && ui.input(|i| i.pointer.press_start_time().map(|t| i.time - t > 0.35).unwrap_or(false));
    let hold_key = ui.id().with(("flyout_held", group as u8));
    let was_held = ui.data(|d| d.get_temp::<bool>(hold_key).unwrap_or(false));
    if held && !was_held {
        app.ui.tool_flyout = Some(group);
    }
    ui.data_mut(|d| d.insert_temp(hold_key, held));
    let extra = if open { "Click again to close the flyout" } else if active { "Click to show all shapes" } else { "Click again (or right-click) for more" };
    let r = tool_tip(r, shown, extra);
    if r.secondary_clicked() {
        app.ui.tool_flyout = if open { None } else { Some(group) };
    } else if r.clicked() && !was_held {
        if active {
            // Already on this group: the click toggles the flyout.
            app.ui.tool_flyout = if open { None } else { Some(group) };
        } else {
            app.set_tool(shown);
        }
    }
}

/// Floating flyout beside the Tools column: a grip (drag to move it) and a
/// two-column grid of the group's tools. Stays open until closed.
pub fn flyout(app: &mut App, ctx: &egui::Context) {
    let Some(group) = app.ui.tool_flyout else { return };
    let anchor = app.ui.flyout_anchor.iter().find(|(g, _)| *g == group).map(|(_, r)| *r);
    let default = anchor.map(|r| pos2(r.right() + 10.0, r.top() - 34.0)).unwrap_or(pos2(50.0, 120.0));
    let screen = ctx.content_rect();
    let pos = app.ui.flyout_pos.unwrap_or(default);
    let frame = egui::Frame::new()
        .fill(theme::BG_PANEL)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(8)
        .inner_margin(egui::Margin { left: 6, right: 6, top: 4, bottom: 6 })
        .shadow(egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(110) });
    let resp = egui::Area::new(egui::Id::new("tool_flyout")).order(egui::Order::Foreground).fixed_pos(pos).constrain_to(screen).show(ctx, |ui| {
        frame.show(ui, |ui| {
            let w = 30.0 * 2.0 + 3.0;
            ui.set_width(w);
            // Header: grip (drag to move), close on the right.
            let (hr, grip) = ui.allocate_exact_size(vec2(w, 18.0), Sense::drag());
            if grip.dragged() {
                app.ui.flyout_pos = Some(pos + grip.drag_delta());
            }
            if grip.hovered() || grip.dragged() {
                ui.ctx().set_cursor_icon(if grip.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
            }
            if grip.double_clicked() {
                app.ui.flyout_pos = None;
            }
            ui.painter().text(hr.center(), egui::Align2::CENTER_CENTER, icon::DOTS_SIX, egui::FontId::proportional(14.0), theme::TEXT_FAINT);
            let close = Rect::from_center_size(pos2(hr.right() - 8.0, hr.center().y), vec2(16.0, 16.0));
            let cr = ui.interact(close, ui.id().with("flyout_close"), Sense::click());
            ui.painter().text(close.center(), egui::Align2::CENTER_CENTER, icon::X, egui::FontId::proportional(11.0), if cr.hovered() { Color32::WHITE } else { theme::TEXT_FAINT });
            if widgets::tip(cr, &format!("Close {}", group.title().to_lowercase()), None).clicked() {
                app.ui.tool_flyout = None;
            }
            ui.add_space(2.0);
            ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
            for row in group.tools().chunks(2) {
                ui.horizontal(|ui| {
                    for t in row.iter().copied() {
                        let r = tool_glyph_button(ui, t, app.tool == t);
                        if tool_tip(r, t, "").clicked() {
                            app.set_tool(t);
                        }
                    }
                });
            }
        });
    });
    let _ = resp;
}

/// Tool button showing the tool's icon; primitives draw their real outline.
fn tool_glyph_button(ui: &mut Ui, t: Tool, selected: bool) -> egui::Response {
    let Some(kind) = preview_kind(t) else { return widgets::tool_button(ui, t.icon(), selected) };
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let bg = if selected {
            Color32::from_rgb(0x2A, 0x4A, 0x78)
        } else if resp.is_pointer_button_down_on() {
            theme::BG_ACTIVE
        } else if resp.hovered() {
            theme::BG_HOVER
        } else {
            Color32::TRANSPARENT
        };
        p.rect_filled(rect, theme::radius(), bg);
        if selected {
            p.rect_stroke(rect, theme::radius(), Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.6)), StrokeKind::Inside);
        }
        let fg = if selected || resp.hovered() { Color32::WHITE } else { theme::TEXT };
        paint_glyph(p, &crate::geom::local_path(&kind), rect.shrink(7.0), fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::Default)
}

/// Shape drawn in the flyout for a primitive tool (None: use the font icon).
fn preview_kind(t: Tool) -> Option<ShapeKind> {
    match t {
        Tool::Prim(PrimKind::Qr) => None,
        Tool::Prim(k) => {
            let mut prim = k.default_prim();
            // Tweaks so small previews read clearly.
            match k {
                PrimKind::Donut => prim.p[0] = 0.55,
                PrimKind::Cog => prim.p = [8.0, 0.22, 0.5, 0.35],
                PrimKind::Spiral => prim.p[0] = 2.5,
                PrimKind::Cloud => prim.p = [6.0, 1.0, 0.0, 0.0],
                _ => {}
            }
            let w = match k {
                PrimKind::Cloud | PrimKind::Callout | PrimKind::CalloutEllipse => 1.25,
                PrimKind::Tear => 0.8,
                _ => 1.0,
            };
            Some(ShapeKind::Prim { w, h: 1.0, prim })
        }
        Tool::Rect => Some(ShapeKind::Rect { w: 1.25, h: 1.0, radii: [0.0; 4] }),
        Tool::Ellipse => Some(ShapeKind::Ellipse { w: 1.0, h: 1.0 }),
        Tool::Star => Some(ShapeKind::Star { w: 1.0, h: 1.0, points: 5, inner: 0.45, corner: 0.0 }),
        Tool::Polygon => Some(ShapeKind::Polygon { w: 1.0, h: 1.0, sides: 6, corner: 0.0 }),
        _ => None,
    }
}

/// Stroke a unit-box path into `r` (aspect kept), like a line icon.
pub fn paint_glyph(p: &egui::Painter, path: &kurbo::BezPath, r: Rect, color: Color32) {
    let b = path.bounding_box();
    let s = (r.width() as f64 / b.width().max(1e-9)).min(r.height() as f64 / b.height().max(1e-9));
    let off = vec2(r.center().x - ((b.center().x) * s) as f32, r.center().y - ((b.center().y) * s) as f32);
    let stroke = Stroke::new(1.4, color);
    let mut pts: Vec<Pos2> = Vec::new();
    let flush = |pts: &mut Vec<Pos2>, closed: bool| {
        if pts.len() > 1 {
            let v = std::mem::take(pts);
            p.add(if closed { egui::Shape::closed_line(v, stroke) } else { egui::Shape::line(v, stroke) });
        }
        pts.clear();
    };
    kurbo::flatten(path, 0.002, |el| match el {
        kurbo::PathEl::MoveTo(q) => {
            flush(&mut pts, false);
            pts.push(pos2((q.x * s) as f32, (q.y * s) as f32) + off);
        }
        kurbo::PathEl::LineTo(q) => pts.push(pos2((q.x * s) as f32, (q.y * s) as f32) + off),
        kurbo::PathEl::ClosePath => flush(&mut pts, true),
        _ => {}
    });
    flush(&mut pts, false);
}
