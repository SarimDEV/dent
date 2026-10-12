//! merman: does it lay out what agents write, headless, and how fast?
//!
//! Diagrams render to SVG with Field Notes colours and our bundled fonts:
//! merman measures label text with the same font resvg then draws with, so
//! labels fit their boxes. resvg is the SVG rasteriser GPUI already uses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use merman::svg::{
    CssOverridePolicy, DeterministicTextMeasurer, HostTheme, HostThemeAppearance,
    MeasurementProfileId, Presentation, SvgOutputPolicy, SvgPipelinePreset, TextMeasurementPolicy,
    TextMeasurementProfile, TextMeasurementProfileIdentity, ThemeRole,
};
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment, SvgRequest};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

use crate::{corpus, pct, stream};

pub const FIXTURES: &[(&str, &str)] = &[
    (
        "flowchart",
        "flowchart LR\n  A[Prompt] --> B{Needs approval?}\n  B -- yes --> C[Ask you]\n  B -- no --> D[Run tool]\n  C --> D\n  D --> E((Done))",
    ),
    (
        "sequence",
        "sequenceDiagram\n  participant C as Client\n  participant D as Daemon\n  participant A as Agent\n  C->>D: subscribe(after: 41)\n  D->>A: prompt\n  A-->>D: tool_use\n  D-->>C: event 42\n  Note over D,A: approval needed\n  C->>D: allow once",
    ),
    (
        "class",
        "classDiagram\n  class AgentDriver {\n    <<trait>>\n    +start(prompt) Session\n    +interrupt()\n  }\n  class ClaudeDriver\n  class CodexDriver\n  AgentDriver <|.. ClaudeDriver\n  AgentDriver <|.. CodexDriver",
    ),
    (
        "state",
        "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Working: prompt\n  Working --> NeedsYou: approval\n  NeedsYou --> Working: allow\n  Working --> Done: result\n  Done --> [*]",
    ),
    (
        "er",
        "erDiagram\n  PROJECT ||--o{ SESSION : has\n  SESSION ||--|{ EVENT : logs\n  MACHINE ||--o{ SESSION : runs",
    ),
    (
        "gantt",
        "gantt\n  title Phase 1\n  dateFormat YYYY-MM-DD\n  section Spikes\n  Claude stream :done, s1, 2026-10-01, 3d\n  Codex :done, s2, after s1, 2d\n  GPUI :done, s3, after s2, 4d\n  Markdown :active, s4, after s3, 3d",
    ),
    (
        "pie",
        "pie title Event types\n  \"text\" : 61\n  \"tool\" : 27\n  \"approval\" : 8\n  \"error\" : 4",
    ),
    (
        "mindmap",
        "mindmap\n  root((dent))\n    Daemon\n      Event log\n      Drivers\n    Clients\n      Desktop\n      iPhone\n      Web\n    Hub",
    ),
    (
        "timeline",
        "timeline\n  title Releases\n  2026-10 : Spikes\n  2026-11 : Local core\n  2026-12 : More agents : Status",
    ),
    (
        "gitGraph",
        "gitGraph\n  commit\n  branch spike\n  commit\n  commit\n  checkout main\n  merge spike\n  commit",
    ),
    (
        "journey",
        "journey\n  title Answer an approval\n  section Phone\n    Notification: 5: You\n    Read diff: 3: You\n    Allow: 5: You",
    ),
    (
        "quadrant",
        "quadrantChart\n  title Effort vs value\n  x-axis Low effort --> High effort\n  y-axis Low value --> High value\n  Streaming: [0.3, 0.9]\n  Mermaid: [0.6, 0.4]\n  Math: [0.4, 0.5]",
    ),
    (
        "xychart",
        "xychart-beta\n  title Frame time\n  x-axis [stream, scroll, jump, diff]\n  y-axis \"ms\" 0 --> 8\n  bar [4.7, 4.5, 5.3, 4.6]",
    ),
    (
        "sankey",
        "sankey-beta\nPrompts,Claude,60\nPrompts,Codex,40\nClaude,Done,50\nClaude,Needs you,10\nCodex,Done,40",
    ),
    (
        "block",
        "block-beta\n  columns 3\n  Client Daemon Agent\n  Client --> Daemon\n  Daemon --> Agent",
    ),
    (
        "architecture",
        "architecture-beta\n  group mac(server)[Mac]\n  service daemon(server)[Daemon] in mac\n  service db(database)[Event log] in mac\n  db:L -- R:daemon",
    ),
    (
        "requirement",
        "requirementDiagram\n  requirement frames {\n    id: 1\n    text: p99 under 8.3 ms\n    risk: medium\n    verifymethod: test\n  }\n  element bench {\n    type: test\n  }\n  bench - verifies -> frames",
    ),
    (
        "kanban",
        "kanban\n  todo[Todo]\n    a[Spike 05]\n  doing[Doing]\n    b[Spike 04]\n  done[Done]\n    c[Spike 03]",
    ),
    (
        "c4",
        "C4Context\n  Person(you, \"You\")\n  System(dent, \"dent\")\n  System_Ext(agent, \"Agent CLI\")\n  Rel(you, dent, \"drives\")\n  Rel(dent, agent, \"spawns\")",
    ),
    (
        "packet",
        "packet-beta\n  0-15: \"Source port\"\n  16-31: \"Destination port\"\n  32-63: \"Sequence number\"",
    ),
    (
        "radar",
        "radar-beta\n  axis s[\"Speed\"], r[\"Reliability\"], d[\"Docs\"]\n  curve a[\"dent\"]{4, 5, 3}",
    ),
    (
        "treemap",
        "treemap-beta\n\"Events\"\n    \"text\": 61\n    \"tool\": 27\n    \"approval\": 8",
    ),
];

pub struct Palette {
    dark: bool,
    canvas: &'static str,
    surface: &'static str,
    raised: &'static str,
    ink: &'static str,
    quiet: &'static str,
    hairline: &'static str,
    line: &'static str,
    note: &'static str,
    series: [&'static str; 5],
}

pub const PAPER: Palette = Palette {
    dark: false,
    canvas: "#F1EEE7",
    surface: "#FAF8F3",
    raised: "#FAF8F3",
    ink: "#171614",
    quiet: "#6E6A62",
    hairline: "#DCD7CC",
    line: "#8C877D",
    note: "#F5D58C",
    series: ["#2BB8D6", "#3FBF4F", "#E8A317", "#B9B4A9", "#E0309F"],
};

pub const CARBON: Palette = Palette {
    dark: true,
    canvas: "#161513",
    surface: "#1F1E1B",
    raised: "#1B1A17",
    ink: "#ECE8DF",
    quiet: "#8F8A80",
    hairline: "#2E2C28",
    line: "#706B63",
    note: "#6B4F14",
    series: ["#4FD3EC", "#5FD86B", "#F0B43A", "#6B675F", "#F04FB8"],
};

const FONT: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf");
const FONT_FAMILY: &str = "IBM Plex Sans";

/// Label widths from the real font, cached per string.
fn measurement() -> TextMeasurementPolicy {
    let face = rustybuzz::Face::from_slice(FONT, 0).expect("bundled font parses");
    let upem = face.units_per_em() as f64;
    let cache = Arc::new(Mutex::new(HashMap::<String, f64>::new()));
    let measurer = DeterministicTextMeasurer::default().with_width_callback(move |text, style| {
        let mut cache = cache.lock().unwrap_or_else(|p| p.into_inner());
        let em = *cache.entry(text.to_string()).or_insert_with(|| {
            let face = rustybuzz::Face::from_slice(FONT, 0).expect("bundled font parses");
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
    let id = TextMeasurementProfileIdentity::new(
        MeasurementProfileId::new("dent.plex-sans").expect("valid id"),
        "1:IBMPlexSans-Regular".to_string(),
    )
    .expect("valid identity");
    TextMeasurementPolicy::uniform(TextMeasurementProfile::new(id, measurer))
}

pub fn renderer(p: &Palette) -> anyhow::Result<(Renderer, SvgRequest)> {
    let theme = HostTheme::new()
        .with_appearance(if p.dark {
            HostThemeAppearance::Dark
        } else {
            HostThemeAppearance::Light
        })
        .try_with_font_family(format!("\"{FONT_FAMILY}\", sans-serif"))?
        .try_with_role(ThemeRole::Canvas, p.canvas)?
        .try_with_role(ThemeRole::Surface, p.surface)?
        .try_with_role(ThemeRole::SurfaceAlt, p.raised)?
        .try_with_role(ThemeRole::SurfaceMuted, p.canvas)?
        .try_with_role(ThemeRole::Text, p.ink)?
        .try_with_role(ThemeRole::SubtleText, p.quiet)?
        .try_with_role(ThemeRole::Border, p.line)?
        .try_with_role(ThemeRole::Line, p.line)?
        .try_with_role(ThemeRole::EdgeLabelBackground, p.canvas)?
        .try_with_role(ThemeRole::ClusterBackground, p.raised)?
        .try_with_role(ThemeRole::ClusterBorder, p.hairline)?
        .try_with_role(ThemeRole::NoteBackground, p.note)?
        .try_with_role(ThemeRole::NoteBorder, p.line)?
        .try_with_role(ThemeRole::NoteText, p.ink)?
        .try_with_role(ThemeRole::ActorBackground, p.surface)?
        .try_with_role(ThemeRole::ActorBorder, p.line)?
        .try_with_role(ThemeRole::ActorText, p.ink)?
        .try_with_series_palette(p.series)?;
    let output = SvgOutputPolicy {
        preset: SvgPipelinePreset::ResvgSafe,
        css_override_policy: CssOverridePolicy::StripExistingImportant,
        root_background_color: Some(p.canvas.to_string()),
        ..SvgOutputPolicy::default()
    };
    let resolved = Presentation::new().with_theme(theme).resolve();
    // Diagrams outside the host theme's roles take Mermaid theme variables.
    let mut vars = serde_json::json!({
        "personBkg": p.ink, "personBorder": p.ink,
        "archEdgeColor": p.line, "archEdgeArrowColor": p.line,
    });
    for i in 0..12 {
        vars[format!("cScale{i}")] = p.series[i % 5].into();
        vars[format!("cScaleLabel{i}")] = p.ink.into();
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

pub fn svg(r: &Renderer, req: &SvgRequest, src: &str) -> Result<String, String> {
    match r.render(RenderRequest::svg(
        src,
        OperationControl::new(),
        req.clone(),
    )) {
        Ok(RenderOutput::Svg(Some(svg))) => Ok(svg.svg().to_string()),
        Ok(_) => Err("no diagram detected".into()),
        Err(e) => Err(format!("{e}").chars().take(120).collect()),
    }
}

pub fn fontdb() -> Arc<resvg::usvg::fontdb::Database> {
    let mut db = resvg::usvg::fontdb::Database::new();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    for f in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if f.path().extension().is_some_and(|e| e == "ttf")
            && let Ok(bytes) = std::fs::read(f.path())
        {
            db.load_font_data(bytes);
        }
    }
    db.set_sans_serif_family(FONT_FAMILY);
    Arc::new(db)
}

/// Rasterise at `scale` the way the app would off the UI thread.
pub fn raster(
    svg: &str,
    db: &Arc<resvg::usvg::fontdb::Database>,
    scale: f32,
) -> Result<resvg::tiny_skia::Pixmap, String> {
    let opt = resvg::usvg::Options {
        fontdb: db.clone(),
        font_family: FONT_FAMILY.to_string(),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    let mut pm = resvg::tiny_skia::Pixmap::new(
        (size.width() * scale).ceil() as u32,
        (size.height() * scale).ceil() as u32,
    )
    .ok_or("empty diagram")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pm.as_mut(),
    );
    Ok(pm)
}

pub fn run(msgs: &[corpus::Message], out: Option<&str>) -> anyhow::Result<()> {
    let db = fontdb();
    // Corpus fences, then fixtures.
    let fences: Vec<String> = msgs
        .iter()
        .flat_map(|m| {
            let mut in_mermaid = false;
            let mut out = Vec::new();
            for ev in Parser::new_ext(&m.text, stream::options()) {
                match ev {
                    Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(l))) => {
                        in_mermaid = l.as_ref() == "mermaid";
                        if in_mermaid {
                            out.push(String::new());
                        }
                    }
                    Event::Text(t) if in_mermaid => {
                        if let Some(s) = out.last_mut() {
                            s.push_str(&t);
                        }
                    }
                    Event::End(_) => in_mermaid = false,
                    _ => {}
                }
            }
            out
        })
        .collect();
    println!("corpus: {} mermaid fences", fences.len());

    for (name, pal) in [("paper", &PAPER), ("carbon", &CARBON)] {
        let t = Instant::now();
        let (r, req) = renderer(pal)?;
        println!(
            "{name}: renderer setup {:.1} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
        let (mut svg_ms, mut raster_ms, mut failed) = (Vec::new(), Vec::new(), Vec::new());
        for (kind, src) in FIXTURES {
            let t = Instant::now();
            let s = match svg(&r, &req, src) {
                Ok(s) => s,
                Err(e) => {
                    failed.push(format!("{kind}: {e}"));
                    continue;
                }
            };
            let ms = t.elapsed().as_secs_f64() * 1e3;
            svg_ms.push(ms);
            let fo = s.contains("<foreignObject");
            let t = Instant::now();
            let pm = raster(&s, &db, 2.);
            let rms = t.elapsed().as_secs_f64() * 1e3;
            raster_ms.push(rms);
            match pm {
                Ok(pm) => {
                    if name == "paper" {
                        println!(
                            "  {kind:13} svg {ms:6.1} ms  raster@2x {rms:5.1} ms  {}x{}{}",
                            pm.width() / 2,
                            pm.height() / 2,
                            if fo { "  foreignObject!" } else { "" }
                        );
                    }
                    if let Some(dir) = out {
                        std::fs::create_dir_all(dir)?;
                        pm.save_png(format!("{dir}/{kind}-{name}.png"))?;
                    }
                }
                Err(e) => failed.push(format!("{kind}: raster {e}")),
            }
        }
        println!(
            "  {}/{} rendered; svg p50 {:.1} ms max {:.1} ms; raster@2x p50 {:.1} ms max {:.1} ms",
            svg_ms.len(),
            FIXTURES.len(),
            pct(&mut svg_ms.clone(), 50.),
            pct(&mut svg_ms, 100.),
            pct(&mut raster_ms.clone(), 50.),
            pct(&mut raster_ms, 100.)
        );
        for f in failed {
            println!("  failed: {f}");
        }
    }
    Ok(())
}
