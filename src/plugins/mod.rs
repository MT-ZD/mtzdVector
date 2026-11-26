//! Plugins: built-in extensions, compiled in and switched on or off in
//! Plugins › Manage Plugins…. A plugin adds commands (menus, palette,
//! shortcuts, MCP `run_command`), Studio tabs, its own windows, a canvas
//! overlay, and settings stored with the app (`Settings.plugins`) or with
//! the document (`Document.plugin_data`).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use egui::{KeyboardShortcut, Ui};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::App;

#[cfg(feature = "ruida")]
pub mod laser;
pub mod manage_ui;
#[cfg(all(test, feature = "ruida"))]
mod tests;

/// A command a plugin adds. Its id is local to the plugin.
#[derive(Clone, Copy, Debug)]
pub struct PluginCommand {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub keys: &'static [KeyboardShortcut],
    /// Other names for the palette (comma-separated).
    pub aliases: &'static str,
}

/// A tab a plugin adds to the Studio's upper group.
#[derive(Clone, Copy, Debug)]
pub struct PanelSpec {
    pub id: &'static str,
    pub title: &'static str,
}

/// A built-in plugin. Everything but its identity is optional.
#[allow(unused_variables)]
pub trait Plugin: Send + std::any::Any {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str {
        ""
    }
    fn icon(&self) -> &'static str {
        egui_phosphor::regular::PUZZLE_PIECE
    }
    /// On until the user switches it off.
    fn default_enabled(&self) -> bool {
        true
    }
    /// Its commands; asked once, at start.
    fn commands(&self) -> Vec<PluginCommand> {
        Vec::new()
    }
    fn run(&mut self, cmd: &str, app: &mut App, ctx: &egui::Context) {}
    fn enabled_cmd(&self, cmd: &str, app: &App) -> bool {
        true
    }
    fn checked_cmd(&self, cmd: &str, app: &App) -> Option<bool> {
        None
    }
    /// Its Studio tabs; asked once, at start.
    fn panels(&self) -> Vec<PanelSpec> {
        Vec::new()
    }
    fn panel_ui(&mut self, panel: &str, app: &mut App, ui: &mut Ui) {}
    /// Its own windows and modals, once a frame.
    fn windows(&mut self, app: &mut App, ctx: &egui::Context) {}
    /// Once a frame, before the UI (poll workers, …).
    fn tick(&mut self, app: &mut App, ctx: &egui::Context) {}
    /// Drawn over the artwork (document → screen with `app.view`).
    fn canvas_overlay(&mut self, app: &App, painter: &egui::Painter) {}
    fn load_settings(&mut self, data: &Value) {}
    fn save_settings(&self) -> Value {
        Value::Null
    }
    fn on_enable(&mut self, app: &mut App) {}
    fn on_disable(&mut self, app: &mut App) {}
}

/// The compiled-in plugins, in menu order.
#[allow(clippy::vec_init_then_push)]
pub fn builtin() -> Vec<Box<dyn Plugin>> {
    #[allow(unused_mut)]
    let mut v: Vec<Box<dyn Plugin>> = Vec::new();
    #[cfg(feature = "ruida")]
    v.push(Box::new(laser::LaserPlugin::default()));
    v
}

/// What `Settings.plugins` keeps per plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginEntry {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

/// A plugin command in the global table (`Cmd::Plugin(index)`).
pub struct CmdEntry {
    pub plugin: usize,
    pub plugin_id: &'static str,
    pub cmd: PluginCommand,
}

struct Registry {
    /// Per plugin: name and icon (for menus).
    names: Vec<(&'static str, &'static str)>,
    cmds: Vec<CmdEntry>,
    panels: Vec<(usize, PanelSpec)>,
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    let plugins = builtin();
    let mut cmds = Vec::new();
    let mut panels = Vec::new();
    for (i, p) in plugins.iter().enumerate() {
        cmds.extend(p.commands().into_iter().map(|cmd| CmdEntry { plugin: i, plugin_id: p.id(), cmd }));
        panels.extend(p.panels().into_iter().map(|s| (i, s)));
    }
    Registry { names: plugins.iter().map(|p| (p.name(), p.icon())).collect(), cmds, panels }
});

/// The plugin command behind `Cmd::Plugin(i)`.
pub fn command(i: u16) -> Option<&'static CmdEntry> {
    REGISTRY.cmds.get(i as usize)
}

/// A plugin's name and icon, by index.
pub fn plugin_label(i: usize) -> Option<(&'static str, &'static str)> {
    REGISTRY.names.get(i).copied()
}

/// Every plugin command (`Cmd::all` lists them; `PluginHost::cmd_on`
/// tells which plugins are on).
pub fn all_commands() -> impl Iterator<Item = u16> {
    (0..REGISTRY.cmds.len()).map(|i| i as u16)
}

pub struct PluginHost {
    pub plugins: Vec<Box<dyn Plugin>>,
    pub enabled: Vec<bool>,
}

impl Default for PluginHost {
    /// Empty: what `App.plugins` holds while a plugin runs (see `with_plugins`).
    fn default() -> Self {
        PluginHost { plugins: Vec::new(), enabled: Vec::new() }
    }
}

impl PluginHost {
    pub fn new() -> Self {
        let plugins = builtin();
        let enabled = plugins.iter().map(|p| p.default_enabled()).collect();
        PluginHost { plugins, enabled }
    }

    pub fn is_on(&self, i: usize) -> bool {
        self.enabled.get(i).copied().unwrap_or(false)
    }

    #[allow(dead_code)] // for plugins talking to each other, and tests
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.plugins.iter().position(|p| p.id() == id)
    }

    /// Whether command `cmd`'s plugin is on.
    pub fn cmd_on(&self, cmd: u16) -> bool {
        command(cmd).is_some_and(|e| self.is_on(e.plugin))
    }

    /// Studio tabs of the plugins that are on: (plugin, panel).
    pub fn active_panels(&self) -> Vec<(usize, PanelSpec)> {
        REGISTRY.panels.iter().filter(|(p, _)| self.is_on(*p)).copied().collect()
    }

    /// Take enablement and plugin settings from loaded settings.
    pub fn load(&mut self, entries: &BTreeMap<String, PluginEntry>) {
        for (i, p) in self.plugins.iter_mut().enumerate() {
            if let Some(e) = entries.get(p.id()) {
                self.enabled[i] = e.enabled;
                p.load_settings(&e.data);
            }
        }
    }

    /// Write enablement and plugin settings back.
    pub fn store(&self, entries: &mut BTreeMap<String, PluginEntry>) {
        for (i, p) in self.plugins.iter().enumerate() {
            let e = PluginEntry { enabled: self.enabled[i], data: p.save_settings() };
            if entries.get(p.id()) != Some(&e) {
                entries.insert(p.id().to_string(), e);
            }
        }
    }

    pub fn set_enabled(&mut self, i: usize, on: bool, app: &mut App) {
        if self.is_on(i) == on {
            return;
        }
        self.enabled[i] = on;
        if on {
            self.plugins[i].on_enable(app);
        } else {
            self.plugins[i].on_disable(app);
        }
    }

    pub fn run(&mut self, cmd: u16, app: &mut App, ctx: &egui::Context) {
        if let Some(e) = command(cmd)
            && self.is_on(e.plugin)
            && let Some(p) = self.plugins.get_mut(e.plugin)
        {
            p.run(e.cmd.id, app, ctx);
        }
    }

    pub fn enabled_cmd(&self, cmd: u16, app: &App) -> bool {
        command(cmd).is_some_and(|e| self.is_on(e.plugin) && self.plugins.get(e.plugin).is_some_and(|p| p.enabled_cmd(e.cmd.id, app)))
    }

    pub fn checked_cmd(&self, cmd: u16, app: &App) -> Option<bool> {
        command(cmd).filter(|e| self.is_on(e.plugin)).and_then(|e| self.plugins.get(e.plugin)?.checked_cmd(e.cmd.id, app))
    }

    pub fn tick(&mut self, app: &mut App, ctx: &egui::Context) {
        for (i, p) in self.plugins.iter_mut().enumerate() {
            if self.enabled[i] {
                p.tick(app, ctx);
            }
        }
    }

    pub fn windows(&mut self, app: &mut App, ctx: &egui::Context) {
        for (i, p) in self.plugins.iter_mut().enumerate() {
            if self.enabled[i] {
                p.windows(app, ctx);
            }
        }
    }

    pub fn canvas_overlay(&mut self, app: &App, painter: &egui::Painter) {
        for (i, p) in self.plugins.iter_mut().enumerate() {
            if self.enabled[i] {
                p.canvas_overlay(app, painter);
            }
        }
    }

    pub fn panel_ui(&mut self, plugin: usize, panel: &str, app: &mut App, ui: &mut Ui) {
        if self.is_on(plugin)
            && let Some(p) = self.plugins.get_mut(plugin)
        {
            p.panel_ui(panel, app, ui);
        }
    }
}

impl App {
    /// Run `f` with the plugins taken out of the app, so a plugin can have
    /// `&mut App`. (Inside, `app.plugins` has no plugins, only which are on.)
    pub fn with_plugins<R>(&mut self, f: impl FnOnce(&mut PluginHost, &mut App) -> R) -> R {
        let stand_in = PluginHost { plugins: Vec::new(), enabled: self.plugins.enabled.clone() };
        let mut host = std::mem::replace(&mut self.plugins, stand_in);
        let r = f(&mut host, self);
        self.plugins = host;
        r
    }

    /// The plugin with this id, downcast (for tests and cross-plugin calls).
    #[allow(dead_code)]
    pub fn plugin_mut<T: 'static>(&mut self, id: &str) -> Option<&mut T> {
        let i = self.plugins.index_of(id)?;
        let p: &mut dyn Plugin = self.plugins.plugins[i].as_mut();
        (p as &mut dyn std::any::Any).downcast_mut::<T>()
    }

    /// Run `f` on the plugin with this id (downcast), with the app.
    #[allow(dead_code)]
    pub fn with_plugin<T: 'static, R>(&mut self, id: &str, f: impl FnOnce(&mut T, &mut App) -> R) -> Option<R> {
        let i = self.plugins.index_of(id)?;
        self.with_plugins(|h, app| {
            let p: &mut dyn Plugin = h.plugins[i].as_mut();
            (p as &mut dyn std::any::Any).downcast_mut::<T>().map(|p| f(p, app))
        })
    }

    /// Switch a plugin on or off by id.
    #[allow(dead_code)]
    pub fn set_plugin_enabled(&mut self, id: &str, on: bool) {
        if let Some(i) = self.plugins.index_of(id) {
            self.with_plugins(|h, app| h.set_enabled(i, on, app));
        }
    }
}
