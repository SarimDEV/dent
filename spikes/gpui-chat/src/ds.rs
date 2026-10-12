//! Design-system tokens (docs/design-system.md). Components use these names,
//! never raw numbers.

#![allow(dead_code)] // the full scale is documented here even where unused

use gpui_kit::*;

use crate::theme::{body, label, mono, sans, serif};

pub mod space {
    pub const HALF: f32 = 2.;
    pub const S1: f32 = 4.;
    pub const S1H: f32 = 6.;
    pub const S2: f32 = 8.;
    pub const S3: f32 = 12.;
    pub const S4: f32 = 16.;
    pub const S6: f32 = 24.;
    pub const S8: f32 = 32.;
}

pub mod size {
    pub const HEADER: f32 = 44.;
    pub const ROW: f32 = 28.;
    pub const LINE: f32 = 20.;
    pub const CONTROL: f32 = 28.;
    pub const CONTROL_SM: f32 = 22.;
    pub const STATUS: f32 = 28.;
    pub const SQUARE: f32 = 12.;
    pub const COLUMN: f32 = 720.;
}

pub mod radius {
    pub const SM: f32 = 4.;
    pub const MD: f32 = 6.;
    pub const LG: f32 = 8.;
    pub const XL: f32 = 12.;
}

pub mod motion {
    pub const FAST: f32 = 120.;
    pub const BASE: f32 = 180.;
    pub const SLOW: f32 = 240.;
    /// The theme colour tween.
    pub const THEME: f32 = 200.;
    pub const STAGGER: f32 = 30.;
    pub const STAGGER_CAP: f32 = 150.;
    /// The working square is deliberately stepped, like pixel animation.
    pub const PIXEL_FPS: f32 = 8.;
    /// Arrive rises this far (px); chrome moves half as far.
    pub const RISE: f32 = 6.;
    pub const NUDGE: f32 = 4.;
}

#[derive(Clone, Copy)]
pub struct Type {
    pub family: fn() -> &'static str,
    pub size: f32,
    pub line: f32,
}

pub const DISPLAY: Type = Type {
    family: serif,
    size: 28.,
    line: 32.,
};
pub const DISPLAY_COMPACT: Type = Type {
    family: serif,
    size: 20.,
    line: 24.,
};
pub const READING: Type = Type {
    family: body,
    size: 15.,
    line: 24.,
};
pub const UI: Type = Type {
    family: sans,
    size: 13.,
    line: 20.,
};
pub const CODE: Type = Type {
    family: mono,
    size: 12.,
    line: 20.,
};
pub const LABEL: Type = Type {
    family: label,
    size: 11.,
    line: 16.,
};

pub trait Typed: Styled + Sized {
    fn ty(self, t: Type) -> Self {
        self.font_family((t.family)())
            .text_size(px(t.size))
            .line_height(px(t.line))
    }
}
impl<T: Styled> Typed for T {}
