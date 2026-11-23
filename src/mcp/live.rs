//! The running window's side of MCP: a Unix socket in the runtime dir that
//! `--mcp` processes connect to. Each line is a tool call
//! (`{"tool": …, "args": …}`) and gets one line back (a `CallToolResult`).
//! Calls run on the UI thread at the end of a frame.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::tools::{self, After, Content, Outcome};
use crate::app::App;

struct Request {
    conn: u64,
    tool: String,
    args: Value,
    reply: Sender<Value>,
}

static QUEUE: Mutex<Vec<Request>> = Mutex::new(Vec::new());

/// Connected agents: (connection, client name).
static CLIENTS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// Let MCP clients drive this window (Preferences / Agents dialog).
pub static ENABLED: AtomicBool = AtomicBool::new(true);

/// The answer while agent control is off (the server then goes headless).
pub const OFF: &str = "agent control is off in mtzdVector (Help › AI Agents)";

/// Hello line a server sends first: names the agent for the UI.
pub const HELLO: &str = "_hello";

pub fn socket_path() -> Option<PathBuf> {
    crate::platform::single::runtime_path("mcp")
}

/// Names of the connected agents.
pub fn clients() -> Vec<String> {
    CLIENTS.lock().unwrap().iter().map(|c| c.1.clone()).collect()
}

/// One tool call, for the activity list.
#[derive(Clone, Debug)]
pub struct Activity {
    pub at: Instant,
    pub client: String,
    pub tool: String,
    pub error: Option<String>,
}

struct Pending {
    reply: Sender<Value>,
    then: After,
    /// Frames to let pass before answering.
    settle: u32,
}

/// In-flight work that spans frames, and what the agents did lately.
#[derive(Default)]
pub struct State {
    inject: VecDeque<Vec<egui::Event>>,
    pending: Option<Pending>,
    /// Who waits for the screenshot, and the scale to give it.
    shot_reply: Option<(Sender<Value>, f32)>,
    ptr: crate::headless::Pointer,
    /// The interface as of the end of the last frame (while agents are connected).
    pub ui: Vec<super::ui_tree::UiNode>,
    pub activity: VecDeque<Activity>,
    accesskit: bool,
    /// Keep the widget tree on (a headless instance serving an agent).
    pub inspect: bool,
}

/// Start listening (once, in the primary window). `wake` repaints it.
pub fn listen(wake: impl Fn() + Send + Clone + 'static) {
    if let Some(path) = socket_path() {
        listen_at(path, wake);
    }
}

pub fn listen_at(path: PathBuf, wake: impl Fn() + Send + Clone + 'static) {
    // Another window (--new-window) already serves agents.
    if UnixStream::connect(&path).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let wake = wake.clone();
            std::thread::spawn(move || serve(stream, wake));
        }
    });
}

fn serve(stream: UnixStream, wake: impl Fn()) {
    let Ok(mut out) = stream.try_clone() else { return };
    let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    CLIENTS.lock().unwrap().push((conn, "Agent".into()));
    wake();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(v) if v.get("tool").and_then(|t| t.as_str()) == Some(HELLO) => {
                let name = v["args"]["client"].as_str().unwrap_or("Agent").chars().take(40).collect::<String>();
                if let Some(c) = CLIENTS.lock().unwrap().iter_mut().find(|c| c.0 == conn) {
                    c.1 = name;
                }
                wake();
                json!({"ok": true})
            }
            Ok(v) => {
                let (tx, rx) = channel();
                let tool = v.get("tool").and_then(|t| t.as_str()).unwrap_or("").to_string();
                QUEUE.lock().unwrap().push(Request { conn, tool, args: v.get("args").cloned().unwrap_or(json!({})), reply: tx });
                wake();
                rx.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|_| tools::result(Err("the window didn't answer in time".into())))
            }
            Err(e) => tools::result(Err(format!("bad request: {e}"))),
        };
        let mut msg = reply.to_string();
        msg.push('\n');
        if out.write_all(msg.as_bytes()).is_err() {
            break;
        }
    }
    CLIENTS.lock().unwrap().retain(|c| c.0 != conn);
    wake();
}

const SHOT_NOTE: &str = "1 px = 1 window point (the coordinates of input / ui_tree), unless max_width shrank it";

fn png(image: &egui::ColorImage, scale: f32) -> Result<Vec<u8>, String> {
    let [w, h] = image.size;
    let px: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    let mut img = image::RgbaImage::from_raw(w as u32, h as u32, px).ok_or("bad screenshot")?;
    if scale < 0.99 {
        let (nw, nh) = (((w as f32) * scale).round().max(1.0) as u32, ((h as f32) * scale).round().max(1.0) as u32);
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
    }
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out)
}

impl App {
    /// Before a frame: feed queued agent input, collect screenshots.
    pub fn mcp_raw_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        for e in &raw.events {
            if let egui::Event::Screenshot { image, .. } = e
                && let Some((tx, scale)) = self.mcp.shot_reply.take()
            {
                let _ = tx.send(tools::result(png(image, scale).map(|p| vec![Content::Png(p), Content::Text(SHOT_NOTE.into())])));
            }
        }
        if let Some(b) = self.mcp.inject.pop_front() {
            raw.events.extend(b);
            ctx.request_repaint();
        }
    }

    /// After a frame: snapshot the interface, answer finished work, run new calls.
    pub fn mcp_tick(&mut self, ctx: &egui::Context) {
        // The widget tree costs a little each frame: only while agents are around.
        let want = self.mcp.inspect || !CLIENTS.lock().unwrap().is_empty() || self.mcp.pending.is_some();
        if want != self.mcp.accesskit {
            self.mcp.accesskit = want;
            if want {
                ctx.enable_accesskit();
                ctx.request_repaint();
            } else {
                ctx.disable_accesskit();
                self.mcp.ui.clear();
            }
        }
        if want {
            self.mcp.ui = super::ui_tree::snapshot(ctx);
        }
        self.mcp_finish(ctx);
        let requests = std::mem::take(&mut *QUEUE.lock().unwrap());
        for r in requests {
            let client = CLIENTS.lock().unwrap().iter().find(|c| c.0 == r.conn).map(|c| c.1.clone()).unwrap_or_else(|| "Agent".into());
            if !ENABLED.load(Ordering::Relaxed) {
                let _ = r.reply.send(tools::result(Err(OFF.into())));
                continue;
            }
            let mut ptr = self.mcp.ptr;
            let out = tools::call(self, ctx, &mut ptr, &r.tool, &r.args, "live");
            self.mcp.ptr = ptr;
            let mut log = |error: Option<String>| {
                self.mcp.activity.push_front(Activity { at: Instant::now(), client: client.clone(), tool: r.tool.clone(), error });
                self.mcp.activity.truncate(40);
            };
            match out {
                Ok(Outcome::Done(c)) => {
                    log(None);
                    let _ = r.reply.send(tools::result(Ok(c)));
                }
                Ok(Outcome::Later(input, then)) => {
                    if self.mcp.pending.is_some() || self.mcp.shot_reply.is_some() {
                        // A long export blocks the agent's next deferred call, not the window.
                        log(Some("busy".into()));
                        let _ = r.reply.send(tools::result(Err("still busy with the previous call".into())));
                        continue;
                    }
                    log(None);
                    self.mcp.inject.extend(input);
                    self.mcp.pending = Some(Pending { reply: r.reply, then, settle: 1 });
                }
                Err(e) => {
                    log(Some(e.clone()));
                    let _ = r.reply.send(tools::result(Err(e)));
                }
            }
            ctx.request_repaint();
        }
    }

    /// Answer a deferred call once its input has been fed and a frame has shown the result.
    fn mcp_finish(&mut self, ctx: &egui::Context) {
        if !self.mcp.inject.is_empty() {
            return;
        }
        let Some(p) = &mut self.mcp.pending else { return };
        if p.settle > 0 {
            p.settle -= 1;
            ctx.request_repaint();
            return;
        }
        let p = self.mcp.pending.take().unwrap();
        match p.then {
            After::Screenshot(max) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                // Window points per screenshot pixel: 1, unless `max` asks for smaller.
                let ppp = ctx.pixels_per_point();
                let w = ctx.content_rect().width();
                let scale = max.map_or(1.0, |m| (m as f32 / w).min(1.0)) / ppp;
                self.mcp.shot_reply = Some((p.reply, scale));
                ctx.request_repaint();
            }
            After::Export(path) if self.export_job.is_some() => {
                // Still encoding: check again shortly.
                self.mcp.pending = Some(Pending { reply: p.reply, then: After::Export(path), settle: 0 });
                ctx.request_repaint_after(Duration::from_millis(150));
            }
            then => {
                let r = tools::after(self, &then, "live");
                let _ = p.reply.send(tools::result(r));
            }
        }
    }
}
