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
}

pub const SHAPES: [Shape; 6] = [
    Shape::Circle,
    Shape::Triangle,
    Shape::Square,
    Shape::Diamond,
    Shape::Plus,
    Shape::Star,
];

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
    let mut taken = [false; 6];
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
        let want = (hash(id) % 6) as usize;
        let ix = (0..6)
            .map(|k| (want + k) % 6)
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
    match shape {
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
        Shape::Star => (0..10)
            .map(|i| {
                let a = -PI / 2. + i as f32 * PI / 5.;
                let rr = if i % 2 == 1 { r * 0.5 } else { r * 1.1 };
                [a.cos() * rr, a.sin() * rr + r * 0.06]
            })
            .collect(),
    }
}

/// Paint `shape` filled, centred in `bounds`.
pub fn paint(shape: Shape, color: Hsla, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = bounds.size.width.as_f32();
    let h = bounds.size.height.as_f32();
    let r = w.min(h) / 2. - 0.5;
    let c = (
        bounds.origin.x.as_f32() + w / 2.,
        bounds.origin.y.as_f32() + h / 2.,
    );
    let pts = outline(shape, r);
    let mut b = PathBuilder::fill();
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
