//! Shapes with several fills or strokes expanded into plain shapes (one
//! paint each) for exporters and the SVG renderer.


use crate::document::{BlendMode, Shape, ShapeKind};

/// Whether `s` needs [`expand`].
pub fn needs_expand(s: &Shape) -> bool {
    !s.fills.is_empty() || !s.strokes.is_empty()
}

fn plain(s: &Shape) -> Shape {
    let mut c = s.clone();
    c.fill = None;
    c.stroke = None;
    c.fills.clear();
    c.strokes.clear();
    c.shadow = None;
    c.inner_shadow = None;
    c.blur = 0.0;
    c.noise = 0.0;
    c.outlines.clear();
    c.mask = None;
    c.blend = BlendMode::Normal;
    c.opacity = 1.0;
    c.component = None;
    c.fill_key = None;
    c.stroke_key = None;
    c.text_wrap = None;
    c
}

/// `s` as a group of single-paint shapes, bottom to top: the fills, the
/// extra strokes, the stroke. The group keeps the effects, opacity, blend
/// mode and mask; each member keeps the clip. None when there is nothing
/// to expand.
pub fn expand(s: &Shape) -> Option<Shape> {
    if !needs_expand(s) {
        return None;
    }
    let base = plain(s);
    let mut children = Vec::new();
    for paint in s.fill.iter().chain(s.fills.iter()) {
        let mut c = base.clone();
        c.fill = Some(paint.clone());
        children.push(c);
    }
    for st in s.strokes.iter().chain(s.stroke.iter()) {
        let mut c = base.clone();
        c.stroke = Some(st.clone());
        children.push(c);
    }
    let mut g = Shape::new(ShapeKind::Group { children });
    g.id = s.id;
    g.name = s.name.clone();
    g.opacity = s.opacity;
    g.shadow = s.shadow.clone();
    g.inner_shadow = s.inner_shadow.clone();
    g.blur = s.blur;
    g.noise = s.noise;
    g.outlines = s.outlines.clone();
    g.mask = s.mask.clone();
    g.blend = s.blend;
    g.visible = s.visible;
    Some(g)
}
