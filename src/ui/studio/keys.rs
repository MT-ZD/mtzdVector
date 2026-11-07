//! Colours tab: the document's named colour keys. Rename and recolour them
//! (linked fills and strokes follow), apply them to the selection, search
//! them, or collect the colours already in use.

use egui::{Color32, CornerRadius, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::app::App;
use crate::document::{Paint, Rgba};
use crate::ui::theme::{self, ACCENT, TEXT_DIM, TEXT_FAINT};
use crate::ui::widgets;

#[derive(Default, Clone)]
pub struct KeysState {
    pub query: String,
    /// Key whose colour editor is open.
    pub open: Option<u64>,
    /// Key whose name field should take focus.
    pub rename: Option<u64>,
}

impl App {
    /// Link the selection's fills (or strokes) to key `id`.
    pub fn apply_color_key(&mut self, id: u64, to_stroke: bool) {
        let Some(key) = self.doc.color_key(id).cloned() else { return };
        let targets = self.style_targets();
        if targets.is_empty() {
            return;
        }
        self.checkpoint("");
        for sid in targets {
            let Some(mut s) = self.doc.get(sid).cloned() else { continue };
            if to_stroke {
                let mut st = s.stroke.clone().unwrap_or(crate::document::Stroke { width: 2.0, ..Default::default() });
                st.color = key.color;
                s.stroke = Some(st);
                s.stroke_key = Some(id);
            } else {
                s.fill = Some(Paint::Solid(key.color));
                s.fill_key = Some(id);
            }
            self.doc.replace(s);
        }
    }

    /// Create keys for the solid colours used in the document that have
    /// none, and link the fills and strokes using them.
    pub fn keys_from_document(&mut self) -> usize {
        let used = super::design::document_colors(self);
        let new: Vec<Rgba> = used.into_iter().filter(|c| !self.doc.colors.iter().any(|k| k.color == *c)).collect();
        if new.is_empty() {
            return 0;
        }
        self.checkpoint("");
        let keys: Vec<(u64, Rgba)> = new.iter().map(|c| (self.doc.add_color_key("", *c), *c)).collect();
        for id in self.doc.all_ids() {
            let Some(mut s) = self.doc.get(id).cloned() else { continue };
            if s.fill_key.is_none() {
                if let Some(c) = s.fill.as_ref().and_then(|f| f.solid()) {
                    s.fill_key = keys.iter().find(|k| k.1 == c).map(|k| k.0);
                }
            }
            if s.stroke_key.is_none() {
                if let Some(c) = s.stroke.as_ref().map(|st| st.color) {
                    s.stroke_key = keys.iter().find(|k| k.1 == c).map(|k| k.0);
                }
            }
            self.doc.replace(s);
        }
        new.len()
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let mut st = std::mem::take(&mut app.ui.keys);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let add_w = theme::ROW_H;
        widgets::text_field(ui, &mut st.query, &format!("{}  Search colour keys", icon::MAGNIFYING_GLASS), ui.available_width() - add_w - 6.0);
        let (r, resp) = ui.allocate_exact_size(vec2(add_w, theme::ROW_H), Sense::click());
        ui.painter().rect_filled(r, theme::radius(), if resp.hovered() { theme::BG_HOVER } else { theme::BG_INPUT });
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, icon::PLUS, egui::FontId::proportional(13.0), theme::TEXT);
        if widgets::tip(resp, "New colour key (from the selection's fill)", None).clicked() {
            let c = app.selected_shapes().first().and_then(|s| s.fill.as_ref()).and_then(|f| f.representative()).unwrap_or(Rgba::from_hex("#3D8BFD").unwrap());
            app.checkpoint("");
            let id = app.doc.add_color_key("", c);
            st.open = Some(id);
            st.rename = Some(id);
            st.query.clear();
        }
    });
    ui.add_space(4.0);
    let keys = app.doc.colors.clone();
    if keys.is_empty() {
        widgets::empty_state(ui, icon::SWATCHES, "No colour keys", "Named colours for this document. Fills and strokes linked to a key change with it.");
        ui.vertical_centered(|ui| {
            if widgets::tip(widgets::flat_button(ui, Some(icon::PALETTE), "Add colours in use", true), "A key for every solid colour in the document, linked to the shapes that use it", None).clicked() {
                let n = app.keys_from_document();
                app.notify(if n == 0 { "No solid colours in this document yet".to_string() } else { format!("Added {n} colour key{}", if n == 1 { "" } else { "s" }) });
            }
        });
        app.ui.keys = st;
        text_styles(app, ui);
        return;
    }
    let q = st.query.trim().to_lowercase();
    let shown: Vec<_> = keys.iter().filter(|k| q.is_empty() || k.name.to_lowercase().contains(&q) || k.color.to_hex().to_lowercase().contains(&q)).collect();
    if shown.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("No matching keys").size(theme::FONT_SM).color(TEXT_FAINT));
    }
    // Keys the selection's fill / stroke follow.
    let sel = app.selected_shapes().first().map(|s| (s.fill_key, s.stroke_key)).unwrap_or((None, None));
    let has_sel = !app.selection.is_empty();
    let mut remove = None;
    for k in shown {
        let (fills, strokes) = app.doc.key_users(k.id);
        let uses = fills.len() + strokes.len();
        let row_h = 30.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
        let open = st.open == Some(k.id);
        let hovered = ui.rect_contains_pointer(rect);
        if hovered || open {
            ui.painter().rect_filled(rect, CornerRadius::same(4), if open { theme::BG_ACTIVE } else { theme::BG_HOVER });
        }
        let sw = egui::Rect::from_center_size(egui::pos2(rect.left() + 15.0, rect.center().y), vec2(18.0, 18.0));
        if k.color.a < 1.0 {
            crate::color::checkerboard(&ui.painter().with_clip_rect(sw), sw, 3.0);
        }
        ui.painter().rect_filled(sw, CornerRadius::same(4), k.color.to_color32(1.0));
        ui.painter().rect_stroke(sw, CornerRadius::same(4), Stroke::new(1.0, Color32::from_white_alpha(40)), StrokeKind::Inside);
        let linked_here = sel.0 == Some(k.id) || sel.1 == Some(k.id);
        // Actions on the right (hover), else the usage count.
        let mut x = rect.right() - 6.0;
        let action = |ui: &mut Ui, glyph: &str, tip: &str, x: &mut f32| -> bool {
            let r = egui::Rect::from_min_max(egui::pos2(*x - 22.0, rect.top() + 4.0), egui::pos2(*x, rect.bottom() - 4.0));
            *x -= 24.0;
            let resp = ui.interact(r, ui.id().with(("key_act", k.id, tip)), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(3), theme::BG_ACTIVE);
            }
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(13.0), if resp.hovered() { theme::TEXT } else { TEXT_DIM });
            widgets::tip(resp, tip, None).clicked()
        };
        if hovered || open {
            if action(ui, icon::TRASH, "Delete key (linked colours stay)", &mut x) {
                remove = Some(k.id);
            }
            if has_sel && action(ui, icon::LINE_SEGMENT, "Apply to the selection's stroke", &mut x) {
                app.apply_color_key(k.id, true);
            }
            if has_sel && action(ui, icon::PAINT_BUCKET, "Apply to the selection's fill", &mut x) {
                app.apply_color_key(k.id, false);
            }
        } else if uses > 0 {
            let t = format!("{uses}");
            ui.painter().text(egui::pos2(x - 4.0, rect.center().y), egui::Align2::RIGHT_CENTER, t, egui::FontId::proportional(theme::FONT_SM), TEXT_FAINT);
            x -= 22.0;
        }
        // Name (editable in place) and hex.
        let name_rect = egui::Rect::from_min_max(egui::pos2(sw.right() + 8.0, rect.top() + 3.0), egui::pos2((x - 64.0).max(sw.right() + 40.0), rect.bottom() - 3.0));
        let mut name = k.name.clone();
        let mut nui = ui.new_child(egui::UiBuilder::new().max_rect(name_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let te = egui::TextEdit::singleline(&mut name)
            .frame(egui::Frame::NONE)
            .desired_width(name_rect.width())
            .font(egui::TextStyle::Body)
            .text_color(theme::TEXT)
            .margin(egui::Margin::symmetric(2, 0));
        let nr = nui.add(te);
        if st.rename == Some(k.id) {
            nr.request_focus();
            st.rename = None;
        }
        if nr.changed() {
            app.checkpoint("key name");
            if let Some(kk) = app.doc.colors.iter_mut().find(|x| x.id == k.id) {
                kk.name = name;
            }
            app.dirty = true;
        }
        let hex_x = name_rect.right() + 6.0;
        ui.painter().text(egui::pos2(hex_x, rect.center().y), egui::Align2::LEFT_CENTER, k.color.with_alpha(1.0).to_hex(), egui::FontId::monospace(10.5), TEXT_DIM);
        if k.spot {
            widgets::tip(ui.interact(egui::Rect::from_center_size(egui::pos2(sw.right() - 1.0, sw.bottom() - 1.0), vec2(10.0, 10.0)), ui.id().with(("spot", k.id)), Sense::hover()), "Spot colour: prints as its own ink", None);
            ui.painter().circle_filled(egui::pos2(sw.right() - 1.0, sw.bottom() - 1.0), 4.5, theme::BG_PANEL);
            ui.painter().text(egui::pos2(sw.right() - 1.0, sw.bottom() - 1.0), egui::Align2::CENTER_CENTER, icon::DROP, egui::FontId::proportional(8.0), ACCENT);
        }
        if linked_here {
            ui.painter().text(egui::pos2(hex_x - 2.0, rect.top() + 6.0), egui::Align2::RIGHT_TOP, icon::LINK, egui::FontId::proportional(9.0), ACCENT);
        }
        // Clicking the swatch or the empty row toggles the colour editor.
        let sw_resp = ui.interact(sw.expand(3.0), ui.id().with(("key_sw", k.id)), Sense::click());
        let sw_resp = widgets::tip(sw_resp, &format!("{} · {uses} use{}", if open { "Close colour editor" } else { "Edit colour" }, if uses == 1 { "" } else { "s" }), None);
        if sw_resp.clicked() || resp.clicked() {
            st.open = if open { None } else { Some(k.id) };
            if !open {
                widgets::request_reveal(ui.ctx(), "paint");
            }
        }
        if open {
            super::design::editor_frame(ui, |ui| {
                let mut c = k.color;
                if crate::color::color_picker(ui, &mut c, egui::Id::new(("key_picker", k.id))) && c != k.color {
                    app.checkpoint("key color");
                    app.doc.set_key_color(k.id, c);
                }
                print_row(app, ui, k);
                ui.label(
                    RichText::new(if uses == 0 { "Not used yet".to_string() } else { format!("{} fill{}, {} stroke{} follow this key", fills.len(), if fills.len() == 1 { "" } else { "s" }, strokes.len(), if strokes.len() == 1 { "" } else { "s" }) })
                        .size(theme::FONT_SM)
                        .color(TEXT_FAINT),
                );
            });
        }
    }
    if let Some(id) = remove {
        app.checkpoint("");
        app.doc.remove_color_key(id);
        if st.open == Some(id) {
            st.open = None;
        }
    }
    ui.add_space(8.0);
    if widgets::tip(widgets::flat_button(ui, Some(icon::PALETTE), "Add colours in use", true), "A key for every solid colour in the document, linked to the shapes that use it", None).clicked() {
        let n = app.keys_from_document();
        app.notify(if n == 0 { "Every colour in use already has a key".to_string() } else { format!("Added {n} colour key{}", if n == 1 { "" } else { "s" }) });
    }
    app.ui.keys = st;
    text_styles(app, ui);
}

/// Paragraph and character styles of the document: rename, apply to the
/// selection, delete. They're created from the Text section.
fn text_styles(app: &mut App, ui: &mut Ui) {
    ui.add_space(14.0);
    ui.label(RichText::new("TEXT STYLES").size(theme::FONT_SM).color(TEXT_FAINT));
    ui.add_space(4.0);
    let styles = app.doc.text_styles.clone();
    if styles.is_empty() {
        ui.label(RichText::new("None yet. Save one from the Text section with +.").size(theme::FONT_SM).color(TEXT_FAINT));
        return;
    }
    let texts: Vec<u64> = app.selection.iter().copied().filter(|id| matches!(app.doc.get(*id).map(|s| &s.kind), Some(crate::document::ShapeKind::Text(_)))).collect();
    let mut remove = None;
    let mut apply = None;
    for st in &styles {
        let uses = app
            .doc
            .all_ids()
            .into_iter()
            .filter(|id| matches!(app.doc.get(*id).map(|s| &s.kind), Some(crate::document::ShapeKind::Text(t)) if t.style_id == Some(st.id) || t.spans.iter().any(|sp| sp.style_id == Some(st.id))))
            .count();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
        let hovered = ui.rect_contains_pointer(rect);
        if hovered {
            ui.painter().rect_filled(rect, CornerRadius::same(4), theme::BG_HOVER);
        }
        let glyph = if st.paragraph { icon::PARAGRAPH } else { icon::TEXT_T };
        ui.painter().text(egui::pos2(rect.left() + 15.0, rect.center().y), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(14.0), TEXT_DIM);
        let mut x = rect.right() - 6.0;
        let action = |ui: &mut Ui, glyph: &str, tip: &str, x: &mut f32| -> bool {
            let r = egui::Rect::from_min_max(egui::pos2(*x - 22.0, rect.top() + 4.0), egui::pos2(*x, rect.bottom() - 4.0));
            *x -= 24.0;
            let resp = ui.interact(r, ui.id().with(("tstyle_act", st.id, tip)), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(3), theme::BG_ACTIVE);
            }
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(13.0), if resp.hovered() { theme::TEXT } else { TEXT_DIM });
            widgets::tip(resp, tip, None).clicked()
        };
        if hovered {
            if action(ui, icon::TRASH, "Delete style (texts keep their look)", &mut x) {
                remove = Some(st.id);
            }
            if st.paragraph && !texts.is_empty() && action(ui, icon::PAINT_BRUSH, "Apply to the selected texts", &mut x) {
                apply = Some(st.id);
            }
        } else if uses > 0 {
            ui.painter().text(egui::pos2(x - 4.0, rect.center().y), egui::Align2::RIGHT_CENTER, format!("{uses}"), egui::FontId::proportional(theme::FONT_SM), TEXT_FAINT);
            x -= 22.0;
        }
        let name_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 30.0, rect.top() + 3.0), egui::pos2(x - 4.0, rect.bottom() - 3.0));
        let mut name = st.name.clone();
        let mut nui = ui.new_child(egui::UiBuilder::new().max_rect(name_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let te = egui::TextEdit::singleline(&mut name).frame(egui::Frame::NONE).desired_width(name_rect.width()).font(egui::TextStyle::Body).text_color(theme::TEXT).margin(egui::Margin::symmetric(2, 0));
        if nui.add(te).changed() {
            app.checkpoint("text style name");
            if let Some(s) = app.doc.text_styles.iter_mut().find(|x| x.id == st.id) {
                s.name = name;
            }
            app.dirty = true;
        }
    }
    if let Some(id) = remove {
        app.delete_text_style(id);
    }
    if let Some(id) = apply {
        app.checkpoint("");
        if let Some(st) = app.doc.text_styles.iter().find(|s| s.id == id).cloned() {
            for tid in texts {
                if let Some(crate::document::ShapeKind::Text(t)) = app.doc.get_mut(tid).map(|s| &mut s.kind) {
                    st.apply_to(t);
                    t.style_id = Some(id);
                }
            }
        }
    }
}

/// Print settings of a key: spot ink, exact CMYK values.
fn print_row(app: &mut App, ui: &mut Ui, k: &crate::document::ColorKey) {
    let mut spot = k.spot;
    let mut exact = k.cmyk.is_some();
    let mut cmyk = k.cmyk.unwrap_or_else(|| crate::print::naive_cmyk(k.color));
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= widgets::tip_long(ui.checkbox(&mut spot, RichText::new("Spot colour").size(theme::FONT_SM)), "Spot colour", None, "PDF export prints it as its own ink, named after the key (e.g. a Pantone name), instead of mixing it from CMYK.").changed();
        changed |= widgets::tip_long(ui.checkbox(&mut exact, RichText::new("Exact CMYK").size(theme::FONT_SM)), "Exact CMYK", None, "Use these ink values in CMYK PDFs instead of converting the colour.").changed();
    });
    if exact {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = ((ui.available_width() - 12.0) / 4.0).max(40.0);
            for (i, l) in ["C", "M", "Y", "K"].iter().enumerate() {
                let mut v = cmyk[i] as f64 * 100.0;
                if widgets::num_field(ui, l, &mut v, widgets::Num::default().range(0.0, 100.0).decimals(0).suffix("%"), w).changed() {
                    cmyk[i] = (v / 100.0).clamp(0.0, 1.0) as f32;
                    changed = true;
                }
            }
        });
    } else {
        let c = crate::print::naive_cmyk(k.color).map(|v| (v * 100.0).round());
        ui.label(RichText::new(format!("CMYK ≈ {} {} {} {} (converted on export)", c[0], c[1], c[2], c[3])).size(theme::FONT_SM).color(TEXT_FAINT));
    }
    if changed {
        app.checkpoint("key print");
        if let Some(kk) = app.doc.colors.iter_mut().find(|x| x.id == k.id) {
            kk.spot = spot;
            kk.cmyk = exact.then_some(cmyk);
        }
        app.dirty = true;
    }
}
