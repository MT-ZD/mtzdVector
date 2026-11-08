//! Application state, document operations and file handling. Layout lives in
//! `ui`, actions in `commands`, platform glue in `platform`.

use std::path::PathBuf;

use egui_phosphor::regular as icon;
use kurbo::{Affine, Point, Rect};

use crate::boolean::Region;
use crate::document::{Document, History, ImageAsset, ImageFill, ImageMode, Paint, Shape, ShapeKind, Stroke};
use crate::fonts::FontManager;
use crate::platform::{Platform, clipboard};
use crate::render::{GeoCache, View};
use crate::ui::UiState;
use crate::{geom, io};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Node,
    Pen,
    Rect,
    Ellipse,
    Star,
    Polygon,
    /// Parametric primitive (triangle, arrow, cog…).
    Prim(crate::prims::PrimKind),
    Text,
    FrameText,
    /// Affinity-style fill tool: drag a gradient across shapes.
    Fill,
    ShapeBuilder,
    Corner,
    Knife,
    /// Freehand line, smoothed into curves.
    Pencil,
    /// Freehand stroke with a variable width (pressure or speed).
    Brush,
    Eyedropper,
    Zoom,
    Hand,
    Artboard,
}

impl Tool {
    pub fn icon(self) -> &'static str {
        match self {
            Tool::Select => icon::CURSOR,
            Tool::Node => icon::NAVIGATION_ARROW,
            Tool::Pen => icon::PEN_NIB,
            Tool::Rect => icon::RECTANGLE,
            Tool::Ellipse => icon::CIRCLE,
            Tool::Star => icon::STAR,
            Tool::Polygon => icon::POLYGON,
            Tool::Prim(k) => k.icon(),
            Tool::Fill => icon::PAINT_BUCKET,
            Tool::Text => icon::TEXT_T,
            Tool::FrameText => icon::TEXTBOX,
            Tool::ShapeBuilder => icon::UNITE,
            Tool::Corner => icon::BEZIER_CURVE,
            Tool::Knife => icon::KNIFE,
            Tool::Pencil => icon::PENCIL_SIMPLE,
            Tool::Brush => icon::PAINT_BRUSH,
            Tool::Eyedropper => icon::EYEDROPPER,
            Tool::Zoom => icon::MAGNIFYING_GLASS,
            Tool::Hand => icon::HAND,
            Tool::Artboard => icon::FRAME_CORNERS,
        }
    }

    /// Display name (shortcuts come from the command registry).
    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "Move",
            Tool::Node => "Node",
            Tool::Pen => "Pen",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Star => "Star",
            Tool::Polygon => "Polygon",
            Tool::Prim(k) => k.name(),
            Tool::Fill => "Fill",
            Tool::Text => "Artistic Text",
            Tool::FrameText => "Frame Text",
            Tool::ShapeBuilder => "Shape Builder",
            Tool::Corner => "Corner",
            Tool::Knife => "Knife",
            Tool::Pencil => "Pencil",
            Tool::Brush => "Brush",
            Tool::Eyedropper => "Eyedropper",
            Tool::Zoom => "Zoom",
            Tool::Hand => "Hand",
            Tool::Artboard => "Artboard",
        }
    }

    pub fn is_text(self) -> bool {
        matches!(self, Tool::Text | Tool::FrameText)
    }

    pub fn is_shape(self) -> bool {
        matches!(self, Tool::Rect | Tool::Ellipse | Tool::Star | Tool::Polygon | Tool::Prim(_))
    }

    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click to select · Drag to move · Shift adds · Alt+drag duplicates · Drag handles to scale · Drag the knob or outside corners to rotate (Shift snaps 15°) · Drag just beside a middle handle to shear (Alt about the centre) · Drag the pivot to move the rotation centre",
            Tool::Node => "Drag nodes or segments · Double-click a segment to add a node · Del removes nodes · Ctrl+drag bends a segment · Alt+drag a handle breaks smoothness",
            Tool::Pen => "Click for corners · Drag for curves · Click last node to drop its handle · Click first node to close · Backspace removes a node · Enter/Esc to finish",
            Tool::Rect | Tool::Ellipse | Tool::Star | Tool::Polygon | Tool::Prim(_) => "Drag to draw · Shift keeps proportions · Alt draws from center · Click selects · Drag the orange handles to reshape",
            Tool::Fill => "Drag across a shape to lay a gradient · Drag the ends, stops and midpoints · Double-click the line adds a stop · Del removes it · Shift snaps 45° · Mesh: drag points, double-click to slice",
            Tool::Text => "Click to add artistic text · Drag up/down to set the text size · Click text to edit · Esc to finish",
            Tool::FrameText => "Drag to draw a text frame (text wraps inside) · Click for a default frame · Click text to edit · Esc to finish",
            Tool::ShapeBuilder => "Drag across regions · Add merges · New keeps originals · Subtract (or Alt) removes",
            Tool::Corner => "Drag a corner inward to round it · Alt on rectangles edits a single corner",
            Tool::Knife => "Drag across filled shapes to cut them apart · Shift for a straight cut · Applies to the selection, or to everything crossed",
            Tool::Pencil => "Drag to draw a freehand line · End near the start to close it",
            Tool::Brush => "Drag to paint a stroke · Its width follows pen pressure (or drawing speed) · Ends taper",
            Tool::Eyedropper => "Click a shape or the artboard to copy its colour onto the selection's fill · Shift applies it to the stroke",
            Tool::Zoom => "Click to zoom in · Alt+click to zoom out · Drag a rectangle to zoom to it",
            Tool::Hand => "Drag to pan · Ctrl+scroll or pinch to zoom",
            Tool::Artboard => "Drag on empty canvas to add an artboard · Drag an artboard to move it with its artwork · Alt+drag clones it · Drag the handles to resize",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BuilderMode {
    /// Merge the marked regions into one shape (replacing the originals).
    Add,
    /// Remove the marked regions.
    Subtract,
    /// Create a new shape from the marked regions; originals stay untouched.
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

pub enum Drag {
    None,
    Pan,
    Move { start: Point, originals: Vec<Shape> },
    /// `frame` maps the box's local space to the document (rotation of a
    /// single rotated shape); `bbox` is in that local space.
    Scale { handle: Handle, bbox: Rect, frame: Affine, originals: Vec<Shape> },
    Rotate { center: Point, start_angle: f64, originals: Vec<Shape> },
    /// Moving the rotation pivot.
    Pivot,
    Marquee { start: Point, additive: bool },
    Create { start: Point, id: u64 },
    Nodes { start: Point, original: Vec<Shape> },
    Handle { id: u64, sub: usize, node: usize, is_out: bool, original: Shape, break_smooth: bool },
    Bend { sub: usize, seg: usize, t: f64, start: Point, original: Shape },
    NodeMarquee { start: Point, additive: bool },
    /// Pulling a node's handles out (pen, Ctrl+drag a node). `out_only`:
    /// only the outgoing handle moves (continuing a path, re-pulling the
    /// last node's handle); Alt does the same while dragging.
    PenHandle { id: u64, sub: usize, node: usize, out_only: bool },
    Corner { id: u64, corner: usize, single: bool, original: Shape },
    Builder { marked: Vec<usize>, mode: BuilderMode },
    /// Text tools: drawing a text frame (or a click for artistic text).
    TextFrame { start: Point },
    /// Dragging a text selection.
    TextSelect { id: u64 },
    /// Zoom tool rectangle.
    ZoomRect { start: Point },
    /// Knife stroke being drawn (document points).
    Knife { pts: Vec<Point> },
    /// Pencil / brush stroke: document points with their time (s) and pen
    /// pressure (when the device reports one); `live` is the shape drawn so
    /// far (in the document, replaced as the stroke grows).
    Freehand { pts: Vec<Point>, times: Vec<f64>, pressure: Vec<Option<f32>>, live: Option<u64> },
    /// Gradient overlay part (ends, stop, midpoint) of the fill of `id`.
    Gradient { id: u64, part: crate::gradient_handles::GradPart },
    /// Fill tool: laying a new gradient from `start` across `targets`.
    FillLay { start: Point, targets: Vec<u64> },
    /// Primitive parameter handle `i` of `id`.
    PrimHandle { id: u64, i: usize },
    /// Shearing by dragging a selection edge: `horizontal` (top or bottom
    /// edge), the grabbed `edge` coordinate, `bbox` and `start` in the
    /// selection `frame`'s local space.
    Shear { horizontal: bool, edge: f64, frame: Affine, bbox: Rect, start: Point, originals: Vec<Shape> },
}

/// An active snap: the aligned line plus what it snapped to.
#[derive(Clone, Debug)]
pub struct SnapGuide {
    /// Vertical line at x = `pos` (else horizontal at y = `pos`).
    pub vertical: bool,
    pub pos: f64,
    /// Bounds of the snap target (a shape or the artboard).
    pub target: Rect,
    /// Bounds of the moved selection after snapping.
    pub moving: Rect,
    /// The target is a shape (false = artboard).
    pub is_shape: bool,
}

pub struct PenState {
    pub id: u64,
    pub sub: usize,
}

pub struct TextEditState {
    pub id: u64,
    pub cursor: usize,
    /// Other end of the selection (== cursor when nothing is selected).
    pub anchor: usize,
    /// Shape consumed as a text path; put back if the text ends up empty.
    pub restore: Option<Shape>,
}

pub struct BuilderState {
    pub sig: Vec<(u64, u64)>,
    pub ids: Vec<u64>,
    pub regions: Vec<Region>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FontSource {
    All,
    Installed,
    Google,
}

pub struct FontPicker {
    pub open: bool,
    pub search: String,
    pub source: FontSource,
    pub category: String,
}

pub struct App {
    /// Identity of the active document's tab (see `tabs`).
    pub doc_uid: u64,
    /// Open documents; the active one's slot is a stale placeholder (its
    /// state lives in the fields below).
    pub tabs: Vec<crate::tabs::DocTab>,
    pub tab: usize,
    pub doc: Document,
    pub history: History,
    pub cache: GeoCache,
    pub fonts: FontManager,
    pub selection: Vec<u64>,
    /// `selection_bounds` for (revision, font generation, shape list, selection).
    sel_bounds_memo: Option<((u64, u64, usize), Vec<u64>, Option<Rect>)>,
    pub(crate) flow_key: Option<(u64, u64, usize)>,
    pub tool: Tool,
    pub view: View,
    pub view_ready: bool,
    pub fit_passes: u32,
    pub drag: Drag,
    pub press: Option<egui::Pos2>,
    pub dragging: bool,
    /// Shape a plain click selects when the press started a move of a covered selection.
    pub press_top: Option<u64>,
    pub hover: Option<u64>,
    pub node_sel: Vec<(u64, usize, usize)>,
    pub pen: Option<PenState>,
    /// Pen tool: where the next click places its node (snapped), for the preview.
    pub pen_hover: Option<Point>,
    pub text_edit: Option<TextEditState>,
    pub spell: crate::spell::SpellChecker,
    /// Misspelled ranges of the edited text: (shape, story revision, ranges).
    pub spell_marks: Option<(u64, u64, Vec<std::ops::Range<usize>>)>,
    /// Running print job (the portal dialog), reporting printed / cancelled.
    pub print_job: Option<std::sync::mpsc::Receiver<Result<crate::print::PrintResult, String>>>,
    /// Auto layout: (doc revision, font generation) last laid out, and where
    /// each group's members were placed (to spot drags).
    pub layout_key: Option<(u64, u64)>,
    /// Document revision the live blends were last checked at.
    pub live_key: Option<u64>,
    /// Live blend → hash of its ends and steps when last rebuilt.
    pub blend_keys: std::collections::HashMap<u64, u64>,
    pub layout_placed: std::collections::HashMap<u64, std::collections::HashMap<u64, Point>>,
    pub builder: Option<BuilderState>,
    pub builder_mode: BuilderMode,
    pub builder_hover: Option<usize>,
    pub star_points: u32,
    /// Knife: how much freehand cuts are smoothed (0..1).
    pub knife_smooth: f32,
    /// Pencil and brush: style, width, smoothing and colour (the pencil
    /// uses the smoothing and colour).
    pub brush: crate::freehand::BrushOpts,
    /// Stabilizer for pencil, brush and knife: on, mode, length (screen px),
    /// and the live one while drawing.
    pub stabilizer: bool,
    pub stab_mode: crate::freehand::StabMode,
    pub stab_len: f32,
    pub stab: Option<crate::freehand::Stabilizer>,
    /// Shearing from a selection edge: the angle being dragged (for the
    /// label), and the edge under the pointer (true: top or bottom edge,
    /// a horizontal shear).
    pub shear_angle: Option<f64>,
    pub shear_hover: Option<bool>,
    /// Pen pressure from the latest touch/stylus event (None: no pressure device).
    pub pen_pressure: Option<f32>,
    /// Pressure from a tablet-v2 pen (see `raw_input_hook`), its last
    /// position, and whether a tablet pen has been used this session.
    pub tablet_pressure: Option<f32>,
    pub tablet_pos: egui::Pos2,
    pub tablet_seen: bool,
    /// Input time of the current frame (s).
    pub input_time: f64,
    pub star_inner: f64,
    pub polygon_sides: u32,
    /// Last-edited parameters per primitive kind (new shapes start from them).
    pub prim_params: std::collections::HashMap<crate::prims::PrimKind, crate::prims::Prim>,
    pub default_text: crate::document::TextData,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    pub toast: Option<(String, f64)>,
    pub font_picker: FontPicker,
    /// In-process copy of the last copied selection (fallback clipboard and
    /// identity check for pastes coming back from the system clipboard).
    pub clipboard: Option<clipboard::Payload>,
    pub clipboard_svg: String,
    pub paste_count: u32,
    pub style_clip: Option<StyleClip>,
    pub canvas_rect: egui::Rect,
    pub cursor_doc: Option<Point>,
    pub corner_radius_ui: f64,
    /// Artboard the panel edits and exports target (see [`App::active`]).
    pub active_artboard: u64,
    /// The active artboard itself is selected (handles; Delete removes it).
    pub artboard_selected: bool,
    /// Screen rects of the artboard name labels (last frame).
    pub artboard_labels: Vec<(u64, egui::Rect)>,
    pub artboard_label_hover: Option<u64>,
    /// Artboard drag on the canvas: (id, start pointer, original rect, shapes moved along with their original transforms).
    pub artboard_drag: Option<crate::artboards::ArtboardDrag>,
    pub layer_drag: Option<u64>,
    /// Artboard being dragged: left out of the snap targets, and other
    /// artboards are the smart-spacing boxes.
    pub snap_artboard: Option<u64>,
    /// Components: per instance, the (main, instance) revisions last synced,
    /// and the document revision after the last quiet sync pass.
    pub comp_sync: std::collections::HashMap<u64, (u64, u64)>,
    pub comp_rev: u64,
    /// Shared component libraries, loaded on demand.
    pub libraries: crate::library::LibraryCache,
    pub renaming: Option<(u64, String)>,
    /// The rename field opened this frame (takes focus once).
    pub rename_fresh: bool,
    /// The rename field is the artboard label on the canvas (not Layers).
    pub rename_on_canvas: bool,
    pub now: f64,
    pub last_title: String,
    pub ui: UiState,
    pub platform: Platform,
    /// Last shape tool used (shown on the grouped tool button).
    /// Rotation pivot relative to the selection box (0..1 per axis), for
    /// the selection in `pivot_for`.
    pub pivot: Option<(f64, f64)>,
    pub pivot_for: Vec<u64>,
    /// Pointer is over a rotation zone (draws the rotate cursor).
    pub rotate_hover: bool,
    pub shape_tool: Tool,
    /// Last text tool used.
    pub text_tool: Tool,
    pub snap: bool,
    /// Snap guides to draw this frame (document-space lines: vertical if `.0`).
    pub snap_guides: Vec<SnapGuide>,
    /// Smart-spacing gaps shown while moving.
    pub gap_guides: Vec<crate::snap::Gap>,
    /// Guide being dragged: (index, None for a new one from the ruler), vertical, position.
    pub guide_drag: Option<(Option<usize>, bool, f64)>,
    pub guide_hover: Option<usize>,
    /// Tool to return to after a one-shot tool (eyedropper).
    pub prev_tool: Option<Tool>,
    /// Last duplicate (copies, bounds of their source) for step-and-repeat.
    pub last_dup: Option<(Vec<u64>, Rect)>,
    pub settings: crate::settings::Settings,
    pub settings_saved: crate::settings::Settings,
    pub settings_last_save: f64,
    /// Command requested from inside a panel closure, run after the UI pass.
    pub pending_cmd: Option<crate::commands::Cmd>,
    /// Select this shape after the frame (panels can't change the selection mid-frame).
    pub pending_select: Option<u64>,
    pub icons: crate::icons::IconLibrary,
    pub session: crate::session::Session,
    pub egui_ctx: egui::Context,
    /// Drag & drop hovering the window (screen position).
    pub drop_hover: Option<egui::Pos2>,
    /// Document position the context menu was opened at ("Paste here").
    pub menu_anchor_doc: Option<Point>,
}

/// Fill, stroke and opacity copied with Copy Style.
#[derive(Clone)]
pub struct StyleClip {
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
    pub opacity: f32,
    pub shadow: Option<crate::document::Shadow>,
    pub inner_shadow: Option<crate::document::Shadow>,
    pub blur: f64,
    pub noise: f32,
    pub outlines: Vec<crate::document::OutlineStep>,
    pub fill_key: Option<u64>,
    pub stroke_key: Option<u64>,
}

impl App {
    pub fn new(ctx: egui::Context, file: Option<PathBuf>) -> Self {
        let mut fonts = FontManager::new(ctx.clone());
        fonts.load_google_list(false);
        let mut default_text = crate::document::TextData::default();
        if !fonts.is_installed(&default_text.family) {
            for f in ["Inter", "Cantarell", "DejaVu Sans"] {
                if fonts.is_installed(f) {
                    default_text.family = f.to_string();
                    break;
                }
            }
        }
        let first = crate::tabs::DocTab::default();
        let mut app = App {
            doc_uid: crate::tabs::next_uid(),
            tabs: vec![first],
            tab: 0,
            doc: Document::default(),
            history: History::default(),
            cache: GeoCache::default(),
            sel_bounds_memo: None,
            flow_key: None,
            fonts,
            selection: Vec::new(),
            tool: Tool::Select,
            view: View { zoom: 0.5, origin: egui::vec2(0.0, 0.0) },
            view_ready: false,
            fit_passes: 0,
            drag: Drag::None,
            press: None,
            dragging: false,
            press_top: None,
            hover: None,
            node_sel: Vec::new(),
            pen: None,
            pen_hover: None,
            text_edit: None,
            spell: Default::default(),
            spell_marks: None,
            print_job: None,
            layout_key: None,
            live_key: None,
            blend_keys: Default::default(),
            layout_placed: Default::default(),
            builder: None,
            builder_mode: BuilderMode::Add,
            builder_hover: None,
            star_points: 5,
            knife_smooth: 0.4,
            brush: Default::default(),
            stabilizer: false,
            stab_mode: Default::default(),
            stab_len: 30.0,
            stab: None,
            shear_angle: None,
            shear_hover: None,
            pen_pressure: None,
            tablet_pressure: None,
            tablet_pos: egui::Pos2::ZERO,
            tablet_seen: false,
            input_time: 0.0,
            star_inner: 0.5,
            polygon_sides: 6,
            prim_params: Default::default(),
            default_text,
            file_path: None,
            dirty: false,
            toast: None,
            font_picker: FontPicker { open: false, search: String::new(), source: FontSource::All, category: "All".into() },
            clipboard: None,
            clipboard_svg: String::new(),
            paste_count: 0,
            style_clip: None,
            canvas_rect: egui::Rect::NOTHING,
            cursor_doc: None,
            corner_radius_ui: 16.0,
            active_artboard: 0,
            artboard_selected: false,
            artboard_labels: Vec::new(),
            artboard_label_hover: None,
            artboard_drag: None,
            layer_drag: None,
            snap_artboard: None,
            comp_sync: Default::default(),
            comp_rev: 0,
            libraries: Default::default(),
            renaming: None,
            rename_fresh: false,
            rename_on_canvas: false,
            now: 0.0,
            last_title: String::new(),
            ui: UiState::default(),
            platform: Platform::none(),
            pivot: None,
            pivot_for: Vec::new(),
            rotate_hover: false,
            shape_tool: Tool::Rect,
            text_tool: Tool::Text,
            snap: true,
            snap_guides: Vec::new(),
            gap_guides: Vec::new(),
            guide_drag: None,
            guide_hover: None,
            prev_tool: None,
            last_dup: None,
            settings: Default::default(),
            settings_saved: Default::default(),
            settings_last_save: 0.0,
            pending_cmd: None,
            pending_select: None,
            icons: crate::icons::IconLibrary::new(ctx.clone()),
            session: Default::default(),
            egui_ctx: ctx.clone(),
            drop_hover: None,
            menu_anchor_doc: None,
        };
        if let Some(f) = file {
            app.open_path(f);
        }
        app
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    pub fn set_unit(&mut self, u: crate::units::Unit) {
        self.doc.unit = u;
        crate::units::set(u, self.doc.dpi);
        self.dirty = true;
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), self.now));
    }

    pub fn checkpoint(&mut self, key: &str) {
        self.history.checkpoint(&self.doc, key, self.now);
        self.dirty = true;
    }

    pub fn selected_shapes(&self) -> Vec<&Shape> {
        self.selection.iter().filter_map(|id| self.doc.get(*id)).collect()
    }

    /// Union of selected shape bounds (document space).
    pub fn selection_bounds(&mut self) -> Option<Rect> {
        let key = (self.doc.revision(), self.fonts.generation, self.doc.shapes().as_ptr() as usize);
        if let Some((k, sel, r)) = &self.sel_bounds_memo {
            if *k == key && *sel == self.selection {
                return *r;
            }
        }
        let mut r = None;
        for id in &self.selection {
            if let Some(b) = self.doc.get(*id).and_then(|s| self.cache.update(s, &mut self.fonts).bounds) {
                r = geom::union_rect(r, b);
            }
        }
        self.sel_bounds_memo = Some((key, self.selection.clone(), r));
        r
    }

    /// Selection box as (local → document frame, local rect). A single rotated
    /// primitive or text gets an oriented box; otherwise it's the axis-aligned
    /// bounds.
    pub fn sel_frame(&mut self) -> Option<(Affine, Rect)> {
        if self.selection.len() == 1 {
            if let Some(s) = self.doc.get(self.selection[0]).cloned() {
                if !matches!(s.kind, ShapeKind::Path(_)) {
                    // The shape's own axes, unit length: the box turns with a
                    // rotation and slants with a shear, like Affinity's.
                    let [a, b, c, d, ..] = s.transform.as_coeffs();
                    let (lu, lv) = ((a * a + b * b).sqrt(), (c * c + d * d).sqrt());
                    let (u, v) = ((a / lu.max(1e-12), b / lu.max(1e-12)), (c / lv.max(1e-12), d / lv.max(1e-12)));
                    let skewed = (u.0 * v.1 - u.1 * v.0).abs() > 0.05;
                    let tilted = (u.0 - 1.0).abs() > 1e-6 || u.1.abs() > 1e-6 || v.0.abs() > 1e-6 || (v.1 - 1.0).abs() > 1e-6;
                    if skewed && tilted && lu > 1e-12 && lv > 1e-12 {
                        let frame = Affine::new([u.0, u.1, v.0, v.1, 0.0, 0.0]);
                        let inv = frame.inverse();
                        let c = self.cache.update(&s, &mut self.fonts);
                        let local = match &c.text {
                            Some(t) => Some((inv * s.transform).transform_rect_bbox(t.bounds)),
                            None => geom::path_bounds(&(inv * (*c.doc_path).clone())),
                        };
                        if let Some(l) = local {
                            return Some((frame, l));
                        }
                    }
                }
            }
        }
        Some((Affine::IDENTITY, self.selection_bounds()?))
    }

    /// Rotation pivot in document space (the box center unless moved).
    pub fn pivot_point(&mut self) -> Option<Point> {
        if self.pivot_for != self.selection {
            self.pivot = None;
            self.pivot_for = self.selection.clone();
        }
        let (frame, r) = self.sel_frame()?;
        let (fx, fy) = self.pivot.unwrap_or((0.5, 0.5));
        Some(frame * Point::new(r.x0 + r.width() * fx, r.y0 + r.height() * fy))
    }

    pub fn set_tool(&mut self, tool: Tool) {
        if self.tool == tool {
            return;
        }
        self.prev_tool = Some(self.tool);
        self.finish_pen();
        self.finish_text_edit();
        self.builder = None;
        self.drag = Drag::None;
        if tool != Tool::Node && tool != Tool::Corner {
            self.node_sel.clear();
        }
        if tool.is_shape() {
            self.shape_tool = tool;
        }
        if tool.is_text() {
            self.text_tool = tool;
        }
        self.tool = tool;
    }

    /// Open the inline rename field for a layer or artboard (Layers tab).
    pub fn start_rename(&mut self, id: u64) {
        let name = self.doc.artboard(id).map(|a| a.name.clone()).or_else(|| self.doc.get(id).map(|s| s.name.clone()));
        if let Some(n) = name {
            self.renaming = Some((id, n));
            self.rename_fresh = true;
            self.rename_on_canvas = false;
        }
    }

    pub fn finish_pen(&mut self) {
        if let Some(p) = self.pen.take() {
            let remove = match self.doc.get(p.id).map(|s| &s.kind) {
                Some(ShapeKind::Path(pd)) => pd.node_count() < 2,
                _ => false,
            };
            if remove {
                self.doc.remove(p.id);
                self.selection.retain(|i| *i != p.id);
            }
        }
    }

    pub fn finish_text_edit(&mut self) {
        if let Some(te) = self.text_edit.take() {
            let empty = matches!(self.doc.get(te.id).map(|s| &s.kind), Some(ShapeKind::Text(t)) if t.content.trim().is_empty() && t.flow.is_none() && t.next.is_none());
            if empty {
                let idx = self.doc.index_of(te.id).unwrap_or(0);
                self.doc.remove(te.id);
                self.selection.retain(|i| *i != te.id);
                if let Some(g) = te.restore {
                    self.doc.insert_shape(idx, g);
                }
            }
        }
    }

    pub fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in std::mem::take(&mut self.selection) {
            self.doc.remove(id);
        }
        self.doc.gc_images();
        self.node_sel.clear();
    }

    /// Duplicate the selection next to itself (inside its group, if any).
    /// Duplicating a copy you just moved repeats that move (step and repeat).
    pub fn duplicate_selection(&mut self, offset: f64) {
        if self.selection.is_empty() {
            return;
        }
        let cur = self.selection_bounds();
        let d = match (&self.last_dup, cur) {
            (Some((ids, src)), Some(c)) if *ids == self.selection && (c.origin() - src.origin()).hypot() > 1e-9 => c.origin() - src.origin(),
            _ => kurbo::Vec2::new(offset, offset),
        };
        self.checkpoint("");
        let mut new_sel = Vec::new();
        for id in self.selection.clone() {
            if let Some(orig) = self.doc.get(id).cloned() {
                let mut s = orig.clone();
                s.id = 0;
                if !s.name.ends_with(" copy") {
                    s.name = format!("{} copy", s.name);
                }
                s.apply_transform(Affine::translate(d));
                if let Some(&nid) = self.doc.replace_with(id, vec![orig.clone(), s]).get(1) {
                    self.instance_from_copy(&orig, nid);
                    new_sel.push(nid);
                }
            }
        }
        self.selection = new_sel.clone();
        self.last_dup = cur.map(|c| (new_sel, c));
    }

    pub fn begin_text_edit(&mut self, id: u64) {
        let len = match self.doc.get(id).map(|s| &s.kind) {
            Some(ShapeKind::Text(t)) => t.content.len(),
            _ => return,
        };
        if !self.tool.is_text() {
            self.tool = Tool::Text;
        }
        self.selection = vec![id];
        self.text_edit = Some(TextEditState { id, cursor: len, anchor: len, restore: None });
    }

    pub fn undo(&mut self) {
        self.finish_pen();
        self.text_edit = None;
        if self.history.undo(&mut self.doc) {
            self.selection.retain(|id| self.doc.get(*id).is_some());
            self.node_sel.clear();
        }
    }

    pub fn redo(&mut self) {
        if self.history.redo(&mut self.doc) {
            self.selection.retain(|id| self.doc.get(*id).is_some());
            self.node_sel.clear();
        }
    }

    pub fn select_all(&mut self) {
        self.selection = self.doc.shapes().iter().filter(|s| !s.locked && s.visible).map(|s| s.id).collect();
    }

    /// Center of the visible canvas in document space.
    pub fn view_center_doc(&self) -> Point {
        if self.canvas_rect.width() > 1.0 {
            self.view.to_doc(self.canvas_rect.center())
        } else {
            self.active().rect().center()
        }
    }

    // ------------------------------------------------------------------
    // Clipboard
    // ------------------------------------------------------------------

    /// Copy the selection: lossless JSON + SVG + text on the system
    /// clipboard (falls back to egui's text clipboard).
    pub fn copy(&mut self, ctx: &egui::Context) {
        if self.selection.is_empty() {
            return;
        }
        let mut payload = clipboard::build_payload(&self.doc, &self.selection);
        payload.origin = self.selection_bounds().and_then(|b| self.doc.artboard_for(b)).map(|a| (a.x, a.y));
        let svg = clipboard::payload_svg(&payload, &mut self.fonts);
        let json = clipboard::payload_to_json(&payload);
        let n = payload.shapes.len();
        self.clipboard = Some(payload);
        self.clipboard_svg = svg.clone();
        self.paste_count = 0;
        let rich = self.platform.clipboard.copy(vec![
            (clipboard::MIME_INTERNAL, json),
            (clipboard::MIME_SVG, svg.clone().into_bytes()),
            ("text/plain", svg.clone().into_bytes()),
        ]);
        if rich.is_err() {
            ctx.copy_text(svg);
        }
        self.notify(format!("Copied {n} object{}", if n == 1 { "" } else { "s" }));
    }

    /// Copy only the SVG markup (for pasting into code editors, browsers…).
    pub fn copy_svg_only(&mut self, ctx: &egui::Context) {
        if self.selection.is_empty() {
            return;
        }
        let mut payload = clipboard::build_payload(&self.doc, &self.selection);
        payload.origin = self.selection_bounds().and_then(|b| self.doc.artboard_for(b)).map(|a| (a.x, a.y));
        let svg = clipboard::payload_svg(&payload, &mut self.fonts);
        let rich = self.platform.clipboard.copy(vec![
            (clipboard::MIME_SVG, svg.clone().into_bytes()),
            ("text/plain", svg.clone().into_bytes()),
        ]);
        if rich.is_err() {
            ctx.copy_text(svg);
        }
        self.notify("Copied as SVG");
    }

    /// Paste from the system clipboard (or the in-process copy). With `at`,
    /// the pasted content is centered on that document point.
    pub fn paste(&mut self, _ctx: &egui::Context, at: Option<Point>) {
        let content = if self.platform.clipboard.available() {
            self.platform.clipboard.read()
        } else {
            self.clipboard.clone().map(clipboard::ClipContent::Internal)
        };
        match content {
            Some(c) => self.paste_content(c, at),
            None => self.notify("Clipboard is empty"),
        }
    }

    /// Paste text that arrived through egui's clipboard (fallback path).
    pub fn paste_text_event(&mut self, text: &str, at: Option<Point>) {
        if !self.clipboard_svg.is_empty() && text.trim() == self.clipboard_svg.trim() {
            if let Some(p) = self.clipboard.clone() {
                self.paste_content(clipboard::ClipContent::Internal(p), at);
                return;
            }
        }
        if clipboard::looks_like_svg(text) {
            self.paste_content(clipboard::ClipContent::Svg(text.as_bytes().to_vec()), at);
        } else if let Some(p) = self.clipboard.clone() {
            self.paste_content(clipboard::ClipContent::Internal(p), at);
        }
    }

    /// Move freshly pasted shapes into the target artboard: at the same
    /// position relative to it when they came from another artboard, centred
    /// when they came from outside one (and an artboard is selected). Returns
    /// whether anything moved.
    fn paste_into_artboard(&mut self, ids: &[u64], origin: Option<(f64, f64)>, target: &crate::document::Artboard) -> bool {
        match origin {
            Some((x, y)) if (x, y) != (target.x, target.y) => {
                self.translate_ids(ids, kurbo::Vec2::new(target.x - x, target.y - y));
                true
            }
            None if self.artboard_selected => {
                self.center_ids_at(ids, target.rect().center());
                true
            }
            _ => false,
        }
    }

    pub fn paste_content(&mut self, content: clipboard::ClipContent, at: Option<Point>) {
        use clipboard::ClipContent;
        let ids = match content {
            ClipContent::Internal(p) => {
                if p.is_empty() {
                    return;
                }
                self.checkpoint("");
                let ids = self.insert_payload(&p);
                let target = self.active().clone();
                let moved = at.is_none() && self.paste_into_artboard(&ids, p.origin, &target);
                if at.is_none() && !moved {
                    // Successive pastes cascade so copies don't stack exactly.
                    self.paste_count += 1;
                    let same = self.clipboard.as_ref().map(|c| c.shapes.first().map(|s| s.id) == p.shapes.first().map(|s| s.id)).unwrap_or(false);
                    if same {
                        let d = 10.0 * self.paste_count as f64;
                        self.translate_ids(&ids, kurbo::Vec2::new(d, d));
                    }
                }
                ids
            }
            ClipContent::Svg(data) => {
                self.checkpoint("");
                match io::svg::import_into(&data, &mut self.fonts, &mut self.doc) {
                    Ok(ids) => {
                        if at.is_none() && self.artboard_selected {
                            let c = self.active().rect().center();
                            self.center_ids_at(&ids, c);
                        }
                        ids
                    }
                    Err(e) => {
                        self.notify(format!("Paste failed: {e}"));
                        return;
                    }
                }
            }
            ClipContent::Image(bytes) => match ImageAsset::from_encoded(bytes) {
                Ok(asset) => {
                    self.checkpoint("");
                    let c = at.unwrap_or_else(|| if self.artboard_selected { self.active().rect().center() } else { self.view_center_doc() });
                    vec![self.add_image_shape(asset, "Pasted image", c)]
                }
                Err(e) => {
                    self.notify(format!("Could not paste image: {e}"));
                    return;
                }
            },
            ClipContent::Files(files) => {
                let at = at.unwrap_or_else(|| self.view_center_doc());
                self.place_files(&files, at);
                return;
            }
            ClipContent::Text => {
                self.notify("Clipboard holds text, not graphics");
                return;
            }
        };
        if ids.is_empty() {
            return;
        }
        if let Some(at) = at {
            self.center_ids_at(&ids, at);
        }
        let n = ids.len();
        self.selection = ids;
        self.set_tool(Tool::Select);
        self.notify(format!("Pasted {n} object{}", if n == 1 { "" } else { "s" }));
    }

    /// Insert payload shapes (remapping image ids) and return the new ids.
    pub fn insert_payload(&mut self, p: &clipboard::Payload) -> Vec<u64> {
        let mut map = std::collections::HashMap::new();
        for (k, img) in &p.images {
            map.insert(*k, self.doc.add_image(img.clone()));
        }
        let mut ids = Vec::new();
        for s in &p.shapes {
            let mut s = s.clone();
            s.id = 0;
            if let Some(Paint::Image(f)) = &mut s.fill {
                f.image = map.get(&f.image).copied().unwrap_or(f.image);
            }
            self.doc.fix_key_links(&mut s);
            let id = self.doc.add_shape(s);
            self.instance_from_copy(&p.shapes[ids.len()], id);
            ids.push(id);
        }
        ids
    }

    pub fn translate_ids(&mut self, ids: &[u64], d: kurbo::Vec2) {
        for id in ids {
            if let Some(s) = self.doc.get_mut(*id) {
                s.apply_transform(Affine::translate(d));
            }
        }
    }

    /// Move shapes so the union of their bounds is centered on `at`.
    pub fn center_ids_at(&mut self, ids: &[u64], at: Point) {
        let mut r = None;
        for id in ids {
            if let Some(s) = self.doc.get(*id).cloned() {
                if let Some(b) = self.cache.update(&s, &mut self.fonts).bounds {
                    r = geom::union_rect(r, b);
                }
            }
        }
        if let Some(b) = r {
            self.translate_ids(ids, at - b.center());
        }
    }

    pub fn reorder(&mut self, how: i32) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("");
        let n = self.doc.shapes().len();
        let mut ids = self.selection.clone();
        ids.sort_by_key(|id| self.doc.index_of(*id).unwrap_or(0));
        if how > 0 {
            ids.reverse();
        }
        for id in ids {
            let Some(i) = self.doc.index_of(id) else { continue };
            let target = match how {
                2 => n,
                1 => i + 1,
                -1 => i.saturating_sub(1),
                _ => 0,
            };
            self.doc.move_to_index(id, target);
        }
    }

    pub fn flip(&mut self, horizontal: bool) {
        let Some(b) = self.selection_bounds() else { return };
        self.checkpoint("");
        let c = b.center();
        let a = Affine::translate(c.to_vec2())
            * if horizontal { Affine::scale_non_uniform(-1.0, 1.0) } else { Affine::scale_non_uniform(1.0, -1.0) }
            * Affine::translate(-c.to_vec2());
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                s.apply_transform(a);
            }
        }
    }

    pub fn convert_to_curves(&mut self) {
        let ids: Vec<u64> = self.selection.clone();
        let mut changed = false;
        for id in ids {
            let Some(s) = self.doc.get(id).cloned() else { continue };
            let pd = match &s.kind {
                ShapeKind::Path(pd) if pd.subpaths.iter().any(|sp| sp.nodes.iter().any(|n| n.corner > 0.0)) => {
                    // Bake live corners into real geometry.
                    Some(geom::bez_to_pathdata(&geom::pathdata_to_bez(pd)))
                }
                ShapeKind::Path(_) => None,
                ShapeKind::Text(_) | ShapeKind::Compound { .. } => {
                    let path = self.cache.update(&s, &mut self.fonts).doc_path.clone();
                    Some(geom::bez_to_pathdata(&path))
                }
                ShapeKind::Rect { radii, .. } if radii.iter().any(|r| *r > 0.0) => {
                    Some(geom::bez_to_pathdata(&(s.transform * geom::local_path(&s.kind))))
                }
                ShapeKind::Star { corner, .. } | ShapeKind::Polygon { corner, .. } if *corner > 0.0 => {
                    Some(geom::bez_to_pathdata(&(s.transform * geom::local_path(&s.kind))))
                }
                kind => geom::kind_nodes(kind).map(|mut pd| {
                    pd.transform(s.transform);
                    pd
                }),
            };
            if let Some(pd) = pd {
                if !changed {
                    self.checkpoint("");
                    changed = true;
                }
                let mut ns = s.clone();
                if matches!(s.kind, ShapeKind::Text(_)) {
                    ns.name = format!("{} (outlines)", s.name);
                }
                ns.kind = ShapeKind::Path(pd);
                ns.transform = Affine::IDENTITY;
                crate::render::keep_gradient(&s, &mut ns, &mut self.fonts);
                self.doc.replace(ns);
            }
        }
        if changed {
            self.notify("Converted to curves");
        }
    }

    pub fn align(&mut self, mode: u8) {
        let Some(sel) = self.selection_bounds() else { return };
        // A single object aligns to its artboard.
        let target = if self.selection.len() == 1 { self.doc.artboard_for(sel).unwrap_or(self.active()).rect() } else { sel };
        self.checkpoint("");
        for id in self.selection.clone() {
            let Some(s) = self.doc.get(id).cloned() else { continue };
            let Some(b) = self.cache.update(&s, &mut self.fonts).bounds else { continue };
            let d = match mode {
                0 => (target.x0 - b.x0, 0.0),
                1 => (target.center().x - b.center().x, 0.0),
                2 => (target.x1 - b.x1, 0.0),
                3 => (0.0, target.y0 - b.y0),
                4 => (0.0, target.center().y - b.center().y),
                _ => (0.0, target.y1 - b.y1),
            };
            if let Some(s) = self.doc.get_mut(id) {
                s.apply_transform(Affine::translate(d));
            }
        }
    }

    /// Fit every artboard in the view.
    pub fn zoom_to_fit(&mut self) {
        let b = self.doc.bounds();
        self.zoom_to_rect(b, 80.0);
    }

    pub fn zoom_at(&mut self, factor: f32, at: egui::Pos2) {
        let nz = (self.view.zoom * factor).clamp(0.02, 256.0);
        let k = nz / self.view.zoom;
        self.view.origin = at.to_vec2() - (at.to_vec2() - self.view.origin) * k;
        self.view.zoom = nz;
    }

    pub fn zoom_to_selection(&mut self) {
        let Some(b) = self.selection_bounds() else { return };
        self.zoom_to_rect(b, 120.0);
    }

    /// Fit a document rectangle into the canvas with `margin` screen px.
    pub fn zoom_to_rect(&mut self, b: Rect, margin: f32) {
        let r = self.canvas_rect;
        if r.width() < 10.0 {
            return;
        }
        let z = ((r.width() - margin) / b.width().max(1.0) as f32).min((r.height() - margin) / b.height().max(1.0) as f32).clamp(0.02, 256.0);
        self.view.zoom = z;
        let c = b.center();
        self.view.origin = egui::vec2(r.center().x - c.x as f32 * z, r.center().y - c.y as f32 * z);
    }

    // ------------------------------------------------------------------
    // Files
    // ------------------------------------------------------------------

    pub fn new_document(&mut self) {
        self.doc = Document::default();
        self.doc.unit = self.settings.default_unit;
        self.history.clear();
        self.cache.invalidate_all();
        self.selection.clear();
        self.file_path = None;
        self.dirty = false;
        self.view_ready = false;
        self.fit_passes = 0;
    }

    /// Open a document in its own tab (switching to it if already open; a
    /// blank active tab is reused).
    pub fn open_path(&mut self, path: PathBuf) {
        if let Some(i) = self.tab_with_path(&path) {
            self.switch_tab(i);
            self.ui.welcome_open = false;
            return;
        }
        match io::open(&path, &mut self.fonts) {
            Ok(doc) => {
                let n = doc.shapes().len();
                self.tab_for_new();
                self.doc = doc;
                self.history.clear();
                self.cache.invalidate_all();
                self.selection.clear();
                self.view_ready = false;
                self.fit_passes = 0;
                self.dirty = false;
                self.notify(format!("Opened {} ({} objects)", path.display(), n));
                crate::recent::push(&path);
                self.ui.welcome_open = false;
                self.file_path = matches!(io::format_of(&path), Some(io::Format::Native)).then_some(path);
            }
            Err(e) => self.notify(format!("Could not open {}: {e}", path.display())),
        }
    }

    pub fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("All supported", &["mvec", "svg", "svgz", "pdf", "eps", "ps", "ai"])
            .add_filter("mtzdVector", &["mvec"])
            .add_filter("SVG", &["svg", "svgz"])
            .add_filter("PDF", &["pdf"])
            .add_filter("Illustrator", &["ai"])
            .add_filter("EPS", &["eps", "ps"])
            .pick_files()
        {
            for p in p {
                self.open_path(p);
            }
        }
    }

    pub fn save(&mut self, save_as: bool) {
        let path = match (&self.file_path, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new().add_filter("mtzdVector", &["mvec"]).set_file_name("Untitled.mvec").save_file(),
        };
        if let Some(mut p) = path {
            if p.extension().is_none() {
                p.set_extension("mvec");
            }
            let opts = io::native::SaveOptions { backup: self.settings.keep_backup, preview: true };
            match io::native::save(&self.doc, &p, Some(&mut self.fonts), opts) {
                Ok(()) => {
                    self.notify(format!("Saved {}", p.display()));
                    crate::recent::push(&p);
                    self.file_path = Some(p);
                    self.dirty = false;
                    crate::session::clear_own(self.doc_uid);
                }
                Err(e) => self.notify(format!("Save failed: {e}")),
            }
        }
    }

    /// Place a vector file or image into the current document.
    pub fn place_path(&mut self, path: &std::path::Path, at: Option<Point>) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" => match std::fs::read(path).map_err(|e| e.to_string()).and_then(ImageAsset::from_encoded) {
                Ok(asset) => {
                    self.checkpoint("");
                    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("Image".into());
                    let at = at.unwrap_or(self.active().rect().center());
                    let id = self.add_image_shape(asset, &name, at);
                    self.selection = vec![id];
                }
                Err(e) => self.notify(format!("Could not load image: {e}")),
            },
            _ => match io::open(path, &mut self.fonts) {
                Ok(src) => {
                    self.checkpoint("");
                    let mut map = std::collections::HashMap::new();
                    for (k, img) in src.images.iter() {
                        map.insert(*k, self.doc.add_image(img.clone()));
                    }
                    let mut ids = Vec::new();
                    for s in src.shapes() {
                        let mut s = Shape::clone(s);
                        s.id = 0;
                        if let Some(Paint::Image(f)) = &mut s.fill {
                            f.image = map.get(&f.image).copied().unwrap_or(f.image);
                        }
                        ids.push(self.doc.add_shape(s));
                    }
                    if let Some(at) = at {
                        self.center_ids_at(&ids, at);
                    }
                    self.notify(format!("Placed {} objects", ids.len()));
                    self.selection = ids;
                }
                Err(e) => self.notify(format!("Could not place file: {e}")),
            },
        }
    }

    /// Add a rectangle filled with an image, centered on `at` (max 1000 px).
    pub fn add_image_shape(&mut self, asset: ImageAsset, name: &str, at: Point) -> u64 {
        let (w, h) = (asset.width as f64, asset.height as f64);
        let k = (1000.0 / w.max(h)).min(1.0);
        let img = self.doc.add_image(asset);
        let mut s = Shape::new(ShapeKind::Rect { w: w * k, h: h * k, radii: [0.0; 4] });
        s.name = name.to_string();
        s.transform = Affine::translate((at.x - w * k / 2.0, at.y - h * k / 2.0));
        s.fill = Some(Paint::Image(ImageFill { image: img, mode: ImageMode::Fill, scale: 1.0, opacity: 1.0 }));
        self.doc.add_shape(s)
    }

    /// Import dropped / pasted files at `at`. Multiple files cascade; a single
    /// `.mvec` (or any document) on an empty canvas is opened instead.
    pub fn place_files(&mut self, files: &[PathBuf], at: Point) {
        // Native documents open in their own tabs.
        let (native, files): (Vec<PathBuf>, Vec<PathBuf>) = files.iter().cloned().partition(|f| matches!(io::format_of(f), Some(io::Format::Native)));
        for f in native {
            self.open_path(f);
        }
        if files.len() == 1 && self.tab_is_blank() && io::format_of(&files[0]).is_some() {
            self.open_path(files[0].clone());
            return;
        }
        let mut all = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let off = 24.0 * i as f64;
            self.place_path(f, Some(at + kurbo::Vec2::new(off, off)));
            all.extend(self.selection.iter().copied());
        }
        if !all.is_empty() {
            self.selection = all;
            self.set_tool(Tool::Select);
        }
    }

    pub fn place_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Vector & images", &["svg", "pdf", "ai", "eps", "png", "jpg", "jpeg", "webp", "gif", "bmp"])
            .pick_file()
        {
            self.place_path(&p, None);
        }
    }

    #[cfg(test)]
    pub fn apply_color(&mut self, c: crate::document::Rgba, to_stroke: bool) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in self.selection.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                if to_stroke {
                    let mut st = s.stroke.clone().unwrap_or(Stroke { width: 2.0, ..Default::default() });
                    st.color = c;
                    s.stroke = Some(st);
                    s.stroke_key = None;
                } else {
                    s.fill = Some(Paint::Solid(c));
                    s.fill_key = None;
                }
            }
        }
    }
}

impl App {
    /// Developer aid for headless screenshots: `MTZD_TOOL=node`, `MTZD_SELECT=2`,
    /// `MTZD_FONTS=google`, `MTZD_PICKER=1`, `MTZD_NODESEL=1`, `MTZD_TAB=design|colors|layers|components|icons|history`,
    /// `MTZD_PALETTE=query`, `MTZD_CTXMENU=x,y|center`, `MTZD_DROP=1` preset UI state.
    pub fn dev_setup(&mut self) {
        if let Ok(sel) = std::env::var("MTZD_SELECT") {
            if self.selection.is_empty() {
                if sel == "all" {
                    self.select_all();
                } else {
                    self.selection = sel.split(',').filter_map(|n| n.trim().parse::<usize>().ok()).filter_map(|n| self.doc.shapes().get(n).map(|s| s.id)).collect();
                }
            }
        }
        if let Ok(t) = std::env::var("MTZD_TOOL") {
            let tool = match t.as_str() {
                "node" => Tool::Node,
                "builder" => Tool::ShapeBuilder,
                "corner" => Tool::Corner,
                "text" => Tool::Text,
                "frametext" => Tool::FrameText,
                "star" => Tool::Star,
                "pen" => Tool::Pen,
                "fill" => Tool::Fill,
                p if p.starts_with("prim:") => crate::prims::PrimKind::ALL.iter().copied().find(|k| k.name().eq_ignore_ascii_case(&p[5..])).map(Tool::Prim).unwrap_or(Tool::Select),
                _ => Tool::Select,
            };
            if self.tool != tool {
                if tool == Tool::ShapeBuilder {
                    self.selection = self.doc.shapes().iter().map(|s| s.id).collect();
                }
                self.tool = tool;
            }
        }
        if let Ok(src) = std::env::var("MTZD_FONTS") {
            self.font_picker.open = true;
            if src == "google" {
                self.font_picker.source = FontSource::Google;
            }
        }
        if let Ok(n) = std::env::var("MTZD_NODESEL") {
            if self.node_sel.is_empty() && !self.selection.is_empty() {
                self.node_sel = vec![(self.selection[0], 0, n.parse().unwrap_or(0))];
            }
        }
        if std::env::var("MTZD_PICKER").is_ok() {
            self.ui.paint_open = Some("fill".into());
        }
        if let Ok(t) = std::env::var("MTZD_TAB") {
            use crate::ui::studio::{DESIGN, LAYERS};
            match t.as_str() {
                "design" => self.ui.tab[DESIGN] = 0,
                "colors" => self.ui.tab[DESIGN] = 1,
                "layers" => self.ui.tab[LAYERS] = 0,
                "components" => self.ui.tab[LAYERS] = 1,
                "icons" => self.ui.tab[LAYERS] = 2,
                "history" => self.ui.tab[LAYERS] = 3,
                _ => {}
            }
        }
        if let Ok(q) = std::env::var("MTZD_PALETTE") {
            if self.ui.palette.is_none() {
                self.ui.open_palette();
                if let Some(p) = &mut self.ui.palette {
                    p.query = q;
                }
            }
        }
        if let Ok(v) = std::env::var("MTZD_CTXMENU") {
            // "x,y" in canvas-relative points, or "center".
            if self.ui.ctx_menu.is_none() && self.view_ready {
                let r = self.canvas_rect;
                let pos = v
                    .split_once(',')
                    .and_then(|(a, b)| Some(r.min + egui::vec2(a.parse().ok()?, b.parse().ok()?)))
                    .unwrap_or(r.center());
                self.open_context_menu(pos, false);
            }
        }
        if std::env::var("MTZD_DROP").is_ok() && self.canvas_rect.width() > 10.0 {
            self.drop_hover = Some(self.canvas_rect.center());
        }
    }
}

