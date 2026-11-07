//! Components, like Figma's. A main component is a group marked as the
//! source; an instance is a group whose children mirror the main's (placed by
//! the instance's own transform). Restyling a layer inside an instance (fill,
//! stroke, opacity, visibility, name, text) is kept as an override; everything
//! else follows the main. Deleting the main detaches its instances.

use std::collections::BTreeMap;

use kurbo::{Affine, Point};

use crate::app::App;
use crate::document::{Component, Document, InstanceLink, Shape, ShapeKind};

/// The main's children as they appear in an instance: placed by `xf`, with
/// stable ids (`ids`, extended for new layers) and overrides applied. Nested
/// component roles are dropped (they stay live through their own main).
pub fn build_children(main: &Shape, link: &mut InstanceLink, doc: &mut Document) -> Vec<Shape> {
    let Some(Component::Main { frame, .. }) = &main.component else { return Vec::new() };
    let m = link.xf * frame.inverse();
    let mut out = Vec::new();
    fn remap(s: &mut Shape, link: &mut InstanceLink, doc: &mut Document) {
        let src = s.id;
        s.id = *link.ids.entry(src).or_insert_with(|| doc.fresh_id());
        s.rev = 0;
        s.component = None;
        if let Some(o) = link.overrides.get(&src) {
            o.apply(s);
        }
        if let Some(children) = s.kind.children_mut() {
            for c in children {
                remap(c, link, doc);
            }
        }
    }
    for c in main.kind.children().into_iter().flatten() {
        let mut c = c.clone();
        c.apply_transform(m);
        remap(&mut c, link, doc);
        out.push(c);
    }
    out
}

/// Equal apart from revision stamps.
fn same(a: &[Shape], b: &[Shape]) -> bool {
    fn clear(s: &mut Shape) {
        s.rev = 0;
        if let Some(c) = s.kind.children_mut() {
            c.iter_mut().for_each(clear);
        }
    }
    let strip = |v: &[Shape]| {
        let mut v = v.to_vec();
        v.iter_mut().for_each(clear);
        v
    };
    a.len() == b.len() && strip(a) == strip(b)
}

fn find<'a>(list: &'a [Shape], id: u64) -> Option<&'a Shape> {
    for s in list {
        if s.id == id {
            return Some(s);
        }
        if let Some(f) = s.kind.children().and_then(|c| find(c, id)) {
            return Some(f);
        }
    }
    None
}

fn walk<'a>(list: &'a [Shape], out: &mut Vec<&'a Shape>) {
    for s in list {
        out.push(s);
        if let Some(c) = s.kind.children() {
            walk(c, out);
        }
    }
}

impl App {
    pub fn is_main(&self, id: u64) -> bool {
        matches!(self.doc.get(id).and_then(|s| s.component.as_ref()), Some(Component::Main { .. }))
    }

    /// A component (main or instance) or a layer inside one.
    pub fn in_component(&self, id: u64) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if self.doc.get(c).map(|s| s.component.is_some()).unwrap_or(false) {
                return true;
            }
            cur = self.doc.parent_of(c);
        }
        false
    }

    pub fn instance_link(&self, id: u64) -> Option<&InstanceLink> {
        match self.doc.get(id)?.component.as_ref()? {
            Component::Instance(l) => Some(l),
            _ => None,
        }
    }

    /// Every main component, in layer order.
    pub fn mains(&self) -> Vec<u64> {
        self.doc.all_ids().into_iter().filter(|id| self.is_main(*id)).collect()
    }

    pub fn instances_of(&self, main: u64) -> Vec<u64> {
        self.doc.all_ids().into_iter().filter(|id| self.instance_link(*id).map(|l| l.main == main).unwrap_or(false)).collect()
    }

    /// Turn the selection into a main component (Ctrl+Alt+K). A single group
    /// is marked in place; anything else is grouped first.
    pub fn create_component(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let single_group = self.selection.len() == 1
            && self.doc.get(self.selection[0]).map(|s| matches!(s.kind, ShapeKind::Group { .. }) && s.component.is_none()).unwrap_or(false);
        if single_group {
            self.checkpoint("");
        } else {
            // Grouping makes its own undo step.
            self.group_selection();
        }
        let Some(id) = self.selection.first().copied() else { return };
        let n = self.mains().len() + 1;
        let origin = self.selection_bounds().map(|b| b.origin().to_vec2()).unwrap_or_default();
        if let Some(s) = self.doc.get_mut(id) {
            s.component = Some(Component::main(Affine::translate(origin)));
            if s.name == "Group" {
                s.name = format!("Component {n}");
            }
        }
        self.dirty = true;
        self.notify("Created a component — copy, paste or drag it from Components to make instances");
    }

    /// A new instance of `main` centred on `at`; returns its id.
    pub fn insert_instance(&mut self, main: u64, at: Point) -> Option<u64> {
        let m = self.doc.get(main)?.clone();
        let Some(Component::Main { frame, .. }) = m.component else { return None };
        let b = self.cache.update(&m, &mut self.fonts).bounds?;
        self.checkpoint("");
        let mut link = InstanceLink { main, xf: Affine::translate(at - b.center()) * frame, ids: BTreeMap::new(), overrides: BTreeMap::new() };
        let children = build_children(&m, &mut link, &mut self.doc);
        let mut g = Shape::new(ShapeKind::Group { children });
        g.fill = None;
        g.name = m.name.clone();
        g.component = Some(Component::Instance(link));
        let id = self.doc.add_shape(g);
        self.selection = vec![id];
        self.dirty = true;
        Some(id)
    }

    /// Selected instances, or the instances containing selected layers.
    fn selected_instances(&self) -> Vec<u64> {
        let mut out = Vec::new();
        for id in &self.selection {
            let mut cur = Some(*id);
            while let Some(c) = cur {
                if self.instance_link(c).is_some() {
                    if !out.contains(&c) {
                        out.push(c);
                    }
                    break;
                }
                cur = self.doc.parent_of(c);
            }
        }
        out
    }

    pub fn has_instance_selected(&self) -> bool {
        !self.selected_instances().is_empty()
    }

    /// Make selected instances plain groups (Ctrl+Alt+B).
    pub fn detach_instance(&mut self) {
        let ids = self.selected_instances();
        if ids.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in &ids {
            if let Some(s) = self.doc.get_mut(*id) {
                s.component = None;
            }
        }
        self.selection = ids;
        self.dirty = true;
    }

    /// Drop every override of the selected instances.
    pub fn reset_instance(&mut self) {
        let ids = self.selected_instances();
        if ids.is_empty() {
            return;
        }
        self.checkpoint("");
        for id in ids {
            if let Some(Component::Instance(l)) = self.doc.get_mut(id).and_then(|s| s.component.as_mut()) {
                l.overrides.clear();
            }
            // Rebuild now (not on the next tick, which would read the reset
            // as an edit of the instance).
            self.resync(id);
        }
        self.dirty = true;
    }

    /// Select the main component of the selected instance and show it.
    pub fn go_to_main(&mut self) {
        let Some(main) = self.selected_instances().first().and_then(|i| self.instance_link(*i)).map(|l| l.main) else { return };
        self.selection = vec![main];
        if let Some(b) = self.selection_bounds() {
            self.zoom_to_rect(b.inflate(b.width().max(b.height()) * 0.5, b.width().max(b.height()) * 0.5), 96.0);
        }
    }

    /// Duplicates and pastes of a main component become instances of it
    /// (when it is still in the document). `src` is the copied shape.
    pub fn instance_from_copy(&mut self, src: &Shape, new_id: u64) {
        let Some(Component::Main { .. }) = &src.component else { return };
        if !self.is_main(src.id) || src.id == new_id {
            return;
        }
        let Some(copy) = self.doc.get(new_id).cloned() else { return };
        // The copy's frame went wherever the copy was moved: exactly the
        // instance placement.
        let Some(Component::Main { frame, .. }) = copy.component.clone() else { return };
        // Pair layers up by position (same structure as when copied).
        let mut a = Vec::new();
        let mut b = Vec::new();
        walk(src.kind.children().map(|c| c.as_slice()).unwrap_or(&[]), &mut a);
        walk(copy.kind.children().map(|c| c.as_slice()).unwrap_or(&[]), &mut b);
        let ids = if a.len() == b.len() { a.iter().zip(&b).map(|(x, y)| (x.id, y.id)).collect() } else { BTreeMap::new() };
        let link = InstanceLink { main: src.id, xf: frame, ids, overrides: BTreeMap::new() };
        let name = self.doc.get(src.id).map(|m| m.name.clone()).unwrap_or_default();
        if let Some(s) = self.doc.get_mut(new_id) {
            s.component = Some(Component::Instance(link));
            s.name = name;
        }
    }

    /// Rebuild one instance from its main (no override detection).
    fn resync(&mut self, id: u64) -> bool {
        let Some(inst) = self.doc.get(id).cloned() else { return false };
        let Some(Component::Instance(mut link)) = inst.component.clone() else { return false };
        let Some(main) = self.doc.get(link.main).cloned() else { return false };
        let children = build_children(&main, &mut link, &mut self.doc);
        let cur = inst.kind.children().cloned().unwrap_or_default();
        let changed = !same(&cur, &children) || inst.component.as_ref() != Some(&Component::Instance(link.clone()));
        if changed {
            // `replace` stamps the whole subtree with a fresh revision, so the
            // render cache redraws the mirrored layers (their ids persist).
            let mut s = inst.clone();
            if let Some(c) = s.kind.children_mut() {
                *c = children;
            }
            s.component = Some(Component::Instance(link));
            self.doc.replace(s);
        }
        let rev = self.doc.get(id).map(|s| s.rev).unwrap_or(0);
        self.comp_sync.insert(id, (main.rev, rev));
        changed
    }

    /// Per frame: keep instances in step with their mains. An instance that
    /// changed while its main didn't was restyled by the user: the
    /// differences become overrides. Instances of a deleted main detach.
    pub fn components_tick(&mut self) {
        if self.doc.revision() == self.comp_rev {
            return;
        }
        let mut wrote = false;
        for id in self.doc.all_ids() {
            let Some(inst) = self.doc.get(id) else { continue };
            let Some(Component::Instance(link)) = &inst.component else { continue };
            let (main_id, inst_rev) = (link.main, inst.rev);
            let main_rev = match self.doc.get(main_id) {
                Some(m) if matches!(m.component, Some(Component::Main { .. })) => m.rev,
                _ => {
                    // Main gone (or no longer a component): a plain group now.
                    if let Some(s) = self.doc.get_mut(id) {
                        s.component = None;
                    }
                    wrote = true;
                    continue;
                }
            };
            // An instance inside its own main would recurse.
            let mut p = self.doc.parent_of(id);
            let mut inside = false;
            while let Some(x) = p {
                inside |= x == main_id;
                p = self.doc.parent_of(x);
            }
            if inside {
                continue;
            }
            let last = self.comp_sync.get(&id).copied();
            if last == Some((main_rev, inst_rev)) {
                continue;
            }
            let instance_edited = matches!(last, Some((m, i)) if m == main_rev && i != inst_rev);
            if instance_edited {
                self.record_overrides(id);
            }
            wrote |= self.resync(id);
        }
        // Changes can cascade (instances inside other mains): look again
        // next frame until nothing moves.
        if wrote {
            self.egui_ctx.request_repaint();
        } else {
            self.comp_rev = self.doc.revision();
        }
    }

    /// Compare an instance's layers with the main's version and store the
    /// differences as overrides; deleted layers become hidden ones.
    fn record_overrides(&mut self, id: u64) {
        let Some(inst) = self.doc.get(id).cloned() else { return };
        let Some(Component::Instance(link)) = inst.component.clone() else { return };
        let Some(main) = self.doc.get(link.main).cloned() else { return };
        let mut probe = link.clone();
        let expected = build_children(&main, &mut probe, &mut self.doc);
        let actual = inst.kind.children().cloned().unwrap_or_default();
        let mut exp = Vec::new();
        walk(&expected, &mut exp);
        let inv: BTreeMap<u64, u64> = probe.ids.iter().map(|(m, i)| (*i, *m)).collect();
        let mut overrides = link.overrides.clone();
        for e in exp {
            let Some(src) = inv.get(&e.id) else { continue };
            let o = overrides.entry(*src).or_default();
            match find(&actual, e.id) {
                Some(a) => o.record(e, a),
                None => o.visible = Some(false),
            }
        }
        overrides.retain(|_, o| !o.is_empty());
        if overrides != link.overrides {
            if let Some(Component::Instance(l)) = self.doc.get_mut(id).and_then(|s| s.component.as_mut()) {
                l.overrides = overrides;
            }
        }
    }

    /// Number of overridden layers of instance `id`.
    pub fn override_count(&self, id: u64) -> usize {
        self.instance_link(id).map(|l| l.overrides.len()).unwrap_or(0)
    }
}
