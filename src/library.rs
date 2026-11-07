//! Shared component libraries: other .mvec files whose main components can
//! be placed in this document. Placing one copies the main into a hidden,
//! locked "Library: NAME" group (so the document stays self-contained) and
//! remembers where it came from; when the library file changes, the copy can
//! be updated in place and its instances follow. Linked colour keys come
//! along by name.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use kurbo::Point;

use crate::app::App;
use crate::document::{Component, Document, LibraryLink, Paint, Shape, ShapeKind};

/// A loaded library file.
pub struct Library {
    pub name: String,
    pub doc: Document,
    mtime: Option<SystemTime>,
}

impl Library {
    /// Main components: (id, name, variant set).
    pub fn components(&self) -> Vec<(u64, String, Option<String>)> {
        let mut out = Vec::new();
        for id in self.doc.all_ids() {
            if let Some(s) = self.doc.get(id) {
                if let Some(Component::Main { set, library: None, .. }) = &s.component {
                    out.push((id, s.name.clone(), set.clone()));
                }
            }
        }
        out
    }
}

/// Library files by path, reloaded when their modification time changes.
#[derive(Default)]
pub struct LibraryCache {
    libs: HashMap<PathBuf, Result<Library, String>>,
}

impl LibraryCache {
    pub fn get(&mut self, path: &Path) -> Result<&Library, String> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let stale = match self.libs.get(path) {
            Some(Ok(l)) => l.mtime != mtime,
            Some(Err(_)) => true,
            None => true,
        };
        if stale {
            let loaded = crate::io::load_native(path).map(|doc| Library { name: library_name(path), doc, mtime });
            self.libs.insert(path.to_path_buf(), loaded);
        }
        match self.libs.get(path) {
            Some(Ok(l)) => Ok(l),
            Some(Err(e)) => Err(e.clone()),
            None => Err("not loaded".into()),
        }
    }

    pub fn forget(&mut self, path: &Path) {
        self.libs.remove(path);
    }
}

pub fn library_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Library".into())
}

fn walk_mut(s: &mut Shape, f: &mut dyn FnMut(&mut Shape)) {
    f(s);
    if let Some(c) = s.kind.children_mut() {
        for c in c {
            walk_mut(c, f);
        }
    }
}

/// Fingerprint of a library main: its artwork and linked key colours.
pub fn content_key(lib: &Document, main: &Shape) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(main).unwrap_or_default().hash(&mut h);
    let mut keys = Vec::new();
    let mut probe = main.clone();
    walk_mut(&mut probe, &mut |s| keys.extend(s.fill_key.into_iter().chain(s.stroke_key)));
    for k in keys {
        if let Some(c) = lib.color_key(k) {
            (c.name.as_str(), c.color.r.to_bits(), c.color.g.to_bits(), c.color.b.to_bits(), c.color.a.to_bits()).hash(&mut h);
        }
    }
    Hasher::finish(&h)
}

impl App {
    /// The local copy of `source` from library `path`, if placed before.
    pub fn library_copy(&self, path: &Path, source: u64) -> Option<u64> {
        self.mains().into_iter().find(|id| matches!(self.doc.get(*id).and_then(|s| s.component.as_ref()), Some(Component::Main { library: Some(l), .. }) if l.path == path && l.source == source))
    }

    /// The library's main as it should look locally: ids mapped through
    /// `ids` (extended with fresh ones), images copied and colour keys
    /// matched by name.
    fn localise(&mut self, lib: &Document, main: &Shape, ids: &mut BTreeMap<u64, u64>) -> Shape {
        let mut s = main.clone();
        let mut key_map: BTreeMap<u64, u64> = BTreeMap::new();
        let mut img_map: BTreeMap<u64, u64> = BTreeMap::new();
        let doc = &mut self.doc;
        walk_mut(&mut s, &mut |sh| {
            sh.id = *ids.entry(sh.id).or_insert_with(|| doc.fresh_id());
            sh.rev = 0;
            if !matches!(sh.component, Some(Component::Main { .. })) || sh.id != ids.get(&main.id).copied().unwrap_or(0) {
                // Nested components become plain artwork.
                sh.component = None;
            }
            for key in [&mut sh.fill_key, &mut sh.stroke_key] {
                let Some(k) = *key else { continue };
                let local = *key_map.entry(k).or_insert_with(|| match lib.color_key(k) {
                    Some(c) => match doc.colors.iter_mut().find(|l| l.name == c.name) {
                        Some(l) => {
                            l.color = c.color;
                            l.id
                        }
                        None => {
                            let id = doc.add_color_key(&c.name, c.color);
                            if let Some(l) = doc.colors.iter_mut().find(|l| l.id == id) {
                                l.spot = c.spot;
                                l.cmyk = c.cmyk;
                            }
                            id
                        }
                    },
                    None => 0,
                });
                *key = (local != 0).then_some(local);
            }
            for p in [sh.fill.as_mut()].into_iter().flatten() {
                if let Paint::Image(f) = p {
                    let src = f.image;
                    f.image = *img_map.entry(src).or_insert_with(|| lib.images.get(&src).cloned().map(|a| doc.add_image(a)).unwrap_or(src));
                }
            }
        });
        s
    }

    /// The hidden group that holds the copies from library `name`.
    fn library_group(&mut self, name: &str) -> u64 {
        let label = format!("Library: {name}");
        if let Some(s) = self.doc.shapes().iter().find(|s| s.name == label && matches!(s.kind, ShapeKind::Group { .. })) {
            return s.id;
        }
        let mut g = Shape::new(ShapeKind::Group { children: Vec::new() });
        g.name = label;
        g.fill = None;
        g.stroke = None;
        g.visible = false;
        g.locked = true;
        self.doc.insert_shape(0, g)
    }

    /// Place an instance of library component `source` at `at`.
    pub fn insert_library_component(&mut self, path: &Path, source: u64, at: Point) -> Option<u64> {
        let local = match self.library_copy(path, source) {
            Some(id) => id,
            None => {
                let (lib_doc, name) = match self.libraries.get(path) {
                    Ok(l) => (l.doc.clone(), l.name.clone()),
                    Err(e) => {
                        self.notify(format!("Could not open the library: {e}"));
                        return None;
                    }
                };
                let main = lib_doc.get(source)?.clone();
                self.checkpoint("");
                let mut ids = BTreeMap::new();
                let mut copy = self.localise(&lib_doc, &main, &mut ids);
                let key = content_key(&lib_doc, &main);
                if let Some(Component::Main { library, set, .. }) = &mut copy.component {
                    // Sets stay per library so local sets don't mix in.
                    *set = set.as_ref().map(|s| format!("{name} / {s}"));
                    *library = Some(LibraryLink { path: path.to_path_buf(), source, ids, key });
                }
                let g = self.library_group(&name);
                let id = copy.id;
                if let Some(Some(children)) = self.doc.get_mut(g).map(|s| s.kind.children_mut()) {
                    children.push(copy);
                }
                self.dirty = true;
                id
            }
        };
        self.insert_instance(local, at)
    }

    /// Local library copies whose source has changed: (local id, source
    /// still exists).
    pub fn library_updates(&mut self) -> Vec<(u64, bool)> {
        let mut out = Vec::new();
        for id in self.mains() {
            let Some(Some(Component::Main { library: Some(l), .. })) = self.doc.get(id).map(|s| s.component.clone()) else { continue };
            let Ok(lib) = self.libraries.get(&l.path) else { continue };
            match lib.doc.get(l.source) {
                Some(src) if content_key(&lib.doc, src) != l.key => out.push((id, true)),
                Some(_) => {}
                None => out.push((id, false)),
            }
        }
        out
    }

    /// Bring local library copies up to date (instances follow, keeping
    /// their overrides). Returns how many were updated.
    pub fn update_library_components(&mut self, only: Option<&Path>) -> usize {
        let pending: Vec<u64> = self.library_updates().into_iter().filter(|(_, exists)| *exists).map(|(id, _)| id).collect();
        let mut n = 0;
        for id in pending {
            let Some(cur) = self.doc.get(id).cloned() else { continue };
            let Some(Component::Main { library: Some(mut link), set, .. }) = cur.component.clone() else { continue };
            if only.is_some_and(|p| p != link.path) {
                continue;
            }
            let Ok(lib) = self.libraries.get(&link.path) else { continue };
            let lib_doc = lib.doc.clone();
            let Some(main) = lib_doc.get(link.source).cloned() else { continue };
            if n == 0 {
                self.checkpoint("");
            }
            let mut ids = link.ids.clone();
            let mut copy = self.localise(&lib_doc, &main, &mut ids);
            link.ids = ids;
            link.key = content_key(&lib_doc, &main);
            if let Some(Component::Main { library, set: s, .. }) = &mut copy.component {
                *library = Some(link);
                *s = set;
            }
            copy.visible = cur.visible;
            self.doc.replace(copy);
            n += 1;
        }
        if n > 0 {
            self.dirty = true;
            self.comp_rev = u64::MAX;
        }
        n
    }
}
