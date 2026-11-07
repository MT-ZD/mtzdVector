//! Iconify (https://iconify.design) icon search and download. Network work
//! runs on background threads; SVGs are cached on disk under the cache dir.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::document::Document;
use crate::fonts::{FontManager, cache_dir, http_get, url_encode};

const API: &str = "https://api.iconify.design";

/// Popular icon sets offered as a filter (prefix, label).
pub const COLLECTIONS: [(&str, &str); 11] = [
    ("", "All sets"),
    ("mdi", "Material Design"),
    ("material-symbols", "Material Symbols"),
    ("ph", "Phosphor"),
    ("lucide", "Lucide"),
    ("tabler", "Tabler"),
    ("heroicons", "Heroicons"),
    ("fa6-solid", "Font Awesome 6"),
    ("bi", "Bootstrap"),
    ("carbon", "Carbon"),
    ("ri", "Remix"),
];

/// A tessellated icon preview in a unit box (0..1, aspect kept).
pub struct IconMesh {
    pub verts: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

pub enum IconState {
    Loading,
    Ready(String),
    Failed,
}

enum Msg {
    Search { generation: u64, result: Result<Vec<String>, String> },
    Svg { name: String, result: Result<String, String> },
}

pub struct IconLibrary {
    pub query: String,
    pub collection: usize,
    pub results: Vec<String>,
    pub searching: bool,
    pub error: Option<String>,
    pub svgs: HashMap<String, IconState>,
    previews: HashMap<String, Option<IconMesh>>,
    generation: u64,
    in_flight: usize,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    /// Icon to insert once its SVG arrives.
    /// An icon to insert once its SVG arrives, and where (else the view centre).
    pub pending_insert: Option<(String, Option<kurbo::Point>)>,
}

fn cache_path(name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    cache_dir().join("iconify").join(format!("{safe}.svg"))
}

/// Search URL for a query (optionally restricted to a set prefix).
pub fn search_url(query: &str, prefix: &str, limit: u32) -> String {
    let mut u = format!("{API}/search?query={}&limit={limit}", url_encode(query.trim()));
    if !prefix.is_empty() {
        u.push_str(&format!("&prefix={}", url_encode(prefix)));
    }
    u
}

/// SVG URL for `prefix:name`, with `currentColor` resolved to `color`.
pub fn svg_url(name: &str, color: &str) -> Option<String> {
    let (prefix, icon) = name.split_once(':')?;
    Some(format!("{API}/{}/{}.svg?height=96&color={}", url_encode(prefix), url_encode(icon), url_encode(color)))
}

pub fn parse_search(json: &[u8]) -> Result<Vec<String>, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let icons = v.get("icons").and_then(|i| i.as_array()).ok_or("unexpected search response")?;
    Ok(icons.iter().filter_map(|i| i.as_str().map(String::from)).collect())
}

impl IconLibrary {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        IconLibrary {
            query: String::new(),
            collection: 0,
            results: Vec::new(),
            searching: false,
            error: None,
            svgs: HashMap::new(),
            previews: HashMap::new(),
            generation: 0,
            in_flight: 0,
            tx,
            rx,
            ctx,
            pending_insert: None,
        }
    }

    pub fn search(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.generation += 1;
        self.searching = true;
        self.error = None;
        let (generation, tx, ctx) = (self.generation, self.tx.clone(), self.ctx.clone());
        let url = search_url(&q, COLLECTIONS[self.collection.min(COLLECTIONS.len() - 1)].0, 96);
        std::thread::spawn(move || {
            let result = http_get(&url).and_then(|b| parse_search(&b));
            let _ = tx.send(Msg::Search { generation, result });
            ctx.request_repaint();
        });
    }

    /// Start fetching an icon's SVG (from disk cache or the API).
    pub fn request(&mut self, name: &str) {
        if self.svgs.contains_key(name) {
            return;
        }
        if let Ok(s) = std::fs::read_to_string(cache_path(name)) {
            self.svgs.insert(name.to_string(), IconState::Ready(s));
            return;
        }
        if self.in_flight >= 12 {
            // Throttled; the grid asks again next frame.
            self.ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        self.in_flight += 1;
        self.svgs.insert(name.to_string(), IconState::Loading);
        let (tx, ctx, name) = (self.tx.clone(), self.ctx.clone(), name.to_string());
        std::thread::spawn(move || {
            let result = svg_url(&name, "#1E1E1E").ok_or_else(|| "bad icon name".to_string()).and_then(|u| http_get(&u)).and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()));
            if let Ok(svg) = &result {
                let p = cache_path(&name);
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(p, svg);
            }
            let _ = tx.send(Msg::Svg { name, result });
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Search { generation, result } if generation == self.generation => {
                    self.searching = false;
                    match result {
                        Ok(r) => {
                            if r.is_empty() {
                                self.error = Some("No icons found".into());
                            }
                            self.results = r;
                        }
                        Err(e) => self.error = Some(format!("Search failed: {e}")),
                    }
                }
                Msg::Search { .. } => {}
                Msg::Svg { name, result } => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    let st = match result {
                        Ok(s) if s.contains("<svg") => IconState::Ready(s),
                        _ => IconState::Failed,
                    };
                    self.svgs.insert(name, st);
                }
            }
        }
    }

    /// Preview geometry for an icon (None while loading or on failure).
    pub fn preview(&mut self, name: &str, fonts: &mut FontManager) -> Option<&IconMesh> {
        if !self.previews.contains_key(name) {
            match self.svgs.get(name) {
                Some(IconState::Ready(svg)) => {
                    let m = icon_mesh(svg, fonts);
                    self.previews.insert(name.to_string(), m);
                }
                Some(_) => return None,
                None => {
                    self.request(name);
                    return None;
                }
            }
        }
        self.previews.get(name).and_then(|m| m.as_ref())
    }

    pub fn svg(&self, name: &str) -> Option<&str> {
        match self.svgs.get(name) {
            Some(IconState::Ready(s)) => Some(s),
            _ => None,
        }
    }
}

/// Import an SVG and flatten it into one fill mesh normalized to a unit box.
pub fn icon_mesh(svg: &str, fonts: &mut FontManager) -> Option<IconMesh> {
    let doc: Document = crate::io::svg::import(svg.as_bytes(), fonts).ok()?;
    let side = doc.primary().w.max(doc.primary().h).max(1e-6);
    let norm = kurbo::Affine::scale(1.0 / side);
    let (mut verts, mut indices) = (Vec::new(), Vec::new());
    for s in doc.shapes() {
        if s.fill.is_none() && s.stroke.is_none() {
            continue;
        }
        let (p, _) = crate::render::compute_outline(s, fonts);
        let (p, rule) = match &s.stroke {
            Some(st) if s.fill.is_none() => (crate::geom::polys_to_bez(&crate::strokes::stroke_area(&p, st, s.fill_rule)), crate::document::FillRule::EvenOdd),
            _ => (p, s.fill_rule),
        };
        let (v, i) = crate::render::tessellate_fill_raw(&(norm * p), 0.002, rule);
        let base = verts.len() as u32;
        verts.extend(v);
        indices.extend(i.into_iter().map(|k| k + base));
    }
    if indices.is_empty() {
        return None;
    }
    Some(IconMesh { verts, indices })
}
