//! Laser settings: per colour layer (saved in the document) and for the
//! machine (saved with the app).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Follow the path (cut / score).
    Line,
    /// Hatch the area (engrave).
    Fill,
    /// Engrave a picture line by line (dithered).
    Image,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Line => "Line",
            Mode::Fill => "Fill",
            Mode::Image => "Image",
        }
    }
}

/// How an image's greys become dots (the laser only burns or doesn't).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dither {
    /// Darker than the threshold burns: logos, line art.
    Threshold,
    /// Error diffusion: photos.
    #[default]
    FloydSteinberg,
    /// Wider error diffusion: smoother photos, finer grain.
    Jarvis,
    /// A regular dot pattern (Bayer): even tones, a screen-print look.
    Ordered,
}

impl Dither {
    pub fn label(self) -> &'static str {
        match self {
            Dither::Threshold => "Threshold",
            Dither::FloydSteinberg => "Floyd–Steinberg",
            Dither::Jarvis => "Jarvis",
            Dither::Ordered => "Ordered",
        }
    }
}

/// How a Fill layer covers its area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillStyle {
    /// Parallel lines at the angle.
    #[default]
    Scan,
    /// Lines at the angle and across it (more even on wood).
    CrossHatch,
    /// Rings following the outline inwards (no stair-stepping on curves).
    Offset,
}

impl FillStyle {
    pub fn label(self) -> &'static str {
        match self {
            FillStyle::Scan => "Scan",
            FillStyle::CrossHatch => "Cross-hatch",
            FillStyle::Offset => "Offset",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerSettings {
    pub mode: Mode,
    /// Sent to the laser (off: kept, but skipped).
    pub output: bool,
    /// mm/s.
    pub speed: f64,
    /// Percent.
    pub min_power: f64,
    pub max_power: f64,
    pub passes: u32,
    pub air: bool,
    /// Fill: line spacing (mm), angle (degrees), alternate directions.
    pub interval: f64,
    pub angle: f64,
    pub bidirectional: bool,
    pub fill_style: FillStyle,
    /// Degrees added to the angle on each pass (scan / cross-hatch).
    pub angle_step: f64,
    /// Fill each shape on its own (less travel between far-apart shapes)
    /// rather than sweeping across all of them.
    pub one_by_one: bool,
    /// Image: dithering, threshold (Threshold: brightness 0–100 below which
    /// it burns), brightness / contrast adjustments (−100…100), negative.
    pub dither: Dither,
    pub threshold: f64,
    pub brightness: f64,
    pub contrast: f64,
    pub invert: bool,
}

impl Default for LayerSettings {
    fn default() -> Self {
        LayerSettings::line()
    }
}

impl LayerSettings {
    pub fn line() -> Self {
        LayerSettings { mode: Mode::Line, output: true, speed: 20.0, min_power: 30.0, max_power: 40.0, passes: 1, air: true, interval: 0.1, angle: 0.0, bidirectional: true, fill_style: FillStyle::Scan, angle_step: 0.0, one_by_one: false, dither: Dither::FloydSteinberg, threshold: 50.0, brightness: 0.0, contrast: 0.0, invert: false }
    }

    pub fn image() -> Self {
        LayerSettings { mode: Mode::Image, speed: 120.0, min_power: 12.0, max_power: 18.0, air: false, ..LayerSettings::line() }
    }

    pub fn defaults_for(mode: Mode) -> Self {
        match mode {
            Mode::Line => LayerSettings::line(),
            Mode::Fill => LayerSettings::fill(),
            Mode::Image => LayerSettings::image(),
        }
    }

    pub fn fill() -> Self {
        LayerSettings { mode: Mode::Fill, speed: 150.0, min_power: 12.0, max_power: 18.0, air: false, ..LayerSettings::line() }
    }
}

/// A saved cut setting ("4 mm plywood – cut"), applied to a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub settings: LayerSettings,
}

/// What the document keeps (`plugin_data["laser"]`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocSettings {
    /// By colour ("#RRGGBB").
    pub layers: BTreeMap<String, LayerSettings>,
}

/// Where the machine's home (0, 0) is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Where the job goes on the bed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    /// The artboard's top-left at the bed's top-left.
    Absolute,
    /// The job's anchor at the head's position.
    CurrentPosition,
    /// The job's anchor at a point set on the machine (Move › Set origin).
    UserOrigin,
}

impl Placement {
    pub fn label(self) -> &'static str {
        match self {
            Placement::Absolute => "Absolute coords",
            Placement::CurrentPosition => "Current position",
            Placement::UserOrigin => "User origin",
        }
    }
}

/// What's cut: the artboard's shapes, or only the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Area {
    Artboard,
    Selection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MachineSettings {
    /// Serial port path ("emulator" for the built-in emulator).
    pub port: String,
    /// Bed size, mm.
    pub bed_w: f64,
    pub bed_h: f64,
    pub origin: Corner,
    /// No layer goes above this (percent).
    pub power_cap: f64,
    pub placement: Placement,
    /// Anchor of the job for `CurrentPosition`: (column, row) 0..=2.
    pub anchor: (u8, u8),
    pub area: Area,
    /// Jog step, mm.
    pub jog: f64,
    /// Rapid speed, for the estimate (mm/s).
    pub travel_speed: f64,
    /// Swizzle magic (0x88 for the RDC644x).
    pub magic: u8,
    /// Use the red pointer when framing.
    pub frame_light: bool,
    /// The cut settings library.
    pub presets: Vec<Preset>,
    /// The user origin, bed mm (x right, y down from the bed's top-left).
    pub user_origin: Option<(f64, f64)>,
}

impl Default for MachineSettings {
    fn default() -> Self {
        MachineSettings {
            port: String::new(),
            bed_w: 600.0,
            bed_h: 400.0,
            origin: Corner::TopRight,
            power_cap: 70.0,
            placement: Placement::Absolute,
            anchor: (0, 0),
            area: Area::Artboard,
            jog: 10.0,
            travel_speed: 300.0,
            magic: 0x88,
            frame_light: false,
            presets: Vec::new(),
            user_origin: None,
        }
    }
}
