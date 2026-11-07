//! Text section of the Design panel and the font dropdown popover with
//! live previews.

use egui::{Color32, CornerRadius, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use egui_phosphor::regular as icon;

use super::{design, edit_style};
use crate::ui::widgets::WellPaint;
use crate::app::{App, FontSource};
use crate::document::{ShapeKind, TextAlign, TextData};
use crate::fonts::{self, LoadState};
use crate::render;
use crate::ui::theme::{self, TEXT, TEXT_DIM};
use crate::ui::widgets::{self, Num};

fn weight_name(w: u16) -> &'static str {
    match w {
        100 => "Thin",
        200 => "Extra Light",
        300 => "Light",
        400 => "Regular",
        500 => "Medium",
        600 => "Semi Bold",
        700 => "Bold",
        800 => "Extra Bold",
        900 => "Black",
        _ => "Custom",
    }
}

/// Font family button showing a live preview; opens a searchable popover.
/// Returns the chosen family.
pub fn font_dropdown(app: &mut App, ui: &mut Ui, id: &str, family: &str, width: f32) -> Option<String> {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
    let popup_id = widgets::popup_id(id);
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let p = ui.painter();
    p.rect_filled(rect, theme::radius(), theme::BG_INPUT);
    let stroke = if open { Stroke::new(1.0, theme::ACCENT) } else if resp.hovered() { Stroke::new(1.0, theme::BORDER) } else { Stroke::NONE };
    p.rect_stroke(rect, theme::radius(), stroke, StrokeKind::Inside);
    let mut budget = 2;
    let google = !app.fonts.is_installed(family) && app.fonts.google_family(family).is_some();
    let clip = rect.shrink2(vec2(6.0, 2.0)).with_max_x(rect.right() - 26.0);
    match app.fonts.preview(family, google, &mut budget) {
        Some(m) => {
            let scale = 1.0;
            let off = rect.left_top() + vec2(10.0, (rect.height() - fonts::PREVIEW_SIZE as f32 * scale) / 2.0);
            render::paint_raw_mesh(&ui.painter().with_clip_rect(clip), &m.vertices, &m.indices, off, scale, TEXT);
        }
        None => {
            ui.painter().with_clip_rect(clip).text(rect.left_center() + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, family, egui::FontId::proportional(15.0), TEXT);
        }
    }
    ui.painter().text(rect.right_center() - vec2(10.0, 0.0), egui::Align2::RIGHT_CENTER, icon::CARET_DOWN, egui::FontId::proportional(12.0), TEXT_DIM);
    let resp = widgets::tip(resp, &format!("Font: {family}"), None);
    if resp.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup_id);
    }
    if std::mem::take(&mut app.font_picker.open) {
        egui::Popup::open_id(ui.ctx(), popup_id);
    }
    let mut chosen = None;
    egui::Popup::from_response(&resp)
        .id(popup_id)
        .open_memory(None)
        .kind(egui::PopupKind::Popup)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(width.max(300.0))
        .gap(4.0)
        .show(|ui| {
            ui.set_width(width.max(300.0));
            chosen = font_list(app, ui, family);
            if chosen.is_some() {
                ui.close();
            }
        });
    chosen
}

fn font_list(app: &mut App, ui: &mut Ui, current: &str) -> Option<String> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(TEXT_DIM));
        let r = widgets::text_field(ui, &mut app.font_picker.search, "Search fonts", f32::INFINITY);
        if !r.has_focus() && app.font_picker.search.is_empty() && ui.memory(|m| m.focused().is_none()) {
            r.request_focus();
        }
    });
    ui.horizontal(|ui| {
        widgets::segmented(ui, &mut app.font_picker.source, &[(FontSource::All, "All"), (FontSource::Installed, "Installed"), (FontSource::Google, "Google")]);
        let cats = ["All", "Sans Serif", "Serif", "Display", "Handwriting", "Monospace"];
        let cur = app.font_picker.category.clone();
        widgets::dropdown(ui, "font_cat", cur, 100.0, |ui| {
            for c in cats {
                ui.selectable_value(&mut app.font_picker.category, c.to_string(), c);
            }
        });
    });
    match app.fonts.google_state.clone() {
        LoadState::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading Google Fonts catalogue…").size(theme::FONT_SM).color(TEXT_DIM));
            });
        }
        LoadState::Failed(e) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Google Fonts unavailable: {e}")).color(theme::DANGER).size(theme::FONT_SM));
                if ui.small_button("Retry").clicked() {
                    app.fonts.google_state = LoadState::Idle;
                    app.fonts.load_google_list(true);
                }
            });
        }
        _ => {}
    }
    let q = app.font_picker.search.to_lowercase();
    let cat = app.font_picker.category.clone();
    let src = app.font_picker.source;
    let mut list: Vec<(String, bool, String)> = Vec::new();
    if src != FontSource::Google && cat == "All" {
        for f in &app.fonts.families {
            if q.is_empty() || f.to_lowercase().contains(&q) {
                list.push((f.clone(), false, String::new()));
            }
        }
    }
    if src != FontSource::Installed {
        for g in &app.fonts.google {
            if (q.is_empty() || g.family.to_lowercase().contains(&q)) && (cat == "All" || g.category == cat) {
                let installed = app.fonts.is_installed(&g.family);
                if src == FontSource::All && installed && cat == "All" {
                    continue;
                }
                list.push((g.family.clone(), !installed, g.category.clone()));
            }
        }
    }
    ui.label(RichText::new(format!("{} fonts", list.len())).size(10.5).color(TEXT_DIM));
    let row_h = 34.0;
    let mut budget = 6u32;
    egui::ScrollArea::vertical().id_salt("font_list").max_height(360.0).auto_shrink([false, true]).show_rows(ui, row_h, list.len(), |ui, range| {
        for i in range {
            let (family, google, category) = &list[i];
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
            let sel = family == current;
            let bg = if sel {
                theme::ACCENT_DIM
            } else if resp.hovered() {
                theme::BG_HOVER
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
            let painter = ui.painter().with_clip_rect(rect.shrink2(vec2(6.0, 0.0)));
            match app.fonts.preview(family, *google, &mut budget) {
                Some(m) => render::paint_raw_mesh(&painter, &m.vertices, &m.indices, rect.left_top() + vec2(8.0, 2.0), 0.8, TEXT),
                None => {
                    painter.text(rect.left_top() + vec2(8.0, 4.0), egui::Align2::LEFT_TOP, family, egui::FontId::proportional(13.0), TEXT_DIM);
                }
            }
            let meta = if *google { format!("{}  {}", icon::GOOGLE_LOGO, category) } else { "Installed".to_string() };
            painter.text(rect.right_bottom() + vec2(-8.0, -3.0), egui::Align2::RIGHT_BOTTOM, format!("{family} · {meta}"), egui::FontId::proportional(9.5), TEXT_DIM);
            if resp.clicked() {
                chosen = Some(family.clone());
            }
        }
    });
    if budget == 0 {
        ui.ctx().request_repaint();
    }
    chosen
}

/// Languages offered for hyphenation and spell checking.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en-us", "English (US)"),
    ("en-gb", "English (UK)"),
    ("pl", "Polish"),
    ("de-1996", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("nb", "Norwegian"),
    ("fi", "Finnish"),
    ("cs", "Czech"),
    ("sk", "Slovak"),
    ("hu", "Hungarian"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("tr", "Turkish"),
    ("el-monoton", "Greek"),
];

pub fn language_name(code: &str) -> &'static str {
    LANGUAGES.iter().find(|l| l.0 == code).map(|l| l.1).unwrap_or("Other")
}

/// Character attributes: font, weight, size, tracking, case, decoration,
/// OpenType features and variable axes. Edits `td`'s object-level fields.
fn char_props(app: &mut App, ui: &mut Ui, td: &mut TextData, id: &str) -> Option<String> {
    let w = ui.available_width();
    let chosen = font_dropdown(app, ui, id, &td.family.clone(), w);
    if !app.fonts.is_installed(&td.family) {
        let google = app.fonts.google_family(&td.family).is_some();
        ui.label(
            RichText::new(format!("{} Not installed{}", icon::DOWNLOAD_SIMPLE, if google { " — downloading from Google Fonts" } else { " — using fallback" }))
                .size(theme::FONT_SM)
                .color(theme::ACCENT),
        );
    }
    let weights = app.fonts.weights_for(&td.family);
    ui.horizontal(|ui| {
        let current = format!("{}{}", weight_name(td.weight), if td.italic { " Italic" } else { "" });
        let ww = (ui.available_width() - 96.0).max(90.0);
        widgets::dropdown(ui, ("weight", id), current, ww, |ui| {
            for (wt, it) in &weights {
                let label = format!("{}{}", weight_name(*wt), if *it { " Italic" } else { "" });
                if ui.selectable_label(td.weight == *wt && td.italic == *it, label).clicked() {
                    td.weight = *wt;
                    td.italic = *it;
                }
            }
        });
        widgets::num_field(ui, "", &mut td.size, Num::raw_px().range(1.0, 2000.0).speed(0.5), 88.0);
    });
    let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
    ui.horizontal(|ui| {
        widgets::tip(widgets::num_field(ui, icon::ARROWS_LEFT_RIGHT, &mut td.letter_spacing, Num::raw_px().range(-50.0, 200.0).speed(0.1), fw), "Letter spacing", None);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            widgets::tip(widgets::icon_toggle(ui, &mut td.strike, icon::TEXT_STRIKETHROUGH), "Strikethrough", None);
            widgets::tip(widgets::icon_toggle(ui, &mut td.underline, icon::TEXT_UNDERLINE), "Underline", None);
        });
    });
    ui.horizontal(|ui| {
        use crate::document::TextCase;
        ui.label(RichText::new("Case").size(theme::FONT_SM).color(TEXT_DIM));
        ui.add_space(6.0);
        let w = ui.available_width();
        let r = ui.scope(|ui| {
            widgets::segmented_width(
                ui,
                &mut td.case,
                &[(TextCase::AsTyped, icon::MINUS), (TextCase::Upper, "AB"), (TextCase::Lower, "ab"), (TextCase::Title, "Ab"), (TextCase::SmallCaps, "Aʙ")],
                w,
            )
        });
        widgets::tip(r.response, "Letter case: as typed, UPPERCASE, lowercase, Title Case, Small caps", None);
    });
    opentype_props(app, ui, td, id);
    chosen
}

/// OpenType features the font offers, and its variable axes.
fn opentype_props(app: &mut App, ui: &mut Ui, td: &mut TextData, id: &str) {
    let offered = app.fonts.font_features(&td.family, td.weight, td.italic);
    let axes = app.fonts.font_axes(&td.family, td.weight, td.italic);
    let feats: Vec<(&str, &str, bool)> = crate::text::FEATURES.iter().copied().filter(|(t, _, _)| offered.iter().any(|o| o == t)).collect();
    let sets: Vec<String> = offered.iter().filter(|t| t.starts_with("ss") && t[2..].parse::<u32>().is_ok()).cloned().collect();
    if feats.is_empty() && sets.is_empty() && axes.is_empty() {
        return;
    }
    let open_id = egui::Id::new(("opentype_open", id));
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    let changed = td.features.len();
    ui.horizontal(|ui| {
        let r = widgets::mini_icon(ui, if open { icon::CARET_DOWN } else { icon::CARET_RIGHT }, TEXT_DIM);
        let l = ui.add(egui::Label::new(RichText::new("OpenType").size(theme::FONT_SM).color(TEXT_DIM)).sense(Sense::click()));
        if r.clicked() || l.clicked() {
            open = !open;
        }
        if changed > 0 || !td.variations.is_empty() {
            ui.label(RichText::new(format!("· {} set", changed + td.variations.len())).size(theme::FONT_SM).color(theme::ACCENT));
        }
    });
    ui.data_mut(|d| d.insert_temp(open_id, open));
    if !open {
        return;
    }
    let state = |td: &TextData, tag: &str, default: bool| td.features.iter().find(|f| f.tag == tag).map(|f| f.value > 0).unwrap_or(default);
    let set = |td: &mut TextData, tag: &str, on: bool, default: bool| {
        td.features.retain(|f| f.tag != tag);
        if on != default {
            td.features.push(crate::document::FontFeature { tag: tag.into(), value: on as u32 });
        }
    };
    egui::Grid::new(("ot_grid", id)).num_columns(2).spacing(vec2(8.0, 2.0)).show(ui, |ui| {
        for (i, (tag, name, default)) in feats.iter().enumerate() {
            let mut on = state(td, tag, *default);
            if ui.checkbox(&mut on, RichText::new(*name).size(theme::FONT_SM)).on_hover_text(format!("OpenType feature “{tag}”")).changed() {
                set(td, tag, on, *default);
            }
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    if !sets.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Stylistic sets").size(theme::FONT_SM).color(TEXT_DIM));
            for t in &sets {
                let mut on = state(td, t, false);
                if ui.toggle_value(&mut on, RichText::new(&t[2..]).size(theme::FONT_SM)).on_hover_text(format!("Stylistic set {}", &t[2..])).changed() {
                    set(td, t, on, false);
                }
            }
        });
    }
    for ax in &axes {
        let cur = td.variations.iter().find(|v| v.tag == ax.tag).map(|v| v.value).unwrap_or(if ax.tag == "wght" { td.weight as f32 } else { ax.default });
        let mut v = cur;
        if widgets::slider_row(ui, &ax.name, &mut v, ax.min..=ax.max, "") && v != cur {
            if ax.tag == "wght" {
                td.weight = v.round().clamp(1.0, 1000.0) as u16;
                td.variations.retain(|x| x.tag != "wght");
            } else {
                td.variations.retain(|x| x.tag != ax.tag);
                if (v - ax.default).abs() > 1e-3 {
                    td.variations.push(crate::document::FontAxis { tag: ax.tag.clone(), value: v });
                }
            }
        }
    }
}

/// Paragraph attributes: line height, alignment, spacing, frame layout.
fn para_props(app: &mut App, ui: &mut Ui, td: &mut TextData) {
    let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
    ui.horizontal(|ui| {
        widgets::tip(widgets::num_field(ui, icon::ARROWS_DOWN_UP, &mut td.line_height, Num::default().range(0.5, 4.0).speed(0.01).suffix("×"), fw), "Line height", None);
        widgets::tip(widgets::num_field(ui, icon::PARAGRAPH, &mut td.paragraph_spacing, Num::raw_px().range(0.0, 1000.0).speed(0.2), fw), "Space after paragraphs", None);
    });
    ui.horizontal(|ui| {
        widgets::segmented(
            ui,
            &mut td.align,
            &[(TextAlign::Left, icon::TEXT_ALIGN_LEFT), (TextAlign::Center, icon::TEXT_ALIGN_CENTER), (TextAlign::Right, icon::TEXT_ALIGN_RIGHT), (TextAlign::Justify, icon::TEXT_ALIGN_JUSTIFY)],
        );
        if td.path.is_some() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::tip(widgets::num_field(ui, icon::PATH, &mut td.path_offset, Num::px().range(-100000.0, 100000.0).speed(0.5), fw), "Offset along the path", None);
            });
        }
    });
    if td.frame.is_none() {
        return;
    }
    ui.horizontal(|ui| {
        let mut cols = td.columns.max(1) as f64;
        if widgets::tip(widgets::num_field(ui, icon::COLUMNS, &mut cols, Num::default().range(1.0, 12.0).speed(0.05).decimals(0), fw), "Columns", None).changed() {
            td.columns = (cols.round() as u32).clamp(1, 12);
        }
        let enabled = td.columns > 1;
        ui.add_enabled_ui(enabled, |ui| {
            widgets::tip(widgets::num_field(ui, icon::ARROWS_OUT_LINE_HORIZONTAL, &mut td.column_gap, Num::raw_px().range(0.0, 1000.0).speed(0.2), fw), "Gap between columns", None);
        });
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut td.hyphenate, RichText::new("Hyphenate").size(theme::FONT_SM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let code = td.lang.clone().unwrap_or_else(|| app.settings.language.clone());
            widgets::dropdown(ui, "text_lang", language_name(&code), 120.0, |ui| {
                for (c, name) in LANGUAGES {
                    if ui.selectable_label(code == *c, *name).clicked() {
                        td.lang = (*c != app.settings.language).then(|| c.to_string());
                    }
                }
            });
            ui.label(RichText::new(icon::TRANSLATE).color(TEXT_DIM)).on_hover_text("Language (hyphenation and spelling)");
        });
    });
}

/// Paragraph style row: pick, create from this text, redefine, detach.
fn paragraph_style_row(app: &mut App, ui: &mut Ui, td: &mut TextData) {
    let styles: Vec<(u64, String)> = app.doc.text_styles.iter().filter(|s| s.paragraph).map(|s| (s.id, s.name.clone())).collect();
    let current = td.style_id.and_then(|id| styles.iter().find(|s| s.0 == id)).map(|s| s.1.clone());
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::PARAGRAPH).color(TEXT_DIM)).on_hover_text("Paragraph style");
        let w = (ui.available_width() - 56.0).max(80.0);
        widgets::dropdown(ui, "para_style", current.clone().unwrap_or_else(|| "No style".into()), w, |ui| {
            if ui.selectable_label(current.is_none(), "No style").clicked() {
                td.style_id = None;
            }
            for (sid, name) in &styles {
                if ui.selectable_label(td.style_id == Some(*sid), name).clicked() {
                    if let Some(st) = app.doc.text_styles.iter().find(|s| s.id == *sid) {
                        st.apply_to(td);
                        td.style_id = Some(*sid);
                    }
                }
            }
        });
        ui.spacing_mut().item_spacing.x = 2.0;
        match td.style_id {
            Some(sid) => {
                if widgets::tip(widgets::mini_icon(ui, icon::ARROWS_CLOCKWISE, TEXT_DIM), "Redefine the style from this text", None).clicked() {
                    app.redefine_text_style(sid, td);
                }
                if widgets::tip(widgets::mini_icon(ui, icon::LINK_BREAK, TEXT_DIM), "Detach from the style", None).clicked() {
                    td.style_id = None;
                }
            }
            None => {
                if widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), "New paragraph style from this text", None).clicked() {
                    let sid = app.add_text_style(true, td, None);
                    td.style_id = Some(sid);
                }
            }
        }
    });
}

/// Character style row for a selected range.
fn char_style_row(app: &mut App, ui: &mut Ui, td: &mut TextData, range: std::ops::Range<usize>) {
    let styles: Vec<(u64, String)> = app.doc.text_styles.iter().filter(|s| !s.paragraph).map(|s| (s.id, s.name.clone())).collect();
    let at = td.spans.iter().find(|sp| sp.start <= range.start && range.start < sp.end).and_then(|sp| sp.style_id);
    let current = at.and_then(|id| styles.iter().find(|s| s.0 == id)).map(|s| s.1.clone());
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::TEXT_T).color(TEXT_DIM)).on_hover_text("Character style");
        let w = (ui.available_width() - 30.0).max(80.0);
        widgets::dropdown(ui, "char_style", current.clone().unwrap_or_else(|| "No character style".into()), w, |ui| {
            if ui.selectable_label(current.is_none(), "None (clear)").clicked() {
                td.apply(range.clone(), |c, sid| {
                    *c = crate::document::CharStyle::default();
                    *sid = None;
                });
            }
            for (sid, name) in &styles {
                if ui.selectable_label(at == Some(*sid), name).clicked() {
                    if let Some(st) = app.doc.text_styles.iter().find(|s| s.id == *sid).cloned() {
                        td.apply(range.clone(), |c, id| {
                            *c = st.style.clone();
                            *id = Some(st.id);
                        });
                    }
                }
            }
        });
        if widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), "New character style from the selection", None).clicked() {
            let sid = app.add_text_style(false, td, Some(range.start));
            if let Some(st) = app.doc.text_styles.iter().find(|s| s.id == sid).cloned() {
                td.apply(range.clone(), |c, id| {
                    *c = st.style.clone();
                    *id = Some(sid);
                });
            }
        }
    });
}

/// Defaults for new text (Text tools with no text selected).
pub fn text_defaults(app: &mut App, ui: &mut Ui) {
    let mut td = app.default_text.clone();
    let chosen = char_props(app, ui, &mut td, "font_default");
    para_props(app, ui, &mut td);
    if let Some(f) = chosen {
        app.apply_font(&f);
    } else {
        app.default_text = td;
    }
}

/// The edited text's selected range, if any.
fn selected_range(app: &App) -> Option<std::ops::Range<usize>> {
    let te = app.text_edit.as_ref()?;
    (te.anchor != te.cursor && app.selection.first() == Some(&te.id)).then(|| te.anchor.min(te.cursor)..te.anchor.max(te.cursor))
}

/// Text section for a selected text object. While editing with a range
/// selected, character attributes apply to the range.
pub fn text_section(app: &mut App, ui: &mut Ui) {
    let has_text = app.selection.first().and_then(|id| app.doc.get(*id)).map(|s| matches!(s.kind, ShapeKind::Text(_))).unwrap_or(false);
    if !has_text {
        return;
    }
    let range = selected_range(app);
    let mut chosen = None;
    let mut convert = false;
    let shape_fill = app.selection.first().and_then(|id| app.doc.get(*id)).and_then(|s| s.fill.as_ref()).and_then(|f| f.solid()).unwrap_or(crate::document::Rgba::BLACK);
    edit_style(app, ui, |app, ui, p| {
        let ShapeKind::Text(td) = &mut p.kind else { return };
        widgets::section_icon(ui, "text", icon::TEXT_AA, "Text", |_| {}, |ui| {
            if let Some(head) = td.flow.as_ref().map(|f| f.head) {
                // A continuation frame shows the story of the thread's first frame.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Text attributes belong to the first frame").size(theme::FONT_SM).color(TEXT_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::flat_button(ui, None, "Select", true).clicked() {
                            app.pending_select = Some(head);
                        }
                    });
                });
                if let Some(f) = &mut td.frame {
                    let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
                    ui.horizontal(|ui| {
                        widgets::num_field(ui, "W", &mut f[0], Num::px().range(1.0, 100000.0), fw);
                        widgets::num_field(ui, "H", &mut f[1], Num::px().range(1.0, 100000.0), fw);
                    });
                }
                thread_row(app, ui, td);
                return;
            }
            paragraph_style_row(app, ui, td);
            match range.clone() {
                Some(r) => {
                    // Character attributes of the selection.
                    let before = td.style_at(r.start);
                    let mut view = td.clone();
                    view.family = before.family.clone();
                    view.weight = before.weight;
                    view.italic = before.italic;
                    view.size = before.size;
                    view.letter_spacing = before.letter_spacing;
                    view.underline = before.underline;
                    view.strike = before.strike;
                    view.case = before.case;
                    view.features = before.features.clone();
                    view.variations = before.variations.clone();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} Selection · {} characters", icon::TEXT_T, td.content[r.clone()].chars().count())).size(theme::FONT_SM).color(theme::ACCENT));
                    });
                    char_style_row(app, ui, td, r.clone());
                    let picked = char_props(app, ui, &mut view, "font_sel");
                    let mut color = before.fill.unwrap_or(shape_fill);
                    let label = format!("#{}", color.to_hex());
                    let open = design::paint_row(app, ui, "text_range", WellPaint::Solid(color), &label, |_| {});
                    let mut recolor = false;
                    if open {
                        design::editor_frame(ui, |ui| recolor = design::picker(app, ui, &mut color, egui::Id::new("text_range_color")));
                    }
                    let mut after = view.base_style();
                    after.fill = if recolor { Some(color) } else { before.fill };
                    if let Some(f) = picked {
                        after.family = f.clone();
                        app.fonts.ensure_available(&f, after.weight, after.italic);
                    }
                    let mut before_cmp = before.clone();
                    before_cmp.fill = before.fill;
                    let d = crate::document::CharStyle::diff(&before_cmp, &after);
                    if !d.is_empty() {
                        td.apply(r.clone(), |c, sid| {
                            c.merge(&d);
                            *sid = None;
                        });
                    }
                }
                None => {
                    let before = td.base_style();
                    chosen = char_props(app, ui, td, "font_sel");
                    // Object-wide edits win over range overrides of the same attribute.
                    let mask = crate::document::CharStyle::diff(&before, &td.base_style());
                    if !mask.is_empty() {
                        td.clear_overrides(&mask);
                    }
                }
            }
            para_props(app, ui, td);
            if td.path.is_none() {
                ui.horizontal(|ui| {
                    let mut framed = td.frame.is_some();
                    if widgets::segmented_width(ui, &mut framed, &[(false, "Artistic"), (true, "Frame")], ui.available_width()) {
                        convert = true;
                    }
                });
            }
            if let Some(f) = &mut td.frame {
                let fw = ((ui.available_width() - 8.0) / 2.0).max(60.0);
                ui.horizontal(|ui| {
                    widgets::num_field(ui, "W", &mut f[0], Num::px().range(1.0, 100000.0), fw);
                    widgets::num_field(ui, "H", &mut f[1], Num::px().range(1.0, 100000.0), fw);
                });
            }
            thread_row(app, ui, td);
        });
        if td.flow.is_some() {
            return;
        }
        app.default_text = TextData { content: String::new(), frame: None, spans: Vec::new(), next: None, flow: None, exclusions: None, ..td.clone() };
    });
    if let Some(f) = chosen {
        app.apply_font(&f);
    }
    if convert {
        app.convert_text_type();
    }
}

/// Threads: where this frame's overflow continues.
fn thread_row(app: &mut App, ui: &mut Ui, td: &mut TextData) {
    if td.frame.is_none() {
        return;
    }
    let follows = td.flow.as_ref().and_then(|f| app.doc.get(f.head)).map(|s| s.name.clone());
    let next = td.next.and_then(|n| app.doc.get(n)).map(|s| s.name.clone());
    if follows.is_none() && next.is_none() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::FLOW_ARROW).color(TEXT_DIM));
        let mut text = String::new();
        if let Some(h) = &follows {
            text.push_str(&format!("Continues “{h}”"));
        }
        if let Some(n) = &next {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            text.push_str(&format!("flows into “{n}”"));
        }
        ui.add(egui::Label::new(RichText::new(text).size(theme::FONT_SM).color(TEXT_DIM)).truncate());
        if next.is_some() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::tip(widgets::mini_icon(ui, icon::LINK_BREAK, TEXT_DIM), "Unlink the next frame", None).clicked() {
                    td.next = None;
                }
            });
        }
    });
}

impl App {
    pub fn apply_font(&mut self, family: &str) {
        // Download the regular style for Google fonts that are not installed yet.
        if !self.fonts.is_installed(family) {
            if let Some(g) = self.fonts.google_family(family).cloned() {
                if let Some(v) = fonts::nearest_variant(&g.variants, 400, false) {
                    self.fonts.request_google_variant(family, &v);
                }
            }
        }
        self.default_text.family = family.to_string();
        let texts: Vec<u64> = self.selection.iter().copied().filter(|id| matches!(self.doc.get(*id).map(|s| &s.kind), Some(ShapeKind::Text(_)))).collect();
        if texts.is_empty() {
            return;
        }
        self.checkpoint("");
        let weights = self.fonts.weights_for(family);
        for id in texts {
            if let Some(s) = self.doc.get_mut(id) {
                if let ShapeKind::Text(t) = &mut s.kind {
                    t.family = family.to_string();
                    if !weights.contains(&(t.weight, t.italic)) {
                        if let Some((w, i)) = weights.iter().min_by_key(|(w, i)| ((*i != t.italic) as i32) * 1000 + (*w as i32 - t.weight as i32).abs()) {
                            t.weight = *w;
                            t.italic = *i;
                        }
                    }
                }
            }
        }
    }
}
