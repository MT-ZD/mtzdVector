//! History tab: every undo step with an automatic description; click a row
//! to jump to that state (later steps stay available to redo).

use std::collections::HashMap;

use egui::{Color32, CornerRadius, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::app::App;
use crate::document::{Shape, Snapshot};
use crate::ui::theme::{self, TEXT, TEXT_DIM, TEXT_FAINT};
use crate::ui::widgets;

fn flatten<'a, S: std::borrow::Borrow<Shape>>(list: &'a [S], out: &mut HashMap<u64, &'a Shape>) {
    for s in list {
        let s = s.borrow();
        out.insert(s.id, s);
        if let Some(c) = s.kind.children() {
            flatten(c, out);
        }
    }
}

fn names(list: &[&Shape]) -> String {
    match list {
        [s] => s.name.clone(),
        _ => format!("{} objects", list.len()),
    }
}

/// What changed from `a` to `b`, in a few words, with an icon.
pub fn describe(a: &Snapshot, b: &Snapshot) -> (&'static str, String) {
    let (mut ma, mut mb) = (HashMap::new(), HashMap::new());
    flatten(&a.shapes, &mut ma);
    flatten(&b.shapes, &mut mb);
    // Artboards added or removed (duplicating one also adds its artwork).
    if let Some(x) = b.artboards.iter().find(|x| !a.artboards.iter().any(|y| y.id == x.id)) {
        return (icon::FRAME_CORNERS, format!("Add {}", x.name));
    }
    if let Some(x) = a.artboards.iter().find(|x| !b.artboards.iter().any(|y| y.id == x.id)) {
        return (icon::TRASH, format!("Delete {}", x.name));
    }
    let added: Vec<&Shape> = b.shapes.iter().map(|s| &**s).filter(|s| !ma.contains_key(&s.id)).collect();
    let removed: Vec<&Shape> = a.shapes.iter().map(|s| &**s).filter(|s| !mb.contains_key(&s.id)).collect();
    if !added.is_empty() && !removed.is_empty() {
        // Replacements: cuts split one shape, booleans and groups combine.
        if added.len() > removed.len() {
            return (icon::KNIFE, format!("Split {} into {}", names(&removed), added.len()));
        }
        let kind = added.first().map(|s| s.kind.type_name()).unwrap_or("Shape");
        return (icon::SHAPES, format!("{kind} from {}", names(&removed)));
    }
    if !added.is_empty() {
        return (icon::PLUS, format!("Add {}", names(&added)));
    }
    if !removed.is_empty() {
        return (icon::TRASH, format!("Delete {}", names(&removed)));
    }
    if a.guides != b.guides {
        return (icon::CROSSHAIR_SIMPLE, if b.guides.len() > a.guides.len() { "Add guide" } else if b.guides.len() < a.guides.len() { "Remove guide" } else { "Move guide" }.into());
    }
    if a.artboards != b.artboards {
        let changed = b.artboards.iter().find(|x| a.artboards.iter().find(|y| y.id == x.id) != Some(*x));
        return (icon::FRAME_CORNERS, match changed {
            Some(x) => format!("Edit {}", x.name),
            None => "Artboards".into(),
        });
    }
    let ids_a: Vec<u64> = a.shapes.iter().map(|s| s.id).collect();
    let ids_b: Vec<u64> = b.shapes.iter().map(|s| s.id).collect();
    // Field-level change on the first modified shape (and how many changed).
    let mut changed: Vec<(&Shape, &Shape)> = Vec::new();
    for (id, sb) in &mb {
        if let Some(sa) = ma.get(id) {
            // Containers differ when a child does; report the leaf.
            let leaf = sa.kind.children().is_none();
            let differs = if leaf { sa != sb } else { sa.name != sb.name || sa.transform != sb.transform || sa.visible != sb.visible || sa.locked != sb.locked || sa.opacity != sb.opacity || sa.fill != sb.fill };
            if differs {
                changed.push((sa, sb));
            }
        }
    }
    changed.sort_by_key(|(s, _)| s.id);
    let Some((sa, sb)) = changed.first() else {
        if ids_a != ids_b {
            return (icon::STACK, "Reorder".into());
        }
        return (icon::DOT_OUTLINE, "No visible change".into());
    };
    let who: Vec<&Shape> = changed.iter().map(|(_, b)| *b).collect();
    let who = names(&who);
    let r = |ic, verb: &str| (ic, format!("{verb} {who}"));
    if sa.name != sb.name {
        return (icon::PENCIL_SIMPLE, format!("Rename to {}", sb.name));
    }
    if sa.visible != sb.visible {
        return r(if sb.visible { icon::EYE } else { icon::EYE_SLASH }, if sb.visible { "Show" } else { "Hide" });
    }
    if sa.locked != sb.locked {
        return r(icon::LOCK_SIMPLE, if sb.locked { "Lock" } else { "Unlock" });
    }
    if sa.kind != sb.kind {
        let text = matches!(sb.kind, crate::document::ShapeKind::Text(_));
        return r(if text { icon::TEXT_T } else { icon::BEZIER_CURVE }, if text { "Edit text of" } else { "Edit shape of" });
    }
    if sa.transform != sb.transform {
        return r(icon::ARROWS_OUT_CARDINAL, "Transform");
    }
    if sa.fill != sb.fill {
        return r(icon::PAINT_BUCKET, "Fill");
    }
    if sa.stroke != sb.stroke {
        return r(icon::LINE_SEGMENT, "Stroke");
    }
    if sa.opacity != sb.opacity {
        return r(icon::DROP_HALF, "Opacity of");
    }
    if sa.clip != sb.clip {
        return r(icon::CROP, "Clip");
    }
    r(icon::SPARKLE, "Effects on")
}

pub fn show(app: &mut App, ui: &mut Ui) {
    // Rows are derived from the stacks; cache them per history generation
    // and document revision (the last step keeps changing while dragging).
    let key = egui::Id::new("history_rows");
    let extra = app.doc.guides.iter().fold(app.doc.guides.len() as f64, |h, g| h * 31.0 + g.pos) + app.doc.artboards.iter().fold(0.0, |h, a| h * 7.0 + a.x * 3.0 + a.y * 5.0 + a.w * 11.0 + a.h * 13.0 + a.name.len() as f64 + a.background.r as f64 + a.id as f64);
    let stamp = (app.history.generation, app.doc.revision(), extra.to_bits());
    let cached = ui.data(|d| d.get_temp::<((u64, u64, u64), Vec<(&'static str, String)>)>(key)).filter(|c| c.0 == stamp);
    let now = app.history.undo_len();
    let rows = match cached {
        Some((_, rows)) => rows,
        None => {
            let current = app.doc.snapshot();
            let (past, future) = app.history.stacks();
            let states: Vec<&Snapshot> = past.iter().chain(std::iter::once(&current)).chain(future).collect();
            let rows: Vec<(&'static str, String)> = states.windows(2).map(|w| describe(w[0], w[1])).collect();
            ui.data_mut(|d| d.insert_temp(key, (stamp, rows.clone())));
            rows
        }
    };
    if rows.is_empty() {
        widgets::empty_state(ui, icon::CLOCK_COUNTER_CLOCKWISE, "No history yet", "Changes you make appear here.\nClick a step to go back to it.");
        return;
    }
    let mut jump: Option<usize> = None;
    egui::ScrollArea::vertical().id_salt("history_scroll").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        // Row 0 is the starting state; row i (1..) is the state after step i.
        let entries = std::iter::once((icon::FILE, "Start".to_string())).chain(rows.iter().cloned());
        for (i, (ic, label)) in entries.enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_H + 2.0), Sense::click());
            let is_now = i == now;
            let undone = i > now;
            let bg = if is_now {
                theme::ACCENT_DIM
            } else if resp.hovered() {
                theme::BG_HOVER
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
            let color = if is_now { Color32::WHITE } else if undone { TEXT_FAINT } else { TEXT };
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            child.spacing_mut().item_spacing.x = 8.0;
            child.label(RichText::new(ic).size(13.0).color(if is_now { Color32::WHITE } else { TEXT_DIM }));
            child.add(egui::Label::new(RichText::new(label).size(theme::FONT_MD).color(color)).truncate().selectable(false));
            if resp.clicked() && !is_now {
                jump = Some(i);
            }
        }
    });
    if let Some(target) = jump {
        let n = app.history.undo_len();
        if target < n {
            for _ in target..n {
                app.undo();
            }
        } else {
            for _ in n..target {
                app.redo();
            }
        }
        app.dirty = true;
    }
}
