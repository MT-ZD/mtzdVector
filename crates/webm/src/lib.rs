//! WebM (AV1) video export: rav1e encoding plus a minimal Matroska writer.
//!
//! Output is 8-bit 4:2:0 BT.709 limited range. Odd sizes are padded up to
//! even by repeating the last row / column; the padded size is what the
//! container reports.

use rav1e::color::{ChromaSamplePosition, ColorDescription, ColorPrimaries, MatrixCoefficients, PixelRange, TransferCharacteristics};
use rav1e::prelude::{ChromaSampling, Config, Context, EncoderConfig, EncoderStatus, FrameType, Rational, SceneDetectionSpeed, SpeedSettings};

/// Encoder speed preset (0 = slowest, 10 = fastest).
const SPEED: u8 = 10;
/// Longest stretch of a cluster (ms), well within the int16 block offset.
const CLUSTER_MS: u64 = 5000;

/// Encode RGBA8 frames (straight alpha, composited onto `background` first) as an AV1 WebM.
/// `quality` 1..=100 (higher = better/larger). Calls `progress(done, total)` after each frame.
/// Odd `width` / `height` are padded up to even.
#[allow(clippy::too_many_arguments)]
pub fn encode(width: u32, height: u32, fps: f64, quality: u8, background: [u8; 3], frames: &mut dyn Iterator<Item = image::RgbaImage>, total: usize, progress: &mut dyn FnMut(usize, usize)) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("Video size must be non-zero".into());
    }
    if !(fps.is_finite() && fps > 0.0) {
        return Err("Frame rate must be positive".into());
    }
    let (w, h) = (width.div_ceil(2) as usize * 2, height.div_ceil(2) as usize * 2);
    let quality = quality.clamp(1, 100) as usize;
    // quality 1..=100 -> quantizer 255..=20.
    let quantizer = 255 - (quality - 1) * 235 / 99;
    // Rational time base (frame duration) from fps, e.g. 29.97 -> 1000/29970.
    let fps_milli = (fps * 1000.0).round().max(1.0) as u64;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let enc = EncoderConfig {
        width: w,
        height: h,
        time_base: Rational::new(1000, fps_milli),
        bit_depth: 8,
        chroma_sampling: ChromaSampling::Cs420,
        chroma_sample_position: ChromaSamplePosition::Vertical,
        pixel_range: PixelRange::Limited,
        color_description: Some(ColorDescription { color_primaries: ColorPrimaries::BT709, transfer_characteristics: TransferCharacteristics::BT709, matrix_coefficients: MatrixCoefficients::BT709 }),
        low_latency: true,
        quantizer,
        min_quantizer: quantizer.saturating_sub(40) as u8,
        bitrate: 0,
        min_key_frame_interval: 1,
        max_key_frame_interval: (fps * 4.0).round().max(1.0) as u64,
        speed_settings: speed(),
        // Tiles are encoded in parallel; aim for one per core, each at least 256x256.
        tiles: threads.min((w / 256).max(1) * (h / 256).max(1)),
        ..Default::default()
    };
    let cfg = Config::new().with_encoder_config(enc).with_threads(threads);
    let mut ctx: Context<u8> = cfg.new_context().map_err(|e| format!("AV1 encoder setup failed: {e}"))?;
    let codec_private = ctx.container_sequence_header();

    let mut packets: Vec<(u64, bool, Vec<u8>)> = Vec::new();
    let mut sent = 0usize;
    let (cw, ch) = (w / 2, h / 2);
    let (mut y, mut u, mut v) = (vec![0u8; w * h], vec![0u8; cw * ch], vec![0u8; cw * ch]);
    for img in frames {
        if img.width() != width || img.height() != height {
            return Err(format!("Frame {} is {}x{}, expected {width}x{height}", sent + 1, img.width(), img.height()));
        }
        rgba_to_yuv(&img, background, w, h, &mut y, &mut u, &mut v);
        let mut frame = ctx.new_frame();
        frame.planes[0].copy_from_raw_u8(&y, w, 1);
        frame.planes[1].copy_from_raw_u8(&u, cw, 1);
        frame.planes[2].copy_from_raw_u8(&v, cw, 1);
        ctx.send_frame(frame).map_err(|e| format!("AV1 encoding failed: {e}"))?;
        drain(&mut ctx, &mut packets)?;
        sent += 1;
        progress(sent, total.max(sent));
    }
    if sent == 0 {
        return Err("No frames to encode".into());
    }
    ctx.flush();
    drain(&mut ctx, &mut packets)?;
    Ok(mux(w as u64, h as u64, fps, sent as u64, &codec_private, &packets))
}

/// rav1e's fastest preset, plus cuts that matter little for flat vector art:
/// minimal lookahead, no scene-cut search (keyframes come from the interval).
fn speed() -> SpeedSettings {
    let mut s = SpeedSettings::from_preset(SPEED);
    s.rdo_lookahead_frames = 1;
    s.scene_detection_mode = SceneDetectionSpeed::None;
    s.cdef = false;
    s
}

/// Collect every packet the encoder has ready.
fn drain(ctx: &mut Context<u8>, out: &mut Vec<(u64, bool, Vec<u8>)>) -> Result<(), String> {
    loop {
        match ctx.receive_packet() {
            Ok(p) => out.push((p.input_frameno, p.frame_type == FrameType::KEY, strip_temporal_delimiters(&p.data))),
            Err(EncoderStatus::Encoded) => {}
            Err(EncoderStatus::NeedMoreData | EncoderStatus::LimitReached) => return Ok(()),
            Err(e) => return Err(format!("AV1 encoding failed: {e}")),
        }
    }
}

/// Composite onto the background and convert to BT.709 limited-range 4:2:0
/// (chroma sited left, i.e. co-located horizontally, between rows vertically).
/// Fixed-point maths, split across threads in bands of row pairs.
fn rgba_to_yuv(img: &image::RgbaImage, bg: [u8; 3], w: usize, h: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    let cw = w / 2;
    let pairs = h / 2;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, pairs.max(1));
    let per = pairs.div_ceil(threads);
    std::thread::scope(|s| {
        let bands = y.chunks_mut(2 * w * per).zip(u.chunks_mut(cw * per)).zip(v.chunks_mut(cw * per));
        for (i, ((yb, ub), vb)) in bands.enumerate() {
            s.spawn(move || yuv_band(img, bg, w, i * per, yb, ub, vb));
        }
    });
}

// BT.709 in 16.16 fixed point, already scaled to limited range.
const KR: f64 = 0.2126;
const KB: f64 = 0.0722;
const KG: f64 = 1.0 - KR - KB;
const fn fx(c: f64) -> i32 {
    (c * 65536.0 + if c < 0.0 { -0.5 } else { 0.5 }) as i32
}
const Y_K: [i32; 3] = [fx(KR * 219.0 / 255.0), fx(KG * 219.0 / 255.0), fx(KB * 219.0 / 255.0)];
const U_K: [i32; 3] = [fx(-KR / (2.0 * (1.0 - KB)) * 224.0 / 255.0), fx(-KG / (2.0 * (1.0 - KB)) * 224.0 / 255.0), fx(0.5 * 224.0 / 255.0)];
const V_K: [i32; 3] = [fx(0.5 * 224.0 / 255.0), fx(-KG / (2.0 * (1.0 - KR)) * 224.0 / 255.0), fx(-KB / (2.0 * (1.0 - KR)) * 224.0 / 255.0)];

/// Convert the row pairs starting at `first_pair` into the band slices.
fn yuv_band(img: &image::RgbaImage, bg: [u8; 3], w: usize, first_pair: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    let (iw, ih) = (img.width() as usize, img.height() as usize);
    let raw = img.as_raw();
    let cw = w / 2;
    // Composited RGB of the two rows of the current pair.
    let mut rows = vec![[0i32; 3]; 2 * w];
    for p in 0..y.len() / (2 * w) {
        for r in 0..2 {
            let sy = (2 * (first_pair + p) + r).min(ih - 1);
            let src = &raw[sy * iw * 4..][..iw * 4];
            for px in 0..w {
                let q = &src[px.min(iw - 1) * 4..][..4];
                let a = q[3] as u32;
                let c = |i: usize| ((q[i] as u32 * a + bg[i] as u32 * (255 - a) + 127) / 255) as i32;
                let rgb = [c(0), c(1), c(2)];
                rows[r * w + px] = rgb;
                let l = Y_K[0] * rgb[0] + Y_K[1] * rgb[1] + Y_K[2] * rgb[2];
                y[(2 * p + r) * w + px] = ((l + (16 << 16) + (1 << 15)) >> 16).clamp(0, 255) as u8;
            }
        }
        for cx in 0..cw {
            // [1 2 1] horizontally around the even column, averaged over two rows (sum of weights 8).
            let x0 = 2 * cx;
            let (xl, xr) = (x0.saturating_sub(1), (x0 + 1).min(w - 1));
            let mut sum = [0i32; 3];
            for row in [&rows[..w], &rows[w..]] {
                for k in 0..3 {
                    sum[k] += row[xl][k] + 2 * row[x0][k] + row[xr][k];
                }
            }
            let dot = |m: [i32; 3]| (m[0] as i64 * sum[0] as i64 + m[1] as i64 * sum[1] as i64 + m[2] as i64 * sum[2] as i64 + (128i64 << 19) + (1 << 18)) >> 19;
            u[p * cw + cx] = dot(U_K).clamp(0, 255) as u8;
            v[p * cw + cx] = dot(V_K).clamp(0, 255) as u8;
        }
    }
}

/// Drop Temporal Delimiter OBUs (Matroska's AV1 mapping forbids them).
fn strip_temporal_delimiters(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let start = i;
        let header = data[i];
        let obu_type = (header >> 3) & 0x0f;
        let has_ext = header & 0x04 != 0;
        let has_size = header & 0x02 != 0;
        i += 1 + has_ext as usize;
        if !has_size {
            // No size field: the OBU runs to the end of the packet.
            if obu_type != 2 {
                out.extend_from_slice(&data[start..]);
            }
            break;
        }
        let (mut size, mut shift) = (0usize, 0);
        while i < data.len() {
            let b = data[i];
            i += 1;
            size |= ((b & 0x7f) as usize) << shift;
            shift += 7;
            if b & 0x80 == 0 || shift >= 56 {
                break;
            }
        }
        let end = (i + size).min(data.len());
        if obu_type != 2 {
            out.extend_from_slice(&data[start..end]);
        }
        i = end;
    }
    out
}

// --- Minimal EBML / Matroska writer ---

/// Append an EBML element: raw ID bytes, minimal-length size, body.
fn el(out: &mut Vec<u8>, id: u32, body: &[u8]) {
    let id_bytes = id.to_be_bytes();
    let skip = id_bytes.iter().position(|&b| b != 0).unwrap_or(3);
    out.extend_from_slice(&id_bytes[skip..]);
    let len = body.len() as u64;
    let n = (1..=8).find(|&n| len < (1u64 << (7 * n)) - 1).unwrap_or(8);
    let marked = len | (1u64 << (7 * n));
    out.extend_from_slice(&marked.to_be_bytes()[8 - n..]);
    out.extend_from_slice(body);
}

fn el_uint(out: &mut Vec<u8>, id: u32, v: u64) {
    let b = v.to_be_bytes();
    let skip = b.iter().position(|&x| x != 0).unwrap_or(7);
    el(out, id, &b[skip..]);
}

fn el_float(out: &mut Vec<u8>, id: u32, v: f64) {
    el(out, id, &v.to_be_bytes());
}

fn el_str(out: &mut Vec<u8>, id: u32, s: &str) {
    el(out, id, s.as_bytes());
}

/// Wrap the packets `(frame index, keyframe, data)` in a WebM container.
fn mux(w: u64, h: u64, fps: f64, frames: u64, codec_private: &[u8], packets: &[(u64, bool, Vec<u8>)]) -> Vec<u8> {
    let ts = |i: u64| (i as f64 * 1000.0 / fps).round() as u64;
    let mut out = Vec::new();

    let mut ebml = Vec::new();
    el_uint(&mut ebml, 0x4286, 1); // EBMLVersion
    el_uint(&mut ebml, 0x42F7, 1); // EBMLReadVersion
    el_uint(&mut ebml, 0x42F2, 4); // EBMLMaxIDLength
    el_uint(&mut ebml, 0x42F3, 8); // EBMLMaxSizeLength
    el_str(&mut ebml, 0x4282, "webm"); // DocType
    el_uint(&mut ebml, 0x4287, 4); // DocTypeVersion
    el_uint(&mut ebml, 0x4285, 2); // DocTypeReadVersion
    el(&mut out, 0x1A45DFA3, &ebml);

    let mut seg = Vec::new();
    let mut info = Vec::new();
    el_uint(&mut info, 0x2AD7B1, 1_000_000); // TimestampScale
    el_str(&mut info, 0x4D80, "mtzdVector"); // MuxingApp
    el_str(&mut info, 0x5741, "mtzdVector"); // WritingApp
    el_float(&mut info, 0x4489, frames as f64 * 1000.0 / fps); // Duration (ms)
    el(&mut seg, 0x1549A966, &info);

    let mut colour = Vec::new();
    el_uint(&mut colour, 0x55B1, 1); // MatrixCoefficients: BT.709
    el_uint(&mut colour, 0x55B7, 1); // ChromaSitingHorz: left
    el_uint(&mut colour, 0x55B8, 2); // ChromaSitingVert: half
    el_uint(&mut colour, 0x55B9, 1); // Range: broadcast (limited)
    el_uint(&mut colour, 0x55BA, 1); // TransferCharacteristics: BT.709
    el_uint(&mut colour, 0x55BB, 1); // Primaries: BT.709
    let mut video = Vec::new();
    el_uint(&mut video, 0xB0, w); // PixelWidth
    el_uint(&mut video, 0xBA, h); // PixelHeight
    el(&mut video, 0x55B0, &colour);
    let mut track = Vec::new();
    el_uint(&mut track, 0xD7, 1); // TrackNumber
    el_uint(&mut track, 0x73C5, 1); // TrackUID
    el_uint(&mut track, 0x83, 1); // TrackType: video
    el_uint(&mut track, 0x9C, 0); // FlagLacing
    el_str(&mut track, 0x86, "V_AV1"); // CodecID
    el(&mut track, 0x63A2, codec_private); // CodecPrivate (av1C)
    el_uint(&mut track, 0x23E383, (1e9 / fps).round() as u64); // DefaultDuration (ns)
    el(&mut track, 0xE0, &video);
    let mut tracks = Vec::new();
    el(&mut tracks, 0xAE, &track);
    el(&mut seg, 0x1654AE6B, &tracks);

    let mut cluster: Vec<u8> = Vec::new();
    let mut cluster_ts = 0u64;
    for (i, (frame, key, data)) in packets.iter().enumerate() {
        let t = ts(*frame);
        if i == 0 || *key || t.saturating_sub(cluster_ts) >= CLUSTER_MS {
            if i > 0 {
                el(&mut seg, 0x1F43B675, &cluster);
            }
            cluster.clear();
            cluster_ts = t;
            el_uint(&mut cluster, 0xE7, t); // Cluster Timestamp
        }
        // SimpleBlock: track number vint, int16 relative timestamp, flags.
        let rel = (t as i64 - cluster_ts as i64) as i16;
        let mut block = Vec::with_capacity(data.len() + 4);
        block.push(0x81);
        block.extend_from_slice(&rel.to_be_bytes());
        block.push(if *key { 0x80 } else { 0x00 });
        block.extend_from_slice(data);
        el(&mut cluster, 0xA3, &block);
    }
    if !packets.is_empty() {
        el(&mut seg, 0x1F43B675, &cluster);
    }
    el(&mut out, 0x18538067, &seg);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A coloured square moving over a transparent / gradient field.
    fn frames(w: u32, h: u32, n: usize) -> impl Iterator<Item = image::RgbaImage> {
        (0..n).map(move |i| {
            image::RgbaImage::from_fn(w, h, |x, y| {
                let sx = (i as u32 * 2) % w.saturating_sub(16).max(1);
                if x >= sx && x < sx + 16 && y >= 8 && y < 24 {
                    image::Rgba([255, 0, 0, 255])
                } else if y >= h / 2 {
                    image::Rgba([0, 0, 255, 255])
                } else if x < w / 4 {
                    image::Rgba([0, 255, 0, 255])
                } else {
                    image::Rgba([0, 0, 0, 0])
                }
            })
        })
    }

    fn run(w: u32, h: u32, n: usize) -> Vec<u8> {
        let mut calls = 0;
        let out = encode(w, h, 12.0, 70, [255, 255, 255], &mut frames(w, h, n), n, &mut |d, t| {
            calls += 1;
            assert_eq!((d, t), (calls, n));
        })
        .unwrap();
        assert_eq!(calls, n);
        out
    }

    #[test]
    fn webm_encodes_moving_square() {
        let out = run(64, 48, 24);
        assert_eq!(&out[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        assert!(out.windows(4).any(|w| w == b"webm"));
        assert!(out.windows(5).any(|w| w == b"V_AV1"));
        assert!(out.len() > 200);
        if let Ok(p) = std::env::var("WEBM_TEST_OUT") {
            std::fs::write(p, &out).unwrap();
        }
    }

    #[test]
    fn webm_pads_odd_size() {
        let out = run(33, 21, 3);
        assert_eq!(&out[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        // PixelWidth 34, PixelHeight 22.
        assert!(out.windows(3).any(|w| w == [0xB0, 0x81, 34]));
        assert!(out.windows(3).any(|w| w == [0xBA, 0x81, 22]));
    }

    #[test]
    fn webm_strips_temporal_delimiters() {
        // TD (type 2, size 0) followed by a padding OBU (type 15, size 1).
        let data = [0x12, 0x00, 0x7A, 0x01, 0xAB];
        assert_eq!(strip_temporal_delimiters(&data), vec![0x7A, 0x01, 0xAB]);
    }

    #[test]
    fn webm_yuv_matches_bt709() {
        // (rgba, expected y, u, v) for flat 4x4 frames; transparent composites onto the background.
        let cases = [([255, 255, 255, 255], 235, 128, 128), ([0, 0, 0, 255], 16, 128, 128), ([128, 128, 128, 255], 126, 128, 128), ([255, 0, 0, 255], 63, 102, 240), ([0, 0, 255, 255], 32, 240, 118), ([0, 0, 0, 0], 235, 128, 128)];
        for (px, ey, eu, ev) in cases {
            let img = image::RgbaImage::from_pixel(4, 4, image::Rgba(px));
            let (mut y, mut u, mut v) = ([0; 16], [0; 4], [0; 4]);
            rgba_to_yuv(&img, [255; 3], 4, 4, &mut y, &mut u, &mut v);
            assert!(y.iter().all(|&a| a == ey) && u.iter().all(|&a| a == eu) && v.iter().all(|&a| a == ev), "{px:?}: {y:?} {u:?} {v:?}");
        }
    }

    #[test]
    fn webm_rejects_empty() {
        let mut it = std::iter::empty();
        assert!(encode(16, 16, 10.0, 50, [0; 3], &mut it, 0, &mut |_, _| {}).is_err());
    }
}
