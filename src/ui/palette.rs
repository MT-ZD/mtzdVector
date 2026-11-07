//! Ctrl+K quick-actions palette: fuzzy search over every command and tool.

use egui::{Align2, Color32, CornerRadius, Key, Modifiers, RichText, Sense, Stroke, vec2};
use egui_phosphor::regular as icon;

use super::theme::{self, ACCENT, TEXT, TEXT_DIM, TEXT_FAINT};
use crate::app::App;
use crate::commands::{Cmd, alias_match, fuzzy_score};

/// Commands matching `query`, best first (enabled before disabled), with
/// the alias that matched when the name itself didn't.
pub fn matches(app: &App, query: &str) -> Vec<(Cmd, Option<&'static str>)> {
    let mut v: Vec<(Cmd, i32, bool, Option<&'static str>)> = Cmd::all()
        .into_iter()
        .filter(|c| *c != Cmd::Palette)
        .filter_map(|c| {
            let m = c.meta();
            // A name that starts a word beats an alias; a scattered fuzzy
            // match of the name ranks below one.
            let named = fuzzy_score(query, m.label).or_else(|| fuzzy_score(query, &format!("{} {}", m.category.label(), m.label)).map(|s| s - 30));
            let alias = alias_match(query, c.aliases()).filter(|_| query.trim().chars().count() >= 2);
            let alias_score = alias.map(|a| 25 * query.trim().chars().count() as i32 - a.len() as i32 / 4);
            match (named, alias_score) {
                (Some(n), Some(a)) if a > n => Some((c, a, c.enabled(app), alias)),
                (Some(n), _) => Some((c, n, c.enabled(app), None)),
                (None, Some(a)) => Some((c, a, c.enabled(app), alias)),
                (None, None) => None,
            }
        })
        .collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
    v.into_iter().map(|x| (x.0, x.3)).collect()
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(state) = &mut app.ui.palette else { return };
    let just_opened = std::mem::replace(&mut state.just_opened, false);
    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    let query = state.query.clone();
    let list = matches(app, &query);
    let state = app.ui.palette.as_mut().unwrap();
    if list.is_empty() {
        state.selected = 0;
    } else {
        if down {
            state.selected = (state.selected + 1) % list.len();
        }
        if up {
            state.selected = (state.selected + list.len() - 1) % list.len();
        }
        state.selected = state.selected.min(list.len() - 1);
    }
    let mut run: Option<Cmd> = if enter { list.get(state.selected).map(|x| x.0) } else { None };
    let mut close = esc;

    let screen = ctx.content_rect();
    // Dimmed backdrop that swallows clicks (clicking it closes the palette).
    egui::Area::new(egui::Id::new("palette_backdrop")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        let r = ui.allocate_rect(screen, Sense::click());
        ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(90));
        if r.clicked() && !just_opened {
            close = true;
        }
    });
    let width = 560.0f32.min(screen.width() - 40.0);
    egui::Area::new(egui::Id::new("palette"))
        .order(egui::Order::Tooltip)
        .fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + 90.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::BG_PANEL)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(8))
                .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(140) })
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    let state = app.ui.palette.as_mut().unwrap();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::LIGHTNING).size(16.0).color(ACCENT));
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut state.query)
                                .hint_text("Type a command or tool…")
                                .frame(egui::Frame::NONE)
                                .font(egui::FontId::proportional(15.0))
                                .desired_width(f32::INFINITY),
                        );
                        if just_opened || !r.has_focus() {
                            r.request_focus();
                        }
                        if r.changed() {
                            state.selected = 0;
                        }
                    });
                    ui.add_space(4.0);
                    let rr = ui.available_rect_before_wrap();
                    ui.painter().hline(rr.x_range(), rr.top(), Stroke::new(1.0, theme::SEPARATOR));
                    ui.add_space(4.0);
                    let selected = state.selected;
                    if list.is_empty() {
                        ui.add_space(12.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("No matching commands").color(TEXT_DIM)));
                        ui.add_space(12.0);
                        return;
                    }
                    egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (i, (c, alias)) in list.iter().enumerate() {
                            let m = c.meta();
                            let enabled = c.enabled(app);
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                            let sel = i == selected;
                            if sel {
                                ui.painter().rect_filled(rect, CornerRadius::same(5), theme::ACCENT_DIM);
                                if down || up {
                                    ui.scroll_to_rect(rect, None);
                                }
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(5), theme::BG_HOVER);
                            }
                            let fg = if !enabled { TEXT_FAINT } else if sel { Color32::WHITE } else { TEXT };
                            let p = ui.painter();
                            p.text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, m.icon, egui::FontId::proportional(15.0), if sel { Color32::WHITE } else { TEXT_DIM });
                            let name = p.text(rect.left_center() + vec2(38.0, 0.0), Align2::LEFT_CENTER, m.label, egui::FontId::proportional(13.0), fg);
                            // Found by another name: say which.
                            if let Some(a) = alias {
                                p.text(egui::pos2(name.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, format!("“{a}”"), egui::FontId::proportional(11.5), TEXT_FAINT);
                            }
                            let mut x = rect.right() - 10.0;
                            if let Some(s) = c.shortcut_text() {
                                let g = p.layout_no_wrap(s, egui::FontId::proportional(11.0), TEXT_DIM);
                                let kr = egui::Rect::from_min_size(egui::pos2(x - g.size().x - 10.0, rect.center().y - 9.0), vec2(g.size().x + 10.0, 18.0));
                                p.rect_filled(kr, CornerRadius::same(3), theme::BG_INPUT);
                                p.galley(kr.min + vec2(5.0, (18.0 - g.size().y) / 2.0), g, TEXT_DIM);
                                x = kr.left() - 10.0;
                            }
                            p.text(egui::pos2(x, rect.center().y), Align2::RIGHT_CENTER, m.category.label(), egui::FontId::proportional(11.0), TEXT_FAINT);
                            if resp.clicked() && enabled {
                                run = Some(*c);
                            }
                            if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                                app.ui.palette.as_mut().unwrap().selected = i;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}{} navigate · Enter run · Esc close", icon::ARROW_UP, icon::ARROW_DOWN)).size(10.5).color(TEXT_FAINT));
                    });
                });
        });
    if let Some(c) = run {
        app.ui.palette = None;
        if c.enabled(app) {
            app.run_cmd(c, ctx);
        }
        return;
    }
    if close {
        app.ui.palette = None;
    }
}
