//! MCP (Model Context Protocol) server, so AI agents can drive the app.
//!
//! `mtzd-vector --mcp [--live | --headless] [FILE]` speaks MCP over stdio
//! (newline-delimited JSON-RPC). Tool calls go to the open mtzdVector window
//! when there is one (through the socket in [`live`]), so the user watches
//! the agent work and can undo it; otherwise to a hidden instance that
//! renders offscreen. `--live` / `--headless` force one or the other.
//!
//! Register it with an MCP client, e.g.
//! `claude mcp add mtzd-vector -- /path/to/mtzd-vector --mcp`.

#[cfg(unix)]
pub mod live;
pub mod anim;
pub mod edit;
pub mod tools;
pub mod ui_tree;
pub mod install;

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

const PROTOCOLS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Auto,
    Live,
    Headless,
}

enum Backend {
    #[cfg(unix)]
    Live { read: BufReader<std::os::unix::net::UnixStream>, write: std::os::unix::net::UnixStream },
    Headless(Box<crate::headless::Session>),
}

struct Server {
    mode: Mode,
    /// The agent's name (from `initialize`), shown in the window.
    client: String,
    file: Option<PathBuf>,
    backend: Option<Backend>,
}

impl Server {
    fn connect(&mut self) -> Result<&mut Backend, String> {
        if self.backend.is_none() {
            #[cfg(unix)]
            if self.mode != Mode::Headless
                && let Some(s) = live::socket_path().and_then(|p| std::os::unix::net::UnixStream::connect(p).ok())
            {
                let write = s.try_clone().map_err(|e| e.to_string())?;
                self.backend = Some(Backend::Live { read: BufReader::new(s), write });
                let client = self.client.clone();
                let _ = self.forward(live::HELLO, &json!({"client": client}));
                if let Some(f) = self.file.take() {
                    let _ = self.forward("open_file", &json!({"path": f}));
                }
            }
            if self.backend.is_none() {
                if self.mode == Mode::Live {
                    return Err("mtzdVector isn't running (start it, or use the server with --headless)".into());
                }
                crate::recent::ENABLED.store(false, std::sync::atomic::Ordering::Relaxed);
                let opts = crate::headless::Options { out: PathBuf::new(), file: self.file.take(), size: [1600, 1000], frames: 0, scale: 1.0, script: None };
                let mut s = crate::headless::Session::new(&opts)?;
                #[cfg(unix)]
                {
                    s.app.mcp.inspect = true;
                }
                // Let the layout settle and the view fit.
                s.frames(12);
                self.backend = Some(Backend::Headless(Box::new(s)));
            }
        }
        Ok(self.backend.as_mut().unwrap())
    }

    #[cfg(unix)]
    fn forward(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        let Some(Backend::Live { read, write }) = &mut self.backend else { return Err("not connected".into()) };
        let mut msg = json!({"tool": tool, "args": args}).to_string();
        msg.push('\n');
        let mut line = String::new();
        let ok = write.write_all(msg.as_bytes()).is_ok() && read.read_line(&mut line).map(|n| n > 0).unwrap_or(false);
        if !ok {
            // Reconnect on the next call (the window may be back by then).
            self.backend = None;
            return Err("lost the connection to the mtzdVector window (was it closed?)".into());
        }
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }

    fn call(&mut self, tool: &str, args: &Value) -> Value {
        let backend = match self.connect() {
            Ok(b) => b,
            Err(e) => return tools::result(Err(e)),
        };
        match backend {
            #[cfg(unix)]
            Backend::Live { .. } => {
                let r = self.forward(tool, args).unwrap_or_else(|e| tools::result(Err(e)));
                let off = r["isError"] == json!(true) && r["content"][0]["text"] == json!(live::OFF);
                if off && self.mode == Mode::Auto {
                    // The user keeps agents out of the window: use a hidden one.
                    self.mode = Mode::Headless;
                    self.backend = None;
                    return self.call(tool, args);
                }
                r
            }
            Backend::Headless(s) => {
                let s = &mut **s;
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let ctx = s.ctx.clone();
                    match tools::call(&mut s.app, &ctx, &mut s.ptr, tool, args, "headless") {
                        Ok(tools::Outcome::Done(c)) => {
                            s.frames(2);
                            Ok(c)
                        }
                        Ok(tools::Outcome::Later(batches, then)) => {
                            for b in batches {
                                s.frame(b);
                            }
                            if let tools::After::Screenshot(max) = then {
                                // Let fade-ins finish.
                                s.frames(10);
                                let png = s.render_png()?;
                                return Ok(vec![tools::Content::Png(shrink(png, max)), tools::Content::Text("1 px = 1 window point (the coordinates of input / ui_tree), unless max_width shrank it".into())]);
                            }
                            if let tools::After::Export(_) = then {
                                while s.app.export_job.is_some() {
                                    std::thread::sleep(std::time::Duration::from_millis(50));
                                    s.frame(Vec::new());
                                }
                            }
                            s.frame(Vec::new());
                            tools::after(&mut s.app, &then, "headless")
                        }
                        Err(e) => Err(e),
                    }
                }));
                tools::result(run.unwrap_or_else(|p| {
                    let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                    Err(format!("internal error: {msg}"))
                }))
            }
        }
    }

    /// Answer one JSON-RPC message (None for notifications).
    fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned()?;
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => {
                if let Some(n) = params["clientInfo"]["title"].as_str().or(params["clientInfo"]["name"].as_str()) {
                    self.client = pretty_client(n);
                }
                let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
                let version = if PROTOCOLS.contains(&asked) { asked } else { PROTOCOLS[2] };
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "mtzd-vector", "title": "mtzdVector", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": tools::INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tools::list()}),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                self.call(&name, &args)
            }
            _ => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("unknown method {method}")}})),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }
}

/// A PNG scaled down to `max` pixels wide.
fn shrink(png: Vec<u8>, max: Option<u32>) -> Vec<u8> {
    let Some(max) = max else { return png };
    let Ok(img) = image::load_from_memory(&png) else { return png };
    if img.width() <= max {
        return png;
    }
    let img = img.resize(max, u32::MAX, image::imageops::FilterType::Triangle);
    let mut out = Vec::new();
    match img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png) {
        Ok(()) => out,
        Err(_) => png,
    }
}

/// "claude-code" → "Claude Code".
fn pretty_client(n: &str) -> String {
    if n.chars().any(|c| c.is_uppercase() || c == ' ') {
        return n.to_string();
    }
    n.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Protocol output: the real stdout. Everything else the app prints goes to
/// stderr so it can't corrupt the stream.
fn protocol_out() -> Box<dyn Write> {
    #[cfg(unix)]
    unsafe {
        use std::os::fd::FromRawFd;
        let fd = libc::dup(1);
        if fd >= 0 && libc::dup2(2, 1) >= 0 {
            return Box::new(std::fs::File::from_raw_fd(fd));
        }
    }
    Box::new(std::io::stdout())
}

pub fn run(args: &[String]) -> i32 {
    let mut out = protocol_out();
    let mode = if args.iter().any(|a| a == "--live") {
        Mode::Live
    } else if args.iter().any(|a| a == "--headless") {
        Mode::Headless
    } else {
        Mode::Auto
    };
    let file = args.iter().find(|a| !a.starts_with("--")).map(|f| std::path::absolute(f).unwrap_or_else(|_| f.into()));
    let mut server = Server { mode, client: "Agent".into(), file, backend: None };
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})),
        };
        if let Some(r) = reply {
            let mut s = r.to_string();
            s.push('\n');
            if out.write_all(s.as_bytes()).and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests;
