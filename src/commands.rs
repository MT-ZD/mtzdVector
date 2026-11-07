//! Command registry. Every user-facing action is a `Cmd` with a single source
//! of truth for its label, icon, shortcut(s), category, enablement and
//! behavior. Menus, toolbars, context menus, the quick-actions palette and the
//! keyboard dispatcher all read from here.

use egui::{Key, KeyboardShortcut, Modifiers};
use egui_phosphor::regular as icon;

use crate::app::{App, Tool};
use crate::boolean::BoolOp;
use crate::document::{ShapeKind, TextAlign};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Cmd {
    // File
    New,
    Open,
    StartScreen,
    Save,
    SaveAs,
    Place,
    Export,
    ExportAgain,
    ExportSlices,
    Print,
    CloseTab,
    NextTab,
    PrevTab,
    Quit,
    // Edit
    Undo,
    Redo,
    Cut,
    Copy,
    CopySvg,
    Paste,
    PasteHere,
    CopyStyle,
    PasteStyle,
    Duplicate,
    Delete,
    SelectAll,
    DeselectAll,
    Preferences,
    // Object
    ConvertToCurves,
    ExpandStroke,
    ExpandOutlines,
    FlipH,
    FlipV,
    ToggleHidden,
    ToggleLocked,
    Rename,
    ArtboardSettings,
    NewArtboard,
    DuplicateArtboard,
    DeleteArtboard,
    NextArtboard,
    PrevArtboard,
    ZoomArtboard,
    Group,
    Ungroup,
    CreateComponent,
    DetachInstance,
    ResetInstance,
    GoToMain,
    // Arrange
    BringToFront,
    BringForward,
    SendBackward,
    SendToBack,
    // Align
    AlignLeft,
    AlignHCenter,
    AlignRight,
    AlignTop,
    AlignVCenter,
    AlignBottom,
    DistributeH,
    DistributeV,
    // Boolean
    Union,
    Subtract,
    Intersect,
    Exclude,
    Flatten,
    MakeClip,
    ReleaseClip,
    AddAutoLayout,
    RemoveAutoLayout,
    MakeMask,
    MakeAlphaMask,
    ReleaseMask,
    BlendShapes,
    EnvelopeWarp,
    ExpandLive,
    ReleaseLive,
    TraceImage,
    // Path (node tool)
    NodesSmooth,
    NodesSharp,
    DeleteNodes,
    ToggleClosed,
    BreakAtNode,
    JoinNodes,
    ReversePath,
    SimplifyPath,
    OffsetPath,
    // Text
    TextAlignLeft,
    TextAlignCenter,
    TextAlignRight,
    ShowTypography,
    ConvertTextType,
    TextOnPath,
    LinkTextFrames,
    UnlinkTextFrames,
    ReleaseTextPath,
    // View
    ZoomIn,
    ZoomOut,
    ZoomFit,
    Zoom100,
    ZoomSelection,
    ToggleRulers,
    ToggleStudio,
    ToggleSnap,
    ToggleGuides,
    ToggleOutline,
    ToggleGrid,
    ClearGuides,
    // Help
    Palette,
    Shortcuts,
    About,
    // Tools
    Tool(Tool),
    /// Set the selection's opacity to n × 10% (0 = 100%).
    Opacity(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    File,
    Edit,
    Object,
    Arrange,
    Align,
    Boolean,
    Path,
    Text,
    View,
    Tools,
    Help,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::File => "File",
            Category::Edit => "Edit",
            Category::Object => "Object",
            Category::Arrange => "Arrange",
            Category::Align => "Align",
            Category::Boolean => "Boolean",
            Category::Path => "Path",
            Category::Text => "Text",
            Category::View => "View",
            Category::Tools => "Tool",
            Category::Help => "Help",
        }
    }
}

/// When a shortcut may fire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// Always (Save, Open, palette…).
    Global,
    /// Not while an egui text field has keyboard focus.
    NoTextField,
    /// Not while typing in a text field nor while editing text on the canvas.
    Canvas,
}

pub struct Meta {
    pub label: &'static str,
    pub icon: &'static str,
    pub keys: &'static [KeyboardShortcut],
    pub category: Category,
    pub scope: Scope,
}

const NONE: Modifiers = Modifiers::NONE;
const CMD: Modifiers = Modifiers::COMMAND;
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::COMMAND };
const CMD_ALT: Modifiers = Modifiers { alt: true, ..Modifiers::COMMAND };
const SHIFT: Modifiers = Modifiers::SHIFT;
const ALT: Modifiers = Modifiers::ALT;
const ALT_SHIFT: Modifiers = Modifiers { alt: true, shift: true, ctrl: false, mac_cmd: false, command: false };

macro_rules! ks {
    ($m:expr, $k:ident) => {
        KeyboardShortcut { modifiers: $m, logical_key: Key::$k }
    };
}

impl Cmd {
    pub const TOOLS: [Cmd; 19] = [
        Cmd::Tool(Tool::Select),
        Cmd::Tool(Tool::Node),
        Cmd::Tool(Tool::Pen),
        Cmd::Tool(Tool::Corner),
        Cmd::Tool(Tool::Knife),
        Cmd::Tool(Tool::Pencil),
        Cmd::Tool(Tool::Brush),
        Cmd::Tool(Tool::Rect),
        Cmd::Tool(Tool::Ellipse),
        Cmd::Tool(Tool::Star),
        Cmd::Tool(Tool::Polygon),
        Cmd::Tool(Tool::Fill),
        Cmd::Tool(Tool::Text),
        Cmd::Tool(Tool::FrameText),
        Cmd::Tool(Tool::ShapeBuilder),
        Cmd::Tool(Tool::Eyedropper),
        Cmd::Tool(Tool::Zoom),
        Cmd::Tool(Tool::Hand),
        Cmd::Tool(Tool::Artboard),
    ];

    /// Every command, in menu order (used by the palette, shortcut table and tests).
    pub fn all() -> Vec<Cmd> {
        use Cmd::*;
        let mut v = vec![
            New, Open, StartScreen, Save, SaveAs, Place, Export, ExportAgain, ExportSlices, Print, CloseTab, NextTab, PrevTab, Quit, Undo, Redo, Cut, Copy, CopySvg, Paste, PasteHere,
            CopyStyle, PasteStyle, Duplicate, Delete, SelectAll, DeselectAll, Preferences, ConvertToCurves, ExpandStroke, ExpandOutlines, FlipH, FlipV, ToggleHidden,
            ToggleLocked, Rename, ArtboardSettings, NewArtboard, DuplicateArtboard, DeleteArtboard, NextArtboard, PrevArtboard, ZoomArtboard, Group, Ungroup, CreateComponent, DetachInstance, ResetInstance, GoToMain, BringToFront, BringForward, SendBackward, SendToBack, AlignLeft, AlignHCenter,
            AlignRight, AlignTop, AlignVCenter, AlignBottom, DistributeH, DistributeV, Union, Subtract, Intersect, Exclude, Flatten, MakeClip, ReleaseClip, AddAutoLayout, RemoveAutoLayout, MakeMask, MakeAlphaMask, ReleaseMask, BlendShapes, EnvelopeWarp, ExpandLive, ReleaseLive, TraceImage, NodesSmooth,
            NodesSharp, DeleteNodes, ToggleClosed, BreakAtNode, JoinNodes, ReversePath, SimplifyPath, OffsetPath, TextAlignLeft, TextAlignCenter, TextAlignRight, ShowTypography, ConvertTextType, TextOnPath, ReleaseTextPath, LinkTextFrames, UnlinkTextFrames, ZoomIn,
            ZoomOut, ZoomFit, Zoom100, ZoomSelection, ToggleRulers, ToggleStudio, ToggleSnap, ToggleGuides, ToggleOutline, ToggleGrid, ClearGuides, Palette, Shortcuts, About,
        ];
        v.extend(Cmd::TOOLS);
        v.extend((0..10).map(Cmd::Opacity));
        v
    }

/// Other names people search for a command by (Ctrl+K): terms from other
    /// apps and plain descriptions, comma-separated.
    pub fn aliases(self) -> &'static str {
        use Cmd::*;
        match self {
            New => "create, blank, new file, canvas",
            StartScreen => "templates, recent files, welcome, start",
            Open => "load, browse, import file",
            Save => "store, write",
            SaveAs => "save copy, rename file",
            Place => "import, insert image, picture, photo, embed, link",
            Export => "save as png, svg, pdf, jpg, webp, render, output, publish",
            ExportAgain => "repeat export, re-export",
            ExportSlices => "slices, assets, batch export",
            Print => "printer, paper, hard copy",
            CloseTab => "close file, close tab",
            Quit => "exit, close app",
            Preferences => "settings, options, configure, setup",
            Undo => "revert, back, step back",
            Redo => "repeat, forward",
            Cut => "clipboard",
            Copy => "clipboard",
            CopySvg => "clipboard svg, copy code",
            Paste => "clipboard, insert",
            PasteHere => "paste in place, paste at cursor",
            CopyStyle => "copy attributes, copy appearance, format painter",
            PasteStyle => "paste attributes, paste appearance, format painter",
            Duplicate => "clone, copy in place",
            Delete => "remove, erase, trash, clear",
            SelectAll => "everything",
            DeselectAll => "select none, clear selection",
            ConvertToCurves => "outline, convert to path, create outlines, expand, editable path, vectorize text",
            ExpandStroke => "outline stroke, stroke to path, convert stroke",
            ExpandOutlines => "expand effect, outline to path",
            FlipH => "mirror, reflect horizontal",
            FlipV => "mirror, reflect vertical",
            ToggleHidden => "visibility, hide, show, invisible",
            ToggleLocked => "lock, unlock, freeze, protect",
            Rename => "name, label",
            ArtboardSettings => "page setup, canvas size, document size, resize artboard",
            NewArtboard => "add page, new page, frame",
            DuplicateArtboard => "copy page, clone page",
            DeleteArtboard => "remove page",
            NextArtboard | PrevArtboard => "page",
            ZoomArtboard => "fit page, show page",
            Group => "combine, bundle, folder, nest",
            Ungroup => "split, separate, dissolve, release group",
            CreateComponent => "symbol, master, reusable, main component",
            DetachInstance => "unlink symbol, break link",
            ResetInstance => "revert symbol, restore",
            GoToMain => "symbol, master, edit component",
            BringToFront => "arrange, order, z-order, top, raise",
            BringForward => "arrange, order, z-order, up, raise",
            SendBackward => "arrange, order, z-order, down, lower",
            SendToBack => "arrange, order, z-order, bottom, lower",
            AlignLeft | AlignHCenter | AlignRight | AlignTop | AlignVCenter | AlignBottom => "arrange, line up, position, center, centre",
            DistributeH | DistributeV => "space evenly, spacing, arrange",
            Union => "add, merge, combine, weld, unite, boolean, pathfinder",
            Subtract => "minus, cut out, punch, difference, front, boolean, pathfinder",
            Intersect => "overlap, common, crop, boolean, pathfinder",
            Exclude => "xor, difference, divide, boolean, pathfinder",
            Flatten => "merge, expand, combine paths, bake",
            MakeClip => "clipping mask, crop, clip, mask",
            ReleaseClip => "unclip, release clipping mask",
            AddAutoLayout => "stack, flex, flexbox, row, column, spacing, padding",
            RemoveAutoLayout => "stack, flex, unstack",
            MakeMask | MakeAlphaMask => "clipping, transparency, opacity mask, luminance, hide",
            ReleaseMask => "unmask",
            BlendShapes => "morph, tween, interpolate, transition, steps",
            EnvelopeWarp => "distort, deform, bend, arc, arch, bulge, wave, flag, mesh warp, transform, perspective, inflate, twist",
            ExpandLive => "bake, apply, flatten warp, flatten blend",
            ReleaseLive => "undo warp, undo blend, remove warp, remove blend",
            TraceImage => "vectorize, vectorise, image trace, live trace, bitmap to vector, convert image, auto trace",
            NodesSmooth => "points, anchors, curve, symmetric",
            NodesSharp => "points, anchors, corner, cusp",
            DeleteNodes => "remove points, remove anchors",
            ToggleClosed => "close path, open path, connect ends",
            BreakAtNode => "split path, cut path, scissors",
            JoinNodes => "connect, merge points, weld points",
            ReversePath => "direction, flip path",
            SimplifyPath => "smooth, reduce points, clean up, optimize",
            OffsetPath => "inset, outset, grow, shrink, expand, contour, outline",
            TextAlignLeft | TextAlignCenter | TextAlignRight => "justify, paragraph, centre",
            ConvertTextType => "text box, area text, point text, frame text",
            TextOnPath => "type on path, curved text, circle text",
            LinkTextFrames => "thread, flow, overflow, connect text",
            UnlinkTextFrames => "unthread",
            ReleaseTextPath => "straighten text",
            ShowTypography => "font, typeface, character, kerning, tracking, leading, opentype, ligatures",
            ZoomIn | ZoomOut => "magnify, scale view, enlarge",
            ZoomFit => "fit, show all, overview",
            Zoom100 => "actual pixels, 1:1, real size",
            ZoomSelection => "focus, frame selection",
            ToggleRulers => "measure, ruler",
            ToggleStudio => "panels, sidebar, inspector, hide ui",
            ToggleSnap => "magnet, align, snap",
            ToggleGuides => "helper lines",
            ToggleOutline => "wireframe, outline mode",
            ToggleGrid => "pixel grid, layout grid",
            ClearGuides => "remove guides",
            Palette => "command palette, search",
            Shortcuts => "hotkeys, keybindings, keys",
            About => "version, info, credits",
            Tool(t) => {
                use crate::app::Tool as T;
                match t {
                    T::Select => "move, pointer, arrow, selection, transform, shear, skew, slant, rotate, scale, resize",
                    T::Node => "direct selection, edit points, anchors, bezier, vertex",
                    T::Pen => "bezier, path, draw, curve, anchor",
                    T::Rect => "rectangle, square, box",
                    T::Ellipse => "circle, oval, round",
                    T::Star => "burst, sparkle",
                    T::Polygon => "hexagon, pentagon, triangle, sides",
                    T::Prim(_) => "shape, primitive",
                    T::Text => "type, font, label, artistic text, write",
                    T::FrameText => "text box, paragraph, area text",
                    T::Fill => "gradient, paint bucket, colour, color, bucket",
                    T::ShapeBuilder => "pathfinder, merge, combine, boolean, regions",
                    T::Corner => "round corners, radius, fillet, chamfer",
                    T::Knife => "cut, slice, split, scissors",
                    T::Pencil => "freehand, draw, sketch",
                    T::Brush => "paint, freehand, draw, stroke, calligraphy, marker, pressure",
                    T::Eyedropper => "colour picker, color picker, sample, pipette, picker",
                    T::Zoom => "magnify, magnifier",
                    T::Hand => "pan, scroll, move view",
                    T::Artboard => "page, frame, canvas",
                }
            }
            Opacity(_) => "transparency, alpha, fade",
            _ => "",
        }
    }

    pub fn meta(self) -> Meta {
        use Category as C;
        use Cmd::*;
        use Scope::*;
        let m = |label, icon, keys, category, scope| Meta { label, icon, keys, category, scope };
        match self {
            New => m("New Document", icon::FILE_PLUS, &[ks!(CMD, N)], C::File, Global),
            StartScreen => m("New from Template / Recent…", icon::SQUARES_FOUR, &[ks!(CMD_SHIFT, N)], C::File, Global),
            Open => m("Open…", icon::FOLDER_OPEN, &[ks!(CMD, O)], C::File, Global),
            Save => m("Save", icon::FLOPPY_DISK, &[ks!(CMD, S)], C::File, Global),
            SaveAs => m("Save As…", icon::FLOPPY_DISK_BACK, &[ks!(CMD_SHIFT, S)], C::File, Global),
            ExportSlices => m("Export Slices…", icon::STACK_SIMPLE, &[], C::File, Global),
            Print => m("Print…", icon::PRINTER, &[ks!(CMD, P)], C::File, Global),
            Place => m("Place File or Image…", icon::IMAGE_SQUARE, &[ks!(CMD_SHIFT, P)], C::File, Global),
            Export => m("Export…", icon::EXPORT, &[ks!(CMD, E)], C::File, Global),
            ExportAgain => m("Export Again", icon::ARROW_CLOCKWISE, &[ks!(CMD_SHIFT, E)], C::File, Global),
            CloseTab => m("Close Document", icon::X, &[ks!(CMD, W)], C::File, Global),
            NextTab => m("Next Document Tab", icon::ARROW_RIGHT, &[ks!(CMD, Tab), ks!(CMD, PageDown)], C::File, Global),
            PrevTab => m("Previous Document Tab", icon::ARROW_LEFT, &[ks!(CMD_SHIFT, Tab), ks!(CMD, PageUp)], C::File, Global),
            Quit => m("Quit", icon::SIGN_OUT, &[ks!(CMD, Q)], C::File, Global),

            Preferences => m("Preferences…", icon::GEAR_SIX, &[ks!(CMD, Comma)], C::Edit, Global),
            Undo => m("Undo", icon::ARROW_COUNTER_CLOCKWISE, &[ks!(CMD, Z)], C::Edit, NoTextField),
            Redo => m("Redo", icon::ARROW_CLOCKWISE, &[ks!(CMD_SHIFT, Z), ks!(CMD, Y)], C::Edit, NoTextField),
            Cut => m("Cut", icon::SCISSORS, &[ks!(CMD, X)], C::Edit, Canvas),
            Copy => m("Copy", icon::COPY, &[ks!(CMD, C)], C::Edit, Canvas),
            CopySvg => m("Copy as SVG", icon::FILE_SVG, &[ks!(CMD_SHIFT, C)], C::Edit, Canvas),
            Paste => m("Paste", icon::CLIPBOARD_TEXT, &[ks!(CMD, V)], C::Edit, Canvas),
            PasteHere => m("Paste Here", icon::CLIPBOARD, &[ks!(CMD_SHIFT, V)], C::Edit, Canvas),
            CopyStyle => m("Copy Style", icon::PAINT_BRUSH, &[ks!(CMD_ALT, C)], C::Edit, Canvas),
            PasteStyle => m("Paste Style", icon::PAINT_BRUSH_BROAD, &[ks!(CMD_ALT, V)], C::Edit, Canvas),
            Duplicate => m("Duplicate", icon::COPY_SIMPLE, &[ks!(CMD, D)], C::Edit, Canvas),
            Delete => m("Delete", icon::TRASH, &[ks!(NONE, Delete), ks!(NONE, Backspace)], C::Edit, Canvas),
            SelectAll => m("Select All", icon::SELECTION_ALL, &[ks!(CMD, A)], C::Edit, Canvas),
            DeselectAll => m("Deselect", icon::SELECTION_SLASH, &[ks!(CMD_SHIFT, A)], C::Edit, Canvas),

            ConvertToCurves => m("Convert to Curves", icon::BEZIER_CURVE, &[ks!(CMD, Enter)], C::Object, Canvas),
            ExpandStroke => m("Expand Stroke", icon::PATH, &[ks!(CMD_ALT, Enter)], C::Object, Canvas),
            ExpandOutlines => m("Expand Outline Effect", icon::CIRCLES_THREE, &[], C::Object, Canvas),
            FlipH => m("Flip Horizontal", icon::FLIP_HORIZONTAL, &[ks!(SHIFT, H)], C::Object, Canvas),
            FlipV => m("Flip Vertical", icon::FLIP_VERTICAL, &[ks!(SHIFT, V)], C::Object, Canvas),
            ToggleHidden => m("Hide / Show", icon::EYE_SLASH, &[ks!(CMD_SHIFT, H)], C::Object, Canvas),
            ToggleLocked => m("Lock / Unlock", icon::LOCK_SIMPLE, &[ks!(CMD_SHIFT, L)], C::Object, Canvas),
            Rename => m("Rename…", icon::PENCIL_SIMPLE, &[ks!(NONE, F2)], C::Object, Canvas),
            ArtboardSettings => m("Artboard Settings…", icon::FRAME_CORNERS, &[], C::Object, Global),
            NewArtboard => m("New Artboard", icon::PLUS_SQUARE, &[], C::Object, Global),
            DuplicateArtboard => m("Duplicate Artboard", icon::COPY, &[], C::Object, Global),
            DeleteArtboard => m("Delete Artboard", icon::TRASH, &[], C::Object, Global),
            NextArtboard => m("Next Artboard", icon::CARET_RIGHT, &[ks!(NONE, PageDown)], C::View, NoTextField),
            PrevArtboard => m("Previous Artboard", icon::CARET_LEFT, &[ks!(NONE, PageUp)], C::View, NoTextField),
            ZoomArtboard => m("Zoom to Artboard", icon::FRAME_CORNERS, &[ks!(CMD_SHIFT, Num0)], C::View, NoTextField),
            Group => m("Group", icon::FOLDER_SIMPLE, &[ks!(CMD, G)], C::Object, Canvas),
            Ungroup => m("Ungroup", icon::FOLDER_OPEN, &[ks!(CMD_SHIFT, G)], C::Object, Canvas),
            CreateComponent => m("Create Component", icon::DIAMONDS_FOUR, &[ks!(CMD_ALT, K)], C::Object, Canvas),
            DetachInstance => m("Detach Instance", icon::LINK_BREAK, &[ks!(CMD_ALT, B)], C::Object, Canvas),
            ResetInstance => m("Reset Instance Overrides", icon::ARROW_COUNTER_CLOCKWISE, &[], C::Object, Canvas),
            GoToMain => m("Go to Main Component", icon::ARROW_SQUARE_OUT, &[], C::Object, Canvas),

            BringToFront => m("Bring to Front", icon::STACK_PLUS, &[ks!(CMD_SHIFT, CloseBracket)], C::Arrange, Canvas),
            BringForward => m("Bring Forward", icon::ARROW_UP, &[ks!(CMD, CloseBracket)], C::Arrange, Canvas),
            SendBackward => m("Send Backward", icon::ARROW_DOWN, &[ks!(CMD, OpenBracket)], C::Arrange, Canvas),
            SendToBack => m("Send to Back", icon::STACK_MINUS, &[ks!(CMD_SHIFT, OpenBracket)], C::Arrange, Canvas),

            AlignLeft => m("Align Left", icon::ALIGN_LEFT, &[ks!(ALT, A)], C::Align, Canvas),
            AlignHCenter => m("Align Horizontal Centers", icon::ALIGN_CENTER_HORIZONTAL, &[ks!(ALT, H)], C::Align, Canvas),
            AlignRight => m("Align Right", icon::ALIGN_RIGHT, &[ks!(ALT, D)], C::Align, Canvas),
            AlignTop => m("Align Top", icon::ALIGN_TOP, &[ks!(ALT, W)], C::Align, Canvas),
            AlignVCenter => m("Align Vertical Centers", icon::ALIGN_CENTER_VERTICAL, &[ks!(ALT, V)], C::Align, Canvas),
            AlignBottom => m("Align Bottom", icon::ALIGN_BOTTOM, &[ks!(ALT, S)], C::Align, Canvas),
            DistributeH => m("Distribute Horizontally", icon::COLUMNS, &[ks!(ALT_SHIFT, H)], C::Align, Canvas),
            DistributeV => m("Distribute Vertically", icon::ROWS, &[ks!(ALT_SHIFT, V)], C::Align, Canvas),

            Union => m("Union", icon::UNITE, &[ks!(ALT_SHIFT, U)], C::Boolean, Canvas),
            Subtract => m("Subtract", icon::SUBTRACT, &[ks!(ALT_SHIFT, S)], C::Boolean, Canvas),
            Intersect => m("Intersect", icon::INTERSECT, &[ks!(ALT_SHIFT, I)], C::Boolean, Canvas),
            Exclude => m("Exclude", icon::EXCLUDE, &[ks!(ALT_SHIFT, X)], C::Boolean, Canvas),
            Flatten => m("Flatten", icon::BEZIER_CURVE, &[ks!(CMD_ALT, E)], C::Boolean, Canvas),

            MakeClip => m("Clip to Top Shape", icon::CROP, &[ks!(CMD, Num7)], C::Object, Canvas),
            ReleaseClip => m("Release Clip", icon::LINK_BREAK, &[ks!(CMD_ALT, Num7)], C::Object, Canvas),
            AddAutoLayout => m("Add Auto Layout", icon::COLUMNS, &[ks!(SHIFT, A)], C::Object, Canvas),
            RemoveAutoLayout => m("Remove Auto Layout", icon::SQUARES_FOUR, &[], C::Object, Canvas),
            MakeMask => m("Mask with Top Shape (Luminance)", icon::CIRCLE_HALF, &[], C::Object, Canvas),
            MakeAlphaMask => m("Mask with Top Shape (Opacity)", icon::DROP_HALF, &[], C::Object, Canvas),
            ReleaseMask => m("Release Mask", icon::LINK_BREAK, &[], C::Object, Canvas),
            BlendShapes => m("Blend Shapes…", icon::CIRCLES_THREE, &[], C::Object, Canvas),
            EnvelopeWarp => m("Envelope Warp…", icon::FLAG, &[], C::Object, Canvas),
            ExpandLive => m("Expand Warp or Blend", icon::ARROWS_OUT, &[], C::Object, Canvas),
            ReleaseLive => m("Release Warp or Blend", icon::ARROW_COUNTER_CLOCKWISE, &[], C::Object, Canvas),
            TraceImage => m("Trace Image…", icon::SCAN, &[], C::Object, Canvas),

            NodesSmooth => m("Smooth Nodes", icon::BEZIER_CURVE, &[], C::Path, Canvas),
            NodesSharp => m("Sharp Nodes", icon::POLYGON, &[], C::Path, Canvas),
            DeleteNodes => m("Delete Nodes", icon::TRASH, &[], C::Path, Canvas),
            ToggleClosed => m("Close / Open Path", icon::CIRCLE_DASHED, &[], C::Path, Canvas),
            BreakAtNode => m("Break at Node", icon::SCISSORS, &[], C::Path, Canvas),
            JoinNodes => m("Join Nodes", icon::LINK_SIMPLE, &[ks!(CMD, J)], C::Path, Canvas),
            ReversePath => m("Reverse Path", icon::ARROWS_COUNTER_CLOCKWISE, &[], C::Path, Canvas),
            SimplifyPath => m("Simplify Path", icon::WAVE_SINE, &[], C::Path, Canvas),
            OffsetPath => m("Offset Path…", icon::ARROWS_OUT_SIMPLE, &[], C::Path, Canvas),

            TextAlignLeft => m("Align Text Left", icon::TEXT_ALIGN_LEFT, &[], C::Text, Canvas),
            TextAlignCenter => m("Align Text Center", icon::TEXT_ALIGN_CENTER, &[], C::Text, Canvas),
            TextAlignRight => m("Align Text Right", icon::TEXT_ALIGN_RIGHT, &[], C::Text, Canvas),
            ConvertTextType => m("Convert Artistic ↔ Frame Text", icon::TEXTBOX, &[], C::Text, Canvas),
            TextOnPath => m("Put Text on Path", icon::PATH, &[], C::Text, Canvas),
            LinkTextFrames => m("Link Text Frames", icon::LINK, &[], C::Text, Canvas),
            UnlinkTextFrames => m("Unlink Text Frames", icon::LINK_BREAK, &[], C::Text, Canvas),
            ReleaseTextPath => m("Remove Text from Path", icon::TEXT_T, &[], C::Text, Canvas),
            ShowTypography => m("Typography Panel", icon::TEXT_AA, &[ks!(CMD, T)], C::Text, Global),

            ZoomIn => m("Zoom In", icon::MAGNIFYING_GLASS_PLUS, &[ks!(CMD, Plus), ks!(CMD, Equals)], C::View, NoTextField),
            ZoomOut => m("Zoom Out", icon::MAGNIFYING_GLASS_MINUS, &[ks!(CMD, Minus)], C::View, NoTextField),
            ZoomFit => m("Zoom to Fit", icon::ARROWS_OUT, &[ks!(CMD, Num0)], C::View, NoTextField),
            Zoom100 => m("Actual Size (100%)", icon::NUMBER_SQUARE_ONE, &[ks!(CMD, Num1)], C::View, NoTextField),
            ZoomSelection => m("Zoom to Selection", icon::FRAME_CORNERS, &[ks!(CMD, Num2)], C::View, NoTextField),
            ToggleRulers => m("Rulers", icon::RULER, &[ks!(CMD_SHIFT, R)], C::View, NoTextField),
            ToggleStudio => m("Studio Panels", icon::SIDEBAR_SIMPLE, &[ks!(CMD, Backslash)], C::View, NoTextField),
            ToggleSnap => m("Snapping", icon::MAGNET, &[ks!(CMD_SHIFT, Semicolon)], C::View, NoTextField),
            ToggleGuides => m("Guides", icon::CROSSHAIR_SIMPLE, &[ks!(CMD, Semicolon)], C::View, NoTextField),
            ToggleOutline => m("Outline View", icon::SELECTION, &[ks!(CMD_ALT, Y)], C::View, NoTextField),
            ToggleGrid => m("Grid", icon::GRID_FOUR, &[ks!(CMD, Quote)], C::View, NoTextField),
            ClearGuides => m("Clear Guides", icon::BROOM, &[], C::View, NoTextField),

            Palette => m("Quick Actions…", icon::LIGHTNING, &[ks!(CMD, K)], C::Help, Global),
            Shortcuts => m("Keyboard Shortcuts", icon::KEYBOARD, &[ks!(CMD, Slash)], C::Help, Global),
            About => m("About mtzdVector", icon::INFO, &[], C::Help, Global),

            Opacity(n) => {
                const LABELS: [&str; 10] = ["Opacity 100%", "Opacity 10%", "Opacity 20%", "Opacity 30%", "Opacity 40%", "Opacity 50%", "Opacity 60%", "Opacity 70%", "Opacity 80%", "Opacity 90%"];
                const KEYS: [[KeyboardShortcut; 1]; 10] = [[ks!(NONE, Num0)], [ks!(NONE, Num1)], [ks!(NONE, Num2)], [ks!(NONE, Num3)], [ks!(NONE, Num4)], [ks!(NONE, Num5)], [ks!(NONE, Num6)], [ks!(NONE, Num7)], [ks!(NONE, Num8)], [ks!(NONE, Num9)]];
                m(LABELS[n as usize % 10], icon::DROP_HALF, &KEYS[n as usize % 10], C::Object, Canvas)
            }
            Tool(t) => {
                let keys: &'static [KeyboardShortcut] = match t {
                    crate::app::Tool::Select => &[ks!(NONE, V)],
                    crate::app::Tool::Node => &[ks!(NONE, A)],
                    crate::app::Tool::Pen => &[ks!(NONE, P)],
                    crate::app::Tool::Rect => &[ks!(NONE, R)],
                    crate::app::Tool::Ellipse => &[ks!(NONE, E), ks!(NONE, O)],
                    crate::app::Tool::Star => &[ks!(NONE, S)],
                    crate::app::Tool::Polygon => &[ks!(SHIFT, S)],
                    crate::app::Tool::Fill => &[ks!(NONE, G)],
                    crate::app::Tool::Prim(_) => &[],
                    crate::app::Tool::Text => &[ks!(NONE, T)],
                    crate::app::Tool::FrameText => &[ks!(SHIFT, T)],
                    crate::app::Tool::ShapeBuilder => &[ks!(NONE, M)],
                    crate::app::Tool::Corner => &[ks!(NONE, C)],
                    crate::app::Tool::Knife => &[ks!(NONE, K)],
                    crate::app::Tool::Pencil => &[ks!(NONE, N)],
                    crate::app::Tool::Brush => &[ks!(NONE, B)],
                    crate::app::Tool::Eyedropper => &[ks!(NONE, I)],
                    crate::app::Tool::Zoom => &[ks!(NONE, Z)],
                    crate::app::Tool::Hand => &[ks!(NONE, H)],
                    crate::app::Tool::Artboard => &[ks!(NONE, F)],
                };
                m(t.name(), t.icon(), keys, C::Tools, Canvas)
            }
        }
    }

    pub fn label(self) -> &'static str {
        self.meta().label
    }

    pub fn icon(self) -> &'static str {
        self.meta().icon
    }

    /// Human-readable primary shortcut ("Ctrl+Shift+S"), if any.
    pub fn shortcut_text(self) -> Option<String> {
        self.meta().keys.first().map(format_shortcut)
    }

    pub fn enabled(self, app: &App) -> bool {
        use Cmd::*;
        let n = app.selection.len();
        let has = n > 0;
        let sel_shapes = || app.selection.iter().filter_map(|id| app.doc.get(*id));
        match self {
            Undo => app.history.can_undo(),
            NextTab | PrevTab => app.tab_count() > 1,
            ClearGuides => !app.doc.guides.is_empty(),
            Redo => app.history.can_redo(),
            Cut | Copy | CopySvg | CopyStyle | Duplicate | ConvertToCurves | FlipH | FlipV | ToggleHidden | ToggleLocked
            | BringToFront | BringForward | SendBackward | SendToBack | AlignLeft | AlignHCenter | AlignRight | AlignTop | AlignVCenter
            | AlignBottom | ZoomSelection | DeselectAll => has,
            Delete => has || app.pen.is_some() || (app.artboard_selected && app.doc.artboards.len() > 1) || (app.tool == crate::app::Tool::Node && !app.node_sel.is_empty()),
            Rename => n == 1,
            DeleteArtboard => app.doc.artboards.len() > 1,
            NextArtboard | PrevArtboard => app.doc.artboards.len() > 1,
            Opacity(_) => has,
            PasteStyle => has && app.style_clip.is_some(),
            SelectAll => !app.doc.shapes().is_empty(),
            DistributeH | DistributeV => n >= 3,
            Union | Subtract | Intersect | Exclude | MakeClip | MakeMask | MakeAlphaMask => n >= 2,
            ReleaseMask => sel_shapes().any(|s| s.mask.is_some()),
            AddAutoLayout => has && !(n == 1 && sel_shapes().any(|s| s.layout.is_some())),
            RemoveAutoLayout => sel_shapes().any(|s| s.layout.is_some()),
            EnvelopeWarp => has,
            BlendShapes => app.selected_bottom_up().len() == 2,
            ExpandLive | ReleaseLive => sel_shapes().any(|s| s.live.is_some()),
            TraceImage => n == 1,
            Flatten => n >= 2 || sel_shapes().any(|s| !matches!(s.kind, ShapeKind::Path(_))),
            Group => has,
            Ungroup => sel_shapes().any(|s| s.kind.is_container()),
            CreateComponent => has && !(n == 1 && sel_shapes().any(|s| s.component.is_some())),
            DetachInstance | GoToMain => app.has_instance_selected(),
            ResetInstance => app.has_instance_selected(),
            ReleaseClip => sel_shapes().any(|s| s.clip.is_some()),
            TextOnPath => n == 2 && sel_shapes().any(|s| matches!(s.kind, ShapeKind::Text(_))) && sel_shapes().any(|s| !matches!(s.kind, ShapeKind::Text(_))),
            LinkTextFrames => sel_shapes().filter(|s| matches!(&s.kind, ShapeKind::Text(t) if t.frame.is_some())).count() >= 2,
            UnlinkTextFrames => sel_shapes().any(|s| matches!(&s.kind, ShapeKind::Text(t) if t.next.is_some() || t.flow.is_some())),
            ReleaseTextPath => sel_shapes().any(|s| matches!(&s.kind, ShapeKind::Text(t) if t.path.is_some())),
            ExpandStroke => sel_shapes().any(|s| s.stroke.as_ref().map(|st| st.width > 0.0).unwrap_or(false)),
            ExpandOutlines => sel_shapes().any(|s| !s.outlines.is_empty()),
            NodesSmooth | NodesSharp | DeleteNodes => app.tool == crate::app::Tool::Node && !app.node_sel.is_empty(),
            BreakAtNode => app.tool == crate::app::Tool::Node && !app.node_sel.is_empty(),
            JoinNodes => app.tool == crate::app::Tool::Node && app.node_sel.len() == 2,
            ReversePath | SimplifyPath => sel_shapes().any(|s| matches!(s.kind, ShapeKind::Path(_))),
            OffsetPath => sel_shapes().any(|s| !s.kind.is_container() || matches!(s.kind, ShapeKind::Compound { .. })),
            ToggleClosed => sel_shapes().any(|s| matches!(s.kind, ShapeKind::Path(_))),
            TextAlignLeft | TextAlignCenter | TextAlignRight | ConvertTextType => sel_shapes().any(|s| matches!(s.kind, ShapeKind::Text(_))),
            _ => true,
        }
    }

    /// Whether the command is a toggle that is currently on (for check marks).
    pub fn checked(self, app: &App) -> Option<bool> {
        match self {
            Cmd::ToggleRulers => Some(app.ui.show_rulers),
            Cmd::ToggleStudio => Some(app.ui.show_studio),
            Cmd::ToggleSnap => Some(app.snap),
            Cmd::ToggleGuides => Some(app.ui.show_guides),
            Cmd::ToggleOutline => Some(app.ui.outline_view),
            Cmd::ToggleGrid => Some(app.ui.show_grid),
            Cmd::Tool(t) => Some(app.tool == t),
            _ => None,
        }
    }

    pub fn run(self, app: &mut App, ctx: &egui::Context) {
        use Cmd::*;
        if !self.enabled(app) {
            return;
        }
        match self {
            StartScreen => app.ui.welcome_open = true,
            New => app.guarded(crate::session::Guarded::New),
            Open => app.guarded(crate::session::Guarded::Open(None)),
            Save => app.save(false),
            SaveAs => app.save(true),
            Place => app.place_dialog(),
            Print => app.print_document(),
            ExportSlices => app.export_slices_dialog(None),
            Export => app.ui.export.open = true,
            ExportAgain => app.export_again(),
            CloseTab => app.request_close_tab(app.tab),
            NextTab => app.next_tab(1),
            PrevTab => app.next_tab(-1),
            Quit => app.guarded(crate::session::Guarded::Quit),
            Undo => app.undo(),
            Redo => app.redo(),
            Cut => {
                app.copy(ctx);
                app.delete_selection();
            }
            Copy => app.copy(ctx),
            CopySvg => app.copy_svg_only(ctx),
            Paste => app.paste(ctx, None),
            PasteHere => {
                let at = app.menu_anchor_doc.or(app.cursor_doc).or_else(|| Some(app.view_center_doc()));
                app.paste(ctx, at);
            }
            CopyStyle => app.copy_style(),
            PasteStyle => app.paste_style(),
            Duplicate => app.duplicate_selection(10.0),
            Delete => {
                if app.gradient_delete_stop() {
                    // Fill tool: removed the selected gradient stop.
                } else if app.pen.is_some() {
                    // While drawing: take back the last node.
                    app.pen_backspace();
                } else if app.tool == crate::app::Tool::Node && !app.node_sel.is_empty() {
                    app.delete_nodes();
                } else if app.selection.is_empty() && app.artboard_selected {
                    app.delete_artboard();
                } else {
                    app.delete_selection();
                }
            }
            SelectAll => app.select_all(),
            Preferences => app.ui.prefs.open = true,
            DeselectAll => {
                app.selection.clear();
                app.node_sel.clear();
            }
            ConvertToCurves => app.convert_to_curves(),
            ExpandStroke => app.expand_stroke(),
            ExpandOutlines => app.expand_outlines(),
            FlipH => app.flip(true),
            FlipV => app.flip(false),
            ToggleHidden => app.toggle_hidden(),
            ToggleLocked => app.toggle_locked(),
            Rename => {
                if let Some(id) = app.selection.first().copied() {
                    app.start_rename(id);
                    app.ui.show_studio = true;
                    app.ui.collapsed[crate::ui::studio::LAYERS] = false;
                    app.ui.tab[crate::ui::studio::LAYERS] = 0;
                }
            }
            ArtboardSettings => app.ui.artboard_open = true,
            NewArtboard => app.new_artboard(),
            DuplicateArtboard => app.duplicate_artboard(),
            DeleteArtboard => app.delete_artboard(),
            NextArtboard => app.step_artboard(1),
            PrevArtboard => app.step_artboard(-1),
            ZoomArtboard => {
                let id = app.active().id;
                app.zoom_to_artboard(id);
            }
            BringToFront => app.reorder(2),
            BringForward => app.reorder(1),
            SendBackward => app.reorder(-1),
            SendToBack => app.reorder(-2),
            AlignLeft => app.align(0),
            AlignHCenter => app.align(1),
            AlignRight => app.align(2),
            AlignTop => app.align(3),
            AlignVCenter => app.align(4),
            AlignBottom => app.align(5),
            DistributeH => app.distribute(true),
            DistributeV => app.distribute(false),
            Union => app.make_compound(BoolOp::Union),
            Subtract => app.make_compound(BoolOp::Subtract),
            Intersect => app.make_compound(BoolOp::Intersect),
            Exclude => app.make_compound(BoolOp::Exclude),
            Flatten => app.flatten(),
            Group => app.group_selection(),
            Ungroup => app.ungroup(),
            CreateComponent => app.create_component(),
            DetachInstance => app.detach_instance(),
            ResetInstance => app.reset_instance(),
            GoToMain => app.go_to_main(),
            MakeClip => app.make_clip(),
            ReleaseClip => app.release_clip(),
            AddAutoLayout => app.add_auto_layout(),
            RemoveAutoLayout => app.remove_auto_layout(),
            MakeMask => app.make_mask(crate::document::MaskMode::Luminance),
            MakeAlphaMask => app.make_mask(crate::document::MaskMode::Alpha),
            ReleaseMask => app.release_mask(),
            BlendShapes => app.begin_blend(),
            EnvelopeWarp => app.begin_warp(),
            ExpandLive | ReleaseLive => {
                let ids: Vec<u64> = app.selection.iter().copied().filter(|id| app.doc.get(*id).is_some_and(|s| s.live.is_some())).collect();
                let mut sel = Vec::new();
                for id in ids {
                    if matches!(self, ExpandLive) {
                        app.expand_live(id);
                    } else {
                        app.release_live(id);
                    }
                    sel.extend(app.selection.iter().copied());
                }
                app.selection = sel;
            }
            TraceImage => app.ui.trace.open = true,
            TextOnPath => app.text_on_path(),
            LinkTextFrames => {
                app.link_text_frames();
            }
            UnlinkTextFrames => app.unlink_text_frames(),
            ReleaseTextPath => app.release_text_path(),
            NodesSmooth => app.set_nodes_smooth(true),
            NodesSharp => app.set_nodes_smooth(false),
            DeleteNodes => app.delete_nodes(),
            ToggleClosed => app.toggle_closed(),
            BreakAtNode => app.break_at_nodes(),
            JoinNodes => {
                if let Err(e) = app.join_nodes() {
                    app.notify(e);
                }
            }
            ReversePath => app.reverse_paths(),
            SimplifyPath => app.simplify_paths(),
            OffsetPath => app.ui.offset.open = true,
            TextAlignLeft => app.set_text_align(TextAlign::Left),
            TextAlignCenter => app.set_text_align(TextAlign::Center),
            TextAlignRight => app.set_text_align(TextAlign::Right),
            ConvertTextType => app.convert_text_type(),
            ShowTypography => app.show_design(),
            ZoomIn => {
                let c = app.canvas_rect.center();
                app.zoom_at(1.25, c);
            }
            ZoomOut => {
                let c = app.canvas_rect.center();
                app.zoom_at(0.8, c);
            }
            ZoomFit => app.zoom_to_fit(),
            Zoom100 => {
                let c = app.canvas_rect.center();
                app.zoom_at(1.0 / app.view.zoom, c);
            }
            ZoomSelection => app.zoom_to_selection(),
            ToggleRulers => app.ui.show_rulers = !app.ui.show_rulers,
            ToggleStudio => app.ui.show_studio = !app.ui.show_studio,
            ToggleSnap => app.snap = !app.snap,
            ToggleGuides => app.ui.show_guides = !app.ui.show_guides,
            ToggleOutline => app.ui.outline_view = !app.ui.outline_view,
            ToggleGrid => app.ui.show_grid = !app.ui.show_grid,
            ClearGuides => {
                app.checkpoint("");
                app.doc.guides.clear();
                app.dirty = true;
            }
            Palette => app.ui.open_palette(),
            Shortcuts => app.ui.shortcuts_open = true,
            About => app.ui.about_open = true,
            Tool(t) => app.set_tool(t),
            Opacity(k) => app.set_opacity(if k == 0 { 1.0 } else { k as f32 / 10.0 }),
        }
    }
}

pub fn format_shortcut(k: &KeyboardShortcut) -> String {
    let m = k.modifiers;
    let mut s = String::new();
    if m.command || m.ctrl {
        s.push_str("Ctrl+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    if m.shift {
        s.push_str("Shift+");
    }
    let key = match k.logical_key {
        Key::Plus => "+",
        Key::Minus => "-",
        Key::Equals => "=",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Backslash => "\\",
        Key::Slash => "/",
        Key::Semicolon => ";",
        Key::Delete => "Del",
        Key::Backspace => "Backspace",
        Key::Enter => "Enter",
        k => return format!("{s}{}", k.symbol_or_name()),
    };
    s.push_str(key);
    s
}

// ---------------------------------------------------------------------------
// Keyboard dispatch
// ---------------------------------------------------------------------------

/// A key press seen this frame: logical key, physical key and modifiers.
#[derive(Clone, Copy, Debug)]
pub struct Press {
    pub key: Key,
    pub physical: Option<Key>,
    pub mods: Modifiers,
}

fn mods_match(want: Modifiers, got: Modifiers, key: Key) -> bool {
    let cmd = |m: Modifiers| m.command || m.ctrl;
    // Shift is needed to type some keys (e.g. "+") so ignore it there.
    let shift_ok = want.shift == got.shift || matches!(key, Key::Plus | Key::Minus | Key::Equals);
    cmd(want) == cmd(got) && want.alt == got.alt && shift_ok
}

/// Find the command bound to a key press.
pub fn lookup(p: &Press) -> Option<Cmd> {
    for c in Cmd::all() {
        for k in c.meta().keys {
            let key_ok = k.logical_key == p.key || Some(k.logical_key) == p.physical;
            if key_ok && mods_match(k.modifiers, p.mods, k.logical_key) {
                return Some(c);
            }
        }
    }
    None
}

/// The alias of `aliases` (comma-separated) that `query` names: one whose
/// words start with the query's words, in order ("dist" finds "distort",
/// "mesh w" finds "mesh warp").
pub fn alias_match<'a>(query: &str, aliases: &'a str) -> Option<&'a str> {
    let q: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    if q.is_empty() {
        return None;
    }
    aliases.split(',').map(str::trim).filter(|a| !a.is_empty()).find(|a| {
        let words: Vec<&str> = a.split(|c: char| c.is_whitespace() || c == '-').collect();
        let mut wi = 0;
        q.iter().all(|qw| {
            while wi < words.len() {
                wi += 1;
                if words[wi - 1].to_lowercase().starts_with(qw.as_str()) {
                    return true;
                }
            }
            false
        })
    })
}

/// Fuzzy subsequence match: returns a score (higher is better) if all query
/// characters appear in order.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, c) in t.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            score += 10;
            if last == Some(i.wrapping_sub(1)) {
                score += 15; // consecutive
            }
            if i == 0 || !t[i - 1].is_alphanumeric() {
                score += 20; // word start
            }
            last = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    Some(score - t.len() as i32 / 4)
}
