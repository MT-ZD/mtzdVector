//! Offscreen rendering and scripted interaction: runs the UI without creating
//! a window and renders frames with wgpu into PNGs.
//!
//! `mtzd-vector --screenshot out.png [file] [--size 1800x900] [--frames 40] [--scale 1.25]`
//! `mtzd-vector --script script.txt|- [file] [--size …] [--scale …]`
//!
//! UI state can be preset with MTZD_TOOL / MTZD_SELECT / MTZD_FONTS / MTZD_PICKER / MTZD_NODESEL /
//! MTZD_TAB / MTZD_PALETTE / MTZD_CTXMENU / MTZD_DROP (see `App::dev_setup`).
//!
//! Script commands (one per line, `#` comments). Coordinates are logical
//! screen points; prefix a command with `d` (`dmove`, `dclick`, `ddrag`) to
//! use document coordinates instead:
//! ```text
//! frames N | wait MS       run N idle frames | pump frames for MS of wall-clock time
//! tool NAME | select all|0,2 | cmd NAME (Cmd debug name or label, e.g. cmd Union, cmd "Opacity 50%")
//! move X Y | down [right] | up [right] | click X Y [right] | dblclick X Y
//! drag X1 Y1 X2 Y2 [steps] (left button)
//! mods shift alt ctrl | mods none    (held for following pointer events)
//! key NAME [ctrl] [shift] [alt] | type TEXT
//! shot out.png | dump      (prints tool, selection and shapes)
//! ```

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui_wgpu::{self, wgpu};

use crate::app::App;
use crate::ui::theme;

pub struct Options {
    pub out: PathBuf,
    pub file: Option<PathBuf>,
    pub size: [u32; 2],
    pub frames: u32,
    pub scale: f32,
    pub script: Option<String>,
}

struct Session {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    ctx: egui::Context,
    app: App,
    size: [u32; 2],
    scale: f32,
    last: Option<egui::FullOutput>,
    pos: egui::Pos2,
    mods: egui::Modifiers,
    /// Simulated time per frame (keeps double-click and tooltips deterministic).
    t: f64,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const MSAA: u32 = 4;

impl Session {
    fn new(opts: &Options) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| format!("device: {e}"))?;
        let renderer = egui_wgpu::Renderer::new(&device, FORMAT, egui_wgpu::RendererOptions { msaa_samples: MSAA, ..Default::default() });
        let ctx = egui::Context::default();
        theme::setup(&ctx);
        ctx.set_zoom_factor(opts.scale);
        let app = App::new(ctx.clone(), opts.file.clone());
        Ok(Session {
            device,
            queue,
            renderer,
            ctx,
            app,
            size: opts.size,
            scale: opts.scale,
            last: None,
            pos: egui::pos2(1.0, 1.0),
            mods: egui::Modifiers::NONE,
            t: 0.0,
        })
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.app.dev_setup();
        self.t += 1.0 / 30.0;
        let [w, h] = self.size;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32) / self.scale)),
            time: Some(self.t),
            focused: true,
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.frame_ui(ui));
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        for id in &out.textures_delta.free {
            self.renderer.free_texture(id);
        }
        out.textures_delta.clear();
        self.last = Some(out);
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame(Vec::new());
        }
    }

    fn move_to(&mut self, p: egui::Pos2) {
        self.pos = p;
        self.frame(vec![egui::Event::PointerMoved(p)]);
    }

    fn button(&mut self, b: egui::PointerButton, pressed: bool) {
        let e = egui::Event::PointerButton { pos: self.pos, button: b, pressed, modifiers: self.mods };
        self.frame(vec![e]);
    }

    fn render(&mut self, path: &str) -> Result<(), String> {
        let out = self.last.take().ok_or("no frame")?;
        let [w, h] = self.size;
        let jobs = self.ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: out.pixels_per_point };
        self.last = Some(out);
        let device = &self.device;
        let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let make_tex = |samples: u32, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let msaa_tex = make_tex(MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve_tex = make_tex(1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let msaa_view = msaa_tex.create_view(&Default::default());
        let resolve_view = resolve_tex.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut cmds = self.renderer.update_buffers(device, &self.queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("headless"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&resolve_view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        let row = (w * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            resolve_tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            extent,
        );
        cmds.push(encoder.finish());
        self.queue.submit(cmds);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
        let data = slice.get_mapped_range().map_err(|e| format!("{e:?}"))?;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            pixels.extend_from_slice(&data[y * row as usize..y * row as usize + (w * 4) as usize]);
        }
        image::RgbaImage::from_raw(w, h, pixels).ok_or("bad image")?.save(path).map_err(|e| e.to_string())
    }

    fn dump(&mut self) {
        println!("tool {:?}  selection {:?}  zoom {:.3}  canvas {:?}", self.app.tool, self.app.selection, self.app.view.zoom, self.app.canvas_rect);
        if let Some(m) = &self.app.ui.ctx_menu {
            println!("context menu open at {:?}", m.pos);
        }
        if let Some(te) = &self.app.text_edit {
            println!("editing #{} cursor {} anchor {} selected {:?}", te.id, te.cursor, te.anchor, self.app.selected_text());
        }
        for (i, s) in self.app.doc.shapes().iter().enumerate() {
            let b = self.app.cache.get(s.id).and_then(|c| c.bounds).map(|b| format!("{:.1},{:.1} {:.1}x{:.1}", b.x0, b.y0, b.width(), b.height())).unwrap_or_default();
            println!("  [{i}] #{} {} '{}' {b} opacity {:.2}", s.id, s.kind.type_name(), s.name, s.opacity);
        }
    }

    fn doc_to_screen(&self, x: f32, y: f32) -> egui::Pos2 {
        self.app.view.to_screen(kurbo::Point::new(x as f64, y as f64))
    }

    fn run_line(&mut self, line: &str) -> Result<(), String> {
        let words = split_words(line);
        let Some(cmd) = words.first().map(|s| s.as_str()) else { return Ok(()) };
        let nums: Vec<f32> = words[1..].iter().filter_map(|w| w.parse().ok()).collect();
        let doc = matches!(cmd, "dmove" | "dclick" | "ddrag");
        let base = if doc { &cmd[1..] } else { cmd };
        let pt = |s: &Self, i: usize| -> Result<egui::Pos2, String> {
            let (x, y) = (*nums.get(i).ok_or("missing x")?, *nums.get(i + 1).ok_or("missing y")?);
            Ok(if doc { s.doc_to_screen(x, y) } else { egui::pos2(x, y) })
        };
        let right = words.iter().any(|w| w == "right");
        let btn = if right { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
        match base {
            "frames" => self.frames(nums.first().copied().unwrap_or(1.0) as u32),
            // Let background work (downloads) finish: wall-clock wait while pumping frames.
            "wait" => {
                let until = std::time::Instant::now() + std::time::Duration::from_millis(nums.first().copied().unwrap_or(500.0) as u64);
                while std::time::Instant::now() < until {
                    self.frames(1);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
            }
            "tool" => {
                let name = words.get(1).ok_or("tool name")?.to_lowercase();
                let t = crate::commands::Cmd::TOOLS
                    .iter()
                    .filter_map(|c| if let crate::commands::Cmd::Tool(t) = c { Some(*t) } else { None })
                    .chain(crate::prims::PrimKind::ALL.into_iter().map(crate::app::Tool::Prim))
                    .find(|t| [format!("{t:?}"), t.name().to_string(), format!("{t:?}").trim_start_matches("Prim(").trim_end_matches(')').to_string()].iter().any(|n| n.to_lowercase() == name))
                    .ok_or(format!("unknown tool {name}"))?;
                self.app.set_tool(t);
                self.frame(Vec::new());
            }
            "flyout" => {
                use crate::ui::tools_panel::Group;
                self.app.ui.tool_flyout = match words.get(1).map(|s| s.as_str()) {
                    Some("shapes") => Some(Group::Shapes),
                    Some("text") => Some(Group::Text),
                    _ => None,
                };
                self.frame(Vec::new());
            }
            "select" => {
                let arg = words.get(1).map(|s| s.as_str()).unwrap_or("");
                if arg == "all" {
                    self.app.select_all();
                } else if arg == "none" {
                    self.app.selection.clear();
                } else {
                    let shapes: Vec<u64> = self.app.doc.shapes().iter().map(|s| s.id).collect();
                    self.app.selection = arg.split(',').filter_map(|i| i.parse::<usize>().ok()).filter_map(|i| shapes.get(i).copied()).collect();
                }
                self.frame(Vec::new());
            }
            "cmd" => {
                let name = words[1..].join(" ").to_lowercase();
                let c = crate::commands::Cmd::all()
                    .into_iter()
                    .find(|c| format!("{c:?}").to_lowercase() == name || c.label().to_lowercase() == name)
                    .ok_or(format!("unknown command {name}"))?;
                c.run(&mut self.app, &self.ctx.clone());
                self.frame(Vec::new());
            }
            "move" => {
                let p = pt(self, 0)?;
                self.move_to(p);
            }
            "down" => self.button(btn, true),
            "up" => self.button(btn, false),
            "click" => {
                let p = pt(self, 0)?;
                self.move_to(p);
                self.button(btn, true);
                self.button(btn, false);
                self.frames(2);
            }
            "dblclick" => {
                let p = egui::pos2(*nums.first().ok_or("x")?, *nums.get(1).ok_or("y")?);
                self.move_to(p);
                for _ in 0..2 {
                    self.button(egui::PointerButton::Primary, true);
                    self.button(egui::PointerButton::Primary, false);
                }
                self.frames(2);
            }
            "drag" => {
                let (a, b) = (pt(self, 0)?, pt(self, 2)?);
                let steps = nums.get(4).copied().unwrap_or(10.0).max(1.0) as usize;
                self.move_to(a);
                self.button(btn, true);
                for i in 1..=steps {
                    self.move_to(a + (b - a) * (i as f32 / steps as f32));
                }
                self.button(btn, false);
                self.frames(2);
            }
            "mods" => {
                let has = |k: &str| words.iter().any(|w| w == k);
                self.mods = egui::Modifiers { shift: has("shift"), alt: has("alt"), ctrl: has("ctrl"), command: has("ctrl"), mac_cmd: false };
                self.frame(vec![egui::Event::ModifiersChanged(self.mods)]);
            }
            "key" => {
                let name = words.get(1).ok_or("key name")?;
                let key = egui::Key::from_name(name)
                    .or_else(|| egui::Key::from_name(name.trim_start_matches("Num")))
                    .or_else(|| egui::Key::ALL.iter().copied().find(|k| format!("{k:?}").eq_ignore_ascii_case(name)))
                    .ok_or(format!("unknown key {name}"))?;
                let has = |k: &str| words.iter().any(|w| w == k);
                let modifiers = egui::Modifiers { shift: has("shift"), alt: has("alt"), ctrl: has("ctrl"), command: has("ctrl"), mac_cmd: false };
                let ev = |pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers };
                self.frame(vec![ev(true)]);
                self.frame(vec![ev(false)]);
            }
            "type" => {
                self.frame(vec![egui::Event::Text(words[1..].join(" "))]);
            }
            "shot" => {
                // Let fade-in animations finish.
                self.frames(10);
                let path = words.get(1).ok_or("shot path")?.clone();
                self.render(&path)?;
                println!("wrote {path}");
            }
            "dump" => self.dump(),
            // Add a shared component library (not saved: headless runs keep no settings).
            "library" => {
                let p = words.get(1).ok_or("library path")?;
                self.app.settings.libraries.push(p.into());
                self.frame(Vec::new());
            }
            // Brush by name (as in the picker, or the style's id) and width.
            "brush" => {
                let name = words.get(1).ok_or("brush name")?.to_lowercase().replace('_', " ");
                let st = crate::brushes::CATEGORIES
                    .iter()
                    .flat_map(|c| c.1.iter().copied())
                    .find(|b| b.label().to_lowercase() == name || format!("{b:?}").to_lowercase() == name.replace(' ', ""))
                    .ok_or(format!("unknown brush {name}"))?;
                self.app.brush.style = st;
                if let Some(w) = nums.first() {
                    self.app.brush.width = *w as f64;
                }
                self.frame(Vec::new());
            }
            // Stabilizer for freehand tools: rope|window [LEN] or off.
            "stabilizer" => {
                let mode = words.get(1).map(|s| s.as_str()).unwrap_or("rope");
                self.app.stabilizer = mode != "off";
                self.app.stab_mode = if mode == "window" { crate::freehand::StabMode::Window } else { crate::freehand::StabMode::Rope };
                if let Some(l) = nums.first() {
                    self.app.stab_len = *l;
                }
                self.frame(Vec::new());
            }
            // Layers-group tab: 0 layers, 1 components, 2 icons, 3 history.
            "tab" => {
                self.app.ui.tab[1] = nums.first().copied().unwrap_or(0.0) as usize;
                self.frame(Vec::new());
            }
            other => return Err(format!("unknown script command `{other}`")),
        }
        Ok(())
    }
}

/// Split on whitespace, honoring double quotes.
fn split_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn run(opts: Options) -> Result<(), String> {
    crate::recent::ENABLED.store(false, std::sync::atomic::Ordering::Relaxed);
    let mut s = Session::new(&opts)?;
    if let Some(script) = &opts.script {
        let text = if script == "-" {
            let mut t = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut t).map_err(|e| e.to_string())?;
            t
        } else {
            std::fs::read_to_string(script).map_err(|e| e.to_string())?
        };
        // Let the layout settle and the view fit first.
        s.frames(12);
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            s.run_line(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        }
        return Ok(());
    }
    for _ in 0..opts.frames.max(2) {
        s.frame(Vec::new());
        std::thread::sleep(Duration::from_millis(60));
    }
    let out = opts.out.to_string_lossy().to_string();
    s.render(&out)
}
