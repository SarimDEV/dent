//! Pages the agent shows inside the chat (`show_visual`): a few live web
//! views lent to the pages on screen, each fitted to its page's height, and
//! a still of each page for when its live view can't show.
//!
//! The web view is a native layer over the window, so it can't be clipped,
//! covered or moved in step with GPUI's drawing. A page is live only while
//! its whole row is in view, nothing floats over the chat and the chat isn't
//! scrolling; otherwise its last still is drawn in its place. Pages beyond
//! the few live views wait their turn with a still, or an empty space of
//! their height until they've been seen.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::web::{PageEvent, Preview};

/// A page in the chat: the artifact it is and which version.
pub type Key = (&'static str, usize);

/// How many pages can be live at once.
const SLOTS: usize = 3;
/// After the chat last moved, how long pages wait before going live again.
const SETTLE: Duration = Duration::from_millis(160);
/// A page's height before it has said, and the most it gets.
pub const GUESS_H: f32 = 320.;
pub const MIN_H: f32 = 80.;
pub const MAX_H: f32 = 4000.;

/// What a row hands over to show its page.
pub struct Page {
    pub path: &'static str,
    pub version: usize,
    pub bytes: Rc<[u8]>,
    pub dir: Option<std::path::PathBuf>,
    pub far: Option<crate::web::FarRoot>,
}

struct Slot {
    page: Rc<RefCell<Preview>>,
    key: Option<Key>,
    /// The frame a row last placed it in, where, and whether all of that
    /// row was in view.
    drawn: u64,
    bounds: Option<Bounds<Pixels>>,
    whole: bool,
    /// Its page has said its height since it was loaded here: it has drawn.
    ready: bool,
}

/// How many pages keep a still; the longest unseen go first.
const STILLS: usize = 24;

#[derive(Default)]
pub struct Inline {
    slots: Vec<Slot>,
    /// Each page's height, in points, as it last laid out.
    pub heights: HashMap<Key, f32>,
    stills: HashMap<Key, Arc<RenderImage>>,
    /// Pages by when their still was last wanted, oldest first.
    seen: std::collections::VecDeque<Key>,
    /// Stills replaced, for the window to free.
    retired: Vec<Arc<RenderImage>>,
    /// The chat's scroll position last frame, and when it last changed.
    top: Option<(usize, f32)>,
    moved_at: Option<Instant>,
    theme: String,
    /// No web view could be made (Wayland, or a test without a window).
    pub broken: bool,
}

fn frame() -> u64 {
    crate::ui::FRAME.load(std::sync::atomic::Ordering::Relaxed)
}

impl Inline {
    /// A row with its page on screen at `bounds`; `whole` when all of it is
    /// in view. Lends it a live view when one is free or showing it already.
    pub fn want(
        &mut self,
        key: Key,
        page: &Page,
        bounds: Bounds<Pixels>,
        whole: bool,
        window: &Window,
    ) {
        let now = frame();
        let at = match self.slots.iter().position(|s| s.key == Some(key)) {
            Some(i) => i,
            // Only a page wholly in view takes a view; one half out of
            // sight keeps its still.
            None if !whole => return,
            None => {
                // A view no row placed this frame or the last.
                let free = self
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.drawn + 1 < now)
                    .min_by_key(|(_, s)| s.drawn)
                    .map(|(i, _)| i);
                let i = match free {
                    Some(i) => i,
                    None if self.slots.len() < SLOTS && !self.broken => {
                        let Some(p) = Preview::new_inline(window) else {
                            self.broken = true;
                            return;
                        };
                        p.set_theme(&self.theme);
                        self.slots.push(Slot {
                            page: Rc::new(RefCell::new(p)),
                            key: None,
                            drawn: 0,
                            bounds: None,
                            whole: false,
                            ready: false,
                        });
                        self.slots.len() - 1
                    }
                    None => return,
                };
                let slot = &mut self.slots[i];
                {
                    let mut p = slot.page.borrow_mut();
                    p.set_visible(false);
                    // Always afresh: another session's page may share its
                    // name, and the last picture was of something else.
                    p.forget();
                    let _ = p.drain();
                    match &page.far {
                        Some(far) => p.show_far(page.path, page.version, &page.bytes, far.clone()),
                        None => p.show(page.path, page.version, &page.bytes, page.dir.as_deref()),
                    }
                }
                slot.key = Some(key);
                slot.ready = false;
                i
            }
        };
        let slot = &mut self.slots[at];
        // Live and now moved, or no longer all in view: out of the way at
        // once, before this frame shows; it comes back once settled.
        {
            let mut p = slot.page.borrow_mut();
            if p.is_visible() && (!whole || slot.bounds != Some(bounds)) {
                p.set_visible(false);
            }
            p.place(bounds);
        }
        slot.drawn = now;
        slot.bounds = Some(bounds);
        slot.whole = whole;
        // Wanted now: the last to go when stills are many.
        if let Some(i) = self.seen.iter().position(|k| *k == key) {
            self.seen.remove(i);
        }
        self.seen.push_back(key);
    }

    /// Once a frame, before drawing: which views show, and the stills
    /// they've taken. `clear` when nothing floats over the chat.
    pub fn show(&mut self, clear: bool, scroll: (usize, f32)) {
        if self.top != Some(scroll) {
            if self.top.is_some() {
                self.moved_at = Some(Instant::now());
            }
            self.top = Some(scroll);
        }
        let settled = self.moved_at.is_none_or(|at| at.elapsed() >= SETTLE);
        let now = frame();
        for slot in &mut self.slots {
            let placed = slot.drawn + 1 >= now;
            let Some(key) = slot.key else {
                slot.page.borrow_mut().set_visible(false);
                continue;
            };
            // Its own page drawn first: shown before, it would flash the last
            // page, a blank or the guessed height.
            let live = clear && settled && placed && slot.whole && slot.ready;
            // Hiding hands the keyboard back to dent too.
            let mut p = slot.page.borrow_mut();
            p.set_visible(live);
            // The view's picture of what it shows now (`snapshot` checks
            // it's of this page), kept as this page's still.
            if slot.ready
                && let Some(snap) = p.snapshot(size(px(0.), px(0.)), true)
                && self
                    .stills
                    .get(&key)
                    .is_none_or(|s| !Arc::ptr_eq(s, &snap.image))
                && let Some(old) = self.stills.insert(key, snap.image)
            {
                self.retired.push(old);
            }
        }
        // A still per page seen lately, not every page ever.
        while self.stills.len() > STILLS {
            let Some(old) = self.seen.pop_front() else {
                break;
            };
            if let Some(img) = self.stills.remove(&old) {
                self.retired.push(img);
            }
        }
        while self.seen.len() > STILLS * 4 {
            self.seen.pop_front();
        }
    }

    /// The chat moved: pages step aside for their stills until it settles.
    pub fn moved(&mut self) {
        self.moved_at = Some(Instant::now());
    }

    /// Still settling after a move: the window draws again once it has.
    pub fn settling(&self) -> bool {
        self.moved_at.is_some_and(|at| at.elapsed() < SETTLE)
    }

    /// How many pages have a still, for checks.
    pub fn stills(&self) -> usize {
        self.stills.len()
    }

    /// How many pages are live on screen now, for checks.
    pub fn live(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.page.borrow().is_visible())
            .count()
    }

    pub fn still(&self, key: Key) -> Option<Arc<RenderImage>> {
        self.stills.get(&key).cloned()
    }

    /// What the pages said since last asked, by page; a page saying its
    /// height has drawn. A link opens only after a press in its page: a
    /// script can't send you to the browser on its own.
    pub fn drain(&mut self) -> Vec<(Key, PageEvent)> {
        let mut out = Vec::new();
        for slot in &mut self.slots {
            let Some(key) = slot.key else { continue };
            let p = slot.page.borrow();
            let pressed = p.was_pressed();
            for e in p.drain() {
                match e {
                    PageEvent::Height(_) => slot.ready = true,
                    PageEvent::External(_) if !pressed => continue,
                    _ => {}
                }
                out.push((key, e));
            }
        }
        out
    }

    /// Snapshots owed, events waiting, or a live view whose row wasn't
    /// drawn last frame (another session, a page over the chat): the
    /// window draws again, and the next frame hides it.
    pub fn tick(&self) -> bool {
        let now = frame();
        self.slots.iter().any(|s| {
            let p = s.page.borrow();
            let owed = p.tick();
            p.pending() || owed || (p.is_visible() && s.drawn < now)
        })
    }

    /// Pictures the window should free.
    pub fn retired(&mut self) -> Vec<Arc<RenderImage>> {
        let mut out = std::mem::take(&mut self.retired);
        for s in &self.slots {
            out.extend(s.page.borrow().retired());
        }
        out
    }

    /// Another machine's pages ask for their own files through here.
    pub fn take_far(&self) -> Vec<crate::web::FarAsk> {
        self.slots
            .iter()
            .flat_map(|s| s.page.borrow().take_far())
            .collect()
    }

    /// dent's colours changed: every page takes them, and every picture
    /// of the old ones goes, the views' own included.
    pub fn set_theme(&mut self, json: String) {
        if self.theme == json {
            return;
        }
        for s in &self.slots {
            let mut p = s.page.borrow_mut();
            p.set_theme(&json);
            p.drop_snapshot();
        }
        self.retired.extend(self.stills.drain().map(|(_, v)| v));
        self.theme = json;
    }

    /// A page keeps the keyboard only after you pressed in it; otherwise
    /// it goes back to dent, as the viewer's page does.
    pub fn keep_keys(&self, pointer: Point<Pixels>) {
        for s in &self.slots {
            s.page.borrow().keep_keys_unless_pressed(pointer);
        }
    }
}

/// A page's height as the chat gives it room: what it said, within limits,
/// or the guess until it has.
pub fn height(said: Option<f32>) -> f32 {
    said.unwrap_or(GUESS_H).clamp(MIN_H, MAX_H)
}

/// dent's colours and type as the CSS variables an inline page uses, named
/// as pages commonly expect them (`--background`, `--foreground`,
/// `--chart-1`…), as JSON for `Preview::set_theme`.
pub fn theme_json(t: &crate::theme::Theme) -> String {
    let hex = |c: Hsla| {
        let c = c.to_rgb();
        format!(
            "#{:02x}{:02x}{:02x}",
            (c.r * 255.).round() as u8,
            (c.g * 255.).round() as u8,
            (c.b * 255.).round() as u8
        )
    };
    let mut vars: Vec<(String, String)> = vec![
        ("background".into(), hex(t.surface)),
        ("foreground".into(), hex(t.ink)),
        ("muted".into(), hex(t.panel)),
        ("muted-foreground".into(), hex(t.quiet)),
        ("faint".into(), hex(t.faint)),
        ("card".into(), hex(t.raised)),
        ("card-foreground".into(), hex(t.ink)),
        ("border".into(), hex(t.hairline)),
        ("primary".into(), hex(t.ink)),
        ("primary-foreground".into(), hex(t.surface)),
        ("accent".into(), hex(t.working.0)),
        ("destructive".into(), hex(t.removed)),
        ("warning".into(), hex(t.waiting.0)),
        ("success".into(), hex(t.done.0)),
        ("info".into(), hex(t.working.0)),
        ("added".into(), hex(t.added)),
        ("removed".into(), hex(t.removed)),
        ("radius".into(), "6px".into()),
        (
            "font-sans".into(),
            "'IBM Plex Sans', -apple-system, system-ui, sans-serif".into(),
        ),
        ("font-serif".into(), "'Literata', Georgia, serif".into()),
        (
            "font-mono".into(),
            "'Commit Mono', ui-monospace, Menlo, monospace".into(),
        ),
    ];
    for (i, c) in t.spectrum.iter().enumerate() {
        vars.push((format!("chart-{}", i + 1), hex(*c)));
    }
    let vars: serde_json::Map<String, serde_json::Value> = vars
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    serde_json::json!({"dark": t.dark, "vars": vars}).to_string()
}

/// A page as a file for the browser: dent's colours and type as fixed CSS
/// variables, since there's no dent there to set them.
pub fn browser_copy(page: &[u8], t: &crate::theme::Theme) -> Vec<u8> {
    let json: serde_json::Value = serde_json::from_str(&theme_json(t)).unwrap_or_default();
    let vars: String = json["vars"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("--{k}:{};", v.as_str().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    let scheme = if t.dark { "dark" } else { "light" };
    // The values are dent's own colours and font names: nothing in them
    // can end the style early.
    let style = format!(
        "<style>:root{{{vars}color-scheme:{scheme}}}:where(html){{font:14px/1.5 var(--font-sans);color:var(--foreground);background:var(--background)}}:where(body){{margin:24px}}</style>"
    );
    crate::web::inject(page, &style)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_page_gets_room_within_limits() {
        use super::height;
        assert_eq!(height(None), super::GUESS_H);
        assert_eq!(height(Some(12.)), super::MIN_H);
        assert_eq!(height(Some(512.)), 512.);
        assert_eq!(height(Some(1e9)), super::MAX_H);
    }
}

/// Pages the agent asked to see before showing them (`preview_visual`):
/// drawn one at a time in a web view just outside the window, fitted to
/// their height, pictured, and sent back with what went wrong in them.
#[derive(Default)]
pub struct Looker {
    page: Option<Rc<RefCell<Preview>>>,
    queue: std::collections::VecDeque<Look>,
    now: Option<Look>,
    /// Keys answered or under way, so each is drawn once.
    seen: std::collections::HashSet<String>,
    n: usize,
}

struct Look {
    session: String,
    key: String,
    html: String,
    width: u32,
    dark: bool,
    began: Option<Instant>,
    height: Option<f32>,
    /// When it was last resized: it settles before its picture.
    sized_at: Option<Instant>,
    /// When it said it had loaded.
    loaded_at: Option<Instant>,
    asked_still: bool,
    console: Vec<String>,
}

/// How long a page has to draw before it's given up on.
const LOOK_GIVE_UP: Duration = Duration::from_secs(15);
/// The tallest picture sent back: taller, scaled to fit the model's
/// limit, its words would be too small to read.
const LOOK_MAX_H: f32 = 2000.;
/// How long a loaded page must sit still before its picture: charts that
/// animate in, fonts and late scripts.
const LOOK_QUIET: Duration = Duration::from_millis(900);

impl Looker {
    /// A page asked to be seen, in this session: queued unless it already
    /// was. `dark` is the theme it's drawn in.
    pub fn ask(&mut self, session: &str, key: &str, html: &str, width: u32, dark: bool) {
        let mark = format!("{session}/{key}");
        if self.seen.contains(&mark) {
            return;
        }
        self.seen.insert(mark);
        let key = key.to_string();
        self.queue.push_back(Look {
            session: session.to_string(),
            key,
            html: html.to_string(),
            width,
            dark,
            began: None,
            height: None,
            sized_at: None,
            loaded_at: None,
            asked_still: false,
            console: Vec::new(),
        });
    }

    /// Forget looks no call waits on any more (`waiting`: session/key), so
    /// the same page asked again later is drawn again.
    pub fn keep_only(&mut self, waiting: &std::collections::HashSet<String>) {
        self.seen.retain(|m| waiting.contains(m));
    }

    pub fn busy(&self) -> bool {
        self.now.is_some() || !self.queue.is_empty()
    }

    /// One step, once a frame: returns a finished look to send.
    pub fn step(&mut self, window: &Window) -> Option<(String, String, dent_protocol::Look)> {
        if self.now.is_none() {
            let mut next = self.queue.pop_front()?;
            if self.page.is_none() {
                match Preview::new_inline(window) {
                    Some(p) => self.page = Some(Rc::new(RefCell::new(p))),
                    None => {
                        return Some(failed(next, "this window can't make a web view"));
                    }
                }
            }
            let page = self.page.clone()?;
            let mut p = page.borrow_mut();
            // Nothing the last look's page still says counts for this one.
            let _ = p.drain();
            p.forget();
            let t = if next.dark {
                crate::theme::Theme::carbon()
            } else {
                crate::theme::Theme::paper()
            };
            p.set_theme(&theme_json(&t));
            self.n += 1;
            p.show(
                ".dent-visuals/look.html",
                self.n,
                next.html.as_bytes(),
                None,
            );
            p.place(off_screen(window, next.width as f32, 800.));
            p.set_visible(true);
            next.began = Some(Instant::now());
            self.now = Some(next);
            return None;
        }
        let page = self.page.clone()?;
        let look = self.now.as_mut()?;
        let events = page.borrow().drain();
        for e in events {
            match e {
                PageEvent::Height(h) => {
                    if look.height.is_none_or(|o| (o - h).abs() >= 1.) {
                        look.height = Some(h);
                        look.sized_at = Some(Instant::now());
                        look.asked_still = false;
                        page.borrow_mut().place(off_screen(
                            window,
                            look.width as f32,
                            h.clamp(1., LOOK_MAX_H),
                        ));
                    }
                }
                PageEvent::Loaded => {
                    look.loaded_at = Some(Instant::now());
                    look.asked_still = false;
                }
                PageEvent::Error(m) | PageEvent::Logged(m) if look.console.len() < 20 => {
                    look.console.push(m.chars().take(500).collect());
                }
                _ => {}
            }
        }
        let p = page.borrow();
        // Settled: loaded, sized, and both a while ago.
        let settled = matches!(
            (look.loaded_at, look.sized_at),
            (Some(l), Some(z)) if l.elapsed() >= LOOK_QUIET && z.elapsed() >= LOOK_QUIET
        );
        if settled && !look.asked_still {
            look.asked_still = true;
            let _ = p.take_still();
            p.take_snapshot();
        }
        if look.asked_still
            && let Some((_, bytes, format)) = p.take_still()
        {
            drop(p);
            let look = self.now.take()?;
            blank(&page);
            return Some(match png(&bytes, format) {
                Some((png, w, h)) => (
                    look.session,
                    look.key,
                    dent_protocol::Look {
                        png,
                        width: look.width,
                        content_height: look.height.unwrap_or(0.).round() as u32,
                        captured_height: if w > 0 {
                            (h as f32 * look.width as f32 / w as f32).round() as u32
                        } else {
                            0
                        },
                        console: look.console,
                        failed: None,
                    },
                ),
                None => failed(look, "its picture couldn't be read"),
            });
        }
        if look.began.is_some_and(|b| b.elapsed() >= LOOK_GIVE_UP) {
            drop(p);
            let look = self.now.take()?;
            blank(&page);
            let why = if look.height.is_none() {
                "it never finished loading"
            } else {
                "it never settled long enough to picture"
            };
            return Some(failed(look, why));
        }
        None
    }
}

/// Done with a look: out of sight and its page's scripts stopped.
fn blank(page: &Rc<RefCell<Preview>>) {
    let mut p = page.borrow_mut();
    p.set_visible(false);
    p.forget();
    p.script("document.open();document.close()");
}

fn failed(look: Look, why: &str) -> (String, String, dent_protocol::Look) {
    (
        look.session,
        look.key,
        dent_protocol::Look {
            width: look.width,
            console: look.console,
            failed: Some(why.to_string()),
            ..Default::default()
        },
    )
}

/// Just past the window's right edge: drawn, but never seen.
fn off_screen(window: &Window, w: f32, h: f32) -> Bounds<Pixels> {
    let right = window.viewport_size().width;
    Bounds::new(point(right + px(40.), px(0.)), size(px(w), px(h)))
}

/// A still as PNG, base64, with its size in pixels.
fn png(bytes: &[u8], format: ImageFormat) -> Option<(String, u32, u32)> {
    let format = match format {
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        _ => image::ImageFormat::Png,
    };
    let img = image::load_from_memory_with_format(bytes, format).ok()?;
    let (w, h) = (img.width(), img.height());
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some((dent_protocol::b64::encode(&out.into_inner()), w, h))
}
