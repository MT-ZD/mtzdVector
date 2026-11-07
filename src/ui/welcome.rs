//! Start screen: document templates and recent documents (with previews).

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Align2, Color32, CornerRadius, Id, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use egui_phosphor::regular as icon;

use super::theme::{self, ACCENT, TEXT_DIM, TEXT_FAINT};
use super::widgets::{self, Num};
use crate::app::App;
use crate::document::{Document, Rgba};
use crate::render::{GeoCache, View};
use crate::units::Unit;

/// (category, name, width, height, unit, dpi)
pub const TEMPLATES: [(&str, &str, f64, f64, Unit, f64); 16] = [
    ("Screen", "Full HD", 1920.0, 1080.0, Unit::Px, 96.0),
    ("Screen", "4K UHD", 3840.0, 2160.0, Unit::Px, 96.0),
    ("Screen", "Desktop web", 1440.0, 900.0, Unit::Px, 96.0),
    ("Screen", "Mobile", 390.0, 844.0, Unit::Px, 96.0),
    ("Screen", "App icon", 1024.0, 1024.0, Unit::Px, 96.0),
    ("Screen", "Icon 24", 24.0, 24.0, Unit::Px, 96.0),
    ("Social", "Square post", 1080.0, 1080.0, Unit::Px, 96.0),
    ("Social", "Story", 1080.0, 1920.0, Unit::Px, 96.0),
    ("Social", "Banner", 1500.0, 500.0, Unit::Px, 96.0),
    ("Social", "Thumbnail", 1280.0, 720.0, Unit::Px, 96.0),
    ("Print", "A4", 210.0, 297.0, Unit::Mm, 300.0),
    ("Print", "A4 landscape", 297.0, 210.0, Unit::Mm, 300.0),
    ("Print", "A5", 148.0, 210.0, Unit::Mm, 300.0),
    ("Print", "A3", 297.0, 420.0, Unit::Mm, 300.0),
    ("Print", "Letter", 8.5, 11.0, Unit::In, 300.0),
    ("Print", "Business card", 85.0, 55.0, Unit::Mm, 300.0),
];

pub struct WelcomeState {
    pub selected: usize,
    pub w: f64,
    pub h: f64,
    pub unit: Unit,
    pub dpi: f64,
    pub transparent: bool,
    recent: Option<Vec<PathBuf>>,
    previews: HashMap<PathBuf, Option<Document>>,
    cache: GeoCache,
}

impl Default for WelcomeState {
    fn default() -> Self {
        let t = TEMPLATES[0];
        WelcomeState { selected: 0, w: t.2, h: t.3, unit: t.4, dpi: t.5, transparent: false, recent: None, previews: HashMap::new(), cache: GeoCache::default() }
    }
}

impl WelcomeState {
    fn pick(&mut self, i: usize) {
        let t = TEMPLATES[i];
        self.selected = i;
        self.w = t.2;
        self.h = t.3;
        self.unit = t.4;
        self.dpi = t.5;
    }
}

impl App {
    /// New document from the start screen's settings.
    pub fn create_from_template(&mut self) {
        let st = &self.ui.welcome;
        let (w, h) = (st.unit.to_px(st.w, st.dpi), st.unit.to_px(st.h, st.dpi));
        let (unit, dpi, transparent) = (st.unit, st.dpi, st.transparent);
        self.new_document();
        self.doc.set_size(w, h);
        self.doc.unit = unit;
        self.doc.dpi = dpi;
        if transparent {
            self.doc.primary_mut().background = Rgba::WHITE.with_alpha(0.0);
        }
        self.ui.welcome_open = false;
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.ui.welcome_open {
        app.ui.welcome.recent = None;
        return;
    }
    if app.ui.welcome.recent.is_none() {
        app.ui.welcome.recent = Some(crate::recent::load());
    }
    let screen = ctx.content_rect();
    // Backdrop that swallows clicks.
    egui::Area::new(Id::new("welcome_backdrop")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(150));
        if resp.clicked() && !app.doc.shapes().is_empty() {
            app.ui.welcome_open = false;
        }
    });
    let size = vec2(940.0f32.min(screen.width() - 40.0), 600.0f32.min(screen.height() - 40.0));
    egui::Area::new(Id::new("welcome")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        egui::Frame::new()
            .fill(theme::BG_PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(8))
            .shadow(ui.visuals().window_shadow)
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                ui.set_width(size.x - 36.0);
                ui.set_height(size.y - 36.0);
                ui.horizontal(|ui| {
                    crate::logo::show(ui, 24.0);
                    ui.add_space(4.0);
                    crate::logo::word_mark(ui, 13.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::tip(widgets::mini_icon(ui, icon::X, TEXT_DIM), "Close (Esc)", None).clicked() {
                            app.ui.welcome_open = false;
                        }
                    });
                });
                ui.add_space(10.0);
                let avail = ui.available_size();
                ui.horizontal_top(|ui| {
                    // The template list scrolls right up to the column
                    // separator, which runs through its scrollbar track.
                    let col = ui.vertical(|ui| {
                        ui.set_width(avail.x * 0.6 + 10.0);
                        ui.set_height(avail.y);
                        templates(app, ui);
                    });
                    let sc = ui.spacing().scroll;
                    let x = col.response.rect.right() - sc.bar_outer_margin - 1.5;
                    ui.painter().vline(x, col.response.rect.y_range(), Stroke::new(1.0, theme::SEPARATOR));
                    ui.add_space(20.0 - ui.spacing().item_spacing.x);
                    ui.vertical(|ui| {
                        ui.set_height(avail.y);
                        recent(app, ui);
                    });
                });
            });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.welcome_open = false;
    }
}

fn templates(app: &mut App, ui: &mut Ui) {
    ui.label(RichText::new("NEW DOCUMENT").size(10.5).color(TEXT_DIM));
    ui.add_space(4.0);
    let mut create = false;
    // The column separator stands in for the scrollbar track.
    ui.spacing_mut().scroll.dormant_background_opacity = 0.0;
    ui.spacing_mut().scroll.active_background_opacity = 0.0;
    egui::ScrollArea::vertical().id_salt("tpl_scroll").auto_shrink([false, true]).max_height(ui.available_height() - 96.0).show(ui, |ui| {
        for cat in ["Screen", "Social", "Print"] {
            ui.label(RichText::new(cat).size(theme::FONT_SM).color(TEXT_FAINT));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for (i, t) in TEMPLATES.iter().enumerate().filter(|(_, t)| t.0 == cat) {
                    let sel = app.ui.welcome.selected == i;
                    let (rect, resp) = ui.allocate_exact_size(vec2(112.0, 96.0), Sense::click());
                    let p = ui.painter();
                    let bg = if sel { theme::ACCENT_DIM } else if resp.hovered() { theme::BG_HOVER } else { theme::BG_INPUT };
                    p.rect_filled(rect, CornerRadius::same(6), bg);
                    if sel {
                        p.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.5, ACCENT), StrokeKind::Inside);
                    }
                    // Proportional page glyph.
                    let k = (40.0 / t.2.max(t.3)) as f32;
                    let pr = egui::Rect::from_center_size(rect.center_top() + vec2(0.0, 32.0), vec2(t.2 as f32 * k, t.3 as f32 * k));
                    p.rect_filled(pr, CornerRadius::same(2), Color32::from_gray(230));
                    p.text(rect.center_bottom() - vec2(0.0, 26.0), Align2::CENTER_CENTER, t.1, egui::FontId::proportional(12.0), theme::TEXT);
                    let dims = format!("{} × {} {}", t.2, t.3, t.4.suffix());
                    p.text(rect.center_bottom() - vec2(0.0, 11.0), Align2::CENTER_CENTER, dims, egui::FontId::proportional(10.5), TEXT_DIM);
                    if resp.clicked() {
                        app.ui.welcome.pick(i);
                    }
                    if resp.double_clicked() {
                        create = true;
                    }
                }
            });
            ui.add_space(8.0);
        }
    });
    ui.add_space(6.0);
    let st = &mut app.ui.welcome;
    ui.horizontal(|ui| {
        // Fields edit raw values in the chosen unit.
        widgets::num_field(ui, "W", &mut st.w, Num::default().range(0.01, 100000.0).decimals(2), 110.0);
        widgets::num_field(ui, "H", &mut st.h, Num::default().range(0.01, 100000.0).decimals(2), 110.0);
        let cur = st.unit;
        widgets::dropdown(ui, "welcome_unit", cur.suffix(), 52.0, |ui| {
            for u in Unit::ALL {
                if ui.selectable_label(u == cur, u.label()).clicked() {
                    // Keep the physical size when switching units.
                    let k = u.per_px(st.dpi) / cur.per_px(st.dpi);
                    st.w *= k;
                    st.h *= k;
                    st.unit = u;
                }
            }
        });
        widgets::num_field(ui, "DPI", &mut st.dpi, Num::default().range(1.0, 2400.0).decimals(0), 92.0);
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut st.transparent, "Transparent background");
        if ui.add(egui::Button::new(RichText::new(format!("{}  Swap", icon::ARROWS_CLOCKWISE)).size(theme::FONT_SM))).clicked() {
            std::mem::swap(&mut st.w, &mut st.h);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Clear of the column separator.
            ui.add_space(14.0);
            if widgets::primary_button(ui, &format!("{}  Create", icon::PLUS)).clicked() {
                create = true;
            }
        });
    });
    if create {
        app.guarded(crate::session::Guarded::Template);
    }
}

fn recent(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("RECENT").size(10.5).color(TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::flat_button(ui, Some(icon::FOLDER_OPEN), "Open…", true).clicked() {
                app.guarded(crate::session::Guarded::Open(None));
            }
        });
    });
    ui.add_space(4.0);
    let list = app.ui.welcome.recent.clone().unwrap_or_default();
    if list.is_empty() {
        widgets::empty_state(ui, icon::CLOCK_COUNTER_CLOCKWISE, "No recent documents", "Documents you open or save\nshow up here.");
        return;
    }
    let mut open = None;
    let mut forget = None;
    egui::ScrollArea::vertical().id_salt("recent_scroll").show(ui, |ui| {
        for p in &list {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(6), theme::BG_HOVER);
            }
            let thumb = egui::Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(72.0, 52.0));
            preview(app, ui, p, thumb);
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let dir = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
            let painter = ui.painter();
            painter.text(thumb.right_top() + vec2(10.0, 8.0), Align2::LEFT_TOP, name, egui::FontId::proportional(13.0), theme::TEXT);
            painter.text(thumb.right_top() + vec2(10.0, 27.0), Align2::LEFT_TOP, ellipsize(&dir, 44), egui::FontId::proportional(10.5), TEXT_DIM);
            if let Some(age) = modified_ago(p) {
                painter.text(thumb.right_top() + vec2(10.0, 42.0), Align2::LEFT_TOP, age, egui::FontId::proportional(10.5), TEXT_FAINT);
            }
            let resp = resp.on_hover_text(p.display().to_string());
            if resp.clicked() {
                open = Some(p.clone());
            }
            resp.context_menu(|ui| {
                if ui.button("Remove from list").clicked() {
                    forget = Some(p.clone());
                    ui.close();
                }
            });
        }
    });
    if let Some(p) = forget {
        crate::recent::remove(&p);
        app.ui.welcome.recent = None;
    }
    if let Some(p) = open {
        app.guarded(crate::session::Guarded::Open(Some(p)));
    }
}

fn ellipsize(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= n { s.to_string() } else { format!("…{}", c[c.len() - n..].iter().collect::<String>()) }
}

fn modified_ago(p: &PathBuf) -> Option<String> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    let secs = std::time::SystemTime::now().duration_since(m).ok()?.as_secs();
    Some(match secs {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    })
}

/// Thumbnail of a native document (icon for other formats).
fn preview(app: &mut App, ui: &mut Ui, path: &PathBuf, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), theme::BG_INPUT);
    let st = &mut app.ui.welcome;
    let doc = st.previews.entry(path.clone()).or_insert_with(|| {
        matches!(crate::io::format_of(path), Some(crate::io::Format::Native)).then(|| crate::io::load_native(path).ok()).flatten()
    });
    let Some(doc) = doc else {
        let glyph = match crate::io::format_of(path) {
            Some(crate::io::Format::Pdf) => icon::FILE_PDF,
            Some(crate::io::Format::Svg) => icon::FILE_SVG,
            _ => icon::FILE,
        };
        painter.text(rect.center(), Align2::CENTER_CENTER, glyph, egui::FontId::proportional(22.0), TEXT_DIM);
        return;
    };
    let inner = rect.shrink(4.0);
    // Thumbnail of the first artboard.
    let ab = doc.primary().clone();
    let zoom = (inner.width() / ab.w as f32).min(inner.height() / ab.h as f32);
    let size = vec2(ab.w as f32, ab.h as f32) * zoom;
    let origin = inner.center() - size / 2.0;
    let page = egui::Rect::from_min_size(origin, size);
    painter.rect_filled(page, 0.0, ab.background.with_alpha(1.0).to_color32(1.0));
    let view = View { zoom, origin: origin.to_vec2() - vec2(ab.x as f32, ab.y as f32) * zoom };
    st.cache.paint_document(&painter.with_clip_rect(page), doc, &mut app.fonts, &view, None);
}
