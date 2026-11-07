//! Print colour: CMYK output profiles (ICC, found on the system or chosen
//! by the user), RGB → CMYK conversion through them (moxcms), and the
//! settings PDF export uses for print (CMYK, PDF/X-4, bleed, crop marks).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use moxcms::{ColorProfile, DataColorSpace, Layout, ProfileText, RenderingIntent, TransformOptions};

use crate::document::Rgba;

/// A CMYK output profile on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct CmykProfile {
    pub name: String,
    pub path: PathBuf,
}

fn icc_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["/usr/share/color/icc", "/usr/local/share/color/icc", "/usr/share/ghostscript/iccprofiles", "/app/share/color/icc", "/run/host/usr/share/color/icc", "/run/host/usr/share/ghostscript/iccprofiles"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Some(d) = dirs::data_dir() {
        v.push(d.join("icc"));
        v.push(d.join("color/icc"));
    }
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".color/icc"));
    }
    v
}

/// The profile's description tag (falls back to the file name).
pub fn profile_name(data: &[u8], path: &Path) -> String {
    let desc = ColorProfile::new_from_slice(data).ok().and_then(|p| match p.description {
        Some(ProfileText::PlainString(s)) => Some(s),
        Some(ProfileText::Localizable(v)) => v.iter().find(|l| l.language == "en").or(v.first()).map(|l| l.value.clone()),
        Some(ProfileText::Description(d)) => Some(if d.ascii_string.is_empty() { d.unicode_string } else { d.ascii_string }),
        None => None,
    });
    desc.map(|s| s.trim_matches(char::from(0)).trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
}

/// Whether `data` is a CMYK output (printer) profile, from its header.
pub fn is_cmyk_profile(data: &[u8]) -> bool {
    data.len() > 128 && &data[16..20] == b"CMYK" && &data[12..16] == b"prtr"
}

fn scan(dir: &Path, depth: usize, out: &mut Vec<CmykProfile>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 3 {
            scan(&p, depth + 1, out);
            continue;
        }
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext != "icc" && ext != "icm" {
            continue;
        }
        // The header is enough to tell; read the whole file only for CMYK ones.
        let mut head = [0u8; 132];
        let ok = std::fs::File::open(&p).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok();
        if !ok || !is_cmyk_profile(&head) {
            continue;
        }
        if let Ok(data) = std::fs::read(&p) {
            let name = profile_name(&data, &p);
            if !out.iter().any(|q| q.name == name) {
                out.push(CmykProfile { name, path: p });
            }
        }
    }
}

/// CMYK output profiles installed on the system (cached), FOGRA/GRACoL-like
/// press profiles first.
pub fn cmyk_profiles() -> &'static [CmykProfile] {
    static CACHE: OnceLock<Vec<CmykProfile>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut out = Vec::new();
        for d in icc_dirs() {
            scan(&d, 0, &mut out);
        }
        let rank = |n: &str| {
            let n = n.to_ascii_lowercase();
            if n.contains("fogra39") || n.contains("coated fogra") || n.contains("iso coated") {
                0
            } else if n.contains("fogra") || n.contains("gracol") || n.contains("swop") {
                1
            } else {
                2
            }
        };
        out.sort_by(|a, b| rank(&a.name).cmp(&rank(&b.name)).then(a.name.cmp(&b.name)));
        out
    })
}

/// sRGB → CMYK (and back, for previews) through an output profile; a plain
/// formula with grey-component replacement when there is none.
pub struct ColorConv {
    to: Option<Arc<moxcms::TransformF32Executor>>,
    #[cfg_attr(not(test), allow(dead_code))]
    from: Option<Arc<moxcms::TransformF32Executor>>,
    to8: Option<Arc<moxcms::Transform8BitExecutor>>,
    /// The ICC data (for PDF/X output intents).
    pub icc: Option<Arc<Vec<u8>>>,
    pub name: String,
}

impl ColorConv {
    pub fn naive() -> Self {
        ColorConv { to: None, from: None, to8: None, icc: None, name: "Generic CMYK".into() }
    }

    pub fn from_icc(data: Arc<Vec<u8>>, name: String) -> Result<Self, String> {
        let dst = ColorProfile::new_from_slice(&data).map_err(|e| format!("{e:?}"))?;
        if dst.color_space != DataColorSpace::Cmyk {
            return Err("Not a CMYK profile".into());
        }
        let srgb = ColorProfile::new_srgb();
        let opts = TransformOptions { rendering_intent: RenderingIntent::RelativeColorimetric, ..Default::default() };
        let to = srgb.create_transform_f32(Layout::Rgb, &dst, Layout::Rgba, opts).map_err(|e| format!("{e:?}"))?;
        let from = dst.create_transform_f32(Layout::Rgba, &srgb, Layout::Rgb, opts).ok();
        let to8 = srgb.create_transform_8bit(Layout::Rgb, &dst, Layout::Rgba, opts).ok();
        Ok(ColorConv { to: Some(to), from, to8, icc: Some(data), name })
    }

    pub fn from_path(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        let name = profile_name(&data, path);
        Self::from_icc(Arc::new(data), name)
    }

    /// CMYK (0..1 each) of an sRGB colour.
    pub fn cmyk(&self, c: Rgba) -> [f32; 4] {
        if let Some(t) = &self.to {
            let src = [c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0)];
            let mut dst = [0f32; 4];
            if t.transform(&src, &mut dst).is_ok() {
                return dst.map(|v| v.clamp(0.0, 1.0));
            }
        }
        naive_cmyk(c)
    }

    /// The sRGB colour a CMYK ink mix prints as.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rgb(&self, cmyk: [f32; 4]) -> Rgba {
        if let Some(t) = &self.from {
            let mut dst = [0f32; 3];
            if t.transform(&cmyk, &mut dst).is_ok() {
                return Rgba { r: dst[0].clamp(0.0, 1.0), g: dst[1].clamp(0.0, 1.0), b: dst[2].clamp(0.0, 1.0), a: 1.0 };
            }
        }
        naive_rgb(cmyk)
    }

    /// RGBA8 pixels → CMYK8 (alpha dropped).
    pub fn cmyk8(&self, rgba: &[u8]) -> Vec<u8> {
        let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        if let Some(t) = &self.to8 {
            let mut out = vec![0u8; rgb.len() / 3 * 4];
            if t.transform(&rgb, &mut out).is_ok() {
                return out;
            }
        }
        rgb.chunks_exact(3)
            .flat_map(|p| naive_cmyk(Rgba { r: p[0] as f32 / 255.0, g: p[1] as f32 / 255.0, b: p[2] as f32 / 255.0, a: 1.0 }).map(|v| (v * 255.0).round() as u8))
            .collect()
    }
}

/// CMYK by formula with full grey-component replacement.
pub fn naive_cmyk(c: Rgba) -> [f32; 4] {
    let (r, g, b) = (c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0));
    let k = 1.0 - r.max(g).max(b);
    if k >= 1.0 - 1e-6 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let d = 1.0 - k;
    [(1.0 - r - k) / d, (1.0 - g - k) / d, (1.0 - b - k) / d, k]
}

#[cfg_attr(not(test), allow(dead_code))] // previews, tests
pub fn naive_rgb(c: [f32; 4]) -> Rgba {
    let k = 1.0 - c[3];
    Rgba { r: (1.0 - c[0]) * k, g: (1.0 - c[1]) * k, b: (1.0 - c[2]) * k, a: 1.0 }
}

/// Output colour of a PDF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ColorOutput {
    #[default]
    Rgb,
    Cmyk,
}

/// Print settings of a PDF export.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PrintOptions {
    pub color: ColorOutput,
    /// CMYK profile path (None: generic conversion, no PDF/X).
    pub profile: Option<PathBuf>,
    /// PDF/X-4 (needs a CMYK profile for the output intent).
    pub pdfx: bool,
    /// Bleed around each page, in millimetres.
    pub bleed_mm: f64,
    pub crop_marks: bool,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions { color: ColorOutput::Rgb, profile: None, pdfx: false, bleed_mm: 0.0, crop_marks: false }
    }
}

impl PrintOptions {
    pub fn is_plain(&self) -> bool {
        self.color == ColorOutput::Rgb && !self.pdfx && self.bleed_mm <= 0.0 && !self.crop_marks
    }

    /// The converter for these options (PDF/X implies CMYK). Errors when the
    /// chosen profile can't be loaded.
    pub fn converter(&self) -> Result<Option<ColorConv>, String> {
        if self.color == ColorOutput::Rgb && !self.pdfx {
            return Ok(None);
        }
        match &self.profile {
            Some(p) => ColorConv::from_path(p).map(Some),
            None if self.pdfx => Err("PDF/X needs a CMYK output profile".into()),
            None => Ok(Some(ColorConv::naive())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naive_round_trip() {
        for c in [Rgba::BLACK, Rgba::WHITE, Rgba::from_hex("#ff0000").unwrap(), Rgba::from_hex("#3366cc").unwrap()] {
            let back = naive_rgb(naive_cmyk(c));
            assert!((back.r - c.r).abs() < 1e-5 && (back.g - c.g).abs() < 1e-5 && (back.b - c.b).abs() < 1e-5);
        }
        assert_eq!(naive_cmyk(Rgba::BLACK), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn icc_conversion_when_a_profile_is_installed() {
        let Some(p) = cmyk_profiles().first() else {
            eprintln!("no CMYK profile installed; skipped");
            return;
        };
        let conv = ColorConv::from_path(&p.path).unwrap();
        assert!(conv.icc.is_some());
        let white = conv.cmyk(Rgba::WHITE);
        assert!(white.iter().all(|v| *v < 0.03), "{} white = {white:?}", p.name);
        let red = conv.cmyk(Rgba::from_hex("#ff0000").unwrap());
        assert!(red[1] > 0.8 && red[2] > 0.8 && red[0] < 0.1, "{} red = {red:?}", p.name);
        // Rich black, not just K, and back to near black.
        let black = conv.cmyk(Rgba::BLACK);
        assert!(black[3] > 0.6, "{black:?}");
        let back = conv.rgb(black);
        assert!(back.r < 0.2 && back.g < 0.2 && back.b < 0.2, "{back:?}");
        let px = conv.cmyk8(&[255, 255, 255, 255, 0, 0, 0, 255]);
        assert_eq!(px.len(), 8);
        assert!(px[..4].iter().all(|v| *v < 8));
    }
}

#[cfg(test)]
#[test]
#[ignore]
fn list_profiles() {
    for p in cmyk_profiles() {
        println!("{} — {}", p.name, p.path.display());
    }
}

impl crate::app::App {
    /// Print every artboard (one page each) through the desktop's print
    /// dialog. The PDF goes to a private temporary file.
    pub fn print_document(&mut self) {
        if self.print_job.is_some() {
            self.notify("The print dialog is already open");
            return;
        }
        if cfg!(test) || !crate::recent::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
            // Headless runs never open system dialogs.
            self.notify("Printing is not available here");
            return;
        }
        let pages: Vec<crate::document::Document> = self.doc.artboards.iter().map(|a| self.doc.page(a.id)).collect();
        let title = self.file_path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        let pdf = match crate::io::pdf::export_pdf(&pages, &mut self.fonts, &crate::io::pdf::PdfOptions { real_text: true, title: title.clone(), ..Default::default() }) {
            Ok(p) => p,
            Err(e) => return self.notify(format!("Print failed: {e}")),
        };
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(format!("mtzd-print-{}", std::process::id()));
        let path = dir.join(format!("{}.pdf", title.replace(['/', '\\'], "-")));
        let write = std::fs::create_dir_all(&dir).and_then(|_| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            std::fs::write(&path, &pdf)
        });
        if let Err(e) = write {
            return self.notify(format!("Print failed: {e}"));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            #[cfg(target_os = "linux")]
            let portal = crate::platform::print_portal::portal_version().is_ok();
            #[cfg(not(target_os = "linux"))]
            let portal = false;
            let r = if portal {
                #[cfg(target_os = "linux")]
                let r = crate::platform::print_portal::print_pdf(&path, &title).map(|ok| if ok { PrintResult::Printed } else { PrintResult::Cancelled });
                #[cfg(not(target_os = "linux"))]
                let r = Ok(PrintResult::Cancelled);
                let _ = std::fs::remove_file(&path);
                r
            } else {
                // No print portal: print from the PDF viewer instead.
                open_in_viewer(&path).map(|_| PrintResult::Opened)
            };
            let _ = tx.send(r);
        });
        self.print_job = Some(rx);
    }

    pub fn print_tick(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.print_job else { return };
        match rx.try_recv() {
            Ok(r) => {
                self.print_job = None;
                match r {
                    Ok(PrintResult::Printed) => self.notify("Sent to the printer"),
                    Ok(PrintResult::Opened) => self.notify("No print dialog on this desktop: opened the PDF in your viewer to print from there"),
                    Ok(PrintResult::Cancelled) => {}
                    Err(e) => self.notify(format!("Print failed: {e}")),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(250)),
            Err(_) => self.print_job = None,
        }
    }
}

/// How a print request ended.
pub enum PrintResult {
    Printed,
    Cancelled,
    /// No print portal: the PDF was opened in the default viewer.
    Opened,
}

fn open_in_viewer(path: &Path) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(opener).arg(path).spawn().map(|_| ()).map_err(|e| format!("no print dialog or PDF viewer: {e}"))
}
