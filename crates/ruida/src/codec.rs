//! Byte-level encoding of the Ruida protocol.
//!
//! A command starts with a byte ≥ 0x80; its parameters are 7-bit bytes
//! (≤ 0x7F), so numbers are packed 7 bits per byte, most significant first.
//! On the wire every byte is "swizzled" with a model-specific magic
//! (0x88 for the RDC644x family).

/// Swizzle magic of the RDC644x controllers (6442, 6445…).
pub const MAGIC_644X: u8 = 0x88;

pub fn swizzle_byte(b: u8, magic: u8) -> u8 {
    let mut b = b;
    b ^= b >> 7;
    b ^= b << 7;
    b ^= b >> 7;
    b ^= magic;
    b.wrapping_add(1)
}

pub fn unswizzle_byte(b: u8, magic: u8) -> u8 {
    let mut b = b.wrapping_sub(1);
    b ^= magic;
    b ^= b >> 7;
    b ^= b << 7;
    b ^= b >> 7;
    b
}

pub fn swizzle(data: &[u8], magic: u8) -> Vec<u8> {
    data.iter().map(|b| swizzle_byte(*b, magic)).collect()
}

pub fn unswizzle(data: &[u8], magic: u8) -> Vec<u8> {
    data.iter().map(|b| unswizzle_byte(*b, magic)).collect()
}

const MASK35: u64 = (1 << 35) - 1;

/// A 35-bit two's complement number in 5 bytes (coordinates in µm,
/// speeds in µm/s, times in µs, colours).
pub fn num35(v: i64) -> [u8; 5] {
    let u = (v as u64) & MASK35;
    [(u >> 28) as u8 & 0x7F, (u >> 21) as u8 & 0x7F, (u >> 14) as u8 & 0x7F, (u >> 7) as u8 & 0x7F, u as u8 & 0x7F]
}

/// Unsigned value of 5 parameter bytes.
pub fn unum35(b: &[u8]) -> u64 {
    b.iter().take(5).fold(0u64, |a, x| (a << 7) | (*x as u64 & 0x7F))
}

/// Signed value of 5 parameter bytes.
pub fn snum35(b: &[u8]) -> i64 {
    let u = unum35(b);
    if u & (1 << 34) != 0 { u as i64 - (1 << 35) } else { u as i64 }
}

/// A 14-bit signed relative distance (µm, ±8191).
pub fn rel14(v: i64) -> [u8; 2] {
    let u = (v.clamp(-8192, 8191) as u16) & 0x3FFF;
    [(u >> 7) as u8 & 0x7F, u as u8 & 0x7F]
}

pub fn unum14(b: &[u8]) -> u16 {
    ((b[0] as u16 & 0x7F) << 7) | (b[1] as u16 & 0x7F)
}

pub fn snum14(b: &[u8]) -> i64 {
    let u = unum14(b) as i64;
    if u & 0x2000 != 0 { u - 0x4000 } else { u }
}

/// Laser power in percent as 14 bits (100 % = 16384, capped at 16383).
pub fn power14(percent: f64) -> [u8; 2] {
    let raw = (percent.clamp(0.0, 100.0) * 16384.0 / 100.0).round().min(16383.0) as u16;
    [(raw >> 7) as u8 & 0x7F, raw as u8 & 0x7F]
}

pub fn power_of(b: &[u8]) -> f64 {
    unum14(b) as f64 * 100.0 / 16384.0
}

/// Speed in mm/s as µm/s.
pub fn speed(mm_s: f64) -> [u8; 5] {
    num35((mm_s.max(0.0) * 1000.0).round() as i64)
}

/// An RGB colour as 0x00BBGGRR.
pub fn color(rgb: [u8; 3]) -> [u8; 5] {
    num35(((rgb[2] as i64) << 16) | ((rgb[1] as i64) << 8) | rgb[0] as i64)
}

pub fn color_of(b: &[u8]) -> [u8; 3] {
    let v = unum35(b);
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swizzle_is_a_bijection() {
        for magic in [0x88, 0x11, 0x38] {
            let mut seen = [false; 256];
            for b in 0..=255u8 {
                let s = swizzle_byte(b, magic);
                assert!(!seen[s as usize]);
                seen[s as usize] = true;
                assert_eq!(unswizzle_byte(s, magic), b);
            }
        }
    }

    #[test]
    fn numbers_match_the_published_examples() {
        // `C6 31 00 13 19`: part 0 laser 1 min power 14.996 %.
        assert!((power_of(&[0x13, 0x19]) - 14.996338).abs() < 1e-5);
        assert_eq!(power14(14.996338), [0x13, 0x19]);
        // `C9 04 00 | 00 00 06 0D 20`: 100 mm/s.
        assert_eq!(speed(100.0), [0x00, 0x00, 0x06, 0x0D, 0x20]);
        // `88 00 00 11 16 50 00 00 02 12 56`: move to (281424, 35158) µm.
        assert_eq!(snum35(&[0x00, 0x00, 0x11, 0x16, 0x50]), 281424);
        assert_eq!(num35(281424), [0x00, 0x00, 0x11, 0x16, 0x50]);
        // `CA 06 00 00 00 00 01 7F`: red.
        assert_eq!(color_of(&[0, 0, 0, 0x01, 0x7F]), [255, 0, 0]);
        assert_eq!(color([255, 0, 0]), [0, 0, 0, 0x01, 0x7F]);
    }

    #[test]
    fn signed_round_trips() {
        for v in [-1i64, -8192, 8191, 0, 1234, -4321] {
            assert_eq!(snum14(&rel14(v)), v);
        }
        for v in [-1i64, -600_000, 0, 1_000_000, (1 << 34) - 1, -(1 << 34)] {
            assert_eq!(snum35(&num35(v)), v);
        }
        assert_eq!(power14(100.0), [0x7F, 0x7F]);
        assert_eq!(power14(150.0), [0x7F, 0x7F]);
        assert_eq!(power14(-3.0), [0, 0]);
    }
}
