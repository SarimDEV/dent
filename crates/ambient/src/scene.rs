//! The display list a background draws into, and the palette it draws with.
//!
//! The vocabulary is deliberately small (rectangles, dots, strokes, filled
//! polygons, circles, text) so any client can paint it. Gradients have two
//! stops, as GPUI's do; `fill_ramp` splits a shape into slices when it needs
//! more.

pub type Pt = [f32; 2];

/// A colour in sRGB, components 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const fn hex(v: u32) -> Self {
        Rgba {
            r: ((v >> 16) & 255) as f32 / 255.,
            g: ((v >> 8) & 255) as f32 / 255.,
            b: (v & 255) as f32 / 255.,
            a: 1.,
        }
    }
    /// This colour with its alpha multiplied by `a`.
    pub fn alpha(self, a: f32) -> Self {
        Rgba {
            a: self.a * a.clamp(0., 1.),
            ..self
        }
    }
    pub fn mix(self, o: Rgba, k: f32) -> Self {
        let k = k.clamp(0., 1.);
        Rgba {
            r: self.r + (o.r - self.r) * k,
            g: self.g + (o.g - self.g) * k,
            b: self.b + (o.b - self.b) * k,
            a: self.a + (o.a - self.a) * k,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Solid(Rgba),
    /// Colour `a` at point `from` blending to `b` at `to`; flat beyond them.
    Linear {
        from: Pt,
        to: Pt,
        a: Rgba,
        b: Rgba,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// The code face: labels, numbers, metadata.
    Mono,
    /// The reading face, italic: names and log lines.
    Book,
    /// The display face: big single letters (H, L, N).
    Display,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        paint: Paint,
    },
    /// Many small squares of one colour (stipple, stars' cores, lakes).
    Dots {
        size: f32,
        color: Rgba,
        pts: Vec<Pt>,
    },
    Stroke {
        pts: Vec<Pt>,
        closed: bool,
        width: f32,
        color: Rgba,
    },
    /// Separate line segments sharing a style: `pts` holds pairs.
    Segments {
        pts: Vec<Pt>,
        width: f32,
        color: Rgba,
    },
    Fill {
        pts: Vec<Pt>,
        paint: Paint,
    },
    Circle {
        c: Pt,
        r: f32,
        fill: Option<Rgba>,
        stroke: Option<(f32, Rgba)>,
    },
    /// `at` is the baseline point; `backing` paints a box behind the text.
    Text {
        at: Pt,
        text: String,
        font: Font,
        size: f32,
        color: Rgba,
        align: Align,
        backing: Option<Rgba>,
    },
    /// Everything until the matching `ClipEnd` is clipped to this rectangle.
    ClipStart {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    ClipEnd,
}

/// Roughly how wide `text` sets in `font` at `size`. Close enough to keep
/// labels apart and boxes snug; the mono face is exact.
pub fn text_width(text: &str, font: Font, size: f32) -> f32 {
    let em = match font {
        Font::Mono => 0.6,
        Font::Book => 0.47,
        Font::Display => 0.5,
    };
    text.chars().count() as f32 * em * size
}

/// `text` shortened with an ellipsis to fit `max` points.
pub fn fit(text: &str, font: Font, size: f32, max: f32) -> String {
    if text_width(text, font, size) <= max {
        return text.to_string();
    }
    let per = text_width("m", font, size).max(0.1);
    let keep = ((max / per) as usize).saturating_sub(1);
    if keep == 0 {
        return String::new();
    }
    let mut s: String = text.chars().take(keep).collect();
    s.push('…');
    s
}

#[derive(Default, Clone, Debug)]
pub struct Scene {
    pub prims: Vec<Prim>,
}

impl Scene {
    pub fn clear(&mut self) {
        self.prims.clear();
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, paint: Paint) {
        if w > 0. && h > 0. {
            self.prims.push(Prim::Rect { x, y, w, h, paint });
        }
    }
    pub fn dots(&mut self, size: f32, color: Rgba, pts: Vec<Pt>) {
        if !pts.is_empty() {
            self.prims.push(Prim::Dots { size, color, pts });
        }
    }
    pub fn line(&mut self, pts: Vec<Pt>, width: f32, color: Rgba) {
        if pts.len() >= 2 {
            self.prims.push(Prim::Stroke {
                pts,
                closed: false,
                width,
                color,
            });
        }
    }
    pub fn outline(&mut self, pts: Vec<Pt>, width: f32, color: Rgba) {
        if pts.len() >= 3 {
            self.prims.push(Prim::Stroke {
                pts,
                closed: true,
                width,
                color,
            });
        }
    }
    pub fn segments(&mut self, pts: Vec<Pt>, width: f32, color: Rgba) {
        if pts.len() >= 2 {
            self.prims.push(Prim::Segments { pts, width, color });
        }
    }
    /// A dashed line from `a` to `b`.
    pub fn dashed(&mut self, a: Pt, b: Pt, dash: f32, gap: f32, width: f32, color: Rgba) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.5 {
            return;
        }
        let (ux, uy) = (dx / len, dy / len);
        let mut pts = Vec::new();
        let mut s = 0.;
        while s < len {
            let e = (s + dash).min(len);
            pts.push([a[0] + ux * s, a[1] + uy * s]);
            pts.push([a[0] + ux * e, a[1] + uy * e]);
            s += dash + gap;
        }
        self.segments(pts, width, color);
    }
    pub fn fill(&mut self, pts: Vec<Pt>, paint: Paint) {
        if pts.len() >= 3 {
            self.prims.push(Prim::Fill { pts, paint });
        }
    }
    /// Fill a polygon with a gradient of several stops along `from`→`to`,
    /// as slices that each carry a two-stop gradient.
    pub fn fill_ramp(&mut self, pts: &[Pt], from: Pt, to: Pt, stops: &[(f32, Rgba)]) {
        let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
        let len2 = dx * dx + dy * dy;
        if len2 < 1e-6 || stops.len() < 2 || pts.len() < 3 {
            if let Some(&(_, c)) = stops.first() {
                self.fill(pts.to_vec(), Paint::Solid(c));
            }
            return;
        }
        let at = |k: f32| [from[0] + dx * k, from[1] + dy * k];
        for (i, w) in stops.windows(2).enumerate() {
            let (k0, c0) = w[0];
            let (k1, c1) = w[1];
            // The first and last slices run out to infinity so nothing is lost.
            let lo = if i == 0 { f32::NEG_INFINITY } else { k0 };
            let hi = if i == stops.len() - 2 {
                f32::INFINITY
            } else {
                k1
            };
            let slice = crate::geom::clip_band(pts, from, [dx, dy], lo, hi);
            if slice.len() >= 3 {
                self.fill(
                    slice,
                    Paint::Linear {
                        from: at(k0),
                        to: at(k1),
                        a: c0,
                        b: c1,
                    },
                );
            }
        }
    }
    pub fn circle(&mut self, c: Pt, r: f32, fill: Option<Rgba>, stroke: Option<(f32, Rgba)>) {
        if r > 0. {
            self.prims.push(Prim::Circle { c, r, fill, stroke });
        }
    }
    pub fn text(
        &mut self,
        at: Pt,
        text: impl Into<String>,
        font: Font,
        size: f32,
        color: Rgba,
        align: Align,
    ) {
        let text = text.into();
        if !text.is_empty() {
            self.prims.push(Prim::Text {
                at,
                text,
                font,
                size,
                color,
                align,
                backing: None,
            });
        }
    }
    /// A small mono label on a box of the page colour, so it reads over lines.
    pub fn label(
        &mut self,
        at: Pt,
        text: impl Into<String>,
        color: Rgba,
        backing: Rgba,
        align: Align,
    ) {
        let text = text.into();
        if !text.is_empty() {
            self.prims.push(Prim::Text {
                at,
                text,
                font: Font::Mono,
                size: 10.,
                color,
                align,
                backing: Some(backing),
            });
        }
    }
    pub fn clip(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.prims.push(Prim::ClipStart {
            x,
            y,
            w: w.max(0.),
            h: h.max(0.),
        });
    }
    pub fn unclip(&mut self) {
        self.prims.push(Prim::ClipEnd);
    }

    /// Drop anything with non-finite geometry and close unbalanced clips, so
    /// a bug in one background can never hand a painter garbage.
    /// Returns how many primitives were dropped.
    pub fn sanitize(&mut self) -> usize {
        let ok = |v: f32| v.is_finite();
        let pt = |p: &Pt| ok(p[0]) && ok(p[1]);
        let col = |c: &Rgba| ok(c.r) && ok(c.g) && ok(c.b) && ok(c.a);
        let paint = |p: &Paint| match p {
            Paint::Solid(c) => col(c),
            Paint::Linear { from, to, a, b } => pt(from) && pt(to) && col(a) && col(b),
        };
        let before = self.prims.len();
        let mut dropped = 0;
        self.prims.retain(|p| match p {
            Prim::Rect {
                x,
                y,
                w,
                h,
                paint: pa,
            } => ok(*x) && ok(*y) && ok(*w) && ok(*h) && paint(pa),
            Prim::Dots { size, color, pts } => ok(*size) && col(color) && pts.iter().all(pt),
            Prim::Stroke {
                pts, width, color, ..
            }
            | Prim::Segments { pts, width, color } => {
                ok(*width) && col(color) && pts.iter().all(pt)
            }
            Prim::Fill { pts, paint: pa } => pts.iter().all(pt) && paint(pa),
            Prim::Circle { c, r, .. } => pt(c) && ok(*r),
            Prim::Text {
                at, size, color, ..
            } => pt(at) && ok(*size) && col(color),
            Prim::ClipStart { x, y, w, h } => ok(*x) && ok(*y) && ok(*w) && ok(*h),
            Prim::ClipEnd => true,
        });
        dropped += before - self.prims.len();
        let mut depth = 0i32;
        self.prims.retain(|p| match p {
            Prim::ClipStart { .. } => {
                depth += 1;
                true
            }
            Prim::ClipEnd if depth == 0 => {
                dropped += 1;
                false
            }
            Prim::ClipEnd => {
                depth -= 1;
                true
            }
            _ => true,
        });
        for _ in 0..depth {
            self.prims.push(Prim::ClipEnd);
        }
        dropped
    }
}

/// The colours a background may use. Clients build it from their theme
/// tokens; `paper` and `carbon` are the Field Notes reference values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The page.
    pub bg: Rgba,
    /// Cards and label boxes, a step lighter than the page.
    pub raised: Rgba,
    /// Fine ruling (Field Notes' light brown on Paper).
    pub rule: Rgba,
    /// Stronger ruling: axes, margins, rings.
    pub rule2: Rgba,
    /// The main ink.
    pub pencil: Rgba,
    /// Secondary ink: labels, minor lines.
    pub soft: Rgba,
    /// The working colour: what's live, and you.
    pub accent: Rgba,
    /// Six hues for kinds of work, violet to orange (think, read, search,
    /// edit, bash, web). Never the needs-you magenta.
    pub spec: [Rgba; 6],
    pub dark: bool,
}

impl Palette {
    pub fn paper() -> Self {
        Palette {
            bg: Rgba::hex(0xF1EEE7),
            raised: Rgba::hex(0xFAF8F3),
            rule: Rgba::hex(0xDDD1BB),
            rule2: Rgba::hex(0xC3B396),
            pencil: Rgba::hex(0x34312C),
            soft: Rgba::hex(0x8C877D),
            accent: Rgba::hex(0x1FA9C9),
            spec: [0x6A4FC9, 0x2A6FD1, 0x1FA9C9, 0x3FAF4F, 0xD99A12, 0xE2672A].map(Rgba::hex),
            dark: false,
        }
    }
    pub fn carbon() -> Self {
        Palette {
            bg: Rgba::hex(0x161513),
            raised: Rgba::hex(0x1F1E1B),
            rule: Rgba::hex(0x2B2721),
            rule2: Rgba::hex(0x463F33),
            pencil: Rgba::hex(0xE2DDD3),
            soft: Rgba::hex(0x7A756C),
            accent: Rgba::hex(0x4FD3EC),
            spec: [0x9B84F0, 0x4F9BF0, 0x4FD3EC, 0x5FD86B, 0xF0B43A, 0xF5884A].map(Rgba::hex),
            dark: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_nan_and_balances_clips() {
        let mut s = Scene::default();
        s.unclip();
        s.clip(0., 0., 10., 10.);
        s.line(vec![[0., 0.], [f32::NAN, 1.]], 1., Rgba::hex(0));
        s.line(vec![[0., 0.], [1., 1.]], 1., Rgba::hex(0));
        assert_eq!(s.sanitize(), 2);
        assert!(matches!(s.prims.last(), Some(Prim::ClipEnd)));
    }

    #[test]
    fn ramp_slices_cover_the_shape() {
        let mut s = Scene::default();
        let sq = [[0., 0.], [100., 0.], [100., 100.], [0., 100.]];
        let c = Rgba::hex(0xff0000);
        s.fill_ramp(
            &sq,
            [0., 50.],
            [100., 50.],
            &[(0., c.alpha(0.)), (0.5, c), (1., c)],
        );
        assert_eq!(s.prims.len(), 2);
    }

    #[test]
    fn fit_shortens() {
        assert_eq!(fit("abc", Font::Mono, 10., 100.), "abc");
        assert!(fit("a long file name.rs", Font::Mono, 10., 40.).ends_with('…'));
    }
}
