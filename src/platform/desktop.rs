//! Linux desktop integration. Wayland compositors take a window's icon and
//! app name from the desktop entry matching its app id (winit can't set a
//! window icon there), and file managers need the `.mvec` MIME type, its
//! handler and a thumbnailer. Native runs install these for themselves in
//! the user's data dir, from the same files the Flatpak ships
//! (`packaging/`). Flatpak builds export all but the thumbnailer, which
//! Flatpak can't export: the app writes that one into the host's data dir
//! (it runs the app through `flatpak run`), like FreeCAD's `.thumbnailer`.
//! Dolphin, GNOME Files, Nemo and Thunar all read these files. Files are
//! only rewritten when their content changes.

use std::path::{Path, PathBuf};

pub const APP_ID: &str = "pro.mtzd.MtzdVector";
const SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

const DESKTOP: &str = include_str!("../../packaging/pro.mtzd.MtzdVector.desktop");
const MIME_XML: &str = include_str!("../../packaging/pro.mtzd.MtzdVector.xml");
const THUMBNAILER: &str = include_str!("../../packaging/pro.mtzd.MtzdVector.thumbnailer");

/// Files from earlier builds that used a different app id.
const LEGACY_ID: &str = "mtzd-vector";

/// The thumbnailer's file name: Dolphin lists it by this name under
/// Settings › Interface › Previews.
pub const THUMBNAILER_FILE: &str = "mtzdVector.thumbnailer";

/// The thumbnailer entry for the Flatpak, run on the host.
const FLATPAK_THUMBNAILER: &str = "[Thumbnailer Entry]\nTryExec=flatpak\nExec=flatpak run --command=mtzd-vector pro.mtzd.MtzdVector --thumbnail %i %o %s\nMimeType=application/x-mtzdvector;\n";

pub fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// Point `Exec=mtzd-vector …` / `TryExec=` lines at `exe`.
fn with_exe(template: &str, exe: &Path) -> String {
    let quoted = format!("\"{}\"", exe.display().to_string().replace('"', "\\\""));
    template
        .lines()
        .map(|l| {
            if let Some(rest) = l.strip_prefix("Exec=mtzd-vector") {
                format!("Exec={quoted}{rest}")
            } else if l.starts_with("TryExec=") {
                format!("TryExec={}", exe.display())
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Write `bytes` to `path` unless it already holds them. True if written.
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

/// What an install changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Installed {
    pub written: usize,
    pub removed: usize,
    /// The MIME database needs rebuilding.
    pub mime_changed: bool,
}

/// Write the logo into a hicolor tree (`DIR/NxN/apps/APP_ID.png`), for
/// packaging (the Flatpak build calls `--write-icons`).
pub fn write_icons(hicolor: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    for px in SIZES {
        let Some(png) = crate::logo::png(px) else { continue };
        n += write_if_changed(&hicolor.join(format!("{px}x{px}/apps/{APP_ID}.png")), &png)? as usize;
    }
    Ok(n)
}

/// Install (or refresh) the desktop entry, icons, MIME type and thumbnailer
/// under `root` (the XDG data home).
pub fn install_into(root: &Path, exe: &Path) -> std::io::Result<Installed> {
    let mut out = Installed::default();
    for px in SIZES {
        let Some(png) = crate::logo::png(px) else { continue };
        let icons = root.join(format!("icons/hicolor/{px}x{px}/apps"));
        out.written += write_if_changed(&icons.join(format!("{APP_ID}.png")), &png)? as usize;
        let old = icons.join(format!("{LEGACY_ID}.png"));
        if old.exists() {
            std::fs::remove_file(old)?;
            out.removed += 1;
        }
    }
    let apps = root.join("applications");
    out.written += write_if_changed(&apps.join(format!("{APP_ID}.desktop")), with_exe(DESKTOP, exe).as_bytes())? as usize;
    let old = apps.join(format!("{LEGACY_ID}.desktop"));
    if old.exists() {
        std::fs::remove_file(old)?;
        out.removed += 1;
    }
    out.mime_changed = write_if_changed(&root.join(format!("mime/packages/{APP_ID}.xml")), MIME_XML.as_bytes())?;
    out.written += out.mime_changed as usize;
    out.written += write_if_changed(&root.join(format!("thumbnailers/{THUMBNAILER_FILE}")), with_exe(THUMBNAILER, exe).as_bytes())? as usize;
    out.removed += remove_old_thumbnailer(root)?;
    Ok(out)
}

/// The thumbnailer from builds that named it after the app id.
fn remove_old_thumbnailer(root: &Path) -> std::io::Result<usize> {
    let old = root.join(format!("thumbnailers/{APP_ID}.thumbnailer"));
    if old.exists() {
        std::fs::remove_file(old)?;
        return Ok(1);
    }
    Ok(0)
}

/// Inside the Flatpak: register the thumbnailer under `root` (the host's
/// XDG data home), unless a native build's working one is there.
pub fn install_flatpak_thumbnailer(root: &Path) -> std::io::Result<Installed> {
    let mut out = Installed::default();
    let path = root.join(format!("thumbnailers/{THUMBNAILER_FILE}"));
    let native = std::fs::read_to_string(&path).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("TryExec=").map(str::to_string))).filter(|exe| exe != "flatpak");
    if native.is_some_and(|exe| Path::new(&exe).is_file()) {
        return Ok(out);
    }
    out.written += write_if_changed(&path, FLATPAK_THUMBNAILER.as_bytes())? as usize;
    out.removed += remove_old_thumbnailer(root)?;
    Ok(out)
}

/// Install for the running executable (inside Flatpak, only the
/// thumbnailer).
pub fn install() -> Result<Installed, String> {
    if in_flatpak() {
        // XDG_DATA_HOME points into the sandbox; the host's is under $HOME.
        let home = std::env::var_os("HOME").ok_or("no home directory")?;
        return install_flatpak_thumbnailer(&Path::new(&home).join(".local/share")).map_err(|e| e.to_string());
    }
    let root: PathBuf = dirs::data_dir().ok_or("no data directory")?;
    // An AppImage runs from a temporary mount; register the file itself.
    let exe = match std::env::var_os("APPIMAGE") {
        Some(p) => PathBuf::from(p),
        None => std::env::current_exe().map_err(|e| e.to_string())?,
    };
    let out = install_into(&root, &exe).map_err(|e| e.to_string())?;
    if out.mime_changed {
        // Best effort: rebuild the user MIME database so `.mvec` is known.
        let _ = std::process::Command::new("update-mime-database").arg(root.join("mime")).status();
    }
    Ok(out)
}
