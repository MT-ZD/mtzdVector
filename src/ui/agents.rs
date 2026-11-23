//! Help › AI Agents: connect AI agents (Claude Code, Cursor, …) to the app
//! through its MCP server with one click, see who is connected and what
//! they did, and switch agent access off.

use std::sync::mpsc::Receiver;

use egui::{Color32, CornerRadius, RichText, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;

use super::dialogs::{footer, modal, modal_text, modal_title};
use super::theme::{self, ACCENT, TEXT, TEXT_DIM, TEXT_FAINT};
use crate::app::App;
use crate::mcp::install::{self, Agent, Status};

#[derive(Default)]
pub struct AgentsUi {
    pub is_open: bool,
    statuses: Vec<(Agent, Status)>,
    /// Add / remove running in the background.
    job: Option<(Agent, Receiver<Result<String, String>>)>,
    /// Last outcome: (text, is error).
    message: Option<(String, bool)>,
}

impl AgentsUi {
    pub fn open(&mut self) {
        self.is_open = true;
        self.refresh();
    }

    fn refresh(&mut self) {
        self.statuses = Agent::ALL.iter().map(|a| (*a, a.status())).collect();
    }

    fn start(&mut self, agent: Agent, add: bool, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(if add { agent.add() } else { agent.remove() });
            ctx.request_repaint();
        });
        self.job = Some((agent, rx));
        self.message = None;
    }

    fn poll(&mut self) {
        if let Some((_, rx)) = &self.job
            && let Ok(r) = rx.try_recv()
        {
            self.job = None;
            self.message = Some(match r {
                Ok(m) => (m, false),
                Err(e) => (e, true),
            });
            self.refresh();
        }
    }
}

/// Connected agents and recent calls (empty outside Unix).
fn connected() -> Vec<String> {
    #[cfg(unix)]
    return crate::mcp::live::clients();
    #[cfg(not(unix))]
    Vec::new()
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title.to_uppercase()).size(10.5).strong().color(TEXT_FAINT));
    ui.add_space(4.0);
}

fn small_button(ui: &mut Ui, label: &str, primary: bool, enabled: bool) -> egui::Response {
    let text = RichText::new(label).size(theme::FONT_MD);
    let b = if primary { egui::Button::new(text.color(Color32::WHITE)).fill(ACCENT).stroke(Stroke::NONE) } else { egui::Button::new(text) };
    ui.add_enabled(enabled, b.corner_radius(5).min_size(vec2(72.0, 26.0)))
}

/// One agent: icon, name, state, and its action button.
fn card(app: &mut App, ui: &mut Ui, agent: Agent, status: &Status, width: f32) {
    let busy = app.ui.agents.job.as_ref().is_some_and(|j| j.0 == agent);
    let any_busy = app.ui.agents.job.is_some();
    egui::Frame::new().fill(theme::BG_INPUT).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 9)).show(ui, |ui| {
        ui.set_width(width - 26.0);
        ui.horizontal(|ui| {
            let installed = *status != Status::Missing;
            let tint = if installed { TEXT } else { TEXT_FAINT };
            ui.label(RichText::new(agent.icon()).size(20.0).color(if *status == Status::Added { theme::SUCCESS } else { tint }));
            ui.add_space(4.0);
            ui.vertical(|ui| {
                // Leave room for the button on the right.
                ui.set_max_width(width - 150.0);
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(agent.name()).size(theme::FONT_LG).strong().color(tint));
                let (text, color) = match status {
                    Status::Added => (format!("{} Added", icon::CHECK), theme::SUCCESS),
                    Status::NotAdded => ("Not added yet".into(), TEXT_DIM),
                    Status::Missing => ("Not installed".into(), TEXT_FAINT),
                    Status::Manual(why) => (why.clone(), TEXT_DIM),
                };
                let r = ui.add(egui::Label::new(RichText::new(text).size(theme::FONT_SM).color(color)).truncate());
                super::widgets::tip(r, &agent.config_path().display().to_string(), None);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if busy {
                    ui.add(egui::Spinner::new().size(16.0));
                    return;
                }
                match status {
                    Status::Added => {
                        let r = small_button(ui, "Remove", false, !any_busy);
                        super::widgets::describe(&r, egui::accesskit::Role::Button, &format!("Remove from {}", agent.name()), None);
                        if super::widgets::tip(r, &format!("Remove mtzdVector from {}", agent.name()), None).clicked() {
                            app.ui.agents.start(agent, false, ui.ctx());
                        }
                    }
                    Status::NotAdded => {
                        let r = small_button(ui, "Add", true, !any_busy);
                        super::widgets::describe(&r, egui::accesskit::Role::Button, &format!("Add to {}", agent.name()), None);
                        let how = if agent == Agent::ClaudeCode { "Runs `claude mcp add` for your user".to_string() } else { format!("Adds mtzdVector to {} (the original is kept as a .mtzd-backup)", agent.config_path().display()) };
                        if super::widgets::tip(r, &how, None).clicked() {
                            app.ui.agents.start(agent, true, ui.ctx());
                        }
                    }
                    Status::Manual(_) => {
                        let r = small_button(ui, "Copy", false, true);
                        if super::widgets::tip(r, "Copy the server entry to paste into its config", None).clicked() {
                            ui.ctx().copy_text(install::snippet());
                            app.notify("Copied the MCP server JSON");
                        }
                    }
                    Status::Missing => {
                        small_button(ui, "Add", false, false);
                    }
                }
            });
        });
    });
}

fn fmt_ago(secs: u64) -> String {
    match secs {
        0..=4 => "now".into(),
        5..=59 => format!("{secs} s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        _ => format!("{} h ago", secs / 3600),
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.ui.agents.is_open {
        return;
    }
    app.ui.agents.poll();
    let mut close = false;
    let r = modal("agents_dialog").show(ctx, |ui| {
        let w = 600.0;
        ui.set_width(w);
        modal_title(ui, icon::ROBOT, "AI Agents");
        modal_text(
            ui,
            "AI agents like Claude Code can draw and edit here through MCP, and use any part of the interface the way you do. You see every change as it happens, and each one is a single undo step.",
        );
        ui.add_space(12.0);

        // Connection status and the master switch.
        let clients = connected();
        egui::Frame::new().fill(theme::BG_INPUT).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(w - 26.0);
            ui.horizontal(|ui| {
                let on = app.settings.agent_control;
                let (dot, text) = if !on {
                    (TEXT_FAINT, "Agents can't use this window".to_string())
                } else if clients.is_empty() {
                    (TEXT_FAINT, "No agent connected".to_string())
                } else {
                    (theme::SUCCESS, format!("{} connected", clients.join(", ")))
                };
                let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.5, dot);
                ui.label(RichText::new(text).size(theme::FONT_LG).color(TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r = ui.checkbox(&mut app.settings.agent_control, "Let agents use this window");
                    super::widgets::tip_long(r, "Agent access", None, "When off, agents get a hidden instance of mtzdVector instead of this window");
                });
            });
        });

        section(ui, "Add to an agent");
        let statuses = app.ui.agents.statuses.clone();
        let col = (w - 10.0) / 2.0;
        ui.spacing_mut().item_spacing.y = 10.0;
        for pair in statuses.chunks(2) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (agent, status) in pair {
                    card(app, ui, *agent, status, col);
                }
            });
        }
        ui.spacing_mut().item_spacing.y = 6.0;
        if let Some((m, err)) = &app.ui.agents.message {
            ui.add_space(8.0);
            let (g, c) = if *err { (icon::WARNING_CIRCLE, theme::DANGER) } else { (icon::CHECK_CIRCLE, theme::SUCCESS) };
            ui.label(RichText::new(format!("{g}  {m}")).size(theme::FONT_MD).color(c));
        }

        section(ui, "Any other agent");
        let launch = install::launch();
        ui.horizontal(|ui| {
            egui::Frame::new().fill(theme::BG_INPUT).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
                ui.set_width(w - 292.0);
                ui.add(egui::Label::new(RichText::new(launch.shell()).monospace().size(theme::FONT_SM).color(TEXT)).truncate());
            });
            let copy = |ui: &mut Ui, app: &mut App, label: &str, tip: &str, text: String| {
                let r = small_button(ui, label, false, true);
                if super::widgets::tip(r, tip, None).clicked() {
                    ui.ctx().copy_text(text);
                    app.notify(format!("Copied: {label}"));
                }
            };
            copy(ui, app, "Command", "Copy the command that starts the MCP server (stdio)", launch.shell());
            copy(ui, app, "JSON", "Copy an mcpServers entry, the config format most agents use", install::snippet());
            copy(ui, app, "claude mcp add", "Copy the Claude Code command line", install::claude_command());
        });

        #[cfg(unix)]
        if !app.mcp.activity.is_empty() {
            section(ui, "Recent activity");
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for a in app.mcp.activity.iter().take(20) {
                    ui.horizontal(|ui| {
                        let (g, c) = if a.error.is_some() { (icon::X_CIRCLE, theme::DANGER) } else { (icon::CHECK_CIRCLE, theme::SUCCESS) };
                        ui.label(RichText::new(g).color(c));
                        ui.add_sized([70.0, 16.0], egui::Label::new(RichText::new(fmt_ago(a.at.elapsed().as_secs())).size(theme::FONT_SM).color(TEXT_DIM)));
                        ui.label(RichText::new(&a.client).size(theme::FONT_SM).color(TEXT_DIM));
                        ui.label(RichText::new(&a.tool).monospace().size(theme::FONT_SM).color(TEXT));
                        if let Some(e) = &a.error {
                            ui.add(egui::Label::new(RichText::new(e).size(theme::FONT_SM).color(theme::DANGER)).truncate());
                        }
                    });
                }
            });
        }

        match footer(ui, &["Refresh"], &[], "Done").as_deref() {
            Some("Done") => close = true,
            Some(_) => app.ui.agents.refresh(),
            None => {}
        }
    });
    if r.should_close() {
        close = true;
    }
    if close {
        app.ui.agents.is_open = false;
    }
}
