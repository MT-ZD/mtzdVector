//! Font management: system fonts via fontdb, Google Fonts catalogue and
//! downloads, text shaping (rustybuzz) and glyph outlines.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use kurbo::{Affine, BezPath, Point};
use rustybuzz::ttf_parser;


const USER_AGENT: &str = "mtzd-vector/0.1";

#[derive(Clone, Debug)]
pub struct GoogleFamily {
    pub family: String,
    pub category: String,
    /// Variants such as "400", "700i".
    pub variants: Vec<String>,
    pub popularity: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoadState {
    Idle,
    Loading,
    Ready,
    Failed(String),
}

/// Tessellated preview of a font family name.
pub struct PreviewMesh {
    pub vertices: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

pub enum Preview {
    Pending,
    Ready(Arc<PreviewMesh>),
    Failed,
}

enum Job {
    GoogleList { force: bool },
    GooglePreview { family: String },
    Download { family: String, variant: String },
}

enum JobResult {
    GoogleList(Result<Vec<GoogleFamily>, String>),
    GooglePreview { family: String, data: Result<Vec<u8>, String> },
    Download { family: String, variant: String, result: Result<PathBuf, String> },
}

pub struct Caret {
    /// Byte index into the content.
    pub index: usize,
    pub x: f64,
    pub line: usize,
}

pub use crate::text::TextLayout;

/// A flattened guide path with cumulative arc lengths.
pub struct Polyline {
    pts: Vec<Point>,
    acc: Vec<f64>,
}

impl Polyline {
    pub fn new(path: &BezPath) -> Self {
        let mut pts: Vec<Point> = Vec::new();
        let mut started = false;
        kurbo::flatten(path.iter(), 0.1, |el| match el {
            kurbo::PathEl::MoveTo(p) => {
                // Only the first subpath guides the text.
                if !started {
                    pts.push(p);
                    started = true;
                } else {
                    started = false;
                    pts.push(Point::new(f64::NAN, f64::NAN));
                }
            }
            kurbo::PathEl::LineTo(p) => {
                if started {
                    pts.push(p)
                }
            }
            kurbo::PathEl::ClosePath => {
                if started && pts.len() > 1 {
                    pts.push(pts[0]);
                }
            }
            _ => {}
        });
        if let Some(k) = pts.iter().position(|p| p.x.is_nan()) {
            pts.truncate(k);
        }
        let mut acc = vec![0.0];
        for w in pts.windows(2) {
            acc.push(acc.last().unwrap() + (w[1] - w[0]).hypot());
        }
        Polyline { pts, acc }
    }

    pub fn length(&self) -> f64 {
        *self.acc.last().unwrap_or(&0.0)
    }

    /// Point and tangent angle at arc length `s` (extrapolated past the ends).
    pub fn at(&self, s: f64) -> (Point, f64) {
        if self.pts.len() < 2 {
            return (self.pts.first().copied().unwrap_or(Point::ZERO), 0.0);
        }
        let i = match self.acc.binary_search_by(|a| a.total_cmp(&s)) {
            Ok(i) => i.min(self.pts.len() - 2),
            Err(i) => i.saturating_sub(1).min(self.pts.len() - 2),
        };
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        let seg = (b - a).hypot().max(1e-9);
        let t = (s - self.acc[i]) / seg;
        (a + (b - a) * t, (b - a).atan2())
    }

    /// Arc length of the point on the polyline nearest to `p`.
    pub fn project(&self, p: Point) -> f64 {
        let mut best = (f64::MAX, 0.0);
        for (i, w) in self.pts.windows(2).enumerate() {
            let d = w[1] - w[0];
            let l2 = d.hypot2().max(1e-12);
            let t = ((p - w[0]).dot(d) / l2).clamp(0.0, 1.0);
            let q = w[0] + d * t;
            let dist = (p - q).hypot2();
            if dist < best.0 {
                best = (dist, self.acc[i] + t * l2.sqrt());
            }
        }
        best.1
    }
}

pub struct FontManager {
    pub db: fontdb::Database,
    /// Sorted, unique family names of installed (system + downloaded) fonts.
    pub families: Vec<String>,
    family_set: HashSet<String>,
    pub google: Vec<GoogleFamily>,
    google_index: HashMap<String, usize>,
    pub google_state: LoadState,
    face_data: HashMap<fontdb::ID, Arc<Vec<u8>>>,
    previews: HashMap<String, Preview>,
    requested: HashSet<String>,
    jobs: Sender<Job>,
    results: Receiver<JobResult>,
    /// Bumped whenever new fonts become available (invalidates text caches).
    pub generation: u64,
    pub status: Option<String>,
    pub downloads_in_flight: usize,
    /// Glyph coverage checks: (face, char) → has a glyph.
    pub(crate) coverage: HashMap<(fontdb::ID, char), bool>,
    /// Fallback face per (char, weight, italic).
    pub(crate) fallbacks: HashMap<(char, u16, bool), Option<fontdb::ID>>,
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("mtzd-vector")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join("mtzd-vector")
}

fn fonts_dir() -> PathBuf {
    data_dir().join("google-fonts")
}

fn variant_file(family: &str, variant: &str) -> PathBuf {
    let safe: String = family.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    fonts_dir().join(format!("{safe}-{variant}.ttf"))
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let mut resp = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| e.to_string())?;
    resp.body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// Resolve a Google Fonts css2 request to the first TrueType URL.
fn css_font_url(query: &str) -> Result<String, String> {
    let css = http_get(&format!("https://fonts.googleapis.com/css2?{query}"))?;
    let css = String::from_utf8_lossy(&css);
    let start = css.find("url(").ok_or("no font url in response")? + 4;
    let end = css[start..].find(')').ok_or("bad css")? + start;
    Ok(css[start..end].trim_matches(|c| c == '\'' || c == '"').to_string())
}

fn parse_google_list(json: &[u8]) -> Result<Vec<GoogleFamily>, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let list = v
        .get("familyMetadataList")
        .and_then(|l| l.as_array())
        .ok_or("unexpected metadata format")?;
    let mut out: Vec<GoogleFamily> = list
        .iter()
        .filter_map(|f| {
            let family = f.get("family")?.as_str()?.to_string();
            let category = f.get("category").and_then(|c| c.as_str()).unwrap_or("").to_string();
            let mut variants: Vec<String> = f
                .get("fonts")
                .and_then(|m| m.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            variants.sort_by_key(|v| (v.ends_with('i'), v.trim_end_matches('i').parse::<u32>().unwrap_or(400)));
            let popularity = f.get("popularity").and_then(|p| p.as_u64()).unwrap_or(99999) as u32;
            Some(GoogleFamily { family, category, variants, popularity })
        })
        .collect();
    out.sort_by_key(|f| f.popularity);
    Ok(out)
}

fn run_job(job: Job) -> JobResult {
    match job {
        Job::GoogleList { force } => {
            let cache = cache_dir().join("google-fonts.json");
            if !force {
                if let Ok(data) = std::fs::read(&cache) {
                    if let Ok(list) = parse_google_list(&data) {
                        return JobResult::GoogleList(Ok(list));
                    }
                }
            }
            let res = http_get("https://fonts.google.com/metadata/fonts").and_then(|data| {
                let list = parse_google_list(&data)?;
                let _ = std::fs::create_dir_all(cache_dir());
                let _ = std::fs::write(&cache, &data);
                Ok(list)
            });
            JobResult::GoogleList(res)
        }
        Job::GooglePreview { family } => {
            let safe: String = family.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
            let path = cache_dir().join("previews").join(format!("{safe}.ttf"));
            if let Ok(data) = std::fs::read(&path) {
                return JobResult::GooglePreview { family, data: Ok(data) };
            }
            let data = css_font_url(&format!("family={}&text={}", url_encode(&family), url_encode(&family)))
                .and_then(|url| http_get(&url));
            if let Ok(d) = &data {
                let _ = std::fs::create_dir_all(path.parent().unwrap());
                let _ = std::fs::write(&path, d);
            }
            JobResult::GooglePreview { family, data }
        }
        Job::Download { family, variant } => {
            let italic = variant.ends_with('i');
            let weight = variant.trim_end_matches('i');
            let query = format!(
                "family={}:ital,wght@{},{}",
                url_encode(&family),
                if italic { 1 } else { 0 },
                weight
            );
            let result = css_font_url(&query).and_then(|url| http_get(&url)).and_then(|data| {
                let path = variant_file(&family, &variant);
                std::fs::create_dir_all(fonts_dir()).map_err(|e| e.to_string())?;
                std::fs::write(&path, data).map_err(|e| e.to_string())?;
                Ok(path)
            });
            JobResult::Download { family, variant, result }
        }
    }
}

struct OutlineSink<'a> {
    path: &'a mut BezPath,
    xf: Affine,
}

impl ttf_parser::OutlineBuilder for OutlineSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.xf * Point::new(x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.xf * Point::new(x as f64, y as f64));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path
            .quad_to(self.xf * Point::new(x1 as f64, y1 as f64), self.xf * Point::new(x as f64, y as f64));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path.curve_to(
            self.xf * Point::new(x1 as f64, y1 as f64),
            self.xf * Point::new(x2 as f64, y2 as f64),
            self.xf * Point::new(x as f64, y as f64),
        );
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

impl FontManager {
    pub fn new(ctx: egui::Context) -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let _ = std::fs::create_dir_all(fonts_dir());
        db.load_fonts_dir(fonts_dir());
        db.set_sans_serif_family("Noto Sans");

        let (jobs, job_rx) = channel::<Job>();
        let (res_tx, results) = channel::<JobResult>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for _ in 0..4 {
            let rx = job_rx.clone();
            let tx = res_tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                loop {
                    let job = { rx.lock().unwrap().recv() };
                    let Ok(job) = job else { break };
                    let r = run_job(job);
                    if tx.send(r).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }

        let mut fm = FontManager {
            db,
            families: Vec::new(),
            family_set: HashSet::new(),
            google: Vec::new(),
            google_index: HashMap::new(),
            google_state: LoadState::Idle,
            face_data: HashMap::new(),
            previews: HashMap::new(),
            requested: HashSet::new(),
            jobs,
            results,
            generation: 1,
            status: None,
            downloads_in_flight: 0,
            coverage: HashMap::new(),
            fallbacks: HashMap::new(),
        };
        fm.rebuild_families();
        fm
    }

    fn rebuild_families(&mut self) {
        let mut set = BTreeSet::new();
        for face in self.db.faces() {
            if let Some((name, _)) = face.families.first() {
                if !name.starts_with('.') {
                    set.insert(name.clone());
                }
            }
        }
        self.family_set = set.iter().cloned().collect();
        self.families = set.into_iter().collect();
    }

    pub fn is_installed(&self, family: &str) -> bool {
        self.family_set.contains(family)
    }

    pub fn google_family(&self, family: &str) -> Option<&GoogleFamily> {
        self.google_index.get(family).map(|&i| &self.google[i])
    }

    pub fn load_google_list(&mut self, force: bool) {
        if self.google_state == LoadState::Loading {
            return;
        }
        self.google_state = LoadState::Loading;
        let _ = self.jobs.send(Job::GoogleList { force });
    }

    /// Process finished background jobs. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(r) = self.results.try_recv() {
            match r {
                JobResult::GoogleList(Ok(list)) => {
                    self.google_index = list.iter().enumerate().map(|(i, f)| (f.family.clone(), i)).collect();
                    self.google = list;
                    self.google_state = LoadState::Ready;
                }
                JobResult::GoogleList(Err(e)) => {
                    self.google_state = LoadState::Failed(e);
                }
                JobResult::GooglePreview { family, data } => {
                    let preview = data
                        .ok()
                        .and_then(|d| build_preview(&d, 0, &family))
                        .map(|m| Preview::Ready(Arc::new(m)))
                        .unwrap_or(Preview::Failed);
                    self.previews.insert(format!("g:{family}"), preview);
                }
                JobResult::Download { family, variant, result } => {
                    self.downloads_in_flight = self.downloads_in_flight.saturating_sub(1);
                    match result {
                        Ok(path) => {
                            let _ = self.db.load_font_file(&path);
                            self.rebuild_families();
                            self.generation += 1;
                            self.status = Some(format!("Installed {family} {variant}"));
                        }
                        Err(e) => {
                            self.status = Some(format!("Failed to download {family}: {e}"));
                        }
                    }
                }
            }
        }
    }

    /// Download a Google font variant (e.g. "400", "700i") unless present.
    pub fn request_google_variant(&mut self, family: &str, variant: &str) {
        let key = format!("{family}|{variant}");
        if self.requested.contains(&key) || variant_file(family, variant).exists() {
            return;
        }
        self.requested.insert(key);
        self.downloads_in_flight += 1;
        self.status = Some(format!("Downloading {family}…"));
        let _ = self.jobs.send(Job::Download { family: family.to_string(), variant: variant.to_string() });
    }

    /// Make sure a face for the text style is available, downloading from
    /// Google Fonts when needed.
    pub fn ensure_available(&mut self, family: &str, weight: u16, italic: bool) {
        let Some(g) = self.google_family(family) else { return };
        let wanted = nearest_variant(&g.variants, weight, italic);
        if let Some(v) = wanted {
            if !variant_file(family, &v).exists() {
                self.request_google_variant(family, &v);
            }
        }
    }

    pub fn weights_for(&self, family: &str) -> Vec<(u16, bool)> {
        if let Some(g) = self.google_family(family) {
            return g
                .variants
                .iter()
                .map(|v| (v.trim_end_matches('i').parse().unwrap_or(400), v.ends_with('i')))
                .collect();
        }
        let mut out: Vec<(u16, bool)> = self
            .db
            .faces()
            .filter(|f| f.families.iter().any(|(n, _)| n == family))
            .map(|f| (f.weight.0, f.style != fontdb::Style::Normal))
            .collect();
        out.sort();
        out.dedup();
        if out.is_empty() {
            out.push((400, false));
        }
        out
    }

    /// A face of exactly `family` (no generic fallback).
    pub(crate) fn query_exact(&self, family: &str, weight: u16, italic: bool) -> Option<fontdb::ID> {
        if !self.family_set.contains(family) {
            return None;
        }
        self.query(family, weight, italic).filter(|id| self.db.face(*id).is_some_and(|f| f.families.iter().any(|(n, _)| n == family)))
    }

    pub(crate) fn query(&self, family: &str, weight: u16, italic: bool) -> Option<fontdb::ID> {
        let families = [fontdb::Family::Name(family), fontdb::Family::SansSerif];
        self.db.query(&fontdb::Query {
            families: &families,
            weight: fontdb::Weight(weight),
            stretch: fontdb::Stretch::Normal,
            style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
        })
    }

    pub(crate) fn data_for(&mut self, id: fontdb::ID) -> Option<(Arc<Vec<u8>>, u32)> {
        let index = self.db.face(id)?.index;
        if let Some(d) = self.face_data.get(&id) {
            return Some((d.clone(), index));
        }
        let data = self.db.with_face_data(id, |d, _| Arc::new(d.to_vec()))?;
        self.face_data.insert(id, data.clone());
        Some((data, index))
    }

    /// Preview mesh for a family name in its own font. Installed fonts are
    /// built synchronously (budgeted by caller); Google fonts are fetched.
    pub fn preview(&mut self, family: &str, google: bool, budget: &mut u32) -> Option<Arc<PreviewMesh>> {
        let key = if google { format!("g:{family}") } else { format!("s:{family}") };
        match self.previews.get(&key) {
            Some(Preview::Ready(m)) => return Some(m.clone()),
            Some(Preview::Failed) | Some(Preview::Pending) => return None,
            None => {}
        }
        if google {
            self.previews.insert(key, Preview::Pending);
            let _ = self.jobs.send(Job::GooglePreview { family: family.to_string() });
            return None;
        }
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let p = self
            .query(family, 400, false)
            .and_then(|id| self.data_for(id))
            .and_then(|(data, index)| build_preview(&data, index, family));
        let entry = match p {
            Some(m) => {
                let m = Arc::new(m);
                self.previews.insert(key, Preview::Ready(m.clone()));
                return Some(m);
            }
            None => Preview::Failed,
        };
        self.previews.insert(key, entry);
        None
    }
}

pub fn nearest_variant(variants: &[String], weight: u16, italic: bool) -> Option<String> {
    variants
        .iter()
        .min_by_key(|v| {
            let vi = v.ends_with('i');
            let vw: i32 = v.trim_end_matches('i').parse().unwrap_or(400);
            (vi != italic) as i32 * 10000 + (vw - weight as i32).abs()
        })
        .cloned()
}

pub const PREVIEW_SIZE: f64 = 17.0;

fn build_preview(data: &[u8], index: u32, text: &str) -> Option<PreviewMesh> {
    let face = rustybuzz::Face::from_slice(data, index)?;
    let scale = PREVIEW_SIZE / face.units_per_em() as f64;
    let mut path = BezPath::new();
    let ascender = face.ascender() as f64 * scale;
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(text);
    let gb = rustybuzz::shape(&face, &[], buf);
    let mut x = 0.0;
    let base = ascender * 0.85;
    for (info, pos) in gb.glyph_infos().iter().zip(gb.glyph_positions()) {
        let xf = Affine::new([scale, 0.0, 0.0, -scale, x + pos.x_offset as f64 * scale, base - pos.y_offset as f64 * scale]);
        let mut sink = OutlineSink { path: &mut path, xf };
        face.outline_glyph(ttf_parser::GlyphId(info.glyph_id as u16), &mut sink);
        x += pos.x_advance as f64 * scale;
    }
    let _width = x;
    if path.elements().is_empty() {
        return None;
    }
    let (vertices, indices) = crate::render::tessellate_fill_raw(&path, 0.05, crate::document::FillRule::NonZero);
    Some(PreviewMesh { vertices, indices })
}
