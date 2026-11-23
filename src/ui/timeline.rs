//! Timeline panel (bottom, turned on in Preferences): transport, a ruler
//! with the playhead, and a row of keys per animated property.

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use super::theme::{self, TEXT_DIM, TEXT_FAINT};
use super::widgets;
use crate::animation::{Ease, KeyRef, Prop, Timeline};
use crate::app::App;

const NAMES_W: f32 = 210.0;
/// Width of the easing column.
const EASE_W: f32 = 300.0;
const ROW_H: f32 = 22.0;
const RULER_H: f32 = 22.0;
/// Room at the ends of the track area so the first and last keys show whole.
const PAD: f32 = 10.0;

/// Box selection in progress: where it started, and the keys kept (Shift).
#[derive(Clone)]
struct Marquee {
    start: Pos2,
    keep: Vec<KeyRef>,
}

/// Drag of selected keys in progress: pointer x where it started.
#[derive(Clone, Copy)]
struct KeyDrag {
    from_x: f32,
}

pub fn show(app: &mut App, ui: &mut Ui) {
    if !app.settings.show_timeline {
        app.anim.hovered = false;
        return;
    }
    let frame = egui::Frame::new().fill(theme::BG_PANEL);
    egui::Panel::bottom("timeline").default_size(190.0).size_range(110.0..=520.0).resizable(true).frame(frame).show(ui, |ui| {
        let full = ui.max_rect();
        ui.expand_to_include_rect(full);
        app.anim.hovered = ui.rect_contains_pointer(full);
        ui.painter().hline(full.x_range(), full.top() + 0.5, Stroke::new(1.0, theme::SEPARATOR));
        // Easing sits docked on the right, always there.
        let split = full.max.x - EASE_W;
        transport(app, ui, Rect::from_min_max(full.min, pos2(split, full.min.y + 32.0)));
        body(app, ui, Rect::from_min_max(pos2(full.min.x, full.min.y + 32.0), pos2(split, full.max.y)));
        ease_panel(app, ui, Rect::from_min_max(pos2(split, full.min.y + 1.0), full.max));
    });
}

fn transport(app: &mut App, ui: &mut Ui, rect: Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 6.0;
    let tl = app.doc.animation.clone().unwrap_or_default();
    if widgets::tip(widgets::mini_icon(ui, icon::SKIP_BACK, TEXT_DIM), "Go to start", None).clicked() {
        app.anim.playing = false;
        app.anim_seek(0.0);
    }
    let (glyph, tip) = if app.anim.playing { (icon::PAUSE, "Pause") } else { (icon::PLAY, "Play") };
    if widgets::tip(widgets::mini_icon(ui, glyph, theme::TEXT), tip, Some("Space")).clicked() {
        app.anim_toggle_play();
    }
    let looping = tl.looping;
    let r = widgets::mini_icon(ui, if looping { icon::REPEAT } else { icon::REPEAT_ONCE }, if looping { theme::ACCENT } else { TEXT_DIM });
    if widgets::tip(r, if looping { "Looping: playback starts over at the end" } else { "Plays once and stops at the end" }, None).clicked() {
        app.checkpoint("loop");
        app.doc.animation.get_or_insert_with(Default::default).looping = !looping;
    }
    let onion = app.anim.onion;
    let r = widgets::mini_icon(ui, icon::GHOST, if onion.on { theme::ACCENT } else { TEXT_DIM });
    let r = widgets::tip_long(r, "Onion skin", None, "Outlines of animated layers a few frames before (red) and after (green) the playhead. Right-click for how many and how far apart.");
    if r.clicked() {
        app.anim.onion.on = !onion.on;
    }
    r.context_menu(|ui| {
        let o = &mut app.anim.onion;
        widgets::prop_row(ui, "Ghosts", |ui| {
            widgets::segmented(ui, &mut o.count, &[(1, "1"), (2, "2"), (3, "3"), (5, "5")]);
        });
        widgets::prop_row(ui, "Every", |ui| {
            widgets::segmented(ui, &mut o.step, &[(1, "1"), (2, "2"), (3, "3"), (5, "5"), (10, "10")]);
            ui.label(RichText::new("frames").size(theme::FONT_SM).color(TEXT_DIM));
        });
        o.on = true;
    });
    let rec = if app.anim.record { theme::DANGER } else { TEXT_DIM };
    let r = widgets::mini_icon(ui, if app.anim.record { icon::RECORD } else { icon::CIRCLE }, rec);
    if widgets::tip_long(r, "Record", None, "Every change to a layer becomes a key at the playhead. With it off, only properties that already have keys get new ones.").clicked() {
        app.anim.record = !app.anim.record;
        if app.doc.animation.is_none() {
            app.doc.animation = Some(Timeline::default());
        }
    }
    let frame_no = (tl.time * tl.fps).round() as i64;
    ui.label(RichText::new(format!("{:.2} s", tl.time)).size(theme::FONT_MD).color(theme::TEXT).monospace());
    ui.label(RichText::new(format!("frame {frame_no}")).size(theme::FONT_SM).color(TEXT_FAINT));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let mut fps = tl.fps;
        let mut duration = tl.duration;
        let fps_changed = widgets::segmented(ui, &mut fps, &[(60.0, "60"), (30.0, "30"), (25.0, "25"), (24.0, "24"), (12.0, "12")]);
        ui.label(RichText::new("fps").size(theme::FONT_SM).color(TEXT_DIM));
        let d_changed = widgets::tip(widgets::num_field(ui, "", &mut duration, widgets::Num::default().range(0.1, 600.0).speed(0.05).suffix(" s"), 80.0), "Length of the animation", None).changed();
        ui.label(RichText::new("Length").size(theme::FONT_SM).color(TEXT_DIM));
        if (fps_changed || d_changed) && (fps != tl.fps || duration != tl.duration) {
            app.checkpoint("timeline");
            let a = app.doc.animation.get_or_insert_with(Default::default);
            a.fps = fps;
            a.duration = duration;
            if a.time > duration {
                app.anim_seek(duration);
            }
        }
    });
}

fn first_ease(app: &App) -> Option<Ease> {
    let (id, p, i) = *app.anim.selected.first()?;
    Some(app.doc.get(id)?.anim.as_deref()?.track(p)?.keys.get(i)?.ease)
}

/// Layers shown: animated ones and the selection, in stacking order (top first).
fn rows(app: &App) -> Vec<u64> {
    let mut ids: Vec<u64> = app.doc.all_ids().into_iter().filter(|id| app.selection.contains(id) || app.doc.get(*id).is_some_and(|s| s.anim.is_some())).collect();
    ids.reverse();
    ids
}

fn body(app: &mut App, ui: &mut Ui, rect: Rect) {
    let tl = app.doc.animation.clone().unwrap_or_default();
    let track_x = rect.min.x + NAMES_W;
    let ruler = Rect::from_min_max(pos2(track_x, rect.min.y), pos2(rect.max.x, rect.min.y + RULER_H));
    // More room on the right: the end keys clear the rows' scroll bar.
    let span = (ruler.width() - PAD - 24.0).max(1.0);
    let x_of = |t: f64| ruler.min.x + PAD + (t / tl.duration.max(1e-6)) as f32 * span;
    let t_of = |x: f32| tl.snap(((x - ruler.min.x - PAD) / span) as f64 * tl.duration);
    let painter = ui.painter().clone();
    painter.rect_filled(Rect::from_min_max(rect.min, pos2(rect.max.x, ruler.max.y)), 0, theme::BG_HEADER);
    painter.vline(track_x - 0.5, rect.y_range(), Stroke::new(1.0, theme::SEPARATOR));
    painter.text(pos2(rect.min.x + 10.0, ruler.center().y), egui::Align2::LEFT_CENTER, "Layers", egui::FontId::proportional(theme::FONT_SM), TEXT_DIM);

    // Ruler: a tick per frame when there's room, labels every second.
    let px_per_s = span as f64 / tl.duration.max(1e-6);
    let label_step = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| s * px_per_s >= 60.0).unwrap_or(120.0);
    let tick_step = if px_per_s / tl.fps >= 6.0 { 1.0 / tl.fps } else { label_step / 5.0 };
    let mut t = 0.0;
    while t <= tl.duration + 1e-9 {
        let x = x_of(t);
        let major = ((t / label_step).round() * label_step - t).abs() < 1e-6;
        painter.vline(x, (ruler.max.y - if major { 8.0 } else { 4.0 })..=ruler.max.y, Stroke::new(1.0, if major { TEXT_DIM } else { TEXT_FAINT }));
        if major {
            let (lx, align) = if x + 28.0 > ruler.max.x { (x - 3.0, egui::Align2::RIGHT_TOP) } else { (x + 3.0, egui::Align2::LEFT_TOP) };
            painter.text(pos2(lx, ruler.min.y + 2.0), align, format!("{}s", trim(t)), egui::FontId::proportional(10.0), TEXT_DIM);
        }
        t += tick_step;
    }
    let scrub = ui.interact(ruler, ui.id().with("tl_ruler"), Sense::click_and_drag());
    if let Some(p) = scrub.interact_pointer_pos().filter(|_| scrub.clicked() || scrub.dragged()) {
        app.anim.playing = false;
        app.anim_seek(t_of(p.x));
    }
    scrub.context_menu(|ui| {
        if ui.button(format!("{}  Set in point at playhead   I", icon::BRACKETS_SQUARE)).clicked() {
            app.anim_set_point(true);
            ui.close();
        }
        if ui.button(format!("{}  Set out point at playhead   O", icon::BRACKETS_SQUARE)).clicked() {
            app.anim_set_point(false);
            ui.close();
        }
        if tl.range.is_some() && ui.button(format!("{}  Clear in and out points", icon::X)).clicked() {
            app.anim_clear_points();
            ui.close();
        }
    });
    // In and out handles: drag them along the ruler.
    let (a, b) = tl.span();
    for (start, t) in [(true, a), (false, b)] {
        let x = x_of(t);
        let hit = Rect::from_center_size(pos2(x, ruler.center().y), vec2(10.0, RULER_H));
        let h = ui.interact(hit, ui.id().with(("tl_point", start)), Sense::click_and_drag());
        // A click still moves the playhead; dragging moves the point.
        if let Some(p) = h.interact_pointer_pos().filter(|_| h.clicked()) {
            app.anim.playing = false;
            app.anim_seek(t_of(p.x));
        }
        if h.drag_started() {
            app.checkpoint("in/out point");
        }
        if let Some(p) = h.interact_pointer_pos().filter(|_| h.dragged()) {
            let t = t_of(p.x);
            let step = 1.0 / tl.fps;
            let (na, nb) = if start { (t.min(b - step).max(0.0), b) } else { (a, t.max(a + step).min(tl.duration)) };
            if let Some(tl) = &mut app.doc.animation {
                tl.range = Some((na, nb));
            }
        }
        let set = tl.range.is_some();
        let col = if h.hovered() || h.dragged() { theme::TEXT } else if set { theme::ACCENT } else { TEXT_FAINT };
        let dir = if start { 1.0 } else { -1.0 };
        let (y0, y1) = (ruler.min.y + 3.0, ruler.max.y - 1.0);
        painter.line_segment([pos2(x, y0), pos2(x, y1)], Stroke::new(2.0, col));
        painter.line_segment([pos2(x, y0), pos2(x + 5.0 * dir, y0)], Stroke::new(2.0, col));
        painter.line_segment([pos2(x, y1), pos2(x + 5.0 * dir, y1)], Stroke::new(2.0, col));
        widgets::tip(h.on_hover_cursor(egui::CursorIcon::ResizeHorizontal), if start { "In point: where playback and export start (I)" } else { "Out point: where playback and export end (O)" }, None);
    }

    // Rows.
    let area = Rect::from_min_max(pos2(rect.min.x, ruler.max.y), rect.max);
    let ids = rows(app);
    let mut content = ui.new_child(egui::UiBuilder::new().max_rect(area));
    egui::ScrollArea::vertical().id_salt("timeline_rows").auto_shrink([false, false]).show(&mut content, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if ids.is_empty() {
            // In the names column, wrapped: the track and playhead stay clear.
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.set_max_width(NAMES_W - 20.0);
                    let hint = format!("Select a layer, then press K over a property or click {} beside it. Or turn on {} Record and change things at different times.", icon::DIAMOND, icon::RECORD);
                    ui.add(egui::Label::new(RichText::new(hint).size(theme::FONT_SM).color(TEXT_DIM)).wrap());
                });
            });
            return;
        }
        let key_drag: Option<KeyDrag> = ui.data(|d| d.get_temp(egui::Id::new("tl_key_drag")));
        let offset = key_drag.zip(ui.ctx().pointer_latest_pos()).map(|(d, p)| (p.x - d.from_x) as f64 / px_per_s).unwrap_or(0.0);
        // Behind the keys: a click moves the playhead, a drag box-selects.
        let clip = ui.clip_rect();
        let bg = ui.interact(Rect::from_min_max(pos2(track_x, clip.min.y), clip.max), ui.id().with("tl_bg"), Sense::click_and_drag());
        let marquee_id = egui::Id::new("tl_marquee");
        if bg.drag_started() {
            // From where the button went down, not where the drag was noticed.
            if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(bg.interact_pointer_pos()) {
                let keep = if ui.input(|i| i.modifiers.shift) { app.anim.selected.clone() } else { Vec::new() };
                ui.data_mut(|d| d.insert_temp(marquee_id, Marquee { start: p, keep }));
            }
        }
        // Where every key is drawn: for the box.
        let mut spots: Vec<(Pos2, Vec<KeyRef>)> = Vec::new();
        for id in ids {
            let Some(s) = app.doc.get(id).cloned() else { continue };
            let selected = app.selection.contains(&id);
            let (row, _) = ui.allocate_exact_size(vec2(area.width(), ROW_H), Sense::hover());
            let resp = ui.interact(Rect::from_min_max(row.min, pos2(track_x, row.max.y)), ui.id().with(("tl_layer", id)), Sense::click());
            if selected {
                ui.painter().rect_filled(row, 0, theme::ACCENT.gamma_multiply(0.12));
            }
            let folded = app.anim.collapsed.contains(&id);
            let caret = Rect::from_center_size(pos2(row.min.x + 14.0, row.center().y), vec2(16.0, 16.0));
            let cr = ui.interact(caret, ui.id().with(("tl_fold", id)), Sense::click());
            let col = if cr.hovered() { theme::TEXT } else { TEXT_DIM };
            ui.painter().text(caret.center(), egui::Align2::CENTER_CENTER, if folded { icon::CARET_RIGHT } else { icon::CARET_DOWN }, egui::FontId::proportional(12.0), col);
            if widgets::tip(cr, if folded { "Show properties" } else { "Hide properties" }, None).clicked() {
                if folded {
                    app.anim.collapsed.remove(&id);
                } else {
                    app.anim.collapsed.insert(id);
                }
            }
            ui.painter().text(pos2(row.min.x + 26.0, row.center().y), egui::Align2::LEFT_CENTER, &s.name, egui::FontId::proportional(theme::FONT_MD), theme::TEXT);
            if resp.clicked() && resp.interact_pointer_pos().is_some_and(|p| p.x < track_x && p.x > caret.max.x) {
                app.selection = vec![id];
            }
            // Summary: every key time of the layer.
            if let Some(a) = s.anim.as_deref() {
                let mut times: Vec<f64> = a.tracks.iter().flat_map(|t| t.keys.iter().map(|k| k.t)).collect();
                times.sort_by(f64::total_cmp);
                times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
                for t in times {
                    let at = pos2(x_of(t), row.center().y);
                    let all: Vec<KeyRef> = a.tracks.iter().filter_map(|tr| tr.key_at(t).map(|i| (id, tr.prop, i))).collect();
                    let on = !all.is_empty() && all.iter().all(|k| app.anim.selected.contains(k));
                    spots.push((at, all));
                    diamond(ui.painter(), at, 3.5, if on { theme::ACCENT } else { TEXT_DIM }, false);
                }
            }
            if folded {
                continue;
            }
            let props: Vec<Prop> = Prop::ALL.into_iter().filter(|p| s.anim.as_deref().is_some_and(|a| a.track(*p).is_some()) || (selected && p.primary() && p.applies(&s))).collect();
            for p in props {
                let (row, _) = ui.allocate_exact_size(vec2(area.width(), ROW_H), Sense::hover());
                app.anim_hover(ui, row, Some(id), &[p]);
                // Right-click the property name: key or paste here.
                let name = Rect::from_min_max(pos2(row.min.x + 32.0, row.min.y), pos2(track_x, row.max.y));
                ui.interact(name, ui.id().with(("tl_prop", id, p as u8)), Sense::click()).context_menu(|ui| {
                    if ui.button(format!("{}  Key at playhead   K", icon::DIAMOND)).clicked() {
                        app.anim_toggle_key(id, p);
                        ui.close();
                    }
                    if !app.anim.clipboard.is_empty() && ui.button(format!("{}  Paste keys at playhead   Ctrl+V", icon::CLIPBOARD_TEXT)).clicked() {
                        app.selection = vec![id];
                        app.anim_paste();
                        ui.close();
                    }
                });
                ui.painter().hline(row.x_range(), row.min.y, Stroke::new(1.0, theme::BG_HEADER));
                let track = s.anim.as_deref().and_then(|a| a.track(p));
                let on_key = track.and_then(|t| t.key_at(tl.time)).is_some();
                let toggle = Rect::from_center_size(pos2(row.min.x + 28.0, row.center().y), vec2(16.0, 16.0));
                let tr = ui.interact(toggle, ui.id().with(("tl_toggle", id, p as u8)), Sense::click());
                let col = if on_key { theme::ACCENT } else if tr.hovered() { theme::TEXT } else { TEXT_DIM };
                diamond(ui.painter(), toggle.center(), 5.0, col, !on_key);
                if widgets::tip(tr, if on_key { "Keyed here. To remove a key, select it and press Delete, or right-click it" } else { "Add a key at the playhead" }, None).clicked() {
                    app.anim_toggle_key(id, p);
                }
                ui.painter().text(pos2(row.min.x + 40.0, row.center().y), egui::Align2::LEFT_CENTER, p.label(), egui::FontId::proportional(theme::FONT_SM), TEXT_DIM);
                let Some(track) = track else { continue };
                // Lines between keys show the interpolated spans.
                for w in track.keys.windows(2) {
                    let hold = w[0].ease == Ease::Hold;
                    ui.painter().hline(x_of(w[0].t)..=x_of(w[1].t), row.center().y, Stroke::new(1.0, if hold { TEXT_FAINT } else { theme::ACCENT_DIM }));
                }
                for (i, k) in track.keys.iter().enumerate() {
                    let key = (id, p, i);
                    let picked = app.anim.selected.contains(&key);
                    let x = x_of(if picked { tl.snap(k.t + offset) } else { k.t });
                    let r = Rect::from_center_size(pos2(x, row.center().y), vec2(12.0, 14.0));
                    spots.push((r.center(), vec![key]));
                    let kr = ui.interact(r, ui.id().with(("tl_key", id, p as u8, i)), Sense::click_and_drag());
                    diamond(ui.painter(), r.center(), 5.0, if picked { theme::ACCENT } else if kr.hovered() { Color32::WHITE } else { theme::TEXT }, false);
                    let shift = ui.input(|i| i.modifiers.shift);
                    if kr.clicked() || kr.drag_started() {
                        if shift {
                            if let Some(j) = app.anim.selected.iter().position(|s| *s == key) {
                                if kr.clicked() {
                                    app.anim.selected.remove(j);
                                }
                            } else {
                                app.anim.selected.push(key);
                            }
                        } else if !picked || kr.clicked() {
                            app.anim.selected = vec![key];
                        }
                        if kr.clicked() {
                            app.anim.playing = false;
                            app.anim_seek(k.t);
                        }
                    }
                    if kr.drag_started() {
                        if let Some(p) = kr.interact_pointer_pos() {
                            ui.data_mut(|d| d.insert_temp(egui::Id::new("tl_key_drag"), KeyDrag { from_x: p.x }));
                        }
                    }
                    if kr.drag_stopped() {
                        ui.data_mut(|d| d.remove::<KeyDrag>(egui::Id::new("tl_key_drag")));
                        if offset.abs() > 0.5 / tl.fps {
                            app.anim_move_selected(offset);
                        }
                    }
                    kr.context_menu(|ui| {
                        if !app.anim.selected.contains(&key) {
                            app.anim.selected = vec![key];
                        }
                        if ui.button(format!("{}  Copy   Ctrl+C", icon::COPY)).clicked() {
                            app.anim_copy();
                            ui.close();
                        }
                        if !app.anim.clipboard.is_empty() && ui.button(format!("{}  Paste at playhead   Ctrl+V", icon::CLIPBOARD_TEXT)).clicked() {
                            app.anim_paste();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button(format!("{}  Delete", icon::TRASH)).clicked() {
                            app.anim_delete_selected();
                            ui.close();
                        }
                    });
                }
            }
        }
        if let Some(p) = bg.interact_pointer_pos().filter(|_| bg.clicked()) {
            if !ui.input(|i| i.modifiers.shift) {
                app.anim.selected.clear();
            }
            app.anim.playing = false;
            app.anim_seek(t_of(p.x));
        }
        // Box selection: keys inside the box (Shift adds to what was selected).
        if let Some(m) = ui.data(|d| d.get_temp::<Marquee>(marquee_id)) {
            if let Some(p) = ui.ctx().pointer_latest_pos() {
                let r = Rect::from_two_pos(m.start, p);
                let mut sel = m.keep.clone();
                for (at, keys) in &spots {
                    if r.contains(*at) {
                        for k in keys {
                            if !sel.contains(k) {
                                sel.push(*k);
                            }
                        }
                    }
                }
                app.anim.selected = sel;
                ui.painter().rect(r, 0, theme::ACCENT.gamma_multiply(0.12), Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
            }
            if !bg.dragged() {
                ui.data_mut(|d| d.remove::<Marquee>(marquee_id));
            }
        }
    });

    // Outside the in and out points: dimmed.
    if tl.range.is_some() {
        let shade = Color32::from_black_alpha(110);
        let (a, b) = tl.span();
        painter.rect_filled(Rect::from_min_max(pos2(track_x, ruler.min.y), pos2(x_of(a), rect.max.y)), 0, shade);
        painter.rect_filled(Rect::from_min_max(pos2(x_of(b), ruler.min.y), pos2(rect.max.x, rect.max.y)), 0, shade);
    }
    // Playhead over ruler and rows.
    let x = x_of(tl.time);
    painter.vline(x, ruler.min.y..=rect.max.y, Stroke::new(1.0, theme::DANGER));
    let head = [pos2(x - 5.0, ruler.min.y), pos2(x + 5.0, ruler.min.y), pos2(x, ruler.min.y + 7.0)];
    painter.add(egui::Shape::convex_polygon(head.to_vec(), theme::DANGER, Stroke::NONE));
    // Ends of the animation.
    painter.vline(x_of(tl.duration) + 0.5, ruler.max.y..=rect.max.y, Stroke::new(1.0, theme::SEPARATOR));
}

fn diamond(p: &egui::Painter, c: Pos2, r: f32, color: Color32, hollow: bool) {
    let pts = vec![pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - r, c.y)];
    if hollow {
        p.add(egui::Shape::closed_line(pts, Stroke::new(1.2, color)));
    } else {
        p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
    }
}

fn trim(t: f64) -> String {
    let s = format!("{t:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The easing curve of `e` in `r` (0,0 bottom left, 1,1 top right; room
/// above and below for overshoot when `r` is taller than wide).
fn curve(p: &egui::Painter, r: Rect, e: Ease, color: Color32, width: f32) {
    let to = |u: f64, v: f64| pos2(r.min.x + u as f32 * r.width(), r.max.y - v as f32 * r.height());
    let pts: Vec<Pos2> = match e {
        Ease::Hold => vec![to(0.0, 0.0), to(1.0, 0.0), to(1.0, 1.0)],
        _ => (0..=40).map(|i| i as f64 / 40.0).map(|u| to(u, e.apply(u))).collect(),
    };
    p.add(egui::Shape::line(pts, Stroke::new(width, color)));
}

/// The easing column: a curve editor with two handles, and preset tiles.
/// Works on the selected keys (dimmed with none).
fn ease_panel(app: &mut App, ui: &mut Ui, rect: Rect) {
    ui.painter().rect_filled(rect, 0, theme::BG_HEADER);
    ui.painter().vline(rect.min.x + 0.5, rect.y_range(), Stroke::new(1.0, theme::SEPARATOR));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 6.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    let current = first_ease(app);
    let n = app.anim.selected.len();
    ui.horizontal(|ui| {
        ui.label(RichText::new("EASING").size(10.5).color(TEXT_DIM));
        let what = match (current, n) {
            (_, 0) => "Select keys to ease them".to_string(),
            (Some(e), 1) => e.label().to_string(),
            (Some(e), n) => format!("{} · {n} keys", e.label()),
            (None, _) => String::new(),
        };
        ui.label(RichText::new(what).size(theme::FONT_SM).color(if n == 0 { TEXT_FAINT } else { theme::TEXT }));
    });
    // A key eases the way to the next one: the last key has nothing to ease.
    let last = |(id, p, i): &KeyRef| app.doc.get(*id).and_then(|s| s.anim.as_deref()).and_then(|a| a.track(*p)).is_some_and(|t| *i + 1 >= t.keys.len());
    if n > 0 && app.anim.selected.iter().all(last) {
        ui.label(RichText::new("Last key: easing works from the key before it").size(10.5).color(Color32::from_rgb(0xE0, 0xB0, 0x50)));
    }
    ui.add_space(4.0);
    let avail = ui.available_size();
    ui.add_enabled_ui(n > 0, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
            let side = (avail.y - 4.0).clamp(60.0, 200.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                curve_editor(app, ui, vec2(side.min(avail.x - 140.0), side - 16.0), current);
                let h = current.map_or(Ease::default().handles(), |e| e.handles());
                ui.label(RichText::new(format!("{:.2}, {:.2}, {:.2}, {:.2}", h[0], h[1], h[2], h[3])).size(10.0).color(TEXT_DIM).monospace());
            });
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                let tile = vec2(40.0, ((side - 8.0) / 3.0).clamp(18.0, 40.0));
                for row in Ease::PRESETS.chunks(3) {
                    ui.horizontal(|ui| {
                        for (e, label) in row {
                            let (r, resp) = ui.allocate_exact_size(tile, Sense::click());
                            let on = n > 0 && current == Some(*e);
                            let fill = if on { theme::ACCENT.gamma_multiply(0.22) } else if resp.hovered() && n > 0 { theme::BG_HOVER } else { theme::BG_INPUT };
                            ui.painter().rect_filled(r, 4, fill);
                            if on {
                                ui.painter().rect_stroke(r, 4, Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
                            }
                            let pad = vec2(10.0, (tile.y * 0.25).min(9.0));
                            let col = if on { theme::ACCENT } else if n > 0 { theme::TEXT } else { TEXT_FAINT };
                            curve(ui.painter(), r.shrink2(pad), *e, col, 1.3);
                            if widgets::tip(resp, label, None).clicked() {
                                app.anim_set_ease(*e);
                            }
                        }
                    });
                }
            });
        });
    });
}

/// The curve of `current` with its two Bézier handles to drag.
fn curve_editor(app: &mut App, ui: &mut Ui, size: egui::Vec2, current: Option<Ease>) {
    let (outer, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(outer, 5, theme::BG_INPUT);
    // The unit square in the middle half; the rest is room for overshoot.
    let m = outer.height() * 0.25;
    let sq = Rect::from_min_max(pos2(outer.min.x + 12.0, outer.min.y + m), pos2(outer.max.x - 12.0, outer.max.y - m));
    ui.painter().rect_stroke(sq, 0, Stroke::new(1.0, theme::SEPARATOR), egui::StrokeKind::Inside);
    let Some(e) = current else {
        curve(ui.painter(), sq, Ease::default(), TEXT_FAINT, 1.5);
        return;
    };
    let reach = (m / sq.height()) as f64 - 0.05;
    let to = |u: f64, v: f64| pos2(sq.min.x + u as f32 * sq.width(), sq.max.y - v as f32 * sq.height());
    let from = |p: Pos2| (((p.x - sq.min.x) / sq.width()) as f64, ((sq.max.y - p.y) / sq.height()) as f64);
    curve(ui.painter(), sq, e, theme::ACCENT, 2.0);
    let mut h = e.handles();
    let mut changed = false;
    for k in 0..2 {
        let (anchor, hp) = if k == 0 { (to(0.0, 0.0), to(h[0], h[1])) } else { (to(1.0, 1.0), to(h[2], h[3])) };
        ui.painter().line_segment([anchor, hp], Stroke::new(1.0, TEXT_DIM));
        let resp = ui.interact(Rect::from_center_size(hp, vec2(14.0, 14.0)), ui.id().with(("ease_handle", k)), Sense::drag());
        ui.painter().circle(hp, 4.5, if resp.hovered() || resp.dragged() { theme::TEXT } else { theme::BG_PANEL }, Stroke::new(1.5, theme::TEXT));
        if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.dragged()) {
            let (u, v) = from(p);
            h[2 * k] = (u * 100.0).round().clamp(0.0, 100.0) / 100.0;
            h[2 * k + 1] = ((v * 100.0).round() / 100.0).clamp(-reach, 1.0 + reach);
            changed = true;
        }
        resp.on_hover_cursor(egui::CursorIcon::Grab);
    }
    if changed {
        app.anim_set_ease(Ease::Bezier(h));
    }
}
