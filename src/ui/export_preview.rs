//! Live preview of the export dialog. A worker thread renders the page and
//! runs the real export to measure it, so the dialog never stalls.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, Rect, RichText, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use super::theme::{self, TEXT_DIM};
use super::widgets;
use crate::app::App;
use crate::document::Document;
use crate::io::export::{self, Format, Options};

/// Wait after the last change before rendering, in seconds.
const SETTLE: f64 = 0.15;

struct Job {
    id: u64,
    doc: Document,
    selection: Vec<u64>,
    active: u64,
    opts: Options,
    page: usize,
    font_generation: u64,
}

enum Reply {
    Page { id: u64, image: egui::ColorImage, geometry: Geometry },
    Size { id: u64, bytes: Option<u64> },
    Failed { id: u64, message: String },
}

#[derive(Clone, Copy, Default)]
struct Geometry {
    size: (f64, f64),
    bleed: f64,
    dpi: f64,
    pages: usize,
}

struct Worker {
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
    latest: Arc<AtomicU64>,
}

fn spawn(ctx: egui::Context) -> Worker {
    let (jobs, job_rx) = channel::<Job>();
    let (reply, replies) = channel();
    let latest = Arc::new(AtomicU64::new(0));
    let newest = latest.clone();
    std::thread::spawn(move || {
        let mut fonts: Option<(u64, crate::fonts::FontManager)> = None;
        while let Ok(job) = job_rx.recv() {
            let stale = || newest.load(Ordering::Relaxed) != job.id;
            if stale() {
                continue;
            }
            // Its own font manager: the app's isn't shared across threads.
            if fonts.as_ref().map(|f| f.0) != Some(job.font_generation) {
                fonts = Some((job.font_generation, crate::fonts::FontManager::new(ctx.clone())));
            }
            let f = &mut fonts.as_mut().unwrap().1;
            let sent = match export::preview_page(&job.doc, &job.selection, job.active, &job.opts, job.page, f) {
                Ok(p) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([p.image.width() as usize, p.image.height() as usize], p.image.as_raw());
                    reply.send(Reply::Page { id: job.id, image, geometry: Geometry { size: p.size, bleed: p.bleed, dpi: p.dpi, pages: p.pages } })
                }
                Err(message) => reply.send(Reply::Failed { id: job.id, message }),
            };
            ctx.request_repaint();
            if sent.is_err() {
                break;
            }
            if stale() {
                continue;
            }
            let msg = match export::exported_size(&job.doc, &job.selection, job.active, &job.opts, f) {
                Ok(bytes) => Reply::Size { id: job.id, bytes },
                Err(message) => Reply::Failed { id: job.id, message },
            };
            ctx.request_repaint();
            if reply.send(msg).is_err() {
                break;
            }
        }
    });
    Worker { jobs, replies, latest }
}

#[derive(Default)]
pub struct Preview {
    worker: Option<Worker>,
    /// What the last job rendered.
    sent: Option<(Options, usize)>,
    /// What's waiting to settle, and since when.
    pending: Option<((Options, usize), f64)>,
    next_id: u64,
    texture: Option<TextureHandle>,
    geometry: Geometry,
    /// The texture and size are for the newest job.
    page_ready: bool,
    size: Option<Option<u64>>,
    error: Option<String>,
    page: usize,
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

/// Keep the preview in step with the options; call every frame.
fn update(app: &mut App, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    let opts = app.ui.export.opts.clone();
    let doc_font_gen = app.fonts.generation;
    let p = &mut app.ui.export.preview;
    if p.worker.is_none() {
        p.worker = Some(spawn(ctx.clone()));
    }
    let want = (opts, p.page);
    if p.sent.as_ref() != Some(&want) {
        match &p.pending {
            Some((k, _)) if *k == want => {}
            _ => p.pending = Some((want.clone(), now)),
        }
    }
    let due = p.pending.as_ref().is_some_and(|(_, t)| now - t >= SETTLE || p.sent.is_none());
    if due {
        let (key, _) = p.pending.take().unwrap();
        p.next_id += 1;
        let id = p.next_id;
        p.page_ready = false;
        p.error = None;
        p.size = None;
        p.sent = Some(key.clone());
        let job = Job { id, doc: app.doc.clone(), selection: app.selection.clone(), active: app.active().id, opts: key.0, page: key.1, font_generation: doc_font_gen };
        let p = &mut app.ui.export.preview;
        if let Some(w) = &p.worker {
            w.latest.store(id, Ordering::Relaxed);
            let _ = w.jobs.send(job);
        }
    } else if p.pending.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }
    let p = &mut app.ui.export.preview;
    let Some(w) = &p.worker else { return };
    let replies: Vec<Reply> = w.replies.try_iter().collect();
    for r in replies {
        match r {
            Reply::Page { id, image, geometry } if id == p.next_id => {
                p.texture = Some(ctx.load_texture("export_preview", image, TextureOptions::LINEAR));
                p.geometry = geometry;
                p.page_ready = true;
            }
            Reply::Size { id, bytes } if id == p.next_id => p.size = Some(bytes),
            Reply::Failed { id, message } if id == p.next_id => {
                p.error = Some(message);
                p.page_ready = true;
            }
            _ => {}
        }
    }
}

/// The preview pane: the page with bleed and crop marks, a caption below.
pub fn show(app: &mut App, ui: &mut Ui, size: egui::Vec2) {
    update(app, &ui.ctx().clone());
    let opts = app.ui.export.opts.clone();
    let p = &mut app.ui.export.preview;
    let g = p.geometry;
    let caption_h = 26.0;
    let (rect, _) = ui.allocate_exact_size(vec2(size.x, size.y - caption_h), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6, theme::BG_INPUT);
    let pdf = opts.format == Format::Pdf;
    if let Some(tex) = p.texture.as_ref().filter(|_| g.size.0 > 0.0) {
        let mm = |v: f64| v * g.dpi / 25.4;
        let mark_gap = g.bleed.max(mm(3.0));
        let mark_len = mm(5.0);
        let margin = if pdf && opts.print.crop_marks { mark_gap + mark_len + mm(1.0) } else { g.bleed };
        let (w, h) = (g.size.0 + 2.0 * margin, g.size.1 + 2.0 * margin);
        let k = ((rect.width() as f64 - 28.0) / w).min((rect.height() as f64 - 28.0) / h);
        let page = Rect::from_center_size(rect.center(), vec2((g.size.0 * k) as f32, (g.size.1 * k) as f32));
        let bleed = page.expand((g.bleed * k) as f32);
        // Formats that keep transparency get a checkerboard, paper is white.
        if opts.format.is_raster() && opts.format != Format::Jpg || opts.format == Format::Svg {
            crate::color::checkerboard(&painter.with_clip_rect(bleed), bleed, 8.0);
        } else {
            painter.rect_filled(bleed, 0, Color32::WHITE);
        }
        painter.image(tex.id(), bleed, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        if g.bleed > 0.0 {
            // Dim what gets trimmed off and outline the trim.
            let dim = Color32::from_black_alpha(90);
            for r in [
                Rect::from_min_max(bleed.min, pos2(bleed.max.x, page.min.y)),
                Rect::from_min_max(pos2(bleed.min.x, page.max.y), bleed.max),
                Rect::from_min_max(pos2(bleed.min.x, page.min.y), pos2(page.min.x, page.max.y)),
                Rect::from_min_max(pos2(page.max.x, page.min.y), pos2(bleed.max.x, page.max.y)),
            ] {
                painter.rect_filled(r, 0, dim);
            }
            painter.rect_stroke(page, 0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Middle);
        }
        if pdf && opts.print.crop_marks {
            let stroke = Stroke::new(1.0, theme::TEXT);
            let (gap, len) = ((mark_gap * k) as f32, (mark_len * k) as f32);
            for (x, sx) in [(page.min.x, -1.0), (page.max.x, 1.0)] {
                for (y, sy) in [(page.min.y, -1.0), (page.max.y, 1.0)] {
                    painter.line_segment([pos2(x + sx * gap, y), pos2(x + sx * (gap + len), y)], stroke);
                    painter.line_segment([pos2(x, y + sy * gap), pos2(x, y + sy * (gap + len))], stroke);
                }
            }
        }
    }
    if let Some(e) = &p.error {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(theme::FONT_MD), TEXT_DIM);
    }
    let busy = !p.page_ready || p.pending.is_some() || (p.size.is_none() && p.error.is_none());
    if busy {
        let spin = Rect::from_min_size(rect.right_top() + vec2(-26.0, 10.0), vec2(16.0, 16.0));
        // A child: `ui.put` would move the caption up here.
        ui.new_child(egui::UiBuilder::new().max_rect(spin)).add(egui::Spinner::new().size(14.0));
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if g.pages > 1 {
            if widgets::mini_icon(ui, icon::CARET_LEFT, TEXT_DIM).clicked() {
                p.page = p.page.saturating_sub(1);
            }
            ui.label(RichText::new(format!("{} / {}", p.page.min(g.pages - 1) + 1, g.pages)).size(theme::FONT_SM).color(TEXT_DIM));
            if widgets::mini_icon(ui, icon::CARET_RIGHT, TEXT_DIM).clicked() {
                p.page = (p.page + 1).min(g.pages - 1);
            }
        }
        ui.label(RichText::new(describe(&opts, g)).size(theme::FONT_SM).color(TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = match p.size {
                Some(Some(b)) => human_size(b),
                _ => "…".into(),
            };
            widgets::tip(ui.label(RichText::new(text).size(theme::FONT_SM).color(theme::TEXT)), "Size of the exported file with these settings", None);
        });
    });
}

fn describe(o: &Options, g: Geometry) -> String {
    let mm = |v: f64| v / g.dpi.max(1.0) * 25.4;
    if !g.size.0.is_finite() || g.size.0 <= 0.0 {
        return String::new();
    }
    if o.format.is_raster() {
        return format!("{} × {} px", (g.size.0 * o.scale).round(), (g.size.1 * o.scale).round());
    }
    let mut s = format!("{:.1} × {:.1} mm", mm(g.size.0), mm(g.size.1));
    if o.format == Format::Pdf {
        if g.bleed > 0.0 {
            s += &format!(" + {:.1} mm bleed", mm(g.bleed));
        }
        if o.print.color == crate::print::ColorOutput::Cmyk {
            s += " · CMYK proof";
        }
    }
    s
}
