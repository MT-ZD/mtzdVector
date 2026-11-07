//! Title bar with the menus: ◆ File Edit Object Text View Help · document
//! name · zoom · window buttons. The OS title bar is off (see `window`).

use egui::{Align2, Color32, CornerRadius, RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use super::theme::{self, TEXT_DIM};
use crate::app::{App, Tool};
use crate::commands::Cmd;

/// A menu entry bound to a command: icon, label, shortcut, enabled state and
/// check mark for toggles. Runs the command when clicked.
pub fn cmd_item(ui: &mut Ui, app: &mut App, cmd: Cmd) {
    cmd_item_labeled(ui, app, cmd, None);
}

pub fn cmd_item_labeled(ui: &mut Ui, app: &mut App, cmd: Cmd, label: Option<&str>) {
    let meta = cmd.meta();
    let enabled = cmd.enabled(app);
    let glyph = match cmd.checked(app) {
        Some(true) if !matches!(cmd, Cmd::Tool(_)) => icon::CHECK_SQUARE,
        Some(false) if !matches!(cmd, Cmd::Tool(_)) => icon::SQUARE,
        _ => meta.icon,
    };
    let text = RichText::new(format!("{glyph}   {}", label.unwrap_or(meta.label))).size(theme::FONT_MD);
    let mut b = egui::Button::new(text).min_size(vec2(0.0, 22.0));
    if let Some(s) = cmd.shortcut_text() {
        b = b.shortcut_text(RichText::new(s).size(theme::FONT_SM).color(TEXT_DIM));
    }
    if ui.add_enabled(enabled, b).clicked() {
        ui.close();
        let ctx = ui.ctx().clone();
        app.menu_anchor_doc = app.ui.ctx_menu.as_ref().map(|m| m.doc);
        app.run_cmd(cmd, &ctx);
    }
}

pub fn cmd_items(ui: &mut Ui, app: &mut App, cmds: &[Cmd]) {
    for c in cmds {
        cmd_item(ui, app, *c);
    }
}

pub fn submenu(ui: &mut Ui, glyph: &str, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.menu_button(RichText::new(format!("{glyph}   {label}")).size(theme::FONT_MD), add);
}

pub const ARRANGE: [Cmd; 4] = [Cmd::BringToFront, Cmd::BringForward, Cmd::SendBackward, Cmd::SendToBack];
pub const ALIGN: [Cmd; 6] = [Cmd::AlignLeft, Cmd::AlignHCenter, Cmd::AlignRight, Cmd::AlignTop, Cmd::AlignVCenter, Cmd::AlignBottom];
pub const BOOLEAN: [Cmd; 4] = [Cmd::Union, Cmd::Subtract, Cmd::Intersect, Cmd::Exclude];
pub const PATH: [Cmd; 9] = [Cmd::NodesSmooth, Cmd::NodesSharp, Cmd::DeleteNodes, Cmd::ToggleClosed, Cmd::BreakAtNode, Cmd::JoinNodes, Cmd::ReversePath, Cmd::SimplifyPath, Cmd::OffsetPath];

pub fn align_menu(ui: &mut Ui, app: &mut App) {
    cmd_items(ui, app, &ALIGN[..3]);
    ui.separator();
    cmd_items(ui, app, &ALIGN[3..]);
    ui.separator();
    cmd_items(ui, app, &[Cmd::DistributeH, Cmd::DistributeV]);
    if app.selection.len() == 1 {
        ui.separator();
        ui.label(RichText::new("Single object aligns to the artboard").size(theme::FONT_SM).color(TEXT_DIM));
    }
}

pub fn boolean_menu(ui: &mut Ui, app: &mut App) {
    cmd_items(ui, app, &BOOLEAN);
    ui.separator();
    cmd_items(ui, app, &[Cmd::Flatten, Cmd::Ungroup]);
    ui.separator();
    cmd_item_labeled(ui, app, Cmd::Tool(Tool::ShapeBuilder), Some("Shape Builder Tool"));
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let frame = egui::Frame::new().fill(theme::BG_HEADER).inner_margin(egui::Margin { left: 10, right: 0, top: 0, bottom: 0 });
    egui::Panel::top("menubar").exact_size(theme::MENUBAR_H).frame(frame).show(ui, |ui| {
        let full = ui.max_rect();
        ui.painter().hline(full.x_range(), full.bottom() - 0.5, Stroke::new(1.0, theme::SEPARATOR));
        // The bar's background moves the window; widgets added after it win.
        super::window::drag_area(ui, full.with_min_x(full.left() - 10.0));
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            crate::logo::show(ui, 18.0);
            ui.add_space(6.0);
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::NONE;
            ui.style_mut().visuals.widgets.hovered.bg_stroke = Stroke::NONE;
            ui.style_mut().visuals.widgets.open.bg_stroke = Stroke::NONE;
            let mut menus_right = full.left();
            egui::MenuBar::new().ui(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.spacing_mut().button_padding = vec2(7.0, 4.0);
                ui.menu_button("File", |ui| {
                    ui.set_min_width(240.0);
                    cmd_items(ui, app, &[Cmd::New, Cmd::StartScreen, Cmd::Open]);
                    let recent = crate::recent::load();
                    submenu(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Open Recent", |ui| {
                        ui.set_min_width(260.0);
                        if recent.is_empty() {
                            ui.label(egui::RichText::new("No recent documents").color(theme::TEXT_DIM));
                        }
                        for p in recent {
                            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            if ui.button(name).on_hover_text(p.display().to_string()).clicked() {
                                app.guarded(crate::session::Guarded::Open(Some(p)));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::Save, Cmd::SaveAs]);
                    ui.separator();
                    cmd_item(ui, app, Cmd::Place);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::Export, Cmd::ExportAgain, Cmd::ExportSlices]);
                    cmd_item(ui, app, Cmd::Print);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::CloseTab, Cmd::Quit]);
                });
                ui.menu_button("Edit", |ui| {
                    ui.set_min_width(240.0);
                    cmd_items(ui, app, &[Cmd::Undo, Cmd::Redo]);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::Cut, Cmd::Copy, Cmd::CopySvg, Cmd::Paste, Cmd::PasteHere]);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::CopyStyle, Cmd::PasteStyle]);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::Duplicate, Cmd::Delete]);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::SelectAll, Cmd::DeselectAll]);
                    ui.separator();
                    cmd_item(ui, app, Cmd::Preferences);
                });
                ui.menu_button("Object", |ui| {
                    ui.set_min_width(240.0);
                    cmd_items(ui, app, &[Cmd::Group, Cmd::Ungroup]);
                    cmd_items(ui, app, &[Cmd::AddAutoLayout, Cmd::RemoveAutoLayout]);
                    cmd_items(ui, app, &[Cmd::CreateComponent, Cmd::DetachInstance]);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::ConvertToCurves, Cmd::ExpandStroke, Cmd::ExpandOutlines]);
                    cmd_items(ui, app, &[Cmd::MakeClip, Cmd::ReleaseClip]);
                    cmd_items(ui, app, &[Cmd::MakeMask, Cmd::MakeAlphaMask, Cmd::ReleaseMask]);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::BlendShapes, Cmd::EnvelopeWarp, Cmd::ExpandLive, Cmd::ReleaseLive, Cmd::TraceImage]);
                    ui.separator();
                    submenu(ui, icon::UNITE, "Boolean", |ui| boolean_menu(ui, app));
                    submenu(ui, icon::STACK, "Arrange", |ui| cmd_items(ui, app, &ARRANGE));
                    submenu(ui, icon::ALIGN_LEFT, "Align", |ui| align_menu(ui, app));
                    submenu(ui, icon::BEZIER_CURVE, "Path", |ui| cmd_items(ui, app, &PATH));
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::FlipH, Cmd::FlipV]);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::ToggleHidden, Cmd::ToggleLocked, Cmd::Rename]);
                    ui.separator();
                    submenu(ui, icon::FRAME_CORNERS, "Artboards", |ui| {
                        ui.set_min_width(220.0);
                        cmd_items(ui, app, &[Cmd::NewArtboard, Cmd::DuplicateArtboard, Cmd::DeleteArtboard]);
                        ui.separator();
                        cmd_items(ui, app, &[Cmd::NextArtboard, Cmd::PrevArtboard, Cmd::ZoomArtboard]);
                        ui.separator();
                        cmd_item_labeled(ui, app, Cmd::Tool(Tool::Artboard), Some("Artboard Tool"));
                        cmd_item(ui, app, Cmd::ArtboardSettings);
                    });
                });
                ui.menu_button("Text", |ui| {
                    ui.set_min_width(240.0);
                    cmd_item(ui, app, Cmd::ShowTypography);
                    cmd_item(ui, app, Cmd::ConvertTextType);
                    cmd_items(ui, app, &[Cmd::TextOnPath, Cmd::ReleaseTextPath, Cmd::LinkTextFrames, Cmd::UnlinkTextFrames]);
                    cmd_item_labeled(ui, app, Cmd::Tool(Tool::Text), Some("Artistic Text Tool"));
                    cmd_item_labeled(ui, app, Cmd::Tool(Tool::FrameText), Some("Frame Text Tool"));
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::TextAlignLeft, Cmd::TextAlignCenter, Cmd::TextAlignRight]);
                    ui.separator();
                    cmd_item_labeled(ui, app, Cmd::ConvertToCurves, Some("Convert Text to Curves"));
                });
                ui.menu_button("View", |ui| {
                    ui.set_min_width(240.0);
                    cmd_items(ui, app, &[Cmd::ZoomIn, Cmd::ZoomOut, Cmd::ZoomFit, Cmd::Zoom100, Cmd::ZoomSelection]);
                    ui.separator();
                    cmd_item(ui, app, Cmd::ToggleOutline);
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::ToggleRulers, Cmd::ToggleGuides, Cmd::ToggleGrid, Cmd::ToggleSnap, Cmd::ClearGuides]);
                    ui.separator();
                    cmd_item(ui, app, Cmd::ToggleStudio);
                });
                ui.menu_button("Help", |ui| {
                    ui.set_min_width(240.0);
                    cmd_items(ui, app, &[Cmd::Palette, Cmd::Shortcuts]);
                    ui.separator();
                    cmd_item(ui, app, Cmd::About);
                });
                menus_right = ui.cursor().min.x;
            });

            // Document tabs between the menus and the zoom menu.
            let tabs_rect = egui::Rect::from_min_max(pos2(menus_right + 16.0, full.top()), pos2(full.right() - RIGHT_RESERVED, full.bottom()));
            if tabs_rect.width() > TAB_MIN_W {
                let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(tabs_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
                tui.set_clip_rect(tabs_rect.intersect(ui.clip_rect()));
                doc_tabs(&mut tui, app);
            }
            // Right: window buttons, then zoom.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                super::window::buttons(ui, app);
                ui.add_space(8.0);
                let z = format!("{:.0}%  {}", app.view.zoom * 100.0, icon::CARET_DOWN);
                ui.menu_button(RichText::new(z).size(theme::FONT_MD).color(TEXT_DIM), |ui| {
                    ui.set_min_width(200.0);
                    for (label, z) in [("25%", 0.25), ("50%", 0.5), ("100%", 1.0), ("200%", 2.0), ("400%", 4.0), ("800%", 8.0)] {
                        if ui.button(label).clicked() {
                            let c = app.canvas_rect.center();
                            app.zoom_at(z / app.view.zoom, c);
                            ui.close();
                        }
                    }
                    ui.separator();
                    cmd_items(ui, app, &[Cmd::ZoomFit, Cmd::ZoomSelection, Cmd::ZoomIn, Cmd::ZoomOut]);
                });
            });
        });
    });
}

/// Space kept free on the right of the tabs (zoom menu and window buttons).
const RIGHT_RESERVED: f32 = 3.0 * 42.0 + 96.0;
const TAB_MAX_W: f32 = 190.0;
const TAB_MIN_W: f32 = 72.0;

/// Open documents as tabs in the title bar: click to switch, middle-click
/// or × to close, drag to reorder, + for a new document.
fn doc_tabs(ui: &mut Ui, app: &mut App) {
    let infos = app.tab_infos();
    let n = infos.len();
    let room = (ui.available_width() - 34.0).max(0.0);
    let tab_w = (room / n as f32).clamp(TAB_MIN_W, TAB_MAX_W);
    let h = theme::MENUBAR_H - 8.0;
    let drag_id = egui::Id::new("doc_tab_drag");
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    let mut rects = Vec::with_capacity(n);
    let mut close = None;
    let mut activate = None;
    ui.spacing_mut().item_spacing.x = 2.0;
    for (i, t) in infos.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(tab_w, h), Sense::click_and_drag());
        rects.push(rect);
        let resp = resp.on_hover_text(t.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Not saved yet".into()));
        let hovered = resp.hovered() || dragging == Some(i);
        let p = ui.painter();
        let bg = if t.active {
            theme::BG_PANEL
        } else if hovered {
            theme::BG_HOVER.gamma_multiply(0.6)
        } else {
            Color32::TRANSPARENT
        };
        p.rect_filled(rect, CornerRadius::same(5), bg);
        if t.active {
            p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
        }
        // Close button / unsaved dot on the right.
        let cr = egui::Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(16.0, 16.0));
        let over_close = ui.rect_contains_pointer(cr) && hovered;
        let text_color = if t.active { theme::TEXT } else { TEXT_DIM };
        let font = egui::FontId::proportional(theme::FONT_MD);
        let text_rect = egui::Rect::from_min_max(pos2(rect.left() + 10.0, rect.top()), pos2(cr.left() - 4.0, rect.bottom()));
        let galley = p.layout(t.name.clone(), font, text_color, f32::INFINITY);
        let clip = p.with_clip_rect(text_rect.intersect(p.clip_rect()));
        clip.galley(pos2(text_rect.left(), rect.center().y - galley.size().y / 2.0), galley.clone(), text_color);
        if galley.size().x > text_rect.width() {
            // Fade the cut-off end of a long name.
            let fade = egui::Rect::from_min_max(pos2(text_rect.right() - 18.0, rect.top() + 1.0), pos2(text_rect.right(), rect.bottom() - 1.0));
            let mut mesh = egui::Mesh::default();
            let c0 = Color32::TRANSPARENT;
            let c1 = if bg == Color32::TRANSPARENT { theme::BG_HEADER } else { bg };
            let base = mesh.vertices.len() as u32;
            for (pt, c) in [(fade.left_top(), c0), (fade.right_top(), c1), (fade.right_bottom(), c1), (fade.left_bottom(), c0)] {
                mesh.vertices.push(egui::epaint::Vertex { pos: pt, uv: egui::epaint::WHITE_UV, color: c });
            }
            mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            p.add(mesh);
        }
        if hovered || t.active {
            if over_close {
                p.rect_filled(cr, CornerRadius::same(3), theme::BG_ACTIVE);
            }
            if t.dirty && !hovered {
                p.circle_filled(cr.center(), 3.5, TEXT_DIM);
            } else {
                p.text(cr.center(), Align2::CENTER_CENTER, icon::X, egui::FontId::proportional(11.0), if over_close { theme::TEXT } else { TEXT_DIM });
            }
        } else if t.dirty {
            p.circle_filled(cr.center(), 3.5, TEXT_DIM);
        }
        if resp.clicked() {
            if over_close {
                close = Some(i);
            } else {
                activate = Some(i);
            }
        }
        if resp.middle_clicked() {
            close = Some(i);
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            dragging = Some(i);
            activate = Some(i);
        }
    }
    // Reorder while dragging: swap with a neighbour once the pointer passes
    // its middle.
    if let Some(from) = dragging {
        if ui.input(|i| i.pointer.primary_down()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(x) = ui.ctx().pointer_interact_pos().map(|p| p.x) {
                let to = rects.iter().position(|r| x < r.center().x).unwrap_or(n.saturating_sub(1));
                let to = if to > from { to - 1 } else { to };
                let to = to.min(n - 1);
                if to != from && (x < rects[from].left() || x > rects[from].right()) {
                    app.move_tab(from, to);
                    dragging = Some(to);
                }
            }
        } else {
            dragging = None;
        }
    }
    ui.data_mut(|d| match dragging {
        Some(i) => {
            d.insert_temp(drag_id, i);
        }
        None => d.remove::<usize>(drag_id),
    });
    // New document.
    let (r, resp) = ui.allocate_exact_size(vec2(h, h), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(5), theme::BG_HOVER);
    }
    ui.painter().text(r.center(), Align2::CENTER_CENTER, icon::PLUS, egui::FontId::proportional(13.0), if resp.hovered() { theme::TEXT } else { TEXT_DIM });
    if super::widgets::tip(resp, "New document", Some("Ctrl+N")).clicked() {
        app.guarded(crate::session::Guarded::New);
    }
    if let Some(i) = activate {
        app.switch_tab(i);
    }
    if let Some(i) = close {
        app.request_close_tab(i);
    }
}
