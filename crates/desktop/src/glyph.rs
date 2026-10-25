//! A shape per machine, so a row or a chip says which machine at a glance:
//! six solid shapes that stay distinct at row size. The shape says which
//! machine; how it stands is a dot on its round holder, or words beside a
//! small one.
//!
//! Shapes follow from the machine ids alone, so every device with the same
//! machines draws the same ones: in id order, each takes the shape its id
//! hashes to, or the next one free. Past six machines shapes repeat; the
//! name always sits beside the shape.

use std::collections::HashMap;
use std::f32::consts::PI;

use gpui_kit::{
    App, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Styled, Window, canvas, point, px,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    Circle,
    Triangle,
    Square,
    Diamond,
    Plus,
    Star,
    /// The same six drawn as outlines, for a seventh machine on.
    Ring,
    OpenTriangle,
    OpenSquare,
    OpenDiamond,
    OpenPlus,
    OpenStar,
}

/// Filled first, then outlined: an index here is what's kept on disk, so
/// new ones only ever go at the end.
pub const SHAPES: [Shape; 12] = [
    Shape::Circle,
    Shape::Triangle,
    Shape::Square,
    Shape::Diamond,
    Shape::Plus,
    Shape::Star,
    Shape::Ring,
    Shape::OpenTriangle,
    Shape::OpenSquare,
    Shape::OpenDiamond,
    Shape::OpenPlus,
    Shape::OpenStar,
];

/// How many forms there are; each comes filled and outlined.
const FORMS: usize = 6;

impl Shape {
    /// The filled form, and whether it's drawn as an outline.
    fn form(self) -> (Shape, bool) {
        let ix = SHAPES.iter().position(|s| *s == self).unwrap_or(0);
        (SHAPES[ix % FORMS], ix >= FORMS)
    }
}

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A shape for each machine id.
#[cfg(test)]
pub fn assign<'a>(ids: impl IntoIterator<Item = &'a str>) -> HashMap<String, Shape> {
    assign_kept(ids, &HashMap::new())
}

/// A shape for each machine id, keeping the ones `kept` already gave: a
/// machine never changes shape because another was added. A new one takes
/// the shape its id points to, or the next free one, so devices mostly
/// agree.
pub fn assign_kept<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    kept: &HashMap<String, Shape>,
) -> HashMap<String, Shape> {
    let mut ids: Vec<&str> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();
    let mut out = HashMap::new();
    let mut taken = [false; SHAPES.len()];
    for id in &ids {
        if let Some(sh) = kept.get(*id)
            && let Some(ix) = SHAPES.iter().position(|s| s == sh)
            && !taken[ix]
        {
            taken[ix] = true;
            out.insert(id.to_string(), *sh);
        }
    }
    let new: Vec<&str> = ids
        .into_iter()
        .filter(|id| !out.contains_key(*id))
        .collect();
    for id in new {
        // Its form by its id, filled if one's free, else outlined; with
        // all twelve taken, shapes repeat.
        let want = (hash(id) % FORMS as u64) as usize;
        let ix = [0, FORMS]
            .into_iter()
            .flat_map(|at| (0..FORMS).map(move |k| at + (want + k) % FORMS))
            .find(|&i| !taken[i])
            .unwrap_or(want);
        taken[ix] = true;
        out.insert(id.to_string(), SHAPES[ix]);
    }
    out
}

/// The outline of `shape` within a circle of radius `r` about the origin,
/// optically balanced so each looks the same size.
fn outline(shape: Shape, r: f32) -> Vec<[f32; 2]> {
    let poly = |n: usize, rr: f32, rot: f32, dy: f32| -> Vec<[f32; 2]> {
        (0..n)
            .map(|i| {
                let a = rot + i as f32 * 2. * PI / n as f32;
                [a.cos() * rr, a.sin() * rr + dy]
            })
            .collect()
    };
    match shape.form().0 {
        Shape::Circle => poly(40, r, 0., 0.),
        Shape::Triangle => poly(3, r * 1.2, -PI / 2., r * 0.22),
        Shape::Square => {
            let s = r * 0.84;
            vec![[-s, -s], [s, -s], [s, s], [-s, s]]
        }
        Shape::Diamond => poly(4, r * 1.14, -PI / 2., 0.),
        Shape::Plus => {
            let (a, b) = (r * 0.36, r);
            vec![
                [-a, -b],
                [a, -b],
                [a, -a],
                [b, -a],
                [b, a],
                [a, a],
                [a, b],
                [-a, b],
                [-a, a],
                [-b, a],
                [-b, -a],
                [-a, -a],
            ]
        }
        _ => (0..10)
            .map(|i| {
                let a = -PI / 2. + i as f32 * PI / 5.;
                let rr = if i % 2 == 1 { r * 0.5 } else { r * 1.1 };
                [a.cos() * rr, a.sin() * rr + r * 0.06]
            })
            .collect(),
    }
}

/// Paint `shape` centred in `bounds`: filled, or an outline as heavy as
/// the glyph is big.
pub fn paint(shape: Shape, color: Hsla, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = bounds.size.width.as_f32();
    let h = bounds.size.height.as_f32();
    let c = (
        bounds.origin.x.as_f32() + w / 2.,
        bounds.origin.y.as_f32() + h / 2.,
    );
    let (_, open) = shape.form();
    let line = (w.min(h) * 0.14).max(1.2);
    // An outline's stroke straddles it: drawn inside the same circle.
    let r = w.min(h) / 2. - 0.5 - if open { line / 2. } else { 0. };
    let pts = outline(shape, r);
    let mut b = if open {
        PathBuilder::stroke(px(line))
    } else {
        PathBuilder::fill()
    };
    b.move_to(point(px(c.0 + pts[0][0]), px(c.1 + pts[0][1])));
    for p in &pts[1..] {
        b.line_to(point(px(c.0 + p[0]), px(c.1 + p[1])));
    }
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

/// `shape` as an element `size` across.
pub fn glyph(shape: Shape, color: Hsla, size: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window: &mut Window, _: &mut App| paint(shape, color, bounds, window),
    )
    .size(px(size))
    .flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_to_six_machines_never_share_a_shape_and_every_device_agrees() {
        let ids = [
            "8e2e34085ddf",
            "3f9a2c710b5e",
            "c27b19e4a0d2",
            "5e0d44b91a7c",
            "7b20f1c8e94a",
        ];
        let a = assign(ids);
        let mut seen: Vec<Shape> = a.values().copied().collect();
        seen.sort_by_key(|s| SHAPES.iter().position(|x| x == s));
        seen.dedup();
        assert_eq!(seen.len(), 5);
        // Another device listing them in another order draws the same.
        let mut rev = ids;
        rev.reverse();
        assert_eq!(assign(rev), a);
    }

    #[test]
    fn twelve_machines_never_share_a_shape_and_the_first_six_are_filled() {
        let ids: Vec<String> = (0..14).map(|i| format!("{i:x}9e{}", i * 7)).collect();
        let six = assign(ids[..6].iter().map(String::as_str));
        assert!(six.values().all(|s| !s.form().1), "{six:?}");
        let mut kept = HashMap::new();
        for n in 1..=ids.len() {
            kept = assign_kept(ids[..n].iter().map(String::as_str), &kept);
        }
        let first: Vec<Shape> = ids[..12].iter().map(|i| kept[i]).collect();
        let mut seen = first.clone();
        seen.sort_by_key(|s| SHAPES.iter().position(|x| x == s));
        seen.dedup();
        assert_eq!(seen.len(), 12, "{first:?}");
        // Past twelve they repeat, and nothing breaks.
        assert_eq!(kept.len(), 14);
    }

    #[test]
    fn a_machine_keeps_its_shape_when_another_is_added() {
        let first = assign(["8e2e34085ddf", "this"]);
        // Whatever the new one's id would have taken, the old ones stay.
        for new in ["0000", "zzzz", "5e0d44b91a7c", "c27b19e4a0d2"] {
            let after = assign_kept(["8e2e34085ddf", "this", new], &first);
            assert_eq!(after["8e2e34085ddf"], first["8e2e34085ddf"]);
            assert_eq!(after["this"], first["this"]);
            assert_ne!(after[new], after["8e2e34085ddf"]);
            assert_ne!(after[new], after["this"]);
        }
    }
}
