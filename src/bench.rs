//! Performance benchmarks on large synthetic documents.
//!
//!     cargo test --release bench_large_documents -- --ignored --nocapture
//!
//! `MTZD_BENCH_SIZES=1000,10000` picks the document sizes (default 1k, 10k, 50k).
//! Each line reports the median of a few runs.

use std::time::{Duration, Instant};

use kurbo::{Affine, Point, Rect};

use crate::app::App;
use crate::document::*;

/// A mixed document of roughly `n` shapes in a grid: rectangles, ellipses,
/// stars, 24-node paths, text, gradients, strokes, shadows and groups.
pub fn synthetic(n: usize) -> Document {
    let mut doc = Document::default();
    let cols = (n as f64).sqrt().ceil() as usize;
    let cell = 60.0;
    doc.set_size(cols as f64 * cell, cols as f64 * cell);
    let mut i = 0;
    while i < n {
        let (x, y) = ((i % cols) as f64 * cell, (i / cols) as f64 * cell);
        if i % 200 == 199 && i + 10 <= n {
            // A group of ten small shapes.
            let children = (0..10)
                .map(|k| {
                    let mut c = Shape::new(ShapeKind::Rect { w: 4.0, h: 40.0, radii: [0.0; 4] });
                    c.id = 0;
                    c.transform = Affine::translate((x + k as f64 * 5.0, y));
                    c
                })
                .collect();
            let g = Shape::new(ShapeKind::Group { children });
            doc.add_shape(g);
            i += 10;
            continue;
        }
        let mut s = match i % 5 {
            0 => Shape::new(ShapeKind::Rect { w: 40.0, h: 30.0, radii: [4.0; 4] }),
            1 => Shape::new(ShapeKind::Ellipse { w: 40.0, h: 40.0 }),
            2 => Shape::new(ShapeKind::Star { w: 40.0, h: 40.0, points: 6, inner: 0.5, corner: 0.0 }),
            3 if i % 50 == 3 => Shape::new(ShapeKind::Text(TextData { content: format!("Item {i}"), size: 12.0, ..Default::default() })),
            _ => {
                let nodes = (0..24)
                    .map(|k| {
                        let a = k as f64 / 24.0 * std::f64::consts::TAU;
                        let r = if k % 2 == 0 { 20.0 } else { 12.0 };
                        Node { p: Point::new(20.0 + r * a.cos(), 20.0 + r * a.sin()), hin: None, hout: None, smooth: false, corner: 0.0 }
                    })
                    .collect();
                Shape::new(ShapeKind::Path(PathData { subpaths: vec![SubPath { nodes, closed: true }] }))
            }
        };
        s.transform = Affine::translate((x, y));
        s.fill = Some(Paint::Solid(Rgba::rgb(((i * 37) % 255) as f32 / 255.0, 0.4, 0.6)));
        if i % 10 == 0 {
            s.fill = Some(Paint::Gradient(Gradient::new(GradientKind::Linear, Rgba::WHITE, Rgba::BLACK)));
        }
        if i % 3 == 0 {
            s.stroke = Some(Stroke { width: 2.0, ..Default::default() });
        }
        if i % 500 == 7 {
            s.shadow = Some(Shadow::default());
        }
        doc.add_shape(s);
        i += 1;
    }
    doc
}

fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> Duration {
    let mut v: Vec<Duration> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            t.elapsed()
        })
        .collect();
    v.sort();
    v[v.len() / 2]
}

fn report(n: usize, what: &str, d: Duration) {
    println!("{n:>7} shapes  {what:<34} {:>10.2} ms", d.as_secs_f64() * 1000.0);
}

struct Frames {
    ctx: egui::Context,
    app: App,
    t: f64,
}

impl Frames {
    fn run(&mut self, events: Vec<egui::Event>) {
        self.t += 1.0 / 60.0;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            time: Some(self.t),
            focused: true,
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let out = self.ctx.run_ui(raw, |ui| app.frame_ui(ui));
        // Include egui's own tessellation (what the GPU would get).
        std::hint::black_box(self.ctx.tessellate(out.shapes, out.pixels_per_point));
    }
}

pub fn run_all() {
    let sizes: Vec<usize> = std::env::var("MTZD_BENCH_SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or(vec![1_000, 10_000, 50_000]);
    for n in sizes {
        bench(n);
        println!();
    }
}

fn bench(n: usize) {
    let doc = synthetic(n);
    let total = doc.all_ids().len();
    let runs = if n > 20_000 { 3 } else { 5 };

    // File format.
    let bytes = crate::io::native::encode(&doc, None, Default::default()).unwrap();
    report(total, &format!("save (encode, {} KB)", bytes.len() / 1024), time(runs, || crate::io::native::encode(&doc, None, Default::default()).unwrap()));
    report(total, "load (decode)", time(runs, || crate::io::native::decode(&bytes).unwrap()));

    let ctx = egui::Context::default();
    crate::ui::theme::setup(&ctx);
    let mut app = App::new(ctx.clone(), None);
    app.doc = doc.clone();
    app.selection.clear();
    let mut f = Frames { ctx, app, t: 0.0 };

    // Geometry cache (outlines, effects) for every shape.
    let t = Instant::now();
    for id in f.app.doc.all_ids() {
        let s = f.app.doc.get(id).unwrap().clone();
        f.app.cache.update(&s, &mut f.app.fonts);
    }
    report(total, "outline cache, cold", t.elapsed());

    // Frames: whole document in view, then zoomed into a corner.
    let t = Instant::now();
    f.app.zoom_to_fit();
    f.run(vec![]);
    report(total, "first frame (tessellate all)", t.elapsed());
    f.run(vec![]);
    report(total, "frame, fit, warm", time(runs, || f.run(vec![])));
    f.app.zoom_to_rect(Rect::new(0.0, 0.0, 600.0, 400.0), 0.0);
    f.run(vec![]);
    report(total, "frame, zoomed in, warm", time(runs, || f.run(vec![])));
    let mid = f.app.view.to_screen(Point::new(300.0, 200.0));
    report(total, "frame with pointer move", time(runs, || f.run(vec![egui::Event::PointerMoved(mid + egui::vec2(1.0, 0.0))])));

    // Hit-testing.
    let b = f.app.doc.primary().rect();
    let pts: Vec<Point> = (0..100).map(|k| Point::new(b.x0 + (k * 7919 % 1000) as f64 / 1000.0 * b.width(), b.y0 + (k * 104_729 % 1000) as f64 / 1000.0 * b.height())).collect();
    report(total, "pick ×100", time(runs, || pts.iter().filter_map(|p| f.app.pick(*p, false)).count()));
    report(total, "hit_shapes ×100 (all under point)", time(runs, || pts.iter().map(|p| f.app.hit_shapes(*p, true).len()).sum::<usize>()));

    // Snapping (runs on every pointer move while dragging).
    let first = f.app.doc.shapes()[0].id;
    report(total, "snap_box (one drag step)", time(runs, || f.app.snap_box(Rect::new(0.0, 0.0, 40.0, 30.0), kurbo::Vec2::new(3.0, 2.0), &[first])));

    // Selecting everything and a selection-dependent frame (panels, handles).
    f.app.selection = f.app.doc.shapes().iter().map(|s| s.id).collect();
    f.run(vec![]);
    report(total, "frame, all selected", time(runs, || f.run(vec![])));
    f.app.selection.clear();

    // Editing: one shape changes (undo snapshot + cache refresh + frame).
    report(
        total,
        "edit one shape + frame",
        time(runs, || {
            f.app.checkpoint("");
            if let Some(s) = f.app.doc.get_mut(first) {
                s.transform = Affine::translate((1.0, 0.0)) * s.transform;
            }
            f.run(vec![]);
        }),
    );

    // Booleans: union of 200 shapes.
    let ids: Vec<u64> = f.app.doc.shapes().iter().filter(|s| !matches!(s.kind, ShapeKind::Group { .. } | ShapeKind::Text(_))).take(200).map(|s| s.id).collect();
    report(
        total,
        "union of 200 shapes",
        time(1, || {
            let polys: Vec<_> = ids
                .iter()
                .map(|id| {
                    let s = f.app.doc.get(*id).unwrap().clone();
                    let c = f.app.cache.update(&s, &mut f.app.fonts);
                    crate::boolean::path_to_polys(&c.doc_path, s.fill_rule)
                })
                .collect();
            crate::boolean::union_all(&polys.iter().collect::<Vec<_>>())
        }),
    );

    // Export.
    let fonts = &mut f.app.fonts;
    report(total, "prepare (export geometry)", time(runs, || crate::io::prepare(&doc, fonts).len()));
    report(total, "SVG export", time(runs, || crate::io::svg::export(&doc, fonts).len()));
    report(total, "PDF export", time(runs, || crate::io::pdf::export(&doc, fonts).len()));
    if n <= 10_000 {
        report(total, "PNG raster, 1x", time(1, || crate::io::export::rasterize(&doc, fonts, 1.0).map(|i| i.len())));
    }
}
