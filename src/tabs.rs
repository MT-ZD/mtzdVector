//! Several open documents, one per tab. The active document lives in the
//! `App` fields the rest of the code works with. The other tabs keep their
//! state in a `DocTab`, which is swapped in when its tab is activated.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::app::{App, Drag};
use crate::document::{Document, History};
use crate::render::{GeoCache, View};

static NEXT_UID: AtomicU64 = AtomicU64::new(1);

pub fn next_uid() -> u64 {
    NEXT_UID.fetch_add(1, Ordering::Relaxed)
}

/// Per-document state of a tab.
pub struct DocTab {
    /// Stable identity (autosave file name).
    pub uid: u64,
    pub doc: Document,
    pub history: History,
    pub cache: GeoCache,
    pub selection: Vec<u64>,
    pub view: View,
    pub view_ready: bool,
    pub fit_passes: u32,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    pub active_artboard: u64,
    pub artboard_selected: bool,
    pub pivot: Option<(f64, f64)>,
    pub pivot_for: Vec<u64>,
    pub comp_sync: std::collections::HashMap<u64, (u64, u64)>,
    pub comp_rev: u64,
    pub last_dup: Option<(Vec<u64>, kurbo::Rect)>,
    pub gradient_edit: Option<u64>,
    pub gradient_stop: usize,
    pub last_autosave: f64,
}

impl Default for DocTab {
    fn default() -> Self {
        DocTab {
            uid: next_uid(),
            doc: Document::default(),
            history: History::default(),
            cache: GeoCache::default(),
            selection: Vec::new(),
            view: View { zoom: 0.5, origin: egui::vec2(0.0, 0.0) },
            view_ready: false,
            fit_passes: 0,
            file_path: None,
            dirty: false,
            active_artboard: 0,
            artboard_selected: false,
            pivot: None,
            pivot_for: Vec::new(),
            comp_sync: Default::default(),
            comp_rev: 0,
            last_dup: None,
            gradient_edit: None,
            gradient_stop: 0,
            last_autosave: 0.0,
        }
    }
}

impl DocTab {
    pub fn name(&self) -> String {
        doc_name(self.file_path.as_ref())
    }
}

pub fn doc_name(path: Option<&PathBuf>) -> String {
    path.and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into())
}

/// What the tab bar shows for one tab.
pub struct TabInfo {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub active: bool,
}

impl App {
    /// Swap the active document's state with `t`.
    fn swap_doc(&mut self, t: &mut DocTab) {
        use std::mem::swap;
        swap(&mut self.doc_uid, &mut t.uid);
        swap(&mut self.doc, &mut t.doc);
        swap(&mut self.history, &mut t.history);
        swap(&mut self.cache, &mut t.cache);
        swap(&mut self.selection, &mut t.selection);
        swap(&mut self.view, &mut t.view);
        swap(&mut self.view_ready, &mut t.view_ready);
        swap(&mut self.fit_passes, &mut t.fit_passes);
        swap(&mut self.file_path, &mut t.file_path);
        swap(&mut self.dirty, &mut t.dirty);
        swap(&mut self.active_artboard, &mut t.active_artboard);
        swap(&mut self.artboard_selected, &mut t.artboard_selected);
        swap(&mut self.pivot, &mut t.pivot);
        swap(&mut self.pivot_for, &mut t.pivot_for);
        swap(&mut self.comp_sync, &mut t.comp_sync);
        swap(&mut self.comp_rev, &mut t.comp_rev);
        swap(&mut self.last_dup, &mut t.last_dup);
        swap(&mut self.ui.gradient_edit, &mut t.gradient_edit);
        swap(&mut self.ui.gradient_stop, &mut t.gradient_stop);
        swap(&mut self.session.last_autosave, &mut t.last_autosave);
    }

    /// Drop in-progress interactions tied to the active document.
    fn reset_transient(&mut self) {
        self.finish_pen();
        self.finish_text_edit();
        self.builder = None;
        self.drag = Drag::None;
        self.press = None;
        self.dragging = false;
        self.hover = None;
        self.node_sel.clear();
        self.renaming = None;
        self.artboard_drag = None;
        self.layer_drag = None;
        self.guide_drag = None;
        self.snap_guides.clear();
        self.gap_guides.clear();
        self.ui.ctx_menu = None;
        self.ui.paint_open = None;
        self.paste_count = 0;
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn tab_infos(&self) -> Vec<TabInfo> {
        (0..self.tabs.len())
            .map(|i| {
                if i == self.tab {
                    TabInfo { name: doc_name(self.file_path.as_ref()), path: self.file_path.clone(), dirty: self.dirty, active: true }
                } else {
                    let t = &self.tabs[i];
                    TabInfo { name: t.name(), path: t.file_path.clone(), dirty: t.dirty, active: false }
                }
            })
            .collect()
    }

    pub fn tab_dirty(&self, i: usize) -> bool {
        if i == self.tab { self.dirty } else { self.tabs.get(i).map(|t| t.dirty).unwrap_or(false) }
    }

    pub fn any_dirty(&self) -> bool {
        (0..self.tabs.len()).any(|i| self.tab_dirty(i))
    }

    /// Make tab `i` the active document.
    pub fn switch_tab(&mut self, i: usize) {
        if i == self.tab || i >= self.tabs.len() {
            return;
        }
        self.reset_transient();
        let mut t = std::mem::take(&mut self.tabs[i]);
        self.swap_doc(&mut t);
        self.tabs[self.tab] = t;
        self.tab = i;
        self.ui.gradient_stop = self.ui.gradient_stop.min(16);
    }

    pub fn next_tab(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        if n > 1 {
            self.switch_tab((self.tab as isize + step).rem_euclid(n) as usize);
        }
    }

    /// Open an empty tab after the active one and switch to it (the caller
    /// fills in the document).
    pub fn push_tab(&mut self) {
        let at = self.tab + 1;
        self.tabs.insert(at, DocTab::default());
        // The stale placeholder of the active tab stays at `self.tab`.
        self.switch_tab(at);
    }

    /// The active tab is an untouched, unsaved blank document (reused
    /// instead of opening another tab).
    pub fn tab_is_blank(&self) -> bool {
        self.file_path.is_none() && !self.dirty && !self.history.can_undo() && self.doc.shapes().is_empty()
    }

    /// Switch to a tab for a new document unless the active one is blank.
    pub fn tab_for_new(&mut self) {
        if !self.tab_is_blank() {
            self.push_tab();
        }
    }

    /// Index of the tab that has `path` open.
    pub fn tab_with_path(&self, path: &std::path::Path) -> Option<usize> {
        let same = |p: &PathBuf| p == path || (p.canonicalize().ok().is_some() && p.canonicalize().ok() == path.canonicalize().ok());
        (0..self.tabs.len()).find(|&i| if i == self.tab { self.file_path.as_ref().is_some_and(same) } else { self.tabs[i].file_path.as_ref().is_some_and(same) })
    }

    /// Close tab `i`, asking first when it has unsaved changes.
    pub fn request_close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tab_dirty(i) {
            self.switch_tab(i);
            self.session.prompt = Some(crate::session::Guarded::CloseTab);
        } else {
            self.close_tab(i);
        }
    }

    /// Close tab `i` without asking. Closing the last tab leaves a blank
    /// document and shows the start screen.
    pub fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            crate::session::clear_own(self.doc_uid);
            self.reset_transient();
            self.new_document();
            self.doc_uid = next_uid();
            self.ui.welcome_open = true;
            return;
        }
        if i == self.tab {
            // Activate a neighbour first; tab `i` then holds the closed state.
            self.switch_tab(if i + 1 < self.tabs.len() { i + 1 } else { i - 1 });
        }
        let t = self.tabs.remove(i);
        crate::session::clear_own(t.uid);
        if self.tab > i {
            self.tab -= 1;
        }
    }

    /// Move tab `from` to position `to` (tab bar drag).
    pub fn move_tab(&mut self, from: usize, to: usize) {
        let n = self.tabs.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        self.tab = if self.tab == from {
            to
        } else if from < self.tab && to >= self.tab {
            self.tab - 1
        } else if from > self.tab && to <= self.tab {
            self.tab + 1
        } else {
            self.tab
        };
    }
}
