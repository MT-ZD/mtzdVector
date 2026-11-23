//! One-click setup of the MCP server in AI agents: each agent's own CLI when
//! it has one, else its config file (JSON or TOML), edited in place with a
//! backup of the original next to it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

pub const NAME: &str = "mtzd-vector";

/// How an agent starts the server.
#[derive(Clone, Debug)]
pub struct Launch {
    pub command: String,
    pub args: Vec<String>,
}

pub fn launch() -> Launch {
    if let Ok(id) = std::env::var("FLATPAK_ID") {
        return Launch { command: "flatpak".into(), args: vec!["run".into(), id, "--mcp".into()] };
    }
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "mtzd-vector".into());
    Launch { command: exe, args: vec!["--mcp".into()] }
}

impl Launch {
    /// For a shell: `cmd --mcp`, quoted where needed.
    pub fn shell(&self) -> String {
        std::iter::once(&self.command).chain(&self.args).map(|a| quote(a)).collect::<Vec<_>>().join(" ")
    }
}

fn quote(a: &str) -> String {
    if a.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:+@".contains(c)) { a.to_string() } else { format!("'{}'", a.replace('\'', "'\\''")) }
}

/// The usual `mcpServers` snippet most agents accept.
pub fn snippet() -> String {
    let l = launch();
    serde_json::to_string_pretty(&json!({"mcpServers": {NAME: {"command": l.command, "args": l.args}}})).unwrap_or_default()
}

pub fn claude_command() -> String {
    format!("claude mcp add --scope user {NAME} -- {}", launch().shell())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
    VsCode,
    Windsurf,
    Zed,
    GeminiCli,
    Codex,
    OpenCode,
    LmStudio,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Added,
    NotAdded,
    /// The agent doesn't seem to be installed.
    Missing,
    /// Installed, but its config can't be edited automatically.
    Manual(String),
}

/// Where in a JSON config the server goes, and how the entry looks.
#[derive(Clone, Copy)]
enum Shape {
    /// `{"mcpServers": {name: {command, args}}}`
    McpServers,
    /// VS Code: `{"servers": {name: {type: stdio, command, args}}}`
    VsCode,
    /// Zed: `{"context_servers": {name: {source: custom, command, args}}}`
    Zed,
    /// opencode: `{"mcp": {name: {type: local, command: [..], enabled}}}`
    OpenCode,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

fn config() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| home().join(".config"))
}

impl Agent {
    pub const ALL: [Agent; 10] =
        [Agent::ClaudeCode, Agent::ClaudeDesktop, Agent::Cursor, Agent::VsCode, Agent::Windsurf, Agent::Zed, Agent::GeminiCli, Agent::Codex, Agent::OpenCode, Agent::LmStudio];

    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::ClaudeDesktop => "Claude Desktop",
            Agent::Cursor => "Cursor",
            Agent::VsCode => "VS Code",
            Agent::Windsurf => "Windsurf",
            Agent::Zed => "Zed",
            Agent::GeminiCli => "Gemini CLI",
            Agent::Codex => "Codex CLI",
            Agent::OpenCode => "opencode",
            Agent::LmStudio => "LM Studio",
        }
    }

    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            Agent::ClaudeCode => i::TERMINAL_WINDOW,
            Agent::ClaudeDesktop => i::SPARKLE,
            Agent::Cursor => i::CURSOR_CLICK,
            Agent::VsCode => i::CODE,
            Agent::Windsurf => i::WIND,
            Agent::Zed => i::LIGHTNING,
            Agent::GeminiCli => i::STAR_FOUR,
            Agent::Codex => i::OPEN_AI_LOGO,
            Agent::OpenCode => i::BRACKETS_CURLY,
            Agent::LmStudio => i::CUBE,
        }
    }

    /// The config file the server entry lives in.
    pub fn config_path(self) -> PathBuf {
        match self {
            Agent::ClaudeCode => home().join(".claude.json"),
            Agent::ClaudeDesktop => config().join("Claude/claude_desktop_config.json"),
            Agent::Cursor => home().join(".cursor/mcp.json"),
            Agent::VsCode => config().join("Code/User/mcp.json"),
            Agent::Windsurf => home().join(".codeium/windsurf/mcp_config.json"),
            Agent::Zed => config().join("zed/settings.json"),
            Agent::GeminiCli => home().join(".gemini/settings.json"),
            Agent::Codex => home().join(".codex/config.toml"),
            Agent::OpenCode => config().join("opencode/opencode.json"),
            Agent::LmStudio => home().join(".lmstudio/mcp.json"),
        }
    }

    fn cli(self) -> Option<&'static str> {
        match self {
            Agent::ClaudeCode => Some("claude"),
            Agent::GeminiCli => Some("gemini"),
            Agent::Codex => Some("codex"),
            Agent::OpenCode => Some("opencode"),
            Agent::Zed => Some("zed"),
            Agent::Cursor => Some("cursor"),
            Agent::VsCode => Some("code"),
            Agent::Windsurf => Some("windsurf"),
            _ => None,
        }
    }

    fn shape(self) -> Shape {
        match self {
            Agent::VsCode => Shape::VsCode,
            Agent::Zed => Shape::Zed,
            Agent::OpenCode => Shape::OpenCode,
            _ => Shape::McpServers,
        }
    }

    fn installed(self) -> bool {
        self.config_path().parent().is_some_and(|d| d.is_dir() && d != home()) || self.config_path().exists() || self.cli().and_then(which).is_some()
    }

    pub fn status(self) -> Status {
        if !self.installed() {
            return Status::Missing;
        }
        let path = self.config_path();
        if self == Agent::Codex {
            return match std::fs::read_to_string(&path) {
                Ok(t) => if toml_section(&t).is_some() { Status::Added } else { Status::NotAdded },
                Err(_) => Status::NotAdded,
            };
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return if self == Agent::ClaudeCode && which("claude").is_none() { Status::Manual("claude command not found".into()) } else { Status::NotAdded },
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => {
                if entry_of(&v, self.shape()).is_some() {
                    Status::Added
                } else if self == Agent::ClaudeCode && which("claude").is_none() {
                    Status::Manual("claude command not found".into())
                } else {
                    Status::NotAdded
                }
            }
            Err(_) => Status::Manual("Config has comments: copy the JSON".into()),
        }
    }

    /// Add the server. Returns a message for the user.
    pub fn add(self) -> Result<String, String> {
        let l = launch();
        match self {
            // Claude Code rewrites ~/.claude.json constantly: let it do the edit.
            Agent::ClaudeCode => {
                let mut args = vec!["mcp", "add", "--scope", "user", NAME, "--"];
                args.push(&l.command);
                args.extend(l.args.iter().map(|s| s.as_str()));
                run("claude", &args)?;
                Ok("Added to Claude Code. New sessions can use it (/mcp in a running one).".into())
            }
            Agent::Codex => {
                let path = self.config_path();
                let mut text = std::fs::read_to_string(&path).unwrap_or_default();
                if toml_section(&text).is_none() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    let args = l.args.iter().map(|a| toml_str(a)).collect::<Vec<_>>().join(", ");
                    text.push_str(&format!("\n[mcp_servers.{NAME}]\ncommand = {}\nargs = [{args}]\n", toml_str(&l.command)));
                    write(&path, &text)?;
                }
                Ok(format!("Added to {}. Restart it to pick it up.", self.name()))
            }
            _ => {
                json_add(&self.config_path(), self.shape(), &l)?;
                Ok(format!("Added to {}. Restart it (or reload its MCP servers) to pick it up.", self.name()))
            }
        }
    }

    pub fn remove(self) -> Result<String, String> {
        match self {
            Agent::ClaudeCode => {
                run("claude", &["mcp", "remove", "--scope", "user", NAME])?;
            }
            Agent::Codex => {
                let path = self.config_path();
                let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                if let Some(r) = toml_section(&text) {
                    let mut t = text[..r.start].trim_end().to_string();
                    let rest = text[r.end..].trim_start();
                    if !rest.is_empty() {
                        t.push_str("\n\n");
                        t.push_str(rest);
                    }
                    t.push('\n');
                    write(&path, &t)?;
                }
            }
            _ => json_remove(&self.config_path(), self.shape())?,
        }
        Ok(format!("Removed from {}.", self.name()))
    }
}

fn key_of(shape: Shape) -> &'static str {
    match shape {
        Shape::McpServers => "mcpServers",
        Shape::VsCode => "servers",
        Shape::Zed => "context_servers",
        Shape::OpenCode => "mcp",
    }
}

fn entry_of(v: &Value, shape: Shape) -> Option<&Value> {
    v.get(key_of(shape))?.get(NAME)
}

/// Put our entry into a JSON config, keeping everything else (and its order).
fn json_add(path: &Path, shape: Shape, l: &Launch) -> Result<(), String> {
    let mut v = read_json(path)?;
    let entry = match shape {
        Shape::McpServers => json!({"command": l.command, "args": l.args}),
        Shape::VsCode => json!({"type": "stdio", "command": l.command, "args": l.args}),
        Shape::Zed => json!({"source": "custom", "command": l.command, "args": l.args, "env": {}}),
        Shape::OpenCode => {
            let mut cmd = vec![l.command.clone()];
            cmd.extend(l.args.clone());
            json!({"type": "local", "command": cmd, "enabled": true})
        }
    };
    let key = key_of(shape);
    let obj = v.as_object_mut().ok_or("the config isn't a JSON object")?;
    let servers = obj.entry(key).or_insert_with(|| json!({}));
    servers.as_object_mut().ok_or(format!("`{key}` isn't an object"))?.insert(NAME.into(), entry);
    write(path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))
}

fn json_remove(path: &Path, shape: Shape) -> Result<(), String> {
    let mut v = read_json(path)?;
    if let Some(m) = v.get_mut(key_of(shape)).and_then(|s| s.as_object_mut()) {
        m.remove(NAME);
    }
    write(path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))
}

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(t) if t.trim().is_empty() => Ok(json!({})),
        Ok(t) => serde_json::from_str(&t).map_err(|_| format!("{} isn't plain JSON (comments?): add it by hand with Copy JSON", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write atomically; the first time, keep the original as `NAME.mtzd-backup`.
fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let mut backup = path.as_os_str().to_owned();
    backup.push(".mtzd-backup");
    if path.exists() && !Path::new(&backup).exists() {
        std::fs::copy(path, &backup).map_err(|e| format!("backup: {e}"))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".mtzd-tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Byte range of our `[mcp_servers.NAME]` table (and its sub-tables).
fn toml_section(text: &str) -> Option<std::ops::Range<usize>> {
    let heads = [format!("[mcp_servers.{NAME}]"), format!("[mcp_servers.\"{NAME}\"]")];
    let mut start = None;
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if let Some(s) = start {
            let ours = heads.iter().any(|h| t.starts_with(&h[..h.len() - 1]) && t[h.len() - 1..].starts_with('.'));
            if t.starts_with('[') && !ours {
                return Some(s..pos);
            }
        } else if heads.iter().any(|h| t == h) {
            start = Some(pos);
        }
        pos += line.len();
    }
    start.map(|s| s..text.len())
}

/// A command on PATH, or in the places agent installers put them (a desktop
/// launch often has a short PATH).
pub fn which(bin: &str) -> Option<PathBuf> {
    let h = home();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend([".local/bin", ".claude/local", ".npm-global/bin", ".bun/bin", ".cargo/bin", ".volta/bin", "bin"].iter().map(|d| h.join(d)));
    dirs.push("/usr/local/bin".into());
    dirs.into_iter().map(|d| d.join(bin)).find(|p| p.is_file())
}

fn run(bin: &str, args: &[&str]) -> Result<(), String> {
    let path = which(bin).ok_or(format!("`{bin}` not found"))?;
    let mut cmd = if std::env::var("FLATPAK_ID").is_ok() {
        let mut c = Command::new("flatpak-spawn");
        c.arg("--host").arg(&path);
        c
    } else {
        Command::new(&path)
    };
    // Its own directory first, so a `#!/usr/bin/env node` script finds a node next to it.
    if let (Some(dir), Some(p)) = (path.parent(), std::env::var_os("PATH")) {
        let mut paths = vec![dir.to_path_buf()];
        paths.extend(std::env::split_paths(&p));
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
    }
    let out = cmd.args(args).stdin(std::process::Stdio::null()).output().map_err(|e| format!("{bin}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout).to_string() } else { err.to_string() };
        Err(format!("{bin}: {}", msg.trim().lines().last().unwrap_or("failed")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_section_round_trip() {
        let t = "model = \"x\"\n\n[mcp_servers.other]\ncommand = \"a\"\n\n[mcp_servers.mtzd-vector]\ncommand = \"b\"\nargs = []\n\n[mcp_servers.mtzd-vector.env]\nA = \"1\"\n\n[profiles.p]\nx = 1\n";
        let r = toml_section(t).unwrap();
        assert!(t[r.clone()].starts_with("[mcp_servers.mtzd-vector]"));
        assert!(t[r.clone()].contains("env"));
        assert!(t[r.end..].starts_with("[profiles.p]"));
        assert!(toml_section("[mcp_servers.other]\n").is_none());
    }

    #[test]
    fn json_configs_keep_other_entries() {
        let dir = std::env::temp_dir().join(format!("mtzd-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("mcp.json");
        let l = Launch { command: "/bin/mtzd".into(), args: vec!["--mcp".into()] };
        // A missing file is created.
        json_add(&path, Shape::McpServers, &l).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"][NAME]["command"], "/bin/mtzd");
        // An existing one keeps its other servers and key order, with a backup.
        std::fs::write(&path, r#"{"z": 1, "mcpServers": {"other": {"command": "x"}}, "a": 2}"#).unwrap();
        json_add(&path, Shape::McpServers, &l).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.find("\"z\"").unwrap() < text.find("\"a\"").unwrap());
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert!(entry_of(&v, Shape::McpServers).is_some());
        assert!(dir.join("mcp.json.mtzd-backup").exists());
        json_remove(&path, Shape::McpServers).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(entry_of(&v, Shape::McpServers).is_none());
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        // Comments: refuse rather than lose them.
        std::fs::write(&path, "{ // hi\n }").unwrap();
        assert!(json_add(&path, Shape::Zed, &l).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quoting() {
        let l = Launch { command: "/opt/My Apps/mtzd-vector".into(), args: vec!["--mcp".into()] };
        assert_eq!(l.shell(), "'/opt/My Apps/mtzd-vector' --mcp");
    }
}
