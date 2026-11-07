//! Studio dock on the right: two resizable, collapsible tab groups
//! [Design | Colours] on top and [Layers | Components | Icons | History] below. Design is one
//! scrolling inspector (tool options, transform, shape, text, fill, stroke,
//! effects) in the spirit of Figma's properties panel.

pub mod appearance;
pub mod color;
pub mod components;
pub mod design;
pub mod gradient;
pub mod history;
pub mod icons;
pub mod keys;
pub mod layout_ui;
pub mod layers;
pub mod stroke;
pub mod transform;
pub mod typography;

use egui::{CursorIcon, Rect, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use super::theme::{self, TAB_H};
use super::widgets;
use crate::app::App;
use crate::document::{Shape, ShapeKind};

/// A layer's icon, and whether it is a component (main or instance).
pub fn shape_icon(s: &Shape) -> (&'static str, bool) {
    match &s.component {
        Some(crate::document::Component::Main { .. }) => (icon::DIAMONDS_FOUR, true),
        Some(crate::document::Component::Instance(_)) => (icon::DIAMOND, true),
        None => (live_icon(s).unwrap_or_else(|| kind_icon(&s.kind)), false),
    }
}

/// Icons of groups that do something: auto layouts (by direction), live
/// warps and blends.
pub fn live_icon(s: &Shape) -> Option<&'static str> {
    use crate::paint_ops::LiveEffect;
    match (&s.layout, &s.live) {
        (_, Some(LiveEffect::Warp(_))) => Some(icon::FLAG),
        (_, Some(LiveEffect::Blend { .. })) => Some(icon::CIRCLES_THREE),
        (Some(l), None) => Some(if l.dir == crate::layout::Direction::Row { icon::COLUMNS } else { icon::ROWS }),
        (None, None) => None,
    }
}

pub fn kind_icon(k: &ShapeKind) -> &'static str {
    match k {
        ShapeKind::Rect { .. } => icon::RECTANGLE,
        ShapeKind::Ellipse { .. } => icon::CIRCLE,
        ShapeKind::Star { .. } => icon::STAR,
        ShapeKind::Polygon { .. } => icon::POLYGON,
        ShapeKind::Prim { prim, .. } => prim.kind.icon(),
        ShapeKind::Path(_) => icon::BEZIER_CURVE,
        ShapeKind::Text(_) => icon::TEXT_T,
        ShapeKind::Group { .. } => icon::FOLDER_SIMPLE,
        ShapeKind::Compound { op, .. } => match op {
            crate::boolean::BoolOp::Union => icon::UNITE,
            crate::boolean::BoolOp::Subtract => icon::SUBTRACT,
            crate::boolean::BoolOp::Intersect => icon::INTERSECT,
            crate::boolean::BoolOp::Exclude => icon::EXCLUDE,
        },
    }
}

pub const DESIGN: usize = 0;
pub const LAYERS: usize = 1;
const GROUPS: [&[&str]; 2] = [&["Design", "Colours"], &["Layers", "Components", "Icons", "History"]];
const N: usize = GROUPS.len();
const MIN_BODY: f32 = 70.0;

impl App {
    /// Bring the Design inspector to the front.
    pub fn show_design(&mut self) {
        self.ui.show_studio = true;
        self.ui.tab[DESIGN] = 0;
        self.ui.collapsed[DESIGN] = false;
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let frame = egui::Frame::new().fill(theme::BG_PANEL);
    egui::Panel::right("studio").default_size(app.settings.studio_width.clamp(240.0, 480.0)).size_range(240.0..=480.0).resizable(true).frame(frame).show(ui, |ui| {
        let full = ui.max_rect();
        app.settings.studio_width = full.width().round();
        // The dock places its groups manually; claim the whole panel so the
        // panel keeps the width the user dragged it to.
        ui.expand_to_include_rect(full);
        ui.painter().vline(full.left() + 0.5, full.y_range(), Stroke::new(1.0, theme::SEPARATOR));
        dock(app, ui, full);
    });
}

fn dock(app: &mut App, ui: &mut Ui, full: Rect) {
    // Heights: collapsed groups take only their tab strip; the last expanded
    // group fills the remaining space.
    let expanded: Vec<usize> = (0..N).filter(|g| !app.ui.collapsed[*g]).collect();
    let last = expanded.last().copied();
    let total = full.height();
    let collapsed_h = (0..N).filter(|g| app.ui.collapsed[*g]).count() as f32 * TAB_H;
    let mut heights = [TAB_H; N];
    let mut used = collapsed_h;
    for g in &expanded {
        if Some(*g) != last {
            let h = (app.ui.group_h[(*g).min(N - 2)] + TAB_H).max(TAB_H + MIN_BODY);
            heights[*g] = h;
            used += h;
        }
    }
    if let Some(l) = last {
        // Shrink the upper groups proportionally when the window is too short.
        let min_last = TAB_H + MIN_BODY * 1.6;
        if used + min_last > total {
            let fixed: f32 = expanded.iter().filter(|g| Some(**g) != last).map(|g| heights[*g]).sum();
            let room = (total - collapsed_h - min_last).max(0.0);
            if fixed > 0.0 {
                let k = (room / fixed).min(1.0);
                for g in &expanded {
                    if Some(*g) != last {
                        heights[*g] = (heights[*g] * k).max(TAB_H + 40.0);
                    }
                }
            }
            used = collapsed_h + expanded.iter().filter(|g| Some(**g) != last).map(|g| heights[*g]).sum::<f32>();
        }
        heights[l] = (total - used).max(TAB_H + 40.0);
    }

    let mut y = full.top();
    let mut rects = [Rect::NOTHING; N];
    for g in 0..N {
        rects[g] = Rect::from_min_size(pos2(full.left() + 1.0, y), vec2(full.width() - 1.0, heights[g]));
        group(app, ui, g, rects[g]);
        y += heights[g];
    }
    // Splitters go on top of the groups so tab headers and scroll areas
    // don't swallow the drag.
    for g in 0..N - 1 {
        if app.ui.collapsed[g] || Some(g) == last {
            continue;
        }
        let rect = rects[g];
        let sr = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 4.0), pos2(rect.right(), rect.bottom() + 4.0));
        let resp = ui.interact(sr, ui.id().with(("split", g)), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(2.0, theme::ACCENT.gamma_multiply(0.7)));
        }
        if resp.drag_started() {
            // Start from the height actually shown (it may have been shrunk to fit).
            app.ui.group_h[g] = heights[g] - TAB_H;
        }
        if resp.dragged() {
            let max = (total - collapsed_h - TAB_H * (expanded.len() as f32) - MIN_BODY * 1.6).max(MIN_BODY);
            app.ui.group_h[g] = (app.ui.group_h[g] + resp.drag_delta().y).clamp(MIN_BODY, max);
        }
    }
}

fn group(app: &mut App, ui: &mut Ui, g: usize, rect: Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut tab = app.ui.tab[g];
    let mut collapsed = app.ui.collapsed[g];
    widgets::tab_group_header(ui, GROUPS[g], &mut tab, &mut collapsed);
    app.ui.tab[g] = tab.min(GROUPS[g].len() - 1);
    app.ui.collapsed[g] = collapsed;
    if collapsed {
        return;
    }
    ui.spacing_mut().item_spacing.y = theme::SP1;
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + TAB_H), rect.max);
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    b.set_clip_rect(body.intersect(ui.clip_rect()));
    let tab = app.ui.tab[g];
    if g == LAYERS {
        // Layers manage their own scrolling (virtualized rows + drag reorder).
        egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 6)).show(&mut b, |ui| {
            ui.set_min_height(body.height() - 12.0);
            match tab {
                0 => layers::show(app, ui),
                1 => components::show(app, ui),
                2 => {
                    ui.set_width(body.width() - 12.0);
                    icons::show(app, ui)
                }
                _ => history::show(app, ui),
            }
        });
        return;
    }
    egui::ScrollArea::vertical().id_salt(("studio_scroll", g, tab)).auto_shrink([false, false]).max_height(body.height()).show(&mut b, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 6, bottom: 16 }).show(ui, |ui| {
            ui.set_width(body.width() - 32.0);
            match tab {
                1 => keys::show(app, ui),
                _ => design::show(app, ui),
            }
        });
    });
}

/// Like [`edit_selection`], but groups stand for their descendants (fill,
/// stroke and text edits reach the shapes inside).
pub fn edit_style(app: &mut App, ui: &mut Ui, f: impl FnOnce(&mut App, &mut Ui, &mut Shape)) {
    let saved = app.selection.clone();
    let targets = app.style_targets();
    if targets == saved {
        return edit_selection(app, ui, f);
    }
    app.selection = targets;
    edit_selection(app, ui, f);
    // Keep the user's selection unless the edit itself changed it.
    app.selection = saved.into_iter().filter(|id| app.doc.get(*id).is_some()).collect();
}

/// Edit the primary selected shape through `f`; style changes (fill, stroke,
/// opacity, fill rule, font) propagate to the rest of the selection.
pub fn edit_selection(app: &mut App, ui: &mut Ui, f: impl FnOnce(&mut App, &mut Ui, &mut Shape)) {
    let Some(base) = app.selection.first().and_then(|id| app.doc.get(*id)).cloned() else { return };
    let mut p = base.clone();
    f(app, ui, &mut p);
    if p == base {
        return;
    }
    // Editing a linked colour by hand unlinks it from its colour key.
    let stroke_color = |s: &Shape| s.stroke.as_ref().map(|st| st.color);
    if p.fill != base.fill && p.fill_key == base.fill_key {
        p.fill_key = None;
    }
    if stroke_color(&p) != stroke_color(&base) && p.stroke_key == base.stroke_key {
        p.stroke_key = None;
    }
    app.checkpoint("inspector");
    app.doc.replace(p.clone());
    for id in app.selection.clone().into_iter().skip(1) {
        let Some(mut s) = app.doc.get(id).cloned() else { continue };
        if p.fill != base.fill {
            s.fill = p.fill.clone();
        }
        if p.fill_key != base.fill_key || p.fill != base.fill {
            s.fill_key = p.fill_key;
        }
        if p.stroke_key != base.stroke_key || stroke_color(&p) != stroke_color(&base) {
            s.stroke_key = p.stroke_key;
        }
        if p.stroke != base.stroke {
            match (&p.stroke, &mut s.stroke, &base.stroke) {
                (Some(ps), Some(ss), Some(bs)) => merge_changed(ps, bs, ss),
                _ => s.stroke = p.stroke.clone(),
            }
        }
        merge_shadow(&p.shadow, &base.shadow, &mut s.shadow);
        merge_shadow(&p.inner_shadow, &base.inner_shadow, &mut s.inner_shadow);
        if p.opacity != base.opacity {
            s.opacity = p.opacity;
        }
        if p.blur != base.blur {
            s.blur = p.blur;
        }
        if p.noise != base.noise {
            s.noise = p.noise;
        }
        if p.outlines != base.outlines {
            s.outlines = p.outlines.clone();
        }
        if p.fill_rule != base.fill_rule {
            s.fill_rule = p.fill_rule;
        }
        if p.fills != base.fills {
            s.fills = p.fills.clone();
        }
        if p.strokes != base.strokes {
            s.strokes = p.strokes.clone();
        }
        if p.blend != base.blend {
            s.blend = p.blend;
        }
        if let (ShapeKind::Text(a), ShapeKind::Text(b), ShapeKind::Text(t)) = (&p.kind, &base.kind, &mut s.kind) {
            if a.family != b.family || a.weight != b.weight || a.italic != b.italic {
                t.family = a.family.clone();
                t.weight = a.weight;
                t.italic = a.italic;
            }
            // Every other object-wide text attribute that changed (not the
            // content, frame, path, spans or threads of each text).
            macro_rules! sync {
                ($($f:ident),*) => { $( if a.$f != b.$f { t.$f = a.$f.clone(); } )* };
            }
            sync!(size, align, line_height, letter_spacing, case, underline, strike, features, variations, paragraph_spacing, columns, column_gap, hyphenate, lang, path_offset);
            if a.style_id != b.style_id {
                t.style_id = a.style_id;
            }
            // Object-wide edits also clear range overrides of that attribute.
            let mask = crate::document::CharStyle::diff(&b.base_style(), &a.base_style());
            if !mask.is_empty() {
                t.clear_overrides(&mask);
            }
        }
        app.doc.replace(s);
    }
}

/// Copy the stroke properties that changed from `before` to `after` onto `target`.
/// Apply the fields of a shadow the user changed (on the first selected
/// shape) to another shape's shadow, keeping its other values.
fn merge_shadow(after: &Option<crate::document::Shadow>, before: &Option<crate::document::Shadow>, target: &mut Option<crate::document::Shadow>) {
    if after == before {
        return;
    }
    match (after, target.as_mut(), before) {
        (Some(ps), Some(ss), Some(bs)) => {
            if ps.color != bs.color {
                ss.color = ps.color;
            }
            if ps.dx != bs.dx {
                ss.dx = ps.dx;
            }
            if ps.dy != bs.dy {
                ss.dy = ps.dy;
            }
            if ps.blur != bs.blur {
                ss.blur = ps.blur;
            }
        }
        _ => *target = after.clone(),
    }
}

fn merge_changed(after: &crate::document::Stroke, before: &crate::document::Stroke, target: &mut crate::document::Stroke) {
    macro_rules! take {
        ($($f:ident),*) => {$(
            if after.$f != before.$f {
                target.$f = after.$f.clone();
            }
        )*};
    }
    take!(color, width, join, cap, miter_limit, align, dash, dash_offset, start_arrow, end_arrow, arrow_scale);
}
