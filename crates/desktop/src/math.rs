//! TeX typeset by RaTeX (0014) and drawn natively from its display list:
//! glyphs in the KaTeX fonts, rules as quads, radicals and stretchy
//! delimiters as paths. Crisp at any scale, themed with ink, no rasterising.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::*;
use ratex_types::display_item::{DisplayItem, DisplayList};
use ratex_types::path_command::PathCommand;

/// Typeset `src`, as display maths or inline. `None` when it isn't TeX
/// RaTeX can read; the caller shows the source instead.
pub fn typeset(src: &str, display: bool) -> Option<DisplayList> {
    let nodes = ratex_parser::parse(src).ok()?;
    let opts = ratex_layout::LayoutOptions {
        style: if display {
            ratex_types::math_style::MathStyle::Display
        } else {
            ratex_types::math_style::MathStyle::Text
        },
        ..Default::default()
    };
    Some(ratex_layout::to_display_list(&ratex_layout::layout(
        &nodes, &opts,
    )))
}

/// Ink for TeX's default black; colours the TeX asks for are kept.
fn tint(c: &ratex_types::color::Color, ink: Hsla) -> Hsla {
    if c.r == 0. && c.g == 0. && c.b == 0. {
        return ink.opacity(c.a);
    }
    Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
    .into()
}

/// Draw a display list with its top-left at `origin`, `em` pixels to the em.
pub fn paint(window: &mut Window, list: &DisplayList, origin: Point<Pixels>, em: f32, ink: Hsla) {
    let at = |x: f64, y: f64| point(origin.x + px(x as f32 * em), origin.y + px(y as f32 * em));
    let hair = 1. / window.scale_factor();
    for item in &list.items {
        match item {
            DisplayItem::GlyphPath {
                x,
                y,
                scale,
                font: name,
                char_code,
                color,
            } => {
                let Some(ch) = char::from_u32(*char_code) else {
                    continue;
                };
                let color = tint(color, ink);
                let size = px(em * *scale as f32);
                // GPUI skips fonts without an "m" glyph, which includes the
                // KaTeX Size fonts (big operators, stretchy delimiters), so
                // those glyphs are drawn from their outlines instead.
                if let Some(cmds) = outline(name, ch) {
                    commands(
                        window,
                        &cmds,
                        at(*x, *y),
                        em * *scale as f32,
                        true,
                        color,
                        hair,
                    );
                    continue;
                }
                let (fam, style) = name.split_once('-').unwrap_or((name, "Regular"));
                let mut f = font(SharedString::from(format!("KaTeX_{fam}")));
                if style.contains("Bold") {
                    f.weight = FontWeight::BOLD;
                }
                if style.contains("Italic") {
                    f.style = FontStyle::Italic;
                }
                let s: SharedString = ch.to_string().into();
                let line = window.text_system().shape_line(
                    s.clone(),
                    size,
                    &[TextRun {
                        len: s.len(),
                        font: f,
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                for run in &line.runs {
                    for g in &run.glyphs {
                        let _ = window.paint_glyph(
                            at(*x, *y) + point(g.position.x, px(0.)),
                            run.font_id,
                            g.id,
                            size,
                            color,
                        );
                    }
                }
            }
            DisplayItem::Line {
                x,
                y,
                width,
                thickness,
                color,
                ..
            } => {
                let t = (*thickness as f32 * em).max(hair);
                let o = at(*x, *y);
                window.paint_quad(fill(
                    Bounds {
                        origin: point(o.x, o.y - px(t / 2.)),
                        size: size(px(*width as f32 * em), px(t)),
                    },
                    tint(color, ink),
                ));
            }
            DisplayItem::Rect {
                x,
                y,
                width,
                height,
                color,
            } => {
                window.paint_quad(fill(
                    Bounds {
                        origin: at(*x, *y),
                        size: size(px(*width as f32 * em), px(*height as f32 * em)),
                    },
                    tint(color, ink),
                ));
            }
            DisplayItem::Path {
                x,
                y,
                commands: cmds,
                fill: filled,
                color,
            } => commands(
                window,
                cmds,
                at(*x, *y),
                em,
                *filled,
                tint(color, ink),
                hair,
            ),
        }
    }
}

/// Fill or stroke path commands given in em units from `origin`, one
/// subpath at a time because stretchy arrows mix windings.
fn commands(
    window: &mut Window,
    cmds: &[PathCommand],
    origin: Point<Pixels>,
    em: f32,
    filled: bool,
    color: Hsla,
    hair: f32,
) {
    let p = |x: f64, y: f64| point(origin.x + px(x as f32 * em), origin.y + px(y as f32 * em));
    let mut start = 0;
    for i in 1..=cmds.len() {
        if i < cmds.len() && !matches!(cmds[i], PathCommand::MoveTo { .. }) {
            continue;
        }
        let mut b = if filled {
            PathBuilder::fill()
        } else {
            PathBuilder::stroke(px(hair.max(1.)))
        };
        for c in &cmds[start..i] {
            match *c {
                PathCommand::MoveTo { x, y } => b.move_to(p(x, y)),
                PathCommand::LineTo { x, y } => b.line_to(p(x, y)),
                PathCommand::CubicTo {
                    x1,
                    y1,
                    x2,
                    y2,
                    x,
                    y,
                } => b.cubic_bezier_to(p(x, y), p(x1, y1), p(x2, y2)),
                PathCommand::QuadTo { x1, y1, x, y } => b.curve_to(p(x, y), p(x1, y1)),
                PathCommand::Close => b.close(),
            }
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
        start = i;
    }
}

type Outlines = HashMap<(String, char), Option<Rc<Vec<PathCommand>>>>;

thread_local! {
    static OUTLINES: RefCell<Outlines> = Default::default();
}

/// The glyph's outline in em units (y down, origin on the baseline), only
/// for KaTeX fonts GPUI won't load. Cached per glyph.
fn outline(name: &str, ch: char) -> Option<Rc<Vec<PathCommand>>> {
    OUTLINES.with(|cache| {
        cache
            .borrow_mut()
            .entry((name.to_string(), ch))
            .or_insert_with(|| {
                let bytes = ratex_katex_fonts::ttf_bytes(&format!("KaTeX_{name}.ttf"))?;
                let face = ttf_parser::Face::parse(&bytes, 0).ok()?;
                if face.glyph_index('m').is_some() {
                    return None;
                }
                let gid = face.glyph_index(ch)?;
                let mut b = Outline(Vec::new(), 1. / face.units_per_em() as f64);
                face.outline_glyph(gid, &mut b)?;
                Some(Rc::new(b.0))
            })
            .clone()
    })
}

/// Collects a glyph outline as path commands, flipped to y down.
struct Outline(Vec<PathCommand>, f64);

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let k = self.1;
        self.0.push(PathCommand::MoveTo {
            x: x as f64 * k,
            y: -y as f64 * k,
        });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let k = self.1;
        self.0.push(PathCommand::LineTo {
            x: x as f64 * k,
            y: -y as f64 * k,
        });
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let k = self.1;
        self.0.push(PathCommand::QuadTo {
            x1: x1 as f64 * k,
            y1: -y1 as f64 * k,
            x: x as f64 * k,
            y: -y as f64 * k,
        });
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let k = self.1;
        self.0.push(PathCommand::CubicTo {
            x1: x1 as f64 * k,
            y1: -y1 as f64 * k,
            x2: x2 as f64 * k,
            y2: -y2 as f64 * k,
            x: x as f64 * k,
            y: -y as f64 * k,
        });
    }
    fn close(&mut self) {
        self.0.push(PathCommand::Close);
    }
}

/// The KaTeX fonts, compiled in. The Size fonts are added too, though GPUI
/// passes over them; their glyphs come from `outline`.
pub fn load_fonts(cx: &mut App) {
    const NAMES: &[&str] = &[
        "AMS-Regular",
        "Caligraphic-Bold",
        "Caligraphic-Regular",
        "Fraktur-Bold",
        "Fraktur-Regular",
        "Main-Bold",
        "Main-BoldItalic",
        "Main-Italic",
        "Main-Regular",
        "Math-BoldItalic",
        "Math-Italic",
        "SansSerif-Bold",
        "SansSerif-Italic",
        "SansSerif-Regular",
        "Script-Regular",
        "Size1-Regular",
        "Size2-Regular",
        "Size3-Regular",
        "Size4-Regular",
        "Typewriter-Regular",
    ];
    let fonts = NAMES
        .iter()
        .filter_map(|n| ratex_katex_fonts::ttf_bytes(&format!("KaTeX_{n}.ttf")))
        .collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("maths fonts: {e}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn typesets_what_agents_write() {
        for src in [
            r"\frac{a}{b}",
            r"\sum_{i=1}^{n} i",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"\sqrt{x^2 + y^2}",
        ] {
            let l = super::typeset(src, true).expect(src);
            assert!(l.width > 0. && !l.items.is_empty(), "{src}");
        }
    }

    #[test]
    fn size_fonts_come_from_outlines() {
        // A big sum lives in KaTeX_Size2, which has no "m".
        assert!(super::outline("Size2-Regular", '∑').is_some());
        // KaTeX_Main loads in GPUI, so it is drawn as text.
        assert!(super::outline("Main-Regular", 'x').is_none());
    }
}
