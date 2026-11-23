//! The MCP tools: what an agent can ask of the app. Each call runs on the
//! UI thread against the live `App` (the window, or a headless one), so
//! edits go through the same history (undo) as the user's.

use serde_json::{Value, json};

use crate::app::App;
use crate::commands::Cmd;
use crate::document::{Paint, Rgba, Shape, ShapeKind, Stroke, TextData};
use crate::headless::Pointer;

/// Part of a tool result.
pub enum Content {
    Text(String),
    Png(Vec<u8>),
}

/// What a call needs before it can answer.
pub enum Outcome {
    Done(Vec<Content>),
    /// Feed these event batches (one per frame), let a frame show the
    /// result, then answer.
    Later(Vec<Vec<egui::Event>>, After),
}

/// The answer to a deferred call.
pub enum After {
    /// A short state summary.
    State,
    /// The widget list (filter, max lines).
    UiTree(String, usize),
    /// What happened to a widget that was clicked or typed into.
    Widget(String),
    /// Capture the window, scaled to fit this many pixels wide (by default
    /// one pixel per window point, the unit `input` uses).
    Screenshot(Option<u32>),
    /// Wait for the background export to `path` to finish.
    Export(std::path::PathBuf),
}

/// A tool result as MCP's `CallToolResult`.
pub fn result(r: Result<Vec<Content>, String>) -> Value {
    use base64::Engine;
    match r {
        Ok(content) => {
            let content: Vec<Value> = content
                .into_iter()
                .map(|c| match c {
                    Content::Text(t) => json!({"type": "text", "text": t}),
                    Content::Png(p) => json!({"type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(p)}),
                })
                .collect();
            json!({"content": content, "isError": false})
        }
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    }
}

pub const INSTRUCTIONS: &str = "\
mtzdVector is a vector editor (like Figma / Affinity) with keyframe animation. These \
tools act on its open window live (the user watches and can undo each call), or on a \
hidden instance when none is open.

Fast paths (use these before touching the UI):
- Look: get_state (artboards, layer tree with ids and bounds), render (artwork PNG), \
screenshot (whole window; only when the UI itself matters).
- Draw: add_svg for anything non-trivial (one call, any SVG: paths, gradients, text \
(stays editable); <g id=\"…\"> stays a group named after its id); add_shape for single \
primitives and text; add_icon for Iconify icons (mdi:home, twemoji:frog). New shapes go on \
top: pass below / above to stack them. Coordinates are document px, y down; the first \
artboard is usually at 0,0. new_document {w, h, background, name} / set_artboard size the canvas.
- Change: update_shapes (set: x/y/w/h/rotate/fill (colour or gradient)/stroke/opacity/text \
styles…; patch: JSON merge patch on get_shapes JSON for everything else; updates: [{id, set, \
patch}…] for many different changes in one call). arrange stacks shapes with a gap / centres \
them in another. run_command for menu actions (Union, Group, AlignLeft, Undo… see \
list_commands). Edits answer with what they created / changed / deleted, not the whole tree.
- Layout: auto_layout makes a group that keeps its items in a row / column with gap and \
padding (buttons, cards, nav bars, lists; nest them); add items with parent: <its id>. \
help layout.
- Reuse: create_component, then instantiate {component, at, overrides: {\"<layer name>\": \
{text, fill…}}} (or instances: [...] for many). help components.
- Animate: ONE animate call per shape with all its keys (a few, with ease \"smooth\" for \
flowing motion; origin for joints; oscillate for waving / bobbing / breathing), e.g. \
{id, duration: 2, keys: [{t: 0, move: [0, 0], ease: \"ease_in\"}, {t: 1, move: [0, 700], \
ease: \"ease_out\"}, {t: 2, move: [0, 0]}]}. Don't build keys with seek + edits or \
the timeline UI. It answers with each shape's bounds at every key time; check \
frames with render {times: [0, 0.5, 1]} (a contact sheet) or render {onion: [...]} (ghosts, \
for motion paths); the playhead stays put. playback {playing: true} plays it in the window.
- Export: export {path}; format from the extension (.webm video, .json Lottie, .svg \
animated SVG, .png…). Videos encode in the background; the call returns when done.

Call help(topic) for details: animation, shapes, text, layout, components, ui, export. The rest of the \
interface (any panel, dialog or option) is reachable with ui_tree + ui_click / ui_type.";

/// Longer guides, by topic.
fn help(topic: &str) -> String {
    let t = topic.to_lowercase();
    let topics: [(&str, &str); 7] = [
        ("animation", "\
ANIMATION
- animate {id | ids, keys, duration?, fps?, loop?, replace? (default true: replaces the tracks the keys touch)}.
- A key: {t (seconds), ease?, and any of:
    move [dx, dy]   offset from where the shape rests (its position when not animated)
    center [x, y]   absolute document position of the shape's centre (alternative to move)
    rotate deg      rotation about the centre, absolute (0 = rest)
    scale s | [sx, sy]   about the centre, 1 = rest size
    skew deg
    opacity 0..1, fill \"#RRGGBB\", stroke \"#RRGGBB\", stroke_width, blur, font_size, letter_spacing,
    text \"…\" (typed on between keys), visible true/false, dash_offset}
  Parts a key leaves out carry on from the key before (a key with only rotate keeps the move).
- ease (from this key to the next): linear, smooth (a curve through the neighbouring keys: no stop \
at keys mid-motion, no overshoot; 5–8 keys give natural motion), ease_in_out (default), ease_in \
(accelerate: falling), ease_out (decelerate: rising, landing), hold (jump), back_in (anticipate), \
back_out (overshoot), back_in_out, or a CSS cubic-bezier [x1, y1, x2, y2] (y may leave 0..1). \
`ease` on the animate call is the default for keys that don't name one.
- origin: the pivot for rotate and scale, kept on the shape: [x, y] in document space where the \
shape rests (a shoulder), or an anchor of its rest bounds: center, top, bottom, left, right, \
top-left, top-right, bottom-left, bottom-right. `animate {id, origin}` alone just sets it. With it, \
an arm swinging from the shoulder is just rotate keys.
- Rigs: group the parts (body > upper arm > forearm, e.g. nested <g id> in add_svg) and key each \
level: a child's keys act inside its parent's motion, so each part only needs its own rotation \
about its own origin.
- oscillate: a sine layered on the keys, baked into smooth keys: {prop: rotate | move_x | move_y \
| scale | scale_x | scale_y | skew | opacity | blur, amp (deg / px / scale / opacity), freq (Hz), \
phase (deg), from?, to?, envelope?: [[t, gain], …] (gain eases between points)} or a list. \
Waving = rotate about the shoulder with an envelope; bobbing = move_y; breathing = scale.
- With more than 40 keys animate answers with a summary (key counts, spans, bounds_envelope); \
verbose: true lists everything.
- Physics recipes: falling = ease_in into the impact key; rising = ease_out into the apex; \
squash on impact = a key with scale [1.25, 0.75] for ~0.08 s; bounces get lower and shorter each time.
- Keys never change where a shape rests, whatever the first key is. animate answers with \
bounds_at_keys (each shape's bounds at every key time): check them before rendering. \
get_state lists rest_bounds next to bounds (at the playhead) for shapes with motion.
- Look at frames without moving the playhead: render {time} or render {times: [...]} (a contact \
sheet, left to right then down) or render {onion: [t…], time?} (faint ghosts of those moments \
under the frame: shows paths, jerks and pivot drift). seek t moves the playhead (the user sees \
that moment); playback {playing, time?, loop?} plays or pauses (idempotent).
- The timeline: duration, fps, loop live on the document (get_animation shows them; \
set_timeline {duration, fps, loop, range} sets them without keys). \
get_animation lists every key exactly as stored; clear_animation {ids, props?, keep?} removes \
tracks and puts the shapes back at rest (keep: \"current\" leaves them as they are now).
- Groups animate as a whole; children can have their own keys on top.
- Editing a shape that has keys (update_shapes) while the playhead is at t changes / adds \
the key at t for what changed. New shapes aren't selected (add_* select=true to select them).
"),
        ("shapes", "\
SHAPES
- add_svg is the quickest way to build artwork: whole illustrations in one call; `group` names a \
group around the result; `at` [x, y] moves its top-left there. Give elements an `id`: it \
becomes the layer name (circles, rects… arrive as paths). The answer lists every new shape's id, \
name and bounds.
- add_shape types: rect (radius), ellipse, star (points, inner), polygon (points = sides), \
path (d = SVG path data), text. Style: fill, stroke, stroke_width, stroke_align (center, \
inside, outside), cap (butt default, round, square), join (miter default, round, bevel), \
opacity, name; parent = a group id; below / above = a shape id to stack it next to (default: \
on top of everything; the answer warns with `covers` when it lands over existing shapes).
- Fills: \"#RRGGBB\" / \"#RRGGBBAA\", null (none), a CSS gradient string \
\"linear-gradient(90deg, #FF5F6D, #FFC371 80%)\" (angles as CSS: 0 up, 90 right; or to right…), \
\"radial-gradient(#ffffff, #000000)\", or {gradient: linear | radial, stops: [\"#hex\" | [\"#hex\", \
offset 0..1]…], angle? | start? / end?}. Natively (get_shapes) a gradient is {\"Gradient\": {kind: \
Linear | Radial | Conic, stops: [{offset, color: {r, g, b, a 0..1}, mid}], start, end}}: start / \
end are in the shape's bounding-box units (0..1 across its box; radial: centre and a point on \
the edge). Strokes are solid colours; gradient strokes from SVG become filled outlines.
- update_shapes set: x, y, w, h (bounds), rotate (relative degrees), fill/stroke (null removes), \
stroke_width, stroke_align, opacity, visible, locked, name, and the text styles. patch: JSON \
merge patch on the get_shapes JSON (shadow, blur, dash, corner radii, blend…); giving another \
enum variant replaces the old one ({\"fill\": {\"Gradient\": {…}}} over a Solid fill). \
updates: [{id | ids, set?, patch?}, …] applies many different changes as one undo step.
- arrange {ids, stack: row | column, gap, align, at} lays shapes out in the given order; \
{ids, within: id, align?, axis?, padding?} centres (or aligns) them inside another shape, \
e.g. a label in its button.
- get_shapes leaves path nodes out (counts only) unless geometry: true.
- Booleans / grouping / arranging / aligning: run_command with ids (Union, Subtract, Intersect, \
Exclude, Group, Ungroup, BringToFront, AlignLeft, DistributeH, MakeClip, MakeMask, ConvertToCurves…).
- Layers in get_state are listed top (front) first.
"),
        ("text", "\
TEXT
- add_shape {type: text, text, x, y, font_size, font_family, weight (400, 700 or bold…), italic, \
align (left, center, right, justify), line_height (× size), letter_spacing (px), fill}. With w it \
is a frame that wraps (height fits the text unless h is given), otherwise it grows with its \
content. x, y is the top-left. The same fields work in update_shapes set (fit_height: true \
refits a frame's height).
- Text answers carry box (the layout box: frame or lines) next to bounds (the letters' \
outlines): align and place by box. To centre a label in a button: a frame as wide as the \
button with align center, or arrange {ids: [label], within: button}.
- add_svg keeps <text> as text (one style per <text>; text on a path or with per-letter \
positions becomes outlines).
- list_fonts shows installed families and Google Fonts (downloaded on first use; the text \
updates when it arrives). Per-character styles: patch the Text kind's spans in get_shapes JSON.
"),
        ("layout", "\
AUTO LAYOUT
- auto_layout {ids, direction: row | column, gap, padding, align, fill?, radius?, stroke?, \
name?} wraps shapes in an auto layout: they stay in order with that gap and padding, and it \
reflows when an item changes size (longer text), is added or removed. Its background \
rectangle is sized to the content + padding: fill / radius / stroke make it a button, chip, \
card or bar. A button = auto_layout {ids: [label], padding: [12, 24], fill: \"#0A84FF\", \
radius: 8}.
- Nest them: a card (column) holding a title, text and a row of buttons; a page (column) of \
sections. Make an auto layout a component and its instances reflow with their overrides.
- Change one: auto_layout {id, gap?, padding?, align?, direction?, fill?, order: [item ids]}. \
remove: true leaves a plain group.
- Add items: add_shape / add_svg / add_icon / instantiate with parent: <layout id> and \
index (0 = first; default last). Remove them with delete_shapes. Items are placed by the \
layout: moving one with update_shapes x / y re-orders it by where it lands; use order.
- get_state shows layout: {direction, gap, padding, align, background} on such groups.
- arrange is a one-off move (stack or centre once), not a layout.
- constraints {h, v} (update_shapes set; start, end, both, center, scale) say how a shape \
follows when its (plain) group is resized: both = stretch, end = stay at the right / bottom.
"),
        ("components", "\
COMPONENTS
- A main component is a group that holds the source artwork; its instances mirror it and \
follow its edits, keeping their own overrides (text, fill, stroke, opacity, visibility, name \
per layer).
- create_component {ids, name?}: makes them a main (one shape that isn't a group is wrapped \
in one named after it).
- instantiate {component: <main id>, at: [x, y] (top-left), overrides: {\"<layer name>\": {text, \
fill, stroke, stroke_width, opacity, visible, font_size, weight, …}}, name?} or several in one \
call: instances: [{at, overrides, name}, …]. Layers are named by name or path (\"Card/Title\"), \
like list_components shows. The answer lists each instance's layers with their ids.
- list_components: mains, their layers and instances. get_state marks them component: \
\"main\" / {instance_of: id}.
- Restyle a layer inside an instance with update_shapes (it becomes an override); edit the main \
to change every instance. run_command DetachInstance / ResetInstance / GoToMain with ids.
- Duplicate (run_command) of a main also makes an instance.
"),
        ("ui", "\
UI
- ui_tree lists widgets on screen: `#id role \"label\" = value [x,y wxh] (flags)`; filter by text \
or role (button, number, textbox, checkbox, tab, menuitem, dropdown). Coordinates are window points.
- ui_click {id | label, role?, index?}, ui_type {id | label, text} (selects the old value, types, Enter).
- Menus: ui_click the menu name, then ui_tree again for its items. Dialogs list their fields while open.
- input sends raw pointer/keys (click X Y, drag …, key S ctrl, dclick in document coordinates).
- screenshot is in window points (same as ui_tree); pass max_width to shrink it.
- Prefer dedicated tools (add_*, update_shapes, animate, run_command, patch_document) over \
driving the UI: they are one call each and exact.
"),
        ("export", "\
EXPORT
- export {path, area?: artboard | all | selection, scale?, transparent?}. Format from the \
extension: png jpg webp avif svg pdf eps dxf, webm (video of the timeline), json (Lottie), \
svg of an animated document plays with CSS.
- Video and other animated exports run in the background with progress in the window's status \
bar; the call waits for the file. Use scale < 1 (e.g. 0.5) for quick previews.
- render is for looking: a PNG of the artboard (or ids) returned to you, nothing written; \
time / times / onion render other moments of the animation; sizes [16, 32, 180] shows an icon at \
those actual pixel sizes in one strip.
- Artboards: new_document {w, h, background, name}; set_artboard {id?, x, y, w, h, background \
(null = transparent), name}. get_state shows a transparent background as #RRGGBB00.
- save {path} writes the native .mvec document.
"),
    ];
    match topics.iter().find(|(k, _)| t.contains(k) || k.contains(t.as_str()) && !t.is_empty()) {
        Some((_, body)) => body.to_string(),
        None => format!("Topics: {}. Example: help {{\"topic\": \"animation\"}}", topics.map(|t| t.0).join(", ")),
    }
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

const IDS: &str = "Shape ids (from get_state)";

/// `tools/list` entries.
pub fn list() -> Vec<Value> {
    let ids = json!({"type": "array", "items": {"type": "integer"}, "description": IDS});
    let color = json!({"type": ["string", "null"], "description": "#RRGGBB or #RRGGBBAA; null removes it"});
    let fill = json!({"description": "#RRGGBB[AA]; a CSS \"linear-gradient(90deg, #f00, #00f)\" / \"radial-gradient(#fff, #000)\"; {gradient: linear|radial, stops: [\"#RRGGBB\" | [\"#RRGGBB\", offset]…], angle?}; or null (none)"});
    let text_style = json!({
        "text": {"type": "string"}, "font_size": {"type": "number"}, "font_family": {"type": "string"},
        "weight": {"description": "100..900 (400 regular, 700 bold) or a name: light, medium, semibold, bold…"},
        "italic": {"type": "boolean"},
        "align": {"type": "string", "enum": ["left", "center", "right", "justify"]},
        "line_height": {"type": "number", "description": "multiple of the font size (default 1.2)"},
        "letter_spacing": {"type": "number", "description": "px between letters"},
    });
    let stack = json!({
        "below": {"type": "integer", "description": "put it just under this shape (same group)"},
        "above": {"type": "integer", "description": "put it just over this shape (same group)"},
        "parent": {"type": "integer", "description": "group or auto layout to add into (default: top level)"},
        "index": {"type": "integer", "description": "with parent: position among its children / an auto layout's items (0 = first; default last)"},
    });
    let style = json!({
        "name": {"type": "string"},
        "fill": fill,
        "stroke": color,
        "stroke_width": {"type": "number"},
        "cap": {"type": "string", "enum": ["butt", "round", "square"], "description": "stroke ends (default butt)"},
        "join": {"type": "string", "enum": ["miter", "round", "bevel"], "description": "stroke corners (default miter)"},
        "stroke_align": {"type": "string", "enum": ["center", "inside", "outside"]},
        "constraints": {"description": "how it follows when its group is resized: {h, v}, each start | end | both (stretch) | center | scale (default)"},
        "opacity": {"type": "number", "description": "0..1"},
    });
    let mut add_props = json!({
        "select": {"type": "boolean", "description": "select the new shape"},
        "type": {"type": "string", "enum": ["rect", "ellipse", "star", "polygon", "path", "text"]},
        "x": {"type": "number"}, "y": {"type": "number"},
        "w": {"type": "number"}, "h": {"type": "number"},
        "radius": {"type": "number", "description": "rect corner radius"},
        "points": {"type": "integer", "description": "star points / polygon sides"},
        "inner": {"type": "number", "description": "star inner radius ratio 0..1"},
        "d": {"type": "string", "description": "path: SVG path data in document coordinates"},
    });
    merge(&mut add_props, &style);
    merge(&mut add_props, &text_style);
    merge(&mut add_props, &stack);
    let mut set_props = json!({
        "x": {"type": "number"}, "y": {"type": "number"}, "w": {"type": "number"}, "h": {"type": "number"},
        "rotate": {"type": "number", "description": "degrees, clockwise, about the centre (relative)"},
        "visible": {"type": "boolean"}, "locked": {"type": "boolean"},
        "fit_height": {"type": "boolean", "description": "text frames: height to fit the text"},
    });
    merge(&mut set_props, &style);
    merge(&mut set_props, &text_style);
    let full = json!({"type": "boolean", "description": "answer with the whole state instead of what changed"});
    let mut svg_props = json!({"svg": {"type": "string"}, "at": {"type": "array", "items": {"type": "number"}, "description": "[x, y]: put the top-left of the result here"}, "group": {"type": "string", "description": "wrap the result in a group of this name"}, "select": {"type": "boolean"}});
    merge(&mut svg_props, &stack);
    let mut inst_props = json!({
        "component": {"type": "integer", "description": "the main component's id (list_components)"},
        "at": {"type": "array", "items": {"type": "number"}, "description": "[x, y] top-left (default: right of the main)"},
        "overrides": {"type": "object", "description": "{\"<layer name or path Card/Title>\": {text, fill, stroke, stroke_width, opacity, visible, font_size, weight, …}}"},
        "name": {"type": "string"},
        "instances": {"type": "array", "items": {"type": "object"}, "description": "several at once: [{at, overrides?, name?}, …]"},
        "select": {"type": "boolean"},
    });
    merge(&mut inst_props, &stack);
    let tool = |name: &str, description: &str, schema: Value| json!({"name": name, "description": description, "inputSchema": schema});
    vec![
        tool(
            "get_state",
            "Open documents, artboards, selection, active tool, animation and the layer tree (id, type, name, bounds [x, y, w, h]).",
            obj(json!({"depth": {"type": "integer", "description": "group levels to list (default 3)"}, "limit": {"type": "integer", "description": "max shapes listed (default 400)"}}), &[]),
        ),
        tool(
            "get_shapes",
            "Full JSON of shapes (the native format): every style and effect field. Path geometry is summarised (node counts) unless geometry=true.",
            obj(json!({"ids": ids, "geometry": {"type": "boolean", "description": "include path nodes (default false)"}}), &["ids"]),
        ),
        tool(
            "add_svg",
            "Add artwork from SVG markup (any SVG: paths, groups, gradients, text, images). SVG user units are document pixels unless `at` moves the result. Elements' `id` attributes become layer names (give each one an id to tell them apart; circles and the like arrive as paths). <text> in one style stays editable text (fonts from Google Fonts download); text on paths or with per-letter positions becomes outlines. New shapes go on top unless below / above. Returns each new shape's id, name and bounds, and warns when it covers existing shapes. Doesn't change the selection unless select=true.",
            obj(svg_props, &["svg"]),
        ),
        tool("add_shape", "Add one shape (on top unless below / above). Text: x, y is the top-left; with w it wraps in a frame that fits its text (unless h). Returns its id and bounds (text: also box, its layout box). Doesn't change the selection unless select=true.", obj(add_props, &["type"])),
        tool(
            "update_shapes",
            "Change shapes. `set` takes simple fields (x/y/w/h set the bounds, fill takes colours or gradients, text styles); `patch` is a JSON merge patch on the get_shapes JSON for anything else (shadows, dashes, text spans…). Many different changes in one call (one undo step): updates: [{id | ids, set?, patch?}, …].",
            obj(
                json!({"ids": ids, "set": {"type": "object", "properties": set_props}, "patch": {"type": "object"}, "updates": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "integer"}, "ids": {"type": "array", "items": {"type": "integer"}}, "set": {"type": "object"}, "patch": {"type": "object"}}}, "description": "per-shape changes: [{id, set?, patch?}, …]"}}),
                &[],
            ),
        ),
        tool("delete_shapes", "Delete shapes.", obj(json!({"ids": ids}), &["ids"])),
        tool(
            "arrange",
            "Move shapes once (one undo step): stack them in a row or column (in the order given) with a gap, and / or align them inside another shape (within: e.g. a label in its button, align center). Returns their bounds. For layouts that stay arranged (reflow when content changes) use auto_layout.",
            obj(
                json!({
                    "ids": ids,
                    "stack": {"type": "string", "enum": ["row", "column"]},
                    "gap": {"type": "number"},
                    "at": {"type": "array", "items": {"type": "number"}, "description": "stack: top-left of the first (default: where it is)"},
                    "align": {"type": "string", "enum": ["start", "center", "end"], "description": "cross-axis in a stack (default start); inside `within` (default center)"},
                    "within": {"type": "integer", "description": "align the ids (as a block) inside this shape's bounds"},
                    "axis": {"type": "string", "enum": ["both", "x", "y"], "description": "within: which axes (default both)"},
                    "padding": {"type": "number", "description": "within: inset from its edges"},
                }),
                &["ids"],
            ),
        ),
        tool(
            "auto_layout",
            "Auto layout (like Figma's): a group that keeps its items in a row or column with a gap and padding, reflowing when they change size, are added (parent: its id) or removed. ids: wrap these shapes in a new one, in that order (or id: make a group one); id of an auto layout: change it. fill / stroke / radius style its background (sized to content + padding: a button, card, nav bar). order sets the item order. remove: true turns it back into a plain group. One undo step; answers with its layout and items.",
            obj(
                json!({
                    "ids": ids,
                    "id": {"type": "integer", "description": "a group to make an auto layout, or an auto layout to change"},
                    "direction": {"type": "string", "enum": ["row", "column"], "description": "default row"},
                    "gap": {"type": "number", "description": "space between items (default 12)"},
                    "padding": {"description": "n | [vertical, horizontal] | [top, right, bottom, left] (default 12)"},
                    "align": {"type": "string", "enum": ["start", "center", "end"], "description": "items across the flow (default center)"},
                    "fill": fill,
                    "stroke": color,
                    "stroke_width": {"type": "number"},
                    "radius": {"type": "number", "description": "background corner radius"},
                    "order": {"type": "array", "items": {"type": "integer"}, "description": "item ids in flow order"},
                    "name": {"type": "string"},
                    "remove": {"type": "boolean"},
                }),
                &[],
            ),
        ),
        tool(
            "create_component",
            "Make shapes a main component (a group; one shape that isn't a group is wrapped, the component taking its name). Copies made with instantiate (or Duplicate) are linked instances.",
            obj(json!({"ids": ids, "name": {"type": "string"}}), &["ids"]),
        ),
        tool(
            "instantiate",
            "Place instances of a main component: at [x, y] (top-left), overrides by layer name ({\"Label\": {text: \"Buy\"}, \"Bg\": {fill: \"#0A84FF\"}}), or several at once with instances: [{at, overrides, name}…]. One undo step. Returns each instance with its layers (ids) and the main layer each mirrors.",
            obj(inst_props, &["component"]),
        ),
        tool("list_components", "Main components: id, name, bounds, layers (names / paths to use in overrides) and their instances.", obj(json!({}), &[])),
        tool(
            "add_icon",
            "Add an icon from Iconify (200 000+ open icons: mdi:home, lucide:search, twemoji:frog, logos:github…; browse icon-sets.iconify.design). size px tall (default 64); color tints monochrome icons. A group named after the icon.",
            obj(
                json!({"name": {"type": "string", "description": "set:icon"}, "at": {"type": "array", "items": {"type": "number"}, "description": "[x, y] top-left"}, "size": {"type": "number"}, "color": {"type": "string"}, "parent": {"type": "integer"}, "index": {"type": "integer"}, "below": {"type": "integer"}, "above": {"type": "integer"}, "select": {"type": "boolean"}}),
                &["name"],
            ),
        ),
        tool("select", "Set the selection (empty: clear). Commands act on it.", obj(json!({"ids": ids, "add": {"type": "boolean"}}), &["ids"])),
        tool(
            "list_commands",
            "Menu commands: name, label, category, shortcut and whether they apply right now.",
            obj(json!({"filter": {"type": "string", "description": "substring of name, label or category"}}), &[]),
        ),
        tool(
            "run_command",
            "Run a menu command by name or label (Union, Group, AlignLeft, Undo, ZoomFit, \"Opacity 50%\"…), optionally selecting `ids` first. Answers with what changed: created (with their layers), changed, deleted, restacked, selection. File dialogs aren't available: use open_file, save, export, place_file.",
            obj(json!({"name": {"type": "string"}, "ids": ids, "full_state": full}), &["name"]),
        ),
        tool("set_tool", "Pick a tool (select, node, pen, rect, ellipse, text, pencil, brush, knife, arrow…), e.g. before `input`.", obj(json!({"name": {"type": "string"}}), &["name"])),
        tool(
            "input",
            "Send pointer and keyboard input to the window, one command per line: move X Y | click X Y [right] | dblclick X Y | drag X1 Y1 X2 Y2 [steps] | down [right] | up [right] | mods shift alt ctrl | mods none | key NAME [ctrl] [shift] [alt] | type TEXT. Coordinates are window points; prefix with d (dclick, ddrag, dmove, ddblclick) for document coordinates.",
            obj(json!({"script": {"type": "string"}}), &["script"]),
        ),
        tool(
            "render",
            "Render the artwork (as exported) to PNG: the active artboard, an artboard, or shapes. `time` renders another moment of the animation, `times` a contact sheet of several (left to right, top to bottom), `onion` faint ghosts of other moments over `time` (motion paths, pivot drift); none of them moves the playhead. `sizes` [16, 32, 180] renders one strip at those actual pixel sizes (icons, favicons).",
            obj(
                json!({"artboard": {"type": "integer"}, "ids": ids, "max_size": {"type": "integer", "description": "longest side in px (default 1024; per frame with `times`, default 384)"}, "time": {"type": "number"}, "times": {"type": "array", "items": {"type": "number"}}, "onion": {"type": "array", "items": {"type": "number"}}, "sizes": {"type": "array", "items": {"type": "integer"}}}),
                &[],
            ),
        ),
        tool(
            "screenshot",
            "Screenshot of the whole app window (panels, canvas, dialogs), 1 px per window point. Use render to look at the artwork instead.",
            obj(json!({"max_width": {"type": "integer", "description": "shrink to this width"}}), &[]),
        ),
        tool("open_file", "Open a document in a new tab (.mvec, SVG, PDF, AI, EPS, Lottie).", obj(json!({"path": {"type": "string"}}), &["path"])),
        tool(
            "new_document",
            "Start a new document in a new tab; its artboard is w × h (default 1920 × 1080) with `background` (null: transparent); `name` names the document (its tab) and the artboard.",
            obj(json!({"w": {"type": "number"}, "h": {"type": "number"}, "background": color, "name": {"type": "string"}}), &[]),
        ),
        tool(
            "set_artboard",
            "Change one artboard (default: the active one): position, size, background (null: transparent), name, clip. One undo step.",
            obj(json!({"id": {"type": "integer"}, "x": {"type": "number"}, "y": {"type": "number"}, "w": {"type": "number"}, "h": {"type": "number"}, "background": color, "name": {"type": "string"}, "clip": {"type": "boolean"}, "full_state": full}), &[]),
        ),
        tool("place_file", "Place a file (vector or image) into the document. Answers with what was created.", obj(json!({"path": {"type": "string"}, "at": {"type": "array", "items": {"type": "number"}, "description": "[x, y] centre"}, "full_state": full}), &["path"])),
        tool("save", "Save the document (.mvec). `path` is needed if it was never saved.", obj(json!({"path": {"type": "string"}}), &[])),
        tool(
            "export",
            "Export to a file; the format comes from the extension (png, jpg, webp, avif, svg, pdf, eps, dxf, webm, json = Lottie).",
            obj(
                json!({"path": {"type": "string"}, "area": {"type": "string", "enum": ["artboard", "all", "selection"]}, "scale": {"type": "number"}, "transparent": {"type": "boolean"}}),
                &["path"],
            ),
        ),
        tool(
            "ui_tree",
            "Every widget in the window right now (menus, panels, dialogs, buttons, fields, checkboxes, tabs, labels) as `#id role \"label\" = value [x,y wxh] (flags)`, indented by nesting. Use with ui_click / ui_type to operate any part of the app, e.g. open a menu, then click its item.",
            obj(json!({"filter": {"type": "string", "description": "only widgets whose label/value contains this, or a role (button, textbox, checkbox, menuitem, tab…)"}, "max": {"type": "integer"}}), &[]),
        ),
        tool(
            "ui_click",
            "Click a widget from ui_tree by `id`, or by `label` (exact, then substring; `role` and `index` disambiguate).",
            obj(json!({"id": {"type": "string", "description": "from ui_tree, e.g. \"#3fa2b1c\""}, "label": {"type": "string"}, "role": {"type": "string"}, "index": {"type": "integer"}, "right": {"type": "boolean"}, "double": {"type": "boolean"}}), &[]),
        ),
        tool(
            "ui_type",
            "Type into a text or number field from ui_tree: clicks it, selects its contents (unless `append`), types, and presses Enter (unless enter=false).",
            obj(json!({"id": {"type": "string", "description": "from ui_tree, e.g. \"#3fa2b1c\""}, "label": {"type": "string"}, "role": {"type": "string"}, "index": {"type": "integer"}, "text": {"type": "string"}, "append": {"type": "boolean"}, "enter": {"type": "boolean"}}), &["text"]),
        ),
        tool(
            "get_document",
            "Document-level JSON: artboards (size, background, export slices), guides, grid, unit, dpi, colour keys, text styles, animation timeline, image list. Shapes are left out (use get_state / get_shapes).",
            obj(json!({}), &[]),
        ),
        tool(
            "patch_document",
            "JSON merge patch on get_document's JSON (colour keys, text styles, guides, grid, unit, animation duration/fps…). `artboards` as an array replaces them all; as an object {\"<id>\": {…}} patches those artboards only (or use set_artboard). One undo step.",
            obj(json!({"patch": {"type": "object"}, "full_state": full}), &["patch"]),
        ),
        tool("get_settings", "App preferences and workspace (panel visibility, snapping, nudge, units, autosave, timeline…).", obj(json!({}), &[])),
        tool("patch_settings", "JSON merge patch on get_settings' JSON; applied immediately.", obj(json!({"patch": {"type": "object"}}), &["patch"])),
        tool(
            "list_fonts",
            "Font families: installed ones, and Google Fonts that download when a text uses them.",
            obj(json!({"filter": {"type": "string"}, "max": {"type": "integer"}}), &[]),
        ),
        tool(
            "set_view",
            "Move the canvas view: `zoom` (1 = 100%), `center` [x, y] in document coordinates, or `fit` shape ids (empty: the whole artboard).",
            obj(json!({"zoom": {"type": "number"}, "center": {"type": "array", "items": {"type": "number"}}, "fit": {"type": "array", "items": {"type": "integer"}}}), &[]),
        ),
        tool("switch_tab", "Make an open document (index from get_state) the active one.", obj(json!({"index": {"type": "integer"}}), &["index"])),
        tool(
            "playback",
            "Play or pause the animation in the window, idempotently: {playing: true|false, time?, loop?}. Returns the state.",
            obj(json!({"playing": {"type": "boolean"}, "time": {"type": "number"}, "loop": {"type": "boolean"}}), &[]),
        ),
        tool(
            "set_timeline",
            "Create or change the animation timeline without keys: duration (s), fps, loop, range [in, out] (null: all). Returns it.",
            obj(json!({"duration": {"type": "number"}, "fps": {"type": "number"}, "loop": {"type": "boolean"}, "range": {"type": ["array", "null"], "items": {"type": "number"}}}), &[]),
        ),
        tool("seek", "Move the animation playhead (seconds); the document shows that moment (then render to see the frame).", obj(json!({"time": {"type": "number"}}), &["time"])),
        tool(
            "animate",
            "Keyframe one or more shapes in one call (creates the timeline if needed). keys: [{t, ease?, move: [dx, dy] | center: [x, y], rotate, scale, opacity, fill, stroke, stroke_width, blur, font_size, text, visible}]. Transform values are relative to where the shape rests. Returns the stored keys and each shape's bounds at every key time. Eases: linear, ease_in_out, ease_in, ease_out, hold, back_in, back_out, back_in_out, or [x1, y1, x2, y2]. See help animation.",
            obj(
                json!({
                    "id": {"type": "integer"},
                    "ids": {"type": "array", "items": {"type": "integer"}},
                    "keys": {"type": "array", "items": {"type": "object", "properties": {"t": {"type": "number"}, "ease": {}, "move": {"type": "array", "items": {"type": "number"}}, "center": {"type": "array", "items": {"type": "number"}}, "rotate": {"type": "number"}, "scale": {}, "opacity": {"type": "number"}, "fill": {"type": "string"}}, "required": ["t"]}},
                    "duration": {"type": "number", "description": "timeline length in seconds (default: up to the last key)"},
                    "fps": {"type": "number"},
                    "loop": {"type": "boolean"},
                    "replace": {"type": "boolean", "description": "replace existing keys of the touched properties (default true)"},
                    "ease": {"description": "ease for keys that don't name one (default ease_in_out; smooth = a curve through the keys)"},
                    "origin": {"description": "pivot for rotate/scale: [x, y] where the shape rests, or center, top, bottom, left, right, top-left, top-right, bottom-left, bottom-right. Kept on the shape."},
                    "oscillate": {"description": "{prop: rotate|move_x|move_y|scale|scale_x|scale_y|skew|opacity|blur, amp, freq (Hz), phase (deg), from?, to?, envelope?: [[t, gain]…]} or a list: a sine on top of the keys, baked into smooth keys"},
                    "verbose": {"type": "boolean", "description": "list every key and bounds at each (default: only up to 40 keys, else a summary)"},
                }),
                &["keys"],
            ),
        ),
        tool("get_animation", "The timeline and every animated shape's keys (or only `ids`).", obj(json!({"ids": {"type": "array", "items": {"type": "integer"}}}), &[])),
        tool(
            "clear_animation",
            "Remove keys from shapes: all tracks, or only `props` (Transform, Opacity, Fill…). The shapes go back to rest (other props: their value at 0 s) unless keep=\"current\". Returns their bounds.",
            obj(json!({"ids": {"type": "array", "items": {"type": "integer"}}, "props": {"type": "array", "items": {"type": "string"}}, "keep": {"type": "string", "enum": ["rest", "current"]}}), &["ids"]),
        ),
        tool("help", "Guides: animation, shapes, text, layout, components, ui, export.", obj(json!({"topic": {"type": "string"}}), &[])),
    ]
}

fn merge(a: &mut Value, b: &Value) {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
}

/// JSON merge patch (RFC 7386).
fn merge_patch(target: &mut Value, patch: &Value) {
    let Some(p) = patch.as_object() else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    // A different variant of an externally tagged enum ({"Solid": …} →
    // {"Gradient": …}) replaces it instead of merging both keys in.
    let variant = |o: &serde_json::Map<String, Value>| o.len() == 1 && o.keys().all(|k| k.starts_with(|c: char| c.is_ascii_uppercase()));
    if let Some(t) = target.as_object()
        && variant(t)
        && variant(p)
        && t.keys().ne(p.keys())
    {
        *target = json!({});
    }
    let t = target.as_object_mut().unwrap();
    for (k, v) in p {
        if v.is_null() {
            t.remove(k);
        } else {
            merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
        }
    }
}

fn text(v: Value) -> Outcome {
    Outcome::Done(vec![Content::Text(serde_json::to_string(&v).unwrap_or_default())])
}

pub(super) fn ids_arg(args: &Value) -> Vec<u64> {
    args.get("ids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_u64()).collect()).unwrap_or_default()
}

pub(super) fn str_arg<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(|v| v.as_str())
}

pub(super) fn num_arg(args: &Value, k: &str) -> Option<f64> {
    args.get(k).and_then(|v| v.as_f64())
}

pub(super) fn point_arg(args: &Value, k: &str) -> Option<kurbo::Point> {
    let a = args.get(k)?.as_array()?;
    Some(kurbo::Point::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?))
}

pub(super) fn color(v: &Value) -> Result<Option<Rgba>, String> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => Rgba::from_hex(s).map(Some).ok_or(format!("bad colour {s} (use #RRGGBB or #RRGGBBAA)")),
        _ => Err("colours are \"#RRGGBB\" strings or null".into()),
    }
}

pub(super) fn existing(app: &App, ids: &[u64]) -> Result<(), String> {
    match ids.iter().find(|id| app.doc.get(**id).is_none()) {
        Some(id) => Err(format!("no shape with id {id}")),
        None => Ok(()),
    }
}

/// `#RRGGBB`, or `#RRGGBBAA` when not opaque.
fn hex(c: Rgba) -> String {
    if c.a >= 1.0 { c.to_hex() } else { format!("{}{:02X}", c.to_hex(), (c.a.clamp(0.0, 1.0) * 255.0).round() as u8) }
}

pub(super) fn rect_json(r: kurbo::Rect) -> Value {
    let r1 = |v: f64| (v * 100.0).round() / 100.0;
    json!([r1(r.x0), r1(r.y0), r1(r.width()), r1(r.height())])
}

pub(super) fn bounds(app: &mut App, s: &Shape) -> Option<kurbo::Rect> {
    app.cache.update(s, &mut app.fonts).bounds
}

/// The selection's (or one shape's) outline bounds.
pub(super) fn ids_bounds(app: &mut App, ids: &[u64]) -> Option<kurbo::Rect> {
    let shapes: Vec<Shape> = ids.iter().filter_map(|id| app.doc.get(*id).cloned()).collect();
    shapes.iter().filter_map(|s| bounds(app, s)).reduce(|a, b| a.union(b))
}

fn tree(app: &mut App, shapes: &[Shape], depth: usize, left: &mut usize) -> Vec<Value> {
    let mut out = Vec::new();
    // Top of the stack first, as in the layers panel.
    for s in shapes.iter().rev() {
        if *left == 0 {
            out.push(json!({"more": "limit reached"}));
            break;
        }
        *left -= 1;
        let mut v = json!({"id": s.id, "type": s.kind.type_name(), "name": s.name});
        if let Some(b) = bounds(app, s) {
            v["bounds"] = rect_json(b);
        }
        if !s.visible {
            v["visible"] = json!(false);
        }
        if s.locked {
            v["locked"] = json!(true);
        }
        if let ShapeKind::Text(t) = &s.kind {
            v["text"] = json!(t.content.chars().take(80).collect::<String>());
        }
        if let Some(c) = super::edit::component_json(s) {
            v["component"] = c;
        }
        if let Some(l) = &s.layout {
            v["layout"] = super::edit::layout_json(l);
        }
        if let Some(a) = s.anim.as_deref() {
            v["animated"] = json!(true);
            v["keys"] = json!(a.tracks.iter().map(|t| t.keys.len()).sum::<usize>());
            if let Some(r) = super::anim::rest_bounds(app, s.id) {
                v["rest_bounds"] = rect_json(r);
            }
        }
        if let Some(ch) = s.kind.children() {
            if depth > 0 {
                v["children"] = json!(tree(app, ch, depth - 1, left));
            } else {
                v["children_count"] = json!(ch.len());
            }
        }
        out.push(v);
    }
    out
}

pub fn state(app: &mut App, depth: usize, limit: usize, mode: &str) -> Value {
    let active = active(app);
    let artboards: Vec<Value> = app
        .doc
        .artboards
        .iter()
        .map(|a| json!({"id": a.id, "name": a.name, "bounds": rect_json(a.rect()), "background": hex(a.background), "active": a.id == active}))
        .collect();
    let tabs: Vec<Value> = app.tab_infos().into_iter().enumerate().map(|(i, t)| json!({"index": i, "name": t.name, "path": t.path, "dirty": t.dirty, "active": t.active})).collect();
    let shapes: Vec<Shape> = app.doc.shapes().iter().map(|s| Shape::clone(s)).collect();
    let mut left = limit;
    let layers = tree(app, &shapes, depth, &mut left);
    let sel = app.selection.clone();
    let sel_bounds = ids_bounds(app, &sel).map(rect_json);
    let v = app.view;
    let visible = kurbo::Rect::from_points(v.to_doc(app.canvas_rect.min), v.to_doc(app.canvas_rect.max));
    json!({
        "mode": mode,
        "documents": tabs,
        "file": app.file_path,
        "dirty": app.dirty,
        "unit": app.doc.unit.label(),
        "artboards": artboards,
        "selection": sel,
        "selection_bounds": sel_bounds,
        "tool": app.tool.name(),
        "view": {"zoom": (v.zoom as f64 * 1000.0).round() / 1000.0, "visible": rect_json(visible), "canvas_on_screen": [app.canvas_rect.min.x, app.canvas_rect.min.y, app.canvas_rect.width(), app.canvas_rect.height()]},
        "animation": app.doc.animation.as_ref().map(|t| json!({"duration": t.duration, "fps": t.fps, "time": t.time})),
        "undo": app.history.can_undo(),
        "redo": app.history.can_redo(),
        "layers": layers,
    })
}

/// Path data as counts (a patch merging into it leaves the nodes alone).
fn summarise_paths(v: &mut Value) {
    match v {
        Value::Object(o) => {
            if let Some(Value::Array(sp)) = o.get("subpaths") {
                let nodes: usize = sp.iter().map(|p| p["nodes"].as_array().map_or(0, |n| n.len())).sum();
                *v = json!({"omitted": format!("{} subpaths, {nodes} nodes (get_shapes geometry: true)", sp.len())});
                return;
            }
            o.values_mut().for_each(summarise_paths);
        }
        Value::Array(a) => a.iter_mut().for_each(summarise_paths),
        _ => {}
    }
}

/// Commands that would open a native file dialog or quit (no use to an agent).
fn blocked(c: Cmd, app: &App) -> Option<&'static str> {
    match c {
        Cmd::Open => Some("use open_file"),
        Cmd::SaveAs => Some("use save with a path"),
        Cmd::Save if app.file_path.is_none() => Some("never saved: use save with a path"),
        Cmd::Place => Some("use place_file"),
        Cmd::Print | Cmd::Quit => Some("not available to agents"),
        _ => None,
    }
}

pub(super) fn set_text(s: &mut Shape, args: &Value) -> Result<(), String> {
    let ShapeKind::Text(t) = &mut s.kind else { return Ok(()) };
    if let Some(c) = str_arg(args, "text") {
        let n = t.content.len();
        t.edit(0..n, c);
    }
    if let Some(v) = num_arg(args, "font_size") {
        t.size = v.max(0.1);
    }
    if let Some(f) = str_arg(args, "font_family") {
        t.family = f.into();
    }
    if let Some(w) = args.get("weight") {
        t.weight = match w {
            Value::Number(n) => n.as_u64().map(|n| n.clamp(1, 1000) as u16),
            Value::String(n) => match n.as_str() {
                "thin" => Some(100),
                "light" => Some(300),
                "regular" | "normal" => Some(400),
                "medium" => Some(500),
                "semibold" => Some(600),
                "bold" => Some(700),
                "black" => Some(900),
                _ => None,
            },
            _ => None,
        }
        .ok_or("weight is 100..900 (400 regular, 700 bold) or thin, light, regular, medium, semibold, bold, black")?;
    }
    if let Some(i) = args.get("italic").and_then(|v| v.as_bool()) {
        t.italic = i;
    }
    if let Some(a) = str_arg(args, "align") {
        use crate::document::TextAlign;
        t.align = match a {
            "left" => TextAlign::Left,
            "center" => TextAlign::Center,
            "right" => TextAlign::Right,
            "justify" => TextAlign::Justify,
            _ => return Err("align is left, center, right or justify".into()),
        };
    }
    if let Some(v) = num_arg(args, "line_height") {
        t.line_height = v.max(0.1);
    }
    if let Some(v) = num_arg(args, "letter_spacing") {
        t.letter_spacing = v;
    }
    Ok(())
}

pub(super) fn set_style(s: &mut Shape, args: &Value) -> Result<(), String> {
    if let Some(n) = str_arg(args, "name") {
        s.name = n.into();
    }
    if let Some(v) = args.get("fill") {
        s.fill = super::edit::paint(v)?;
        s.fill_key = None;
    }
    if let Some(v) = args.get("stroke") {
        if v.as_str().is_some_and(|c| c.contains("gradient(")) || v.is_object() {
            return Err("strokes are solid colours (an SVG gradient stroke in add_svg becomes a filled outline)".into());
        }
        match color(v)? {
            Some(c) => s.stroke.get_or_insert_with(Stroke::default).color = c,
            None => s.stroke = None,
        }
        s.stroke_key = None;
    }
    if let Some(w) = num_arg(args, "stroke_width") {
        s.stroke.get_or_insert_with(Stroke::default).width = w;
    }
    if let Some(c) = str_arg(args, "cap") {
        use crate::document::LineCap;
        let cap = match c {
            "butt" => LineCap::Butt,
            "round" => LineCap::Round,
            "square" => LineCap::Square,
            _ => return Err("cap is butt, round or square".into()),
        };
        s.stroke.get_or_insert_with(Stroke::default).cap = cap;
    }
    if let Some(j) = str_arg(args, "join") {
        use crate::document::LineJoin;
        let join = match j {
            "miter" => LineJoin::Miter,
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            _ => return Err("join is miter, round or bevel".into()),
        };
        s.stroke.get_or_insert_with(Stroke::default).join = join;
    }
    if let Some(a) = str_arg(args, "stroke_align") {
        use crate::document::StrokeAlign;
        let align = match a {
            "center" => StrokeAlign::Center,
            "inside" => StrokeAlign::Inside,
            "outside" => StrokeAlign::Outside,
            _ => return Err("stroke_align is center, inside or outside".into()),
        };
        s.stroke.get_or_insert_with(Stroke::default).align = align;
    }
    if let Some(c) = args.get("constraints") {
        if c.is_null() {
            s.constraints = None;
        } else {
            let o = c.as_object().ok_or("constraints is {h, v}: start | end | both | center | scale")?;
            let mut k = s.constraints.unwrap_or_default();
            if let Some(h) = o.get("h").and_then(|v| v.as_str()) {
                k.h = super::edit::pin(h)?;
            }
            if let Some(v) = o.get("v").and_then(|v| v.as_str()) {
                k.v = super::edit::pin(v)?;
            }
            s.constraints = Some(k);
        }
    }
    if let Some(o) = num_arg(args, "opacity") {
        s.opacity = o.clamp(0.0, 1.0) as f32;
    }
    Ok(())
}

/// Height of a text frame `w` wide that shows all of its text.
fn fit_height(app: &mut App, td: &TextData, w: f64) -> f64 {
    let mut probe = td.clone();
    probe.frame = Some([w, 1e7]);
    let l = app.fonts.layout(&probe);
    l.lines.iter().filter(|l| !l.hidden).map(|l| l.y1).fold(0.0, f64::max).ceil().max(1.0)
}

/// Download Google fonts the new texts use (they relayout when they land).
fn fetch_fonts(app: &mut App, ids: &[u64]) {
    let mut families = Vec::new();
    for id in ids {
        let Some(s) = app.doc.get(*id).cloned() else { continue };
        let mut stack = vec![s];
        while let Some(s) = stack.pop() {
            if let ShapeKind::Text(t) = &s.kind {
                families.push((t.family.clone(), t.weight));
            }
            stack.extend(s.kind.children().cloned().unwrap_or_default());
        }
    }
    for (f, w) in families {
        if !app.fonts.is_installed(&f) && app.fonts.google.iter().any(|g| g.family == f) {
            app.fonts.request_google_variant(&f, &w.to_string());
        }
    }
}

/// `covers` warning for new shapes that sit over existing ones.
fn warn_covers(app: &mut App, ids: &[u64], anchor: Option<(u64, bool)>, out: &mut Value) {
    if anchor.is_some() {
        return;
    }
    let c = super::edit::covered(app, ids);
    if !c.is_empty() {
        out["covers"] = json!(c);
        out["warning"] = json!("drawn over these existing shapes (it is above them); pass below: <id> to put it underneath");
    }
}

fn add_shape(app: &mut App, args: &Value) -> Result<Outcome, String> {
    let ty = str_arg(args, "type").ok_or("type is required")?;
    let (x, y) = (num_arg(args, "x").unwrap_or(0.0), num_arg(args, "y").unwrap_or(0.0));
    let (w, h) = (num_arg(args, "w").unwrap_or(100.0), num_arg(args, "h").unwrap_or(100.0));
    let points = args.get("points").and_then(|v| v.as_u64()).map(|n| n.max(3) as u32);
    let anchor = super::edit::stack_anchor(app, args)?;
    let kind = match ty {
        "rect" => ShapeKind::Rect { w, h, radii: [num_arg(args, "radius").unwrap_or(0.0); 4] },
        "ellipse" => ShapeKind::Ellipse { w, h },
        "star" => ShapeKind::Star { w, h, points: points.unwrap_or(5), inner: num_arg(args, "inner").unwrap_or(0.5), corner: 0.0 },
        "polygon" => ShapeKind::Polygon { w, h, sides: points.unwrap_or(6), corner: 0.0 },
        "path" => {
            let d = str_arg(args, "d").ok_or("path needs `d`")?;
            let bp = kurbo::BezPath::from_svg(d).map_err(|e| format!("bad path data: {e}"))?;
            ShapeKind::Path(crate::geom::bez_to_pathdata(&bp))
        }
        "text" => ShapeKind::Text(TextData { content: str_arg(args, "text").unwrap_or("Text").into(), ..Default::default() }),
        _ => return Err(format!("unknown type {ty}")),
    };
    let is_path = matches!(kind, ShapeKind::Path(_));
    let mut s = Shape::new(kind);
    if !is_path {
        s.transform = kurbo::Affine::translate((x, y));
    }
    if ty == "text" {
        s.fill = Some(Paint::Solid(Rgba::BLACK));
        set_text(&mut s, args)?;
        // A width makes a frame that wraps; without a height it fits the text.
        if args.get("w").is_some() {
            let fh = match num_arg(args, "h") {
                Some(h) => h,
                None => match &s.kind {
                    ShapeKind::Text(t) => fit_height(app, &t.clone(), w),
                    _ => h,
                },
            };
            if let ShapeKind::Text(t) = &mut s.kind {
                t.frame = Some([w, fh]);
            }
        }
    }
    set_style(&mut s, args)?;
    let parent = super::edit::parent_arg(app, args)?;
    app.checkpoint("");
    let id = app.doc.add_shape(s);
    super::edit::insert_into(app, &[id], parent, index_arg(args));
    super::edit::restack(app, &[id], anchor);
    fetch_fonts(app, &[id]);
    super::edit::settle(app);
    if args.get("select").and_then(|v| v.as_bool()).unwrap_or(false) {
        app.selection = vec![id];
    }
    let mut out = json!({"id": id, "bounds": ids_bounds(app, &[id]).map(rect_json)});
    if let Some(b) = app.doc.get(id).cloned().and_then(|s| super::edit::text_box(app, &s)).map(rect_json).filter(|b| *b != out["bounds"]) {
        out["box"] = b;
    }
    warn_covers(app, &[id], anchor, &mut out);
    Ok(text(out))
}

fn index_arg(args: &Value) -> Option<usize> {
    args.get("index").and_then(|v| v.as_u64()).map(|i| i as usize)
}

fn add_svg(app: &mut App, args: &Value) -> Result<Outcome, String> {
    let svg = str_arg(args, "svg").ok_or("svg is required")?;
    let anchor = super::edit::stack_anchor(app, args)?;
    let parent = super::edit::parent_arg(app, args)?;
    app.checkpoint("");
    let mut ids = match crate::io::svg::import_into(svg.as_bytes(), &mut app.fonts, &mut app.doc) {
        Ok(ids) if !ids.is_empty() => ids,
        Ok(_) => {
            app.history.cancel(&mut app.doc);
            return Err("the SVG has no visible content".into());
        }
        Err(e) => {
            app.history.cancel(&mut app.doc);
            return Err(e);
        }
    };
    if let Some(name) = str_arg(args, "group") {
        // Not Cmd::Group: that would be a second undo step.
        let keep = std::mem::replace(&mut app.selection, ids.clone());
        let shapes = app.selected_bottom_up();
        app.selection = keep;
        let mut g = Shape::new(ShapeKind::Group { children: shapes.clone() });
        g.fill = None;
        g.name = name.into();
        ids = vec![app.combine_into(&shapes, g)];
    }
    if let Some(at) = point_arg(args, "at")
        && let Some(b) = ids_bounds(app, &ids)
    {
        app.translate_ids(&ids, at - b.origin());
    }
    super::edit::insert_into(app, &ids, parent, index_arg(args));
    super::edit::restack(app, &ids, anchor);
    fetch_fonts(app, &ids);
    super::edit::settle(app);
    if args.get("select").and_then(|v| v.as_bool()).unwrap_or(false) {
        app.selection = ids.clone();
    }
    let b = ids_bounds(app, &ids).map(rect_json);
    let added: Vec<Shape> = ids.iter().filter_map(|id| app.doc.get(*id).cloned()).collect();
    let mut shapes = Vec::new();
    for s in &added {
        let mut v = super::edit::brief(app, s);
        if s.kind.children().is_some() {
            v["layers"] = json!(s.kind.children().map(|c| c.len()).unwrap_or(0));
        }
        shapes.push(v);
    }
    let mut out = json!({"ids": ids, "bounds": b, "shapes": shapes});
    warn_covers(app, &ids, anchor, &mut out);
    Ok(text(out))
}

/// `set` keys update_shapes knows (from its schema).
fn set_keys() -> &'static [String] {
    static KEYS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        list().into_iter().find(|t| t["name"] == "update_shapes").and_then(|t| t["inputSchema"]["properties"]["set"]["properties"].as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default()
    })
}

/// One `set` / `patch` on shapes, written to the document.
fn apply_update(app: &mut App, ids: &[u64], set: &Value, patch: Option<&Value>) -> Result<(), String> {
    existing(app, ids)?;
    if let Some(bad) = set.as_object().and_then(|o| o.keys().find(|k| !set_keys().contains(k))) {
        return Err(format!("set has no field `{bad}` (fields: {})", set_keys().join(", ")));
    }
    let mut changed = Vec::new();
    for &id in ids {
        let mut s = app.doc.get(id).cloned().ok_or(format!("no shape with id {id}"))?;
        if let Some(p) = patch {
            let mut v = serde_json::to_value(&s).map_err(|e| e.to_string())?;
            merge_patch(&mut v, p);
            let rev = s.rev;
            s = serde_json::from_value(v).map_err(|e| format!("patch on {id}: {e}"))?;
            s.id = id;
            s.rev = rev;
        }
        set_style(&mut s, set)?;
        set_text(&mut s, set)?;
        if let Some(v) = set.get("visible").and_then(|v| v.as_bool()) {
            s.visible = v;
        }
        if let Some(v) = set.get("locked").and_then(|v| v.as_bool()) {
            s.locked = v;
        }
        let geo = ["x", "y", "w", "h", "rotate"].iter().any(|k| set.get(*k).is_some());
        if geo {
            let b = bounds(app, &s).ok_or(format!("{id} has no bounds"))?;
            if let Some(deg) = num_arg(set, "rotate") {
                s.apply_transform(kurbo::Affine::rotate_about(deg.to_radians(), b.center()));
            }
            let b = if num_arg(set, "rotate").is_some() { bounds(app, &s).unwrap_or(b) } else { b };
            let nx = num_arg(set, "x").unwrap_or(b.x0);
            let ny = num_arg(set, "y").unwrap_or(b.y0);
            let sx = num_arg(set, "w").map(|w| w / b.width().max(1e-9)).unwrap_or(1.0);
            let sy = num_arg(set, "h").map(|h| h / b.height().max(1e-9)).unwrap_or(1.0);
            let a = kurbo::Affine::translate((nx, ny)) * kurbo::Affine::scale_non_uniform(sx, sy) * kurbo::Affine::translate(-b.origin().to_vec2());
            if a != kurbo::Affine::IDENTITY {
                s.apply_transform(a);
            }
        }
        if set.get("fit_height").and_then(|v| v.as_bool()) == Some(true)
            && let ShapeKind::Text(t) = &s.kind
            && let Some([w, _]) = t.frame
        {
            let h = fit_height(app, &t.clone(), w);
            if let ShapeKind::Text(t) = &mut s.kind {
                t.frame = Some([w, h]);
            }
        }
        changed.push(s);
    }
    if patch.is_some_and(|p| p.get("anim").is_some()) {
        // Keys only change through the animation system (edits are compared
        // with the last sync and would otherwise lose them).
        app.anim_edit_quiet(|doc| {
            for s in changed {
                doc.replace(s);
            }
        });
    } else {
        for s in changed {
            app.doc.replace(s);
        }
    }
    Ok(())
}

fn update_shapes(app: &mut App, args: &Value) -> Result<Outcome, String> {
    let mut jobs: Vec<(Vec<u64>, Value, Option<Value>)> = Vec::new();
    if args.get("ids").is_some() || args.get("set").is_some() || args.get("patch").is_some() {
        jobs.push((ids_arg(args), args.get("set").cloned().unwrap_or(json!({})), args.get("patch").cloned()));
    }
    if let Some(list) = args.get("updates") {
        let list = list.as_array().ok_or("updates is a list of {id | ids, set?, patch?}")?;
        for (i, u) in list.iter().enumerate() {
            if let Some(bad) = u.as_object().ok_or(format!("updates[{i}] is an object"))?.keys().find(|k| !["id", "ids", "set", "patch"].contains(&k.as_str())) {
                return Err(format!("updates[{i}] has no field `{bad}` (id | ids, set, patch)"));
            }
            let ids = match u.get("id").and_then(|v| v.as_u64()) {
                Some(id) => vec![id],
                None => ids_arg(u),
            };
            jobs.push((ids, u.get("set").cloned().unwrap_or(json!({})), u.get("patch").cloned()));
        }
    }
    if jobs.is_empty() || jobs.iter().any(|j| j.0.is_empty()) {
        return Err("give ids with set / patch, or updates: [{id, set?, patch?}, …]".into());
    }
    // One undo step; a bad update rolls back the whole call.
    app.checkpoint("");
    for (i, (ids, set, patch)) in jobs.iter().enumerate() {
        if let Err(e) = apply_update(app, ids, set, patch.as_ref()) {
            app.history.cancel(&mut app.doc);
            app.cache.invalidate_all();
            return Err(if jobs.len() > 1 { format!("updates[{i}]: {e} (nothing was changed)") } else { e });
        }
    }
    super::edit::settle(app);
    let mut seen = Vec::new();
    for (ids, _, _) in &jobs {
        for id in ids {
            if !seen.contains(id) {
                seen.push(*id);
            }
        }
    }
    let mut out = Vec::new();
    for id in seen {
        let mut v = json!({"id": id, "bounds": ids_bounds(app, &[id]).map(rect_json)});
        if let Some(b) = app.doc.get(id).cloned().and_then(|s| super::edit::text_box(app, &s)).map(rect_json).filter(|b| *b != v["bounds"]) {
            v["box"] = b;
        }
        out.push(v);
    }
    Ok(text(json!(out)))
}

fn render(app: &mut App, args: &Value) -> Result<Outcome, String> {
    let ids = ids_arg(args);
    existing(app, &ids)?;
    let artboard = args.get("artboard").and_then(|v| v.as_u64()).unwrap_or_else(|| active(app));
    app.doc.artboard(artboard).ok_or(format!("no artboard {artboard}"))?;
    if let Some(sizes) = args.get("sizes").and_then(|v| v.as_array()) {
        let sizes: Vec<u64> = sizes.iter().filter_map(|v| v.as_u64()).map(|n| n.clamp(8, 1024)).collect();
        if sizes.is_empty() || sizes.len() > 12 {
            return Err("sizes is 1 to 12 pixel sizes, e.g. [16, 32, 180]".into());
        }
        let doc = app.doc.clone();
        let mut imgs = Vec::new();
        for n in &sizes {
            let (png, _, _) = render_doc(&doc, app, &ids, artboard, *n as f64, true)?;
            imgs.push(image::load_from_memory(&png).map_err(|e| e.to_string())?.to_rgba8());
        }
        const GAP: u32 = 12;
        let w = imgs.iter().map(|i| i.width()).sum::<u32>() + GAP * (imgs.len() as u32 + 1);
        let h = imgs.iter().map(|i| i.height()).max().unwrap_or(1) + 2 * GAP;
        // A checkerboard shows transparency.
        let mut strip = image::RgbaImage::from_fn(w, h, |x, y| if (x / 8 + y / 8) % 2 == 0 { image::Rgba([235, 235, 235, 255]) } else { image::Rgba([205, 205, 205, 255]) });
        let mut x = GAP;
        for i in &imgs {
            image::imageops::overlay(&mut strip, i, x as i64, (h - GAP - i.height()) as i64);
            x += i.width() + GAP;
        }
        let mut png = Vec::new();
        strip.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        let s: Vec<String> = sizes.iter().map(|n| n.to_string()).collect();
        return Ok(Outcome::Done(vec![Content::Png(png), Content::Text(format!("actual pixels, left to right: {} px (longest side), bottoms aligned, on a checkerboard", s.join(", ")))]));
    }
    if let Some(ghosts) = args.get("onion").and_then(|v| v.as_array()) {
        let ghosts: Vec<f64> = ghosts.iter().filter_map(|v| v.as_f64()).collect();
        if ghosts.is_empty() || ghosts.len() > 48 {
            return Err("onion is 1 to 48 times".into());
        }
        let max = args.get("max_size").and_then(|v| v.as_u64()).unwrap_or(1024).clamp(16, 4096) as f64;
        app.anim_sync();
        let t = num_arg(args, "time").unwrap_or(app.anim_time());
        let main = crate::animation::frame(&app.doc, t);
        let (png, region, scale) = render_doc(&main, app, &ids, artboard, max, false)?;
        let mut out = image::load_from_memory(&png).map_err(|e| e.to_string())?.to_rgba8();
        // Ghosts at a third, under the frame itself drawn again on top.
        for g in &ghosts {
            let d = crate::animation::frame(&app.doc, *g);
            let (png, _, _) = render_doc(&d, app, &ids, artboard, max, true)?;
            let mut ghost = image::load_from_memory(&png).map_err(|e| e.to_string())?.to_rgba8();
            ghost.pixels_mut().for_each(|p| p.0[3] = (p.0[3] as f32 * 0.33) as u8);
            image::imageops::overlay(&mut out, &ghost, 0, 0);
        }
        let (png, _, _) = render_doc(&main, app, &ids, artboard, max, true)?;
        image::imageops::overlay(&mut out, &image::load_from_memory(&png).map_err(|e| e.to_string())?.to_rgba8(), 0, 0);
        let mut png = Vec::new();
        out.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        return Ok(Outcome::Done(vec![Content::Png(png), Content::Text(format!("t = {t} s, with faint ghosts at {:?} s; region {} at {scale:.3}×", ghosts, rect_json(region)))]));
    }
    let times: Option<Vec<f64>> = args.get("times").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|t| t.as_f64()).collect());
    if let Some(times) = times.filter(|t| !t.is_empty()) {
        if times.len() > 36 {
            return Err("at most 36 times".into());
        }
        let max = args.get("max_size").and_then(|v| v.as_u64()).unwrap_or(384).clamp(16, 2048) as f64;
        app.anim_sync();
        let mut frames = Vec::new();
        for t in &times {
            let doc = crate::animation::frame(&app.doc, *t);
            let (png, _, _) = render_doc(&doc, app, &ids, artboard, max, false)?;
            frames.push(image::load_from_memory(&png).map_err(|e| e.to_string())?.to_rgba8());
        }
        let cols = (times.len() as f64).sqrt().ceil() as u32;
        let rows = (times.len() as u32).div_ceil(cols);
        let (cw, ch) = (frames.iter().map(|f| f.width()).max().unwrap_or(1), frames.iter().map(|f| f.height()).max().unwrap_or(1));
        const GAP: u32 = 6;
        let mut sheet = image::RgbaImage::from_pixel(cols * cw + (cols - 1) * GAP, rows * ch + (rows - 1) * GAP, image::Rgba([128, 128, 128, 255]));
        for (i, f) in frames.iter().enumerate() {
            let (c, r) = (i as u32 % cols, i as u32 / cols);
            image::imageops::overlay(&mut sheet, f, (c * (cw + GAP)) as i64, (r * (ch + GAP)) as i64);
        }
        let mut png = Vec::new();
        sheet.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        let t: Vec<String> = times.iter().map(|t| format!("{t}")).collect();
        return Ok(Outcome::Done(vec![Content::Png(png), Content::Text(format!("{cols}×{rows} sheet, left to right then down, at t = {} s", t.join(", ")))]));
    }
    let max = args.get("max_size").and_then(|v| v.as_u64()).unwrap_or(1024).clamp(16, 4096) as f64;
    let (png, region, scale) = match num_arg(args, "time") {
        Some(t) => {
            app.anim_sync();
            let doc = crate::animation::frame(&app.doc, t);
            render_doc(&doc, app, &ids, artboard, max, false)?
        }
        None => {
            let doc = app.doc.clone();
            render_doc(&doc, app, &ids, artboard, max, false)?
        }
    };
    Ok(Outcome::Done(vec![Content::Png(png), Content::Text(format!("region {} at {scale:.3}×", rect_json(region)))]))
}

/// PNG of an artboard (or of `ids`) of `doc`, longest side `max` px.
fn render_doc(doc: &crate::document::Document, app: &mut App, ids: &[u64], artboard: u64, max: f64, transparent: bool) -> Result<(Vec<u8>, kurbo::Rect, f64), String> {
    use crate::io::export::{Area, Format, Options};
    let region = if ids.is_empty() {
        doc.artboard(artboard).ok_or(format!("no artboard {artboard}"))?.rect()
    } else {
        let refs: Vec<&Shape> = ids.iter().filter_map(|id| doc.get(*id)).collect();
        crate::io::export::visual_bounds(&refs, &mut app.fonts).ok_or("nothing to render")?
    };
    let scale = (max / region.width().max(region.height()).max(1.0)).min(4.0);
    let opts = Options { format: Format::Png, area: if ids.is_empty() { Area::Artboard } else { Area::Selection }, scale, background: !transparent, ..Default::default() };
    let path = temp_path("png");
    let files = crate::io::export::export(doc, ids, artboard, &opts, &path, &mut app.fonts)?;
    let f = files.first().cloned().unwrap_or(path);
    let png = std::fs::read(&f).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&f);
    Ok((png?, region, scale))
}

/// The active artboard (the first one before the UI has picked one).
fn active(app: &App) -> u64 {
    if app.doc.artboard(app.active_artboard).is_some() { app.active_artboard } else { app.doc.primary().id }
}

fn temp_path(ext: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("mtzd-mcp-{}-{n}.{ext}", std::process::id()))
}

fn export(app: &mut App, args: &Value) -> Result<Outcome, String> {
    use crate::io::export::{Area, Format, Options};
    let path = std::path::PathBuf::from(str_arg(args, "path").ok_or("path is required")?);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let format = Format::from_ext(ext).or((ext == "lottie").then_some(Format::Lottie)).ok_or(format!("unknown format .{ext}"))?;
    let area = match str_arg(args, "area").unwrap_or("artboard") {
        "all" => Area::AllArtboards,
        "selection" => Area::Selection,
        _ => Area::Artboard,
    };
    if area == Area::Selection && app.selection.is_empty() {
        return Err("nothing selected".into());
    }
    let opts = Options { format, area, scale: num_arg(args, "scale").unwrap_or(1.0), background: !args.get("transparent").and_then(|v| v.as_bool()).unwrap_or(false), ..Default::default() };
    if crate::io::export::is_animated(&app.doc, &opts) {
        // Frame by frame in the background (progress in the status bar):
        // the window stays responsive, the call answers when it's written.
        if app.export_job.is_some() {
            return Err("another export is still running".into());
        }
        app.toast = None;
        app.export_to(&path, &opts);
        return Ok(Outcome::Later(Vec::new(), After::Export(path)));
    }
    let files = crate::io::export::export(&app.doc, &app.selection, active(app), &opts, &path, &mut app.fonts)?;
    Ok(text(json!({"written": files})))
}

/// What an edit changed, or the whole state with full_state: true.
fn answer(app: &mut App, args: &Value, before: &super::edit::Before, mode: &str) -> Outcome {
    if args.get("full_state").and_then(|v| v.as_bool()) == Some(true) {
        return Outcome::Done(state_text(app, mode));
    }
    text(super::edit::changes(app, before))
}

fn artboard_json(app: &App, a: &crate::document::Artboard) -> Value {
    json!({"id": a.id, "name": a.name, "bounds": rect_json(a.rect()), "background": hex(a.background), "clip": a.clip, "active": a.id == active(app)})
}

fn state_text(app: &mut App, mode: &str) -> Vec<Content> {
    let mut s = state(app, 1, 60, mode);
    s.as_object_mut().map(|o| o.remove("documents"));
    vec![Content::Text(serde_json::to_string(&s).unwrap_or_default())]
}

/// Run a tool. `mode` is "live" or "headless" (reported by get_state).
/// Parameter names per tool, from the schemas.
fn params(tool: &str) -> Option<&'static [String]> {
    static MAP: std::sync::OnceLock<std::collections::HashMap<String, Vec<String>>> = std::sync::OnceLock::new();
    let map = MAP.get_or_init(|| {
        list().into_iter().filter_map(|t| Some((t["name"].as_str()?.to_string(), t["inputSchema"]["properties"].as_object()?.keys().cloned().collect()))).collect()
    });
    map.get(tool).map(|v| v.as_slice())
}

pub fn call(app: &mut App, ctx: &egui::Context, ptr: &mut Pointer, name: &str, args: &Value, mode: &str) -> Result<Outcome, String> {
    // A misnamed parameter would otherwise be ignored without a word.
    if let (Some(known), Some(given)) = (params(name), args.as_object())
        && let Some(bad) = given.keys().find(|k| !known.contains(k))
    {
        return Err(format!("{name} has no parameter `{bad}` (parameters: {})", known.join(", ")));
    }
    let ids = ids_arg(args);
    // A Google font that isn't installed yet starts downloading (text updates when it lands).
    for f in [str_arg(args, "font_family"), args.get("set").and_then(|s| str_arg(s, "font_family"))].into_iter().flatten() {
        if !app.fonts.is_installed(f) && app.fonts.google.iter().any(|g| g.family == f) {
            app.fonts.request_google_variant(f, "400");
        }
    }
    match name {
        "get_state" => {
            let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(400) as usize;
            Ok(text(state(app, depth, limit, mode)))
        }
        "get_shapes" => {
            existing(app, &ids)?;
            let geometry = args.get("geometry").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut v: Vec<Value> = ids.iter().filter_map(|id| app.doc.get(*id)).map(|s| serde_json::to_value(s).unwrap_or_default()).collect();
            if !geometry {
                v.iter_mut().for_each(summarise_paths);
            }
            Ok(text(json!(v)))
        }
        "add_svg" => add_svg(app, args),
        "add_icon" => {
            let name = str_arg(args, "name").ok_or("name is required")?;
            let size = num_arg(args, "size").unwrap_or(64.0).clamp(4.0, 4096.0);
            let svg = super::edit::icon_svg(name, size, str_arg(args, "color"))?;
            let mut a = json!({"svg": svg, "group": name});
            for k in ["at", "below", "above", "select", "parent", "index"] {
                if let Some(v) = args.get(k) {
                    a[k] = v.clone();
                }
            }
            add_svg(app, &a)
        }
        "arrange" => Ok(text(super::edit::arrange(app, args)?)),
        "auto_layout" => Ok(text(super::edit::auto_layout(app, args)?)),
        "create_component" => Ok(text(super::edit::create_component(app, args)?)),
        "instantiate" => Ok(text(super::edit::instantiate(app, args)?)),
        "list_components" => Ok(text(super::edit::list_components(app))),
        "set_timeline" => {
            let range = match args.get("range") {
                None => None,
                Some(Value::Null) => Some(None),
                Some(r) => {
                    let a = r.as_array().and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))).ok_or("range is [in, out] seconds, or null")?;
                    if a.1 <= a.0 {
                        return Err("range out must be after in".into());
                    }
                    Some(Some(a))
                }
            };
            if num_arg(args, "duration").is_some_and(|d| d <= 0.0) {
                return Err("duration must be > 0".into());
            }
            app.checkpoint("");
            let tl = app.doc.animation.get_or_insert_with(crate::animation::Timeline::default);
            if let Some(d) = num_arg(args, "duration") {
                tl.duration = d;
            }
            if let Some(f) = num_arg(args, "fps") {
                tl.fps = f.clamp(1.0, 240.0);
            }
            if let Some(l) = args.get("loop").and_then(|v| v.as_bool()) {
                tl.looping = l;
            }
            if let Some(r) = range {
                tl.range = r;
            }
            let (t, d) = (tl.time, tl.duration);
            if t > d {
                app.anim_seek(d);
            }
            Ok(text(super::anim::timeline_json(app)))
        }
        "add_shape" => add_shape(app, args),
        "update_shapes" => update_shapes(app, args),
        "delete_shapes" => {
            existing(app, &ids)?;
            app.checkpoint("");
            for id in &ids {
                app.doc.remove(*id);
            }
            app.selection.retain(|id| app.doc.get(*id).is_some());
            super::edit::settle(app);
            Ok(text(json!({"deleted": ids, "selection": app.selection})))
        }
        "select" => {
            existing(app, &ids)?;
            app.finish_text_edit();
            if !args.get("add").and_then(|v| v.as_bool()).unwrap_or(false) {
                app.selection.clear();
            }
            for id in ids {
                if !app.selection.contains(&id) {
                    app.selection.push(id);
                }
            }
            Ok(text(json!({"selection": app.selection})))
        }
        "list_commands" => {
            let f = str_arg(args, "filter").unwrap_or("").to_lowercase();
            let v: Vec<Value> = Cmd::all()
                .into_iter()
                .filter(|c| !matches!(c, Cmd::Tool(_)) && blocked(*c, app).is_none())
                .filter(|c| f.is_empty() || [format!("{c:?}"), c.label().into(), c.meta().category.label().into()].iter().any(|s| s.to_lowercase().contains(&f)))
                .map(|c| json!({"name": format!("{c:?}"), "label": c.label(), "category": c.meta().category.label(), "shortcut": c.shortcut_text(), "enabled": c.enabled(app)}))
                .collect();
            Ok(text(json!(v)))
        }
        "run_command" => {
            let c = crate::headless::find_cmd(str_arg(args, "name").ok_or("name is required")?)?;
            if let Some(why) = blocked(c, app) {
                return Err(format!("{c:?}: {why}"));
            }
            if args.get("ids").is_some() {
                existing(app, &ids)?;
                app.finish_text_edit();
                app.selection = ids;
            }
            if !c.enabled(app) {
                return Err(format!("{c:?} doesn't apply now (check the selection or tool)"));
            }
            let before = super::edit::before(app);
            c.run(app, ctx);
            super::edit::settle(app);
            Ok(answer(app, args, &before, mode))
        }
        "set_tool" => {
            let t = crate::headless::find_tool(str_arg(args, "name").ok_or("name is required")?)?;
            app.set_tool(t);
            Ok(text(json!({"tool": t.name()})))
        }
        "input" => {
            let script = str_arg(args, "script").ok_or("script is required")?;
            let view = app.view;
            let mut batches = Vec::new();
            for (n, line) in script.lines().map(str::trim).enumerate().filter(|(_, l)| !l.is_empty() && !l.starts_with('#')) {
                let words = crate::headless::split_words(line);
                let b = crate::headless::input_events(&words, ptr, &|x, y| view.to_screen(kurbo::Point::new(x as f64, y as f64)))
                    .map_err(|e| format!("line {}: {e}", n + 1))?
                    .ok_or(format!("line {}: unknown input `{}`", n + 1, words[0]))?;
                batches.extend(b);
            }
            Ok(Outcome::Later(batches, After::State))
        }
        "render" => render(app, args),
        "screenshot" => Ok(Outcome::Later(Vec::new(), After::Screenshot(args.get("max_width").and_then(|v| v.as_u64()).map(|v| v.clamp(200, 4000) as u32)))),
        "animate" => Ok(text(super::anim::animate(app, args)?)),
        "get_animation" => Ok(text(super::anim::get_animation(app, args))),
        "clear_animation" => Ok(text(super::anim::clear(app, args)?)),
        "help" => Ok(Outcome::Done(vec![Content::Text(help(str_arg(args, "topic").unwrap_or("")))])),
        "ui_tree" => {
            let max = args.get("max").and_then(|v| v.as_u64()).unwrap_or(600) as usize;
            Ok(Outcome::Later(vec![vec![]], After::UiTree(str_arg(args, "filter").unwrap_or("").into(), max)))
        }
        "ui_click" => {
            let (ev, what) = click_widget(app, ptr, args, &[])?;
            Ok(Outcome::Later(ev, After::Widget(what)))
        }
        "ui_type" => {
            let t = str_arg(args, "text").ok_or("text is required")?;
            let (mut ev, what) = click_widget(app, ptr, args, &[])?;
            let key = |k: egui::Key, ctrl: bool| {
                let modifiers = if ctrl { egui::Modifiers::COMMAND } else { egui::Modifiers::NONE };
                [true, false].map(|pressed| vec![egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers }])
            };
            if args.get("append").and_then(|v| v.as_bool()) != Some(true) {
                ev.extend(key(egui::Key::A, true));
            }
            ev.push(vec![egui::Event::Text(t.into())]);
            if args.get("enter").and_then(|v| v.as_bool()).unwrap_or(true) {
                ev.extend(key(egui::Key::Enter, false));
            }
            ev.push(vec![]);
            Ok(Outcome::Later(ev, After::Widget(format!("{what}, typed {t:?}"))))
        }
        "get_document" => {
            let mut v = serde_json::to_value(&app.doc).map_err(|e| e.to_string())?;
            if let Some(o) = v.as_object_mut() {
                o.remove("shapes");
                let images: Vec<Value> = app.doc.images.iter().map(|(id, a)| json!({"id": id, "width": a.width, "height": a.height})).collect();
                o.insert("images".into(), json!(images));
            }
            Ok(text(v))
        }
        "patch_document" => {
            let patch = args.get("patch").ok_or("patch is required")?;
            if patch.get("shapes").is_some() || patch.get("images").is_some() {
                return Err("shapes and images aren't patched here: use the shape tools".into());
            }
            let mut v = serde_json::to_value(&app.doc).map_err(|e| e.to_string())?;
            let mut patch = patch.clone();
            // {"artboards": {"<id>": {...}}} patches those artboards only.
            if let Some(by_id) = patch.get("artboards").and_then(|a| a.as_object()).cloned() {
                let mut list = v["artboards"].clone();
                for (k, p) in by_id {
                    let item = list.as_array_mut().and_then(|l| l.iter_mut().find(|a| a["id"].as_u64().map(|i| i.to_string()) == Some(k.clone()))).ok_or(format!("no artboard {k}"))?;
                    merge_patch(item, &p);
                }
                patch["artboards"] = list;
            }
            merge_patch(&mut v, &patch);
            let mut doc: crate::document::Document = serde_json::from_value(v).map_err(|e| format!("patch: {e}"))?;
            doc.after_load();
            if doc.artboards.is_empty() {
                return Err("a document needs at least one artboard".into());
            }
            app.checkpoint("");
            app.doc = doc;
            app.cache.invalidate_all();
            app.selection.retain(|id| app.doc.get(*id).is_some());
            if args.get("full_state").and_then(|v| v.as_bool()) == Some(true) {
                return Ok(Outcome::Done(state_text(app, mode)));
            }
            let boards: Vec<Value> = app.doc.artboards.iter().map(|a| artboard_json(app, a)).collect();
            Ok(text(json!({"artboards": boards, "animation": super::anim::timeline_json(app), "unit": app.doc.unit.label()})))
        }
        "get_settings" => Ok(text(serde_json::to_value(&app.settings).map_err(|e| e.to_string())?)),
        "patch_settings" => {
            let mut v = serde_json::to_value(&app.settings).map_err(|e| e.to_string())?;
            merge_patch(&mut v, args.get("patch").ok_or("patch is required")?);
            let s: crate::settings::Settings = serde_json::from_value(v).map_err(|e| format!("patch: {e}"))?;
            app.apply_settings(s, ctx);
            Ok(text(serde_json::to_value(&app.settings).unwrap_or_default()))
        }
        "list_fonts" => {
            let f = str_arg(args, "filter").unwrap_or("").to_lowercase();
            let max = args.get("max").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
            let installed: Vec<&String> = app.fonts.families.iter().filter(|n| n.to_lowercase().contains(&f)).take(max).collect();
            let google: Vec<&String> = app.fonts.google.iter().map(|g| &g.family).filter(|n| n.to_lowercase().contains(&f) && !app.fonts.is_installed(n)).take(max).collect();
            Ok(text(json!({"installed": installed, "google_fonts_download_on_use": google})))
        }
        "set_view" => {
            if let Some(z) = num_arg(args, "zoom") {
                let c = app.canvas_rect.center();
                app.zoom_at((z as f32) / app.view.zoom, c);
            }
            if let Some(p) = point_arg(args, "center") {
                let now = app.view.to_screen(p);
                app.view.origin += app.canvas_rect.center() - now;
            }
            if let Some(ids) = args.get("fit").and_then(|v| v.as_array()) {
                let ids: Vec<u64> = ids.iter().filter_map(|v| v.as_u64()).collect();
                match ids_bounds(app, &ids) {
                    Some(b) => app.zoom_to_rect(b, 40.0),
                    None => app.zoom_to_fit(),
                }
            }
            app.view_ready = true;
            Ok(Outcome::Later(vec![vec![]], After::State))
        }
        "open_file" => {
            let p = std::path::PathBuf::from(str_arg(args, "path").ok_or("path is required")?);
            if !p.exists() {
                return Err(format!("{} doesn't exist", p.display()));
            }
            app.toast = None;
            app.open_path(std::path::absolute(&p).unwrap_or(p));
            if let Some((m, _)) = app.toast.as_ref().filter(|t| t.0.starts_with("Could not open")) {
                return Err(m.clone());
            }
            Ok(Outcome::Done(state_text(app, mode)))
        }
        "new_document" => {
            let bg = args.get("background").map(color).transpose()?;
            app.tab_for_new();
            app.new_document();
            app.ui.welcome_open = false;
            let a = &mut app.doc.artboards[0];
            if let Some(w) = num_arg(args, "w") {
                a.w = w.max(1.0);
            }
            if let Some(h) = num_arg(args, "h") {
                a.h = h.max(1.0);
            }
            if let Some(bg) = bg {
                a.background = bg.unwrap_or(Rgba { a: 0.0, ..Rgba::WHITE });
            }
            if let Some(n) = str_arg(args, "name") {
                a.name = n.into();
                app.doc.title = Some(n.into());
            }
            app.view_ready = false;
            Ok(Outcome::Done(state_text(app, mode)))
        }
        "set_artboard" => {
            let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or_else(|| active(app));
            let bg = args.get("background").map(color).transpose()?;
            let mut a = app.doc.artboard(id).cloned().ok_or(format!("no artboard {id}"))?;
            for (k, f) in [("x", &mut a.x), ("y", &mut a.y), ("w", &mut a.w), ("h", &mut a.h)] {
                if let Some(v) = num_arg(args, k) {
                    *f = if k == "w" || k == "h" { v.max(1.0) } else { v };
                }
            }
            if let Some(bg) = bg {
                a.background = bg.unwrap_or(Rgba { a: 0.0, ..Rgba::WHITE });
            }
            if let Some(n) = str_arg(args, "name") {
                a.name = n.into();
            }
            if let Some(c) = args.get("clip").and_then(|v| v.as_bool()) {
                a.clip = c;
            }
            app.checkpoint("");
            if let Some(slot) = app.doc.artboards.iter_mut().find(|b| b.id == id) {
                *slot = a;
            }
            app.cache.invalidate_all();
            if args.get("full_state").and_then(|v| v.as_bool()) == Some(true) {
                return Ok(Outcome::Done(state_text(app, mode)));
            }
            let a = app.doc.artboard(id).cloned().ok_or("artboard vanished")?;
            Ok(text(json!({"artboard": artboard_json(app, &a)})))
        }
        "place_file" => {
            let p = std::path::PathBuf::from(str_arg(args, "path").ok_or("path is required")?);
            if !p.exists() {
                return Err(format!("{} doesn't exist", p.display()));
            }
            app.toast = None;
            let n = app.history.undo_len();
            let before = super::edit::before(app);
            app.place_path(&p, point_arg(args, "at"));
            if app.history.undo_len() == n {
                return Err(app.toast.take().map(|t| t.0).unwrap_or("could not place the file".into()));
            }
            Ok(answer(app, args, &before, mode))
        }
        "save" => {
            if let Some(p) = str_arg(args, "path") {
                let mut p = std::path::absolute(p).map_err(|e| e.to_string())?;
                if p.extension().is_none() {
                    p.set_extension("mvec");
                }
                app.file_path = Some(p);
            }
            if app.file_path.is_none() {
                return Err("never saved: pass a path".into());
            }
            app.toast = None;
            app.dirty = true;
            app.save(false);
            if app.dirty {
                return Err(app.toast.take().map(|t| t.0).unwrap_or("save failed".into()));
            }
            Ok(text(json!({"saved": app.file_path})))
        }
        "export" => export(app, args),
        "switch_tab" => {
            let i = args.get("index").and_then(|v| v.as_u64()).ok_or("index is required")? as usize;
            if i >= app.tab_count() {
                return Err(format!("only {} documents are open", app.tab_count()));
            }
            app.switch_tab(i);
            Ok(Outcome::Done(state_text(app, mode)))
        }
        "playback" => {
            if app.doc.animation.is_none() {
                return Err("the document isn't animated".into());
            }
            if let Some(l) = args.get("loop").and_then(|v| v.as_bool()) {
                app.doc.animation.as_mut().unwrap().looping = l;
            }
            if let Some(t) = num_arg(args, "time") {
                let d = app.doc.animation.as_ref().map_or(0.0, |a| a.duration);
                app.anim_seek(t.clamp(0.0, d));
            }
            if let Some(p) = args.get("playing").and_then(|v| v.as_bool())
                && p != app.anim.playing
            {
                app.anim_toggle_play();
            }
            let tl = app.doc.animation.as_ref().unwrap();
            Ok(text(json!({"playing": app.anim.playing, "time": tl.time, "loop": tl.looping, "duration": tl.duration})))
        }
        "seek" => {
            let t = num_arg(args, "time").ok_or("time is required")?;
            let Some(tl) = &app.doc.animation else { return Err("the document isn't animated".into()) };
            let t = t.clamp(0.0, tl.duration);
            app.anim_seek(t);
            Ok(text(json!({"time": t})))
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

/// The answer to a deferred call (not `Screenshot`: the backend captures that).
pub fn after(app: &mut App, then: &After, mode: &str) -> Result<Vec<Content>, String> {
    match then {
        After::State | After::Screenshot(_) => Ok(state_text(app, mode)),
        After::Export(path) => match app.toast.as_ref().map(|t| t.0.clone()) {
            Some(m) if m.starts_with("Export failed") => Err(m),
            _ => Ok(vec![Content::Text(serde_json::to_string(&json!({"written": [path]})).unwrap_or_default())]),
        },
        After::UiTree(filter, max) => Ok(vec![Content::Text(super::ui_tree::format(&app.mcp.ui, filter, *max))]),
        After::Widget(what) => {
            let mut s = state(app, 0, 0, mode);
            if let Some(o) = s.as_object_mut() {
                o.retain(|k, _| ["selection", "tool", "dirty", "undo"].contains(&k.as_str()));
            }
            let toast = app.toast.as_ref().filter(|t| app.now - t.1 < 3.0).map(|t| t.0.clone());
            Ok(vec![Content::Text(format!("{what}\n{}{}", serde_json::to_string(&s).unwrap_or_default(), toast.map(|t| format!("\nmessage: {t}")).unwrap_or_default()))])
        }
    }
}

/// Click the middle of a widget from the last UI snapshot.
fn click_widget(app: &App, ptr: &mut Pointer, args: &Value, extra: &[&str]) -> Result<(Vec<Vec<egui::Event>>, String), String> {
    let n = super::ui_tree::find(
        &app.mcp.ui,
        str_arg(args, "id"),
        str_arg(args, "label"),
        str_arg(args, "role"),
        args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
    )?;
    if n.disabled {
        return Err(format!("#{} {:?} is disabled", n.id, n.label));
    }
    let c = n.rect.center();
    let mut line = vec![if args.get("double").and_then(|v| v.as_bool()) == Some(true) { "dblclick" } else { "click" }.to_string(), c.x.to_string(), c.y.to_string()];
    if args.get("right").and_then(|v| v.as_bool()) == Some(true) {
        line.push("right".into());
    }
    line.extend(extra.iter().map(|s| s.to_string()));
    let ev = crate::headless::input_events(&line, ptr, &|x, y| egui::pos2(x, y))?.unwrap_or_default();
    Ok((ev, format!("clicked #{} {:?}", super::ui_tree::short(n.id), if n.label.is_empty() { &n.value } else { &n.label })))
}
