//! Field Notes motion: the dither dissolve and easing (docs/design.md,
//! Texture and motion).

use std::time::Instant;

use gpui_kit::*;

/// Ordered-dither thresholds: cells reveal in a stable, even pattern instead of
/// a fade, which is cheap and reads as "printed".
const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Panels and messages resolve in over this long.
pub const DISSOLVE_MS: f32 = 220.;

/// Linear progress in 0..=1 since `start`.
pub fn progress(start: Instant, ms: f32) -> f32 {
    (start.elapsed().as_secs_f32() * 1000. / ms).clamp(0., 1.)
}

pub fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// An overlay that covers every cell not yet revealed. At `progress` 0 the
/// element is hidden behind `cover`; at 1 nothing is painted.
pub fn dissolve(progress: f32, cover: Hsla, cell: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if progress >= 1. {
                return;
            }
            let cols = (bounds.size.width.as_f32() / cell).ceil() as usize;
            let rows = (bounds.size.height.as_f32() / cell).ceil() as usize;
            for y in 0..rows {
                for x in 0..cols {
                    let threshold = (BAYER[y % 8][x % 8] as f32 + 0.5) / 64.;
                    if threshold > progress {
                        let origin = point(
                            bounds.origin.x + px(x as f32 * cell),
                            bounds.origin.y + px(y as f32 * cell),
                        );
                        window.paint_quad(fill(
                            Bounds {
                                origin,
                                size: size(px(cell), px(cell)),
                            },
                            cover,
                        ));
                    }
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
