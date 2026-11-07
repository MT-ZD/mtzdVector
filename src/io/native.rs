//! The native `.mvec` format.
//!
//! Version 2+ is a zip container (like ODF):
//!
//! | entry           | content                                                   |
//! |-----------------|-----------------------------------------------------------|
//! | `mimetype`      | `application/x-mtzdvector`, stored first and uncompressed |
//! | `document.json` | the document, without image bytes                         |
//! | `images/ID.EXT` | each image asset in its original encoding                 |
//! | `preview.png`   | first artboard, at most `PREVIEW_PX` (for thumbnailers)    |
//!
//! Version 1 files are a single JSON document with base64 images; they still
//! load. Every document records `format_version`; loading migrates older
//! versions and refuses newer ones rather than dropping what it doesn't
//! understand. Saving is atomic: a temporary file in the same folder is
//! written, synced and renamed over the target.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::fonts::FontManager;

pub const MIME: &str = "application/x-mtzdvector";
/// Version written by this build.
pub const FORMAT_VERSION: u32 = 2;
/// Longest side of the embedded preview.
pub const PREVIEW_PX: u32 = 512;

// Limits against malformed or hostile files.
const MAX_JSON: u64 = 512 << 20;
const MAX_IMAGE: u64 = 256 << 20;
const MAX_ENTRIES: usize = 100_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct SaveOptions {
    /// Keep the previous file as `NAME.mvec~`.
    pub backup: bool,
    /// Embed `preview.png` (skipped for autosaves).
    pub preview: bool,
}

/// Is `data` a zip container (else legacy JSON)?
pub fn is_container(data: &[u8]) -> bool {
    data.starts_with(b"PK\x03\x04")
}

/// Encode a document as a container.
pub fn encode(doc: &Document, fonts: Option<&mut FontManager>, opts: SaveOptions) -> Result<Vec<u8>, String> {
    let mut stripped = doc.clone();
    stripped.format_version = FORMAT_VERSION;
    let mut images = Vec::new();
    for (id, img) in stripped.images.iter_mut() {
        let ext = image_ext(&img.encoded);
        images.push((format!("images/{id}.{ext}"), std::mem::take(&mut img.encoded)));
    }
    let json = serde_json::to_vec(&stripped).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6));
    let z = |e: zip::result::ZipError| e.to_string();
    zip.start_file("mimetype", stored).map_err(z)?;
    zip.write_all(MIME.as_bytes()).map_err(|e| e.to_string())?;
    zip.start_file("document.json", deflated).map_err(z)?;
    zip.write_all(&json).map_err(|e| e.to_string())?;
    for (name, bytes) in &images {
        // Image formats are already compressed.
        zip.start_file(name.as_str(), stored).map_err(z)?;
        zip.write_all(bytes).map_err(|e| e.to_string())?;
    }
    if let (true, Some(fonts)) = (opts.preview, fonts) {
        if let Ok(png) = preview_png(doc, fonts, PREVIEW_PX) {
            zip.start_file("preview.png", stored).map_err(z)?;
            zip.write_all(&png).map_err(|e| e.to_string())?;
        }
    }
    Ok(zip.finish().map_err(z)?.into_inner())
}

/// PNG of the first artboard fitting `size`×`size`.
pub fn preview_png(doc: &Document, fonts: &mut FontManager, size: u32) -> Result<Vec<u8>, String> {
    let ab = doc.primary();
    let scale = size.clamp(16, 4096) as f64 / ab.w.max(ab.h).max(1e-6);
    let img = super::export::rasterize(doc, fonts, scale)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn image_ext(bytes: &[u8]) -> &'static str {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "png",
        Ok(image::ImageFormat::Jpeg) => "jpg",
        Ok(image::ImageFormat::WebP) => "webp",
        Ok(image::ImageFormat::Gif) => "gif",
        Ok(image::ImageFormat::Bmp) => "bmp",
        _ => "bin",
    }
}

/// Decode a native document (container or legacy JSON).
pub fn decode(data: &[u8]) -> Result<Document, String> {
    let (json, mut images) = if is_container(data) { read_container(data)? } else { (data.to_vec(), Vec::new()) };
    let mut doc = parse_document(&json)?;
    for (id, bytes) in images.drain(..) {
        if let Some(img) = doc.images.get_mut(&id) {
            img.encoded = bytes;
        }
    }
    doc.images.retain(|_, img| !img.encoded.is_empty());
    doc.format_version = FORMAT_VERSION;
    doc.after_load();
    Ok(doc)
}

fn invalid(e: serde_json::Error) -> String {
    format!("Not a valid mtzdVector document: {e}")
}

/// Parse `document.json`: straight into the document when it is current,
/// through a JSON tree (migrated) when older.
fn parse_document(json: &[u8]) -> Result<Document, String> {
    #[derive(serde::Deserialize)]
    struct Version {
        format_version: Option<u32>,
    }
    let version = serde_json::from_slice::<Version>(json).map_err(invalid)?.format_version.unwrap_or(1);
    if version == FORMAT_VERSION {
        return serde_json::from_slice(json).map_err(invalid);
    }
    let mut value: serde_json::Value = serde_json::from_slice(json).map_err(invalid)?;
    migrate(&mut value)?;
    serde_json::from_value(value).map_err(invalid)
}

fn read_container(data: &[u8]) -> Result<(Vec<u8>, Vec<(u64, Vec<u8>)>), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| format!("Damaged document: {e}"))?;
    if zip.len() > MAX_ENTRIES {
        return Err("Damaged document: too many entries".into());
    }
    let json = read_entry(&mut zip, "document.json", MAX_JSON)?.ok_or("Damaged document: document.json is missing")?;
    let mut images = Vec::new();
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).filter(|n| n.starts_with("images/")).collect();
    for name in names {
        let stem = name.trim_start_matches("images/").split('.').next().unwrap_or("");
        let Ok(id) = stem.parse::<u64>() else { continue };
        if let Some(bytes) = read_entry(&mut zip, &name, MAX_IMAGE)? {
            images.push((id, bytes));
        }
    }
    Ok((json, images))
}

/// Read one entry, refusing ones larger than `limit` (zip bombs).
fn read_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(format!("Damaged document: {e}")),
    };
    if file.size() > limit {
        return Err(format!("Damaged document: {name} is too large"));
    }
    let mut out = Vec::with_capacity(file.size() as usize);
    file.take(limit + 1).read_to_end(&mut out).map_err(|e| format!("Damaged document: {e}"))?;
    if out.len() as u64 > limit {
        return Err(format!("Damaged document: {name} is too large"));
    }
    Ok(Some(out))
}

/// The embedded preview, if the file has one.
pub fn embedded_preview(data: &[u8]) -> Option<Vec<u8>> {
    if !is_container(data) {
        return None;
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).ok()?;
    read_entry(&mut zip, "preview.png", 64 << 20).ok().flatten()
}

/// Bring an older document up to `FORMAT_VERSION` (in place, as JSON).
fn migrate(v: &mut serde_json::Value) -> Result<(), String> {
    let version = v.get("format_version").and_then(|x| x.as_u64()).unwrap_or(1) as u32;
    if version > FORMAT_VERSION {
        return Err(format!(
            "This document was saved by a newer version of mtzdVector (format {version}; this version reads up to {FORMAT_VERSION}). Update the app to open it."
        ));
    }
    // 1 → 2: images moved out of the JSON into the container; the JSON
    // shape is otherwise unchanged (legacy base64 images still deserialize).
    if let Some(obj) = v.as_object_mut() {
        obj.insert("format_version".into(), FORMAT_VERSION.into());
    }
    Ok(())
}

/// Write `bytes` to `path` atomically: temp file in the same folder, sync,
/// rename. With `backup`, the previous file is kept as `NAME~`.
pub fn write_atomic(path: &Path, bytes: &[u8], backup: bool) -> std::io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let tmp: PathBuf = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        // Keep the original's permissions.
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        if backup && path.exists() {
            let bak = dir.join(format!("{name}~"));
            let _ = std::fs::remove_file(&bak);
            std::fs::hard_link(path, &bak).or_else(|_| std::fs::copy(path, &bak).map(|_| ()))?;
        }
        std::fs::rename(&tmp, path)?;
        // Make the rename durable.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub fn save(doc: &Document, path: &Path, fonts: Option<&mut FontManager>, opts: SaveOptions) -> Result<(), String> {
    let bytes = encode(doc, fonts, opts)?;
    write_atomic(path, &bytes, opts.backup).map_err(|e| format!("Could not save {}: {e}", path.display()))
}

pub fn load(path: &Path) -> Result<Document, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > 2 << 30 {
        return Err("File is too large".into());
    }
    decode(&std::fs::read(path).map_err(|e| e.to_string())?)
}
