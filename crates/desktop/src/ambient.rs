//! Ambient backgrounds on the new-session page (docs/ambient.md). The
//! simulations live in `dent-ambient`; this module picks one, feeds it live
//! signals from the window's state, and paints its scene with GPUI.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dent_ambient::{Align, Font, Paint, Prim, Runner, Signals, Spec};
use dent_client_core::Entry;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::Dent;
use crate::theme::Theme;

/// The background you chose, by id; `None` for none. Kept here (like the
/// notification sound) so the menu bar can tick it.
static CHOICE: Mutex<Option<&'static str>> = Mutex::new(None);

pub fn chosen() -> Option<&'static Spec> {
    let id = *CHOICE.lock().unwrap_or_else(|e| e.into_inner());
    id.and_then(dent_ambient::find)
}

/// Choose a background by id. Unknown ids (one removed since you picked
/// it) mean none.
pub fn choose(id: Option<&str>) {
    let spec = id.and_then(dent_ambient::find).map(|s| s.id);
    *CHOICE.lock().unwrap_or_else(|e| e.into_inner()) = spec;
}

/// View › Ambient Background › None, or a background by its id.
#[derive(Clone, PartialEq, Debug, Default, Action)]
#[action(namespace = dent, no_json)]
pub struct SetAmbient {
    pub id: Option<String>,
}

/// The View menu's Ambient Background submenu, the current choice ticked.
pub fn menu() -> MenuItem {
    let now = chosen().map(|s| s.id);
    let mut items = vec![
        MenuItem::action("None", SetAmbient { id: None }).checked(now.is_none()),
        MenuItem::separator(),
    ];
    items.extend(dent_ambient::ALL.iter().map(|s| {
        MenuItem::action(
            s.name,
            SetAmbient {
                id: Some(s.id.to_string()),
            },
        )
        .checked(now == Some(s.id))
    }));
    MenuItem::submenu(gpui_kit::Menu {
        name: "Ambient Background".into(),
        items,
        disabled: false,
    })
}

/// The running background and what it has seen of the live state.
#[derive(Default)]
pub struct State {
    runner: Option<Runner>,
    last: Option<Instant>,
    feed: Feed,
    cost: Cost,
}

/// What the background has cost the main thread since it was last read:
/// frames painted, and time spent ticking, building and painting them.
#[derive(Default, Clone, Copy, Debug)]
pub struct Cost {
    pub frames: u32,
    pub busy: Duration,
}

impl State {
    /// The cost so far, and start counting afresh.
    pub fn take_cost(&mut self) -> Cost {
        std::mem::take(&mut self.cost)
    }

    /// A key was typed in the new-session box.
    pub fn key(&mut self) {
        if let Some(r) = &mut self.runner {
            r.key();
        }
    }

    /// You started a session.
    pub fn submit(&mut self) {
        if let Some(r) = &mut self.runner {
            r.submit();
        }
    }

    /// The runner for the current choice, made fresh when it changes.
    fn sync(&mut self) -> Option<&mut Runner> {
        let spec = chosen();
        if self.runner.as_ref().map(|r| r.spec().id) != spec.map(|s| s.id) {
            self.runner = spec.map(|s| Runner::new(s, now_ms()));
            self.last = None;
        }
        self.runner.as_mut()
    }
}

/// After the page has been away this long, the background starts afresh.
const AWAY: Duration = Duration::from_secs(60);

/// Whether the new-session page needs a frame for the background.
pub fn wants_frames(d: &Dent) -> bool {
    d.selected.is_none()
        && d.window_visible
        && !crate::reduce_motion()
        && chosen().is_some()
        && !d
            .ambient
            .borrow()
            .runner
            .as_ref()
            .is_some_and(|r| r.failed())
}

/// `DENT_AMBIENT_DEMO=1` feeds the stand-in (a busy afternoon) instead of
/// live state, for demos and recordings.
fn demo() -> bool {
    static DEMO: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEMO.get_or_init(|| std::env::var_os("DENT_AMBIENT_DEMO").is_some())
}

/// The background as a layer behind the new-session page, or nothing.
pub fn layer(d: &Dent) -> Option<AnyElement> {
    chosen()?;
    let signals = if demo() {
        Signals::default()
    } else {
        d.ambient.borrow_mut().feed.collect(d)
    };
    let state = d.ambient.clone();
    let painter = d.ambient.clone();
    let probes = d.probes.clone();
    let veils = d.probes.clone();
    let surface = d.t.surface;
    let pal = palette(&d.t);
    let still = crate::reduce_motion();
    Some(
        canvas(
            move |bounds, window, _| {
                let mut s = state.borrow_mut();
                let now = Instant::now();
                let start = now;
                // Back after a while away: start afresh rather than resume,
                // which also keeps the background's clock small and precise.
                if s.last.is_some_and(|last| now - last > AWAY) {
                    s.runner = None;
                }
                let dt = match s.last {
                    Some(last) if !still => (now - last).as_secs_f32(),
                    _ => 0.,
                };
                s.last = Some(now);
                let Some(r) = s.sync() else {
                    return;
                };
                // The pointer counts only over open background, never over the page's own controls.
                let m = window.mouse_position();
                let over_ui = probes
                    .borrow()
                    .get("ambient-ui")
                    .is_some_and(|b| b.contains(&m));
                let local = [
                    f32::from(m.x - bounds.origin.x),
                    f32::from(m.y - bounds.origin.y),
                ];
                let inside = bounds.contains(&m);
                r.pointer((inside && !over_ui).then_some(local));
                let size = dent_ambient::Size {
                    w: f32::from(bounds.size.width),
                    h: f32::from(bounds.size.height),
                };
                r.tick(dt, size, signals);
                s.cost.busy += start.elapsed();
            },
            move |bounds, (), window, cx| {
                let start = Instant::now();
                let mut s = painter.borrow_mut();
                let Some(r) = s.runner.as_mut() else { return };
                let scene = r.draw(&pal);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    paint(&scene.prims, bounds.origin, window, cx);
                    let veiled: Vec<Bounds<Pixels>> = {
                        let probes = veils.borrow();
                        VEILED
                            .iter()
                            .filter_map(|n| probes.get(*n).copied())
                            .collect()
                    };
                    for text in veiled {
                        veil(window, text, surface);
                    }
                });
                s.cost.busy += start.elapsed();
                s.cost.frames += 1;
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element(),
    )
}

/// The page's own text that floats over the background, by probe name. A
/// veil is painted under each so the words stay readable.
const VEILED: [&str; 2] = ["ambient-title", "ambient-chips"];

/// A soft veil of the page colour under text: a blurred shadow cast from a
/// zero-height line through the middle of the text. A drop shadow leaves its
/// element's own area hollow, and a line has no area, so the veil is solid
/// in the middle and fades out with no edge anywhere.
fn veil(window: &mut Window, text: Bounds<Pixels>, surface: Hsla) {
    let half = text.size.height / 2.;
    let line = Bounds {
        origin: point(text.origin.x, text.origin.y + half),
        size: size(text.size.width, px(0.)),
    };
    // A blur narrower than the shape keeps the middle solid; the fade is
    // about twice the blur wide, so it still has no visible edge.
    let reach = half + px(20.);
    window.paint_drop_shadows(
        line,
        Corners::all(reach),
        &[BoxShadow {
            color: surface.opacity(0.94),
            offset: point(px(0.), px(0.)),
            blur_radius: px(14.),
            spread_radius: reach,
            inset: false,
        }],
    );
}

/// The theme's tokens as the background's palette.
pub fn palette(t: &Theme) -> dent_ambient::Palette {
    let c = |h: Hsla| {
        let r = h.to_rgb();
        dent_ambient::Rgba {
            r: r.r,
            g: r.g,
            b: r.b,
            a: r.a,
        }
    };
    dent_ambient::Palette {
        bg: c(t.surface),
        raised: c(t.raised),
        rule: c(t.hairline),
        rule2: c(crate::theme::mix_pub(t.hairline, t.faint, 0.45)),
        pencil: c(crate::theme::mix_pub(t.ink, t.faint, 0.18)),
        soft: c(t.faint),
        accent: c(t.working.0),
        spec: t.spectrum.map(c),
        dark: t.dark,
    }
}

fn hsla(c: dent_ambient::Rgba) -> Hsla {
    Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
    .into()
}

/// A scene paint as a GPUI background over a shape with these bounds.
/// GPUI's gradient stretches its direction by the box's aspect ratio, so
/// the angle and stop positions are worked back from that.
fn background(p: &Paint, min: [f32; 2], max: [f32; 2]) -> Background {
    match *p {
        Paint::Solid(c) => hsla(c).into(),
        Paint::Linear { from, to, a, b } => {
            let (w, h) = (max[0] - min[0], max[1] - min[1]);
            let (vx, vy) = (to[0] - from[0], to[1] - from[1]);
            if w < 0.5 || h < 0.5 || vx.abs() + vy.abs() < 1e-3 {
                return hsla(a.mix(b, 0.5)).into();
            }
            let (sx, sy) = if w > h { (1., h / w) } else { (w / h, 1.) };
            let angle = (vx / sx).atan2(-vy / sy).to_degrees().rem_euclid(360.);
            let r = (angle - 90.).to_radians();
            let dir = [r.cos() * sx, r.sin() * sy];
            let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
            let c = [min[0] + w / 2., min[1] + h / 2.];
            let t = |p: [f32; 2]| {
                let raw = ((p[0] - c[0]) * dir[0] + (p[1] - c[1]) * dir[1]) / len;
                if dir[0].abs() > dir[1].abs() {
                    (raw + w / 2.) / w
                } else {
                    (raw + h / 2.) / h
                }
            };
            linear_gradient(
                angle,
                linear_color_stop(hsla(a), t(from)),
                linear_color_stop(hsla(b), t(to)),
            )
        }
    }
}

fn bbox(pts: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    pts.iter()
        .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        })
}

/// Paint `prims` from `o`, recursing into clips. Returns where it stopped
/// (just past a `ClipEnd`, or the end).
fn paint(prims: &[Prim], o: Point<Pixels>, window: &mut Window, cx: &mut App) -> usize {
    let at = |x: f32, y: f32| point(o.x + px(x), o.y + px(y));
    let is_quads = |p: &Prim| matches!(p, Prim::Rect { .. } | Prim::Dots { .. });
    let mut i = 0;
    while i < prims.len() {
        // A run of rects and dots (stipple can be tens of thousands) goes in
        // one layer: GPUI then skips ordering each quad against everything
        // else, and quads keep their order among themselves.
        if is_quads(&prims[i]) {
            let end = i + prims[i..].iter().take_while(|p| is_quads(p)).count();
            let area = window.content_mask().bounds;
            window.paint_layer(area, |window| {
                for p in &prims[i..end] {
                    quads(p, o, window);
                }
            });
            i = end;
            continue;
        }
        match &prims[i] {
            Prim::Rect { .. } | Prim::Dots { .. } => quads(&prims[i], o, window),
            Prim::Stroke {
                pts,
                closed,
                width,
                color,
            } => {
                let mut b = PathBuilder::stroke(px(*width));
                b.move_to(at(pts[0][0], pts[0][1]));
                for p in &pts[1..] {
                    b.line_to(at(p[0], p[1]));
                }
                if *closed {
                    b.close();
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, hsla(*color));
                }
            }
            Prim::Segments { pts, width, color } => {
                let mut b = PathBuilder::stroke(px(*width));
                for [a, z] in pts.as_chunks::<2>().0 {
                    b.move_to(at(a[0], a[1]));
                    b.line_to(at(z[0], z[1]));
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, hsla(*color));
                }
            }
            Prim::Fill { pts, paint: p } => {
                let mut b = PathBuilder::fill();
                b.move_to(at(pts[0][0], pts[0][1]));
                for q in &pts[1..] {
                    b.line_to(at(q[0], q[1]));
                }
                b.close();
                let (lo, hi) = bbox(pts);
                if let Ok(path) = b.build() {
                    window.paint_path(path, background(p, lo, hi));
                }
            }
            Prim::Circle {
                c,
                r,
                fill: f,
                stroke,
            } => {
                let b = Bounds {
                    origin: at(c[0] - r, c[1] - r),
                    size: size(px(r * 2.), px(r * 2.)),
                };
                let bg: Hsla = f.map(hsla).unwrap_or(transparent_black());
                let mut q = fill(b, bg).corner_radii(px(*r));
                if let Some((w, col)) = stroke {
                    q = q.border_widths(px(*w)).border_color(hsla(*col));
                }
                window.paint_quad(q);
            }
            Prim::Text {
                at: a,
                text,
                font: f,
                size: s,
                color,
                align,
                backing,
            } => {
                text_run(
                    text,
                    *f,
                    *s,
                    *color,
                    *align,
                    *backing,
                    at(a[0], a[1]),
                    window,
                    cx,
                );
            }
            Prim::ClipStart { x, y, w, h } => {
                let bounds = Bounds {
                    origin: at(*x, *y),
                    size: size(px(*w), px(*h)),
                };
                let rest = &prims[i + 1..];
                let used = window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    paint(rest, o, window, cx)
                });
                i += used;
            }
            Prim::ClipEnd => return i + 1,
        }
        i += 1;
    }
    i
}

/// Paint one rect or dots primitive.
fn quads(p: &Prim, o: Point<Pixels>, window: &mut Window) {
    let at = |x: f32, y: f32| point(o.x + px(x), o.y + px(y));
    match p {
        Prim::Rect {
            x,
            y,
            w,
            h,
            paint: p,
        } => {
            let b = Bounds {
                origin: at(*x, *y),
                size: size(px(*w), px(*h)),
            };
            window.paint_quad(fill(b, background(p, [*x, *y], [x + w, y + h])));
        }
        Prim::Dots {
            size: s,
            color,
            pts,
        } => {
            let c = hsla(*color);
            for p in pts {
                window.paint_quad(fill(
                    Bounds {
                        origin: at(p[0], p[1]),
                        size: size(px(*s), px(*s)),
                    },
                    c,
                ));
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn text_run(
    text: &str,
    f: Font,
    s: f32,
    color: dent_ambient::Rgba,
    align: Align,
    backing: Option<dent_ambient::Rgba>,
    baseline: Point<Pixels>,
    window: &mut Window,
    _cx: &mut App,
) {
    let mut family = font(SharedString::from(match f {
        Font::Mono => crate::theme::mono(),
        Font::Book => crate::theme::body(),
        Font::Display => crate::theme::serif(),
    }));
    if f == Font::Book {
        family.style = FontStyle::Italic;
    }
    let color = hsla(color);
    let s_px = px(s);
    let text: SharedString = text.to_string().into();
    let line = window.text_system().shape_line(
        text.clone(),
        s_px,
        &[TextRun {
            len: text.len(),
            font: family,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let w = line.width;
    let x = match align {
        Align::Left => baseline.x,
        Align::Center => baseline.x - w / 2.,
        Align::Right => baseline.x - w,
    };
    if let Some(b) = backing {
        let r = Bounds {
            origin: point(x - px(3.), baseline.y - px(s * 0.9)),
            size: size(w + px(6.), px(s * 1.3)),
        };
        window.paint_quad(fill(r, hsla(b)));
    }
    for run in &line.runs {
        for g in &run.glyphs {
            let _ = window.paint_glyph(
                point(x + g.position.x, baseline.y),
                run.font_id,
                g.id,
                s_px,
                color,
            );
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Minutes since local midnight.
fn minute_of(tm: &libc::tm) -> f32 {
    tm.tm_hour as f32 * 60. + tm.tm_min as f32 + tm.tm_sec as f32 / 60.
}

fn local_minute(ms: u64) -> Option<f32> {
    crate::data::local_tm((ms / 1000) as i64).map(|tm| minute_of(&tm))
}

/// What the background has seen of the live state. Each frame it passes on
/// only what's new (tool calls, files touched, sessions started); the slower
/// state (sessions, titles, usage, files) is rebuilt only when it changes.
#[derive(Default)]
struct Feed {
    /// Per session: entries seen, and calls seen in the last of them.
    seen: HashMap<String, (usize, usize)>,
    /// When the feed last ran, to tell a page that's been showing from one
    /// that just came back.
    last: Option<Instant>,
    /// The state half of the signals and the fingerprint it was built from.
    state: Option<(u64, Signals)>,
    lines: Lines,
}

impl Feed {
    fn collect(&mut self, d: &Dent) -> Signals {
        let now = now_ms();
        // New entries are news only once the daemon has replayed its history
        // and while the page has been showing; otherwise catch up silently,
        // so history (or an hour spent elsewhere) never arrives as one burst.
        let showing = self
            .last
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1));
        self.last = Some(Instant::now());
        let news = d.core.live && showing;
        let mut s = self.state(d, now);
        if let Some(tm) = crate::data::local_tm((now / 1000) as i64) {
            s.minute = Some(minute_of(&tm));
            s.date = Some((tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32));
        }
        let calls_in = |e: &Entry| {
            if let Entry::Tools { calls } = e {
                calls.len()
            } else {
                0
            }
        };
        for (id, v) in &d.core.list {
            let n = v.entries.len();
            let last = v.entries.last().map_or(0, calls_in);
            let seen = self.seen.insert(id.clone(), (n, last));
            if !news {
                continue;
            }
            let (from, from_calls) = match seen {
                Some((seen, seen_calls)) if seen <= n => (seen, seen_calls),
                Some(_) => continue,
                // A session that appears while the page is showing is new.
                None => {
                    s.started += 1;
                    (0, 0)
                }
            };
            let mut emit = |e: &Entry, skip: usize| {
                if let Entry::Tools { calls } = e {
                    for c in calls.iter().skip(skip) {
                        s.calls.push(dent_ambient::Kind::of_tool(&c.name));
                        if let Some(p) = c.path() {
                            let edit = matches!(
                                c.name.as_str(),
                                "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
                            );
                            let mut parts: Vec<&str> = p.rsplit('/').take(2).collect();
                            parts.reverse();
                            s.touches.push(dent_ambient::Touch {
                                path: parts.join("/"),
                                edit,
                            });
                        }
                    }
                }
            };
            if from > 0 && from <= n {
                emit(&v.entries[from - 1], from_calls);
            }
            for e in &v.entries[from.min(n)..] {
                emit(e, 0);
            }
        }
        s
    }

    /// The slower half of the signals, from cache unless something it's
    /// made from changed (or the minute turned, for usage's countdown).
    fn state(&mut self, d: &Dent, now: u64) -> Signals {
        let project = d.defaults.project.as_deref();
        let files = project.and_then(|p| {
            d.sessions
                .iter()
                .filter(|x| x.cwd == p)
                .max_by_key(|x| x.updated)
                .and_then(|x| d.files.get(&x.id))
        });
        let counted = match (project, files) {
            (Some(p), Some(f)) => self.lines.get(p, f),
            _ => None,
        };
        let limits = d.limits();
        let mut h = std::hash::DefaultHasher::new();
        (
            now / 60_000,
            &d.machine,
            project,
            counted.as_ref().map(Vec::len),
        )
            .hash(&mut h);
        for x in &d.sessions {
            (
                x.updated,
                x.closed,
                x.status == crate::theme::Status::Working,
                &x.title,
            )
                .hash(&mut h);
        }
        for w in &limits {
            (&w.name, w.used.to_bits(), w.resets_at).hash(&mut h);
        }
        let key = h.finish();
        if let Some((k, s)) = &self.state
            && *k == key
        {
            return s.clone();
        }
        let mut open: Vec<&Rc<crate::data::Session>> =
            d.sessions.iter().filter(|x| !x.closed).collect();
        open.sort_by_key(|x| x.updated);
        let working = |x: &&Rc<crate::data::Session>| x.status == crate::theme::Status::Working;
        let now_s = (now / 1000) as i64;
        let s = Signals {
            project: project.map(Dent::project_name),
            machine: (!d.machine.is_empty()).then(|| d.machine.clone()),
            working: Some(open.iter().filter(|x| working(x)).count() as u32),
            sessions: open
                .iter()
                .rev()
                .take(16)
                .rev()
                .map(|x| dent_ambient::Session {
                    name: x.title.clone(),
                    working: working(x),
                })
                .collect(),
            log: open
                .iter()
                .rev()
                .take(30)
                .rev()
                .filter(|x| !x.title.trim().is_empty())
                .filter_map(|x| {
                    Some(dent_ambient::LogLine {
                        minute: local_minute(x.updated)?,
                        text: x.title.clone(),
                    })
                })
                .collect(),
            usage: limits
                .iter()
                .find(|w| w.name == "five_hour")
                .map(|w| dent_ambient::Usage {
                    used: w.used as f32,
                    resets_in: (w.resets_at - now_s).max(0) as f32,
                    window: 5. * 3600.,
                }),
            files: counted.unwrap_or_default(),
            ..Default::default()
        };
        self.state = Some((key, s.clone()));
        s
    }
}

/// The open project's files with their line counts, counted once per
/// project on a background thread (never on a frame).
#[derive(Default)]
struct Lines {
    project: String,
    done: Arc<Mutex<Option<Vec<dent_ambient::File>>>>,
}

/// Files counted per project, and the largest file read.
const MAX_COUNTED: usize = 2000;
const MAX_BYTES: u64 = 2_000_000;

impl Lines {
    fn get(&mut self, project: &str, paths: &Rc<Vec<String>>) -> Option<Vec<dent_ambient::File>> {
        if self.project != project {
            self.project = project.to_string();
            self.done = Arc::default();
            let out = self.done.clone();
            let root = std::path::PathBuf::from(project);
            let list: Vec<String> = paths.iter().take(MAX_COUNTED).cloned().collect();
            std::thread::spawn(move || {
                let files = list
                    .iter()
                    .filter_map(|f| {
                        let path = root.join(f);
                        let meta = std::fs::metadata(&path).ok()?;
                        if !meta.is_file() || meta.len() > MAX_BYTES {
                            return None;
                        }
                        let bytes = std::fs::read(&path).ok()?;
                        // Skip binaries: a NUL early on means it isn't text.
                        if bytes.iter().take(8192).any(|&b| b == 0) {
                            return None;
                        }
                        let lines = bytes.iter().filter(|&&b| b == b'\n').count().max(1);
                        let mut parts = f.split('/');
                        let first = parts.next().unwrap_or(f);
                        let group = if parts.next().is_some() { first } else { "." };
                        Some(dent_ambient::File {
                            group: group.to_string(),
                            name: f.rsplit('/').next().unwrap_or(f).to_string(),
                            weight: lines as f32,
                        })
                    })
                    .collect();
                if let Ok(mut d) = out.lock() {
                    *d = Some(files);
                }
            });
        }
        self.done.lock().ok()?.clone()
    }
}
