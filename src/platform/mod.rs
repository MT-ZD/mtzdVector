//! Platform integration that egui/winit does not provide: multi-MIME
//! clipboard (Wayland data-control) and Wayland drag & drop.

pub mod clipboard;
#[cfg(target_os = "linux")]
pub mod desktop;
#[cfg(unix)]
pub mod single;
#[cfg(target_os = "linux")]
pub mod dnd_wayland;
#[cfg(target_os = "linux")]
pub mod print_portal;
#[cfg(target_os = "linux")]
pub mod tablet_wayland;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

/// Clipboard shortcut seen by the platform keyboard listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKey {
    Copy,
    Cut,
    Paste,
}

#[derive(Debug)]
pub enum PlatformEvent {
    /// Drag entered / moved over the window (surface-local logical px).
    DragHover(egui::Pos2),
    DragLeave,
    DropFiles(Vec<PathBuf>, egui::Pos2),
    DropData { mime: String, bytes: Vec<u8>, pos: egui::Pos2 },
    /// Ctrl+C/X/V (with modifiers) as seen by our own `wl_keyboard`.
    Key { key: ClipKey, mods: egui::Modifiers },
}

/// Pen input from a drawing tablet (surface-local logical px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PenEvent {
    Motion(egui::Pos2),
    /// 0..1.
    Pressure(f32),
    Button { button: egui::PointerButton, pressed: bool },
    Leave,
}

pub struct Platform {
    rx: Option<Receiver<PlatformEvent>>,
    /// Tablet pens (Wayland tablet-v2), when enabled and available.
    pub pen_rx: Option<Receiver<PenEvent>>,
    /// Set once the Wayland worker listens to the keyboard; clipboard
    /// shortcuts then come from there instead of egui's Copy/Paste events.
    keyboard: Arc<AtomicBool>,
    pub clipboard: clipboard::SystemClipboard,
    /// Wayland DnD worker is running (else winit's X11 drop path is used).
    pub wayland_dnd: bool,
}

impl Platform {
    /// No native integration (tests, headless screenshots).
    pub fn none() -> Self {
        Platform { rx: None, pen_rx: None, keyboard: Arc::new(AtomicBool::new(false)), clipboard: clipboard::SystemClipboard::unavailable(), wayland_dnd: false }
    }

    pub fn init(cc: &eframe::CreationContext, tablet: bool) -> Self {
        let mut p = Platform::none();
        p.clipboard = clipboard::SystemClipboard::probe();
        #[cfg(target_os = "linux")]
        {
            use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
            let display = cc.display_handle().ok().map(|h| h.as_raw());
            let window = cc.window_handle().ok().map(|h| h.as_raw());
            if let (Some(RawDisplayHandle::Wayland(d)), Some(RawWindowHandle::Wayland(w))) = (display, window) {
                let flag = p.keyboard.clone();
                match dnd_wayland::start(d.display.as_ptr(), w.surface.as_ptr(), cc.egui_ctx.clone(), flag, tablet) {
                    Ok((rx, pen)) => {
                        p.rx = Some(rx);
                        p.pen_rx = pen;
                        p.wayland_dnd = true;
                    }
                    Err(e) => eprintln!("mtzd-vector: Wayland drag & drop unavailable: {e}"),
                }
            }
        }
        p
    }

    pub fn poll(&self) -> Vec<PlatformEvent> {
        self.rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default()
    }

    /// Clipboard shortcuts are delivered as `PlatformEvent::Key`.
    pub fn has_keyboard(&self) -> bool {
        self.keyboard.load(Ordering::Relaxed)
    }
}

/// Parse a `text/uri-list` payload into local file paths (percent-decoded).
pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let rest = l.strip_prefix("file://")?;
            // Strip an optional host ("file://localhost/…" or "file:///…").
            let path = if rest.starts_with('/') { rest } else { &rest[rest.find('/')?..] };
            Some(PathBuf::from(percent_decode(path)))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
