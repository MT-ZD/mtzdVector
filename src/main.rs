mod app;
mod appearance;
mod brushes;
mod freehand;
mod layout;
mod paint_ops;
mod variants;
mod cli_export;
mod library;
mod print;
mod artboards;
mod boolean;
mod canvas;
mod color;
mod commands;
mod components;
mod document;
mod eyedropper;
mod fonts;
mod geom;
mod gradient_handles;
mod headless;
mod icons;
mod logo;
mod io;
mod ops;
mod path_ops;
mod platform;
mod prim_handles;
mod prims;
mod recent;
mod render;
mod session;
mod settings;
mod snap;
mod spell;
mod strokes;
mod tabs;
mod text;
mod text_edit;
mod text_flow;
mod thumbnail;
mod units;
mod ui;

/// The window's context, for waking it from other threads.
struct Wake(std::sync::OnceLock<egui::Context>);

impl Wake {
    fn set(&self, ctx: egui::Context) {
        let _ = self.0.set(ctx);
    }
    fn with_ctx(&self, f: impl FnOnce(&egui::Context)) {
        if let Some(c) = self.0.get() {
            f(c);
        }
    }
}

static WAKE: Wake = Wake(std::sync::OnceLock::new());

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(target_os = "linux")]
    if let Some(i) = args.iter().position(|a| a == "--write-icons") {
        let dir = args.get(i + 1).map(std::path::PathBuf::from).unwrap_or_else(|| "hicolor".into());
        if let Err(e) = platform::desktop::write_icons(&dir) {
            eprintln!("writing icons failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    if args.iter().any(|a| a == "--install-desktop") {
        match platform::desktop::install() {
            Ok(r) => println!("desktop entry, icons, MIME type and thumbnailer up to date ({} written, {} old removed)", r.written, r.removed),
            Err(e) => {
                eprintln!("install failed: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--export") {
        std::process::exit(cli_export::run(&args[i + 1..]));
    }
    if let Some(i) = args.iter().position(|a| a == "--thumbnail") {
        if let Err(e) = thumbnail::run(&args[i + 1..]) {
            eprintln!("thumbnail failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--screenshot" || a == "--script") {
        let script = (args[i] == "--script").then(|| args.get(i + 1).cloned().unwrap_or("-".into()));
        let mut opts = headless::Options { out: args.get(i + 1).cloned().unwrap_or("shot.png".into()).into(), file: None, size: [1800, 900], frames: 40, scale: 1.25, script };
        let mut k = 0;
        while k < args.len() {
            match args[k].as_str() {
                "--screenshot" | "--script" => k += 1,
                "--size" => {
                    if let Some((a, b)) = args.get(k + 1).and_then(|s| s.split_once('x')) {
                        opts.size = [a.parse().unwrap_or(1800), b.parse().unwrap_or(900)];
                    }
                    k += 1;
                }
                "--scale" => {
                    opts.scale = args.get(k + 1).and_then(|s| s.parse().ok()).unwrap_or(1.25);
                    k += 1;
                }
                "--frames" => {
                    opts.frames = args.get(k + 1).and_then(|s| s.parse().ok()).unwrap_or(40);
                    k += 1;
                }
                f => opts.file = Some(f.into()),
            }
            k += 1;
        }
        if let Err(e) = headless::run(opts) {
            eprintln!("headless render failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    // Files to open (a file manager may pass several).
    let mut files: Vec<std::path::PathBuf> = args.iter().filter(|a| !a.starts_with("--")).map(std::path::PathBuf::from).collect();
    // A running instance opens the files instead (unless --new-window).
    // The wake-up is wired once the window's egui context exists.
    #[cfg(unix)]
    if !args.iter().any(|a| a == "--new-window") {
        if let platform::single::Role::Forwarded = platform::single::claim(&files, || WAKE.with_ctx(|c| c.request_repaint())) {
            return Ok(());
        }
    }
    let file = (!files.is_empty()).then(|| files.remove(0));
    // Wayland shows the icon and app name from the desktop entry.
    #[cfg(target_os = "linux")]
    std::thread::spawn(|| {
        if let Err(e) = platform::desktop::install() {
            eprintln!("desktop entry: {e}");
        }
    });
    let mut viewport = egui::ViewportBuilder::default();
    if let Some(icon) = logo::icon_data() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_title("mtzdVector")
            .with_app_id(platform::desktop::APP_ID)
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 560.0])
            // The menu bar is the title bar (ui::window draws the frame).
            .with_decorations(false),
        renderer: eframe::Renderer::Wgpu,
        // MSAA gives smooth anti-aliased edges for the tessellated vector meshes.
        multisampling: 4,
        ..Default::default()
    };
    eframe::run_native(
        "mtzd-vector",
        options,
        Box::new(move |cc| {
            ui::theme::setup(&cc.egui_ctx);
            let show_welcome = file.is_none();
            let mut app = app::App::new(cc.egui_ctx.clone(), file);
            app.ui.welcome_open = show_welcome;
            app.session.recovery = session::find_recovery();
            app.apply_settings(settings::load(), &cc.egui_ctx);
            app.platform = platform::Platform::init(cc, app.settings.tablet);
            WAKE.set(cc.egui_ctx.clone());
            for f in files {
                app.open_path(f);
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests_text;
#[cfg(test)]
mod tests_paint;
#[cfg(test)]
mod tests_print;
#[cfg(test)]
mod tests_workflow;
