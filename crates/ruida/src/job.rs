//! A laser job and its `.rd` stream (the layout RDWorks writes: header,
//! layer table, then each layer's moves and cuts, end of file).

use crate::cmd::Command;

/// A point in machine coordinates, µm.
pub type Pt = (i64, i64);

#[derive(Clone, Debug, PartialEq)]
pub struct JobLayer {
    pub rgb: [u8; 3],
    /// mm/s.
    pub speed: f64,
    /// Percent; the power at slow corners and at full speed.
    pub min_power: f64,
    pub max_power: f64,
    pub air: bool,
    pub passes: u32,
    /// Polylines, each cut from its first point (moved to with the laser off).
    pub paths: Vec<Vec<Pt>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Job {
    pub layers: Vec<JobLayer>,
}

fn bounds_of<'a>(pts: impl Iterator<Item = &'a Pt>) -> Option<(Pt, Pt)> {
    pts.fold(None, |b, p| match b {
        None => Some((*p, *p)),
        Some((lo, hi)) => Some(((lo.0.min(p.0), lo.1.min(p.1)), (hi.0.max(p.0), hi.1.max(p.1)))),
    })
}

impl JobLayer {
    pub fn bounds(&self) -> Option<(Pt, Pt)> {
        bounds_of(self.paths.iter().flatten())
    }
}

/// Totals for the confirmation and the panel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    /// mm with the laser on, mm travelled with it off.
    pub cut_mm: f64,
    pub travel_mm: f64,
    /// Seconds (cuts at layer speed, travel at `travel_speed`, plus a
    /// little per path for acceleration).
    pub seconds: f64,
}

impl Job {
    pub fn bounds(&self) -> Option<(Pt, Pt)> {
        bounds_of(self.layers.iter().flat_map(|l| l.paths.iter().flatten()))
    }

    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| l.paths.iter().all(|p| p.len() < 2))
    }

    pub fn max_power(&self) -> f64 {
        self.layers.iter().map(|l| l.max_power.max(l.min_power)).fold(0.0, f64::max)
    }

    pub fn estimate(&self, start: Pt, travel_speed: f64) -> Estimate {
        let d = |a: Pt, b: Pt| (((a.0 - b.0) as f64).powi(2) + ((a.1 - b.1) as f64).powi(2)).sqrt() / 1000.0;
        let mut e = Estimate::default();
        let mut at = start;
        for l in &self.layers {
            for _ in 0..l.passes.max(1) {
                for p in l.paths.iter().filter(|p| p.len() >= 2) {
                    let t = d(at, p[0]);
                    e.travel_mm += t;
                    e.seconds += t / travel_speed.max(1.0) + 0.02;
                    let c: f64 = p.windows(2).map(|w| d(w[0], w[1])).sum();
                    e.cut_mm += c;
                    e.seconds += c / l.speed.max(0.1);
                    at = *p.last().unwrap();
                }
            }
        }
        e
    }

    /// The commands of the whole job. Coordinates are absolute machine
    /// coordinates (reference point = machine zero).
    pub fn commands(&self) -> Vec<Command> {
        use Command::*;
        let layers: Vec<&JobLayer> = self.layers.iter().filter(|l| l.paths.iter().any(|p| p.len() >= 2)).collect();
        let ((x0, y0), (x1, y1)) = self.bounds().unwrap_or(((0, 0), (0, 0)));
        let mut c = vec![
            RefPointMode(0x10),
            RefPointSet,
            SetAbsolute,
            BlockCutting(0),
            StartProcess,
            FeedRepeat,
            ProcessTopLeft { x: x0, y: y0 },
            ProcessBottomRight { x: x1, y: y1 },
            DocumentMin { x: x0, y: y0 },
            DocumentMax { x: x1, y: y1 },
            ProcessRepeat,
            ArrayDirection(0),
        ];
        for (i, l) in layers.iter().enumerate() {
            let part = i as u8;
            let ((a0, b0), (a1, b1)) = l.bounds().unwrap_or(((0, 0), (0, 0)));
            c.extend([
                PartSpeed { part, mm_s: l.speed },
                PartPower { which: 0x31, part, percent: l.min_power },
                PartPower { which: 0x32, part, percent: l.max_power },
                PartColor { part, rgb: l.rgb },
                WorkMode { part, mode: 0 },
                PartMin { part, x: a0, y: b0 },
                PartMax { part, x: a1, y: b1 },
            ]);
        }
        c.push(MaxLayer(layers.len().saturating_sub(1) as u8));
        c.extend([PenOffset { axis: 0, v: 0 }, PenOffset { axis: 1, v: 0 }, LayerOffset { axis: 0, v: 0 }, LayerOffset { axis: 1, v: 0 }, DisplayOffset { x: 0, y: 0 }]);
        for (i, l) in layers.iter().enumerate() {
            c.extend([
                LayerNumber(i as u8),
                LayerFlag(0x30),
                LayerFlag(0x10),
                LayerFlag(if l.air { 0x13 } else { 0x12 }),
                Speed(l.speed),
                LaserTime { which: 0x15, us: 0 },
                LaserTime { which: 0x16, us: 0 },
                Power { which: 0x01, percent: l.min_power },
                Power { which: 0x02, percent: l.max_power },
            ]);
            for _ in 0..l.passes.max(1) {
                for p in l.paths.iter().filter(|p| p.len() >= 2) {
                    c.push(MoveAbs { x: p[0].0, y: p[0].1 });
                    c.extend(p[1..].iter().map(|q| CutAbs { x: q.0, y: q.1 }));
                }
            }
            c.push(LayerFlag(0x00));
        }
        c.extend([BlockEnd, EndOfFile]);
        c
    }

    /// The plain (unswizzled) `.rd` stream.
    pub fn encode(&self) -> Vec<u8> {
        self.commands().iter().flat_map(|c| c.encode()).collect()
    }

    /// The `.rd` file: the stream swizzled with the controller's magic.
    pub fn rd_file(&self, magic: u8) -> Vec<u8> {
        crate::codec::swizzle(&self.encode(), magic)
    }
}

/// The cuts a stream describes, per layer number: (layer, from, to). Used
/// to check jobs (and by the emulator).
pub fn trace(cmds: &[Command]) -> Vec<(u8, Pt, Pt)> {
    let mut at = (0, 0);
    let mut layer = 0;
    let mut out = Vec::new();
    for c in cmds {
        match c {
            Command::LayerNumber(n) => layer = *n,
            Command::MoveAbs { x, y } => at = (*x, *y),
            Command::MoveRel { dx, dy } => at = (at.0 + dx, at.1 + dy),
            Command::CutAbs { x, y } => {
                out.push((layer, at, (*x, *y)));
                at = (*x, *y);
            }
            Command::CutRel { dx, dy } => {
                let to = (at.0 + dx, at.1 + dy);
                out.push((layer, at, to));
                at = to;
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{Command, parse};

    fn square(x: i64, y: i64, s: i64) -> Vec<Pt> {
        vec![(x, y), (x + s, y), (x + s, y + s), (x, y + s), (x, y)]
    }

    #[test]
    fn job_stream_round_trips() {
        let job = Job {
            layers: vec![
                JobLayer { rgb: [255, 0, 0], speed: 20.0, min_power: 30.0, max_power: 40.0, air: true, passes: 2, paths: vec![square(10_000, 10_000, 20_000)] },
                JobLayer { rgb: [0, 0, 255], speed: 200.0, min_power: 10.0, max_power: 15.0, air: false, passes: 1, paths: vec![vec![(0, 0), (5_000, 0)], vec![(1, 1)]] },
            ],
        };
        let cmds = parse(&job.encode());
        assert_eq!(cmds.iter().flat_map(|c| c.encode()).collect::<Vec<u8>>(), job.encode());
        assert_eq!(cmds.last(), Some(&Command::EndOfFile));
        let cuts = trace(&cmds);
        // 4 sides × 2 passes on layer 0, one line on layer 1 (a single point is no path).
        assert_eq!(cuts.iter().filter(|c| c.0 == 0).count(), 8);
        assert_eq!(cuts.iter().filter(|c| c.0 == 1).collect::<Vec<_>>(), [&(1, (0, 0), (5_000, 0))]);
        assert!(cmds.contains(&Command::PartPower { which: 0x32, part: 1, percent: power_roundtrip(15.0) }));
        assert!(cmds.contains(&Command::LayerFlag(0x13)) && cmds.contains(&Command::LayerFlag(0x12)));
        assert_eq!(job.bounds(), Some(((0, 0), (30_000, 30_000))));
        let e = job.estimate((0, 0), 300.0);
        assert!((e.cut_mm - (2.0 * 80.0 + 5.0)).abs() < 1e-6, "{e:?}");
        // The file is the stream, swizzled.
        assert_eq!(crate::codec::unswizzle(&job.rd_file(0x88), 0x88), job.encode());
    }

    fn power_roundtrip(p: f64) -> f64 {
        crate::codec::power_of(&crate::codec::power14(p))
    }
}
