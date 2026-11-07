//! Document units. Geometry is always stored in px; fields, rulers and
//! readouts convert through the current unit and DPI (set once per frame).

use std::cell::Cell;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    #[default]
    Px,
    Mm,
    Cm,
    In,
    Pt,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::Px, Unit::Mm, Unit::Cm, Unit::In, Unit::Pt];

    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Px => "px",
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::In => "in",
            Unit::Pt => "pt",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Unit::Px => "Pixels",
            Unit::Mm => "Millimeters",
            Unit::Cm => "Centimeters",
            Unit::In => "Inches",
            Unit::Pt => "Points",
        }
    }

    /// Value of one px in this unit at `dpi`.
    pub fn per_px(self, dpi: f64) -> f64 {
        let dpi = dpi.max(1.0);
        match self {
            Unit::Px => 1.0,
            Unit::Mm => 25.4 / dpi,
            Unit::Cm => 2.54 / dpi,
            Unit::In => 1.0 / dpi,
            Unit::Pt => 72.0 / dpi,
        }
    }

    pub fn decimals(self) -> usize {
        match self {
            Unit::Px | Unit::Pt => 2,
            Unit::Mm => 2,
            Unit::Cm | Unit::In => 3,
        }
    }

    /// Convert a length in this unit to px.
    pub fn to_px(self, v: f64, dpi: f64) -> f64 {
        v / self.per_px(dpi)
    }
}

thread_local! {
    static CURRENT: Cell<(Unit, f64)> = const { Cell::new((Unit::Px, 96.0)) };
}

pub fn set(unit: Unit, dpi: f64) {
    CURRENT.with(|c| c.set((unit, dpi)));
}

pub fn current() -> (Unit, f64) {
    CURRENT.with(|c| c.get())
}

/// Current unit value of one px.
pub fn factor() -> f64 {
    let (u, dpi) = current();
    u.per_px(dpi)
}

/// Format a px length in the current unit without the suffix.
pub fn fmt_value(px: f64) -> String {
    let (u, dpi) = current();
    let v = px * u.per_px(dpi);
    let s = format!("{:.*}", u.decimals(), v);
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

/// Format a px length in the current unit, e.g. `210 mm`.
pub fn fmt(px: f64) -> String {
    format!("{} {}", fmt_value(px), current().0.suffix())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert!((Unit::Mm.to_px(25.4, 96.0) - 96.0).abs() < 1e-9);
        assert!((Unit::Pt.per_px(72.0) - 1.0).abs() < 1e-9);
        set(Unit::Mm, 72.0);
        assert_eq!(fmt(595.2755905511812), "210 mm");
        set(Unit::Px, 96.0);
        assert_eq!(fmt(12.5), "12.5 px");
    }
}

/// Evaluate a field entry like `120/2`, `(10 + 5) * 2` or `10mm + 4`.
/// With `unit` (the field shows lengths in that unit at `dpi`), numbers may
/// carry their own unit and are converted; other suffixes (%, °) are ignored.
pub fn eval_expr(src: &str, unit: Option<(Unit, f64)>) -> Option<f64> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
        unit: Option<(Unit, f64)>,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && (self.s[self.i] as char).is_whitespace() {
                self.i += 1;
            }
        }
        fn peek(&mut self) -> Option<u8> {
            self.ws();
            self.s.get(self.i).copied()
        }
        fn expr(&mut self) -> Option<f64> {
            let mut v = self.term()?;
            while let Some(c) = self.peek() {
                match c {
                    b'+' => {
                        self.i += 1;
                        v += self.term()?;
                    }
                    b'-' => {
                        self.i += 1;
                        v -= self.term()?;
                    }
                    _ => break,
                }
            }
            Some(v)
        }
        fn term(&mut self) -> Option<f64> {
            let mut v = self.factor()?;
            while let Some(c) = self.peek() {
                match c {
                    b'*' | b'x' => {
                        self.i += 1;
                        v *= self.factor()?;
                    }
                    b'/' => {
                        self.i += 1;
                        let d = self.factor()?;
                        if d == 0.0 {
                            return None;
                        }
                        v /= d;
                    }
                    _ => break,
                }
            }
            Some(v)
        }
        fn factor(&mut self) -> Option<f64> {
            match self.peek()? {
                b'-' => {
                    self.i += 1;
                    Some(-self.factor()?)
                }
                b'+' => {
                    self.i += 1;
                    self.factor()
                }
                b'(' => {
                    self.i += 1;
                    let v = self.expr()?;
                    (self.peek()? == b')').then(|| self.i += 1)?;
                    Some(v)
                }
                _ => {
                    let start = self.i;
                    while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.' || self.s[self.i] == b',') {
                        self.i += 1;
                    }
                    let num: f64 = std::str::from_utf8(&self.s[start..self.i]).ok()?.replace(',', ".").parse().ok()?;
                    // Optional unit suffix.
                    self.ws();
                    let us = self.i;
                    while self.i < self.s.len() && (self.s[self.i].is_ascii_alphabetic() || self.s[self.i] == b'%' || self.s[self.i] >= 0x80) {
                        self.i += 1;
                    }
                    let suffix = std::str::from_utf8(&self.s[us..self.i]).ok()?.to_lowercase();
                    let from = Unit::ALL.into_iter().find(|u| u.suffix() == suffix);
                    Some(match (from, self.unit) {
                        (Some(f), Some((to, dpi))) => f.to_px(num, dpi) * to.per_px(dpi),
                        _ => num,
                    })
                }
            }
        }
    }
    let mut p = P { s: src.as_bytes(), i: 0, unit };
    let v = p.expr()?;
    p.ws();
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}
