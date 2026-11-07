# Performance

Benchmarks run on synthetic documents: a grid of rectangles, ellipses,
stars, 24-node paths and text, with gradients, strokes, shadows and groups
(`src/bench.rs`).

    cargo test --release bench_large_documents -- --ignored --nocapture
    MTZD_BENCH_SIZES=1000,10000 cargo test --release bench_large_documents -- --ignored --nocapture

A document to try in the app:

    MTZD_BENCH_OUT=/tmp/big.mvec MTZD_BENCH_N=10000 cargo test --release bench_write_document -- --ignored

Frames are measured headless: the whole UI (panels, canvas, overlays) plus
egui's tessellation, without the GPU upload. Every value is a median, in ms.

| 10 049 shapes | before | after |
|---|---:|---:|
| load | 116 | 35 |
| warm frame, whole document in view | 357 | 16 |
| warm frame, zoomed in | 347 | 1.5 |
| frame, everything selected | 6 180 | 5.8 |
| pick (one click) | 2.7 | 0.02 |
| snap (one drag step) | 235 | 2.7 |
| edit one shape + frame | 401 | 3.4 |
| SVG / PDF export | 83 / 46 | 87 / 44 |

After, by document size:

| | 1 004 | 10 049 | 50 249 |
|---|---:|---:|---:|
| save / load | 2.5 / 3.6 | 24 / 35 | 126 / 180 |
| first frame (tessellate everything) | 25 | 116 | 560 |
| warm frame, fit / zoomed in | 0.8 / 0.4 | 16 / 1.5 | 97 / 19 |
| frame, everything selected | 0.9 | 5.8 | 63 |
| pick ×100 | 0.2 | 2.1 | 12.6 |
| snap (one drag step) | 0.09 | 2.7 | 19 |
| edit one shape + frame | 0.6 | 3.4 | 49 |
| union of 200 shapes | 54 | 45 | 49 |
| SVG / PDF export | 7.9 / 4.8 | 87 / 44 | 493 / 233 |

## What changed

- **Id index.** `Document::get` / `parent_of` / `index_of` walked the shape
  tree on every call, so per-shape loops were quadratic. A lazily built
  id → index-path map makes them O(1). Entries are checked against the tree
  before use, and a miss rebuilds the map only if the document changed.
- **Shared undo snapshots.** Top-level shapes are `Arc`s, copied on write. A
  checkpoint copies pointers instead of every shape, so 200 undo steps no
  longer mean 200 copies of the document.
- **Hit testing.** It scans a flat array of hit areas (bounds grown by
  effects), cached per revision, and runs the exact test only on candidates.
  The shape list is no longer cloned on every click or hover.
- **Snapping.** Targets come from one walk of the tree. Smart spacing finds
  neighbour gaps with a sorted scan, which used to compare every pair, and
  only considers shapes in view.
- **Layers panel.** Rows out of view only reserve their height, in one
  allocation per run. Sections and rows read shapes directly, and the
  selection's ancestors are computed once per frame.
- **Large selections.** Style targets are deduplicated with a set, and the
  selection bounds are memoized per revision. Selection outlines are culled
  to the view, and are drawn as boxes beyond 1 000 shapes.
- **Cache pruning** runs only when the document changed.
- **Loading** parses current-format documents straight into the model,
  skipping the intermediate JSON tree. Older versions still go through it to
  be migrated.

## Known limits

- The first frame tessellates every shape: about 0.5 s at 50k shapes.
- With the whole of a 50k document in view, each frame costs about 100 ms.
  Most of it goes to emitting and tessellating 50k meshes. Caching the
  rendered document in a texture would cut that.
