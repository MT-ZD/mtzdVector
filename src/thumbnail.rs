//! `--thumbnail IN OUT [SIZE]`: a PNG preview of a document's first
//! artboard for file managers, rendered on the CPU (no window, no GPU, no
//! user data written). `-` reads the document from stdin / writes the PNG
//! to stdout, so sandboxed builds need no access to the file's location.

use std::io::{Read, Write};

const TIMEOUT_SECS: u64 = 15;
const MAX_INPUT: u64 = 256 << 20;

/// Render `data` (a native document) to a PNG fitting `size`×`size`.
pub fn render(data: &[u8], size: u32) -> Result<Vec<u8>, String> {
    // Saved files carry a preview: scale it instead of rendering.
    if size <= crate::io::native::PREVIEW_PX {
        if let Some(img) = crate::io::native::embedded_preview(data).and_then(|p| image::load_from_memory(&p).ok()) {
            let img = if img.width().max(img.height()) > size { img.thumbnail(size, size) } else { img };
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            return Ok(out.into_inner());
        }
    }
    let doc = crate::io::parse_native(data)?;
    let ab = doc.primary();
    let side = ab.w.max(ab.h).max(1e-6);
    let scale = size.clamp(16, 2048) as f64 / side;
    let mut fonts = crate::fonts::FontManager::new(egui::Context::default());
    let img = crate::io::export::rasterize(&doc, &mut fonts, scale)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

pub fn run(args: &[String]) -> Result<(), String> {
    let (input, output) = match args {
        [i, o, ..] => (i.as_str(), o.as_str()),
        _ => return Err("usage: mtzd-vector --thumbnail INPUT|- OUTPUT.png|- [SIZE]".into()),
    };
    let size = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(256);
    // File managers call this for every file shown: never hang or balloon.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(TIMEOUT_SECS));
        eprintln!("thumbnail: timed out");
        std::process::exit(3);
    });
    let data = if input == "-" {
        let mut v = Vec::new();
        std::io::stdin().take(MAX_INPUT + 1).read_to_end(&mut v).map_err(|e| e.to_string())?;
        if v.len() as u64 > MAX_INPUT {
            return Err("document too large for a thumbnail".into());
        }
        v
    } else {
        // Thumbnailers may pass a file:// URI.
        let path = input.strip_prefix("file://").map(url_decode).unwrap_or_else(|| input.to_string());
        if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > MAX_INPUT {
            return Err("document too large for a thumbnail".into());
        }
        std::fs::read(path).map_err(|e| e.to_string())?
    };
    let png = render(&data, size)?;
    if output == "-" {
        std::io::stdout().write_all(&png).map_err(|e| e.to_string())
    } else {
        std::fs::write(output, png).map_err(|e| e.to_string())
    }
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
