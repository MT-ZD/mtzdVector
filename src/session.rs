//! Protecting work: the unsaved-changes prompt before replacing or closing
//! the document, periodic autosave and crash recovery.

use std::path::PathBuf;

use crate::app::App;
use crate::fonts::data_dir;


/// An action that would discard the current document.
#[derive(Clone, Debug, PartialEq)]
pub enum Guarded {
    New,
    /// Open a file (`None` shows the file dialog).
    Open(Option<PathBuf>),
    /// Create from the start screen's template settings.
    Template,
    /// Close the active tab.
    CloseTab,
    Quit,
}

impl Guarded {
    pub fn verb(&self) -> &'static str {
        match self {
            Guarded::New | Guarded::Template => "creating a new document",
            Guarded::Open(_) => "opening another document",
            Guarded::CloseTab => "closing it",
            Guarded::Quit => "quitting",
        }
    }
}

/// A recovery file left behind by a session that didn't exit cleanly.
#[derive(Clone, Debug)]
pub struct Recovery {
    pub file: PathBuf,
    /// Document the autosave belonged to (None for an untitled document).
    pub original: Option<PathBuf>,
    pub modified: std::time::SystemTime,
}

#[derive(Default)]
pub struct Session {
    /// Action waiting for the user's answer to the unsaved-changes prompt.
    pub prompt: Option<Guarded>,
    /// Quit already confirmed: let the next close request through.
    pub allow_close: bool,
    pub last_autosave: f64,
    pub recovery: Option<Recovery>,
}

fn recovery_dir() -> PathBuf {
    data_dir().join("recovery")
}

/// Autosave of one tab of this session.
fn own_file(uid: u64) -> PathBuf {
    recovery_dir().join(format!("autosave-{}-{uid}.mvec", std::process::id()))
}

fn meta_of(file: &std::path::Path) -> PathBuf {
    file.with_extension("json")
}

fn enabled() -> bool {
    !cfg!(test) && crate::recent::ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether the process that wrote an autosave is still running.
fn pid_alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    } else {
        false
    }
}

/// Newest autosave from a session that is no longer running.
pub fn find_recovery() -> Option<Recovery> {
    if !enabled() {
        return None;
    }
    let mut best: Option<Recovery> = None;
    for e in std::fs::read_dir(recovery_dir()).ok()?.flatten() {
        let file = e.path();
        if file.extension().and_then(|x| x.to_str()) != Some("mvec") {
            continue;
        }
        let pid = file.file_stem().and_then(|s| s.to_str()).and_then(|s| s.strip_prefix("autosave-")).and_then(|s| s.split('-').next()).and_then(|s| s.parse::<u32>().ok());
        if pid.map(pid_alive).unwrap_or(false) || pid == Some(std::process::id()) {
            continue;
        }
        let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        let original = std::fs::read(meta_of(&file)).ok().and_then(|d| serde_json::from_slice::<Option<PathBuf>>(&d).ok()).flatten();
        if best.as_ref().map(|b| modified > b.modified).unwrap_or(true) {
            best = Some(Recovery { file, original, modified });
        }
    }
    best
}

pub fn discard(r: &Recovery) {
    let _ = std::fs::remove_file(&r.file);
    let _ = std::fs::remove_file(meta_of(&r.file));
}

/// Remove a tab's autosave (after a save or a deliberate discard).
pub fn clear_own(uid: u64) {
    if !enabled() {
        return;
    }
    let f = own_file(uid);
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_file(meta_of(&f));
}

/// Remove every autosave of this session (clean exit).
pub fn clear_all_own() {
    if !enabled() {
        return;
    }
    let prefix = format!("autosave-{}-", std::process::id());
    for e in std::fs::read_dir(recovery_dir()).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

impl App {
    /// Run `action` now, or ask first when it would lose unsaved changes.
    /// New and opened documents get their own tab, so only closing asks.
    pub fn guarded(&mut self, action: Guarded) {
        let ctx = self.egui_ctx.clone();
        let ctx = &ctx;
        match action {
            Guarded::CloseTab if self.dirty => self.session.prompt = Some(action),
            Guarded::Quit if self.any_dirty() => self.ask_next_quit(),
            _ => self.perform(action, ctx),
        }
    }

    /// Quitting: show the prompt for the next tab with unsaved changes.
    fn ask_next_quit(&mut self) {
        if let Some(i) = (0..self.tabs.len()).find(|&i| self.tab_dirty(i)) {
            self.switch_tab(i);
            self.session.prompt = Some(Guarded::Quit);
        }
    }

    /// Carry out a guarded action (changes are discarded).
    pub fn perform(&mut self, action: Guarded, ctx: &egui::Context) {
        match action {
            Guarded::New => {
                self.tab_for_new();
                self.new_document();
            }
            Guarded::Open(Some(p)) => self.open_path(p),
            Guarded::Open(None) => self.open_dialog(),
            Guarded::Template => {
                self.tab_for_new();
                self.create_from_template();
            }
            Guarded::CloseTab => self.close_tab(self.tab),
            Guarded::Quit => {
                if self.any_dirty() {
                    // Another tab still has unsaved changes: ask about it next.
                    self.ask_next_quit();
                    return;
                }
                clear_all_own();
                self.session.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The user's answer to the prompt: Some(true) save, Some(false) discard,
    /// None cancel.
    pub fn answer_prompt(&mut self, save: Option<bool>, ctx: &egui::Context) {
        let Some(action) = self.session.prompt.take() else { return };
        match save {
            None => {}
            Some(true) => {
                self.save(false);
                // Save dialog cancelled or failed: keep the document.
                if !self.dirty {
                    self.perform(action, ctx);
                }
            }
            Some(false) => {
                self.dirty = false;
                clear_own(self.doc_uid);
                self.perform(action, ctx);
            }
        }
    }

    /// Per-frame: intercept window close with unsaved changes, autosave.
    pub fn session_tick(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.any_dirty() && !self.session.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.ask_next_quit();
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.any_dirty() {
            clear_all_own();
        }
        let every = self.settings.autosave_secs;
        if self.dirty && enabled() && every > 0.0 && self.now - self.session.last_autosave > every {
            self.session.last_autosave = self.now;
            self.autosave();
        }
    }

    pub fn autosave(&mut self) {
        let f = own_file(self.doc_uid);
        let _ = std::fs::create_dir_all(recovery_dir());
        if crate::io::save_native(&self.doc, &f).is_ok() {
            let _ = std::fs::write(meta_of(&f), serde_json::to_vec(&self.file_path).unwrap_or_default());
        }
    }

    /// Load a recovery file as an unsaved document.
    pub fn recover(&mut self, r: Recovery) {
        match crate::io::load_native(&r.file) {
            Ok(doc) => {
                self.tab_for_new();
                self.new_document();
                self.doc = doc;
                self.file_path = r.original.clone();
                self.dirty = true;
                self.ui.welcome_open = false;
                self.notify("Recovered unsaved changes — save to keep them");
                discard(&r);
                self.autosave();
            }
            Err(e) => self.notify(format!("Could not recover: {e}")),
        }
    }
}
