//! Mermaid diagrams (0015): merman lays them out to SVG in Field Notes, a
//! recolour pass snaps every colour to the nearest theme token, and resvg
//! (0039) rasterises the result. All of it runs off the UI thread.
//!
//! Labels are measured with the bundled IBM Plex Sans through rustybuzz
//! (0041), the same face resvg then draws them in, so they fit their boxes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::*;
use merman::svg::{
    CssOverridePolicy, DeterministicTextMeasurer, HostTheme, HostThemeAppearance,
    MeasurementProfileId, Presentation, SvgOutputPolicy, SvgPipelinePreset, TextMeasurementPolicy,
    TextMeasurementProfile, TextMeasurementProfileIdentity, ThemeRole,
};
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment, SvgRequest};

use crate::theme::Theme;

const FONT: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf");
const FONT_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf");
const FAMILY: &str = "IBM Plex Sans";
/// Rasterised at twice its size, so it stays sharp on Retina screens.
pub const SCALE: f32 = 2.;

/// A finished diagram: the picture and its size in points.
pub struct Drawn {
    pub image: Arc<RenderImage>,
    pub w: f32,
    pub h: f32,
}

/// The theme's colours as `#rrggbb`, in the roles a diagram needs.
#[derive(Clone)]
pub struct Palette {
    dark: bool,
    canvas: String,
    surface: String,
    raised: String,
    ink: String,
    quiet: String,
    hairline: String,
    line: String,
    note: String,
    series: [String; 5],
    /// Every token, for the recolour pass.
    tokens: Vec<[u8; 3]>,
}

fn rgb8(c: Hsla) -> [u8; 3] {
    let c = c.to_rgb();
    let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    [b(c.r), b(c.g), b(c.b)]
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

impl Palette {
    pub fn of(t: &Theme) -> Self {
        let h = |c: Hsla| hex(rgb8(c));
        let tokens = [
            t.surface,
            t.raised,
            t.panel,
            t.ink,
            t.quiet,
            t.faint,
            t.hairline,
            t.working.0,
            t.working.1,
            t.needs.0,
            t.needs.1,
            t.done.0,
            t.done.1,
            t.waiting.0,
            t.waiting.1,
            t.idle.0,
        ]
        .into_iter()
        .map(rgb8)
        .collect();
        Self {
            dark: t.dark,
            canvas: h(t.surface),
            surface: h(t.raised),
            raised: h(t.panel),
            ink: h(t.ink),
            quiet: h(t.quiet),
            hairline: h(t.hairline),
            line: h(t.faint),
            note: h(t.waiting.1),
            series: [
                h(t.working.0),
                h(t.done.0),
                h(t.waiting.0),
                h(t.idle.0),
                h(t.needs.0),
            ],
            tokens,
        }
    }
}

/// Label widths from the real font, cached per string.
fn measurement() -> TextMeasurementPolicy {
    let cache = Arc::new(Mutex::new(HashMap::<String, f64>::new()));
    let measurer = DeterministicTextMeasurer::default().with_width_callback(move |text, style| {
        let mut cache = cache.lock().unwrap_or_else(|p| p.into_inner());
        let em = *cache.entry(text.to_string()).or_insert_with(|| {
            let Some(face) = rustybuzz::Face::from_slice(FONT, 0) else {
                return text.chars().count() as f64 * 0.55;
            };
            let upem = face.units_per_em() as f64;
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(text);
            let shaped = rustybuzz::shape(&face, &[], buf);
            shaped
                .glyph_positions()
                .iter()
                .map(|g| g.x_advance as f64)
                .sum::<f64>()
                / upem
        });
        em * style.font_size.max(1.0)
    });
    let id = MeasurementProfileId::new("dent.plex-sans").and_then(|id| {
        TextMeasurementProfileIdentity::new(id, "1:IBMPlexSans-Regular".to_string())
    });
    match id {
        Ok(id) => TextMeasurementPolicy::uniform(TextMeasurementProfile::new(id, measurer)),
        Err(_) => TextMeasurementPolicy::default(),
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn renderer(p: &Palette) -> Result<(Renderer, SvgRequest), String> {
    let theme = HostTheme::new()
        .with_appearance(if p.dark {
            HostThemeAppearance::Dark
        } else {
            HostThemeAppearance::Light
        })
        .try_with_font_family(format!("\"{FAMILY}\", sans-serif"))
        .map_err(err)?
        .try_with_role(ThemeRole::Canvas, &p.canvas)
        .map_err(err)?
        .try_with_role(ThemeRole::Surface, &p.surface)
        .map_err(err)?
        .try_with_role(ThemeRole::SurfaceAlt, &p.raised)
        .map_err(err)?
        .try_with_role(ThemeRole::SurfaceMuted, &p.canvas)
        .map_err(err)?
        .try_with_role(ThemeRole::Text, &p.ink)
        .map_err(err)?
        .try_with_role(ThemeRole::SubtleText, &p.quiet)
        .map_err(err)?
        .try_with_role(ThemeRole::Border, &p.line)
        .map_err(err)?
        .try_with_role(ThemeRole::Line, &p.line)
        .map_err(err)?
        .try_with_role(ThemeRole::EdgeLabelBackground, &p.canvas)
        .map_err(err)?
        .try_with_role(ThemeRole::ClusterBackground, &p.raised)
        .map_err(err)?
        .try_with_role(ThemeRole::ClusterBorder, &p.hairline)
        .map_err(err)?
        .try_with_role(ThemeRole::NoteBackground, &p.note)
        .map_err(err)?
        .try_with_role(ThemeRole::NoteBorder, &p.line)
        .map_err(err)?
        .try_with_role(ThemeRole::NoteText, &p.ink)
        .map_err(err)?
        .try_with_role(ThemeRole::ActorBackground, &p.surface)
        .map_err(err)?
        .try_with_role(ThemeRole::ActorBorder, &p.line)
        .map_err(err)?
        .try_with_role(ThemeRole::ActorText, &p.ink)
        .map_err(err)?
        .try_with_series_palette(p.series.clone())
        .map_err(err)?;
    let output = SvgOutputPolicy {
        preset: SvgPipelinePreset::ResvgSafe,
        css_override_policy: CssOverridePolicy::StripExistingImportant,
        root_background_color: Some(p.canvas.clone()),
        ..SvgOutputPolicy::default()
    };
    let resolved = Presentation::new().with_theme(theme).resolve();
    // Diagrams outside the host theme's roles take Mermaid theme variables.
    let mut vars = serde_json::json!({
        "personBkg": p.ink, "personBorder": p.ink,
        "archEdgeColor": p.line, "archEdgeArrowColor": p.line,
    });
    for i in 0..12 {
        vars[format!("cScale{i}")] = p.series[i % 5].clone().into();
        vars[format!("cScaleLabel{i}")] = p.ink.clone().into();
    }
    let site = merman::MermaidConfig::from_value(serde_json::json!({
        "themeVariables": vars,
        "sankey": { "linkColor": "source", "nodeAlignment": "justify" },
    }));
    let engine = merman::Engine::new().with_site_config(site);
    let renderer = Renderer::new().with_engine(resolved.materialize_engine(engine));
    let request = SvgRequest {
        pipeline: Some(output.pipeline()),
        presentation: resolved.render_policy(),
        environment: SvgEnvironment::deterministic().with_text_measurement_policy(measurement()),
        ..Default::default()
    };
    Ok((renderer, request))
}

/// The bundled faces resvg draws labels with, loaded once.
fn fontdb() -> Arc<resvg::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_font_data(FONT.to_vec());
        db.load_font_data(FONT_MEDIUM.to_vec());
        db.set_sans_serif_family(FAMILY);
        Arc::new(db)
    })
    .clone()
}

/// Mermaid source to SVG in the palette's colours.
pub fn svg(src: &str, p: &Palette) -> Result<String, String> {
    let (r, req) = renderer(p)?;
    match r.render(RenderRequest::svg(src, OperationControl::new(), req)) {
        Ok(RenderOutput::Svg(Some(svg))) => Ok(recolour(svg.svg(), &p.tokens)),
        Ok(_) => Err("no diagram found".into()),
        Err(e) => Err(format!("{e}").chars().take(160).collect()),
    }
}

/// Render a diagram to a picture GPUI can draw. Runs on a background thread.
pub fn render(src: &str, p: &Palette) -> Result<Drawn, String> {
    let svg = svg(src, p)?;
    let opt = resvg::usvg::Options {
        fontdb: fontdb(),
        font_family: FAMILY.to_string(),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(&svg, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    let mut pm = resvg::tiny_skia::Pixmap::new(
        (size.width() * SCALE).ceil() as u32,
        (size.height() * SCALE).ceil() as u32,
    )
    .ok_or("empty diagram")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(SCALE, SCALE),
        &mut pm.as_mut(),
    );
    let (w, h) = (pm.width(), pm.height());
    // tiny-skia is premultiplied RGBA; GPUI wants straight BGRA.
    let mut px = pm.take();
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as f32 / 255.;
        if a > 0. {
            for c in &mut p[..3] {
                *c = (*c as f32 / a).round().min(255.) as u8;
            }
        }
        p.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(w, h, px).ok_or("bad picture buffer")?;
    Ok(Drawn {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(buf)])),
        w: w as f32 / SCALE,
        h: h as f32 / SCALE,
    })
}

/// Properties whose value is a colour, in attributes and in CSS.
const COLOUR_PROPS: &[&str] = &[
    "fill",
    "stroke",
    "stop-color",
    "flood-color",
    "lighting-color",
    "background-color",
    "color",
];

/// Snap every colour in the SVG to the nearest theme token, so diagram
/// types that ignore the host theme still sit in Field Notes. Alpha is kept.
pub fn recolour(svg: &str, tokens: &[[u8; 3]]) -> String {
    if tokens.is_empty() {
        return svg.to_string();
    }
    let b = svg.as_bytes();
    let mut out = String::with_capacity(svg.len());
    let mut copied = 0;
    let mut i = 0;
    while i < b.len() {
        let boundary = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'-');
        let prop = boundary
            .then(|| {
                COLOUR_PROPS
                    .iter()
                    .find(|p| b[i..].starts_with(p.as_bytes()))
            })
            .flatten();
        let Some(prop) = prop else {
            i += 1;
            continue;
        };
        // `fill="…"`, `fill='…'` or `fill: …`.
        let mut j = i + prop.len();
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        let sep = b.get(j).copied();
        if sep != Some(b'=') && sep != Some(b':') {
            i += prop.len();
            continue;
        }
        j += 1;
        while j < b.len() && matches!(b[j], b' ' | b'"' | b'\'') {
            j += 1;
        }
        match parse_colour(&svg[j..]) {
            Some((rgb, alpha, len)) => {
                let near = nearest(rgb, tokens);
                out.push_str(&svg[copied..j]);
                match alpha {
                    Some(a) => {
                        out.push_str(&format!("rgba({},{},{},{})", near[0], near[1], near[2], a))
                    }
                    None => out.push_str(&hex(near)),
                }
                copied = j + len;
                i = copied;
            }
            None => i = j,
        }
    }
    out.push_str(&svg[copied..]);
    out
}

fn nearest(c: [u8; 3], tokens: &[[u8; 3]]) -> [u8; 3] {
    // "Redmean": a cheap distance that tracks how different colours look.
    let d = |a: [u8; 3], b: [u8; 3]| {
        let r = (a[0] as f32 + b[0] as f32) / 2.;
        let (dr, dg, db) = (
            a[0] as f32 - b[0] as f32,
            a[1] as f32 - b[1] as f32,
            a[2] as f32 - b[2] as f32,
        );
        (2. + r / 256.) * dr * dr + 4. * dg * dg + (2. + (255. - r) / 256.) * db * db
    };
    tokens
        .iter()
        .copied()
        .min_by(|a, b| d(c, *a).total_cmp(&d(c, *b)))
        .unwrap_or(c)
}

/// A CSS colour at the start of `s`: its RGB, alpha if not opaque, and how
/// many bytes it took. Only the forms SVG renderers emit.
fn parse_colour(s: &str) -> Option<([u8; 3], Option<f32>, usize)> {
    let lower = s.get(..s.len().min(5))?.to_ascii_lowercase();
    if let Some(rest) = s.strip_prefix('#') {
        let n = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
        let v = |h: &str| u8::from_str_radix(h, 16).ok();
        let d = |c: &str| v(&c.repeat(2));
        return match n {
            3 => Some(([d(&rest[0..1])?, d(&rest[1..2])?, d(&rest[2..3])?], None, 4)),
            6 | 8 => {
                let rgb = [v(&rest[0..2])?, v(&rest[2..4])?, v(&rest[4..6])?];
                let a = match n {
                    8 => Some(v(&rest[6..8])? as f32 / 255.),
                    _ => None,
                };
                Some((rgb, a.filter(|a| *a < 1.), n + 1))
            }
            _ => None,
        };
    }
    if lower.starts_with("rgb") || lower.starts_with("hsl") {
        let open = s.find('(')?;
        let close = s.find(')')?;
        let parts: Vec<&str> = s[open + 1..close]
            .split([',', ' ', '/'])
            .filter(|p| !p.is_empty())
            .collect();
        // A number, and whether it was a percentage.
        let num = |p: &str| -> Option<(f32, bool)> {
            match p.strip_suffix('%') {
                Some(n) => Some((n.parse().ok()?, true)),
                None => Some((p.trim_end_matches("deg").parse().ok()?, false)),
            }
        };
        let nums: Vec<(f32, bool)> = parts.iter().map(|p| num(p)).collect::<Option<_>>()?;
        if nums.len() < 3 {
            return None;
        }
        let rgb = if lower.starts_with("rgb") {
            let ch = |(v, pct): (f32, bool)| {
                (if pct { v * 2.55 } else { v }).round().clamp(0., 255.) as u8
            };
            [ch(nums[0]), ch(nums[1]), ch(nums[2])]
        } else {
            rgb8(hsla(
                nums[0].0 / 360.,
                nums[1].0 / 100.,
                nums[2].0 / 100.,
                1.,
            ))
        };
        let a = nums
            .get(3)
            .map(|&(a, pct)| if pct { a / 100. } else { a })
            .filter(|a| *a < 1.);
        return Some((rgb, a, close + 1));
    }
    for (name, rgb) in [("white", [255, 255, 255]), ("black", [0, 0, 0])] {
        if s.len() >= name.len()
            && s[..name.len()].eq_ignore_ascii_case(name)
            && !s[name.len()..].starts_with(|c: char| c.is_ascii_alphanumeric())
        {
            return Some((rgb, None, name.len()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Palette, recolour, render, svg};
    use crate::theme::Theme;

    const TOKENS: &[[u8; 3]] = &[[0xF1, 0xEE, 0xE7], [0x17, 0x16, 0x14], [0x2B, 0xB8, 0xD6]];

    #[test]
    fn colours_snap_to_the_nearest_token() {
        let svg = r##"<rect fill="#FFFFFF" stroke="#000" style="fill:#2AB0D0;stroke: rgb(250,250,250)"/><use href="#abc"/><text fill="white" color="rgba(0,0,0,0.5)">fill</text>"##;
        let out = recolour(svg, TOKENS);
        assert_eq!(
            out,
            r##"<rect fill="#F1EEE7" stroke="#171614" style="fill:#2BB8D6;stroke: #F1EEE7"/><use href="#abc"/><text fill="#F1EEE7" color="rgba(23,22,20,0.5)">fill</text>"##
        );
    }

    #[test]
    fn non_colours_are_left_alone() {
        let svg = r#"<g fill="none" stroke="url(#grad)" stroke-width="2" class="fill-x"/>"#;
        assert_eq!(recolour(svg, TOKENS), svg);
    }

    #[test]
    fn a_flowchart_renders_in_the_theme() {
        let p = Palette::of(&Theme::paper());
        let svg = svg(
            "flowchart LR\n  A[Start] --> B{Ok?}\n  B -- yes --> C[Done]",
            &p,
        )
        .expect("renders");
        assert!(svg.contains("Start") && svg.contains("<svg"));
        let d = render("flowchart LR\n  A[Chunk arrives] --> B[Re-parse tail]\n  B --> C{Block closed?}\n  C -- yes --> D[Commit]\n  C -- no --> E[Draw]", &p).expect("rasterises");
        assert!(d.w > 10. && d.h > 10.);
    }
}
