//! Real text in PDF: glyph runs shown with embedded font subsets (Type 0 /
//! Identity-H, CID = glyph id in the subset) and a ToUnicode map so the
//! text can be selected, searched and copied.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use kurbo::Affine;
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, UnicodeCmap};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect as PRect, Ref, Str};
use rustybuzz::ttf_parser;

use crate::document::Rgba;
use crate::text::TextLayout;

struct EmbFont {
    data: Arc<Vec<u8>>,
    index: u32,
    coords: Vec<(ttf_parser::Tag, f32)>,
    remap: subsetter::GlyphRemapper,
    /// New glyph id → text it stands for, and advance (1/1000 em).
    unicode: BTreeMap<u16, String>,
    widths: BTreeMap<u16, f32>,
    name: String,
    postscript: String,
    r: Ref,
}

/// Fonts used across all pages of a PDF.
#[derive(Default)]
pub struct PdfFonts {
    fonts: Vec<EmbFont>,
    index: HashMap<(fontdb::ID, Vec<(u32, u32)>), usize>,
}

fn coeffs(a: Affine) -> [f32; 6] {
    a.as_coeffs().map(|v| v as f32)
}

impl PdfFonts {
    fn font(&mut self, face: &crate::text::FaceRef, coords: &[(ttf_parser::Tag, f32)], alloc: &mut dyn FnMut() -> Ref) -> usize {
        let key = (face.id, coords.iter().map(|(t, v)| (t.0, v.to_bits())).collect::<Vec<_>>());
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let i = self.fonts.len();
        let mut remap = subsetter::GlyphRemapper::new();
        remap.remap(0);
        let ps: String = face.postscript.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        self.fonts.push(EmbFont {
            data: face.data.clone(),
            index: face.index,
            coords: coords.to_vec(),
            remap,
            unicode: BTreeMap::new(),
            widths: BTreeMap::new(),
            name: format!("F{i}"),
            postscript: if ps.is_empty() { format!("Font{i}") } else { ps },
            r: alloc(),
        });
        self.index.insert(key, i);
        i
    }

    /// Show the runs of `t` (local coordinates, mapped by `transform`) in
    /// `base` unless a run has its own colour. Returns the font resource
    /// names used.
    #[allow(clippy::too_many_arguments)]
    pub fn show(&mut self, c: &mut Content, t: &TextLayout, transform: Affine, base: Rgba, ink: &dyn Fn(&mut Content, Rgba), alloc: &mut dyn FnMut() -> Ref, used: &mut BTreeMap<String, Ref>) {
        c.save_state();
        c.transform(coeffs(transform));
        for run in &t.runs {
            let Some(face) = t.faces.get(run.face) else { continue };
            let fi = self.font(face, &run.coords, alloc);
            let f = &mut self.fonts[fi];
            used.insert(f.name.clone(), f.r);
            let Ok(mut tf) = ttf_parser::Face::parse(&f.data, f.index) else { continue };
            for (tag, v) in &f.coords {
                tf.set_variation(*tag, *v);
            }
            let upem = tf.units_per_em().max(1) as f64;
            ink(c, run.style.fill.unwrap_or(base));
            c.begin_text();
            let size = run.style.size.max(0.01);
            c.set_font(Name(f.name.as_bytes()), 1.0);
            // Groups of glyphs on one baseline, with TJ adjustments for kerning,
            // tracking and justification.
            let mut k = 0;
            while k < run.glyphs.len() {
                let g0 = &run.glyphs[k];
                c.set_text_matrix([size as f32, 0.0, 0.0, -size as f32, g0.x as f32, g0.y as f32]);
                let mut pen = g0.x;
                let mut sp = c.show_positioned();
                let mut items = sp.items();
                let mut j = k;
                while j < run.glyphs.len() && (run.glyphs[j].y - g0.y).abs() < 1e-6 {
                    let g = &run.glyphs[j];
                    let adj = (g.x - pen) * 1000.0 / size;
                    if adj.abs() > 0.01 {
                        items.adjust(-adj as f32);
                    }
                    let new = f.remap.remap(g.id);
                    let w = tf.glyph_hor_advance(ttf_parser::GlyphId(g.id)).unwrap_or(0) as f64 * 1000.0 / upem;
                    f.widths.insert(new, w as f32);
                    if !g.text.is_empty() {
                        f.unicode.entry(new).or_insert_with(|| g.text.clone());
                    }
                    items.show(Str(&new.to_be_bytes()));
                    pen = g.x + w * size / 1000.0;
                    j += 1;
                }
                drop(items);
                drop(sp);
                k = j.max(k + 1);
            }
            c.end_text();
        }
        // Underline and strike bars.
        for (r, col) in &t.decorations {
            ink(c, col.unwrap_or(base));
            c.rect(r.x0 as f32, r.y0 as f32, r.width() as f32, r.height() as f32);
            c.fill_nonzero();
        }
        c.restore_state();
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// Write the font subsets. Fonts that fail to subset are left out (their
    /// text won't render), so callers check `subset_ok` first.
    pub fn write(self, pdf: &mut Pdf, alloc: &mut dyn FnMut() -> Ref) {
        for f in self.fonts {
            let tags: Vec<(subsetter::Tag, f32)> = f.coords.iter().map(|(t, v)| (subsetter::Tag::new(&t.to_bytes()), *v)).collect();
            let sub = if tags.is_empty() { subsetter::subset(&f.data, f.index, &f.remap) } else { subsetter::subset_with_variations(&f.data, f.index, &tags, &f.remap) };
            let Ok(sub) = sub else { continue };
            let sub: Vec<u8> = sub;
            let Ok(face) = ttf_parser::Face::parse(&sub, 0) else { continue };
            let cff = face.tables().cff.is_some();
            let upem = face.units_per_em().max(1) as f32;
            let s = 1000.0 / upem;
            let (cid_r, fd_r, ff_r, cmap_r) = (alloc(), alloc(), alloc(), alloc());
            // Subset tag: six capitals derived from the font index.
            let tag: String = (0..6).map(|k| (b'A' + ((f.r.get() as u32 / 26u32.pow(k)) % 26) as u8) as char).collect();
            let base = format!("{tag}+{}", f.postscript);
            pdf.type0_font(f.r).base_font(Name(base.as_bytes())).encoding_predefined(Name(b"Identity-H")).descendant_font(cid_r).to_unicode(cmap_r);
            {
                let mut cid = pdf.cid_font(cid_r);
                cid.subtype(if cff { CidFontType::Type0 } else { CidFontType::Type2 });
                cid.base_font(Name(base.as_bytes()));
                cid.system_info(SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 });
                cid.font_descriptor(fd_r);
                cid.default_width(0.0);
                if !cff {
                    cid.cid_to_gid_map_predefined(Name(b"Identity"));
                }
                let mut w = cid.widths();
                for (g, wd) in &f.widths {
                    w.consecutive(*g, [*wd]);
                }
            }
            let bb = face.global_bounding_box();
            let mut flags = FontFlags::SYMBOLIC;
            if face.is_italic() {
                flags |= FontFlags::ITALIC;
            }
            if face.is_monospaced() {
                flags |= FontFlags::FIXED_PITCH;
            }
            let mut fd = pdf.font_descriptor(fd_r);
            fd.name(Name(base.as_bytes()))
                .flags(flags)
                .bbox(PRect::new(bb.x_min as f32 * s, bb.y_min as f32 * s, bb.x_max as f32 * s, bb.y_max as f32 * s))
                .italic_angle(face.italic_angle())
                .ascent(face.ascender() as f32 * s)
                .descent(face.descender() as f32 * s)
                .cap_height(face.capital_height().unwrap_or(face.ascender()) as f32 * s)
                .stem_v(80.0);
            if cff {
                fd.font_file3(ff_r);
            } else {
                fd.font_file2(ff_r);
            }
            fd.finish();
            let data = super::pdf::deflate_pub(&sub);
            let mut st = pdf.stream(ff_r, &data);
            st.filter(Filter::FlateDecode);
            if cff {
                st.pair(Name(b"Subtype"), Name(b"OpenType"));
            }
            st.finish();
            let mut cmap = UnicodeCmap::new(Name(b"Custom"), SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"UCS"), supplement: 0 });
            for (g, text) in &f.unicode {
                cmap.pair_with_multiple(*g, text.chars());
            }
            let buf = cmap.finish();
            pdf.cmap(cmap_r, &buf);
        }
    }
}
