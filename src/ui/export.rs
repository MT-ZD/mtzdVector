//! Export dialog: format, area and raster options, then a save dialog.

use egui::{RichText, Ui};
use egui_phosphor::regular as icon;

use super::dialogs::{footer, modal_title};
use super::theme::{self, TEXT_DIM};
use super::widgets;
use crate::app::App;
use crate::io::export::{Area, Format, Options};

#[derive(Default)]
pub struct ExportState {
    pub open: bool,
    pub opts: Options,
    /// Last export target, for Export Again.
    pub last: Option<(std::path::PathBuf, Options)>,
}

const SCALES: [(f64, &str); 5] = [(0.5, "0.5×"), (1.0, "1×"), (2.0, "2×"), (3.0, "3×"), (4.0, "4×")];

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.ui.export.open {
        return;
    }
    let mut action = None;
    let r = super::dialogs::modal("export_dialog").show(ctx, |ui| {
        ui.set_width(400.0);
        modal_title(ui, icon::EXPORT, "Export");
        body(app, ui);
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
        widgets::prop_row(ui, "Size", |ui| {
            ui.label(RichText::new(format!("{w} × {h} px")).size(theme::FONT_MD).color(TEXT_DIM));
        });
        if o.format != Format::Jpg {
            widgets::prop_row(ui, "Background", |ui| {
                widgets::segmented(ui, &mut o.background, &[(true, "Artboard"), (false, "Transparent")]);
            });
        }
        if matches!(o.format, Format::Jpg | Format::Avif) {
            let mut q = o.quality as f32;
            if widgets::slider_row(ui, "Quality", &mut q, 1.0..=100.0, "%") {
                o.quality = q.round() as u8;
            }
        }
        if o.format == Format::Webp {
            ui.label(RichText::new("Lossless WebP: exact colours, smaller than PNG for most artwork.").size(theme::FONT_SM).color(TEXT_DIM));
        }
    } else {
        if opts.area == Area::AllArtboards && opts.format != Format::Pdf {
            ui.label(RichText::new("One file per artboard, named after the artboard.").size(theme::FONT_SM).color(TEXT_DIM));
        }
        let note = match opts.format {
            Format::Pdf if opts.area == Area::AllArtboards => "Vector PDF with one page per artboard. Blur, noise and shadows are embedded as 300 DPI images.",
            Format::Pdf => "Vector PDF. Blur, noise and shadows are embedded as 300 DPI images.",
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
        ui.label(RichText::new(note).size(theme::FONT_SM).color(TEXT_DIM));
        if opts.format == Format::Pdf {
            print_options(app, ui);
        }
        if opts.format == Format::Svg {
            svg_options(app, ui);
        }
    }
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
