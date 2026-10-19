//! Pictures open large: click one in the chat (a screenshot you attached, a
//! picture in a reply, an attachment chip) and it zooms from where it sits to
//! the middle of the window as the backdrop darkens. Esc, Space or a click
//! outside zooms it back.
//!
//! Pictures belong to a set (the attachments waiting to be sent, the
//! pictures of one message): ← and → step through it, so a dozen dropped
//! screenshots can be looked through in a row.
//!
//! Large, a picture zooms in for a closer look: pinch, ⌘-scroll, ⌘+ and ⌘−
//! (or +, − and 0), or a double-click, which goes to its own size and back.
//! Zoomed, it drags (or scrolls) to show its other parts, and never leaves
//! the window's edge for the backdrop.
//!
//! Every picture that opens is drawn through `picture`, which notes where it
//! sits each frame, so the zoom starts from the thumbnail itself and goes
//! back to whichever one is showing when it closes.

use crate::ui::Tipped;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::anim;
use crate::app::Dent;
use crate::ds::{LABEL, Typed, motion, radius, space};
use crate::ui::{Control, Press, icon, tip_above};

/// The lift takes a little longer than a small enter: the picture travels.
const OPEN_MS: f32 = motion::SLOW + 40.;
/// Exits are quicker than enters.
const CLOSE_MS: f32 = motion::BASE;
/// A double-click's or a key's zoom eases; a pinch or a drag follows you.
const ZOOM_MS: f32 = motion::BASE;
/// Zoomed in at most this many times its own size.
const MOST: f32 = 8.;
/// What a key's step zooms by.
const STEP: f32 = 1.5;

/// One picture that can open: its element id (where it sits) and its file.
#[derive(Clone, PartialEq, Debug)]
pub struct Shot {
    pub key: SharedString,
    pub path: PathBuf,
}

/// The pictures that step together.
pub type Set = Rc<Vec<Shot>>;

/// Whether a picture covers the window, so what's under it keeps quiet.
pub fn covering() -> bool {
    OPEN.with(|o| o.get())
}

/// A set of one.
pub fn single(key: impl Into<SharedString>, path: PathBuf) -> Set {
    Rc::new(vec![Shot {
        key: key.into(),
        path,
    }])
}

/// A picture open large.
pub struct Lightbox {
    set: Set,
    /// Which of the set is showing.
    at: usize,
    /// Where the picture zooms from and back to, in window coordinates.
    from: Bounds<Pixels>,
    /// The showing picture's size in pixels, (0, 0) if unreadable.
    size: (u32, u32),
    opened: Instant,
    closing: Option<Instant>,
    /// How far it's zoomed past fitting the window: 1 fits.
    zoom: f32,
    /// How far its middle has moved from the fitted middle, in points.
    pan: (f32, f32),
    /// A key's or a double-click's zoom on its way: the zoom and pan it
    /// left, and when.
    easing: Option<(f32, (f32, f32), Instant)>,
    /// A drag moving it: where the pointer went down, and the pan then.
    drag: Option<(Point<Pixels>, (f32, f32))>,
    /// Where it fitted last frame.
    fit: Fit,
}

/// Where a picture fits in the window, in points: its box, the window's
/// size, and how much it was shrunk to fit (1 at its own size).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fit {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    vw: f32,
    vh: f32,
    k: f32,
}

impl Default for Fit {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
            vw: 1.,
            vh: 1.,
            k: 1.,
        }
    }
}

impl Fit {
    fn middle(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }

    /// The most it zooms: `MOST` times its own size.
    fn most(&self) -> f32 {
        (MOST / self.k.max(0.001)).max(1.)
    }

    /// `pan` kept so the zoomed picture never shows backdrop past an edge
    /// it could cover; along a side it doesn't fill, it stays centred.
    fn clamped(&self, zoom: f32, pan: (f32, f32)) -> (f32, f32) {
        let (mx, my) = self.middle();
        let axis = |p: f32, size: f32, mid: f32, view: f32| {
            if size <= view {
                0.
            } else {
                p.clamp(view - size / 2. - mid, size / 2. - mid)
            }
        };
        (
            axis(pan.0, self.w * zoom, mx, self.vw),
            axis(pan.1, self.h * zoom, my, self.vh),
        )
    }

    /// Zoomed from `zoom` and `pan` to `to`, keeping the point under
    /// `about` (in the window) where it is.
    fn zoomed(&self, zoom: f32, pan: (f32, f32), to: f32, about: (f32, f32)) -> (f32, (f32, f32)) {
        let to = to.clamp(1., self.most());
        let (mx, my) = self.middle();
        let (cx, cy) = (mx + pan.0, my + pan.1);
        let r = to / zoom.max(0.001);
        let (nx, ny) = (about.0 - (about.0 - cx) * r, about.1 - (about.1 - cy) * r);
        (to, self.clamped(to, (nx - mx, ny - my)))
    }

    /// Where a double-click zooms to: its own size, or twice the fit
    /// when that's already about its own size.
    fn closer(&self) -> f32 {
        let own = 1. / self.k.max(0.001);
        if own > 1.25 { own } else { 2. }
    }
}

impl Lightbox {
    fn shot(&self) -> &Shot {
        &self.set[self.at]
    }

    /// The zoom and pan showing now, part way along an easing.
    fn shown(&self) -> (f32, (f32, f32)) {
        match self.easing {
            Some((z, (px, py), at)) if !crate::reduce_motion() => {
                let e = anim::ease_out(anim::progress(at, ZOOM_MS));
                (
                    lerp(z, self.zoom, e),
                    (lerp(px, self.pan.0, e), lerp(py, self.pan.1, e)),
                )
            }
            _ => (self.zoom, self.pan),
        }
    }

    fn easing_now(&self) -> bool {
        self.easing
            .is_some_and(|(.., at)| !crate::reduce_motion() && anim::progress(at, ZOOM_MS) < 1.)
    }

    /// What shows becomes where it is: a pinch or a drag starts from there.
    fn settle(&mut self) {
        (self.zoom, self.pan) = self.shown();
        self.easing = None;
    }

    /// Zoom to `to` about `about` at once, as a pinch does.
    fn zoom_now(&mut self, to: f32, about: (f32, f32)) {
        self.settle();
        (self.zoom, self.pan) = self.fit.zoomed(self.zoom, self.pan, to, about);
    }

    /// Zoom to `to` about `about`, easing there.
    fn zoom_eased(&mut self, to: f32, about: (f32, f32)) {
        self.settle();
        let from = (self.zoom, self.pan);
        (self.zoom, self.pan) = self.fit.zoomed(self.zoom, self.pan, to, about);
        if from != (self.zoom, self.pan) {
            self.easing = Some((from.0, from.1, Instant::now()));
        }
    }

    fn zoomed_in(&self) -> bool {
        self.zoom > 1.001
    }

    /// Back to fitting, at once (stepping to another picture).
    fn unzoom(&mut self) {
        self.zoom = 1.;
        self.pan = (0., 0.);
        self.easing = None;
        self.drag = None;
    }
}

thread_local! {
    /// Where each picture that can open was drawn last frame, by its id:
    /// the same file shown twice zooms from the one clicked.
    static PLACED: RefCell<HashMap<SharedString, Bounds<Pixels>>> = RefCell::default();
    /// A picture is open over the window this frame.
    static OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A picture clicked this frame, opened by the next render.
    static ASKED: RefCell<Option<(Set, usize)>> = const { RefCell::new(None) };
}

/// The `at`th picture of `set`, opening large when clicked. `inner` is
/// what's shown in place (the thumbnail, sized by the caller); the wrapper
/// notes where it sits.
pub fn picture(set: &Set, at: usize, inner: impl IntoElement) -> Stateful<Div> {
    pictures(set, at, at + 1, inner)
}

/// Like [`picture`], for a tile that also stands for the pictures after it
/// up to `upto` (a "+3" tile): stepping onto one of those and closing goes
/// back to this tile.
pub fn pictures(set: &Set, at: usize, upto: usize, inner: impl IntoElement) -> Stateful<Div> {
    let id = set[at].key.clone();
    let marks: Vec<SharedString> = set[at..upto.clamp(at + 1, set.len())]
        .iter()
        .map(|s| s.key.clone())
        .collect();
    let set = set.clone();
    div()
        .id(id)
        .relative()
        .cursor_pointer()
        // A press here opens the picture, never starts a text selection.
        .control()
        .on_press(move |_, window, _| {
            ask(set.clone(), at);
            window.refresh();
        })
        .child(inner)
        .child(
            canvas(
                move |bounds, _, _| {
                    PLACED.with(|p| {
                        let mut p = p.borrow_mut();
                        for mark in &marks {
                            p.insert(mark.clone(), bounds);
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
}

/// Open the `at`th picture of `set` on the next frame, as a click does.
pub fn ask(set: Set, at: usize) {
    if at < set.len() {
        ASKED.with(|a| *a.borrow_mut() = Some((set, at)));
    }
}

/// What the large view calls a picture. A tool's pictures are kept under
/// made-up names (`tool-<call>-<n>.png`) that mean nothing to you; a long
/// name keeps its start and end so the band stays on screen.
fn shown_name(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.starts_with("tool-")
        && path
            .parent()
            .and_then(|p| p.parent())
            .is_some_and(|p| p.ends_with("attachments"))
    {
        return "From a tool".into();
    }
    const MOST: usize = 48;
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= MOST {
        return name;
    }
    let head: String = chars[..MOST / 2].iter().collect();
    let tail: String = chars[chars.len() - (MOST / 2 - 1)..].iter().collect();
    format!("{head}…{tail}")
}

/// A picture's size in pixels, read from the file's header.
fn pixel_size(path: &std::path::Path) -> (u32, u32) {
    image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|r| r.into_dimensions().ok())
        .unwrap_or((0, 0))
}

/// Where a picture was last drawn, or a point in the middle if nowhere.
fn placed(key: &SharedString, window: &Window) -> Bounds<Pixels> {
    PLACED
        .with(|p| p.borrow().get(key).copied())
        .unwrap_or_else(|| {
            let c = window.viewport_size();
            Bounds::centered_at(point(c.width / 2., c.height / 2.), size(px(1.), px(1.)))
        })
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Dent {
    /// Open a picture clicked since the last frame, from where it sat.
    pub fn take_lightbox_request(&mut self, window: &Window) {
        let Some((set, at)) = ASKED.with(|a| a.borrow_mut().take()) else {
            return;
        };
        let from = placed(&set[at].key, window);
        let size = pixel_size(&set[at].path);
        self.lightbox = Some(Lightbox {
            set,
            at,
            from,
            size,
            opened: Instant::now(),
            closing: None,
            zoom: 1.,
            pan: (0., 0.),
            easing: None,
            drag: None,
            fit: Fit::default(),
        });
    }

    /// How far the open picture is zoomed and moved, as showing now.
    pub fn lightbox_zoom(&self) -> Option<(f32, (f32, f32))> {
        self.lightbox.as_ref().map(|l| l.shown())
    }

    /// Zoom the open picture a step in (`1`), out (`-1`) or back to fitting
    /// (`0`), about its middle. False if none is open.
    pub fn zoom_lightbox(&mut self, by: i32, cx: &mut Context<Self>) -> bool {
        let Some(l) = &mut self.lightbox else {
            return false;
        };
        if l.closing.is_none() {
            let about = l.fit.middle();
            let to = match by {
                0 => 1.,
                b if b > 0 => l.zoom * STEP,
                _ => l.zoom / STEP,
            };
            l.zoom_eased(to, about);
            cx.notify();
        }
        true
    }

    /// Show the next (`1`) or previous (`-1`) picture of the set, at once:
    /// stepping from the keyboard is never animated. False at either end.
    pub fn step_lightbox(&mut self, by: isize, cx: &mut Context<Self>) -> bool {
        let Some(l) = &mut self.lightbox else {
            return false;
        };
        let next = l.at as isize + by;
        if l.closing.is_some() || next < 0 || next as usize >= l.set.len() {
            return false;
        }
        l.at = next as usize;
        l.size = pixel_size(&l.shot().path);
        l.unzoom();
        cx.notify();
        true
    }

    /// Send it back to where it came from. True if one was open.
    pub fn close_lightbox(&mut self, cx: &mut Context<Self>) -> bool {
        match &mut self.lightbox {
            Some(l) if l.closing.is_none() => {
                l.settle();
                l.drag = None;
                // Back to where the showing picture sits, which after
                // stepping isn't the one first clicked.
                if let Some(b) = PLACED.with(|p| p.borrow().get(&l.set[l.at].key).copied()) {
                    l.from = b;
                }
                // Leaving mid-lift starts from where it got to.
                let shown = 1. - anim::progress(l.opened, OPEN_MS);
                l.closing = Some(
                    Instant::now() - std::time::Duration::from_secs_f32(shown * CLOSE_MS / 1000.),
                );
                cx.notify();
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    pub fn lightbox_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        OPEN.with(|o| o.set(self.lightbox.is_some()));
        let l = self.lightbox.as_ref()?;
        let t = self.t;
        let still = crate::reduce_motion();
        // How open it is, 0 at the thumbnail, 1 in the middle.
        let open = match l.closing {
            Some(c) => {
                let p = if still {
                    1.
                } else {
                    anim::progress(c, CLOSE_MS)
                };
                if p >= 1. {
                    self.lightbox = None;
                    OPEN.with(|o| o.set(false));
                    cx.notify();
                    return None;
                }
                1. - anim::ease_out(p)
            }
            None if still => 1.,
            None => anim::ease_out(anim::progress(l.opened, OPEN_MS)),
        };
        let moving = open > 0. && open < 1.;
        // The backdrop leads the picture both ways: dark by three quarters
        // of the lift, and clear before the picture is home again.
        let cover = if l.closing.is_some() {
            ((open - 0.3) / 0.7).clamp(0., 1.)
        } else {
            (open / 0.75).min(1.)
        };
        // The picture fits the window with room for its name below, never
        // larger than it is (in points, at the screen's scale).
        let vw = window.viewport_size();
        let (vw, vh) = (vw.width.as_f32(), vw.height.as_f32());
        let scale = window.scale_factor().max(1.);
        let (pw, ph) = match l.size {
            (0, 0) | (_, 0) | (0, _) => (
                l.from.size.width.as_f32().max(1.),
                l.from.size.height.as_f32().max(1.),
            ),
            (w, h) => (w as f32 / scale, h as f32 / scale),
        };
        // Room to breathe around it: the window shows through the dither.
        let room_w = (vw * 0.84).min(vw - 2. * space::S12).max(80.);
        let room_h = (vh * 0.78).min(vh - 2. * space::S12 - space::S8).max(80.);
        let k = (room_w / pw).min(room_h / ph).min(1.);
        let (tw, th) = (pw * k, ph * k);
        let (tx, ty) = ((vw - tw) / 2., (vh - th - space::S8) / 2.);
        let fit = Fit {
            x: tx,
            y: ty,
            w: tw,
            h: th,
            vw,
            vh,
            k,
        };
        // The window changed (or it just opened): the zoom keeps within
        // what fits now.
        let (zoom, pan, easing, dragging) = {
            let l = self.lightbox.as_mut()?;
            if l.fit != fit {
                l.settle();
                l.fit = fit;
                l.zoom = l.zoom.clamp(1., fit.most());
                l.pan = fit.clamped(l.zoom, l.pan);
            }
            let (z, p) = l.shown();
            (z, p, l.easing_now(), l.drag.is_some())
        };
        if moving || easing {
            window.request_animation_frame();
        }
        let zoomed = zoom > 1.001;
        let l = self.lightbox.as_ref()?;
        // Zoomed about the fitted middle, moved by the pan; the lift goes
        // to (and comes back from) wherever that is.
        let (zw, zh) = (tw * zoom, th * zoom);
        let (zx, zy) = (
            tx + tw / 2. + pan.0 - zw / 2.,
            ty + th / 2. + pan.1 - zh / 2.,
        );
        let f = l.from;
        let x = lerp(f.origin.x.as_f32(), zx, open);
        let y = lerp(f.origin.y.as_f32(), zy, open);
        let w = lerp(f.size.width.as_f32(), zw, open);
        let h = lerp(f.size.height.as_f32(), zh, open);
        let path = l.shot().path.clone();
        let name = shown_name(&path);
        let mut caption = match l.size {
            (0, 0) => name,
            (pw, ph) => format!("{name} · {pw} × {ph}"),
        };
        // Zoomed, how large against its own size.
        if zoomed && l.size.0 > 0 {
            caption = format!("{caption} · {}%", (k * zoom * 100.).round());
        }
        let (n, at) = (l.set.len(), l.at);
        if n > 1 {
            caption = format!("{caption} · {} of {n}", at + 1);
        }
        let copied = self
            .copied_at
            .is_some_and(|at| at.elapsed().as_secs_f32() < 1.5);
        // The backdrop is dark in either theme, as photos are shown.
        // Dark enough that the chat behind stops reading, so a small
        // picture stands alone.
        let shade = if t.dark { 0.86 } else { 0.82 };
        Some(
            div()
                .id("lightbox")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                // Takes every press: nothing under it is clicked.
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_lightbox(cx);
                    }),
                )
                // Zoomed, a drag moves it; a release anywhere lets go.
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                    let Some(l) = &mut this.lightbox else { return };
                    let Some((start, from)) = l.drag else { return };
                    if e.pressed_button != Some(MouseButton::Left) {
                        l.drag = None;
                    } else {
                        let d = e.position - start;
                        l.pan = l
                            .fit
                            .clamped(l.zoom, (from.0 + d.x.as_f32(), from.1 + d.y.as_f32()));
                    }
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if let Some(l) = &mut this.lightbox
                            && l.drag.take().is_some()
                        {
                            cx.notify();
                        }
                    }),
                )
                // A pinch zooms about where it's pinched.
                .on_pinch(cx.listener(|this, e: &PinchEvent, _, cx| {
                    let Some(l) = &mut this.lightbox else { return };
                    if l.closing.is_none() {
                        let to = l.shown().0 * (1. + e.delta).max(0.1);
                        l.zoom_now(to, (e.position.x.as_f32(), e.position.y.as_f32()));
                        cx.notify();
                    }
                }))
                // ⌘- or ⌃-scrolling zooms, as with a mouse; zoomed, scrolling
                // moves it.
                .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, window, cx| {
                    let Some(l) = &mut this.lightbox else { return };
                    if l.closing.is_some() {
                        return;
                    }
                    let d = e.delta.pixel_delta(window.line_height());
                    let (dx, dy) = (d.x.as_f32(), d.y.as_f32());
                    if e.modifiers.platform || e.modifiers.control {
                        let to = l.shown().0 * (dy * 0.01).exp();
                        l.zoom_now(to, (e.position.x.as_f32(), e.position.y.as_f32()));
                    } else if l.zoomed_in() {
                        l.settle();
                        l.pan = l.fit.clamped(l.zoom, (l.pan.0 + dx, l.pan.1 + dy));
                    } else {
                        return;
                    }
                    cx.notify();
                }))
                // The backdrop fades with the zoom, leading it both ways.
                .child(
                    div()
                        .absolute()
                        .size_full()
                        // (`opacity` scales the alpha it's given, so
                        // from opaque black: the shade is applied once.)
                        .bg(gpui_kit::black().opacity(shade * cover)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(h))
                        .rounded(px(lerp(radius::MD, radius::LG, open)))
                        .overflow_hidden()
                        // A faint light edge, so a dark picture still reads
                        // against the dark backdrop; the shadow only once it
                        // has landed (shadows are never animated).
                        .border_1()
                        .border_color(gpui_kit::white().opacity(0.14 * open))
                        .when(!moving, |d| {
                            d.shadow(vec![BoxShadow {
                                color: hsla(0., 0., 0., 0.5),
                                offset: point(px(0.), px(16.)),
                                blur_radius: px(48.),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                        })
                        .when(zoomed, |d| {
                            d.cursor(if dragging {
                                CursorStyle::ClosedHand
                            } else {
                                CursorStyle::OpenHand
                            })
                        })
                        // A press on the picture itself keeps it open: a
                        // double-click zooms in to its own size where it's
                        // clicked, or back out; zoomed, a press starts a drag.
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                let Some(l) = &mut this.lightbox else { return };
                                if l.closing.is_some() {
                                    return;
                                }
                                if e.click_count == 2 {
                                    let to = if l.zoomed_in() { 1. } else { l.fit.closer() };
                                    l.drag = None;
                                    l.zoom_eased(
                                        to,
                                        (e.position.x.as_f32(), e.position.y.as_f32()),
                                    );
                                } else if l.zoomed_in() {
                                    l.settle();
                                    l.drag = Some((e.position, l.pan));
                                }
                                cx.notify();
                            }),
                        )
                        .child(img(path.clone()).size_full().object_fit(ObjectFit::Contain))
                        .child(crate::ui::probe(&self.probes, "lightbox-picture")),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(tx))
                        .top(px(ty + th + space::S3))
                        .w(px(tw))
                        .flex()
                        .justify_center()
                        .opacity(((open - 0.6) / 0.4).clamp(0., 1.))
                        // On its own band, clear of whatever shows through,
                        // with what you can do with the picture beside it.
                        .child(
                            div()
                                .px(px(space::S2))
                                .py(px(space::HALF))
                                .rounded(px(radius::SM))
                                .bg(gpui_kit::black().opacity(0.6))
                                .flex()
                                .items_center()
                                .gap(px(space::S3))
                                .ty(LABEL)
                                .text_color(gpui_kit::white().opacity(0.86))
                                .child(caption)
                                // A hairline sets the actions apart from
                                // what the picture is.
                                .child(
                                    div()
                                        .w(px(1.))
                                        .h(px(14.))
                                        .bg(gpui_kit::white().opacity(0.25)),
                                )
                                .child(
                                    band_action(
                                        "lightbox-zoom",
                                        if zoomed { "Fit" } else { "Zoom" },
                                    )
                                    .on_press(cx.listener(
                                        |this, _, _, cx| {
                                            if let Some(l) = &mut this.lightbox {
                                                let to =
                                                    if l.zoomed_in() { 1. } else { l.fit.closer() };
                                                let about = l.fit.middle();
                                                l.zoom_eased(to, about);
                                                cx.notify();
                                            }
                                        },
                                    )),
                                )
                                .child(
                                    band_action(
                                        "lightbox-copy",
                                        if copied { "Copied" } else { "Copy" },
                                    )
                                    .relative()
                                    .child(crate::ui::probe(&self.probes, "lightbox-copy"))
                                    .on_press(cx.listener({
                                        let path = path.clone();
                                        move |this, _, _, cx| {
                                            if let Some(item) = picture_item(&path) {
                                                cx.write_to_clipboard(item);
                                                this.copied_at = Some(std::time::Instant::now());
                                                cx.notify();
                                            }
                                        }
                                    })),
                                )
                                .child(
                                    band_action("lightbox-preview", "Open in Preview").on_press(
                                        cx.listener({
                                            let path = path.clone();
                                            move |_, _, _, _| {
                                                let _ = std::process::Command::new("open")
                                                    .args(["-a", "Preview"])
                                                    .arg(&path)
                                                    .spawn();
                                            }
                                        }),
                                    ),
                                ),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        // Below the header row, clear of its buttons.
                        .top(px(crate::ds::size::HEADER + space::S3))
                        .right(px(space::S4))
                        .opacity(open)
                        .child(
                            round("lightbox-close", "x", true, "Close", "Esc", t).on_press(
                                cx.listener(|this, _, _, cx| {
                                    this.close_lightbox(cx);
                                }),
                            ),
                        ),
                )
                // Through the set: the previous and next picture, at the
                // window's sides, level with the picture's middle.
                .when(n > 1, |d| {
                    let mid = ty + th / 2. - 18.;
                    let first = at == 0;
                    let last = at + 1 == n;
                    d.child(
                        div()
                            .absolute()
                            .left(px(space::S4))
                            .top(px(mid))
                            .opacity(open)
                            .child(
                                round(
                                    "lightbox-prev",
                                    "chevron-left",
                                    !first,
                                    if first {
                                        "This is the first picture"
                                    } else {
                                        "Previous"
                                    },
                                    "←",
                                    t,
                                )
                                .on_press(cx.listener(
                                    |this, _, _, cx| {
                                        this.step_lightbox(-1, cx);
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(space::S4))
                            .top(px(mid))
                            .opacity(open)
                            .child(
                                round(
                                    "lightbox-next",
                                    "chevron-right",
                                    !last,
                                    if last {
                                        "This is the last picture"
                                    } else {
                                        "Next"
                                    },
                                    "→",
                                    t,
                                )
                                .on_press(cx.listener(
                                    |this, _, _, cx| {
                                        this.step_lightbox(1, cx);
                                    },
                                )),
                            ),
                    )
                })
                .into_any_element(),
        )
    }
}

/// A round button, light on the dark backdrop in either theme. One that
/// can't be used looks off, shows the not-allowed cursor and says why.
fn round(
    id: &'static str,
    name: &str,
    on: bool,
    says: &'static str,
    keys: &'static str,
    t: crate::theme::Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(36.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        // A dark grey, not a white wash: it reads on the backdrop and on a
        // light picture zoomed in under it alike.
        .bg(hsla(0., 0., 0.16, if on { 0.78 } else { 0.5 }))
        .when(on, |d| {
            d.cursor_pointer().hover(|d| d.bg(hsla(0., 0., 0.26, 0.88)))
        })
        .when(!on, |d| d.cursor(CursorStyle::OperationNotAllowed))
        // A press on a button never reaches the backdrop, which would close.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .with_tip(tip_above(says, on.then_some(keys), t))
        .child(icon(
            name,
            16.,
            gpui_kit::white().opacity(if on { 0.9 } else { 0.3 }),
        ))
}

/// A word on the picture's band that does something: white on the dark
/// band in either theme, brighter on hover. A press on it keeps the
/// picture open.
fn band_action(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(space::S2))
        .py(px(space::HALF))
        .rounded(px(radius::SM))
        .ty(crate::ds::UI)
        .cursor_pointer()
        .text_color(gpui_kit::white().opacity(0.9))
        .hover(|d| d.bg(gpui_kit::white().opacity(0.14)))
        .active(|d| d.bg(gpui_kit::white().opacity(0.22)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(label)
}

/// The picture at `path` for the clipboard, as its own kind of image.
fn picture_item(path: &std::path::Path) -> Option<ClipboardItem> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let format = match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "svg" => ImageFormat::Svg,
        "tif" | "tiff" => ImageFormat::Tiff,
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    Some(ClipboardItem::new_image(&Image::from_bytes(format, bytes)))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` attribute would come with it.
    use super::{Fit, pixel_size};

    /// A 1000 × 500 pt window, a picture shrunk to half to fit it.
    fn fit() -> Fit {
        Fit {
            x: 100.,
            y: 50.,
            w: 800.,
            h: 400.,
            vw: 1000.,
            vh: 500.,
            k: 0.5,
        }
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let f = fit();
        // Twice the fit about its middle: nothing moves.
        assert_eq!(f.zoomed(1., (0., 0.), 2., (500., 250.)), (2., (0., 0.)));
        // About a point left of the middle: that point stays put, so the
        // middle moves right.
        let (z, pan) = f.zoomed(1., (0., 0.), 2., (300., 250.));
        assert_eq!(z, 2.);
        assert_eq!(pan, (200., 0.));
        // About its left edge, it would move 400, but its edge stops at
        // the window's.
        assert_eq!(f.zoomed(1., (0., 0.), 2., (100., 250.)).1, (300., 0.));
        // Its own size is twice the fit; at most eight times its own size.
        assert_eq!(f.closer(), 2.);
        assert_eq!(f.zoomed(1., (0., 0.), 100., (500., 250.)).0, 16.);
        // Never smaller than fitting, and fitting is centred again.
        assert_eq!(f.zoomed(3., (120., 40.), 0.2, (0., 0.)), (1., (0., 0.)));
    }

    #[test]
    fn a_zoomed_picture_never_shows_backdrop_past_its_edge() {
        let f = fit();
        // 1600 wide in a 1000 window: its middle stays within 300 of the
        // window's, which is the fitted middle.
        assert_eq!(f.clamped(2., (900., 0.)), (300., 0.));
        assert_eq!(f.clamped(2., (-900., 0.)), (-300., 0.));
        // 800 high in a 500 window: up to 150 either way.
        assert_eq!(f.clamped(2., (0., 400.)), (0., 150.));
        // A small one already at its own size zooms to twice that.
        let small = Fit { k: 1., ..f };
        assert_eq!(small.closer(), 2.);
        // Along a side it doesn't fill, it stays centred.
        assert_eq!(small.clamped(1.1, (50., 0.)), (0., 0.));
    }

    #[test]
    fn reads_a_pictures_size() {
        let dir = std::env::temp_dir().join(format!("dent-lightbox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.png");
        image::RgbImage::new(30, 20).save(&p).unwrap();
        assert_eq!(pixel_size(&p), (30, 20));
        assert_eq!(pixel_size(&dir.join("missing.png")), (0, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
