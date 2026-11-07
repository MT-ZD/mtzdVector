//! Design tokens and egui style setup. Dark only, dense "studio" look in the
//! spirit of Affinity Designer: flat panels, crisp 1-px separators, one accent.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, vec2};

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

pub const BG_APP: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);
pub const BG_PANEL: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x2B);
pub const BG_HEADER: Color32 = Color32::from_rgb(0x23, 0x23, 0x23);
pub const BG_INPUT: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
pub const SEPARATOR: Color32 = Color32::from_rgb(0x16, 0x16, 0x16);
pub const BG_HOVER: Color32 = Color32::from_rgb(0x38, 0x38, 0x38);
pub const BG_ACTIVE: Color32 = Color32::from_rgb(0x42, 0x42, 0x42);
pub const BORDER: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
pub const ACCENT: Color32 = Color32::from_rgb(0x3D, 0x8B, 0xFD);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x24, 0x4F, 0x8C);
/// Main components and instances (layer icons, names, canvas outline).
pub const COMPONENT: Color32 = Color32::from_rgb(0xA8, 0x85, 0xFF);
pub const TEXT: Color32 = Color32::from_rgb(0xE4, 0xE4, 0xE4);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8C, 0x8C, 0x8C);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x5E, 0x5E, 0x5E);
pub const DANGER: Color32 = Color32::from_rgb(0xF0, 0x5A, 0x5A);
/// Canvas pasteboard around the artboard.
pub const CANVAS: Color32 = BG_APP;

// ---------------------------------------------------------------------------
// Metrics (4-px scale)
// ---------------------------------------------------------------------------

pub const SP1: f32 = 4.0;
pub const RADIUS: u8 = 4;
pub const ROW_H: f32 = 24.0;
pub const FONT_SM: f32 = 11.0;
pub const FONT_MD: f32 = 12.0;
pub const FONT_LG: f32 = 13.0;
pub const ICON: f32 = 16.0;

pub const MENUBAR_H: f32 = 32.0;
pub const TOOLBAR_H: f32 = 36.0;
pub const STATUSBAR_H: f32 = 24.0;
pub const TOOLS_W: f32 = 40.0;
pub const RULER: f32 = 18.0;
pub const TAB_H: f32 = 26.0;

pub fn radius() -> CornerRadius {
    CornerRadius::same(RADIUS)
}

pub fn setup(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // Prefer a highly readable system UI font if available.
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    for name in ["Inter", "Inter Variable", "Noto Sans", "Cantarell", "DejaVu Sans"] {
        let q = fontdb::Query { families: &[fontdb::Family::Name(name)], weight: fontdb::Weight::NORMAL, ..Default::default() };
        if let Some(id) = db.query(&q) {
            if let Some(data) = db.with_face_data(id, |d, _| d.to_vec()) {
                fonts.font_data.insert("ui".into(), egui::FontData::from_owned(data).into());
                fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "ui".into());
                break;
            }
        }
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(FONT_LG, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(FONT_MD, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(FONT_MD, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(FONT_SM, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(FONT_MD, FontFamily::Monospace)),
        ]
        .into();
        let sp = &mut style.spacing;
        sp.item_spacing = vec2(SP1 + 2.0, SP1);
        sp.button_padding = vec2(6.0, 2.0);
        sp.interact_size = vec2(ROW_H, ROW_H);
        sp.slider_width = 110.0;
        sp.combo_width = 110.0;
        sp.window_margin = Margin::same(10);
        sp.menu_margin = Margin::same(4);
        sp.menu_spacing = 2.0;
        sp.indent = 12.0;
        sp.icon_width = 13.0;
        sp.icon_width_inner = 7.0;
        sp.scroll = egui::style::ScrollStyle::thin();

        style.interaction.tooltip_delay = 0.45;
        style.interaction.show_tooltips_only_when_still = true;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG_PANEL;
        v.window_fill = BG_PANEL;
        v.extreme_bg_color = BG_INPUT;
        v.text_edit_bg_color = Some(BG_INPUT);
        v.faint_bg_color = Color32::from_rgb(0x30, 0x30, 0x30);
        v.code_bg_color = BG_INPUT;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(6);
        v.menu_corner_radius = CornerRadius::same(5);
        v.window_shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(110) };
        v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(110) };
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.override_text_color = None;
        v.text_cursor.stroke = Stroke::new(1.5, ACCENT);
        v.striped = false;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        let r = radius();
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = BG_PANEL;
        w.noninteractive.weak_bg_fill = BG_PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, SEPARATOR);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = r;
        w.inactive.bg_fill = BG_INPUT;
        w.inactive.weak_bg_fill = BG_HOVER;
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = BG_HOVER;
        w.hovered.weak_bg_fill = BG_ACTIVE;
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(0x5A));
        w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = ACCENT_DIM;
        w.active.weak_bg_fill = ACCENT_DIM;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;
        w.open.bg_fill = BG_HOVER;
        w.open.weak_bg_fill = BG_ACTIVE;
        w.open.bg_stroke = Stroke::new(1.0, BORDER);
        w.open.fg_stroke = Stroke::new(1.0, TEXT);
        w.open.corner_radius = r;
    });
}
