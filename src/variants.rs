//! Component variants (mains grouped in a set, each with property values
//! such as State = Hover) and instance properties: switching an instance
//! to another variant keeps its overrides on layers of the same name.

use std::collections::BTreeMap;

use kurbo::Affine;

use crate::app::App;
use crate::document::{Component, Shape, ShapeKind};

/// Layer paths of a main's members ("Label", "Icon/Shape", with "#n" for
/// repeated names) → layer id.
pub fn layer_paths(main: &Shape) -> BTreeMap<String, u64> {
    fn walk(list: &[Shape], prefix: &str, out: &mut BTreeMap<String, u64>) {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for s in list {
            let n = seen.entry(s.name.as_str()).or_default();
            let path = if *n == 0 { format!("{prefix}{}", s.name) } else { format!("{prefix}{}#{n}", s.name) };
            *n += 1;
            out.insert(path.clone(), s.id);
            if let Some(c) = s.kind.children() {
                walk(c, &format!("{path}/"), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(main.kind.children().map(|c| c.as_slice()).unwrap_or(&[]), "", &mut out);
    out
}

fn set_of(s: &Shape) -> Option<&str> {
    match &s.component {
        Some(Component::Main { set: Some(set), .. }) => Some(set),
        _ => None,
    }
}

pub fn props_of(s: &Shape) -> BTreeMap<String, String> {
    match &s.component {
        Some(Component::Main { props, .. }) => props.clone(),
        _ => BTreeMap::new(),
    }
}

impl App {
    /// The mains in `main`'s variant set (itself only when it has none).
    pub fn variants_of(&self, main: u64) -> Vec<u64> {
        let Some(set) = self.doc.get(main).and_then(set_of).map(str::to_string) else { return vec![main] };
        self.mains().into_iter().filter(|m| self.doc.get(*m).and_then(set_of) == Some(set.as_str())).collect()
    }

    /// Every property of the set and the values its variants use.
    pub fn variant_options(&self, main: u64) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for m in self.variants_of(main) {
            for (k, v) in self.doc.get(m).map(props_of).unwrap_or_default() {
                let e = out.entry(k).or_default();
                if !e.contains(&v) {
                    e.push(v);
                }
            }
        }
        out
    }

    /// Make the selected mains variants of one set (named after the first).
    pub fn combine_as_variants(&mut self) {
        let mains: Vec<u64> = self.selection.iter().copied().filter(|id| self.is_main(*id)).collect();
        if mains.len() < 2 {
            self.notify("Select two or more main components to combine");
            return;
        }
        self.checkpoint("");
        let set = self.doc.get(mains[0]).and_then(|s| set_of(s).map(str::to_string).or_else(|| Some(s.name.clone()))).unwrap_or_else(|| "Component".into());
        for id in &mains {
            if let Some(s) = self.doc.get_mut(*id) {
                let name = s.name.clone();
                if let Some(Component::Main { set: st, props, .. }) = &mut s.component {
                    *st = Some(set.clone());
                    if props.is_empty() {
                        props.insert("Variant".into(), name);
                    }
                }
            }
        }
        self.dirty = true;
        self.notify(format!("{} variants of {set}", mains.len()));
    }

    /// A copy of main `id` as a new variant beside it.
    pub fn add_variant(&mut self, id: u64) -> Option<u64> {
        let src = self.doc.get(id)?.clone();
        let b = self.cache.update(&src, &mut self.fonts).bounds?;
        self.checkpoint("");
        let set = set_of(&src).map(str::to_string).unwrap_or_else(|| src.name.clone());
        let mut props = props_of(&src);
        if props.is_empty() {
            props.insert("Variant".into(), src.name.clone());
        }
        // A value no variant uses yet for the first property.
        let key = props.keys().next().cloned().unwrap_or_else(|| "Variant".into());
        let used = self.variant_options(id).remove(&key).unwrap_or_default();
        let mut n = used.len() + 1;
        while used.contains(&format!("Variant {n}")) {
            n += 1;
        }
        let mut copy = src.clone();
        copy.id = 0;
        let dx = b.width() + 40.0;
        copy.apply_transform(Affine::translate((dx, 0.0)));
        let mut new_props = props.clone();
        new_props.insert(key, format!("Variant {n}"));
        let frame = match &copy.component {
            Some(Component::Main { frame, .. }) => *frame,
            _ => Affine::translate(b.origin().to_vec2() + kurbo::Vec2::new(dx, 0.0)),
        };
        copy.component = Some(Component::Main { frame, set: Some(set.clone()), props: new_props, library: None });
        copy.name = format!("{} {n}", set);
        let nid = self.doc.add_shape(copy);
        // The original joins the set too.
        if let Some(Some(Component::Main { set: st, props: p, .. })) = self.doc.get_mut(id).map(|s| s.component.as_mut()) {
            *st = Some(set);
            if p.is_empty() {
                *p = props;
            }
        }
        self.selection = vec![nid];
        self.dirty = true;
        Some(nid)
    }

    /// Set a property value of main `id` (renaming the key when `key` is
    /// new and `old_key` given).
    pub fn set_variant_prop(&mut self, id: u64, old_key: Option<&str>, key: &str, value: &str) {
        self.checkpoint("variant prop");
        if let Some(Some(Component::Main { props, .. })) = self.doc.get_mut(id).map(|s| s.component.as_mut()) {
            if let Some(k) = old_key.filter(|k| *k != key) {
                props.remove(k);
            }
            if key.is_empty() {
                return;
            }
            props.insert(key.to_string(), value.to_string());
        }
        self.dirty = true;
    }

    /// Switch instance `inst` to the variant of its set with `key` = `value`
    /// (and as many of its other properties as possible).
    pub fn set_instance_variant(&mut self, inst: u64, key: &str, value: &str) -> bool {
        let Some(link) = self.instance_link(inst).cloned() else { return false };
        let Some(cur) = self.doc.get(link.main).cloned() else { return false };
        let mut want = props_of(&cur);
        want.insert(key.to_string(), value.to_string());
        let best = self
            .variants_of(link.main)
            .into_iter()
            .filter_map(|m| self.doc.get(m).map(|s| (m, props_of(s))))
            .filter(|(_, p)| p.get(key).map(String::as_str) == Some(value))
            .max_by_key(|(_, p)| want.iter().filter(|(k, v)| p.get(*k) == Some(*v)).count())
            .map(|(m, _)| m);
        let Some(target) = best.filter(|t| *t != link.main) else { return false };
        let Some(tmain) = self.doc.get(target).cloned() else { return false };
        self.checkpoint("");
        // Carry instance ids and overrides over by layer path.
        let (old_paths, new_paths) = (layer_paths(&cur), layer_paths(&tmain));
        let old_by_id: BTreeMap<u64, &String> = old_paths.iter().map(|(p, id)| (*id, p)).collect();
        let mut ids = BTreeMap::new();
        let mut overrides = BTreeMap::new();
        for (src, local) in &link.ids {
            if let Some(nid) = old_by_id.get(src).and_then(|p| new_paths.get(*p)) {
                ids.insert(*nid, *local);
            }
        }
        for (src, o) in &link.overrides {
            if let Some(nid) = old_by_id.get(src).and_then(|p| new_paths.get(*p)) {
                overrides.insert(*nid, o.clone());
            }
        }
        // Keep the instance's top-left where it is.
        let (Some(Component::Main { frame: f1, .. }), Some(Component::Main { frame: f0, .. })) = (&tmain.component, &cur.component) else { return false };
        let place = link.xf * f0.inverse();
        let tl = |s: &Shape, app: &mut App| app.cache.update(s, &mut app.fonts).bounds.map(|b| b.origin()).unwrap_or_default();
        let (a, b) = (tl(&cur, self), tl(&tmain, self));
        let d = place * a - place * b;
        let xf = Affine::translate(d) * place * *f1;
        let new_link = crate::document::InstanceLink { main: target, xf, ids, overrides };
        if let Some(s) = self.doc.get_mut(inst) {
            s.component = Some(Component::Instance(new_link));
            s.name = tmain.name.clone();
        }
        self.comp_sync.remove(&inst);
        self.comp_rev = u64::MAX;
        self.components_tick();
        self.dirty = true;
        true
    }

    /// Text layers of instance `inst`: (layer id, name, content).
    pub fn instance_texts(&self, inst: u64) -> Vec<(u64, String, String)> {
        fn walk(list: &[Shape], out: &mut Vec<(u64, String, String)>) {
            for s in list {
                if let ShapeKind::Text(t) = &s.kind {
                    out.push((s.id, s.name.clone(), t.content.clone()));
                }
                if let Some(c) = s.kind.children() {
                    walk(c, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(c) = self.doc.get(inst).and_then(|s| s.kind.children()) {
            walk(c, &mut out);
        }
        out
    }

    /// Replace the text of a layer inside an instance (kept as an override).
    pub fn set_instance_text(&mut self, layer: u64, text: &str) {
        self.checkpoint("instance text");
        if let Some(mut s) = self.doc.get(layer).cloned() {
            if let ShapeKind::Text(td) = &mut s.kind {
                let n = td.content.len();
                td.edit(0..n, text);
            }
            self.doc.replace(s);
        }
    }
}
