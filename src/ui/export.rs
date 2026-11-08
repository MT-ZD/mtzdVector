//! Export dialog: a live preview beside format, area, quality and print
//! options, then a save dialog.

use egui::{RichText, Ui};
use egui_phosphor::regular as icon;

use super::dialogs::{footer, modal_title};
use super::theme::{self, TEXT_DIM};
use super::widgets;
use crate::app::App;
use crate::io::export::{Area, Format, Options, PdfPreset};

#[derive(Default)]
pub struct ExportState {
    pub open: bool,
    pub opts: Options,
    /// Last export target, for Export Again.
    pub last: Option<(std::path::PathBuf, Options)>,
    pub(super) preview: super::export_preview::Preview,
}

const PREVIEW_W: f32 = 440.0;
const OPTIONS_W: f32 = 420.0;

const SCALES: [(f64, &str); 5] = [(0.5, "0.5×"), (1.0, "1×"), (2.0, "2×"), (3.0, "3×"), (4.0, "4×")];

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.ui.export.open {
        // Stops the worker and frees the texture.
        app.ui.export.preview = Default::default();
        return;
    }
    let mut action = None;
    let height = (ctx.content_rect().height() - 190.0).clamp(300.0, 560.0);
    let r = super::dialogs::modal("export_dialog").show(ctx, |ui| {
        ui.set_width(PREVIEW_W + 20.0 + OPTIONS_W);
        modal_title(ui, icon::EXPORT, "Export");
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            ui.vertical(|ui| {
                ui.set_width(PREVIEW_W);
                super::export_preview::show(app, ui, egui::vec2(PREVIEW_W, height));
            });
            ui.vertical(|ui| {
                ui.set_width(OPTIONS_W);
                egui::ScrollArea::vertical().id_salt("export_options").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(OPTIONS_W - 14.0);
                    body(app, ui);
                });
            });
        });
        action = footer(ui, &[], &["Cancel"], "Export…");
    });
    if r.should_close() && action.is_none() {
        action = Some("Cancel".into());
    }
    match action.as_deref() {
        Some("Export…") => {
            app.ui.export.open = false;
            app.export_with_dialog();
        }
        Some(_) => app.ui.export.open = false,
        None => {}
    }
}

fn body(app: &mut App, ui: &mut Ui) {
    let has_sel = !app.selection.is_empty();
    let o = &mut app.ui.export.opts;
    if !has_sel && o.area == Area::Selection {
        o.area = Area::Artboard;
    }
    let many = app.doc.artboards.len() > 1;
    if !many && o.area == Area::AllArtboards {
        o.area = Area::Artboard;
    }
    widgets::prop_row(ui, "Format", |ui| {
        let opts: Vec<(Format, &str)> = Format::ALL.iter().map(|f| (*f, f.label())).collect();
        widgets::segmented(ui, &mut o.format, &opts);
    });
    widgets::prop_row(ui, "Area", |ui| {
        let mut opts = vec![(Area::Artboard, if many { "Active artboard" } else { "Artboard" })];
        if many {
            opts.push((Area::AllArtboards, "All artboards"));
        }
        if has_sel {
            opts.push((Area::Selection, "Selection"));
        }
        widgets::segmented(ui, &mut o.area, &opts);
    });
    if o.format == Format::Pdf {
        pdf_presets(ui, o);
    }
    let opts = o.clone();
    if opts.format.is_raster() {
        let sel_bounds = (opts.area == Area::Selection)
            .then(|| {
                let shapes: Vec<&crate::document::Shape> = app.selection.iter().filter_map(|id| app.doc.get(*id)).collect();
                crate::io::export::visual_bounds(&shapes, &mut app.fonts)
            })
            .flatten();
        let active = app.active().clone();
        let o = &mut app.ui.export.opts;
        widgets::prop_row(ui, "Scale", |ui| {
            widgets::segmented(ui, &mut o.scale, &SCALES);
        });
        let (w, h) = crate::io::export::raster_size(&active, sel_bounds, o.scale);
        let (base_w, base_h) = crate::io::export::raster_size(&active, sel_bounds, 1.0);
        let px = || widgets::Num::default().range(1.0, 20000.0).decimals(0).suffix(" px");
        widgets::prop_row(ui, "Size", |ui| {
            // Typing an exact size sets the scale; proportions stay.
            let (mut nw, mut nh) = (w as f64, h as f64);
            if widgets::tip(widgets::num_field(ui, "W", &mut nw, px(), 100.0), "Exact width: sets the scale", None).changed() {
                o.scale = (nw / base_w as f64).clamp(0.01, 64.0);
            }
            if widgets::tip(widgets::num_field(ui, "H", &mut nh, px(), 100.0), "Exact height: sets the scale", None).changed() {
                o.scale = (nh / base_h as f64).clamp(0.01, 64.0);
            }
        });
        if o.format != Format::Jpg {
            widgets::prop_row(ui, "Background", |ui| {
                widgets::segmented(ui, &mut o.background, &[(true, "Artboard"), (false, "Transparent")]);
            });
        }
        if o.format == Format::Webp {
            widgets::prop_row(ui, "Mode", |ui| {
                widgets::segmented(ui, &mut o.webp_lossy, &[(false, "Lossless"), (true, "Lossy")]);
            });
        }
        if matches!(o.format, Format::Jpg | Format::Avif) || (o.format == Format::Webp && o.webp_lossy) {
            let mut q = o.quality as f32;
            if widgets::slider_row(ui, "Quality", &mut q, 1.0..=100.0, "%") {
                o.quality = q.round() as u8;
            }
        }
        if o.format == Format::Webp && !o.webp_lossy {
            ui.label(RichText::new("Exact colours, smaller than PNG for most artwork.").size(theme::FONT_SM).color(TEXT_DIM));
        }
    } else {
        if opts.area == Area::AllArtboards && opts.format != Format::Pdf {
            ui.label(RichText::new("One file per artboard, named after the artboard.").size(theme::FONT_SM).color(TEXT_DIM));
        }
        let note = match opts.format {
            Format::Pdf if opts.area == Area::AllArtboards => "One page per artboard.",
            Format::Pdf => "",
            Format::Eps => "Vector EPS. Effects and image fills are not supported.",
            Format::Dxf => "DXF (R12) in millimetres for CAD, laser cutters and plotters: every outline as a polyline, one DXF layer per top-level layer. Fills, gradients and effects don't carry over; text is outlined.",
            _ => "Vector SVG.",
        };
        if !matches!(opts.format, Format::Eps | Format::Dxf) {
            let o = &mut app.ui.export.opts;
            widgets::prop_row(ui, "Text", |ui| {
                widgets::segmented(ui, &mut o.text_outlines, &[(false, "Text"), (true, "Outlines")]);
            });
            let hint = if o.text_outlines {
                "Text becomes shapes: looks the same everywhere, but can't be edited or searched."
            } else if opts.format == Format::Pdf {
                "Selectable text with the fonts embedded (subsets). Path text and coloured emoji are outlined."
            } else {
                "Editable <text>: viewers need the fonts installed. Path text, gradient-filled text and coloured emoji are outlined."
            };
            ui.label(RichText::new(hint).size(theme::FONT_SM).color(TEXT_DIM));
        }
        if !note.is_empty() {
            ui.label(RichText::new(note).size(theme::FONT_SM).color(TEXT_DIM));
        }
        if opts.format == Format::Pdf {
            pdf_quality(app, ui);
            print_options(app, ui);
            pdf_metadata(app, ui);
        }
        if opts.format == Format::Svg {
            svg_options(app, ui);
        }
    }
}

/// Preset tiles for PDF. The highlighted tile is the one the options are
/// exactly on; any change after that is "Custom".
fn pdf_presets(ui: &mut Ui, o: &mut Options) {
    let current = PdfPreset::of(o);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("PRESET").size(10.5).color(TEXT_DIM));
        if current.is_none() {
            ui.label(RichText::new("Custom").size(theme::FONT_SM).color(theme::ACCENT));
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = ((ui.available_width() - 18.0) / 4.0).floor();
        for p in PdfPreset::ALL {
            let (glyph, label, title, body) = match p {
                PdfPreset::Web => (icon::GLOBE, "Web", "Web and email", "Small file for sharing on screen. Images are reduced to 150 ppi and JPEG compressed."),
                PdfPreset::Standard => (icon::FILE_PDF, "Standard", "Standard", "Full quality RGB. Every image pixel is kept, lossless."),
                PdfPreset::Print => (icon::PRINTER, "Print", "Print", "CMYK with 3 mm bleed and images up to 300 ppi. What most online printers ask for."),
                PdfPreset::Press => (icon::STAMP, "Press", "Press (PDF/X-4)", "PDF/X-4 with the output profile embedded, 3 mm bleed and crop marks, for professional print shops."),
            };
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 58.0), egui::Sense::click());
            let on = current == Some(p);
            let fill = if on { theme::ACCENT.gamma_multiply(0.22) } else if resp.hovered() { theme::BG_HOVER } else { theme::BG_INPUT };
            ui.painter().rect_filled(rect, 6, fill);
            if on {
                ui.painter().rect_stroke(rect, 6, egui::Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
            }
            let color = if on { theme::TEXT } else { TEXT_DIM };
            ui.painter().text(rect.center_top() + egui::vec2(0.0, 20.0), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(20.0), color);
            ui.painter().text(rect.center_bottom() - egui::vec2(0.0, 13.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(theme::FONT_SM), color);
            if widgets::tip_long(resp, title, None, body).clicked() {
                p.apply(o);
            }
        }
    });
    if matches!(current, Some(PdfPreset::Print | PdfPreset::Press)) && crate::print::cmyk_profiles().is_empty() {
        ui.label(RichText::new("No CMYK profile is installed, so colours use a generic conversion and PDF/X-4 is off. Choose your printer's profile below.").size(theme::FONT_SM).color(TEXT_DIM));
    }
    ui.add_space(4.0);
}

/// PDF image handling: effect resolution, downsampling, compression.
fn pdf_quality(app: &mut App, ui: &mut Ui) {
    let cmyk = app.ui.export.opts.print.color == crate::print::ColorOutput::Cmyk || app.ui.export.opts.print.pdfx;
    let o = &mut app.ui.export.opts.pdf;
    ui.add_space(8.0);
    ui.label(RichText::new("IMAGES").size(10.5).color(TEXT_DIM));
    widgets::prop_row(ui, "Resolution", |ui| {
        widgets::tip(ui.scope(|ui| widgets::segmented(ui, &mut o.image_ppi, &[(None, "Keep"), (Some(150), "150"), (Some(220), "220"), (Some(300), "300")])).response, "Reduce images placed above this ppi", None);
    });
    widgets::prop_row(ui, "Quality", |ui| {
        ui.add_enabled_ui(!cmyk, |ui| {
            widgets::segmented(ui, &mut o.jpeg_quality, &[(None, "Lossless"), (Some(90), "High"), (Some(75), "Medium"), (Some(50), "Low")]);
        });
    });
    if cmyk {
        ui.label(RichText::new("CMYK images stay lossless.").size(theme::FONT_SM).color(TEXT_DIM));
    }
    widgets::prop_row(ui, "Effects", |ui| {
        widgets::tip(ui.scope(|ui| widgets::segmented(ui, &mut o.effect_dpi, &[(150.0, "150"), (300.0, "300"), (600.0, "600")])).response, "Resolution blur, noise and shadows are embedded at (dpi)", None);
    });
}

/// PDF document properties.
fn pdf_metadata(app: &mut App, ui: &mut Ui) {
    let stem = app.file_path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "File name".into());
    let o = &mut app.ui.export.opts.pdf;
    ui.add_space(8.0);
    egui::CollapsingHeader::new(RichText::new("METADATA").size(10.5).color(TEXT_DIM)).id_salt("pdf_metadata").show(ui, |ui| {
        let w = (ui.available_width() - 100.0).max(120.0);
        for (label, value, hint) in [("Title", &mut o.title, stem.as_str()), ("Author", &mut o.author, ""), ("Subject", &mut o.subject, ""), ("Keywords", &mut o.keywords, "Comma separated")] {
            widgets::prop_row(ui, label, |ui| {
                ui.add(egui::TextEdit::singleline(value).desired_width(w).hint_text(hint));
            });
        }
    });
}

/// SVG settings: precision, ids, minify, responsive size.
fn svg_options(app: &mut App, ui: &mut Ui) {
    let o = &mut app.ui.export.opts.svg;
    ui.add_space(8.0);
    ui.label(RichText::new("SVG").size(10.5).color(TEXT_DIM));
    widgets::prop_row(ui, "Decimals", |ui| {
        let mut p = o.precision;
        if widgets::segmented(ui, &mut p, &[(1, "1"), (2, "2"), (3, "3"), (4, "4"), (6, "6")]) {
            o.precision = p;
        }
    });
    widgets::prop_row(ui, "Markup", |ui| {
        widgets::tip(ui.checkbox(&mut o.ids, "Layer names as ids"), "Each element gets an id from its layer name (made unique)", None);
    });
    widgets::prop_row(ui, "", |ui| {
        widgets::tip(ui.checkbox(&mut o.minify, "Minify"), "No indentation or line breaks: smaller files", None);
        widgets::tip(ui.checkbox(&mut o.responsive, "Responsive"), "No fixed width and height: scales to fill its container (keeps the viewBox)", None);
    });
}

/// PDF print settings: colour output, profile, PDF/X-4, bleed and marks.
fn print_options(app: &mut App, ui: &mut Ui) {
    use crate::print::{ColorOutput, cmyk_profiles};
    ui.add_space(8.0);
    ui.label(RichText::new("PRINT").size(10.5).color(TEXT_DIM));
    let mut pick = false;
    {
        let p = &mut app.ui.export.opts.print;
        widgets::prop_row(ui, "Colour", |ui| {
            if widgets::segmented(ui, &mut p.color, &[(ColorOutput::Rgb, "RGB"), (ColorOutput::Cmyk, "CMYK")]) && p.color == ColorOutput::Cmyk && p.profile.is_none() {
                // The best press profile installed (FOGRA39 / GRACoL first).
                p.profile = cmyk_profiles().first().map(|q| q.path.clone());
            }
        });
        if p.color == ColorOutput::Cmyk || p.pdfx {
            let current = match &p.profile {
                Some(path) => cmyk_profiles().iter().find(|q| &q.path == path).map(|q| q.name.clone()).unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
                None => "Generic CMYK (no profile)".into(),
            };
            widgets::prop_row(ui, "Profile", |ui| {
                widgets::dropdown(ui, "pdf_profile", current, (ui.available_width() - 4.0).max(120.0), |ui| {
                    if ui.selectable_label(p.profile.is_none(), "Generic CMYK (no profile)").clicked() {
                        p.profile = None;
                    }
                    for q in cmyk_profiles() {
                        if ui.selectable_label(p.profile.as_ref() == Some(&q.path), &q.name).clicked() {
                            p.profile = Some(q.path.clone());
                        }
                    }
                    ui.separator();
                    if ui.selectable_label(false, "Choose a Profile…").clicked() {
                        pick = true;
                    }
                });
            });
        }
        widgets::prop_row(ui, "Standard", |ui| {
            let can = p.profile.is_some();
            let r = ui.add_enabled(can, egui::Checkbox::new(&mut p.pdfx, "PDF/X-4"));
            widgets::tip_long(r, "PDF/X-4", None, if can { "For print shops: CMYK with the output profile embedded, trim and bleed boxes, live transparency." } else { "Choose a CMYK profile first." });
            if !can {
                p.pdfx = false;
            }
        });
        widgets::prop_row(ui, "Bleed", |ui| {
            widgets::num_field(ui, "", &mut p.bleed_mm, widgets::Num::default().range(0.0, 50.0).speed(0.1).suffix(" mm"), 90.0);
            ui.checkbox(&mut p.crop_marks, "Crop marks");
        });
        if p.pdfx {
            p.color = ColorOutput::Cmyk;
        }
    }
    let p = &app.ui.export.opts.print;
    let hint = if p.color == ColorOutput::Cmyk {
        if p.profile.is_some() { "Colours, gradients and images are converted to the profile's CMYK. Spot colour keys print as their own inks." } else { "Colours are converted with a generic formula. Choose your printer's profile for accurate colour." }
    } else {
        "Spot colour keys print as their own inks in any mode."
    };
    ui.label(RichText::new(hint).size(theme::FONT_SM).color(TEXT_DIM));
    if pick {
        if let Some(path) = rfd::FileDialog::new().add_filter("ICC profile", &["icc", "icm"]).pick_file() {
            match crate::print::ColorConv::from_path(&path) {
                Ok(_) => app.ui.export.opts.print.profile = Some(path),
                Err(e) => app.notify(format!("Can't use this profile: {e}")),
            }
        }
    }
}

impl App {
    fn export_name(&self, opts: &Options) -> String {
        let stem = self.file_path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
        let mut name = stem;
        if opts.area == Area::Selection {
            if let Some(s) = self.selection.first().and_then(|id| self.doc.get(*id)).filter(|_| self.selection.len() == 1) {
                name = s.name.replace(['/', '\\', ':'], "-");
            } else {
                name.push_str("-selection");
            }
        }
        if opts.area == Area::Artboard && self.doc.artboards.len() > 1 {
            name = format!("{name}-{}", self.active().name.replace(['/', '\\', ':'], "-"));
        }
        if opts.format.is_raster() && opts.scale != 1.0 {
            name.push_str(&format!("@{}x", opts.scale));
        }
        format!("{name}.{}", opts.format.ext())
    }

    /// Ask for a path, then export with the dialog's options.
    pub fn export_with_dialog(&mut self) {
        let opts = self.ui.export.opts.clone();
        let ext = opts.format.ext();
        let Some(mut p) = rfd::FileDialog::new().add_filter(opts.format.label(), &[ext]).set_file_name(self.export_name(&opts)).save_file() else { return };
        if p.extension().is_none() {
            p.set_extension(ext);
        }
        self.export_to(&p, &opts);
    }

    pub fn export_to(&mut self, path: &std::path::Path, opts: &Options) {
        let active = self.active().id;
        match crate::io::export::export(&self.doc, &self.selection, active, opts, path, &mut self.fonts) {
            Ok(files) => {
                match files.as_slice() {
                    [one] => self.notify(format!("Exported {}", one.display())),
                    many => self.notify(format!("Exported {} files to {}", many.len(), path.parent().map(|p| p.display().to_string()).unwrap_or_default())),
                }
                self.ui.export.last = Some((path.to_path_buf(), opts.clone()));
            }
            Err(e) => self.notify(format!("Export failed: {e}")),
        }
    }

    /// Export slices into a folder chosen by the user: `targets`, or every
    /// layer and artboard with export settings.
    pub fn export_slices_dialog(&mut self, targets: Option<Vec<crate::io::export::SliceTarget>>) {
        let targets = targets.unwrap_or_else(|| crate::io::export::slice_targets(&self.doc));
        if targets.is_empty() {
            self.notify("Nothing to export: add export sizes to layers or artboards in the Design panel");
            return;
        }
        let start = self.file_path.as_ref().and_then(|p| p.parent()).map(|p| p.to_path_buf());
        let mut dlg = rfd::FileDialog::new().set_title("Export Slices To");
        if let Some(d) = start {
            dlg = dlg.set_directory(d);
        }
        let Some(dir) = dlg.pick_folder() else { return };
        match crate::io::export::export_slices(&self.doc, &targets, &dir, &mut self.fonts) {
            Ok(files) => self.notify(format!("Exported {} file{} to {}", files.len(), if files.len() == 1 { "" } else { "s" }, dir.display())),
            Err(e) => self.notify(format!("Export failed: {e}")),
        }
    }

    /// Repeat the last export (same file and options), or open the dialog.
    pub fn export_again(&mut self) {
        match self.ui.export.last.clone() {
            Some((p, o)) => self.export_to(&p, &o),
            None => self.ui.export.open = true,
        }
    }
}
