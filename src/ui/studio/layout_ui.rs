//! Design panel: auto layout of groups, constraints of group members, and
//! export settings (slices) of the selection or the artboard.

use egui::{RichText, Ui};
use egui_phosphor::regular as icon;

use super::super::theme::{self, TEXT_DIM, TEXT_FAINT};
use super::super::widgets::{self, Num};
use crate::app::App;
use crate::document::{ExportSlice, ShapeKind};
use crate::io::export::Format;
use crate::layout::{Align, Constraints, Direction, Pin};

/// Auto layout of the selected group (or an "Add" button for a group or
/// several shapes).
pub fn auto_layout_section(app: &mut App, ui: &mut Ui) {
    let Some(&id) = app.selection.first() else { return };
    let Some(s) = app.doc.get(id).cloned() else { return };
    let single = app.selection.len() == 1;
    let is_group = matches!(s.kind, ShapeKind::Group { .. }) && s.component.is_none();
    let Some(mut l) = s.layout.clone().filter(|_| single) else {
        if (single && is_group) || app.selection.len() > 1 {
            widgets::section_icon(ui, "auto_layout", icon::ROWS, "Auto layout", |_| {}, |ui| {
                ui.horizontal(|ui| {
                    if widgets::tip(widgets::flat_button(ui, Some(icon::PLUS), "Add auto layout", true), "Stack the layers in a row or column with even gaps and padding", Some("Shift+A")).clicked() {
                        app.add_auto_layout();
                    }
                });
            });
        }
        return;
    };
    let before = l.clone();
    let mut remove = false;
    widgets::section_icon(
        ui,
        "auto_layout",
        icon::ROWS,
        "Auto layout",
        |ui| {
            remove = widgets::tip(widgets::mini_icon(ui, icon::MINUS, TEXT_DIM), "Remove auto layout (keeps the layers where they are)", None).clicked();
        },
        |ui| {
            let (row, col) = (format!("{}  Row", icon::ARROW_RIGHT), format!("{}  Column", icon::ARROW_DOWN));
            widgets::prop_row(ui, "Direction", |ui| {
                widgets::segmented_width(ui, &mut l.dir, &[(Direction::Row, row.as_str()), (Direction::Column, col.as_str())], ui.available_width());
            });
            widgets::prop_row(ui, "Align", |ui| {
                let labels = if l.dir == Direction::Row { ["Top", "Middle", "Bottom"] } else { ["Left", "Centre", "Right"] };
                widgets::segmented_width(ui, &mut l.align, &[(Align::Start, labels[0]), (Align::Center, labels[1]), (Align::End, labels[2])], ui.available_width());
            });
            // Gap and padding; padding per side behind a toggle.
            let sides_id = ui.id().with(("al_sides", id));
            let uniform = l.padding.iter().all(|v| (*v - l.padding[0]).abs() < 1e-9);
            let mut sides = ui.data(|d| d.get_temp::<bool>(sides_id)).unwrap_or(!uniform);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let fw = ((ui.available_width() - 4.0 - 4.0 - 22.0) / 2.0).floor().max(60.0);
                widgets::tip(widgets::num_field(ui, icon::ARROWS_HORIZONTAL, &mut l.gap, Num::px().range(-10000.0, 10000.0), fw), "Gap between layers", None);
                if sides {
                    ui.add_enabled_ui(false, |ui| {
                        let mut v = l.padding[0];
                        widgets::num_field(ui, icon::FRAME_CORNERS, &mut v, Num::px(), fw);
                    });
                } else {
                    let mut p = l.padding[0];
                    if widgets::tip(widgets::num_field(ui, icon::FRAME_CORNERS, &mut p, Num::px().range(0.0, 10000.0), fw), "Padding on all sides", None).changed() {
                        l.padding = [p; 4];
                    }
                }
                let r = widgets::icon_toggle(ui, &mut sides, icon::SQUARE_SPLIT_HORIZONTAL);
                widgets::tip(r, "Padding per side", None);
            });
            if sides {
                for pair in [[(0usize, "Top"), (2, "Bottom")], [(3, "Left"), (1, "Right")]] {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let fw = ((ui.available_width() - 4.0) / 2.0).floor().max(60.0);
                        for (i, lab) in pair {
                            widgets::num_field(ui, &lab[..1], &mut l.padding[i], Num::px().range(0.0, 10000.0), fw).on_hover_text(format!("{lab} padding"));
                        }
                    });
                }
            }
            ui.data_mut(|d| d.insert_temp(sides_id, sides));
            ui.label(RichText::new("Drag layers along the stack to reorder, onto it to add, or out of it to remove.").size(theme::FONT_SM).color(TEXT_FAINT));
        },
    );
    if remove {
        app.remove_auto_layout();
    } else if l != before {
        app.checkpoint("auto layout");
        if let Some(mut g) = app.doc.get(id).cloned() {
            g.layout = Some(l);
            app.doc.replace(g);
        }
        app.layout_key = None;
    }
}

/// Constraints of a group member (not in an auto layout).
pub fn constraints_section(app: &mut App, ui: &mut Ui) {
    let ids: Vec<u64> = app.selection.iter().copied().filter(|id| app.doc.parent_of(*id).is_some() && app.layout_parent(*id).is_none()).collect();
    if ids.is_empty() || ids.len() != app.selection.len() {
        return;
    }
    let cur = app.doc.get(ids[0]).and_then(|s| s.constraints).unwrap_or_default();
    let mut k = cur;
    widgets::section_icon(ui, "constraints", icon::ARROWS_OUT_CARDINAL, "Constraints", |_| {}, |ui| {
        for horizontal in [true, false] {
            widgets::prop_row(ui, if horizontal { "Horizontal" } else { "Vertical" }, |ui| {
                let pin = if horizontal { &mut k.h } else { &mut k.v };
                let w = (ui.available_width() - 4.0).max(100.0);
                widgets::dropdown(ui, ("constraint", horizontal), pin.label(horizontal), w, |ui| {
                    for p in Pin::ALL {
                        if ui.selectable_label(*pin == p, p.label(horizontal)).clicked() {
                            *pin = p;
                        }
                    }
                });
            });
        }
        ui.label(RichText::new("How the layer follows when its group is resized.").size(theme::FONT_SM).color(TEXT_FAINT));
    });
    if k != cur {
        app.checkpoint("constraints");
        for id in ids {
            if let Some(s) = app.doc.get_mut(id) {
                s.constraints = (k != Constraints::default()).then_some(k);
            }
        }
    }
}

const SLICE_FORMATS: [Format; 7] = [Format::Png, Format::Jpg, Format::Webp, Format::Avif, Format::Svg, Format::Pdf, Format::Dxf];

/// Export settings of the selection (or of the active artboard when
/// nothing is selected), with an Export button.
pub fn export_section(app: &mut App, ui: &mut Ui) {
    let artboard = app.selection.is_empty();
    let mut list: Vec<ExportSlice> = if artboard {
        app.active().exports.clone()
    } else {
        app.doc.get(app.selection[0]).map(|s| s.exports.clone()).unwrap_or_default()
    };
    let before = list.clone();
    let mut export_now = false;
    let add = |list: &mut Vec<ExportSlice>| {
        let next = match list.last() {
            None => ExportSlice::new(1.0, Format::Png),
            Some(l) => ExportSlice::new(if l.format.is_raster() { (l.scale + 1.0).min(4.0) } else { 1.0 }, l.format),
        };
        list.push(next);
    };
    let mut plus = false;
    widgets::section_icon(
        ui,
        "export_slices",
        icon::EXPORT,
        "Export",
        |ui| {
            plus = widgets::tip(widgets::mini_icon(ui, icon::PLUS, TEXT_DIM), "Add an export size", None).clicked();
        },
        |ui| {
            let mut remove = None;
            for (i, e) in list.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let raster = e.format.is_raster();
                    let scale_txt = format!("{}x", crate::io::num(e.scale));
                    widgets::dropdown(ui, ("slice_scale", i), if raster { scale_txt } else { "—".into() }, 52.0, |ui| {
                        for s in [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0] {
                            if ui.selectable_label(e.scale == s, format!("{}x", crate::io::num(s))).clicked() {
                                let auto = e.suffix.is_empty() || e.suffix == ExportSlice::new(e.scale, e.format).suffix;
                                e.scale = s;
                                if auto {
                                    e.suffix = ExportSlice::new(s, e.format).suffix;
                                }
                            }
                        }
                    });
                    let w = (ui.available_width() - 80.0).max(50.0);
                    ui.add(egui::TextEdit::singleline(&mut e.suffix).hint_text("Suffix").desired_width(w));
                    widgets::dropdown(ui, ("slice_fmt", i), e.format.label(), 52.0, |ui| {
                        for f in SLICE_FORMATS {
                            if ui.selectable_label(e.format == f, f.label()).clicked() {
                                e.format = f;
                            }
                        }
                    });
                    if widgets::tip(widgets::mini_icon(ui, icon::MINUS, TEXT_DIM), "Remove", None).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                list.remove(i);
            }
            if list.is_empty() {
                ui.horizontal(|ui| {
                    if widgets::flat_button(ui, None, "@1x @2x @3x", true).on_hover_text("PNG at three sizes, for app assets").clicked() {
                        list = [1.0, 2.0, 3.0].map(|s| ExportSlice::new(s, Format::Png)).to_vec();
                    }
                    ui.label(RichText::new("or + for one size").size(theme::FONT_SM).color(TEXT_FAINT));
                });
            } else {
                let n = list.len();
                let what = if artboard { "the artboard".to_string() } else if app.selection.len() == 1 { "the layer".to_string() } else { format!("{} layers", app.selection.len()) };
                ui.horizontal(|ui| {
                    export_now = widgets::flat_button(ui, Some(icon::EXPORT), &format!("Export {what}"), true).on_hover_text(format!("{n} file{} per layer into a folder", if n == 1 { "" } else { "s" })).clicked();
                });
                ui.label(RichText::new("File › Export Slices exports every layer and artboard with settings at once.").size(theme::FONT_SM).color(TEXT_DIM));
            }
        },
    );
    if plus {
        add(&mut list);
    }
    if list != before {
        app.checkpoint("export settings");
        if artboard {
            app.active_mut().exports = list.clone();
            app.dirty = true;
        } else {
            for id in app.selection.clone() {
                if let Some(s) = app.doc.get_mut(id) {
                    s.exports = list.clone();
                }
            }
        }
    }
    if export_now {
        let targets = if artboard { vec![crate::io::export::SliceTarget::Artboard(app.active().id)] } else { app.selection.iter().map(|id| crate::io::export::SliceTarget::Shape(*id)).collect() };
        app.export_slices_dialog(Some(targets));
    }
}
