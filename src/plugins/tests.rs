//! Plugin host and Laser plugin tests (planner, job, emulator).

use crate::app::App;
use crate::commands::Cmd;

fn app_svg(svg: &str) -> App {
    let mut app = App::new(egui::Context::default(), None);
    app.doc = crate::io::svg::import(svg.as_bytes(), &mut app.fonts).expect("svg");
    app
}

#[test]
fn plugin_commands_are_commands() {
    let all = Cmd::all();
    let names: Vec<String> = all.iter().map(|c| c.name()).collect();
    for n in ["laser.panel", "laser.connect", "laser.frame", "laser.start", "laser.stop", "laser.save_rd"] {
        assert!(names.iter().any(|x| x == n), "{n} missing: {names:?}");
    }
    let c = crate::headless::find_cmd("laser.start").unwrap();
    assert_eq!(c.label(), "Start Laser Job…");
    assert_eq!(c.meta().category, crate::commands::Category::Plugins);
    let mut app = App::new(egui::Context::default(), None);
    // Not connected: nothing to start or stop; the panel opens.
    assert!(!c.enabled(&app));
    assert!(!crate::headless::find_cmd("laser.stop").unwrap().enabled(&app));
    crate::headless::find_cmd("laser.panel").unwrap().run(&mut app, &egui::Context::default());
    let panels = app.plugins.active_panels();
    assert_eq!(app.ui.tab[crate::ui::studio::DESIGN], 2 + panels.iter().position(|(_, p)| p.id == "laser").unwrap());
}

#[test]
fn plugin_settings_persist() {
    let mut app = App::new(egui::Context::default(), None);
    app.plugin_mut::<super::laser::LaserPlugin>("laser").unwrap().machine.power_cap = 55.0;
    let mut s = app.settings.clone();
    app.plugins.store(&mut s.plugins);
    let json = serde_json::to_string(&s).unwrap();
    let back: crate::settings::Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.plugins["laser"].data["power_cap"], 55.0);
    assert!(back.plugins["laser"].enabled);
    let mut other = App::new(egui::Context::default(), None);
    other.plugins.load(&back.plugins);
    assert_eq!(other.plugin_mut::<super::laser::LaserPlugin>("laser").unwrap().machine.power_cap, 55.0);

    // Layer settings live in the document: saved with it, undoable.
    let mut doc = super::laser::settings::DocSettings::default();
    doc.layers.insert("#FF0000".into(), super::laser::settings::LayerSettings { speed: 12.0, ..Default::default() });
    super::laser::set_doc_settings(&mut app, &doc);
    let bytes = crate::io::native::encode(&app.doc, None, Default::default()).unwrap();
    let loaded = crate::io::native::decode(&bytes).unwrap();
    assert_eq!(loaded.plugin_data["laser"]["layers"]["#FF0000"]["speed"], 12.0);
    app.undo();
    assert!(!app.doc.plugin_data.contains_key("laser"));
}

#[test]
fn switching_a_plugin_off_hides_it() {
    let mut app = App::new(egui::Context::default(), None);
    let start = crate::headless::find_cmd("laser.start").unwrap();
    assert!(start.available(&app));
    app.set_plugin_enabled("laser", false);
    assert!(!Cmd::all().iter().any(|c| matches!(c, Cmd::Plugin(_)) && c.available(&app)));
    assert!(app.plugins.active_panels().is_empty());
    assert!(!crate::headless::find_cmd("laser.panel").unwrap().enabled(&app));
    app.set_plugin_enabled("laser", true);
    assert!(start.available(&app));
}

const SQUARES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96">
  <rect x="0" y="0" width="48" height="48" fill="none" stroke="#ff0000" stroke-width="1"/>
  <rect x="12" y="12" width="24" height="24" fill="none" stroke="#ff0000" stroke-width="1"/>
</svg>"##;

fn laser(app: &mut App) -> &mut super::laser::LaserPlugin {
    app.plugin_mut::<super::laser::LaserPlugin>("laser").unwrap()
}

fn with<R>(app: &mut App, f: impl FnOnce(&mut super::laser::LaserPlugin, &mut App) -> R) -> R {
    app.with_plugin("laser", f).unwrap()
}

fn job_of(app: &mut App) -> mtzd_ruida::Job {
    with(app, |p, app| p.job(app)).unwrap()
}

#[test]
fn planner_units_mirroring_and_order() {
    use super::laser::settings::Corner;
    let mut app = app_svg(SQUARES);
    laser(&mut app).machine.origin = Corner::TopLeft;
    let job = job_of(&mut app);
    // 48 px at 96 dpi = 12.7 mm; the stroke is ignored (centreline).
    assert_eq!(job.bounds(), Some(((0, 0), (12_700, 12_700))));
    assert_eq!(job.layers.len(), 1);
    let l = &job.layers[0];
    assert_eq!(l.rgb, [255, 0, 0]);
    assert_eq!(l.paths.len(), 2);
    // The inner square is cut first, and each square closes.
    let first = &l.paths[0];
    assert!(first.iter().all(|p| p.0 >= 3_175 && p.0 <= 9_525), "{first:?}");
    assert_eq!(first.first(), first.last());

    // Home at the top right of a 600 mm bed: x mirrors.
    laser(&mut app).machine.origin = Corner::TopRight;
    let job = job_of(&mut app);
    assert_eq!(job.bounds(), Some(((600_000 - 12_700, 0), (600_000, 12_700))));
    // At the head, centred: the job's middle at the head.
    {
        let m = &mut laser(&mut app).machine;
        m.origin = Corner::TopLeft;
        m.placement = super::laser::settings::Placement::CurrentPosition;
        m.anchor = (1, 1);
    }
    let plan = with(&mut app, |p, app| p.plan(app));
    let m = laser(&mut app).machine.clone();
    let j = super::laser::plan::to_job(&plan, &m, Some((100_000, 50_000))).unwrap();
    assert_eq!(j.bounds(), Some(((100_000 - 6_350, 50_000 - 6_350), (100_000 + 6_350, 50_000 + 6_350))));
    assert!(super::laser::plan::to_job(&plan, &m, None).is_err());
    // Off the bed: refused.
    assert!(super::laser::plan::to_job(&plan, &m, Some((599_000, 50_000))).is_err());
}

#[test]
fn power_is_capped_and_layers_follow_settings() {
    use super::laser::settings::{DocSettings, LayerSettings};
    let mut app = app_svg(SQUARES);
    laser(&mut app).machine.origin = super::laser::settings::Corner::TopLeft;
    let mut doc = DocSettings::default();
    doc.layers.insert("#FF0000".into(), LayerSettings { min_power: 85.0, max_power: 95.0, passes: 3, ..LayerSettings::line() });
    super::laser::set_doc_settings(&mut app, &doc);
    let job = job_of(&mut app);
    assert_eq!(job.layers[0].max_power, 70.0);
    assert_eq!(job.layers[0].min_power, 70.0);
    assert_eq!(job.layers[0].passes, 3);
    assert!(job.max_power() <= 70.0);
    doc.layers.get_mut("#FF0000").unwrap().output = false;
    super::laser::set_doc_settings(&mut app, &doc);
    assert!(with(&mut app, |p, app| p.job(app)).is_err(), "nothing to cut");
}

#[test]
fn fill_hatches_inside_and_skips_holes() {
    // A 96 × 48 px (25.4 × 12.7 mm) filled ring with a hole, text included.
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
      <path d="M0 0H96V48H0Z M24 12H72V36H24Z" fill="#000000" fill-rule="evenodd"/>
    </svg>"##;
    let mut app = app_svg(svg);
    laser(&mut app).machine.origin = super::laser::settings::Corner::TopLeft;
    let job = job_of(&mut app);
    let l = &job.layers[0];
    let interval_um = 100; // the fill default: 0.1 mm
    // Lines through the hole are split in two.
    let mid = 6_350;
    let through: Vec<_> = l.paths.iter().filter(|p| (p[0].1 - mid).abs() <= interval_um / 2).collect();
    assert_eq!(through.len(), 2, "{through:?}");
    for p in &through {
        let (a, b) = (p[0].0.min(p[1].0), p[0].0.max(p[1].0));
        assert!(b <= 6_350 + 5 || a >= 19_050 - 5, "segment crosses the hole: {a}..{b}");
    }
    // Spacing and coverage: about 127 lines over 12.7 mm.
    let mut ys: Vec<i64> = l.paths.iter().map(|p| p[0].1).collect();
    ys.sort();
    ys.dedup();
    assert!((ys.len() as i64 - 127).abs() <= 1, "{}", ys.len());
    assert!(ys.windows(2).all(|w| (w[1] - w[0] - interval_um).abs() <= 1));
    // Alternate directions.
    let dir = |p: &Vec<mtzd_ruida::Pt>| p[1].0 > p[0].0;
    let rows: Vec<&Vec<mtzd_ruida::Pt>> = l.paths.iter().filter(|p| p[0].1 < 3_000).collect();
    assert_ne!(dir(rows[0]), dir(rows[1]));
}

#[test]
fn text_and_groups_are_cut() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="100">
      <g id="g"><rect x="10" y="10" width="20" height="20" fill="none" stroke="#0000ff"/></g>
      <text x="40" y="60" font-size="30" fill="#00ff00" font-family="DejaVu Sans">Hi</text>
    </svg>"##;
    let mut app = app_svg(svg);
    let job = job_of(&mut app);
    let colours: Vec<[u8; 3]> = job.layers.iter().map(|l| l.rgb).collect();
    // The text engraves first, then the group's square is cut.
    assert_eq!(colours, [[0, 255, 0], [0, 0, 255]]);
}

#[test]
fn emulated_job_runs_end_to_end() {
    let mut app = app_svg(SQUARES);
    let ctx = egui::Context::default();
    {
        let p = laser(&mut app);
        p.machine.port = super::laser::EMULATOR.into();
        p.machine.origin = super::laser::settings::Corner::TopLeft;
        p.connect(&ctx);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let tick = |app: &mut App| app.with_plugins(|h, app| h.tick(app, &egui::Context::default()));
    while !laser(&mut app).connected() && std::time::Instant::now() < deadline {
        tick(&mut app);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(laser(&mut app).connected(), "{:?}", laser(&mut app).error);
    // Start asks first; nothing is sent until it's confirmed.
    crate::headless::find_cmd("laser.start").unwrap().run(&mut app, &ctx);
    assert!(laser(&mut app).confirm.is_some());
    let emu = laser(&mut app).emulator.clone().unwrap();
    assert!(emu.lock().unwrap().cuts.is_empty());
    laser(&mut app).confirm.take();
    crate::headless::find_cmd("laser.start").unwrap().run(&mut app, &ctx);
    // What the modal's Start does.
    laser(&mut app).start_confirmed();
    while emu.lock().unwrap().cuts.len() < 8 && std::time::Instant::now() < deadline {
        tick(&mut app);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let cuts = emu.lock().unwrap().cuts.clone();
    assert_eq!(cuts.len(), 8, "two squares, four sides each");
    assert!(cuts.iter().all(|c| c.2.0 <= 12_700 && c.2.1 <= 12_700));
    // The job finishes (status goes idle) and the panel follows.
    while laser(&mut app).busy != super::laser::Busy::Idle && std::time::Instant::now() < deadline {
        tick(&mut app);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(laser(&mut app).busy, super::laser::Busy::Idle);
    laser(&mut app).disconnect();
}

#[test]
fn artboard_fits_the_bed_and_presets_apply() {
    let mut app = app_svg(SQUARES);
    {
        let m = &mut laser(&mut app).machine;
        m.bed_w = 600.0;
        m.bed_h = 400.0;
    }
    crate::headless::find_cmd("laser.fit_artboard").unwrap().run(&mut app, &egui::Context::default());
    let a = app.doc.primary().clone();
    // 600 mm at 96 dpi.
    assert!((a.w - 600.0 / 25.4 * 96.0).abs() < 1e-6 && (a.h - 400.0 / 25.4 * 96.0).abs() < 1e-6, "{} × {}", a.w, a.h);
    app.undo();
    assert_eq!(app.doc.primary().w, 96.0);

    // Save a layer's settings as a preset, then give it to another document.
    let ply = super::laser::settings::LayerSettings { speed: 8.0, max_power: 60.0, min_power: 55.0, passes: 2, ..Default::default() };
    super::laser::panel::save_preset(laser(&mut app), "4 mm plywood – cut".into(), &ply);
    super::laser::panel::save_preset(laser(&mut app), "4 mm plywood – cut".into(), &super::laser::settings::LayerSettings { speed: 9.0, ..ply.clone() });
    let presets = laser(&mut app).machine.presets.clone();
    assert_eq!(presets.len(), 1, "same name replaces");
    assert_eq!(presets[0].settings.speed, 9.0);
    // Presets are app settings: they survive a restart.
    let mut s = app.settings.clone();
    app.plugins.store(&mut s.plugins);
    let mut other = App::new(egui::Context::default(), None);
    other.plugins.load(&s.plugins);
    assert_eq!(laser(&mut other).machine.presets, presets);
}

#[test]
fn output_off_skips_a_layer_but_keeps_it_listed() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96">
      <rect x="0" y="0" width="48" height="48" fill="none" stroke="#ff0000"/>
      <rect x="50" y="0" width="20" height="20" fill="none" stroke="#0000ff"/>
    </svg>"##;
    let mut app = app_svg(svg);
    let mut doc = super::laser::settings::DocSettings::default();
    doc.layers.insert("#FF0000".into(), super::laser::settings::LayerSettings { output: false, ..Default::default() });
    super::laser::set_doc_settings(&mut app, &doc);
    let plan = with(&mut app, |p, app| p.plan(app));
    assert_eq!(plan.layers.iter().map(|l| l.key.as_str()).collect::<Vec<_>>(), ["#FF0000", "#0000FF"]);
    let job = job_of(&mut app);
    assert_eq!(job.layers.iter().map(|l| l.rgb).collect::<Vec<_>>(), [[0, 0, 255]]);
}

fn fill_job(svg: &str, st: super::laser::settings::LayerSettings) -> mtzd_ruida::Job {
    let mut app = app_svg(svg);
    laser(&mut app).machine.origin = super::laser::settings::Corner::TopLeft;
    let mut doc = super::laser::settings::DocSettings::default();
    doc.layers.insert("#000000".into(), st);
    super::laser::set_doc_settings(&mut app, &doc);
    job_of(&mut app)
}

// 10 × 10 mm (37.795 px at 96 dpi) and a second square 30 mm to the right.
const TWO_SQUARES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="100">
  <rect x="0" y="0" width="37.795276" height="37.795276" fill="#000000"/>
  <rect x="151.181102" y="0" width="37.795276" height="37.795276" fill="#000000"/>
</svg>"##;

#[test]
fn fill_styles() {
    use super::laser::settings::{FillStyle, LayerSettings};
    let base = LayerSettings { interval: 1.0, ..LayerSettings::fill() };
    let horizontal = |p: &Vec<mtzd_ruida::Pt>| (p[0].1 - p[1].1).abs() < 2;
    let vertical = |p: &Vec<mtzd_ruida::Pt>| (p[0].0 - p[1].0).abs() < 2;

    // Scan: 10 lines per square, all horizontal.
    let scan = fill_job(TWO_SQUARES, base.clone());
    assert_eq!(scan.layers[0].paths.len(), 20);
    assert!(scan.layers[0].paths.iter().all(horizontal));

    // Cross-hatch: as many again, across.
    let cross = fill_job(TWO_SQUARES, LayerSettings { fill_style: FillStyle::CrossHatch, ..base.clone() });
    let p = &cross.layers[0].paths;
    assert_eq!(p.len(), 40);
    assert_eq!(p.iter().filter(|q| vertical(q)).count(), 20);

    // Offset: 5 closed rings per square, the outer one 0.5 mm in.
    let off = fill_job(TWO_SQUARES, LayerSettings { fill_style: FillStyle::Offset, ..base.clone() });
    let p = &off.layers[0].paths;
    assert_eq!(p.len(), 10, "{}", p.len());
    assert!(p.iter().all(|r| r.first() == r.last() && r.len() >= 5));
    let outer = p.iter().map(|r| r.iter().map(|q| q.0).min().unwrap()).min().unwrap();
    assert!((outer - 500).abs() <= 20, "{outer}");

    // Angle step: the passes are baked in, each turned by 90°.
    let turned = fill_job(TWO_SQUARES, LayerSettings { passes: 2, angle_step: 90.0, ..base.clone() });
    let l = &turned.layers[0];
    assert_eq!(l.passes, 1);
    assert_eq!(l.paths.len(), 40);
    assert!(l.paths[..20].iter().all(horizontal) && l.paths[20..].iter().all(vertical));
    // Without a step, passes repeat the same lines.
    let twice = fill_job(TWO_SQUARES, LayerSettings { passes: 2, ..base.clone() });
    assert_eq!((twice.layers[0].passes, twice.layers[0].paths.len()), (2, 20));

    // All at once: a line sweeps across both squares (alternating between
    // them); one by one: the first square is done before the second.
    let left = |p: &Vec<mtzd_ruida::Pt>| p[0].0 < 20_000;
    let sides: Vec<bool> = scan.layers[0].paths.iter().map(left).collect();
    assert!(sides.windows(2).any(|w| w[0] != w[1] && w.iter().any(|x| *x)) && sides.windows(2).filter(|w| w[0] != w[1]).count() > 2);
    let one = fill_job(TWO_SQUARES, LayerSettings { one_by_one: true, ..base });
    let sides: Vec<bool> = one.layers[0].paths.iter().map(left).collect();
    assert_eq!(sides.windows(2).filter(|w| w[0] != w[1]).count(), 1, "{sides:?}");
}

/// A 20 × 10 px PNG: the left half black, the right half white, shown at
/// 10 × 5 mm (37.795 × 18.898 px at 96 dpi).
fn half_black_svg() -> String {
    use base64::Engine as _;
    let img = image::RgbaImage::from_fn(20, 10, |x, _| if x < 10 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    format!(r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="200" height="100"><image x="0" y="0" width="37.795276" height="18.897638" preserveAspectRatio="none" xlink:href="data:image/png;base64,{b64}"/></svg>"##)
}

#[test]
fn images_are_engraved_line_by_line() {
    use super::laser::settings::{Dither, LayerSettings};
    let mut app = app_svg(&half_black_svg());
    laser(&mut app).machine.origin = super::laser::settings::Corner::TopLeft;
    let plan = with(&mut app, |p, app| p.plan(app));
    assert_eq!(plan.layers.len(), 1, "{:?}", plan.layers.iter().map(|l| &l.key).collect::<Vec<_>>());
    assert_eq!(plan.layers[0].key, super::laser::plan::IMAGE_KEY);
    assert_eq!(plan.layers[0].settings.mode, super::laser::settings::Mode::Image);
    let mut doc = super::laser::settings::DocSettings::default();
    doc.layers.insert("Image".into(), LayerSettings { interval: 0.5, dither: Dither::Threshold, ..LayerSettings::image() });
    super::laser::set_doc_settings(&mut app, &doc);
    let job = job_of(&mut app);
    let l = &job.layers[0];
    // One run per row (10 rows of 0.5 mm), each over the black half only.
    assert!((10..=11).contains(&l.paths.len()), "{}", l.paths.len());
    for p in &l.paths {
        let (a, b) = (p[0].0.min(p[1].0), p[0].0.max(p[1].0));
        assert!(a <= 100 && (b - 5_000).abs() <= 600, "{a}..{b}");
    }
    // Rows alternate direction.
    assert_ne!(l.paths[0][0].0 < l.paths[0][1].0, l.paths[1][0].0 < l.paths[1][1].0);
    // Negative: the white half burns instead.
    doc.layers.get_mut("Image").unwrap().invert = true;
    super::laser::set_doc_settings(&mut app, &doc);
    let job = job_of(&mut app);
    assert!(job.layers[0].paths.iter().all(|p| p[0].0.min(p[1].0) >= 4_400), "{:?}", job.layers[0].paths[0]);
}

#[test]
fn dithering_keeps_the_tone() {
    use super::laser::settings::{Dither, LayerSettings};
    let grey = vec![0.5f32; 64 * 64];
    for d in [Dither::FloydSteinberg, Dither::Jarvis, Dither::Ordered] {
        let on = super::laser::plan::dither(&grey, 64, 64, &LayerSettings { dither: d, ..LayerSettings::image() }).iter().filter(|b| **b).count();
        assert!((on as f64 / 4096.0 - 0.5).abs() < 0.03, "{d:?}: {on}");
    }
    let dark = vec![0.2f32; 64 * 64];
    let on = super::laser::plan::dither(&dark, 64, 64, &LayerSettings { dither: Dither::FloydSteinberg, ..LayerSettings::image() }).iter().filter(|b| **b).count();
    assert!((on as f64 / 4096.0 - 0.8).abs() < 0.03, "{on}");
    let t = super::laser::plan::dither(&[0.3, 0.7], 2, 1, &LayerSettings { dither: Dither::Threshold, threshold: 50.0, ..LayerSettings::image() });
    assert_eq!(t, [true, false]);
}

#[test]
fn user_origin_places_the_job() {
    use super::laser::settings::{Corner, Placement};
    let mut app = app_svg(SQUARES);
    let plan = with(&mut app, |p, app| p.plan(app));
    let mut m = laser(&mut app).machine.clone();
    m.origin = Corner::TopLeft;
    m.placement = Placement::UserOrigin;
    m.anchor = (0, 0);
    assert!(super::laser::plan::to_job(&plan, &m, None).is_err(), "no origin set");
    m.user_origin = Some((100.0, 50.0));
    let j = super::laser::plan::to_job(&plan, &m, None).unwrap();
    assert_eq!(j.bounds(), Some(((100_000, 50_000), (112_700, 62_700))));
    // The start mark sits at the job's anchor (document px); absolute: at
    // the home corner of the bed.
    m.anchor = (2, 2);
    assert_eq!(super::laser::plan::start_point(&plan, &m), Some(kurbo::Point::new(48.0, 48.0)));
    m.placement = Placement::Absolute;
    m.origin = Corner::TopRight;
    let p = super::laser::plan::start_point(&plan, &m).unwrap();
    assert!((p.x - 600.0 / 25.4 * 96.0).abs() < 1e-6 && p.y == 0.0);
}

// Offset fill at a fine interval used to offset each ring from the last
// one; the points piled up and planning took minutes (the app froze).
#[test]
fn offset_fill_is_fast() {
    use super::laser::settings::{FillStyle, LayerSettings};
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="2268" height="1512">
  <rect x="814.12" y="378.88" width="339.68" height="377.02" fill="#000000"/>
  <rect x="1046.59" y="662.64" width="426.79" height="408.45" fill="#000000"/>
</svg>"##;
    let t = std::time::Instant::now();
    let job = fill_job(svg, LayerSettings { fill_style: FillStyle::Offset, interval: 0.1, ..LayerSettings::fill() });
    let rings = job.layers[0].paths.len();
    // ~540 rings 0.1 mm apart across the L, more once it splits in two.
    assert!((540..1200).contains(&rings), "{rings}");
    assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
}
