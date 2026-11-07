//! On-canvas text editing: caret and selection movement, clipboard, mouse
//! selection and the selection highlight.

use egui::{Color32, Stroke};
use kurbo::Point;

use crate::app::App;
use crate::document::ShapeKind;
use crate::fonts::TextLayout;

pub fn prev_char(s: &str, c: usize) -> usize {
    s[..c].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
}

pub fn next_char(s: &str, c: usize) -> usize {
    s[c..].chars().next().map(|ch| c + ch.len_utf8()).unwrap_or(c)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// Start of the word before `c` (skipping spaces first), like Ctrl+Left.
pub fn word_left(s: &str, c: usize) -> usize {
    let mut i = c;
    while i > 0 && !is_word(s[..i].chars().next_back().unwrap()) {
        i = prev_char(s, i);
        if s[i..].starts_with('\n') && i + 1 < c {
            return i + 1;
        }
    }
    while i > 0 && is_word(s[..i].chars().next_back().unwrap()) {
        i = prev_char(s, i);
    }
    i
}

/// End of the next word after `c`, like Ctrl+Right.
pub fn word_right(s: &str, c: usize) -> usize {
    let mut i = c;
    while i < s.len() && !is_word(s[i..].chars().next().unwrap()) {
        if s[i..].starts_with('\n') && i > c {
            return i;
        }
        i = next_char(s, i);
    }
    while i < s.len() && is_word(s[i..].chars().next().unwrap()) {
        i = next_char(s, i);
    }
    i
}

/// Word (or run of spaces / punctuation) around `c`.
pub fn word_at(s: &str, c: usize) -> (usize, usize) {
    let c = c.min(s.len());
    let class = |ch: char| if is_word(ch) { 0 } else if ch == '\n' { 2 } else if ch.is_whitespace() { 1 } else { 3 };
    let here = s[c..].chars().next().or_else(|| s[..c].chars().next_back());
    let Some(k) = here.map(class) else { return (c, c) };
    if k == 2 {
        return (c, c);
    }
    let mut a = c;
    while a > 0 && class(s[..a].chars().next_back().unwrap()) == k {
        a = prev_char(s, a);
    }
    let mut b = c;
    while b < s.len() && class(s[b..].chars().next().unwrap()) == k {
        b = next_char(s, b);
    }
    (a, b)
}

/// Paragraph (line between newlines) around `c`.
pub fn paragraph_at(s: &str, c: usize) -> (usize, usize) {
    let a = s[..c].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let b = s[c..].find('\n').map(|i| c + i).unwrap_or(s.len());
    (a, b)
}

/// Index of the caret entry for byte position `cursor`.
fn caret_index(l: &TextLayout, cursor: usize) -> Option<usize> {
    l.carets.iter().enumerate().filter(|(_, k)| k.index <= cursor).max_by_key(|(_, k)| k.index).map(|(i, _)| i)
}

/// Caret one line up (`dir` = -1) or down (+1) at the same x.
pub fn vertical(l: &TextLayout, cursor: usize, dir: i32, len: usize) -> usize {
    let Some(ci) = caret_index(l, cursor) else { return cursor };
    let (x, line) = (l.carets[ci].x, l.carets[ci].line as i32);
    let target = line + dir;
    let last = l.carets.iter().map(|c| c.line).max().unwrap_or(0) as i32;
    if target < 0 {
        return 0;
    }
    if target > last {
        return len;
    }
    l.carets
        .iter()
        .filter(|c| c.line as i32 == target)
        .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
        .map(|c| c.index)
        .unwrap_or(cursor)
}

impl App {
    /// Text byte index nearest to document point `p` in text `id`.
    pub fn caret_at(&self, id: u64, p: Point) -> Option<usize> {
        let s = self.doc.get(id)?;
        let l = self.cache.get(id)?.text.clone()?;
        let lp = s.transform.inverse() * p;
        if let Some(op) = &l.on_path {
            let best = op.iter().enumerate().min_by(|a, b| (a.1.0 - lp).hypot2().total_cmp(&(b.1.0 - lp).hypot2()))?;
            return Some(l.carets[best.0].index);
        }
        // Nearest line box (lines can differ in height and sit in columns).
        let dist = |b: &crate::text::LineBox| {
            let dy = if lp.y < b.y0 { b.y0 - lp.y } else if lp.y > b.y1 { lp.y - b.y1 } else { 0.0 };
            let dx = if lp.x < b.x0 { b.x0 - lp.x } else if lp.x > b.x1 { lp.x - b.x1 } else { 0.0 };
            dy * 4.0 + dx
        };
        let line = l.lines.iter().enumerate().filter(|(_, b)| !b.hidden).min_by(|a, b| dist(a.1).total_cmp(&dist(b.1))).map(|(i, _)| i).unwrap_or(0);
        l.carets.iter().filter(|k| k.line == line).min_by(|a, b| (a.x - lp.x).abs().total_cmp(&(b.x - lp.x).abs())).map(|k| k.index)
    }

    /// Mouse press in the edited text: place the caret, extend with Shift,
    /// select a word on double-click and a paragraph on triple-click.
    pub fn text_mouse(&mut self, id: u64, p: Point, shift: bool, clicks: u8) {
        let Some(idx) = self.caret_at(id, p) else { return };
        let content = match self.doc.get(self.thread_head(id)).map(|s| &s.kind) {
            Some(ShapeKind::Text(t)) => t.content.clone(),
            _ => return,
        };
        let Some(te) = self.text_edit.as_mut() else { return };
        match clicks {
            2 => (te.anchor, te.cursor) = word_at(&content, idx),
            3.. => (te.anchor, te.cursor) = paragraph_at(&content, idx),
            _ => {
                te.cursor = idx;
                if !shift {
                    te.anchor = idx;
                }
            }
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let te = self.text_edit.as_ref()?;
        let Some(ShapeKind::Text(t)) = self.doc.get(self.thread_head(te.id)).map(|s| &s.kind) else { return None };
        let (a, b) = (te.anchor.min(te.cursor), te.anchor.max(te.cursor));
        (a < b).then(|| t.content[a.min(t.content.len())..b.min(t.content.len())].to_string())
    }

    /// Keyboard input while editing text.
    pub fn text_keys(&mut self, ctx: &egui::Context, events: &[egui::Event]) {
        let Some(te) = &self.text_edit else { return };
        let id = te.id;
        // A frame continuing a thread edits the story in its first frame;
        // its carets already index that story.
        let story = self.thread_head(id);
        let Some(ShapeKind::Text(td)) = self.doc.get(story).map(|s| &s.kind) else {
            self.text_edit = None;
            return;
        };
        // Edits go through `TextData::edit` so character styles stay put.
        let mut td = td.clone();
        let mut s = td.content.clone();
        let layout = self.cache.get(id).and_then(|c| c.text.clone());
        let mut cur = te.cursor.min(s.len());
        let mut anchor = te.anchor.min(s.len());
        let mut changed = false;
        let mut exit = false;
        // Replace the selection with `text`.
        let mut insert = |s: &mut String, cur: &mut usize, anchor: &mut usize, text: &str| {
            let (a, b) = ((*anchor).min(*cur), (*anchor).max(*cur));
            td.edit(a..b, text);
            *s = td.content.clone();
            *cur = a + text.len();
            *anchor = *cur;
        };
        for e in events {
            match e {
                egui::Event::Text(t) | egui::Event::Paste(t) => {
                    insert(&mut s, &mut cur, &mut anchor, t);
                    changed = true;
                }
                egui::Event::Copy | egui::Event::Cut => {
                    let (a, b) = (anchor.min(cur), anchor.max(cur));
                    if a < b {
                        ctx.copy_text(s[a..b].to_string());
                        if matches!(e, egui::Event::Cut) {
                            insert(&mut s, &mut cur, &mut anchor, "");
                            changed = true;
                        }
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                    use egui::Key;
                    let has_sel = anchor != cur;
                    let mut moved = None;
                    match key {
                        Key::Backspace | Key::Delete => {
                            if !has_sel {
                                anchor = match (key, m.command || m.alt) {
                                    (Key::Backspace, true) => word_left(&s, cur),
                                    (Key::Backspace, false) => prev_char(&s, cur),
                                    (_, true) => word_right(&s, cur),
                                    _ => next_char(&s, cur),
                                };
                            }
                            insert(&mut s, &mut cur, &mut anchor, "");
                            changed = true;
                        }
                        Key::ArrowLeft => {
                            moved = Some(if has_sel && !m.shift {
                                anchor.min(cur)
                            } else if m.command || m.alt {
                                word_left(&s, cur)
                            } else {
                                prev_char(&s, cur)
                            })
                        }
                        Key::ArrowRight => {
                            moved = Some(if has_sel && !m.shift {
                                anchor.max(cur)
                            } else if m.command || m.alt {
                                word_right(&s, cur)
                            } else {
                                next_char(&s, cur)
                            })
                        }
                        Key::ArrowUp | Key::ArrowDown => {
                            let dir = if *key == Key::ArrowUp { -1 } else { 1 };
                            moved = Some(match &layout {
                                Some(l) if l.on_path.is_none() => vertical(l, cur, dir, s.len()),
                                _ => if dir < 0 { 0 } else { s.len() },
                            });
                        }
                        Key::Home => moved = Some(if m.command { 0 } else { paragraph_at(&s, cur).0 }),
                        Key::End => moved = Some(if m.command { s.len() } else { paragraph_at(&s, cur).1 }),
                        Key::Enter if !m.command => {
                            insert(&mut s, &mut cur, &mut anchor, "\n");
                            changed = true;
                        }
                        Key::Escape => exit = true,
                        Key::A if m.command => {
                            anchor = 0;
                            cur = s.len();
                        }
                        _ => {}
                    }
                    if let Some(c) = moved {
                        cur = c;
                        if !m.shift {
                            anchor = c;
                        }
                    }
                }
                _ => {}
            }
        }
        drop(insert);
        if changed {
            self.checkpoint("text-type");
            if let Some(sh) = self.doc.get_mut(story) {
                if let ShapeKind::Text(t) = &mut sh.kind {
                    t.content = s;
                    t.spans = td.spans;
                }
            }
        }
        if let Some(te) = &mut self.text_edit {
            te.cursor = cur;
            te.anchor = anchor;
        }
        if exit {
            self.finish_text_edit();
            self.tool = crate::app::Tool::Select;
        }
    }

    /// Highlight the selected range of the edited text.
    pub fn paint_text_selection(&self, painter: &egui::Painter) {
        let Some(te) = &self.text_edit else { return };
        let (a, b) = (te.anchor.min(te.cursor), te.anchor.max(te.cursor));
        if a == b {
            return;
        }
        let (Some(s), Some(c)) = (self.doc.get(te.id), self.cache.get(te.id)) else { return };
        let Some(l) = &c.text else { return };
        let fill = Color32::from_rgba_unmultiplied(0x3D, 0x8B, 0xFF, 70);
        let to = |p: Point| self.view.to_screen(s.transform * p);
        let lh = l.line_height;
        if let Some(op) = &l.on_path {
            // One quad per selected glyph, standing on the path.
            for (i, w) in l.carets.windows(2).enumerate() {
                if w[0].index < a || w[1].index > b {
                    continue;
                }
                let (p0, a0) = op[i];
                let (p1, a1) = op[i + 1];
                let up = |ang: f64| kurbo::Vec2::new(ang.sin(), -ang.cos()) * lh * 0.85;
                let quad = vec![to(p0), to(p1), to(p1 + up(a1)), to(p0 + up(a0))];
                painter.add(egui::Shape::convex_polygon(quad, fill, Stroke::NONE));
            }
            return;
        }
        let last = l.carets.iter().map(|k| k.line).max().unwrap_or(0);
        for line in 0..=last {
            let xs: Vec<f64> = l.carets.iter().filter(|k| k.line == line && k.index >= a && k.index <= b).map(|k| k.x).collect();
            if xs.is_empty() {
                continue;
            }
            let x0 = xs.iter().copied().fold(f64::MAX, f64::min);
            let mut x1 = xs.iter().copied().fold(f64::MIN, f64::max);
            // Show selected line breaks as a little extra width.
            let line_end = l.carets.iter().filter(|k| k.line == line).map(|k| k.index).max().unwrap_or(0);
            if b > line_end && line < last {
                x1 += lh * 0.25;
            }
            if x1 <= x0 {
                continue;
            }
            let Some(lb) = l.lines.get(line) else { continue };
            if lb.hidden {
                continue;
            }
            let (y0, y1) = (lb.y0, lb.y1);
            let quad = vec![to(Point::new(x0, y0)), to(Point::new(x1, y0)), to(Point::new(x1, y1)), to(Point::new(x0, y1))];
            painter.add(egui::Shape::convex_polygon(quad, fill, Stroke::NONE));
        }
    }

    /// Language of text `id` (its own, else the default from Preferences).
    pub fn text_lang(&self, id: u64) -> String {
        match self.doc.get(self.thread_head(id)).map(|s| &s.kind) {
            Some(ShapeKind::Text(t)) => t.lang.clone().unwrap_or_else(|| self.settings.language.clone()),
            _ => self.settings.language.clone(),
        }
    }

    /// Recheck the spelling of the edited text when it changes.
    pub fn spell_tick(&mut self, ctx: &egui::Context) {
        let Some(te) = &self.text_edit else {
            self.spell_marks = None;
            return;
        };
        if !self.settings.spell_check {
            self.spell_marks = None;
            return;
        }
        let id = te.id;
        let head = self.thread_head(id);
        let Some(sh) = self.doc.get(head) else { return };
        let rev = sh.rev;
        if matches!(&self.spell_marks, Some((i, r, _)) if *i == id && *r == rev) {
            return;
        }
        let ShapeKind::Text(t) = &sh.kind else { return };
        let content = t.content.clone();
        let lang = self.text_lang(id);
        let marks = self.spell.misspelled(&content, &lang);
        if self.spell.loading(&lang) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            self.spell_marks = None;
            return;
        }
        self.spell_marks = Some((id, rev, marks));
    }

    /// Misspelled word of the edited text at document point `p`.
    pub fn misspelled_at(&self, p: Point) -> Option<std::ops::Range<usize>> {
        let te = self.text_edit.as_ref()?;
        let (id, _, marks) = self.spell_marks.as_ref()?;
        if *id != te.id {
            return None;
        }
        let i = self.caret_at(te.id, p)?;
        marks.iter().find(|r| r.start <= i && i <= r.end).cloned()
    }

    /// Wavy red underlines under misspelled words of the edited text.
    pub fn paint_spelling(&self, painter: &egui::Painter) {
        let (Some(te), Some((id, _, marks))) = (&self.text_edit, &self.spell_marks) else { return };
        if *id != te.id || marks.is_empty() {
            return;
        }
        let (Some(s), Some(c)) = (self.doc.get(te.id), self.cache.get(te.id)) else { return };
        let Some(l) = &c.text else { return };
        if l.on_path.is_some() {
            return;
        }
        let to = |p: Point| self.view.to_screen(s.transform * p);
        let col = Color32::from_rgb(0xE5, 0x48, 0x4D);
        for r in marks {
            if r.end < l.start || r.start > l.end {
                continue;
            }
            let at = |i: usize| l.carets.iter().find(|k| k.index == i);
            let (Some(a), Some(b)) = (at(r.start), at(r.end)) else { continue };
            if a.line != b.line || b.x <= a.x {
                continue;
            }
            let Some(lb) = l.lines.get(a.line) else { continue };
            if lb.hidden {
                continue;
            }
            let y = lb.baseline + (lb.y1 - lb.baseline) * 0.45;
            let (p0, p1) = (to(Point::new(a.x, y)), to(Point::new(b.x, y)));
            let len = (p1 - p0).length();
            if len < 2.0 {
                continue;
            }
            let dir = (p1 - p0) / len;
            let nrm = egui::vec2(-dir.y, dir.x);
            let step = 2.0;
            let n = (len / step).ceil() as usize;
            let pts: Vec<egui::Pos2> = (0..=n).map(|k| p0 + dir * (k as f32 * step).min(len) + nrm * if k % 2 == 0 { -1.2 } else { 1.2 }).collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.0, col)));
        }
    }

    /// Replace `range` of the edited story with `text`, caret after it.
    pub fn replace_text_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        let Some(te) = &self.text_edit else { return };
        let head = self.thread_head(te.id);
        self.checkpoint("");
        if let Some(ShapeKind::Text(t)) = self.doc.get_mut(head).map(|s| &mut s.kind) {
            if range.end <= t.content.len() {
                t.edit(range.clone(), text);
            }
        }
        if let Some(te) = &mut self.text_edit {
            te.cursor = range.start + text.len();
            te.anchor = te.cursor;
        }
    }
}
