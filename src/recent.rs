//! Recently opened / saved documents (persisted in the data dir).

use std::path::{Path, PathBuf};

const MAX: usize = 12;

/// Off for headless/scripted runs so they don't touch the user's list.
pub static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn store() -> PathBuf {
    crate::fonts::data_dir().join("recent.json")
}

pub fn load() -> Vec<PathBuf> {
    std::fs::read(store())
        .ok()
        .and_then(|d| serde_json::from_slice::<Vec<PathBuf>>(&d).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.exists())
        .collect()
}

pub fn push(path: &Path) {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut list = load();
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(MAX);
    save(&list);
}

pub fn remove(path: &Path) {
    let mut list = load();
    list.retain(|p| p != path);
    save(&list);
}

fn save(list: &[PathBuf]) {
    if cfg!(test) || !ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let _ = std::fs::create_dir_all(crate::fonts::data_dir());
    if let Ok(d) = serde_json::to_vec_pretty(list) {
        let _ = std::fs::write(store(), d);
    }
}
