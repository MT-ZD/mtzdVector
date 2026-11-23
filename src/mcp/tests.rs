use serde_json::{Value, json};

use super::tools::{self, Content, Outcome};
use super::{Mode, Server};
use crate::app::App;
use crate::document::{Paint, ShapeKind};

fn app() -> App {
    App::new(egui::Context::default(), None)
}

fn call(app: &mut App, name: &str, args: Value) -> Result<Value, String> {
    let ctx = egui::Context::default();
    let mut ptr = Default::default();
    match tools::call(app, &ctx, &mut ptr, name, &args, "test")? {
        Outcome::Done(c) => Ok(c
            .into_iter()
            .find_map(|c| if let Content::Text(t) = c { serde_json::from_str(&t).ok() } else { None })
            .unwrap_or(Value::Null)),
        Outcome::Later(b, _) => Ok(json!(b.len())),
    }
}

#[test]
fn protocol_handshake_and_listing() {
    let mut s = Server { mode: Mode::Headless, client: "Test".into(), file: None, backend: None };
    let r = s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    let r = s.handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}})).unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    // Notifications get no answer.
    assert!(s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
    let r = s.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})).unwrap();
    let list = r["result"]["tools"].as_array().unwrap();
    let mut names = std::collections::HashSet::new();
    for t in list {
        assert!(names.insert(t["name"].as_str().unwrap().to_string()), "duplicate {}", t["name"]);
        assert_eq!(t["inputSchema"]["type"], "object");
        for req in t["inputSchema"]["required"].as_array().unwrap() {
            assert!(t["inputSchema"]["properties"].get(req.as_str().unwrap()).is_some(), "{} requires unknown {req}", t["name"]);
        }
    }
    let r = s.handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "nope"})).unwrap();
    assert_eq!(r["error"]["code"], -32601);
}

#[test]
fn edits_are_single_undo_steps() {
    let mut a = app();
    let r = call(&mut a, "add_shape", json!({"type": "rect", "x": 10, "y": 20, "w": 100, "h": 50, "fill": "#ff0000", "name": "Box"})).unwrap();
    let id = r["id"].as_u64().unwrap();
    assert_eq!(r["bounds"], json!([10.0, 20.0, 100.0, 50.0]));
    let r = call(&mut a, "add_svg", json!({"svg": "<svg xmlns='http://www.w3.org/2000/svg'><rect x='0' y='0' width='10' height='10'/><circle cx='50' cy='50' r='5'/></svg>", "group": "Icon", "at": [200, 300]})).unwrap();
    let g = r["ids"][0].as_u64().unwrap();
    assert_eq!(r["ids"].as_array().unwrap().len(), 1);
    assert_eq!(a.doc.get(g).unwrap().name, "Icon");
    assert_eq!((r["bounds"][0].as_f64(), r["bounds"][1].as_f64()), (Some(200.0), Some(300.0)));
    call(&mut a, "update_shapes", json!({"ids": [id], "set": {"x": 0, "y": 0, "w": 200, "opacity": 0.5}, "patch": {"blur": 3.0}})).unwrap();
    let s = a.doc.get(id).unwrap();
    assert_eq!((s.opacity, s.blur), (0.5, 3.0));
    assert!(matches!(s.kind, ShapeKind::Rect { w, .. } if (w - 200.0).abs() < 1e-6), "{:?}", s.kind);
    // A bad patch changes nothing.
    assert!(call(&mut a, "update_shapes", json!({"ids": [id], "patch": {"opacity": "x"}})).is_err());
    assert_eq!(a.doc.get(id).unwrap().opacity, 0.5);
    // Undo walks back one tool call at a time.
    call(&mut a, "run_command", json!({"name": "Undo"})).unwrap();
    assert_eq!(a.doc.get(id).unwrap().opacity, 1.0);
    assert!(a.doc.get(g).is_some());
    call(&mut a, "run_command", json!({"name": "undo"})).unwrap();
    assert!(a.doc.get(g).is_none());
    assert!(a.doc.get(id).is_some());
}

#[test]
fn commands_selection_and_guards() {
    let mut a = app();
    let r1 = call(&mut a, "add_shape", json!({"type": "ellipse", "x": 0, "y": 0, "w": 100, "h": 100})).unwrap()["id"].as_u64().unwrap();
    let r2 = call(&mut a, "add_shape", json!({"type": "ellipse", "x": 50, "y": 0, "w": 100, "h": 100})).unwrap()["id"].as_u64().unwrap();
    assert!(call(&mut a, "run_command", json!({"name": "Open"})).unwrap_err().contains("open_file"));
    assert!(call(&mut a, "run_command", json!({"name": "Union", "ids": []})).unwrap_err().contains("doesn't apply"));
    // Commands answer with what changed, not the whole tree.
    let st = call(&mut a, "run_command", json!({"name": "Union", "ids": [r1, r2]})).unwrap();
    assert!(st.get("layers").is_none());
    assert_eq!(st["created"].as_array().unwrap().len(), 1, "{st}");
    assert_eq!(st["created"][0]["layers"].as_array().unwrap().len(), 2, "{st}");
    let full = call(&mut a, "run_command", json!({"name": "SelectAll", "full_state": true})).unwrap();
    assert_eq!(full["layers"].as_array().unwrap().len(), 1);
    assert!(call(&mut a, "delete_shapes", json!({"ids": [12345]})).is_err());
    let cmds = call(&mut a, "list_commands", json!({"filter": "align"})).unwrap();
    assert!(cmds.as_array().unwrap().iter().any(|c| c["name"] == "AlignLeft"));
    assert!(call(&mut a, "add_shape", json!({"type": "path", "d": "M0 0 L10 0 L10 10 Z", "stroke": "#000000"})).is_ok());
    assert!(call(&mut a, "add_shape", json!({"type": "path", "d": "nonsense"})).is_err());
    // Input becomes per-frame event batches: move, press, release, 2 idle.
    assert_eq!(call(&mut a, "input", json!({"script": "click 10 10"})).unwrap(), json!(5));
    assert!(call(&mut a, "input", json!({"script": "wave 1 2"})).is_err());
}

#[test]
fn render_returns_png() {
    let mut a = app();
    call(&mut a, "add_shape", json!({"type": "rect", "x": 0, "y": 0, "w": 100, "h": 100, "fill": "#00ff00"})).unwrap();
    let ctx = egui::Context::default();
    let Ok(Outcome::Done(c)) = tools::call(&mut a, &ctx, &mut Default::default(), "render", &json!({"max_size": 200}), "test") else { panic!() };
    let Some(Content::Png(png)) = c.into_iter().next() else { panic!("no image") };
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!(img.width().max(img.height()), 200);
}

/// The whole live chain without a window: server → socket → the app's
/// end-of-frame tick (and raw-input hook for agent input).
#[cfg(unix)]
#[test]
fn live_window_round_trip() {
    let path = std::env::temp_dir().join(format!("mtzd-mcp-test-{}.sock", std::process::id()));
    super::live::listen_at(path.clone(), || {});
    let stream = std::os::unix::net::UnixStream::connect(&path).unwrap();
    let write = stream.try_clone().unwrap();
    let mut server = Server { mode: Mode::Live, client: "Test".into(), file: None, backend: Some(super::Backend::Live { read: std::io::BufReader::new(stream), write }) };
    let client = std::thread::spawn(move || {
        server.forward(super::live::HELLO, &json!({"client": "Test Agent"})).unwrap();
        assert!(super::live::clients().contains(&"Test Agent".to_string()));
        let a = server.call("add_shape", &json!({"type": "rect", "w": 10, "h": 10}));
        let b = server.call("input", &json!({"script": "move 5 5\nkey Escape"}));
        (a, b)
    });
    let mut a = app();
    let ctx = egui::Context::default();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !client.is_finished() && std::time::Instant::now() < until {
        let mut raw = egui::RawInput::default();
        a.mcp_raw_input(&ctx, &mut raw);
        a.mcp_tick(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let (r1, r2) = client.join().unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(r1["isError"], false, "{r1}");
    assert_eq!(a.doc.shapes().len(), 1);
    assert_eq!(r2["isError"], false, "{r2}");
    let st: Value = serde_json::from_str(r2["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(st["mode"], "live");
}

/// Agents see the interface as a widget list and can act on it by name.
#[cfg(unix)]
#[test]
fn ui_tree_names_widgets() {
    let ctx = egui::Context::default();
    crate::ui::theme::setup(&ctx);
    let mut a = App::new(ctx.clone(), None);
    a.mcp.inspect = true;
    let raw = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
    for _ in 0..3 {
        ctx.run_ui(raw(), |ui| a.frame_ui(ui)).textures_delta.clear();
    }
    call(&mut a, "add_shape", json!({"type": "rect", "w": 10, "h": 10})).unwrap();
    for _ in 0..2 {
        ctx.run_ui(raw(), |ui| a.frame_ui(ui)).textures_delta.clear();
    }
    let tree = super::ui_tree::format(&a.mcp.ui, "", 2000);
    for want in ["button \"File\"", "tab \"Layers\"", "number \"X\"", "button \"AI Agents"] {
        assert!(tree.contains(want), "{want} missing from\n{tree}");
    }
    assert!(!tree.contains("textrun"));
    let n = super::ui_tree::find(&a.mcp.ui, None, Some("w"), Some("number"), 0).unwrap();
    assert_eq!(n.label, "W");
    let id = super::ui_tree::short(n.id);
    assert_eq!(super::ui_tree::find(&a.mcp.ui, Some(&format!("#{id}")), None, None, 0).unwrap().id, n.id);
    // Typing into a field becomes click, select all, text, Enter.
    assert!(call(&mut a, "ui_type", json!({"label": "W", "role": "number", "text": "40"})).unwrap().as_u64().unwrap() >= 8);
}

/// Keys are relative to where the shape rests, even when the first key isn't
/// the rest pose; clearing goes back to rest.
#[test]
fn animate_keys_are_relative_to_rest() {
    let mut a = app();
    let id = call(&mut a, "add_shape", json!({"type": "rect", "x": 100, "y": 500, "w": 100, "h": 100})).unwrap()["id"].as_u64().unwrap();
    let b = |a: &mut App| call(a, "get_state", json!({})).unwrap()["layers"][0]["bounds"].clone();
    assert!(a.selection.is_empty(), "new shapes aren't selected");
    let r = call(&mut a, "animate", json!({"id": id, "duration": 2, "keys": [{"t": 0, "move": [0, -400], "scale": 0.5, "ease": "linear"}, {"t": 1, "move": [0, 0], "scale": 1}]})).unwrap();
    let shape = &r["animation"][0];
    assert_eq!(shape["bounds_at_keys"], json!([{"t": 0.0, "bounds": [125.0, 125.0, 50.0, 50.0]}, {"t": 1.0, "bounds": [100.0, 500.0, 100.0, 100.0]}]));
    // Stored values echoed in full; the last key has no ease.
    let keys = &shape["tracks"]["Transform"];
    assert_eq!(keys[1]["value"], json!({"move": [0.0, 0.0], "rotate": 0.0, "scale": 1.0}));
    assert_eq!(keys[0]["ease"], "linear");
    assert!(keys[1].get("ease").is_none());
    // Rest bounds next to the bounds at the playhead (t = 0 here).
    let l = call(&mut a, "get_state", json!({})).unwrap()["layers"][0].clone();
    assert_eq!(l["rest_bounds"], json!([100.0, 500.0, 100.0, 100.0]));
    // Frames at other moments leave the playhead alone.
    let t = a.anim_time();
    match tools::call(&mut a, &egui::Context::default(), &mut Default::default(), "render", &json!({"times": [0, 0.5, 1, 1.5], "max_size": 64}), "test").unwrap() {
        Outcome::Done(c) => {
            let Content::Png(png) = &c[0] else { panic!("no image") };
            let img = image::load_from_memory(png).unwrap();
            assert_eq!((img.width(), img.height()), (2 * 64 + 6, 2 * 36 + 6));
        }
        _ => panic!("render is immediate"),
    }
    assert_eq!(a.anim_time(), t);
    call(&mut a, "seek", json!({"time": 0})).unwrap();
    assert_eq!(b(&mut a), json!([125.0, 125.0, 50.0, 50.0]));
    call(&mut a, "seek", json!({"time": 1})).unwrap();
    assert_eq!(b(&mut a), json!([100.0, 500.0, 100.0, 100.0]));
    // Animating again from another moment doesn't move the rest either.
    call(&mut a, "seek", json!({"time": 0.5})).unwrap();
    call(&mut a, "animate", json!({"id": id, "keys": [{"t": 0, "move": [50, 0]}, {"t": 1, "move": [0, 0]}]})).unwrap();
    call(&mut a, "seek", json!({"time": 1})).unwrap();
    assert_eq!(b(&mut a), json!([100.0, 500.0, 100.0, 100.0]));
    call(&mut a, "seek", json!({"time": 0})).unwrap();
    assert_eq!(b(&mut a), json!([150.0, 500.0, 100.0, 100.0]));
    // Clearing returns to rest, wherever the playhead is.
    call(&mut a, "clear_animation", json!({"ids": [id]})).unwrap();
    assert_eq!(b(&mut a), json!([100.0, 500.0, 100.0, 100.0]));
}

fn layer(a: &mut App, id: u64) -> Value {
    fn find(v: &Value, id: u64) -> Option<Value> {
        v.as_array()?.iter().find_map(|l| if l["id"] == id { Some(l.clone()) } else { find(&l["children"], id) })
    }
    find(&call(a, "get_state", json!({"depth": 5})).unwrap()["layers"], id).unwrap()
}

fn close(v: &Value, want: [f64; 4]) -> bool {
    (0..4).all(|i| (v[i].as_f64().unwrap() - want[i]).abs() < 0.05)
}

/// Rotation about an origin stays about it between keys; a group's motion
/// carries its animated children; rest bounds don't follow the children.
#[test]
fn animate_origin_and_groups() {
    let mut a = app();
    let arm = call(&mut a, "add_shape", json!({"type": "rect", "x": 100, "y": 100, "w": 100, "h": 20})).unwrap()["id"].as_u64().unwrap();
    let r = call(&mut a, "animate", json!({"id": arm, "origin": "left", "ease": "linear", "keys": [{"t": 0, "rotate": 0}, {"t": 1, "rotate": 90}]})).unwrap();
    assert_eq!(r["animation"][0]["origin"], json!([100.0, 110.0]));
    call(&mut a, "seek", json!({"time": 0.5})).unwrap();
    let b = layer(&mut a, arm)["bounds"].clone();
    assert!(close(&b, [92.93, 102.93, 84.85, 84.85]), "{b}");
    call(&mut a, "seek", json!({"time": 1})).unwrap();
    assert!(close(&layer(&mut a, arm)["bounds"], [90.0, 110.0, 20.0, 100.0]));

    // Into a group with a body; the group moves, the arm still swings about its shoulder.
    let body = call(&mut a, "add_shape", json!({"type": "rect", "x": 0, "y": 100, "w": 100, "h": 200})).unwrap()["id"].as_u64().unwrap();
    call(&mut a, "run_command", json!({"name": "Group", "ids": [arm, body]})).unwrap();
    let g = a.selection[0];
    call(&mut a, "seek", json!({"time": 0})).unwrap();
    let rest = layer(&mut a, g)["bounds"].clone();
    call(&mut a, "animate", json!({"id": g, "ease": "linear", "keys": [{"t": 0, "move": [0, 0]}, {"t": 1, "move": [300, 0]}]})).unwrap();
    for t in [0.0, 0.5, 1.0] {
        call(&mut a, "seek", json!({"time": t})).unwrap();
        assert_eq!(layer(&mut a, g)["rest_bounds"], rest, "group rest at {t}");
    }
    assert!(close(&layer(&mut a, arm)["bounds"], [390.0, 110.0, 20.0, 100.0]));
    call(&mut a, "seek", json!({"time": 0.5})).unwrap();
    assert!(close(&layer(&mut a, arm)["bounds"], [242.93, 102.93, 84.85, 84.85]));
}

#[test]
fn animate_smooth_and_oscillate() {
    let mut a = app();
    let id = call(&mut a, "add_shape", json!({"type": "rect", "x": 0, "y": 0, "w": 10, "h": 10})).unwrap()["id"].as_u64().unwrap();
    // Smooth: no stop at the middle key (ease_in_out would nearly halt there).
    call(&mut a, "animate", json!({"id": id, "ease": "smooth", "keys": [{"t": 0, "move": [0, 0]}, {"t": 1, "move": [50, 0]}, {"t": 2, "move": [100, 0]}]})).unwrap();
    let x = |a: &mut App, t: f64| {
        call(a, "seek", json!({"time": t})).unwrap();
        layer(a, id)["bounds"][0].as_f64().unwrap()
    };
    let speed = (x(&mut a, 1.1) - x(&mut a, 0.9)) / 0.2;
    // (Ease in and out would be at ~10 here.)
    assert!((speed - 50.0).abs() < 6.0, "{speed}");
    assert!((x(&mut a, 1.0) - 50.0).abs() < 1e-6);
    // A wave on top of the move, fading in; many keys come back as a summary.
    let r = call(&mut a, "animate", json!({"id": id, "duration": 2, "replace": false, "oscillate": {"prop": "move_y", "amp": 20, "freq": 2, "envelope": [[0, 1], [2, 1]]}})).unwrap();
    assert!(r["animation"][0]["tracks"]["Transform"]["keys"].as_u64().unwrap() > 20, "{r}");
    assert!(r["animation"][0]["bounds_envelope"].is_array());
    call(&mut a, "seek", json!({"time": 0.125})).unwrap();
    let y = layer(&mut a, id)["bounds"][1].as_f64().unwrap();
    assert!((y - 20.0).abs() < 0.5, "{y}");
    assert!((x(&mut a, 1.0) - 50.0).abs() < 0.5);
}

#[test]
fn documents_svg_groups_and_params() {
    let mut a = app();
    // new_document / set_artboard / artboards patched by id.
    call(&mut a, "new_document", json!({"w": 512, "h": 256, "background": null, "name": "Icon"})).unwrap();
    let st = call(&mut a, "get_state", json!({})).unwrap();
    let ab = &st["artboards"][0];
    assert_eq!((ab["bounds"].clone(), ab["background"].clone(), ab["name"].clone()), (json!([0.0, 0.0, 512.0, 256.0]), json!("#FFFFFF00"), json!("Icon")));
    let id = ab["id"].as_u64().unwrap();
    call(&mut a, "set_artboard", json!({"w": 64, "h": 64, "background": "#112233"})).unwrap();
    call(&mut a, "patch_document", json!({"patch": {"artboards": {id.to_string(): {"name": "Fav"}}}})).unwrap();
    let ab = &call(&mut a, "get_state", json!({})).unwrap()["artboards"][0];
    assert_eq!((ab["bounds"].clone(), ab["background"].clone(), ab["name"].clone()), (json!([0.0, 0.0, 64.0, 64.0]), json!("#112233"), json!("Fav")));

    // <g id> stays a group; a gradient stroke becomes a gradient-filled outline.
    let svg = "<svg xmlns='http://www.w3.org/2000/svg'><defs><linearGradient id='gr' gradientUnits='userSpaceOnUse' x1='0' y1='0' x2='60' y2='0'><stop offset='0' stop-color='#f00'/><stop offset='1' stop-color='#00f'/></linearGradient></defs>\
        <g id='arm'><rect id='upper' x='0' y='0' width='20' height='5'/><rect id='lower' x='20' y='0' width='20' height='5'/></g>\
        <path id='line' d='M0 40 L60 40' stroke='url(#gr)' stroke-width='6' fill='none'/></svg>";
    let r = call(&mut a, "add_svg", json!({"svg": svg})).unwrap();
    let names: Vec<&str> = r["shapes"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["arm", "line"]);
    assert_eq!(r["shapes"][0]["type"], "Group");
    let line = a.doc.get(r["ids"][1].as_u64().unwrap()).unwrap();
    assert!(line.stroke.is_none() && matches!(line.fill, Some(Paint::Gradient(_))), "{:?}", line.fill);

    // Icon strip at real sizes; onion skins.
    match tools::call(&mut a, &egui::Context::default(), &mut Default::default(), "render", &json!({"sizes": [16, 32]}), "test").unwrap() {
        Outcome::Done(c) => {
            let Content::Png(png) = &c[0] else { panic!() };
            let img = image::load_from_memory(png).unwrap();
            assert_eq!((img.width(), img.height()), (16 + 32 + 3 * 12, 32 + 2 * 12));
        }
        _ => panic!(),
    }
    let ball = call(&mut a, "add_shape", json!({"type": "ellipse", "w": 10, "h": 10})).unwrap()["id"].as_u64().unwrap();
    call(&mut a, "animate", json!({"id": ball, "keys": [{"t": 0, "move": [0, 0]}, {"t": 1, "move": [40, 0]}]})).unwrap();
    assert!(call(&mut a, "render", json!({"onion": [0.25, 0.5, 0.75], "time": 1})).is_ok());

    // Playback is idempotent.
    for _ in 0..2 {
        assert_eq!(call(&mut a, "playback", json!({"playing": true, "time": 0.5})).unwrap()["playing"], true);
    }
    assert_eq!(call(&mut a, "playback", json!({"playing": false})).unwrap()["playing"], false);
    assert_eq!(get_layer_keys(&mut a, ball), 2);

    // Misnamed parameters are named in the error.
    let e = call(&mut a, "run_command", json!({"command": "Group"})).unwrap_err();
    assert!(e.contains("`command`") && e.contains("name"), "{e}");
    // Round caps and joins.
    let p = call(&mut a, "add_shape", json!({"type": "path", "d": "M0 0 L10 10", "stroke": "#000000", "cap": "round", "join": "round"})).unwrap()["id"].as_u64().unwrap();
    let st = a.doc.get(p).unwrap().stroke.clone().unwrap();
    assert_eq!((st.cap, st.join), (crate::document::LineCap::Round, crate::document::LineJoin::Round));
}

fn get_layer_keys(a: &mut App, id: u64) -> u64 {
    layer(a, id)["keys"].as_u64().unwrap()
}

/// Nested groups added at once get unique ids (they used to collide, and
/// walking up the tree then never ended).
#[test]
fn nested_svg_groups_have_unique_ids() {
    let mut a = app();
    let svg = "<svg xmlns='http://www.w3.org/2000/svg'><g id='robot'><rect id='body' width='10' height='10'/><circle id='head' cx='5' cy='-5' r='4'/>\
        <g id='arm'><rect id='upper' x='10' width='10' height='3'/><rect id='fore' x='20' width='10' height='3'/></g></g></svg>";
    call(&mut a, "add_svg", json!({"svg": svg})).unwrap();
    call(&mut a, "add_svg", json!({"svg": svg})).unwrap();
    let ids = a.doc.all_ids();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    for id in ids {
        let mut seen = 0;
        let mut p = a.doc.parent_of(id);
        while let Some(x) = p {
            seen += 1;
            assert!(seen < 10, "parent loop at {id}");
            p = a.doc.parent_of(x);
        }
    }
}

#[test]
fn components_layout_and_batches() {
    let mut a = app();
    let bg = call(&mut a, "add_shape", json!({"type": "rect", "x": 0, "y": 0, "w": 200, "h": 50, "fill": "linear-gradient(90deg, #ff0000, #0000ff)", "name": "Bg"})).unwrap()["id"].as_u64().unwrap();
    let Some(Paint::Gradient(g)) = &a.doc.get(bg).unwrap().fill else { panic!("gradient fill") };
    assert_eq!((g.start, g.end, g.stops.len()), ([0.0, 0.5], [1.0, 0.5], 2));
    // A text frame without a height fits its text; text styles in add_shape.
    let r = call(&mut a, "add_shape", json!({"type": "text", "x": 0, "y": 10, "w": 200, "text": "Go", "font_size": 20, "weight": "bold", "align": "center", "name": "Label"})).unwrap();
    let label = r["id"].as_u64().unwrap();
    let ShapeKind::Text(t) = &a.doc.get(label).unwrap().kind else { panic!() };
    let [_, h] = t.frame.unwrap();
    assert!((20.0..40.0).contains(&h) && t.weight == 700 && t.align == crate::document::TextAlign::Center, "{h}");
    // Centre the label in the button.
    call(&mut a, "arrange", json!({"ids": [label], "within": bg, "axis": "y"})).unwrap();
    let lb = tools::ids_bounds(&mut a, &[label]).unwrap();
    assert!((lb.center().y - 25.0).abs() < 0.5, "{lb:?}");

    // Component, instances with overrides by layer name.
    let c = call(&mut a, "create_component", json!({"ids": [bg, label], "name": "Button"})).unwrap();
    let main = c["id"].as_u64().unwrap();
    assert_eq!((c["component"].clone(), c["name"].clone()), (json!("main"), json!("Button")));
    let r = call(&mut a, "instantiate", json!({"component": main, "instances": [{"at": [300, 0], "overrides": {"Label": {"text": "Buy"}, "Bg": {"fill": "#00ff00"}}}, {"at": [300, 100]}]})).unwrap();
    let inst = &r["instances"][0];
    assert_eq!((inst["bounds"][0].as_f64(), inst["component"].clone()), (Some(300.0), json!({"instance_of": main})));
    let lab = inst["layers"].as_array().unwrap().iter().find(|l| l["name"] == "Label").unwrap();
    assert_eq!((lab["text"].clone(), lab["main_layer"].clone()), (json!("Buy"), json!(label)));
    assert!(call(&mut a, "instantiate", json!({"component": main, "overrides": {"Nope": {"text": "x"}}})).unwrap_err().contains("Label"));
    assert!(call(&mut a, "instantiate", json!({"component": bg})).is_err());
    let l = call(&mut a, "list_components", json!({})).unwrap();
    assert_eq!(l["components"][0]["instances"].as_array().unwrap().len(), 2);
    assert_eq!(layer(&mut a, inst["id"].as_u64().unwrap())["component"], json!({"instance_of": main}));
    // Duplicate reports the new instance and its layers.
    let d = call(&mut a, "run_command", json!({"name": "Duplicate", "ids": [main]})).unwrap();
    assert_eq!(d["created"][0]["component"], json!({"instance_of": main}), "{d}");
    assert_eq!(d["created"][0]["layers"].as_array().unwrap().len(), 2);
    assert!(d.get("changed").is_none(), "{d}");

    // Several updates, one undo step; a bad one changes nothing.
    let n = a.history.undo_len();
    call(&mut a, "update_shapes", json!({"updates": [{"id": bg, "set": {"x": 5}}, {"id": label, "set": {"text": "Go!", "letter_spacing": 2}}]})).unwrap();
    assert_eq!(a.history.undo_len(), n + 1);
    let e = call(&mut a, "update_shapes", json!({"updates": [{"id": bg, "set": {"x": 50}}, {"id": 999999, "set": {"x": 1}}]})).unwrap_err();
    assert!(e.contains("updates[1]"), "{e}");
    assert_eq!(tools::ids_bounds(&mut a, &[bg]).unwrap().x0, 5.0);
    assert!(call(&mut a, "update_shapes", json!({"ids": [bg], "set": {"colour": "#fff"}})).unwrap_err().contains("colour"));
    // A patch switching Solid → Gradient replaces the variant.
    call(&mut a, "update_shapes", json!({"ids": [bg], "set": {"fill": "#123456"}})).unwrap();
    call(&mut a, "update_shapes", json!({"ids": [bg], "patch": {"fill": {"Gradient": {"kind": "Radial", "stops": [{"offset": 0.0, "color": {"r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0}}, {"offset": 1.0, "color": {"r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0}}], "start": [0.5, 0.5], "end": [1.0, 0.5]}}}})).unwrap();
    assert!(matches!(a.doc.get(bg).unwrap().fill, Some(Paint::Gradient(_))));
    call(&mut a, "update_shapes", json!({"ids": [bg], "set": {"stroke": "#000000", "stroke_align": "outside"}})).unwrap();
    assert_eq!(a.doc.get(bg).unwrap().stroke.as_ref().unwrap().align, crate::document::StrokeAlign::Outside);

    // Stacks: a row with a gap; new shapes below another; covering warns.
    let ids: Vec<u64> = (0..3).map(|i| call(&mut a, "add_shape", json!({"type": "rect", "x": i * 7, "y": 400, "w": 40, "h": 20 + i * 10})).unwrap()["id"].as_u64().unwrap()).collect();
    call(&mut a, "arrange", json!({"ids": ids, "stack": "row", "gap": 10, "at": [0, 400], "align": "end"})).unwrap();
    let xs: Vec<(f64, f64)> = ids.iter().map(|i| tools::ids_bounds(&mut a, &[*i]).unwrap()).map(|b| (b.x0, b.y1)).collect();
    assert_eq!(xs, [(0.0, 440.0), (50.0, 440.0), (100.0, 440.0)]);
    let cover = call(&mut a, "add_shape", json!({"type": "rect", "x": -10, "y": 390, "w": 300, "h": 100, "fill": "#ffffff"})).unwrap();
    assert!(cover["covers"].as_array().unwrap().contains(&json!(ids[0])), "{cover}");
    let under = call(&mut a, "add_shape", json!({"type": "rect", "x": -10, "y": 390, "w": 300, "h": 100, "fill": "#eeeeee", "below": ids[0]})).unwrap();
    assert!(under.get("covers").is_none());
    let under = under["id"].as_u64().unwrap();
    assert!(a.doc.index_of(under).unwrap() < a.doc.index_of(ids[0]).unwrap());

    // Timeline without keys; geometry left out of get_shapes.
    let tl = call(&mut a, "set_timeline", json!({"fps": 60, "duration": 2, "loop": false})).unwrap();
    assert_eq!((tl["fps"].as_f64(), tl["duration"].as_f64(), tl["loop"].clone()), (Some(60.0), Some(2.0), json!(false)));
    let p = call(&mut a, "add_shape", json!({"type": "path", "d": "M0 0 C10 0 10 10 20 20 L30 0 Z", "fill": "#000000"})).unwrap()["id"].as_u64().unwrap();
    let short = call(&mut a, "get_shapes", json!({"ids": [p]})).unwrap();
    assert!(short[0]["kind"]["Path"]["omitted"].as_str().unwrap().contains("nodes"), "{short}");
    let full = call(&mut a, "get_shapes", json!({"ids": [p], "geometry": true})).unwrap();
    assert!(full[0]["kind"]["Path"]["subpaths"].is_array());
    // A patch on summarised JSON keeps the nodes.
    call(&mut a, "update_shapes", json!({"ids": [p], "patch": {"kind": {"Path": {"omitted": "x"}}, "opacity": 0.5}})).unwrap();
    assert!(matches!(&a.doc.get(p).unwrap().kind, ShapeKind::Path(d) if !d.subpaths.is_empty()));
}

#[test]
fn svg_text_stays_text() {
    let mut a = app();
    call(&mut a, "new_document", json!({"w": 400, "h": 200, "name": "Hero"})).unwrap();
    assert_eq!(a.tab_infos()[0].name, "Hero");
    let svg = "<svg xmlns='http://www.w3.org/2000/svg'>\
        <text id='t1' x='200' y='100' text-anchor='middle' font-family='Noto Sans' font-size='40' font-weight='700' fill='#ff0000'>Hello</text>\
        <text id='t2' x='10' y='150' font-size='20'><tspan x='10' y='150'>one</tspan><tspan x='10' y='175'>two</tspan></text>\
        <text id='t3' font-size='20'><tspan x='10 30 50' y='190'>abc</tspan></text></svg>";
    let r = call(&mut a, "add_svg", json!({"svg": svg})).unwrap();
    let shapes = r["shapes"].as_array().unwrap();
    assert_eq!(shapes.iter().map(|s| s["type"].as_str().unwrap()).collect::<Vec<_>>(), ["Text", "Text", "Group"], "{r}");
    assert_eq!(shapes[2]["name"], "t3 (outlined text)");
    let t1 = a.doc.get(shapes[0]["id"].as_u64().unwrap()).unwrap().clone();
    let ShapeKind::Text(td) = &t1.kind else { panic!() };
    assert_eq!((td.content.as_str(), td.weight, td.size, td.align), ("Hello", 700, 40.0, crate::document::TextAlign::Center));
    // Centred on x = 200, its baseline at y = 100.
    let b = tools::ids_bounds(&mut a, &[t1.id]).unwrap();
    assert!((b.center().x - 200.0).abs() < 2.0 && b.y0 < 100.0 && b.y1 > 95.0, "{b:?}");
    let ShapeKind::Text(td) = &a.doc.get(shapes[1]["id"].as_u64().unwrap()).unwrap().kind else { panic!() };
    assert_eq!((td.content.as_str(), td.line_height), ("one\ntwo", 1.25));
}

#[test]
fn auto_layouts() {
    let mut a = app();
    let label = call(&mut a, "add_shape", json!({"type": "text", "x": 100, "y": 100, "text": "Go", "font_size": 20, "name": "Label"})).unwrap()["id"].as_u64().unwrap();
    let lb = tools::ids_bounds(&mut a, &[label]).unwrap();
    // A button: the label padded inside a filled, rounded background.
    let r = call(&mut a, "auto_layout", json!({"ids": [label], "padding": [10, 20], "fill": "#0A84FF", "radius": 8, "name": "Button"})).unwrap();
    let btn = r["id"].as_u64().unwrap();
    assert_eq!(r["layout"]["padding"], json!([10.0, 20.0, 10.0, 20.0]), "{r}");
    let b = tools::ids_bounds(&mut a, &[btn]).unwrap();
    assert!((b.width() - (lb.width() + 40.0)).abs() < 0.5 && (b.height() - (lb.height() + 20.0)).abs() < 0.5, "{b:?} {lb:?}");
    let bg = r["layout"]["background"].as_u64().unwrap();
    assert!(matches!(a.doc.get(bg).unwrap().kind, ShapeKind::Rect { radii, .. } if radii[0] == 8.0));
    assert_eq!(layer(&mut a, btn)["layout"]["direction"], "row");
    // Longer text: the button grows (one undo step for the edit).
    call(&mut a, "update_shapes", json!({"ids": [label], "set": {"text": "Go somewhere"}})).unwrap();
    let b2 = tools::ids_bounds(&mut a, &[btn]).unwrap();
    assert!(b2.width() > b.width() + 20.0, "{b2:?}");
    // Items added with parent / index flow in; order rearranges them.
    let icon = call(&mut a, "add_shape", json!({"type": "ellipse", "w": 16, "h": 16, "parent": btn, "index": 0, "name": "Dot"})).unwrap()["id"].as_u64().unwrap();
    let (ib, tb) = (tools::ids_bounds(&mut a, &[icon]).unwrap(), tools::ids_bounds(&mut a, &[label]).unwrap());
    assert!(ib.x1 <= tb.x0 && (ib.x0 - (b2.x0 + 20.0)).abs() < 0.5, "{ib:?} {tb:?}");
    let r = call(&mut a, "auto_layout", json!({"id": btn, "order": [label, icon], "gap": 4})).unwrap();
    assert_eq!(r["members"].as_array().unwrap().iter().map(|m| m["id"].as_u64().unwrap()).collect::<Vec<_>>(), [label, icon]);
    let (ib, tb) = (tools::ids_bounds(&mut a, &[icon]).unwrap(), tools::ids_bounds(&mut a, &[label]).unwrap());
    assert!((ib.x0 - tb.x1 - 4.0).abs() < 0.5, "{ib:?} {tb:?}");
    assert!(call(&mut a, "auto_layout", json!({"id": btn, "order": [12345]})).is_err());
    assert!(call(&mut a, "auto_layout", json!({"id": btn, "direction": "diagonal"})).is_err());
    assert!(call(&mut a, "arrange", json!({"ids": [label], "stack": "row"})).unwrap_err().contains("auto layout"));
    // A column card holding the button; nested layouts reflow together.
    let title = call(&mut a, "add_shape", json!({"type": "text", "x": 0, "y": 0, "text": "Title", "font_size": 30})).unwrap()["id"].as_u64().unwrap();
    let card = call(&mut a, "auto_layout", json!({"ids": [title, btn], "direction": "column", "align": "start", "gap": 16, "padding": 24, "fill": "#ffffff"})).unwrap()["id"].as_u64().unwrap();
    let (cb, tt, bb) = (tools::ids_bounds(&mut a, &[card]).unwrap(), tools::ids_bounds(&mut a, &[title]).unwrap(), tools::ids_bounds(&mut a, &[btn]).unwrap());
    assert!((tt.x0 - cb.x0 - 24.0).abs() < 0.5 && (bb.x0 - cb.x0 - 24.0).abs() < 0.5 && bb.y0 >= tt.y1 + 15.5, "{cb:?} {tt:?} {bb:?}");
    // An auto-layout component: instances reflow to their own text.
    let main = call(&mut a, "create_component", json!({"ids": [btn]})).unwrap()["id"].as_u64().unwrap();
    let r = call(&mut a, "instantiate", json!({"component": main, "at": [500, 500], "overrides": {"Label": {"text": "A much longer label here"}}})).unwrap();
    let inst = r["instances"][0]["id"].as_u64().unwrap();
    let (mb, ib) = (tools::ids_bounds(&mut a, &[main]).unwrap(), tools::ids_bounds(&mut a, &[inst]).unwrap());
    assert!(ib.width() > mb.width() + 40.0, "{mb:?} {ib:?}");
    let rev = a.doc.revision();
    a.components_tick();
    a.layout_key = None;
    a.layout_tick();
    assert_eq!(a.doc.revision(), rev, "instance layout is stable");
    // Removing leaves a plain group; constraints in set.
    call(&mut a, "auto_layout", json!({"id": card, "remove": true})).unwrap();
    assert!(a.doc.get(card).unwrap().layout.is_none());
    call(&mut a, "update_shapes", json!({"ids": [title], "set": {"constraints": {"h": "both", "v": "top"}}})).unwrap();
    let c = a.doc.get(title).unwrap().constraints.unwrap();
    assert_eq!((c.h, c.v), (crate::layout::Pin::Both, crate::layout::Pin::Start));
}
