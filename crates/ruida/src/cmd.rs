//! Ruida commands: building them, splitting a byte stream back into them,
//! and describing them for logs. Coordinates are µm, speeds mm/s, powers %.

use crate::codec::*;

/// Memory addresses read with `DA 00`.
pub mod mem {
    pub const MACHINE_STATUS: u16 = 0x0400;
    pub const POS_X: u16 = 0x0421;
    pub const POS_Y: u16 = 0x0431;
    pub const POS_Z: u16 = 0x0441;
    pub const POS_U: u16 = 0x0451;
    pub const CARD_ID: u16 = 0x057E;
    pub const MAINBOARD_VERSION: u16 = 0x057F;
    pub const BED_X: u16 = 0x0026;
    pub const BED_Y: u16 = 0x0036;
}

/// Byte values of single-byte replies (before swizzling).
pub const ACK: u8 = 0xCC;
pub const NAK: u8 = 0xCF;
pub const ERR: u8 = 0xCD;
pub const ENQ: u8 = 0xCE;

/// Options of a `D9` rapid move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rapid {
    /// Through the origin.
    Origin = 0,
    OriginLight = 1,
    Plain = 2,
    /// With the red-dot pointer on.
    Light = 3,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    MoveAbs { x: i64, y: i64 },
    MoveRel { dx: i64, dy: i64 },
    CutAbs { x: i64, y: i64 },
    CutRel { dx: i64, dy: i64 },
    /// `C6 <which> <power>`: 01/02 laser 1 min/max, 21/22 laser 2…
    Power { which: u8, percent: f64 },
    /// `C6 <which> <part> <power>`: 31/32 part laser 1 min/max…
    PartPower { which: u8, part: u8, percent: f64 },
    /// `C6 <which> <time µs>`: delays (15/16 laser on2/off2…).
    LaserTime { which: u8, us: i64 },
    /// `C9 02`: cutting speed of the current layer.
    Speed(f64),
    /// `C9 04 <part>`: a part's speed.
    PartSpeed { part: u8, mm_s: f64 },
    /// `CA 01 <flag>`: 00 end layer, 10/11 layer device, 12/13 air off/on, 30/31 laser 2 offset.
    LayerFlag(u8),
    LayerNumber(u8),
    PartColor { part: u8, rgb: [u8; 3] },
    MaxLayer(u8),
    WorkMode { part: u8, mode: u8 },
    SetAbsolute,
    BlockEnd,
    ProcessTopLeft { x: i64, y: i64 },
    ProcessBottomRight { x: i64, y: i64 },
    DocumentMin { x: i64, y: i64 },
    DocumentMax { x: i64, y: i64 },
    PartMin { part: u8, x: i64, y: i64 },
    PartMax { part: u8, x: i64, y: i64 },
    FeedRepeat,
    ProcessRepeat,
    ArrayDirection(u8),
    PenOffset { axis: u8, v: i64 },
    LayerOffset { axis: u8, v: i64 },
    DisplayOffset { x: i64, y: i64 },
    BlockCutting(u8),
    RefPointSet,
    /// `D8 10/11/12`: reference point = machine zero / anchor / current.
    RefPointMode(u8),
    StartProcess,
    StopProcess,
    PauseProcess,
    ResumeProcess,
    HomeXY,
    RapidXY { opt: Rapid, x: i64, y: i64 },
    ReadMem(u16),
    /// `DA 01 <addr> <value>`: a read's reply (or a write).
    MemValue { addr: u16, value: u64 },
    EndOfFile,
    Ack,
    Nak,
    Err,
    Enq,
    /// Anything else, kept byte for byte.
    Other(Vec<u8>),
}

fn xy(op: &[u8], x: i64, y: i64) -> Vec<u8> {
    let mut v = op.to_vec();
    v.extend(num35(x));
    v.extend(num35(y));
    v
}

impl Command {
    pub fn encode(&self) -> Vec<u8> {
        use Command::*;
        match self {
            MoveAbs { x, y } => xy(&[0x88], *x, *y),
            MoveRel { dx, dy } => [&[0x89][..], &rel14(*dx), &rel14(*dy)].concat(),
            CutAbs { x, y } => xy(&[0xA8], *x, *y),
            CutRel { dx, dy } => [&[0xA9][..], &rel14(*dx), &rel14(*dy)].concat(),
            Power { which, percent } => [&[0xC6, *which][..], &power14(*percent)].concat(),
            PartPower { which, part, percent } => [&[0xC6, *which, *part][..], &power14(*percent)].concat(),
            LaserTime { which, us } => [&[0xC6, *which][..], &num35(*us)].concat(),
            Speed(s) => [&[0xC9, 0x02][..], &speed(*s)].concat(),
            PartSpeed { part, mm_s } => [&[0xC9, 0x04, *part][..], &speed(*mm_s)].concat(),
            LayerFlag(f) => vec![0xCA, 0x01, *f],
            LayerNumber(n) => vec![0xCA, 0x02, *n],
            PartColor { part, rgb } => [&[0xCA, 0x06, *part][..], &color(*rgb)].concat(),
            MaxLayer(n) => vec![0xCA, 0x22, *n],
            WorkMode { part, mode } => vec![0xCA, 0x41, *part, *mode],
            SetAbsolute => vec![0xE6, 0x01],
            BlockEnd => vec![0xE7, 0x00],
            ProcessTopLeft { x, y } => xy(&[0xE7, 0x03], *x, *y),
            ProcessBottomRight { x, y } => xy(&[0xE7, 0x07], *x, *y),
            DocumentMin { x, y } => xy(&[0xE7, 0x50], *x, *y),
            DocumentMax { x, y } => xy(&[0xE7, 0x51], *x, *y),
            PartMin { part, x, y } => xy(&[0xE7, 0x52, *part], *x, *y),
            PartMax { part, x, y } => xy(&[0xE7, 0x53, *part], *x, *y),
            FeedRepeat => [&[0xE7, 0x06][..], &[0; 10]].concat(),
            ProcessRepeat => [&[0xE7, 0x04, 0x00, 0x01, 0x00, 0x01][..], &[0; 10]].concat(),
            ArrayDirection(d) => vec![0xE7, 0x05, *d],
            PenOffset { axis, v } => [&[0xE7, 0x54, *axis][..], &num35(*v)].concat(),
            LayerOffset { axis, v } => [&[0xE7, 0x55, *axis][..], &num35(*v)].concat(),
            DisplayOffset { x, y } => xy(&[0xF1, 0x03], *x, *y),
            BlockCutting(v) => vec![0xF1, 0x02, *v],
            RefPointSet => vec![0xF0],
            RefPointMode(m) => vec![0xD8, *m],
            StartProcess => vec![0xD8, 0x00],
            StopProcess => vec![0xD8, 0x01],
            PauseProcess => vec![0xD8, 0x02],
            ResumeProcess => vec![0xD8, 0x03],
            HomeXY => vec![0xD8, 0x2A],
            RapidXY { opt, x, y } => xy(&[0xD9, 0x10, *opt as u8], *x, *y),
            ReadMem(a) => vec![0xDA, 0x00, (a >> 8) as u8 & 0x7F, *a as u8 & 0x7F],
            MemValue { addr, value } => [&[0xDA, 0x01, (addr >> 8) as u8 & 0x7F, *addr as u8 & 0x7F][..], &num35(*value as i64)].concat(),
            EndOfFile => vec![0xD7],
            Ack => vec![ACK],
            Nak => vec![NAK],
            Err => vec![ERR],
            Enq => vec![ENQ],
            Other(b) => b.clone(),
        }
    }

    /// One command from its bytes (opcode + parameters).
    pub fn decode(b: &[u8]) -> Command {
        use Command::*;
        let p = &b[1.min(b.len())..];
        let n = |i: usize| p.get(i..i + 5).filter(|s| s.len() == 5);
        let xy = |i: usize| Some((snum35(n(i)?), snum35(n(i + 5)?)));
        let other = || Other(b.to_vec());
        let op = b.first().copied().unwrap_or(0);
        let sub = p.first().copied();
        let q = &p[1.min(p.len())..];
        let qn = |i: usize| q.get(i..i + 5).filter(|s| s.len() == 5);
        let qxy = |i: usize| Some((snum35(qn(i)?), snum35(qn(i + 5)?)));
        match (op, sub) {
            (0x88, _) => xy(0).map(|(x, y)| MoveAbs { x, y }).unwrap_or_else(other),
            (0xA8, _) => xy(0).map(|(x, y)| CutAbs { x, y }).unwrap_or_else(other),
            (0x89, _) if p.len() == 4 => MoveRel { dx: snum14(&p[0..2]), dy: snum14(&p[2..4]) },
            (0xA9, _) if p.len() == 4 => CutRel { dx: snum14(&p[0..2]), dy: snum14(&p[2..4]) },
            (0x8A, _) if p.len() == 2 => MoveRel { dx: snum14(p), dy: 0 },
            (0x8B, _) if p.len() == 2 => MoveRel { dx: 0, dy: snum14(p) },
            (0xAA, _) if p.len() == 2 => CutRel { dx: snum14(p), dy: 0 },
            (0xAB, _) if p.len() == 2 => CutRel { dx: 0, dy: snum14(p) },
            (0xC6, Some(w @ (0x10..=0x16))) if q.len() == 5 => LaserTime { which: w, us: snum35(q) },
            (0xC6, Some(w)) if q.len() == 2 => Power { which: w, percent: power_of(q) },
            (0xC6, Some(w)) if q.len() == 3 => PartPower { which: w, part: q[0], percent: power_of(&q[1..]) },
            (0xC9, Some(0x02)) if q.len() == 5 => Speed(unum35(q) as f64 / 1000.0),
            (0xC9, Some(0x04)) if q.len() == 6 => PartSpeed { part: q[0], mm_s: unum35(&q[1..]) as f64 / 1000.0 },
            (0xCA, Some(0x01)) if q.len() == 1 => LayerFlag(q[0]),
            (0xCA, Some(0x02)) if q.len() == 1 => LayerNumber(q[0]),
            (0xCA, Some(0x06)) if q.len() == 6 => PartColor { part: q[0], rgb: color_of(&q[1..]) },
            (0xCA, Some(0x22)) if q.len() == 1 => MaxLayer(q[0]),
            (0xCA, Some(0x41)) if q.len() == 2 => WorkMode { part: q[0], mode: q[1] },
            (0xE6, Some(0x01)) => SetAbsolute,
            (0xE7, Some(0x00)) => BlockEnd,
            (0xE7, Some(0x03)) => qxy(0).map(|(x, y)| ProcessTopLeft { x, y }).unwrap_or_else(other),
            (0xE7, Some(0x07)) => qxy(0).map(|(x, y)| ProcessBottomRight { x, y }).unwrap_or_else(other),
            (0xE7, Some(0x50)) => qxy(0).map(|(x, y)| DocumentMin { x, y }).unwrap_or_else(other),
            (0xE7, Some(0x51)) => qxy(0).map(|(x, y)| DocumentMax { x, y }).unwrap_or_else(other),
            (0xE7, Some(0x52)) if q.len() == 11 => PartMin { part: q[0], x: snum35(&q[1..6]), y: snum35(&q[6..11]) },
            (0xE7, Some(0x53)) if q.len() == 11 => PartMax { part: q[0], x: snum35(&q[1..6]), y: snum35(&q[6..11]) },
            (0xE7, Some(0x06)) => FeedRepeat,
            (0xE7, Some(0x04)) => ProcessRepeat,
            (0xE7, Some(0x05)) if q.len() == 1 => ArrayDirection(q[0]),
            (0xE7, Some(0x54)) if q.len() == 6 => PenOffset { axis: q[0], v: snum35(&q[1..]) },
            (0xE7, Some(0x55)) if q.len() == 6 => LayerOffset { axis: q[0], v: snum35(&q[1..]) },
            (0xF1, Some(0x03)) => qxy(0).map(|(x, y)| DisplayOffset { x, y }).unwrap_or_else(other),
            (0xF1, Some(0x02)) if q.len() == 1 => BlockCutting(q[0]),
            (0xF0, None) => RefPointSet,
            (0xD8, Some(0x00)) => StartProcess,
            (0xD8, Some(0x01)) => StopProcess,
            (0xD8, Some(0x02)) => PauseProcess,
            (0xD8, Some(0x03)) => ResumeProcess,
            (0xD8, Some(0x2A)) => HomeXY,
            (0xD8, Some(m @ (0x10..=0x12))) => RefPointMode(m),
            (0xD9, Some(0x10)) if q.len() == 11 => {
                let opt = match q[0] {
                    0 => Rapid::Origin,
                    1 => Rapid::OriginLight,
                    3 => Rapid::Light,
                    _ => Rapid::Plain,
                };
                RapidXY { opt, x: snum35(&q[1..6]), y: snum35(&q[6..11]) }
            }
            (0xDA, Some(0x00)) if q.len() == 2 => ReadMem(((q[0] as u16) << 8) | q[1] as u16),
            (0xDA, Some(0x01)) if q.len() >= 7 => MemValue { addr: ((q[0] as u16) << 8) | q[1] as u16, value: unum35(&q[2..]) },
            (0xD7, None) => EndOfFile,
            (ACK, None) => Ack,
            (NAK, None) => Nak,
            (ERR, None) => Err,
            (ENQ, None) => Enq,
            _ => other(),
        }
    }

    /// A short readable form (mm, mm/s, %).
    pub fn describe(&self) -> String {
        use Command::*;
        let mm = |v: &i64| format!("{:.3}", *v as f64 / 1000.0);
        match self {
            MoveAbs { x, y } => format!("move to {}, {}", mm(x), mm(y)),
            MoveRel { dx, dy } => format!("move by {}, {}", mm(dx), mm(dy)),
            CutAbs { x, y } => format!("cut to {}, {}", mm(x), mm(y)),
            CutRel { dx, dy } => format!("cut by {}, {}", mm(dx), mm(dy)),
            Power { which, percent } => format!("power {which:02X} {percent:.1} %"),
            PartPower { which, part, percent } => format!("layer {part} power {which:02X} {percent:.1} %"),
            LaserTime { which, us } => format!("laser time {which:02X} {us} µs"),
            Speed(s) => format!("speed {s:.1} mm/s"),
            PartSpeed { part, mm_s } => format!("layer {part} speed {mm_s:.1} mm/s"),
            LayerFlag(0) => "end layer".into(),
            LayerFlag(0x12) => "air assist off".into(),
            LayerFlag(0x13) => "air assist on".into(),
            LayerFlag(f) => format!("layer flag {f:02X}"),
            LayerNumber(n) => format!("layer {n}"),
            PartColor { part, rgb } => format!("layer {part} colour #{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]),
            MaxLayer(n) => format!("last layer {n}"),
            WorkMode { part, mode } => format!("layer {part} mode {mode}"),
            ProcessTopLeft { x, y } | DocumentMin { x, y } => format!("{:?} {}, {}", self.name(), mm(x), mm(y)),
            ProcessBottomRight { x, y } | DocumentMax { x, y } | DisplayOffset { x, y } => format!("{:?} {}, {}", self.name(), mm(x), mm(y)),
            PartMin { part, x, y } => format!("layer {part} min {}, {}", mm(x), mm(y)),
            PartMax { part, x, y } => format!("layer {part} max {}, {}", mm(x), mm(y)),
            RapidXY { opt, x, y } => format!("rapid to {}, {} ({opt:?})", mm(x), mm(y)),
            ReadMem(a) => format!("read {a:04X}"),
            MemValue { addr, value } => format!("value {addr:04X} = {value} (0x{value:X})"),
            Other(b) => format!("? {}", hex(b)),
            _ => self.name().to_string(),
        }
    }

    fn name(&self) -> &'static str {
        use Command::*;
        match self {
            SetAbsolute => "absolute coordinates",
            BlockEnd => "block end",
            ProcessTopLeft { .. } => "top left",
            ProcessBottomRight { .. } => "bottom right",
            DocumentMin { .. } => "document min",
            DocumentMax { .. } => "document max",
            DisplayOffset { .. } => "display offset",
            FeedRepeat => "feed repeat",
            ProcessRepeat => "process repeat",
            ArrayDirection(_) => "array direction",
            PenOffset { .. } => "pen offset",
            LayerOffset { .. } => "layer offset",
            BlockCutting(_) => "block cutting",
            RefPointSet => "reference point set",
            RefPointMode(_) => "reference point mode",
            StartProcess => "START",
            StopProcess => "STOP",
            PauseProcess => "pause",
            ResumeProcess => "resume",
            HomeXY => "home XY",
            EndOfFile => "end of file",
            Ack => "ACK",
            Nak => "NAK",
            Err => "ERR",
            Enq => "ENQ",
            _ => "command",
        }
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

/// Split a plain (unswizzled) stream into commands: each starts at a byte
/// ≥ 0x80. A trailing command may be incomplete when the stream is cut.
pub fn split(stream: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, b) in stream.iter().enumerate() {
        if *b >= 0x80 {
            if let Some(s) = start {
                out.push(&stream[s..i]);
            }
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(&stream[s..]);
    }
    out
}

pub fn parse(stream: &[u8]) -> Vec<Command> {
    split(stream).into_iter().map(Command::decode).collect()
}

/// Commands that move the head or fire the laser.
pub fn is_motion(c: &Command) -> bool {
    use Command::*;
    matches!(c, MoveAbs { .. } | MoveRel { .. } | CutAbs { .. } | CutRel { .. } | StartProcess | ResumeProcess | HomeXY | RapidXY { .. })
        || matches!(c, Other(b) if matches!(b.first(), Some(0x80..=0x8F | 0xA0..=0xAF | 0xD8 | 0xD9)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip() {
        use Command::*;
        let all = vec![
            MoveAbs { x: 281424, y: 35158 },
            CutAbs { x: -5, y: 1_000_000 },
            MoveRel { dx: -100, dy: 8191 },
            CutRel { dx: 1, dy: -1 },
            Power { which: 0x01, percent: 15.0 },
            PartPower { which: 0x32, part: 2, percent: 70.0 },
            LaserTime { which: 0x15, us: 0 },
            Speed(100.0),
            PartSpeed { part: 1, mm_s: 12.5 },
            LayerFlag(0x13),
            LayerNumber(3),
            PartColor { part: 0, rgb: [255, 0, 0] },
            MaxLayer(2),
            WorkMode { part: 0, mode: 0 },
            SetAbsolute,
            BlockEnd,
            ProcessTopLeft { x: 1, y: 2 },
            ProcessBottomRight { x: 3, y: 4 },
            DocumentMin { x: 5, y: 6 },
            DocumentMax { x: 7, y: 8 },
            PartMin { part: 1, x: 9, y: 10 },
            PartMax { part: 1, x: 11, y: 12 },
            FeedRepeat,
            ProcessRepeat,
            ArrayDirection(0),
            PenOffset { axis: 1, v: 0 },
            LayerOffset { axis: 0, v: 0 },
            DisplayOffset { x: 0, y: 0 },
            BlockCutting(0),
            RefPointSet,
            RefPointMode(0x10),
            StartProcess,
            StopProcess,
            PauseProcess,
            ResumeProcess,
            HomeXY,
            RapidXY { opt: Rapid::Light, x: 10_000, y: 20_000 },
            ReadMem(mem::CARD_ID),
            MemValue { addr: mem::POS_X, value: 123_456 },
            EndOfFile,
        ];
        let bytes: Vec<u8> = all.iter().flat_map(|c| c.encode()).collect();
        let back = parse(&bytes);
        assert_eq!(back.len(), all.len());
        // Equal up to the 14-bit power steps.
        assert_eq!(back.iter().flat_map(|c| c.encode()).collect::<Vec<u8>>(), bytes);
        for (a, b) in back.iter().zip(&all) {
            match (a, b) {
                (Power { percent: p, .. } | PartPower { percent: p, .. }, Power { percent: q, .. } | PartPower { percent: q, .. }) => assert!((p - q).abs() < 0.01),
                _ => assert_eq!(a, b),
            }
        }
        // The published example bytes.
        assert_eq!(MoveAbs { x: 281424, y: 35158 }.encode(), [0x88, 0, 0, 0x11, 0x16, 0x50, 0, 0, 0x02, 0x12, 0x56]);
        assert_eq!(Command::decode(&[0xC6, 0x31, 0x00, 0x13, 0x19]), PartPower { which: 0x31, part: 0, percent: 2457.0 * 100.0 / 16384.0 });
        assert_eq!(ReadMem(mem::CARD_ID).encode(), [0xDA, 0x00, 0x05, 0x7E]);
    }

    /// The connection probe sends `DA 00 05 7E` swizzled with the 644x
    /// magic. A controller of another family (another magic) must not read
    /// it as motion. (Sent with 0x11, a 0x38 controller would see `A8`, so
    /// the probe never tries other magics on its own.)
    #[test]
    fn probing_with_a_wrong_magic_is_harmless() {
        let magics = [0x88u8, 0x11, 0x38];
        let probe = Command::ReadMem(mem::CARD_ID).encode();
        for sent in [crate::codec::MAGIC_644X] {
            for actual in magics.into_iter().filter(|m| *m != sent) {
                let seen = crate::codec::unswizzle(&crate::codec::swizzle(&probe, sent), actual);
                for c in parse(&seen) {
                    assert!(!is_motion(&c), "{sent:02X} read as {actual:02X}: {}", hex(&seen));
                }
            }
        }
    }
}
