//! Color math and color picker widgets (HSL and RGB), plus swatches storage.

use egui::{Color32, CornerRadius, Id, Mesh, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::document::Rgba;
use crate::ui::{theme, widgets};

pub fn rgb_to_hsl(c: Rgba) -> (f32, f32, f32) {
    let (r, g, b) = (c.r, c.g, c.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(360.0) / 360.0;
    if s <= 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

fn c32(r: f32, g: f32, b: f32) -> Color32 {
    Color32::from_rgb((r.clamp(0.0, 1.0) * 255.0) as u8, (g.clamp(0.0, 1.0) * 255.0) as u8, (b.clamp(0.0, 1.0) * 255.0) as u8)
}

fn hsl32(h: f32, s: f32, l: f32) -> Color32 {
    let (r, g, b) = hsl_to_rgb(h, s, l);
    c32(r, g, b)
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, serde::Serialize, serde::Deserialize)]
pub enum PickerMode {
    #[default]
    Hsl,
    Rgb,
}

#[derive(Clone, Default)]
struct PickerState {
    h: f32,
    s: f32,
    l: f32,
    last: Option<[f32; 4]>,
    hex: String,
}

/// How every colour picker looks: the wheel or the square, and HSL or RGB
/// sliders. Shared by all pickers and saved with the settings.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PickerPrefs {
    pub wheel: bool,
    pub mode: PickerMode,
}

fn prefs_id() -> Id {
    Id::new("color_picker_prefs")
}

pub fn picker_prefs(ctx: &egui::Context) -> PickerPrefs {
    ctx.data(|d| d.get_temp(prefs_id())).unwrap_or_default()
}

pub fn set_picker_prefs(ctx: &egui::Context, p: PickerPrefs) {
    ctx.data_mut(|d| d.insert_temp(prefs_id(), p));
}

pub fn checkerboard(painter: &egui::Painter, rect: Rect, cell: f32) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(250));
    let nx = (rect.width() / cell).ceil() as i32;
    let ny = (rect.height() / cell).ceil() as i32;
    for y in 0..ny {
        for x in 0..nx {
            if (x + y) % 2 == 0 {
                let r = Rect::from_min_size(rect.min + vec2(x as f32 * cell, y as f32 * cell), Vec2::splat(cell))
                    .intersect(rect);
                painter.rect_filled(r, 0.0, Color32::from_gray(205));
            }
        }
    }
}

/// A rounded color chip. Shows a checkerboard under transparent colors.
pub fn swatch(ui: &mut Ui, color: Rgba, size: Vec2, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter().with_clip_rect(rect);
        if color.a < 1.0 {
            checkerboard(&p, rect, 5.0);
        }
        p.rect_filled(rect, CornerRadius::same(4), color.to_color32(1.0));
        let stroke = if selected {
            Stroke::new(2.0, theme::ACCENT)
        } else if resp.hovered() {
            Stroke::new(1.0, Color32::from_gray(200))
        } else {
            Stroke::new(1.0, Color32::from_black_alpha(60))
        };
        ui.painter().rect_stroke(rect, CornerRadius::same(4), stroke, StrokeKind::Inside);
    }
    resp
}

/// Horizontal slider whose track is painted with a gradient.
fn gradient_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    max: f32,
    suffix: &str,
    alpha_track: bool,
    color_at: impl Fn(f32) -> Color32,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([14.0, 18.0], egui::Label::new(egui::RichText::new(label).color(theme::TEXT_DIM)));
        let w = (ui.available_width() - 62.0).max(40.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click_and_drag());
        let track = rect.shrink2(vec2(0.0, 3.0));
        let painter = ui.painter();
        if alpha_track {
            checkerboard(&painter.with_clip_rect(track), track, 6.0);
        }
        let mut mesh = Mesh::default();
        let n = 24;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let x = track.left() + t * track.width();
            let c = color_at(t * max);
            mesh.colored_vertex(pos2(x, track.top()), c);
            mesh.colored_vertex(pos2(x, track.bottom()), c);
            if i > 0 {
                let k = (i * 2) as u32;
                mesh.add_triangle(k - 2, k - 1, k);
                mesh.add_triangle(k - 1, k, k + 1);
            }
        }
        painter.add(mesh);
        painter.rect_stroke(track, CornerRadius::same(3), Stroke::new(1.0, Color32::from_black_alpha(80)), StrokeKind::Inside);
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on() {
                let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
                let nv = t * max;
                if (nv - *value).abs() > f32::EPSILON {
                    *value = nv;
                    changed = true;
                }
            }
        }
        let x = track.left() + (*value / max).clamp(0.0, 1.0) * track.width();
        let knob = Rect::from_center_size(pos2(x, rect.center().y), vec2(8.0, 18.0));
        painter.rect_filled(knob, CornerRadius::same(3), Color32::WHITE);
        painter.rect_stroke(knob, CornerRadius::same(3), Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Outside);

        let (scale, decimals) = if max <= 1.0 { (100.0, 0) } else { (1.0, 0) };
        let mut shown = *value * scale;
        let r = ui.add_sized(
            [54.0, 20.0],
            egui::DragValue::new(&mut shown)
                .range(0.0..=max * scale)
                .max_decimals(decimals)
                .suffix(suffix),
        );
        if r.changed() {
            *value = shown / scale;
            changed = true;
        }
    });
    changed
}

/// Affinity-style HSL wheel: hue ring around a triangle (hue, white, black)
/// that rotates with the hue. Returns true when h/s/l changed.
pub fn hsl_wheel(ui: &mut Ui, h: &mut f32, s: &mut f32, l: &mut f32, id: Id) -> bool {
    let side = ui.available_width().min(168.0);
    let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), side), Sense::click_and_drag());
    let c = outer.center();
    let r_out = side / 2.0 - 2.0;
    let ring = (r_out * 0.16).max(10.0);
    let r_in = r_out - ring;
    let tri_r = r_in - 4.0;
    // Screen angle of a hue (hue grows counter-clockwise, red at the right).
    let ang = |hue: f32| -hue.to_radians();
    let at = |a: f32, r: f32| c + vec2(a.cos(), a.sin()) * r;
    let painter = ui.painter();
    // Hue ring.
    let n = 120;
    let mut mesh = Mesh::default();
    for i in 0..=n {
        let hue = i as f32 / n as f32 * 360.0;
        let col = hsl32(hue, 1.0, 0.5);
        mesh.colored_vertex(at(ang(hue), r_in), col);
        mesh.colored_vertex(at(ang(hue), r_out), col);
    }
    for i in 0..n as u32 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 2, k + 3);
    }
    painter.add(mesh);
    // Triangle vertices: pure hue, black, white.
    let pv = at(ang(*h), tri_r);
    let kv = at(ang(*h + 120.0), tri_r);
    let wv = at(ang(*h + 240.0), tri_r);
    let mut tri = Mesh::default();
    tri.colored_vertex(pv, hsl32(*h, 1.0, 0.5));
    tri.colored_vertex(kv, Color32::BLACK);
    tri.colored_vertex(wv, Color32::WHITE);
    tri.add_triangle(0, 1, 2);
    painter.add(tri);
    // Current color: barycentric weights (chroma, blackness, whiteness).
    let chroma = (1.0 - (2.0 * *l - 1.0).abs()) * *s;
    let m = *l - chroma / 2.0;
    let max = *l + chroma / 2.0;
    let knob = pv * chroma + (kv.to_vec2() * (1.0 - max)) + (wv.to_vec2() * m);
    painter.circle_stroke(knob, 6.0, Stroke::new(2.0, Color32::WHITE));
    painter.circle_stroke(knob, 7.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
    let hk = at(ang(*h), r_in + ring / 2.0);
    painter.circle_stroke(hk, ring / 2.0 - 1.0, Stroke::new(2.0, Color32::WHITE));
    painter.circle_stroke(hk, ring / 2.0 + 0.5, Stroke::new(1.0, Color32::from_black_alpha(160)));

    // Interaction: the press decides between ring and triangle.
    let key = id.with("wheel_drag");
    let mut changed = false;
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.drag_started() || resp.clicked() || (resp.is_pointer_button_down_on() && !resp.dragged()) {
            let on_ring = (p - c).length() > r_in - 2.0;
            ui.data_mut(|d| d.insert_temp(key, on_ring));
        }
        let on_ring = ui.data_mut(|d| d.get_temp::<bool>(key).unwrap_or(false));
        if resp.is_pointer_button_down_on() || resp.dragged() || resp.clicked() {
            if on_ring {
                let v = p - c;
                *h = (-v.y.atan2(v.x)).to_degrees().rem_euclid(360.0);
            } else {
                // Barycentric coordinates in (pure, black, white), clamped to the triangle.
                let (a, b, cw) = barycentric(p, pv, kv, wv);
                let (a, b, cw) = (a.max(0.0), b.max(0.0), cw.max(0.0));
                let sum = (a + b + cw).max(1e-6);
                let (chroma, white) = (a / sum, cw / sum);
                let mx = chroma + white;
                let nl = (mx + white) / 2.0;
                let denom = 1.0 - (2.0 * nl - 1.0).abs();
                *l = nl.clamp(0.0, 1.0);
                *s = if denom > 1e-4 { (chroma / denom).clamp(0.0, 1.0) } else { 0.0 };
            }
            changed = true;
        }
    }
    changed
}

fn barycentric(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> (f32, f32, f32) {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let d = v0.x * v1.y - v1.x * v0.y;
    if d.abs() < 1e-6 {
        return (1.0, 0.0, 0.0);
    }
    let v = (v2.x * v1.y - v1.x * v2.y) / d;
    let w = (v0.x * v2.y - v2.x * v0.y) / d;
    (1.0 - v - w, v, w)
}

/// Full color picker with an S/L field, hue strip and HSL or RGB sliders.
pub fn color_picker(ui: &mut Ui, color: &mut Rgba, id: Id) -> bool {
    let mut st: PickerState = ui.data_mut(|d| d.get_temp(id).unwrap_or_default());
    let cur = [color.r, color.g, color.b, color.a];
    if st.last != Some(cur) {
        let (h, s, l) = rgb_to_hsl(*color);
        // Keep hue/saturation stable for greys.
        if s > 1e-4 && l > 1e-4 && l < 1.0 - 1e-4 {
            st.h = h;
            st.s = s;
        } else if st.last.is_none() {
            st.h = h;
            st.s = s;
        }
        st.l = l;
        st.hex = color.to_hex();
        st.last = Some(cur);
    }
    let mut changed = false;
    let mut hsl_changed = false;

    ui.spacing_mut().item_spacing.y = 6.0;
    let mut prefs = picker_prefs(ui.ctx());
    let before = prefs;
    if prefs.wheel {
        hsl_changed |= hsl_wheel(ui, &mut st.h, &mut st.s, &mut st.l, id);
    } else {
        // Saturation / lightness field.
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 140.0), Sense::click_and_drag());
        {
            let painter = ui.painter().with_clip_rect(rect);
            let mut mesh = Mesh::default();
            let (nx, ny) = (12, 12);
            for yi in 0..=ny {
                for xi in 0..=nx {
                    let s = xi as f32 / nx as f32;
                    let l = 1.0 - yi as f32 / ny as f32;
                    mesh.colored_vertex(
                        pos2(rect.left() + s * rect.width(), rect.top() + (1.0 - l) * rect.height()),
                        hsl32(st.h, s, l),
                    );
                }
            }
            for yi in 0..ny {
                for xi in 0..nx {
                    let i = (yi * (nx + 1) + xi) as u32;
                    let r = (nx + 1) as u32;
                    mesh.add_triangle(i, i + 1, i + r);
                    mesh.add_triangle(i + 1, i + r, i + r + 1);
                }
            }
            painter.add(mesh);
            let knob = pos2(rect.left() + st.s * rect.width(), rect.top() + (1.0 - st.l) * rect.height());
            painter.circle_stroke(knob, 6.0, Stroke::new(2.0, Color32::WHITE));
            painter.circle_stroke(knob, 7.5, Stroke::new(1.0, Color32::from_black_alpha(140)));
            ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, Color32::from_black_alpha(90)), StrokeKind::Inside);
        }
        if let Some(p) = resp.interact_pointer_pos() {
            if resp.is_pointer_button_down_on() || resp.dragged() || resp.clicked() {
                st.s = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                st.l = 1.0 - ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                hsl_changed = true;
            }
        }
    }

    // Mode switch.
    ui.horizontal(|ui| {
        widgets::segmented(ui, &mut prefs.wheel, &[(false, egui_phosphor::regular::SQUARE), (true, egui_phosphor::regular::CIRCLE_HALF)]);
        widgets::segmented(ui, &mut prefs.mode, &[(PickerMode::Hsl, "HSL"), (PickerMode::Rgb, "RGB")]);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut st.hex).desired_width(76.0).font(egui::TextStyle::Monospace));
            if r.changed() || r.lost_focus() {
                if let Some(c) = Rgba::from_hex(&st.hex) {
                    let a = color.a;
                    *color = c.with_alpha(if st.hex.trim_start_matches('#').len() == 8 { c.a } else { a });
                    changed = true;
                }
            }
            ui.label(egui::RichText::new("Hex").color(theme::TEXT_DIM));
        });
    });

    let (h, s, l) = (st.h, st.s, st.l);
    hsl_changed |= gradient_slider(ui, "H", &mut st.h, 360.0, "°", false, |v| hsl32(v, 1.0, 0.5));
    if prefs != before {
        set_picker_prefs(ui.ctx(), prefs);
    }
    match prefs.mode {
        PickerMode::Hsl => {
            hsl_changed |= gradient_slider(ui, "S", &mut st.s, 1.0, "%", false, |v| hsl32(h, v, l));
            hsl_changed |= gradient_slider(ui, "L", &mut st.l, 1.0, "%", false, |v| hsl32(h, s, v));
        }
        PickerMode::Rgb => {
            let mut r = color.r * 255.0;
            let mut g = color.g * 255.0;
            let mut b = color.b * 255.0;
            let (cr, cg, cb) = (color.r, color.g, color.b);
            let mut rgb_changed = false;
            rgb_changed |= gradient_slider(ui, "R", &mut r, 255.0, "", false, |v| c32(v / 255.0, cg, cb));
            rgb_changed |= gradient_slider(ui, "G", &mut g, 255.0, "", false, |v| c32(cr, v / 255.0, cb));
            rgb_changed |= gradient_slider(ui, "B", &mut b, 255.0, "", false, |v| c32(cr, cg, v / 255.0));
            if rgb_changed {
                color.r = (r / 255.0).clamp(0.0, 1.0);
                color.g = (g / 255.0).clamp(0.0, 1.0);
                color.b = (b / 255.0).clamp(0.0, 1.0);
                let (nh, ns, nl) = rgb_to_hsl(*color);
                if ns > 1e-4 {
                    st.h = nh;
                }
                st.s = ns;
                st.l = nl;
                changed = true;
            }
        }
    }
    let base = *color;
    let mut a = color.a;
    if gradient_slider(ui, "A", &mut a, 1.0, "%", true, |v| base.with_alpha(v).to_color32(1.0)) {
        color.a = a;
        changed = true;
    }

    if hsl_changed {
        let (r, g, b) = hsl_to_rgb(st.h, st.s, st.l);
        color.r = r;
        color.g = g;
        color.b = b;
        changed = true;
    }
    if changed {
        st.last = Some([color.r, color.g, color.b, color.a]);
        st.hex = color.to_hex();
    }
    ui.data_mut(|d| d.insert_temp(id, st));
    changed
}

