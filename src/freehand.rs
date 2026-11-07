//! Pencil and brush strokes: freehand points fitted into curves; the brush
//! adds a width profile from pen pressure or drawing speed.

use kurbo::Point;

use crate::document::{LineCap, LineJoin, Node, PathData, Rgba, Shape, ShapeKind, Stroke, SubPath};
use crate::geom;

pub use crate::brushes::{BrushStyle, brush_shape};

fn ink() -> Rgba {
    Rgba::from_hex("#1E1E1E").unwrap()
}

/// Fit the drawn points into a subpath. `px` is one screen pixel in
/// document units (the fitting tolerance follows the zoom).
pub(crate) fn fit(pts: &[Point], smooth: f32, px: f64, closed: bool) -> Option<SubPath> {
    let mut clean: Vec<Point> = Vec::with_capacity(pts.len());
    for p in pts {
        if clean.last().is_none_or(|q: &Point| (*q - *p).hypot() > px * 0.5) {
            clean.push(*p);
        }
    }
    if clean.len() < 2 {
        return None;
    }
    // Even spacing (2 screen px), so smoothing acts over a screen distance
    // whatever the input rate.
    let clean = resample(&clean, px * 2.0);
    let mut sm = geom::smooth_polyline(&clean, smooth);
    if closed {
        sm.pop();
    }
    let raw = SubPath { nodes: sm.iter().map(|p| Node::corner(*p)).collect(), closed };
    let tol = px * (0.6 + 2.4 * smooth as f64);
    let sp = geom::simplify_subpath(&raw, tol);
    (sp.nodes.len() >= 2).then_some(sp)
}

/// Points every `step` along the polyline (ends kept).
pub(crate) fn resample(pts: &[Point], step: f64) -> Vec<Point> {
    let mut out = vec![pts[0]];
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let len = (w[1] - w[0]).hypot();
        let mut d = step - carry;
        while d <= len {
            out.push(w[0].lerp(w[1], d / len));
            d += step;
        }
        carry = len - (d - step);
    }
    if out.last().is_some_and(|l| (*l - pts[pts.len() - 1]).hypot() > step * 0.25) {
        out.push(pts[pts.len() - 1]);
    }
    out
}

pub(crate) fn path_length(pts: &[Point]) -> f64 {
    pts.windows(2).map(|w| (w[1] - w[0]).hypot()).sum()
}

/// Pencil: a 2 px line; closed when it ends within 8 screen pixels of where
/// it started (and is long enough to be a loop).
pub fn pencil_shape(pts: &[Point], smooth: f32, px: f64, color: Rgba) -> Option<Shape> {
    let len = path_length(pts);
    if len < px * 3.0 {
        return None;
    }
    let closed = pts.len() > 4 && (pts[0] - *pts.last()?).hypot() < px * 8.0 && len > px * 40.0;
    let sp = fit(pts, smooth, px, closed)?;
    let mut s = Shape::new(ShapeKind::Path(PathData { subpaths: vec![sp] }));
    s.fill = None;
    s.name = "Pencil stroke".into();
    s.stroke = Some(Stroke { width: 2.0, color, cap: LineCap::Round, join: LineJoin::Round, ..Default::default() });
    Some(s)
}

/// Where a brush's width comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WidthSource {
    /// Pen pressure when the device reports it, else drawing speed.
    #[default]
    Auto,
    Pressure,
    Speed,
    Fixed,
}

impl WidthSource {
    pub const ALL: [WidthSource; 4] = [WidthSource::Auto, WidthSource::Pressure, WidthSource::Speed, WidthSource::Fixed];

    pub fn label(self) -> &'static str {
        match self {
            WidthSource::Auto => "Pressure or speed",
            WidthSource::Pressure => "Pen pressure",
            WidthSource::Speed => "Drawing speed",
            WidthSource::Fixed => "Fixed",
        }
    }
}

/// Brush settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushOpts {
    pub width: f64,
    /// 0..1: how much pressure / speed changes the width.
    pub vary: f32,
    pub smooth: f32,
    pub style: BrushStyle,
    pub source: WidthSource,
    pub color: Rgba,
}

impl Default for BrushOpts {
    fn default() -> Self {
        BrushOpts { width: 8.0, vary: 0.8, smooth: 0.5, style: BrushStyle::Round, source: WidthSource::Auto, color: ink() }
    }
}

/// Relative width (0..1) at each drawn point: pen pressure when the device
/// reports it, otherwise slower drawing paints wider.
#[cfg(test)]
pub fn widths(pts: &[Point], times: &[f64], pressure: &[Option<f32>], vary: f32) -> Vec<f64> {
    widths_from(pts, times, pressure, vary, WidthSource::Auto)
}

pub(crate) fn widths_from(pts: &[Point], times: &[f64], pressure: &[Option<f32>], vary: f32, source: WidthSource) -> Vec<f64> {
    let n = pts.len();
    let vary = vary.clamp(0.0, 1.0) as f64;
    let has_pressure = pressure.iter().filter(|p| p.is_some()).count() * 2 > n;
    let use_pressure = match source {
        WidthSource::Auto => has_pressure,
        WidthSource::Pressure => true,
        WidthSource::Speed | WidthSource::Fixed => false,
    };
    let raw: Vec<f64> = if source == WidthSource::Fixed || (use_pressure && !has_pressure) {
        vec![1.0; n]
    } else if use_pressure {
        let mut last = 1.0;
        pressure
            .iter()
            .map(|p| {
                if let Some(v) = p {
                    last = (*v as f64).clamp(0.0, 1.0);
                }
                last
            })
            .collect()
    } else {
        // Speed in document units per second, relative to the median.
        let mut speed = vec![0.0; n];
        for i in 1..n {
            let dt = (times.get(i).copied().unwrap_or(0.0) - times.get(i - 1).copied().unwrap_or(0.0)).max(1.0 / 240.0);
            speed[i] = (pts[i] - pts[i - 1]).hypot() / dt;
        }
        if n > 1 {
            speed[0] = speed[1];
        }
        let mut sorted = speed.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted.get(n / 2).copied().unwrap_or(1.0).max(1e-6);
        speed.iter().map(|s| 1.0 / (1.0 + (s / median - 0.5).max(0.0))).collect()
    };
    // Smooth the widths so they don't jitter.
    let mut w = raw;
    for _ in 0..4 {
        let prev = w.clone();
        for i in 1..n.saturating_sub(1) {
            w[i] = (prev[i - 1] + 2.0 * prev[i] + prev[i + 1]) / 4.0;
        }
    }
    w.iter().map(|v| 1.0 - vary + vary * v).collect()
}

/// Cheap repeatable noise in 0..1 (charcoal grain).
pub(crate) fn grain(x: f64) -> f64 {
    let h = |i: f64| {
        let v = (i * 12.9898).sin() * 43758.5453;
        v - v.floor()
    };
    let (i, f) = (x.floor(), x - x.floor());
    let f = f * f * (3.0 - 2.0 * f);
    h(i) * (1.0 - f) + h(i + 1.0) * f
}

/// How the stabilizer follows the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StabMode {
    /// The tip trails the pointer on a rope of fixed length: it only moves
    /// when the rope is taut, so wobbles shorter than the rope vanish.
    #[default]
    Rope,
    /// The tip is the average of the recent pointer positions.
    Window,
}

/// Steadies freehand input (pencil, brush, knife).
#[derive(Clone, Debug, Default)]
pub struct Stabilizer {
    pub mode: StabMode,
    /// Rope length or averaging window, in document units.
    pub len: f64,
    pub tip: Point,
    recent: std::collections::VecDeque<Point>,
}

impl Stabilizer {
    pub fn new(mode: StabMode, len: f64, at: Point) -> Self {
        Stabilizer { mode, len: len.max(0.0), tip: at, recent: [at].into() }
    }

    /// The new tip for pointer position `p`, if it moved.
    pub fn feed(&mut self, p: Point) -> Option<Point> {
        let next = match self.mode {
            StabMode::Rope => {
                let d = p - self.tip;
                let dist = d.hypot();
                if dist <= self.len {
                    return None;
                }
                self.tip + d * ((dist - self.len) / dist)
            }
            StabMode::Window => {
                self.recent.push_back(p);
                // Keep the recent points within `len` of the pointer.
                while self.recent.len() > 2 && self.recent.front().is_some_and(|q| (*q - p).hypot() > self.len) {
                    self.recent.pop_front();
                }
                let n = self.recent.len() as f64;
                let sum = self.recent.iter().fold(kurbo::Vec2::ZERO, |a, q| a + q.to_vec2());
                (sum / n).to_point()
            }
        };
        self.tip = next;
        Some(next)
    }

    /// Release: the window catches up with the pointer; the rope stays.
    pub fn finish(&mut self, p: Point) -> Option<Point> {
        match self.mode {
            StabMode::Rope => None,
            StabMode::Window => {
                self.tip = p;
                Some(p)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(n: usize) -> Vec<Point> {
        (0..n).map(|i| Point::new(i as f64 * 2.0, (i as f64 * 0.2).sin() * 20.0)).collect()
    }

    #[test]
    fn pencil_fits_few_nodes() {
        let pts = wave(200);
        let s = pencil_shape(&pts, 0.5, 1.0, ink()).unwrap();
        let ShapeKind::Path(pd) = &s.kind else { panic!() };
        let sp = &pd.subpaths[0];
        assert!(!sp.closed);
        assert!(sp.nodes.len() < 40, "{} nodes", sp.nodes.len());
        assert!((sp.nodes[0].p - pts[0]).hypot() < 1.0);
        // A loop closes.
        let ring: Vec<Point> = (0..=100).map(|i| Point::new(50.0 * (i as f64 * 0.0628).cos(), 50.0 * (i as f64 * 0.0628).sin())).collect();
        let s = pencil_shape(&ring, 0.5, 1.0, ink()).unwrap();
        let ShapeKind::Path(pd) = &s.kind else { panic!() };
        assert!(pd.subpaths[0].closed);
        // A click draws nothing.
        assert!(pencil_shape(&[Point::ZERO, Point::new(0.5, 0.0)], 0.5, 1.0, ink()).is_none());
    }

    #[test]
    fn brush_width_follows_pressure_and_speed() {
        let pts = wave(100);
        // Pressure rising along the stroke.
        let times: Vec<f64> = (0..100).map(|i| i as f64 / 60.0).collect();
        let pressure: Vec<Option<f32>> = (0..100).map(|i| Some(i as f32 / 99.0)).collect();
        let s = brush_shape(&pts, &times, &pressure, 1.0, &BrushOpts { width: 10.0, vary: 1.0, smooth: 0.5, ..Default::default() }).unwrap();
        let st = s.stroke.as_ref().unwrap();
        assert_eq!(st.width, 10.0);
        let at = |t: f64| crate::document::profile_at(&st.profile, t);
        assert!(at(0.3) < at(0.7), "{} {}", at(0.3), at(0.7));
        // Ends taper.
        assert!(at(0.0) < at(0.5) * 0.5);
        // Without pressure: the slow half is wider than the fast half.
        let mut times = vec![0.0];
        for i in 1..100 {
            times.push(times[i - 1] + if i < 50 { 0.1 } else { 0.005 });
        }
        let w = widths(&pts, &times, &vec![None; 100], 1.0);
        assert!(w[25] > w[75] * 1.5, "{} {}", w[25], w[75]);
        // The outline is a filled ring with the profile applied.
        let path = crate::geom::pathdata_to_bez(match &s.kind {
            ShapeKind::Path(pd) => pd,
            _ => unreachable!(),
        });
        let o = crate::strokes::variable_outline(&path, st);
        let b = crate::geom::path_bounds(&o).unwrap();
        assert!(b.height() > 40.0 && b.height() < 60.0, "{b:?}");
    }

    #[test]
    fn brush_styles_and_stabilizer() {
        let pts = wave(120);
        let times: Vec<f64> = (0..120).map(|i| i as f64 / 60.0).collect();
        let none = vec![None; 120];
        let mk = |style| brush_shape(&pts, &times, &none, 1.0, &BrushOpts { width: 12.0, style, ..Default::default() }).unwrap();
        let st = |s: &Shape| s.stroke.clone().unwrap();
        assert!(st(&mk(BrushStyle::Marker)).profile.is_empty());
        let dots = st(&mk(BrushStyle::Dots));
        assert!(dots.profile.is_empty() && dots.dash.len() == 2 && dots.dash[0] == 0.0);
        // Calligraphy: thin where the stroke runs along the nib.
        let cal = st(&mk(BrushStyle::Calligraphy));
        let (lo, hi) = cal.profile.iter().fold((1.0f64, 0.0f64), |(a, b), p| (a.min(p.w), b.max(p.w)));
        assert!(hi > lo * 2.0, "{lo} {hi}");
        // Fixed width: no variation away from the tapered ends.
        let fixed = brush_shape(&pts, &times, &none, 1.0, &BrushOpts { width: 12.0, source: WidthSource::Fixed, ..Default::default() }).unwrap();
        let p = &st(&fixed).profile;
        assert!(p[p.len() / 2].w > 0.99);
        // Pressure requested but none reported: even width.
        let pr = brush_shape(&pts, &times, &none, 1.0, &BrushOpts { width: 12.0, source: WidthSource::Pressure, ..Default::default() }).unwrap();
        assert!(st(&pr).profile[60.min(st(&pr).profile.len() / 2)].w > 0.99);
        // Rope: a jittery line comes out within the rope's reach of straight.
        let mut rope = Stabilizer::new(StabMode::Rope, 10.0, Point::ZERO);
        let mut out = Vec::new();
        for i in 1..200 {
            let p = Point::new(i as f64 * 2.0, if i % 2 == 0 { 4.0 } else { -4.0 });
            if let Some(q) = rope.feed(p) {
                out.push(q);
            }
        }
        let wobble = out.iter().skip(10).map(|q| q.y.abs()).fold(0.0, f64::max);
        assert!(wobble < 1.0, "{wobble}");
        assert!(rope.feed(Point::new(out.last().unwrap().x + 5.0, 0.0)).is_none(), "slack rope holds");
        // Window: averages.
        let mut win = Stabilizer::new(StabMode::Window, 20.0, Point::ZERO);
        let mut ys = Vec::new();
        for i in 1..100 {
            ys.push(win.feed(Point::new(i as f64 * 2.0, if i % 2 == 0 { 4.0 } else { -4.0 })).unwrap().y.abs());
        }
        assert!(ys[50..].iter().all(|y| *y < 1.0), "{ys:?}");
    }
}
