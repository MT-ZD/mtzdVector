//! Text styles (create, redefine, sync), threads between text frames and
//! text wrap around shapes.
//!
//! Threads and wrap exclusions are derived data on `TextData` (`flow`,
//! `exclusions`), recomputed by `resolve_flows` whenever the document
//! changes: the app runs it every frame (cheaply skipped when nothing
//! changed); exporters without an app run it once on load.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kurbo::BezPath;

use crate::app::App;
use crate::document::{CharStyle, Document, FlowIn, Shape, ShapeKind, TextData, TextStyle};
use crate::fonts::FontManager;
use crate::text::TextLayout;

/// Derive threads and wrap exclusions. `outline` gives a shape's document
/// outline and text layout (cached in the app). Returns true if anything
/// changed.
pub fn resolve_flows(doc: &mut Document, outline: &mut dyn FnMut(&Shape) -> (Arc<BezPath>, Option<Arc<TextLayout>>)) -> bool {
    let ids = doc.all_ids();
    let mut texts: Vec<u64> = Vec::new();
    let mut wrappers: Vec<u64> = Vec::new();
    let mut nexts: HashMap<u64, u64> = HashMap::new();
    for id in &ids {
        let Some(s) = doc.get(*id) else { continue };
        if let ShapeKind::Text(t) = &s.kind {
            if t.frame.is_some() {
                texts.push(*id);
                if let Some(n) = t.next {
                    nexts.insert(*id, n);
                }
            }
        }
        if s.text_wrap.is_some() && s.visible {
            wrappers.push(*id);
        }
    }
    let mut changed = false;

    // Broken links: the next frame was deleted or isn't frame text.
    for (from, to) in nexts.clone() {
        if !texts.contains(&to) || to == from {
            if let Some(ShapeKind::Text(t)) = doc.get_mut(from).map(|s| &mut s.kind) {
                t.next = None;
            }
            nexts.remove(&from);
            changed = true;
        }
    }

    // Wrap exclusions (local coordinates of each text).
    for tid in &texts {
        let Some(t) = doc.get(*tid).cloned() else { continue };
        let ShapeKind::Text(td) = &t.kind else { continue };
        let tb = outline(&t).1.map(|l| t.transform.transform_rect_bbox(l.bounds));
        let key = doc.order_key(*tid);
        let inv = t.transform.inverse();
        let mut ex: Vec<BezPath> = Vec::new();
        for wid in &wrappers {
            if wid == tid || doc.order_key(*wid) <= key {
                continue;
            }
            let Some(w) = doc.get(*wid).cloned() else { continue };
            let off = w.text_wrap.unwrap_or(0.0).max(0.0);
            let (path, _) = outline(&w);
            let Some(wb) = crate::geom::path_bounds(&path) else { continue };
            if let Some(tb) = tb {
                if !wb.inflate(off, off).overlaps(tb) {
                    continue;
                }
            }
            let grown = if off > 0.0 {
                let round = crate::document::Stroke { join: crate::document::LineJoin::Round, cap: crate::document::LineCap::Round, width: off * 2.0, ..Default::default() };
                let ring = crate::strokes::outline(&path, &round, off * 2.0);
                let u = crate::boolean::op(
                    &crate::boolean::path_to_polys(&path, crate::document::FillRule::NonZero),
                    &crate::boolean::path_to_polys(&ring, crate::document::FillRule::NonZero),
                    crate::boolean::BoolOp::Union,
                );
                crate::geom::polys_to_bez(&u)
            } else {
                (*path).clone()
            };
            ex.push(inv * grown);
        }
        let new = (!ex.is_empty()).then(|| Arc::new(ex));
        if td.exclusions.as_deref() != new.as_deref() {
            if let Some(ShapeKind::Text(t)) = doc.get_mut(*tid).map(|s| &mut s.kind) {
                t.exclusions = new;
            }
            changed = true;
        }
    }

    // Threads: heads are frames with a next that nobody flows into.
    let followers: HashSet<u64> = nexts.values().copied().collect();
    let mut flowing: HashSet<u64> = HashSet::new();
    for head in texts.iter().copied().filter(|t| nexts.contains_key(t) && !followers.contains(t)) {
        let Some(h) = doc.get(head).cloned() else { continue };
        let ShapeKind::Text(htd) = &h.kind else { continue };
        let story = Arc::new(TextData { flow: None, exclusions: None, ..htd.clone() });
        let mut cur = head;
        let mut seen = HashSet::from([head]);
        while let Some(&n) = nexts.get(&cur) {
            if !seen.insert(n) {
                break;
            }
            let Some(cs) = doc.get(cur).cloned() else { break };
            let end = outline(&cs).1.map(|l| l.end).unwrap_or(0);
            let want = FlowIn { head, head_rev: h.rev, start: end, story: story.clone() };
            if let Some(ShapeKind::Text(t)) = doc.get(n).map(|s| &s.kind) {
                if t.flow.as_ref() != Some(&want) {
                    if let Some(ShapeKind::Text(t)) = doc.get_mut(n).map(|s| &mut s.kind) {
                        t.flow = Some(want);
                    }
                    changed = true;
                }
            }
            flowing.insert(n);
            cur = n;
        }
    }
    // Frames no longer in a thread show their own text again.
    for tid in &texts {
        if flowing.contains(tid) {
            continue;
        }
        if let Some(ShapeKind::Text(t)) = doc.get(*tid).map(|s| &s.kind) {
            if t.flow.is_some() {
                if let Some(ShapeKind::Text(t)) = doc.get_mut(*tid).map(|s| &mut s.kind) {
                    t.flow = None;
                }
                changed = true;
            }
        }
    }
    changed
}

/// `resolve_flows` without an app cache (exports, thumbnails), repeated
/// until threads settle.
pub fn resolve_flows_uncached(doc: &mut Document, fonts: &mut FontManager) {
    if !needs_flow(doc) {
        return;
    }
    for _ in 0..8 {
        let changed = resolve_flows(doc, &mut |s| {
            let (p, t) = crate::render::compute_outline(s, fonts);
            (Arc::new(p), t.map(Arc::new))
        });
        if !changed {
            break;
        }
    }
}

/// Whether the document uses threads or text wrap at all.
pub fn needs_flow(doc: &Document) -> bool {
    fn walk<S: std::borrow::Borrow<Shape>>(list: &[S]) -> bool {
        list.iter().any(|s| {
            let s = s.borrow();
            s.text_wrap.is_some() || matches!(&s.kind, ShapeKind::Text(t) if t.next.is_some() || t.flow.is_some() || t.exclusions.is_some()) || s.kind.children().is_some_and(|c| walk(c))
        })
    }
    walk(doc.shapes())
}

impl App {
    /// Threads and wrap: re-derive after document changes.
    pub fn text_flow_tick(&mut self) {
        let key = (self.doc.revision(), self.fonts.generation, self.doc.shapes().as_ptr() as usize);
        if self.flow_key == Some(key) {
            return;
        }
        if needs_flow(&self.doc) {
            let (cache, fonts) = (&mut self.cache, &mut self.fonts);
            for _ in 0..4 {
                let changed = resolve_flows(&mut self.doc, &mut |s| {
                    let c = cache.update(s, fonts);
                    (c.doc_path.clone(), c.text.clone())
                });
                if !changed {
                    break;
                }
            }
        }
        self.flow_key = Some((self.doc.revision(), self.fonts.generation, self.doc.shapes().as_ptr() as usize));
    }

    /// Link the selected frame texts into a thread, in selection order.
    /// Text already in the later frames is appended to the story.
    pub fn link_text_frames(&mut self) -> bool {
        let frames: Vec<u64> = self
            .selection
            .iter()
            .copied()
            .filter(|id| matches!(self.doc.get(*id).map(|s| &s.kind), Some(ShapeKind::Text(t)) if t.frame.is_some()))
            .collect();
        if frames.len() < 2 {
            self.notify("Select two or more text frames to link (in reading order)");
            return false;
        }
        self.checkpoint("");
        // Continue from the end of an existing thread of the first frame.
        let mut tail = frames[0];
        let mut guard = 0;
        while let Some(n) = self.text_next(tail) {
            tail = n;
            guard += 1;
            if guard > 1000 {
                break;
            }
        }
        let head = self.thread_head(frames[0]);
        for &f in &frames[1..] {
            if f == tail || self.thread_head(f) == head {
                continue;
            }
            // Move the frame's own text to the end of the story.
            let own = match self.doc.get(f).map(|s| &s.kind) {
                Some(ShapeKind::Text(t)) if t.flow.is_none() => t.content.clone(),
                _ => String::new(),
            };
            if !own.trim().is_empty() {
                if let Some(ShapeKind::Text(t)) = self.doc.get_mut(head).map(|s| &mut s.kind) {
                    let at = t.content.len();
                    t.edit(at..at, &format!("\n{own}"));
                }
            }
            if let Some(ShapeKind::Text(t)) = self.doc.get_mut(f).map(|s| &mut s.kind) {
                t.content.clear();
                t.spans.clear();
            }
            if let Some(ShapeKind::Text(t)) = self.doc.get_mut(tail).map(|s| &mut s.kind) {
                t.next = Some(f);
            }
            tail = f;
        }
        true
    }

    /// Unlink the selected frames from their threads (text stays in the
    /// first frame).
    pub fn unlink_text_frames(&mut self) {
        let sel = self.selection.clone();
        self.checkpoint("");
        for id in sel {
            for pid in self.doc.all_ids() {
                if self.text_next(pid) == Some(id) {
                    if let Some(ShapeKind::Text(t)) = self.doc.get_mut(pid).map(|s| &mut s.kind) {
                        t.next = None;
                    }
                }
            }
            if let Some(ShapeKind::Text(t)) = self.doc.get_mut(id).map(|s| &mut s.kind) {
                t.next = None;
            }
        }
    }

    fn text_next(&self, id: u64) -> Option<u64> {
        match self.doc.get(id).map(|s| &s.kind) {
            Some(ShapeKind::Text(t)) => t.next,
            _ => None,
        }
    }

    /// First frame of the thread `id` is in (itself when not threaded).
    pub fn thread_head(&self, id: u64) -> u64 {
        match self.doc.get(id).map(|s| &s.kind) {
            Some(ShapeKind::Text(t)) => t.flow.as_ref().map(|f| f.head).unwrap_or(id),
            _ => id,
        }
    }

    // ------------------------------------------------------------------
    // Text styles
    // ------------------------------------------------------------------

    /// New style from a text: paragraph (its object attributes) or
    /// character (the overrides at byte `at`).
    pub fn add_text_style(&mut self, paragraph: bool, td: &TextData, at: Option<usize>) -> u64 {
        self.checkpoint("");
        let id = self.doc.fresh_id();
        let n = self.doc.text_styles.iter().filter(|s| s.paragraph == paragraph).count() + 1;
        let name = if paragraph { format!("Paragraph {n}") } else { format!("Character {n}") };
        let style = if paragraph {
            TextStyle::from_text(id, name, td)
        } else {
            let i = at.unwrap_or(0);
            let st = CharStyle::diff(&td.base_style(), &td.style_at(i));
            TextStyle { id, name, paragraph: false, style: st, line_height: None, align: None, paragraph_spacing: None }
        };
        self.doc.text_styles.push(style);
        self.notify("Text style saved · manage styles in the Colours tab");
        id
    }

    /// Redefine style `id` from a text's attributes, updating every user.
    pub fn redefine_text_style(&mut self, id: u64, td: &TextData) {
        let Some(i) = self.doc.text_styles.iter().position(|s| s.id == id) else { return };
        self.checkpoint("");
        let old = self.doc.text_styles[i].clone();
        self.doc.text_styles[i] = if old.paragraph { TextStyle { name: old.name.clone(), ..TextStyle::from_text(id, String::new(), td) } } else { old };
        self.sync_text_style(id);
    }

    /// Copy style `id` into everything linked to it.
    pub fn sync_text_style(&mut self, id: u64) {
        let Some(st) = self.doc.text_styles.iter().find(|s| s.id == id).cloned() else { return };
        for sid in self.doc.all_ids() {
            let Some(ShapeKind::Text(td)) = self.doc.get(sid).map(|s| &s.kind) else { continue };
            let mut t = td.clone();
            if st.paragraph && t.style_id == Some(id) {
                st.apply_to(&mut t);
            }
            for sp in &mut t.spans {
                if sp.style_id == Some(id) {
                    sp.style = st.style.clone();
                }
            }
            if &t != td {
                if let Some(ShapeKind::Text(x)) = self.doc.get_mut(sid).map(|s| &mut s.kind) {
                    *x = t;
                }
            }
        }
    }

    /// Delete a style; its users keep their attributes.
    pub fn delete_text_style(&mut self, id: u64) {
        self.checkpoint("");
        self.doc.text_styles.retain(|s| s.id != id);
        for sid in self.doc.all_ids() {
            let uses = matches!(self.doc.get(sid).map(|s| &s.kind), Some(ShapeKind::Text(t)) if t.style_id == Some(id) || t.spans.iter().any(|sp| sp.style_id == Some(id)));
            if uses {
                if let Some(ShapeKind::Text(t)) = self.doc.get_mut(sid).map(|s| &mut s.kind) {
                    if t.style_id == Some(id) {
                        t.style_id = None;
                    }
                    for sp in &mut t.spans {
                        if sp.style_id == Some(id) {
                            sp.style_id = None;
                        }
                    }
                }
            }
        }
    }
}
