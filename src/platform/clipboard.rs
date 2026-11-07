//! System clipboard with real MIME types.
//!
//! Copy offers the selection three ways at once:
//! * `application/x-mtzd-vector` — lossless JSON (shapes + referenced images),
//! * `image/svg+xml` — the selection as SVG cropped to its bounds,
//! * `text/plain` — the same SVG as text.
//!
//! On Wayland compositors with the data-control protocol (KDE, wlroots) this
//! uses `wl-clipboard-rs`; elsewhere the app falls back to egui's text
//! clipboard plus an in-process copy.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use kurbo::{Affine, Rect};
use serde::{Deserialize, Serialize};

use crate::document::{Document, ImageAsset, Paint, Rgba, Shape};
use crate::fonts::FontManager;
use crate::{geom, io, render};

pub const MIME_INTERNAL: &str = "application/x-mtzd-vector";
pub const MIME_SVG: &str = "image/svg+xml";
pub const MIME_URI_LIST: &str = "text/uri-list";
const IMAGE_MIMES: [&str; 5] = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"];
const TEXT_MIMES: [&str; 4] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING"];

/// Lossless clipboard payload exchanged between app instances.
#[derive(Clone, Serialize, Deserialize)]
pub struct Payload {
    pub app: String,
    pub version: u32,
    pub shapes: Vec<Shape>,
    pub images: BTreeMap<u64, ImageAsset>,
    /// Origin of the artboard the shapes were copied from (None: loose).
    #[serde(default)]
    pub origin: Option<(f64, f64)>,
}

impl Payload {
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

/// Collect the given shapes (in z-order) and the images they reference.
pub fn build_payload(doc: &Document, ids: &[u64]) -> Payload {
    // In document order; children of booleans and groups too (unless an
    // ancestor is copied as well).
    let has_ancestor = |id: u64| {
        let mut p = doc.parent_of(id);
        while let Some(x) = p {
            if ids.contains(&x) {
                return true;
            }
            p = doc.parent_of(x);
        }
        false
    };
    let mut shapes: Vec<Shape> = doc.all_ids().into_iter().filter(|id| ids.contains(id) && !has_ancestor(*id)).filter_map(|id| doc.get(id).cloned()).collect();
    shapes.iter_mut().for_each(|s| s.rev = 0);
    let mut images = BTreeMap::new();
    for s in &shapes {
        if let Some(Paint::Image(f)) = &s.fill {
            if let Some(a) = doc.images.get(&f.image) {
                images.insert(f.image, a.clone());
            }
        }
    }
    Payload { app: "mtzd-vector".into(), version: 1, shapes, images, origin: None }
}

pub fn payload_to_json(p: &Payload) -> Vec<u8> {
    serde_json::to_vec(p).unwrap_or_default()
}

pub fn parse_payload(data: &[u8]) -> Result<Payload, String> {
    let mut p: Payload = serde_json::from_slice(data).map_err(|e| e.to_string())?;
    for img in p.images.values_mut() {
        img.ensure_decoded();
    }
    Ok(p)
}

/// Document-space bounds of shapes including half the stroke width.
pub fn shapes_bounds(shapes: &[Shape], fonts: &mut FontManager) -> Option<Rect> {
    let mut r = None;
    for s in shapes {
        let (path, _) = render::compute_outline(s, fonts);
        if let Some(b) = geom::path_bounds(&path) {
            let sw = s.stroke.as_ref().map(|st| st.reach()).unwrap_or(0.0);
            r = geom::union_rect(r, b.inflate(sw, sw));
        }
    }
    r
}

/// The payload's shapes as a standalone SVG cropped to their bounds.
pub fn payload_svg(p: &Payload, fonts: &mut FontManager) -> String {
    let b = shapes_bounds(&p.shapes, fonts).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
    let mut tmp = Document::default();
    tmp.set_size(b.width().max(1.0).ceil(), b.height().max(1.0).ceil());
    tmp.primary_mut().background = Rgba::WHITE.with_alpha(0.0);
    tmp.images = p.images.clone();
    let shift = Affine::translate((-b.x0, -b.y0));
    for s in &p.shapes {
        let mut s = s.clone();
        s.apply_transform(shift);
        tmp.add_shape(s);
    }
    io::svg::export(&tmp, fonts)
}

/// What we found on the clipboard, in priority order.
pub enum ClipContent {
    Internal(Payload),
    Svg(Vec<u8>),
    Files(Vec<PathBuf>),
    Image(Vec<u8>),
    /// Plain text that is not graphics.
    Text,
}

/// Choose the best MIME type to read from the offered ones.
pub fn choose_mime(offered: &[String]) -> Option<String> {
    let has = |m: &str| offered.iter().any(|o| o == m);
    if has(MIME_INTERNAL) {
        return Some(MIME_INTERNAL.into());
    }
    if has(MIME_SVG) {
        return Some(MIME_SVG.into());
    }
    if has(MIME_URI_LIST) {
        return Some(MIME_URI_LIST.into());
    }
    for m in IMAGE_MIMES {
        if has(m) {
            return Some(m.into());
        }
    }
    TEXT_MIMES.iter().find(|m| has(m)).map(|m| m.to_string())
}

/// Interpret clipboard bytes of the given MIME type.
pub fn decode(mime: &str, bytes: Vec<u8>) -> Option<ClipContent> {
    if mime == MIME_INTERNAL {
        return parse_payload(&bytes).ok().map(ClipContent::Internal);
    }
    if mime == MIME_SVG {
        return Some(ClipContent::Svg(bytes));
    }
    if mime == MIME_URI_LIST {
        let files = super::parse_uri_list(&String::from_utf8_lossy(&bytes));
        return (!files.is_empty()).then_some(ClipContent::Files(files));
    }
    if IMAGE_MIMES.contains(&mime) {
        return Some(ClipContent::Image(bytes));
    }
    let text = String::from_utf8(bytes).ok()?;
    if looks_like_svg(&text) {
        return Some(ClipContent::Svg(text.into_bytes()));
    }
    Some(ClipContent::Text)
}

pub fn looks_like_svg(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<svg") || (t.starts_with("<?xml") && t.contains("<svg"))
}

pub struct SystemClipboard {
    available: bool,
}

impl SystemClipboard {
    pub fn unavailable() -> Self {
        SystemClipboard { available: false }
    }

    /// Check whether the compositor offers a data-control protocol.
    pub fn probe() -> Self {
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                use wl_clipboard_rs::paste::{ClipboardType, Error, Seat, get_mime_types};
                let ok = match get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
                    Ok(_) | Err(Error::ClipboardEmpty) | Err(Error::NoMimeType) | Err(Error::NoSeats) => true,
                    Err(e) => {
                        eprintln!("mtzd-vector: rich clipboard unavailable ({e}); using text clipboard");
                        false
                    }
                };
                return SystemClipboard { available: ok };
            }
        }
        SystemClipboard { available: false }
    }

    pub fn available(&self) -> bool {
        self.available
    }

    /// Offer several representations at once.
    pub fn copy(&self, items: Vec<(&str, Vec<u8>)>) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if self.available {
            use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
            let sources = items
                .into_iter()
                .map(|(m, b)| MimeSource { source: Source::Bytes(b.into_boxed_slice()), mime_type: MimeType::Specific(m.to_string()) })
                .collect();
            return Options::new().copy_multi(sources).map_err(|e| e.to_string());
        }
        let _ = items;
        Err("rich clipboard unavailable".into())
    }

    /// Read the best representation currently on the clipboard.
    pub fn read(&self) -> Option<ClipContent> {
        #[cfg(target_os = "linux")]
        if self.available {
            use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents, get_mime_types};
            let offered: Vec<String> = get_mime_types(ClipboardType::Regular, Seat::Unspecified).ok()?.into_iter().collect();
            let mime = choose_mime(&offered)?;
            let (tx, rx) = std::sync::mpsc::channel();
            let m = mime.clone();
            // Read on a helper thread so a stuck source app can't freeze the UI.
            std::thread::spawn(move || {
                let r = get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Specific(&m)).ok().and_then(|(mut pipe, _)| {
                    let mut buf = Vec::new();
                    pipe.read_to_end(&mut buf).ok().map(|_| buf)
                });
                let _ = tx.send(r);
            });
            let bytes = rx.recv_timeout(Duration::from_millis(2000)).ok()??;
            return decode(&mime, bytes);
        }
        None
    }
}
