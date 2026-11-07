//! Preferences and the persisted workspace (panel layout, view toggles),
//! stored as settings.json in the data dir. Headless/test runs never write it.

use serde::{Deserialize, Serialize};

use crate::app::App;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // Workspace.
    pub studio_width: f32,
    pub dock_split: [f32; 1],
    pub dock_tabs: [usize; 2],
    pub dock_folded: [bool; 2],
    pub show_studio: bool,
    pub show_rulers: bool,
    pub show_guides: bool,
    pub show_grid: bool,
    pub snap: bool,
    // Preferences.
    /// Interface zoom (1.0 = 100 %).
    pub ui_scale: f32,
    /// Arrow-key nudge (px), and with Shift.
    pub nudge: f64,
    pub nudge_big: f64,
    /// Seconds between autosaves; 0 turns autosave off.
    pub autosave_secs: f64,
    pub default_unit: crate::units::Unit,
    /// Keep the previous version as `NAME.mvec~` when saving.
    pub keep_backup: bool,
    /// Colour picker look (wheel or square, HSL or RGB), shared by all pickers.
    pub picker: crate::color::PickerPrefs,
    /// Default text language (hyphenation, spelling).
    pub language: String,
    /// Underline misspelled words while editing text.
    pub spell_check: bool,
    /// Shared component libraries (.mvec files) listed in the Components tab.
    pub libraries: Vec<std::path::PathBuf>,
    /// Read drawing tablets directly (Wayland tablet-v2) for pen pressure.
    /// Takes effect on the next start.
    pub tablet: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            studio_width: 284.0,
            dock_split: [540.0],
            dock_tabs: [0, 0],
            dock_folded: [false; 2],
            show_studio: true,
            show_rulers: true,
            show_guides: true,
            show_grid: false,
            snap: true,
            ui_scale: 1.0,
            nudge: 1.0,
            nudge_big: 10.0,
            autosave_secs: 30.0,
            default_unit: crate::units::Unit::Px,
            keep_backup: false,
            picker: Default::default(),
            language: "en-us".into(),
            spell_check: true,
            libraries: Vec::new(),
            tablet: true,
        }
    }
}

/// Preferences dialog state.
#[derive(Default)]
pub struct PrefsState {
    pub open: bool,
}

fn store() -> std::path::PathBuf {
    crate::fonts::data_dir().join("settings.json")
}

fn enabled() -> bool {
    !cfg!(test) && crate::recent::ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn load() -> Settings {
    if !enabled() {
        return Settings::default();
    }
    std::fs::read(store()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

fn save(s: &Settings) {
    if !enabled() {
        return;
    }
    let _ = std::fs::create_dir_all(crate::fonts::data_dir());
    if let Ok(d) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(store(), d);
    }
}

impl App {
    /// Apply loaded settings to the UI state (startup, or Reset).
    pub fn apply_settings(&mut self, s: Settings, ctx: &egui::Context) {
        self.ui.group_h = s.dock_split;
        self.ui.tab = s.dock_tabs;
        self.ui.collapsed = s.dock_folded;
        self.ui.show_studio = s.show_studio;
        self.ui.show_rulers = s.show_rulers;
        self.ui.show_guides = s.show_guides;
        self.ui.show_grid = s.show_grid;
        self.snap = s.snap;
        crate::color::set_picker_prefs(ctx, s.picker);
        if (s.ui_scale - ctx.zoom_factor()).abs() > 1e-3 && enabled() {
            ctx.set_zoom_factor(s.ui_scale.clamp(0.5, 2.0));
        }
        self.settings = s.clone();
        self.settings_saved = s;
    }

    /// Per-frame: copy the live workspace into the settings and save when
    /// something changed (throttled).
    pub fn settings_tick(&mut self, ctx: &egui::Context) {
        let s = &mut self.settings;
        s.dock_split = self.ui.group_h;
        s.dock_tabs = self.ui.tab;
        s.dock_folded = self.ui.collapsed;
        s.show_studio = self.ui.show_studio;
        s.show_rulers = self.ui.show_rulers;
        s.show_guides = self.ui.show_guides;
        s.show_grid = self.ui.show_grid;
        s.snap = self.snap;
        s.picker = crate::color::picker_prefs(ctx);
        if (ctx.zoom_factor() - s.ui_scale).abs() > 1e-3 && enabled() {
            ctx.set_zoom_factor(s.ui_scale.clamp(0.5, 2.0));
        }
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        if self.settings != self.settings_saved && !pointer_down && self.now - self.settings_last_save > 1.0 {
            self.settings_last_save = self.now;
            save(&self.settings);
            self.settings_saved = self.settings.clone();
        }
    }
}
