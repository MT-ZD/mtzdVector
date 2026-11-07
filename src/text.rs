//! Text layout: rich runs, font fallback, bidi, line breaking (UAX #14) with
//! hyphenation, columns, wrap exclusions and threads across frames.
//!
//! The story (content + spans) is cut into *items*: maximal byte ranges with
//! one style, one bidi level and one font face (the style's font, or a
//! fallback face for characters it lacks). Paragraphs are shaped once to
//! measure, broken into lines, then each line is shaped again in visual
//! order and placed.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use kurbo::{Affine, BezPath, Point, Rect, Shape as _};
use rustybuzz::ttf_parser;

use crate::document::{FontAxis, FontFeature, Rgba, RunStyle, TextAlign, TextCase, TextData};
use crate::fonts::{Caret, FontManager, Polyline};

/// A face in the layout's font table.
#[derive(Clone)]
pub struct FaceRef {
    pub id: fontdb::ID,
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    /// Font family and PostScript name (for real-text export).
    pub family: String,
    pub postscript: String,
}

/// One placed glyph (local coordinates, baseline origin).
#[derive(Clone, Debug)]
pub struct PlacedGlyph {
    pub id: u16,
    pub x: f64,
    pub y: f64,
    /// The characters this glyph stands for.
    pub text: String,
}

/// Glyphs of one face and style on one line, for real-text export.
#[derive(Clone)]
pub struct GlyphRun {
    pub face: usize,
    pub style: RunStyle,
    pub glyphs: Vec<PlacedGlyph>,
    /// Displayed text in logical order (case applied).
    pub text: String,
    /// Left end of the run on the baseline.
    pub origin: Point,
    pub width: f64,
    pub rtl: bool,
    /// Variation coordinates the face is drawn with.
    pub coords: Vec<(ttf_parser::Tag, f32)>,
    /// Synthesized small capitals or colour glyphs: exporters use outlines.
    pub needs_outline: bool,
}

/// A laid-out line box (local coordinates).
#[derive(Clone, Debug)]
pub struct LineBox {
    pub y0: f64,
    pub y1: f64,
    pub baseline: f64,
    pub x0: f64,
    pub x1: f64,
    pub hidden: bool,
}

pub struct TextLayout {
    /// Glyphs drawn with the shape's own fill.
    pub path: BezPath,
    /// Glyphs (and colour-glyph layers) with their own colour.
    pub colored: Vec<(BezPath, Rgba)>,
    pub carets: Vec<Caret>,
    pub lines: Vec<LineBox>,
    /// Height of the first line (single-style text: of every line).
    pub line_height: f64,
    pub bounds: Rect,
    /// Frame text has more text than fits in its frame (and its thread).
    pub overflow: bool,
    /// Path text: baseline point and angle for each caret (same order).
    pub on_path: Option<Vec<(Point, f64)>>,
    /// Story offsets this frame shows: `start..end` (`end` = where the next
    /// frame of a thread continues).
    pub start: usize,
    pub end: usize,
    pub faces: Vec<FaceRef>,
    pub runs: Vec<GlyphRun>,
    /// Underline and strike-through bars (local), with their run colour.
    pub decorations: Vec<(Rect, Option<Rgba>)>,
}

impl TextLayout {
    fn empty(lh: f64, frame: Option<[f64; 2]>) -> Self {
        TextLayout {
            path: BezPath::new(),
            colored: Vec::new(),
            carets: Vec::new(),
            lines: Vec::new(),
            line_height: lh,
            bounds: frame.map(|f| Rect::new(0.0, 0.0, f[0], f[1])).unwrap_or(Rect::new(0.0, 0.0, 1.0, lh)),
            overflow: false,
            on_path: None,
            start: 0,
            end: 0,
            faces: Vec::new(),
            runs: Vec::new(),
            decorations: Vec::new(),
        }
    }

}

/// Byte range with one style, bidi level and face.
#[derive(Clone, Debug)]
struct Item {
    range: Range<usize>,
    style: usize,
    face: usize,
    rtl: bool,
}

/// A shaped glyph before placement.
#[derive(Clone, Copy, Debug)]
struct Gl {
    id: u16,
    /// Story byte offset of the source character.
    cluster: usize,
    adv: f64,
    dx: f64,
    dy: f64,
    /// Font size of this glyph.
    size: f64,
}

/// Small-capital and case handling for one face.
struct CaseStyle {
    case: TextCase,
    smcp: bool,
    small: f64,
}

impl CaseStyle {
    fn new(face: &rustybuzz::Face, case: TextCase) -> Self {
        let smcp = case == TextCase::SmallCaps && face.tables().gsub.is_some_and(|g| g.features.into_iter().any(|f| f.tag == ttf_parser::Tag::from_bytes(b"smcp")));
        let small = match (face.x_height(), face.capital_height()) {
            (Some(x), Some(c)) if c > 0 => (x as f64 / c as f64 * 1.08).clamp(0.6, 0.85),
            _ => 0.75,
        };
        CaseStyle { case, smcp, small }
    }
}

fn tag(s: &str) -> Option<ttf_parser::Tag> {
    let b = s.as_bytes();
    (b.len() == 4).then(|| ttf_parser::Tag::from_bytes(&[b[0], b[1], b[2], b[3]]))
}

/// Variation coordinates for a style on a face (`wght` from the weight).
fn coords_for(face: &rustybuzz::Face, st: &RunStyle) -> Vec<(ttf_parser::Tag, f32)> {
    if !face.is_variable() {
        return Vec::new();
    }
    let axes: Vec<ttf_parser::Tag> = face.variation_axes().into_iter().map(|a| a.tag).collect();
    let mut out: Vec<(ttf_parser::Tag, f32)> = Vec::new();
    let wght = ttf_parser::Tag::from_bytes(b"wght");
    if axes.contains(&wght) {
        out.push((wght, st.weight as f32));
    }
    let ital = ttf_parser::Tag::from_bytes(b"ital");
    if st.italic && axes.contains(&ital) {
        out.push((ital, 1.0));
    }
    for FontAxis { tag: t, value } in &st.variations {
        if let Some(t) = tag(t).filter(|t| axes.contains(t)) {
            out.retain(|(x, _)| *x != t);
            out.push((t, *value));
        }
    }
    out
}

fn features_for(st: &RunStyle, cs: &CaseStyle) -> Vec<rustybuzz::Feature> {
    let mut out: Vec<rustybuzz::Feature> = Vec::new();
    if cs.smcp {
        out.push(rustybuzz::Feature::new(ttf_parser::Tag::from_bytes(b"smcp"), 1, ..));
    }
    for FontFeature { tag: t, value } in &st.features {
        if let Some(t) = tag(t) {
            out.push(rustybuzz::Feature::new(t, *value, ..));
        }
    }
    if st.letter_spacing.abs() > 0.01 {
        // Tracked text loses its ligatures, as in other design tools.
        for t in [b"liga", b"clig", b"dlig"] {
            if !st.features.iter().any(|f| f.tag.as_bytes() == t) {
                out.push(rustybuzz::Feature::new(ttf_parser::Tag::from_bytes(t), 0, ..));
            }
        }
    }
    out
}

/// Shape `range` of `story` with one face and style. Glyphs come in visual
/// order (right to left runs reversed, as rustybuzz returns them).
fn shape(face: &rustybuzz::Face, story: &str, range: Range<usize>, st: &RunStyle, rtl: bool, extra: Option<char>) -> Vec<Gl> {
    let cs = CaseStyle::new(face, st.case);
    let upem = face.units_per_em().max(1) as f64;
    let mut buf = rustybuzz::UnicodeBuffer::new();
    let text = &story[range.clone()];
    let mut small: HashMap<u32, bool> = HashMap::new();
    let mut word_start = range.start == 0 || story[..range.start].chars().next_back().is_none_or(|c| c.is_whitespace());
    for (i, ch) in text.char_indices() {
        let cl = (range.start + i) as u32;
        let mut push = |c: char| buf.add(c, cl);
        match cs.case {
            TextCase::AsTyped => push(ch),
            TextCase::Upper => ch.to_uppercase().for_each(push),
            TextCase::Lower => ch.to_lowercase().for_each(push),
            TextCase::Title if word_start => ch.to_uppercase().for_each(push),
            TextCase::Title => push(ch),
            TextCase::SmallCaps if cs.smcp => push(ch),
            TextCase::SmallCaps => {
                if ch.is_lowercase() {
                    small.insert(cl, true);
                }
                ch.to_uppercase().for_each(push);
            }
        }
        word_start = ch.is_whitespace() || matches!(ch, '-' | '/' | '(' | '"' | '\u{201C}');
    }
    if let Some(c) = extra {
        buf.add(c, range.end as u32);
    }
    buf.set_direction(if rtl { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
    buf.guess_segment_properties();
    let gb = rustybuzz::shape(face, &features_for(st, &cs), buf);
    gb.glyph_infos()
        .iter()
        .zip(gb.glyph_positions())
        .map(|(info, pos)| {
            let k = if small.get(&info.cluster).copied().unwrap_or(false) { cs.small } else { 1.0 };
            let size = st.size * k;
            let s = size / upem;
            Gl { id: info.glyph_id as u16, cluster: info.cluster as usize, adv: pos.x_advance as f64 * s, dx: pos.x_offset as f64 * s, dy: pos.y_offset as f64 * s, size }
        })
        .collect()
}

/// Story text and its style runs, resolved once per layout.
struct Story<'a> {
    td: &'a TextData,
    text: &'a str,
    styles: Vec<RunStyle>,
    /// Style index per byte range, in order.
    style_runs: Vec<(Range<usize>, usize)>,
}

impl<'a> Story<'a> {
    fn new(td: &'a TextData) -> Self {
        let mut styles: Vec<RunStyle> = Vec::new();
        let mut style_runs = Vec::new();
        for (r, st) in td.runs(0..td.content.len()) {
            let i = match styles.iter().position(|s| *s == st) {
                Some(i) => i,
                None => {
                    styles.push(st);
                    styles.len() - 1
                }
            };
            style_runs.push((r, i));
        }
        if styles.is_empty() {
            styles.push(td.base_style());
        }
        Story { td, text: &td.content, styles, style_runs }
    }

    fn style_at(&self, i: usize) -> usize {
        self.style_runs.iter().find(|(r, _)| r.start <= i && i < r.end).or(self.style_runs.last()).map(|(_, s)| *s).unwrap_or(0)
    }
}

/// Faces used by one layout (data kept alive while shaping).
struct FaceTable {
    refs: Vec<FaceRef>,
}

impl FaceTable {
    fn add(&mut self, fm: &mut FontManager, id: fontdb::ID) -> Option<usize> {
        if let Some(i) = self.refs.iter().position(|f| f.id == id) {
            return Some(i);
        }
        let (data, index) = fm.data_for(id)?;
        let info = fm.db.face(id)?;
        let family = info.families.first().map(|(n, _)| n.clone()).unwrap_or_default();
        let r = FaceRef { id, data, index, family, postscript: info.post_script_name.clone() };
        self.refs.push(r);
        Some(self.refs.len() - 1)
    }
}

/// Parse a face with the style's variations applied.
fn face_for<'d>(r: &'d FaceRef, st: &RunStyle) -> Option<(rustybuzz::Face<'d>, Vec<(ttf_parser::Tag, f32)>)> {
    let mut f = rustybuzz::Face::from_slice(&r.data, r.index)?;
    let coords = coords_for(&f, st);
    if !coords.is_empty() {
        let vars: Vec<rustybuzz::Variation> = coords.iter().map(|(t, v)| rustybuzz::Variation { tag: *t, value: *v }).collect();
        f.set_variations(&vars);
    }
    Some((f, coords))
}

/// Characters that stay in the face of the character before them.
fn sticks(c: char) -> bool {
    matches!(c, '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}' | '\u{1F3FB}'..='\u{1F3FF}' | '\u{20E3}')
        || unicode_mark(c)
        || c.is_whitespace()
        || c.is_control()
}

fn unicode_mark(c: char) -> bool {
    // Combining diacritical marks (the common blocks).
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
}

/// One paragraph cut into items (logical order) with measuring data.
struct Para {
    range: Range<usize>,
    items: Vec<Item>,
    /// Advance (px) up to each byte of the paragraph, logical order.
    prefix: Vec<f64>,
    /// Allowed break positions (story offsets), mandatory last.
    breaks: Vec<usize>,
    levels: Vec<unicode_bidi::Level>,
}

impl Para {
    fn width(&self, a: usize, b: usize) -> f64 {
        let o = self.range.start;
        self.prefix[(b - o).min(self.prefix.len() - 1)] - self.prefix[(a - o).min(self.prefix.len() - 1)]
    }
}

/// Lines of a paragraph before placement.
struct LineSpec {
    range: Range<usize>,
    /// Ends at a hyphenation point: draw a hyphen.
    hyphen: bool,
    seg: (f64, f64),
    y0: f64,
    height: f64,
    ascent: f64,
    last_in_para: bool,
    hidden: bool,
}

impl FontManager {
    /// Lay out a text block in local coordinates (origin = top-left).
    pub fn layout(&mut self, td: &TextData) -> TextLayout {
        // A continuation frame: paragraph attributes come from the story;
        // the frame, its columns, thread and wrap stay its own.
        let merged;
        let td = match &td.flow {
            Some(f) => {
                merged = TextData {
                    frame: td.frame,
                    columns: td.columns,
                    column_gap: td.column_gap,
                    next: td.next,
                    flow: td.flow.clone(),
                    exclusions: td.exclusions.clone(),
                    path: None,
                    content: String::new(),
                    spans: Vec::new(),
                    ..(*f.story).clone()
                };
                &merged
            }
            None => td,
        };
        let (story_td, start) = td.story();
        // Geometry comes from this frame, the text from the story.
        let story = Story::new(story_td);
        let lh0 = story_td.size * story_td.line_height;
        let mut faces = FaceTable { refs: Vec::new() };
        // Primary face per style.
        let mut primary: Vec<Option<usize>> = Vec::new();
        for st in &story.styles {
            self.ensure_available(&st.family, st.weight, st.italic);
            let id = self.query(&st.family, st.weight, st.italic);
            primary.push(id.and_then(|id| faces.add(self, id)));
        }
        if primary.iter().all(|p| p.is_none()) {
            return TextLayout::empty(lh0, td.frame);
        }
        if td.path.is_some() && td.flow.is_none() {
            return self.layout_on_path(td, &story, &mut faces, &primary);
        }
        let paras = self.paragraphs(&story, start, &mut faces, &primary);
        let mut out = TextLayout::empty(lh0, td.frame);
        out.start = start;
        place(td, &story, &paras, &mut faces, &primary, &mut out, self);
        out.faces = faces.refs;
        out
    }

    /// Itemize and measure the paragraphs of the story from `start`.
    fn paragraphs(&mut self, story: &Story, start: usize, faces: &mut FaceTable, primary: &[Option<usize>]) -> Vec<Para> {
        let text = story.text;
        let mut out = Vec::new();
        let mut a = start.min(text.len());
        // A continuation starting mid-paragraph begins at the paragraph's start
        // for bidi and line breaking, then skips to `start`.
        loop {
            let pstart = text[..a].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let b = text[a..].find('\n').map(|i| a + i).unwrap_or(text.len());
            let mut p = self.paragraph(story, pstart..b, faces, primary);
            if pstart < a {
                p.items.retain_mut(|it| {
                    it.range.start = it.range.start.max(a);
                    it.range.start < it.range.end
                });
                p.breaks.retain(|x| *x > a);
                p.range.start = a;
                let skip = a - pstart;
                p.prefix.drain(..skip);
                p.levels.drain(..skip);
            }
            out.push(p);
            if b >= text.len() {
                break;
            }
            a = b + 1;
        }
        out
    }

    fn paragraph(&mut self, story: &Story, range: Range<usize>, faces: &mut FaceTable, primary: &[Option<usize>]) -> Para {
        let text = &story.text[range.clone()];
        let bidi = unicode_bidi::BidiInfo::new(text, None);
        let levels = bidi.levels.clone();
        // Items: style × level × face.
        let mut items: Vec<Item> = Vec::new();
        let mut prev_face: Option<usize> = None;
        for (i, ch) in text.char_indices() {
            let at = range.start + i;
            let si = story.style_at(at);
            let st = &story.styles[si];
            let rtl = levels[i].is_rtl();
            let prim = primary[si].or(primary.iter().flatten().next().copied());
            let face = match prim {
                Some(p) if self.covers(faces.refs[p].id, ch) => Some(p),
                _ if sticks(ch) => prev_face.or(prim),
                _ => self.fallback(ch, st.weight, st.italic).and_then(|id| faces.add(self, id)).or(prim),
            };
            let Some(face) = face else { continue };
            prev_face = Some(face);
            let end = at + ch.len_utf8();
            match items.last_mut() {
                Some(it) if it.style == si && it.face == face && it.rtl == rtl && it.range.end == at => it.range.end = end,
                _ => items.push(Item { range: at..end, style: si, face, rtl }),
            }
        }
        // Measure: advances summed per source byte.
        let mut adv = vec![0.0f64; text.len() + 1];
        for it in &items {
            let st = &story.styles[it.style];
            let Some((face, _)) = face_for(&faces.refs[it.face], st) else { continue };
            for g in shape(&face, story.text, it.range.clone(), st, it.rtl, None) {
                let k = g.cluster.saturating_sub(range.start).min(text.len());
                adv[k] += g.adv + st.letter_spacing;
            }
        }
        let mut prefix = Vec::with_capacity(text.len() + 1);
        let mut acc = 0.0;
        for a in &adv {
            prefix.push(acc);
            acc += a;
        }
        let breaks: Vec<usize> = unicode_linebreak::linebreaks(text).map(|(i, _)| range.start + i).collect();
        Para { range, items, prefix, breaks, levels }
    }

    /// Does face `id` have a glyph for `c`?
    fn covers(&mut self, id: fontdb::ID, c: char) -> bool {
        if let Some(v) = self.coverage.get(&(id, c)) {
            return *v;
        }
        // A glyph with an outline (or colour layers): bitmap-only glyphs can't
        // be drawn as vectors.
        struct Nop;
        impl ttf_parser::OutlineBuilder for Nop {
            fn move_to(&mut self, _: f32, _: f32) {}
            fn line_to(&mut self, _: f32, _: f32) {}
            fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
            fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
            fn close(&mut self) {}
        }
        let v = self
            .db
            .with_face_data(id, |d, i| {
                let f = ttf_parser::Face::parse(d, i).ok()?;
                let g = f.glyph_index(c)?;
                (c.is_whitespace() || f.is_color_glyph(g) || f.outline_glyph(g, &mut Nop).is_some()).then_some(())
            })
            .flatten()
            .is_some();
        self.coverage.insert((id, c), v);
        v
    }

    /// A face for a character the style's font lacks (emoji, CJK, symbols…).
    fn fallback(&mut self, c: char, weight: u16, italic: bool) -> Option<fontdb::ID> {
        let key = (c, weight, italic);
        if let Some(v) = self.fallbacks.get(&key) {
            return *v;
        }
        let emoji = matches!(c, '\u{1F000}'..='\u{1FAFF}' | '\u{2600}'..='\u{27BF}' | '\u{2B00}'..='\u{2BFF}');
        let cjk = matches!(c, '\u{2E80}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}' | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}' | '\u{20000}'..='\u{3FFFF}');
        let preferred: &[&str] = if emoji {
            &["Noto Color Emoji", "Noto Emoji", "Twemoji", "Apple Color Emoji", "Segoe UI Emoji", "Symbola", "Noto Sans Symbols 2", "Noto Sans Symbols"]
        } else if cjk {
            &["Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans CJK KR", "Noto Sans CJK TC", "Source Han Sans", "WenQuanYi Micro Hei", "Droid Sans Fallback"]
        } else {
            &["Noto Sans", "DejaVu Sans", "Noto Sans Symbols 2", "Noto Sans Symbols", "Noto Sans Math", "Symbola", "FreeSans"]
        };
        let mut found = None;
        for fam in preferred {
            if let Some(id) = self.query_exact(fam, weight, italic) {
                if self.covers(id, c) {
                    found = Some(id);
                    break;
                }
            }
        }
        if found.is_none() {
            // Any installed face, regular styles first.
            let mut ids: Vec<(bool, fontdb::ID)> = self.db.faces().map(|f| (f.style != fontdb::Style::Normal || f.weight.0 != 400, f.id)).collect();
            ids.sort_by_key(|x| x.0);
            for (_, id) in ids {
                if self.covers(id, c) {
                    found = Some(id);
                    break;
                }
            }
        }
        self.fallbacks.insert(key, found);
        found
    }

    /// Text along a guide path: one line, glyph centres on the path.
    fn layout_on_path(&mut self, td: &TextData, story: &Story, faces: &mut FaceTable, primary: &[Option<usize>]) -> TextLayout {
        let lh = td.size * td.line_height;
        let paras = self.paragraphs(story, 0, faces, primary);
        // All paragraphs on one line (newlines read as spaces).
        let mut glyphs: Vec<(Gl, usize)> = Vec::new();
        for p in &paras {
            for it in visual_items(p, p.range.clone()) {
                let st = &story.styles[it.style];
                let Some((face, _)) = face_for(&faces.refs[it.face], st) else { continue };
                for g in shape(&face, story.text, it.range.clone(), st, it.rtl, None) {
                    glyphs.push((g, it.style));
                }
            }
        }
        let guide = crate::geom::pathdata_to_bez(td.path.as_ref().unwrap());
        let poly = Polyline::new(&guide);
        let total: f64 = glyphs.iter().map(|(g, s)| g.adv + story.styles[*s].letter_spacing).sum::<f64>() - glyphs.last().map(|(_, s)| story.styles[*s].letter_spacing).unwrap_or(0.0);
        let len = poly.length();
        let start = td.path_offset
            + match td.align {
                TextAlign::Left | TextAlign::Justify => 0.0,
                TextAlign::Center => (len - total) / 2.0,
                TextAlign::Right => len - total,
            };
        let mut out = TextLayout::empty(lh, None);
        let mut carets: Vec<(usize, f64)> = Vec::new();
        let mut x = 0.0;
        let face_of = |cluster: usize| paras.iter().flat_map(|p| p.items.iter()).find(|it| it.range.start <= cluster && cluster < it.range.end).map(|it| it.face).unwrap_or(0);
        for (g, si) in &glyphs {
            let st = &story.styles[*si];
            carets.push((g.cluster, x));
            let mid = start + x + g.adv / 2.0;
            let (p, ang) = poly.at(mid);
            let place = Affine::translate(p.to_vec2()) * Affine::rotate(ang) * Affine::translate((-g.adv / 2.0 + g.dx, -g.dy - st.baseline));
            let fr = &faces.refs[face_of(g.cluster)];
            if let Some((face, _)) = face_for(fr, st) {
                let mut glyph = BezPath::new();
                outline(&face, g.id, Affine::new([g.size / face.units_per_em() as f64, 0.0, 0.0, -g.size / face.units_per_em() as f64, 0.0, 0.0]), &mut glyph);
                let target = paint_target(&mut out, st.fill);
                for el in glyph.elements() {
                    target.push(place * *el);
                }
            }
            x += g.adv + st.letter_spacing;
        }
        let text_len = story.text.len();
        carets.push((text_len, total.max(0.0)));
        carets.sort_by_key(|c| c.0);
        carets.dedup_by_key(|c| c.0);
        out.carets = carets.iter().map(|(i, x)| Caret { index: *i, x: *x, line: 0 }).collect();
        out.on_path = Some(out.carets.iter().map(|c| poly.at(start + c.x)).collect());
        let mut all = out.path.clone();
        for (p, _) in &out.colored {
            all.extend(p.elements().iter().copied());
        }
        out.bounds = match (crate::geom::path_bounds(&all), crate::geom::path_bounds(&guide)) {
            (Some(a), Some(b)) => a.union(b),
            (a, b) => a.or(b).unwrap_or(Rect::new(0.0, 0.0, 1.0, lh)),
        };
        out.lines = vec![LineBox { y0: 0.0, y1: lh, baseline: lh * 0.8, x0: 0.0, x1: total, hidden: false }];
        out.end = text_len;
        out.faces = std::mem::take(&mut faces.refs);
        out
    }
}

/// Where glyphs of colour `fill` go.
fn paint_target(out: &mut TextLayout, fill: Option<Rgba>) -> &mut BezPath {
    match fill {
        None => &mut out.path,
        Some(c) => {
            let i = match out.colored.iter().position(|(_, k)| *k == c) {
                Some(i) => i,
                None => {
                    out.colored.push((BezPath::new(), c));
                    out.colored.len() - 1
                }
            };
            &mut out.colored[i].0
        }
    }
}

struct OutlineSink<'a> {
    path: &'a mut BezPath,
    xf: Affine,
    open: bool,
}

impl ttf_parser::OutlineBuilder for OutlineSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.xf * Point::new(x as f64, y as f64));
        self.open = true;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.xf * Point::new(x as f64, y as f64));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path.quad_to(self.xf * Point::new(x1 as f64, y1 as f64), self.xf * Point::new(x as f64, y as f64));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path.curve_to(self.xf * Point::new(x1 as f64, y1 as f64), self.xf * Point::new(x2 as f64, y2 as f64), self.xf * Point::new(x as f64, y as f64));
    }
    fn close(&mut self) {
        self.path.close_path();
        self.open = false;
    }
}

fn outline(face: &rustybuzz::Face, gid: u16, xf: Affine, path: &mut BezPath) {
    let mut sink = OutlineSink { path, xf, open: false };
    face.outline_glyph(ttf_parser::GlyphId(gid), &mut sink);
}

/// Colour glyph layers (COLR v0/v1 solid paints) in local coordinates.
fn color_layers(face: &rustybuzz::Face, gid: u16, xf: Affine, foreground: Rgba) -> Option<Vec<(BezPath, Option<Rgba>)>> {
    if !face.is_color_glyph(ttf_parser::GlyphId(gid)) {
        return None;
    }
    struct Painter<'f, 'a> {
        face: &'f rustybuzz::Face<'a>,
        xf: Vec<Affine>,
        current: BezPath,
        out: Vec<(BezPath, Option<Rgba>)>,
        fg: ttf_parser::RgbaColor,
    }
    impl<'a> ttf_parser::colr::Painter<'a> for Painter<'_, 'a> {
        fn outline_glyph(&mut self, glyph_id: ttf_parser::GlyphId) {
            self.current = BezPath::new();
            let xf = *self.xf.last().unwrap();
            let mut sink = OutlineSink { path: &mut self.current, xf, open: false };
            self.face.outline_glyph(glyph_id, &mut sink);
        }
        fn paint(&mut self, paint: ttf_parser::colr::Paint<'a>) {
            use ttf_parser::colr::Paint;
            let c = match paint {
                Paint::Solid(c) => c,
                Paint::LinearGradient(g) => g.stops(0, &[]).next().map(|s| s.color).unwrap_or(self.fg),
                Paint::RadialGradient(g) => g.stops(0, &[]).next().map(|s| s.color).unwrap_or(self.fg),
                Paint::SweepGradient(g) => g.stops(0, &[]).next().map(|s| s.color).unwrap_or(self.fg),
            };
            let col = (c != self.fg).then(|| Rgba { r: c.red as f32 / 255.0, g: c.green as f32 / 255.0, b: c.blue as f32 / 255.0, a: c.alpha as f32 / 255.0 });
            self.out.push((std::mem::take(&mut self.current), col));
        }
        fn push_clip(&mut self) {}
        fn push_clip_box(&mut self, _: ttf_parser::colr::ClipBox) {}
        fn pop_clip(&mut self) {}
        fn push_layer(&mut self, _: ttf_parser::colr::CompositeMode) {}
        fn pop_layer(&mut self) {}
        fn push_transform(&mut self, t: ttf_parser::Transform) {
            let top = *self.xf.last().unwrap();
            self.xf.push(top * Affine::new([t.a as f64, t.b as f64, t.c as f64, t.d as f64, t.e as f64, t.f as f64]));
        }
        fn pop_transform(&mut self) {
            if self.xf.len() > 1 {
                self.xf.pop();
            }
        }
    }
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let fg = ttf_parser::RgbaColor::new(to8(foreground.r), to8(foreground.g), to8(foreground.b), to8(foreground.a));
    let mut p = Painter { face, xf: vec![xf], current: BezPath::new(), out: Vec::new(), fg };
    face.paint_color_glyph(ttf_parser::GlyphId(gid), 0, fg, &mut p)?;
    Some(p.out)
}

/// Items of `line` (a range of paragraph `p`) in visual order, clipped.
fn visual_items(p: &Para, line: Range<usize>) -> Vec<Item> {
    let o = p.range.start;
    let mut runs: Vec<(Range<usize>, bool)> = Vec::new();
    // Level runs within the line, reordered (rule L2).
    let (a, b) = (line.start - o, line.end - o);
    if a >= b {
        return Vec::new();
    }
    let levels = &p.levels[a..b];
    let mut lruns: Vec<(Range<usize>, unicode_bidi::Level)> = Vec::new();
    for (i, lv) in levels.iter().enumerate() {
        match lruns.last_mut() {
            Some((r, l)) if *l == *lv => r.end = a + i + 1,
            _ => lruns.push((a + i..a + i + 1, *lv)),
        }
    }
    let max = lruns.iter().map(|r| r.1.number()).max().unwrap_or(0);
    let min_odd = lruns.iter().map(|r| r.1.number()).filter(|n| n % 2 == 1).min().unwrap_or(max + 1);
    let mut order: Vec<usize> = (0..lruns.len()).collect();
    let mut lvl = max;
    while lvl >= min_odd && lvl > 0 {
        let mut i = 0;
        while i < order.len() {
            if lruns[order[i]].1.number() >= lvl {
                let mut j = i;
                while j < order.len() && lruns[order[j]].1.number() >= lvl {
                    j += 1;
                }
                order[i..j].reverse();
                i = j;
            } else {
                i += 1;
            }
        }
        lvl -= 1;
    }
    for k in order {
        let (r, lv) = &lruns[k];
        runs.push(((r.start + o)..(r.end + o), lv.is_rtl()));
    }
    let mut out = Vec::new();
    for (r, rtl) in runs {
        let mut part: Vec<Item> = p
            .items
            .iter()
            .filter(|it| it.range.start < r.end && it.range.end > r.start)
            .map(|it| Item { range: it.range.start.max(r.start)..it.range.end.min(r.end), ..it.clone() })
            .collect();
        if rtl {
            part.reverse();
        }
        out.extend(part);
    }
    out
}

/// Hyphenation dictionaries, loaded on first use.
fn hyphenator(lang: &str) -> Option<&'static hyphenation::Standard> {
    use hyphenation::Load;
    use std::sync::{Mutex, OnceLock};
    static DICTS: OnceLock<Mutex<HashMap<String, Option<&'static hyphenation::Standard>>>> = OnceLock::new();
    let code = lang.to_lowercase();
    let mut map = DICTS.get_or_init(Default::default).lock().ok()?;
    if let Some(d) = map.get(&code) {
        return *d;
    }
    let language = hyphenation::Language::try_from_code(&code)
        .or_else(|| hyphenation::Language::try_from_code(code.split('-').next().unwrap_or("")))
        .or_else(|| match code.split('-').next() {
            Some("en") => Some(hyphenation::Language::EnglishUS),
            Some("de") => Some(hyphenation::Language::German1996),
            _ => None,
        });
    let d = language.and_then(|l| hyphenation::Standard::from_embedded(l).ok()).map(|d| &*Box::leak(Box::new(d)));
    map.insert(code, d);
    d
}

/// Hyphenation points (story offsets) inside the word at `a..b`.
fn hyphen_points(text: &str, a: usize, b: usize, lang: &str) -> Vec<usize> {
    use hyphenation::Hyphenator;
    let word = &text[a..b];
    let trimmed = word.trim_end_matches(|c: char| !c.is_alphanumeric());
    let lead = trimmed.len() - trimmed.trim_start_matches(|c: char| !c.is_alphanumeric()).len();
    let core = &trimmed[lead..];
    if core.chars().count() < 5 {
        return Vec::new();
    }
    let Some(d) = hyphenator(lang) else { return Vec::new() };
    d.hyphenate(core).breaks.into_iter().map(|i| a + lead + i).collect()
}

/// Horizontal free segments of `col` (x0, x1) in the band `y0..y1` after
/// removing exclusion outlines.
fn free_segments(col: (f64, f64), y0: f64, y1: f64, excl: &[Vec<Vec<Point>>], min_w: f64) -> Vec<(f64, f64)> {
    let mut blocked: Vec<(f64, f64)> = Vec::new();
    for shape in excl {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for poly in shape {
            for w in poly.windows(2) {
                let (p, q) = (w[0], w[1]);
                // Clip the edge to the band and take its x extent.
                let (ya, yb) = (p.y.min(q.y), p.y.max(q.y));
                if yb < y0 || ya > y1 {
                    continue;
                }
                let at = |y: f64| if (q.y - p.y).abs() < 1e-9 { p.x } else { p.x + (q.x - p.x) * ((y - p.y) / (q.y - p.y)).clamp(0.0, 1.0) };
                for x in [at(y0.max(ya)), at(y1.min(yb))] {
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
        }
        if lo < hi {
            blocked.push((lo, hi));
        }
    }
    blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut x = col.0;
    for (lo, hi) in blocked {
        if lo > x {
            out.push((x, lo.min(col.1)));
        }
        x = x.max(hi);
        if x >= col.1 {
            break;
        }
    }
    if x < col.1 {
        out.push((x, col.1));
    }
    out.retain(|(a, b)| b - a >= min_w);
    out
}

/// Break the paragraphs into lines and place them in the frame's columns.
fn place(td: &TextData, story: &Story, paras: &[Para], faces: &mut FaceTable, primary: &[Option<usize>], out: &mut TextLayout, fm: &mut FontManager) {
    let st0 = story.td;
    let lh_mult = st0.line_height;
    let frame = td.frame;
    let ncol = if frame.is_some() { td.columns.max(1) } else { 1 } as usize;
    let gap = td.column_gap.max(0.0);
    let fw = frame.map(|f| f[0].max(1.0));
    let fh = frame.map(|f| f[1].max(1.0));
    let colw = fw.map(|w| ((w - gap * (ncol as f64 - 1.0)) / ncol as f64).max(1.0));
    let threaded = td.next.is_some();
    let lang = st0.lang.clone().unwrap_or_else(|| "en-us".into());
    let excl: Vec<Vec<Vec<Point>>> = td
        .exclusions
        .as_ref()
        .map(|ex| {
            ex.iter()
                .map(|p| {
                    let mut polys: Vec<Vec<Point>> = Vec::new();
                    kurbo::flatten(p.iter(), 0.5, |el| match el {
                        kurbo::PathEl::MoveTo(q) => polys.push(vec![q]),
                        kurbo::PathEl::LineTo(q) => {
                            if let Some(l) = polys.last_mut() {
                                l.push(q)
                            }
                        }
                        kurbo::PathEl::ClosePath => {
                            if let Some(l) = polys.last_mut() {
                                if let Some(f) = l.first().copied() {
                                    l.push(f)
                                }
                            }
                        }
                        _ => {}
                    });
                    polys
                })
                .collect()
        })
        .unwrap_or_default();
    // Line metrics of a style: (height, ascent).
    let mut metric_cache: HashMap<usize, (f64, f64, f64)> = HashMap::new();
    let mut metrics = |si: usize, faces: &FaceTable| -> (f64, f64, f64) {
        *metric_cache.entry(si).or_insert_with(|| {
            let st = &story.styles[si];
            let fi = primary[si].or(primary.iter().flatten().next().copied()).unwrap_or(0);
            let Some((f, _)) = face_for(&faces.refs[fi], st) else { return (st.size * lh_mult, st.size * 0.8, st.size * 0.2) };
            let s = st.size / f.units_per_em().max(1) as f64;
            (st.size * lh_mult, f.ascender() as f64 * s, -(f.descender() as f64) * s)
        })
    };
    let line_metrics = |range: &Range<usize>, metrics: &mut dyn FnMut(usize, &FaceTable) -> (f64, f64, f64), faces: &FaceTable| -> (f64, f64) {
        let mut sis: Vec<usize> = story.style_runs.iter().filter(|(r, _)| r.start < range.end.max(range.start + 1) && r.end > range.start).map(|(_, s)| *s).collect();
        if sis.is_empty() {
            sis.push(story.style_at(range.start));
        }
        let (mut h, mut asc, mut desc) = (0.0f64, 0.0f64, 0.0f64);
        for si in sis {
            let (lh, a, d) = metrics(si, faces);
            h = h.max(lh);
            asc = asc.max(a);
            desc = desc.max(d);
        }
        let top = (h - (asc + desc)) / 2.0;
        (h, top + asc)
    };

    // Break and place.
    let mut specs: Vec<LineSpec> = Vec::new();
    let (mut col, mut y) = (0usize, 0.0f64);
    let mut stopped_at: Option<usize> = None;
    let mut hidden = false;
    'paras: for (pi, p) in paras.iter().enumerate() {
        let mut s = p.range.start;
        let pend = p.range.end;
        loop {
            // Height estimate from the line's first style.
            let (mut h, mut asc) = line_metrics(&(s..(s + 1).min(pend.max(s))), &mut metrics, faces);
            let mut segs: Vec<(f64, f64)>;
            loop {
                if let (Some(cw), Some(fh)) = (colw, fh) {
                    if y + h > fh + 1e-6 && !(y == 0.0 && specs.iter().all(|l| l.hidden)) {
                        // Next column, or past the frame.
                        if col + 1 < ncol {
                            col += 1;
                            y = 0.0;
                            continue;
                        }
                        if threaded {
                            stopped_at = Some(s);
                            break 'paras;
                        }
                        hidden = true;
                    }
                    let x0 = col as f64 * (cw + gap);
                    segs = if hidden || excl.is_empty() { vec![(x0, x0 + cw)] } else { free_segments((x0, x0 + cw), y, y + h, &excl, (st0.size * 2.0).min(cw)) };
                    if segs.is_empty() {
                        // Band fully blocked: move down.
                        y += (h / 4.0).max(1.0);
                        continue;
                    }
                } else {
                    segs = vec![(0.0, f64::INFINITY)];
                }
                break;
            }
            // Fill the segments of this band left to right.
            let mut first = true;
            for (si, seg) in segs.iter().enumerate() {
                if s >= pend && !(first && s == pend) {
                    break;
                }
                let w = seg.1 - seg.0;
                let (e, hyphen) = if s >= pend { (pend, false) } else { break_line(story, p, s, w, st0.hyphenate && fw.is_some(), &lang, &mut *fm, faces, primary) };
                let range = s..e;
                if si == 0 {
                    let (h2, a2) = line_metrics(&range, &mut metrics, faces);
                    if h2 > h {
                        h = h2;
                        asc = a2;
                    } else {
                        asc = a2.max(asc.min(a2));
                    }
                }
                let last = e >= pend;
                specs.push(LineSpec { range, hyphen, seg: *seg, y0: y, height: h, ascent: asc, last_in_para: last, hidden });
                s = e;
                first = false;
                if last {
                    break;
                }
            }
            y += h;
            if s >= pend {
                break;
            }
        }
        if pi + 1 < paras.len() {
            y += st0.paragraph_spacing;
        }
    }
    out.end = stopped_at.unwrap_or(story.text.len());
    out.overflow = stopped_at.is_some() || specs.iter().any(|l| l.hidden);
    if threaded {
        // The rest continues in the next frame: only the last one overflows.
        out.overflow = false;
    }

    // Shape each line (visual order) and measure it.
    struct Shaped {
        runs: Vec<(Item, Vec<Gl>)>,
        width: f64,
        trimmed_end: usize,
        spaces: usize,
    }
    let mut shaped_lines: Vec<Shaped> = Vec::new();
    for l in &specs {
        let p = paras.iter().find(|p| p.range.start <= l.range.start && l.range.start <= p.range.end).unwrap();
        // Trailing spaces don't count for alignment.
        let trimmed_end = l.range.start + story.text[l.range.clone()].trim_end().len();
        let items = visual_items(p, l.range.clone());
        let mut runs: Vec<(Item, Vec<Gl>)> = Vec::new();
        // The hyphen goes after the logically last item.
        let last_logical = items.iter().enumerate().max_by_key(|(_, it)| it.range.end).map(|(k, _)| k);
        for (k, it) in items.iter().enumerate() {
            let st = &story.styles[it.style];
            let Some((face, _)) = face_for(&faces.refs[it.face], st) else { continue };
            let extra = (l.hyphen && Some(k) == last_logical).then_some('-');
            runs.push((it.clone(), shape(&face, story.text, it.range.clone(), st, it.rtl, extra)));
        }
        let counted = |g: &Gl| g.cluster < trimmed_end || g.cluster >= l.range.end;
        let width: f64 = runs.iter().map(|(it, gs)| gs.iter().filter(|g| counted(g)).map(|g| g.adv + story.styles[it.style].letter_spacing).sum::<f64>()).sum::<f64>()
            - runs.last().map(|(it, _)| story.styles[it.style].letter_spacing).unwrap_or(0.0);
        let spaces = runs.iter().flat_map(|(_, gs)| gs.iter()).filter(|g| g.cluster < trimmed_end && story.text[g.cluster..].starts_with(' ')).count();
        shaped_lines.push(Shaped { runs, width: width.max(0.0), trimmed_end, spaces });
    }
    let max_w = shaped_lines.iter().map(|s| s.width).fold(0.0, f64::max);

    // Draw.
    for (li, (l, sl)) in specs.iter().zip(&shaped_lines).enumerate() {
        let width = sl.width;
        let trimmed_end = sl.trimmed_end;
        let avail = if l.seg.1.is_finite() { l.seg.1 - l.seg.0 } else { max_w };
        let justify = td.align == TextAlign::Justify && !l.last_in_para && sl.spaces > 0 && l.seg.1.is_finite();
        let dx = match td.align {
            TextAlign::Left | TextAlign::Justify => 0.0,
            TextAlign::Center => (avail - width) / 2.0,
            TextAlign::Right => avail - width,
        };
        let extra_space = if justify { ((avail - width) / sl.spaces as f64).max(0.0) } else { 0.0 };
        let x_origin = if l.seg.1.is_finite() { l.seg.0 } else { 0.0 } + dx;
        let baseline = l.y0 + l.ascent;
        let mut x = x_origin;
        let mut carets: Vec<(usize, f64)> = Vec::new();
        for (it, gs) in &sl.runs {
            let st = &story.styles[it.style];
            let fr = faces.refs[it.face].clone();
            let Some((face, coords)) = face_for(&fr, st) else { continue };
            let upem = face.units_per_em().max(1) as f64;
            let run_x0 = x;
            let mut run = GlyphRun { face: it.face, style: st.clone(), glyphs: Vec::new(), text: String::new(), origin: Point::new(x, baseline - st.baseline), width: 0.0, rtl: it.rtl, coords: coords.clone(), needs_outline: false };
            // Characters each glyph stands for: its cluster up to the next one.
            let mut clusters: Vec<usize> = gs.iter().map(|g| g.cluster).collect();
            clusters.sort_unstable();
            clusters.dedup();
            let mut seen: Vec<usize> = Vec::new();
            for g in gs {
                carets.push((g.cluster, if it.rtl { x + g.adv } else { x }));
                let gx = x + g.dx;
                let gy = baseline - g.dy - st.baseline;
                if !l.hidden {
                    let s = g.size / upem;
                    let xf = Affine::new([s, 0.0, 0.0, -s, gx, gy]);
                    match color_layers(&face, g.id, xf, st.fill.unwrap_or(Rgba::BLACK)) {
                        Some(layers) => {
                            run.needs_outline = true;
                            for (lp, c) in layers {
                                let t = paint_target(out, c.or(st.fill));
                                t.extend(lp.elements().iter().copied());
                            }
                        }
                        None => {
                            let t = paint_target(out, st.fill);
                            outline(&face, g.id, xf, t);
                        }
                    }
                }
                let text = if g.cluster >= l.range.end {
                    "-".to_string()
                } else if seen.contains(&g.cluster) {
                    String::new()
                } else {
                    seen.push(g.cluster);
                    let end = clusters.iter().copied().find(|c| *c > g.cluster).unwrap_or(it.range.end).min(it.range.end).max(g.cluster);
                    story.text[g.cluster..end].to_string()
                };
                if (g.size - st.size).abs() > 1e-9 {
                    run.needs_outline = true;
                }
                run.glyphs.push(PlacedGlyph { id: g.id, x: gx, y: gy, text });
                x += g.adv + st.letter_spacing;
                if g.cluster < trimmed_end && story.text[g.cluster..].starts_with(' ') {
                    x += extra_space;
                }
            }
            // Displayed text of the run (logical order, case applied).
            let raw = &story.text[it.range.clone()];
            run.text = match st.case {
                TextCase::Upper => raw.to_uppercase(),
                TextCase::Lower => raw.to_lowercase(),
                _ => raw.to_string(),
            };
            if run.glyphs.last().is_some_and(|g| g.text == "-" && g.x >= 0.0) && l.hyphen {
                run.text.push('-');
            }
            run.width = x - run_x0;
            // Underline and strike-through.
            if !l.hidden && (st.underline || st.strike) && x > run_x0 {
                let s = st.size / upem;
                let thick_default = st.size / 16.0;
                let x_end = x - st.letter_spacing.max(0.0);
                let mut deco = |pos: f64, thick: f64| {
                    let r = Rect::new(run_x0, baseline - st.baseline - pos - thick / 2.0, x_end, baseline - st.baseline - pos + thick / 2.0);
                    out.decorations.push((r, st.fill));
                    let t = paint_target(out, st.fill);
                    t.extend(r.to_path(0.1).elements().iter().copied());
                };
                if st.underline {
                    let m = face.underline_metrics();
                    deco(m.map(|m| m.position as f64 * s).unwrap_or(-st.size * 0.1), m.map(|m| m.thickness as f64 * s).unwrap_or(thick_default).max(0.5));
                }
                if st.strike {
                    let m = face.strikeout_metrics();
                    deco(m.map(|m| m.position as f64 * s).unwrap_or(st.size * 0.3), m.map(|m| m.thickness as f64 * s).unwrap_or(thick_default).max(0.5));
                }
            }
            if !l.hidden && !run.glyphs.is_empty() {
                out.runs.push(run);
            }
        }
        // End-of-line caret.
        let last_ls = sl.runs.last().map(|(it, _)| story.styles[it.style].letter_spacing).unwrap_or(0.0);
        carets.push((l.range.end, if sl.runs.is_empty() { x } else { x - last_ls }));
        carets.retain(|c| c.0 <= l.range.end);
        carets.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        carets.dedup_by_key(|c| c.0);
        for (i, cx) in carets {
            out.carets.push(Caret { index: i, x: cx, line: li });
        }
        out.lines.push(LineBox { y0: l.y0, y1: l.y0 + l.height, baseline, x0: x_origin, x1: x_origin + width, hidden: l.hidden });
    }
    if specs.is_empty() {
        let (h, asc) = line_metrics(&(out.start..out.start), &mut metrics, faces);
        out.lines.push(LineBox { y0: 0.0, y1: h, baseline: asc, x0: 0.0, x1: 0.0, hidden: false });
        out.carets.push(Caret { index: out.start, x: 0.0, line: 0 });
    }
    out.line_height = out.lines.first().map(|l| l.y1 - l.y0).unwrap_or(out.line_height);
    let total_h = specs.iter().map(|l| l.y0 + l.height).fold(0.0, f64::max).max(out.line_height);
    out.bounds = match frame {
        Some(f) => Rect::new(0.0, 0.0, f[0].max(1.0), f[1].max(1.0)),
        None => Rect::new(0.0, 0.0, max_w.max(1.0), total_h),
    };
}

/// End of the line starting at `s` that fits `w` (and whether it ends at a
/// hyphenation point).
#[allow(clippy::too_many_arguments)]
fn break_line(story: &Story, p: &Para, s: usize, w: f64, hyphenate: bool, lang: &str, fm: &mut FontManager, faces: &mut FaceTable, primary: &[Option<usize>]) -> (usize, bool) {
    let text = story.text;
    let pend = p.range.end;
    let trim = |b: usize| -> usize { s + text[s..b].trim_end().len() };
    if !w.is_finite() || p.width(s, trim(pend)) <= w {
        return (pend, false);
    }
    let mut best = None;
    for &b in p.breaks.iter().filter(|b| **b > s) {
        if p.width(s, trim(b)) <= w + 1e-6 {
            best = Some(b);
        } else {
            break;
        }
    }
    // The word that didn't fit (from the last fitting break).
    let word_start = best.unwrap_or(s);
    if hyphenate {
        let word_end = p.breaks.iter().copied().find(|b| *b > word_start).unwrap_or(pend);
        let word_end_core = word_start + text[word_start..word_end].trim_end().len();
        let si = story.style_at(word_start);
        let st = &story.styles[si];
        let hyph_w = primary[si]
            .and_then(|fi| face_for(&faces.refs[fi], st))
            .map(|(f, _)| shape(&f, "-", 0..1, st, false, None).iter().map(|g| g.adv).sum::<f64>())
            .unwrap_or(st.size * 0.3);
        let _ = fm;
        for h in hyphen_points(text, word_start, word_end_core, lang).into_iter().rev() {
            if h > s && p.width(s, h) + hyph_w <= w + 1e-6 {
                return (h, true);
            }
        }
    }
    if let Some(b) = best {
        return (b, false);
    }
    // A single word wider than the line: break inside it (at least one char).
    let mut e = s;
    for (k, ch) in text[s..pend].char_indices() {
        let next = s + k + ch.len_utf8();
        if p.width(s, next) > w && e > s {
            break;
        }
        e = next;
    }
    (e.max(s + text[s..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)).min(pend), false)
}

/// A variable-font axis (for the typography panel).
#[derive(Clone, Debug)]
pub struct AxisInfo {
    pub tag: String,
    pub name: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

/// OpenType features worth offering, with readable names. Features on by
/// default in shapers are marked.
pub const FEATURES: &[(&str, &str, bool)] = &[
    ("liga", "Standard ligatures", true),
    ("dlig", "Discretionary ligatures", false),
    ("calt", "Contextual alternates", true),
    ("kern", "Kerning", true),
    ("onum", "Oldstyle figures", false),
    ("lnum", "Lining figures", false),
    ("tnum", "Tabular figures", false),
    ("pnum", "Proportional figures", false),
    ("frac", "Fractions", false),
    ("zero", "Slashed zero", false),
    ("sups", "Superscript", false),
    ("subs", "Subscript", false),
    ("ordn", "Ordinals", false),
    ("swsh", "Swashes", false),
    ("salt", "Stylistic alternates", false),
    ("case", "Case-sensitive forms", false),
    ("c2sc", "Capitals to small caps", false),
];

impl FontManager {
    fn parsed<R>(&mut self, family: &str, weight: u16, italic: bool, f: impl FnOnce(&ttf_parser::Face) -> R) -> Option<R> {
        let id = self.query(family, weight, italic)?;
        let (data, index) = self.data_for(id)?;
        let face = ttf_parser::Face::parse(&data, index).ok()?;
        Some(f(&face))
    }

    /// Feature tags the font's layout tables offer (GSUB and GPOS).
    pub fn font_features(&mut self, family: &str, weight: u16, italic: bool) -> Vec<String> {
        self.parsed(family, weight, italic, |face| {
            let mut tags: Vec<String> = Vec::new();
            for table in [face.tables().gsub, face.tables().gpos].into_iter().flatten() {
                for feat in table.features {
                    let t = feat.tag.to_string();
                    if !tags.contains(&t) {
                        tags.push(t);
                    }
                }
            }
            tags.sort();
            tags
        })
        .unwrap_or_default()
    }

    /// Variation axes of a variable font (empty for static fonts).
    pub fn font_axes(&mut self, family: &str, weight: u16, italic: bool) -> Vec<AxisInfo> {
        self.parsed(family, weight, italic, |face| {
            face.variation_axes()
                .into_iter()
                .filter(|a| !a.hidden)
                .map(|a| {
                    let name = face
                        .names()
                        .into_iter()
                        .filter(|n| n.name_id == a.name_id)
                        .find_map(|n| n.to_string())
                        .unwrap_or_else(|| a.tag.to_string());
                    AxisInfo { tag: a.tag.to_string(), name, min: a.min_value, default: a.def_value, max: a.max_value }
                })
                .collect()
        })
        .unwrap_or_default()
    }
}
