//! Layers tab: z-ordered rows with rename, visibility, lock, drag reorder and
//! the shared right-click menu.

use egui::{Align2, Color32, CornerRadius, RichText, Sense, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;

use super::shape_icon;
use crate::app::{App, Tool};
use crate::ui::theme::{self, TEXT, TEXT_DIM, TEXT_FAINT};
use crate::ui::widgets;

/// Indent of rows under a section header.
const SECTION_INDENT: f32 = 10.0;

enum Item {
    /// Section header: artboard (None: outside artboards), member count.
    Header(Option<u64>, usize),
    /// Shape row: id, depth inside containers, section.
    Row(u64, usize, Option<u64>),
}

#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Header(Option<u64>),
    /// A row: shape, section, depth, and whether layers can be dropped in.
    Row(u64, Option<u64>, usize, bool),
}

/// Where a dragged layer lands.
#[derive(Clone, Copy, PartialEq)]
enum Drop {
    /// Above / below this shape (any depth), in this section.
    Above(u64, Option<u64>),
    Below(u64, Option<u64>),
    /// Into this group or compound, on top of its layers.
    Into(u64, Option<u64>),
    /// Top or bottom of a section.
    Top(Option<u64>),
    Bottom(Option<u64>),
}

impl Drop {
    /// The receiving artboard, if any.
    fn section(self) -> Option<u64> {
        match self {
            Drop::Above(_, s) | Drop::Below(_, s) | Drop::Into(_, s) | Drop::Top(s) | Drop::Bottom(s) => s,
        }
    }
}

impl Slot {
    fn sec(self) -> Option<u64> {
        match self {
            Slot::Header(s) | Slot::Row(_, s, _, _) => s,
        }
    }
}

/// Drop marker: a line at `y` indented by `depth`, or (for `Into`) the row.
struct Marker {
    y: f32,
    depth: usize,
    row: Option<egui::Rect>,
}

/// The drop position for pointer height `y`: on the middle of a group's row
/// the layer goes into it, else above or below the row.
fn drop_target(slots: &[(Slot, egui::Rect)], y: f32) -> Option<(Drop, Marker)> {
    let (last, last_r) = slots.last()?;
    let Some(k) = slots.iter().position(|(_, r)| y < r.bottom()) else {
        return Some((Drop::Bottom(last.sec()), Marker { y: last_r.bottom(), depth: 0, row: None }));
    };
    let (slot, r) = slots[k];
    let rel = (y - r.top()) / r.height().max(1.0);
    Some(match slot {
        Slot::Row(id, sec, depth, container) if container && (0.25..0.75).contains(&rel) => (Drop::Into(id, sec), Marker { y: r.center().y, depth, row: Some(r) }),
        Slot::Row(id, sec, depth, _) if rel < 0.5 => (Drop::Above(id, sec), Marker { y: r.top(), depth, row: None }),
        Slot::Row(id, sec, depth, _) => (Drop::Below(id, sec), Marker { y: r.bottom(), depth, row: None }),
        Slot::Header(sec) if k == 0 || rel >= 0.5 => (Drop::Top(sec), Marker { y: r.bottom(), depth: 0, row: None }),
        Slot::Header(_) => (Drop::Bottom(slots[k - 1].0.sec()), Marker { y: r.top(), depth: 0, row: None }),
    })
}

/// Apply a layer drop: move the shape into the target artboard (shifting its
/// geometry) and restack it.
fn drop_layer(app: &mut App, sections: &[(Option<u64>, Vec<u64>)], id: u64, from: Option<u64>, to: Drop) {
    if matches!(to, Drop::Above(x, _) | Drop::Below(x, _) | Drop::Into(x, _) if x == id) {
        return;
    }
    // Nested layers (or nesting): reparent, keeping the geometry.
    let nested_target = match to {
        Drop::Above(x, _) | Drop::Below(x, _) => app.doc.parent_of(x).is_some(),
        Drop::Into(..) => true,
        _ => false,
    };
    if nested_target || app.doc.parent_of(id).is_some() {
        // An instance can't go inside its own main component.
        let dest = match to {
            Drop::Into(g, _) => Some(g),
            Drop::Above(x, _) | Drop::Below(x, _) => app.doc.parent_of(x),
            _ => None,
        };
        let mains: Vec<u64> = app.doc.all_ids().into_iter().filter(|i| app.doc.is_within(*i, id)).filter_map(|i| app.instance_link(i).map(|l| l.main)).collect();
        if dest.is_some_and(|d| mains.iter().any(|m| app.doc.is_within(d, *m))) {
            app.notify("An instance can't go inside its own main component");
            return;
        }
        app.checkpoint("");
        let ok = match to {
            Drop::Into(g, _) => app.doc.move_into(id, Some(g), usize::MAX),
            Drop::Above(x, _) => app.doc.move_beside(id, x, true),
            Drop::Below(x, _) => app.doc.move_beside(id, x, false),
            Drop::Top(_) | Drop::Bottom(_) => app.doc.move_into(id, None, if matches!(to, Drop::Top(_)) { usize::MAX } else { 0 }),
        };
        if ok {
            app.dirty = true;
            app.layout_key = None;
            if let Drop::Into(g, _) = to {
                let name = app.doc.get(g).map(|s| s.name.clone()).unwrap_or_default();
                app.notify(format!("Moved into {name}"));
            }
        }
        return;
    }
    let members: Vec<u64> = sections.iter().find(|s| s.0 == to.section()).map(|s| s.1.iter().copied().filter(|x| *x != id).collect()).unwrap_or_default();
    let before = (app.doc.index_of(id), from);
    app.checkpoint("");
    let moved = app.move_to_artboard(id, from, to.section());
    // Members are listed top-most first.
    match to {
        Drop::Above(x, _) => app.doc.move_next_to(id, x, true),
        Drop::Below(x, _) => app.doc.move_next_to(id, x, false),
        Drop::Into(..) => {}
        Drop::Top(_) => {
            if let Some(x) = members.first() {
                app.doc.move_next_to(id, *x, true);
            } else if to.section().is_none() {
                let n = app.doc.shapes().len();
                app.doc.move_to_index(id, n);
            }
        }
        Drop::Bottom(_) => {
            if let Some(x) = members.last() {
                app.doc.move_next_to(id, *x, false);
            }
        }
    }
    if moved || app.doc.index_of(id) != before.0 {
        app.dirty = true;
    }
    if moved {
        let name = to.section().and_then(|a| app.doc.artboard(a)).map(|a| a.name.clone());
        app.notify(match name {
            Some(n) => format!("Moved into {n}"),
            None => "Moved out of the artboard".to_string(),
        });
    }
}

fn fold_key(sec: Option<u64>) -> egui::Id {
    egui::Id::new(("layer_section_fold", sec))
}

/// An artboard's (or the loose canvas's) header row: fold caret, name,
/// count; click selects the artboard, double-click zooms to it; the crop
/// icon toggles clipping. Returns the row rect.
fn section_header(app: &mut App, ui: &mut Ui, sec: Option<u64>, n: usize, h: f32) -> egui::Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let ab = sec.and_then(|a| app.doc.artboard(a)).cloned();
    let selected = ab.as_ref().map(|a| app.artboard_selected && app.active().id == a.id).unwrap_or(false);
    let bg = if selected { theme::ACCENT_DIM } else if resp.hovered() { theme::BG_HOVER } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(4.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 4.0;
    let folded = child.data(|d| d.get_temp::<bool>(fold_key(sec))).unwrap_or(false);
    if widgets::mini_icon(&mut child, if folded { icon::CARET_RIGHT } else { icon::CARET_DOWN }, TEXT_DIM).clicked() {
        child.data_mut(|d| d.insert_temp(fold_key(sec), !folded));
    }
    let fg = if selected { Color32::WHITE } else { TEXT };
    let (glyph, name) = match &ab {
        Some(a) => (icon::FRAME_CORNERS, a.name.clone()),
        None => (icon::DOTS_NINE, "Outside artboards".to_string()),
    };
    child.label(RichText::new(glyph).size(13.0).color(if selected { Color32::WHITE } else { TEXT_DIM }));
    let renaming = !app.rename_on_canvas && sec.is_some() && app.renaming.as_ref().map(|r| Some(r.0)) == Some(sec);
    if renaming {
        let w = (child.available_width() - 40.0).max(40.0);
        rename_field(app, &mut child, w);
    } else {
        child.add(egui::Label::new(RichText::new(name).size(theme::FONT_MD).strong().color(fg)).truncate().selectable(false));
    }
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if let Some(a) = &ab {
            let on = a.clip;
            if on || ui.rect_contains_pointer(rect) {
                let r = widgets::mini_icon(ui, icon::CROP, if on { TEXT } else { TEXT_DIM });
                if widgets::tip(r, if on { "Clipping content — click to show overflow" } else { "Clip content to the artboard" }, None).clicked() {
                    app.checkpoint("");
                    if let Some(a) = app.doc.artboard_mut(a.id) {
                        a.clip = !on;
                    }
                    app.dirty = true;
                }
            }
        }
        ui.label(RichText::new(n.to_string()).size(theme::FONT_SM).color(TEXT_FAINT));
    });
    if let Some(a) = &ab {
        if row_double_click(ui, &resp, a.id) {
            // Like layers: double-click renames.
            app.activate_artboard(a.id, true);
            app.start_rename(a.id);
        } else if resp.clicked() {
            app.activate_artboard(a.id, true);
        }
    }
    rect
}

/// Two clicks on the same row in quick succession. (egui counts clicks
/// globally, so a click elsewhere just before — e.g. selecting on the canvas
/// — would turn this into a "triple click".)
pub fn row_double_click(ui: &Ui, resp: &egui::Response, id: u64) -> bool {
    if !resp.clicked() {
        return false;
    }
    let key = egui::Id::new("row_last_click");
    let now = ui.input(|i| i.time);
    let last: Option<(u64, f64)> = ui.data(|d| d.get_temp(key));
    let double = matches!(last, Some((l, t)) if l == id && now - t < 0.45);
    ui.data_mut(|d| d.insert_temp(key, if double { (0u64, f64::MIN) } else { (id, now) }));
    double
}

/// The inline name editor of the layer or artboard in `app.renaming`. Takes
/// focus (all text selected) when it opens; Enter or clicking elsewhere
/// commits, Escape cancels.
pub fn rename_field(app: &mut App, ui: &mut Ui, width: f32) {
    let Some((id, mut text)) = app.renaming.clone() else { return };
    let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(width).font(egui::TextStyle::Body));
    if app.rename_fresh {
        app.rename_fresh = false;
        r.request_focus();
        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id) {
            let n = text.chars().count();
            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
            st.store(ui.ctx(), r.id);
        }
        app.renaming = Some((id, text));
        return;
    }
    app.renaming = Some((id, text.clone()));
    if r.lost_focus() || !r.has_focus() {
        let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let old = app.doc.artboard(id).map(|a| a.name.clone()).or_else(|| app.doc.get(id).map(|s| s.name.clone()));
        if !cancel && !text.trim().is_empty() && old.as_deref() != Some(text.as_str()) {
            app.checkpoint("");
            if let Some(a) = app.doc.artboard_mut(id) {
                a.name = text;
            } else if let Some(s) = app.doc.get_mut(id) {
                s.name = text;
            }
            app.dirty = true;
        }
        app.renaming = None;
    }
}

fn open_key(id: u64) -> egui::Id {
    egui::Id::new(("layer_open", id))
}

/// Whether a boolean's children are listed: explicit toggle, else open while
/// one of its descendants is selected.
/// `ancestors`: every container above a selected shape (`selected_ancestors`).
fn is_open(ui: &Ui, id: u64, ancestors: &std::collections::HashSet<u64>) -> bool {
    ui.data(|d| d.get_temp::<bool>(open_key(id))).unwrap_or_else(|| ancestors.contains(&id))
}

fn selected_ancestors(app: &App) -> std::collections::HashSet<u64> {
    let mut out = std::collections::HashSet::new();
    for s in &app.selection {
        let mut p = app.doc.parent_of(*s);
        while let Some(x) = p {
            if !out.insert(x) {
                break;
            }
            p = app.doc.parent_of(x);
        }
    }
    out
}

/// Whether `id` or one of its descendants has a name containing `q` (lowercase).
fn matches(app: &App, id: u64, q: &str) -> bool {
    let Some(s) = app.doc.get(id) else { return false };
    s.name.to_lowercase().contains(q) || s.kind.children().map(|c| c.iter().any(|c| matches(app, c.id, q))).unwrap_or(false)
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let ids: Vec<u64> = app.doc.shapes().iter().rev().map(|s| s.id).collect();
    if ids.is_empty() {
        widgets::empty_state(ui, icon::STACK, "No layers yet", "Draw with a tool on the left,\nor drop an SVG / PDF / image on the canvas.");
        return;
    }
    // Search by name.
    let qid = egui::Id::new("layer_query");
    let mut query = ui.data(|d| d.get_temp::<String>(qid)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(TEXT_DIM));
        let r = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search layers").desired_width(ui.available_width() - 22.0).font(egui::TextStyle::Body));
        if !query.is_empty() && (widgets::mini_icon(ui, icon::X, TEXT_DIM).clicked() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)))) {
            query.clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(qid, query.clone()));
    let q = query.trim().to_lowercase();
    let ids: Vec<u64> = ids.into_iter().filter(|id| q.is_empty() || matches(app, *id, &q)).collect();
    if ids.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new(format!("No layers match “{}”", query.trim())).size(theme::FONT_SM).color(TEXT_FAINT));
        return;
    }
    let canvas_zoom = app.view.zoom;
    let row_h = theme::ROW_H + 2.0;
    // Sections: one per artboard (creation order), then the loose canvas.
    let mut sections: Vec<(Option<u64>, Vec<u64>)> = app.doc.artboards.iter().map(|a| (Some(a.id), Vec::new())).collect();
    sections.push((None, Vec::new()));
    // Top-level indices of each section's members (parallel to `sections`).
    let mut section_idx: Vec<Vec<usize>> = vec![Vec::new(); sections.len()];
    let shown: Option<std::collections::HashSet<u64>> = (!q.is_empty()).then(|| ids.iter().copied().collect());
    for (i, s) in app.doc.shapes().iter().enumerate().rev() {
        if shown.as_ref().is_some_and(|set| !set.contains(&s.id)) {
            continue;
        }
        let sec = app.cache.update(s, &mut app.fonts).bounds.and_then(|b| app.doc.member_of(b)).map(|a| a.id);
        if let Some(k) = sections.iter().position(|e| e.0 == sec) {
            sections[k].1.push(s.id);
            section_idx[k].push(i);
        }
    }
    let dragging = app.layer_drag.is_some();
    egui::ScrollArea::vertical().id_salt("layers_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        // Drop slots in display order: section headers and top-level rows.
        let mut slots: Vec<(Slot, egui::Rect)> = Vec::new();
        // Tree rows: (id, depth, section); container children follow their
        // parent when expanded.
        let mut items: Vec<Item> = Vec::new();
        let ancestors = selected_ancestors(app);
        #[allow(clippy::too_many_arguments)]
        fn push_tree(app: &App, ui: &Ui, s: &crate::document::Shape, depth: usize, sec: Option<u64>, q: &str, ancestors: &std::collections::HashSet<u64>, items: &mut Vec<Item>) {
            items.push(Item::Row(s.id, depth, sec));
            let Some(children) = s.kind.children() else { return };
            // While searching, containers with matches open by themselves.
            if is_open(ui, s.id, ancestors) || !q.is_empty() {
                // An auto layout's background is the group's own fill.
                let bg = s.layout.as_ref().and_then(|l| l.background);
                for c in children.iter().rev() {
                    if Some(c.id) != bg && (q.is_empty() || matches(app, c.id, q)) {
                        push_tree(app, ui, c, depth + 1, sec, q, ancestors, items);
                    }
                }
            }
        }
        for (k, (sec, members)) in sections.iter().enumerate() {
            // Empty sections only while dragging (to drop into) or for artboards
            // when not searching.
            let show = !members.is_empty() || dragging || (sec.is_some() && q.is_empty());
            if !show {
                continue;
            }
            items.push(Item::Header(*sec, members.len()));
            if !ui.data(|d| d.get_temp::<bool>(fold_key(*sec))).unwrap_or(false) || !q.is_empty() {
                for &i in &section_idx[k] {
                    push_tree(app, ui, &app.doc.shapes()[i], 0, *sec, &q, &ancestors, &mut items);
                }
            }
        }
        let visible_ids: Vec<u64> = items.iter().filter_map(|i| if let Item::Row(id, ..) = i { Some(*id) } else { None }).collect();
        let anchor_id = egui::Id::new("layer_anchor");
        let selection: std::collections::HashSet<u64> = app.selection.iter().copied().collect();
        // Height of the rows skipped (out of view) since the last drawn one.
        let gap = ui.spacing().item_spacing.y;
        let mut skipped = 0.0f32;
        let flush = |ui: &mut Ui, skipped: &mut f32| {
            if *skipped > 0.0 {
                ui.allocate_space(vec2(ui.available_width(), *skipped - gap));
                *skipped = 0.0;
            }
        };
        for item in &items {
            let (id, depth, sec) = match item {
                Item::Header(sec, n) => {
                    flush(ui, &mut skipped);
                    let rect = section_header(app, ui, *sec, *n, row_h);
                    slots.push((Slot::Header(*sec), rect));
                    continue;
                }
                Item::Row(id, depth, sec) => (id, *depth, *sec),
            };
            // Rows scrolled out of view only take their space.
            let rect = egui::Rect::from_min_size(ui.cursor().min + vec2(0.0, skipped), vec2(ui.available_width(), row_h));
            if !ui.is_rect_visible(rect) && app.renaming.as_ref().map(|r| r.0) != Some(*id) {
                skipped += row_h + gap;
                slots.push((Slot::Row(*id, sec, depth, accepts_drop(app, *id)), rect));
                continue;
            }
            flush(ui, &mut skipped);
            let Some(s) = app.doc.get(*id) else { continue };
            let (name, (kind_ic, comp), visible, locked) = (s.name.clone(), shape_icon(s), s.visible, s.locked);
            let clipped = s.clip.is_some();
            let thumb = !s.kind.is_container() && !matches!(s.kind, crate::document::ShapeKind::Text(_));
            let is_compound = s.kind.is_container();
            let selected = selection.contains(id);
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
            slots.push((Slot::Row(*id, sec, depth, accepts_drop(app, *id)), rect));
            let hovered = resp.hovered() || app.hover == Some(*id);
            let bg = if selected {
                theme::ACCENT_DIM
            } else if hovered {
                theme::BG_HOVER
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
            if hovered && !selected {
                app.hover = Some(*id);
            }
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            child.spacing_mut().item_spacing.x = 6.0;
            child.add_space(SECTION_INDENT + depth as f32 * 14.0);
            if is_compound {
                let open = is_open(&child, *id, &ancestors);
                let r = widgets::mini_icon(&mut child, if open { icon::CARET_DOWN } else { icon::CARET_RIGHT }, TEXT_DIM);
                if r.clicked() {
                    child.data_mut(|d| d.insert_temp(open_key(*id), !open));
                }
            } else if depth > 0 {
                child.add_space(16.0);
            }
            if thumb {
                // Thumbnail on a neutral tile.
                let (tr, _) = child.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                child.painter().rect_filled(tr, CornerRadius::same(3), Color32::from_gray(0x44));
                if let Some(shape) = app.doc.get(*id).cloned() {
                    let painter = child.painter().clone();
                    app.cache.paint_thumbnail(&painter, &shape, &app.doc, &mut app.fonts, tr, canvas_zoom);
                }
            } else {
                child.label(RichText::new(kind_ic).size(14.0).color(if selected { Color32::WHITE } else if comp { theme::COMPONENT } else { TEXT_DIM }));
            }
            let renaming = !app.rename_on_canvas && app.renaming.as_ref().map(|r| r.0) == Some(*id);
            if renaming {
                let w = (child.available_width() - 46.0).max(40.0);
                rename_field(app, &mut child, w);
            } else {
                let color = if !visible { TEXT_FAINT } else if selected { Color32::WHITE } else if comp { theme::COMPONENT } else { TEXT };
                child.add(egui::Label::new(RichText::new(&name).size(theme::FONT_MD).color(color)).truncate().selectable(false));
            }
            child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let show_icons = ui.rect_contains_pointer(rect);
                if clipped {
                    let r = widgets::mini_icon(ui, icon::CROP, if selected { Color32::WHITE } else { TEXT_DIM });
                    if widgets::tip(r, "Clipped by a mask — edit or release it in Design", None).clicked() {
                        app.selection = vec![*id];
                        app.show_design();
                    }
                }
                if show_icons || locked {
                    let r = widgets::mini_icon(ui, if locked { icon::LOCK_SIMPLE } else { icon::LOCK_SIMPLE_OPEN }, if locked { TEXT } else { TEXT_DIM });
                    if widgets::tip(r, if locked { "Unlock" } else { "Lock" }, None).clicked() {
                        app.checkpoint("");
                        if let Some(s) = app.doc.get_mut(*id) {
                            s.locked = !s.locked;
                        }
                        if !locked {
                            app.selection.retain(|x| x != id);
                        }
                    }
                }
                if show_icons || !visible {
                    let r = widgets::mini_icon(ui, if visible { icon::EYE } else { icon::EYE_SLASH }, if visible { TEXT_DIM } else { TEXT });
                    if widgets::tip(r, if visible { "Hide" } else { "Show" }, None).clicked() {
                        app.checkpoint("");
                        if let Some(s) = app.doc.get_mut(*id) {
                            s.visible = !s.visible;
                        }
                    }
                }
            });
            if row_double_click(ui, &resp, *id) {
                app.start_rename(*id);
            } else if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                let anchor = ui.data(|d| d.get_temp::<u64>(anchor_id)).filter(|a| visible_ids.contains(a));
                if m.shift && anchor.is_some() {
                    // Range from the last clicked row (same level only).
                    let (a, b) = (visible_ids.iter().position(|x| Some(*x) == anchor).unwrap(), visible_ids.iter().position(|x| x == id).unwrap());
                    let parent = app.doc.parent_of(*id);
                    let range: Vec<u64> = visible_ids[a.min(b)..=a.max(b)].iter().copied().filter(|x| app.doc.parent_of(*x) == parent).collect();
                    if m.command {
                        for x in range {
                            if !app.selection.contains(&x) {
                                app.selection.push(x);
                            }
                        }
                    } else {
                        app.selection = range;
                    }
                } else if m.command {
                    if let Some(i) = app.selection.iter().position(|x| x == id) {
                        app.selection.remove(i);
                    } else {
                        app.selection.push(*id);
                    }
                    ui.data_mut(|d| d.insert_temp(anchor_id, *id));
                } else {
                    app.selection = vec![*id];
                    ui.data_mut(|d| d.insert_temp(anchor_id, *id));
                }
                if app.tool.is_text() {
                    app.set_tool(Tool::Select);
                }
            }
            if resp.hovered() && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) {
                if !app.selection.contains(id) {
                    app.selection = vec![*id];
                }
                if let Some(pos) = ui.input(|i| i.pointer.press_origin()).or(resp.hover_pos()) {
                    app.open_context_menu(pos, true);
                }
            }
            if resp.drag_started() && q.is_empty() {
                app.layer_drag = Some(*id);
            }
        }
        flush(ui, &mut skipped);
        // Drag & drop: restack, or move into another artboard.
        if let Some(drag_id) = app.layer_drag {
            let target = ui.input(|i| i.pointer.interact_pos()).and_then(|pos| drop_target(&slots, pos.y));
            if let Some((t, m)) = &target {
                let t = *t;
                if let Some(r) = m.row {
                    ui.painter().rect_stroke(r, CornerRadius::same(3), Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
                } else {
                    let x = slots.first().map(|r| r.1.x_range()).unwrap_or(egui::Rangef::new(0.0, 0.0));
                    let x = egui::Rangef::new(x.min + SECTION_INDENT + m.depth as f32 * 14.0, x.max);
                    ui.painter().hline(x, m.y, Stroke::new(2.0, theme::ACCENT));
                    ui.painter().circle_filled(egui::pos2(x.min + 2.0, m.y), 3.0, theme::ACCENT);
                }
                if let Some(sec) = t.section() {
                    // Outline the receiving artboard's header.
                    if let Some((_, r)) = slots.iter().find(|(s, _)| *s == Slot::Header(Some(sec))) {
                        ui.painter().rect_stroke(*r, CornerRadius::same(3), Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
                    }
                }
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let Some((t, _)) = target {
                    let from = slots.iter().find_map(|(s, _)| if let Slot::Row(i, sec, _, _) = s { (*i == drag_id).then_some(*sec) } else { None }).flatten();
                    // A selected row drags the whole selection along (top-most first).
                    let mut ids = if app.selection.contains(&drag_id) { app.selection.clone() } else { vec![drag_id] };
                    ids.sort_by_key(|i| std::cmp::Reverse(app.doc.order_key(*i)));
                    // Each move lands next to the target: go bottom-most first
                    // where later moves would otherwise land above earlier ones.
                    if matches!(t, Drop::Below(..) | Drop::Bottom(_) | Drop::Into(..)) {
                        ids.reverse();
                    }
                    for id in ids {
                        drop_layer(app, &sections, id, from, t);
                    }
                }
                app.layer_drag = None;
            }
        }
    });
    let _ = Align2::CENTER_CENTER;
}

/// Layers can be dropped into groups and compounds (not instances, whose
/// layers mirror their main component).
fn accepts_drop(app: &App, id: u64) -> bool {
    app.doc.get(id).is_some_and(|s| s.kind.children().is_some() && !matches!(s.component, Some(crate::document::Component::Instance(_))))
}
