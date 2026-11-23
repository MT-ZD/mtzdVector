//! The interface as agents see it: egui's AccessKit tree (every button,
//! field, checkbox, menu item, tab and label laid out this frame), flattened
//! into a list an agent can read and act on by id or label.

use egui::accesskit::{self, Role};

#[derive(Clone, Debug)]
pub struct UiNode {
    pub id: u64,
    pub role: Role,
    pub label: String,
    pub value: String,
    pub rect: egui::Rect,
    pub depth: usize,
    pub disabled: bool,
    pub toggled: Option<bool>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
}

/// Short widget id for agents (7 hex digits of egui's id).
pub fn short(id: u64) -> String {
    format!("{:07x}", id & 0xFFF_FFFF)
}

/// Icon glyphs (private use area) say nothing to an agent.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !('\u{E000}'..='\u{F8FF}').contains(c)).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Roles worth listing even without text (they can be clicked or typed in).
fn interactive(r: Role) -> bool {
    matches!(
        r,
        Role::Button
            | Role::DefaultButton
            | Role::CheckBox
            | Role::RadioButton
            | Role::Switch
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::SearchInput
            | Role::SpinButton
            | Role::Slider
            | Role::ComboBox
            | Role::MenuItem
            | Role::MenuItemCheckBox
            | Role::MenuItemRadio
            | Role::Tab
            | Role::Link
            | Role::ColorWell
            | Role::ListBoxOption
            | Role::TreeItem
    )
}

/// The current pass's tree (call at the end of the frame, with AccessKit on).
pub fn snapshot(ctx: &egui::Context) -> Vec<UiNode> {
    ctx.viewport(|vp| {
        let Some(st) = vp.this_pass.accesskit_state.as_ref() else { return Vec::new() };
        let by_id: std::collections::HashMap<accesskit::NodeId, (u64, &accesskit::Node)> = st.nodes.iter().map(|(id, n)| (id.accesskit_id(), (id.value(), n))).collect();
        let mut out = Vec::new();
        let root = egui::accesskit_root_id().accesskit_id();
        let mut stack = vec![(root, 0usize)];
        while let Some((nid, depth)) = stack.pop() {
            let Some(&(id, n)) = by_id.get(&nid) else { continue };
            let label = clean(n.label().unwrap_or(""));
            let mut value = clean(n.value().unwrap_or(""));
            if value.is_empty()
                && let Some(v) = n.numeric_value()
            {
                value = format!("{v}");
            }
            // Text runs repeat their label's text.
            let shown = nid != root && !n.is_hidden() && n.role() != Role::TextRun && (interactive(n.role()) || !label.is_empty() || !value.is_empty());
            if shown && let Some(b) = n.bounds() {
                out.push(UiNode {
                    id,
                    role: n.role(),
                    label,
                    value,
                    rect: egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)),
                    depth,
                    disabled: n.is_disabled(),
                    toggled: n.toggled().map(|t| t == accesskit::Toggled::True),
                    selected: n.is_selected(),
                    expanded: n.is_expanded(),
                });
            }
            let d = if shown { depth + 1 } else { depth };
            for c in n.children().iter().rev() {
                stack.push((*c, d));
            }
        }
        out
    })
}

fn role_name(r: Role) -> String {
    match r {
        Role::Button | Role::DefaultButton => "button".into(),
        Role::CheckBox => "checkbox".into(),
        Role::RadioButton => "radio".into(),
        Role::TextInput | Role::SearchInput | Role::MultilineTextInput => "textbox".into(),
        Role::SpinButton => "number".into(),
        Role::Slider => "slider".into(),
        Role::ComboBox => "dropdown".into(),
        Role::MenuItem | Role::MenuItemCheckBox | Role::MenuItemRadio => "menuitem".into(),
        Role::Label => "text".into(),
        Role::Tab => "tab".into(),
        Role::Link => "link".into(),
        Role::Window | Role::Dialog => "window".into(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// One line per widget, indented by nesting:
/// `#id role "label" = value [x,y wxh] (checked, disabled)`.
pub fn format(nodes: &[UiNode], filter: &str, max: usize) -> String {
    let f = filter.to_lowercase();
    let mut out = String::new();
    let mut n = 0;
    for node in nodes {
        if !f.is_empty() && !node.label.to_lowercase().contains(&f) && !node.value.to_lowercase().contains(&f) && role_name(node.role) != f {
            continue;
        }
        if n == max {
            out.push_str(&format!("… {} more (use filter)\n", nodes.len() - n));
            break;
        }
        n += 1;
        let indent = if f.is_empty() { "  ".repeat(node.depth.min(12)) } else { String::new() };
        let mut line = format!("{indent}#{} {}", short(node.id), role_name(node.role));
        if !node.label.is_empty() {
            line.push_str(&format!(" {:?}", node.label));
        }
        if !node.value.is_empty() && node.value != node.label {
            line.push_str(&format!(" = {:?}", node.value));
        }
        let r = node.rect;
        line.push_str(&format!(" [{:.0},{:.0} {:.0}x{:.0}]", r.min.x, r.min.y, r.width(), r.height()));
        let mut flags = Vec::new();
        match node.toggled {
            Some(true) => flags.push("checked"),
            Some(false) => flags.push("unchecked"),
            None => {}
        }
        if node.selected == Some(true) {
            flags.push("selected");
        }
        match node.expanded {
            Some(true) => flags.push("expanded"),
            Some(false) => flags.push("collapsed"),
            None => {}
        }
        if node.disabled {
            flags.push("disabled");
        }
        if !flags.is_empty() {
            line.push_str(&format!(" ({})", flags.join(", ")));
        }
        out.push_str(&line);
        out.push('\n');
    }
    if out.is_empty() {
        out = "no matching widgets".into();
    }
    out
}

/// A widget by id, or by label/value (exact match first, then substring;
/// `role` narrows it, `index` picks among several).
pub fn find<'a>(nodes: &'a [UiNode], id: Option<&str>, label: Option<&str>, role: Option<&str>, index: usize) -> Result<&'a UiNode, String> {
    if let Some(id) = id {
        let id = id.trim().trim_start_matches('#').to_lowercase();
        return nodes.iter().find(|n| short(n.id) == id).ok_or(format!("no widget #{id} on screen now (call ui_tree again)"));
    }
    let label = label.ok_or("pass `id` or `label`")?.to_lowercase();
    let role_ok = |n: &&UiNode| role.is_none_or(|r| role_name(n.role) == r.to_lowercase());
    let exact: Vec<&UiNode> = nodes.iter().filter(role_ok).filter(|n| n.label.to_lowercase() == label || n.value.to_lowercase() == label).collect();
    let hits = if exact.is_empty() { nodes.iter().filter(role_ok).filter(|n| n.label.to_lowercase().contains(&label)).collect() } else { exact };
    // Prefer something clickable over the text next to it.
    let mut hits: Vec<&UiNode> = hits;
    hits.sort_by_key(|n| !interactive(n.role));
    match hits.get(index) {
        Some(n) => Ok(n),
        None if hits.is_empty() => Err(format!("no widget labelled {label:?} on screen (see ui_tree)")),
        None => Err(format!("only {} widgets match {label:?}", hits.len())),
    }
}
