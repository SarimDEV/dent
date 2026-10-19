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
/// Zoomed in at most this many times its own size.
const MOST: f32 = 8.;
/// What a key's step zooms by.
const STEP: f32 = 1.5;
/// What a mouse wheel's notch zooms by.
const NOTCH: f32 = 1.1;
/// How much ⌘-scrolling a trackpad zooms: `ln(zoom)` a point, so a 200 pt
/// swipe zooms about 2.7× (as maps take a trackpad's scroll).
const SCROLL_ZOOM: f32 = 1. / 200.;

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
    /// How it's zoomed and moved, and where it's heading.
    cam: crate::pan_zoom::PanZoom,
    /// Where it fitted last frame.
    frame: crate::pan_zoom::Frame,
    /// How much it was shrunk to fit (1 at its own size).
    k: f32,
    /// A drag holding it: where the pointer was lately, for how fast it
    /// was moving when let go.
    drag: Option<std::collections::VecDeque<(Instant, Point<Pixels>)>>,
    /// When it last moved on, for the next frame's step.
    ticked: Instant,
    /// It was moving last frame. From rest, a move's first step is one
    /// frame's, not the whole time it sat still.
    going: bool,
}

/// Where a double-click zooms to, shrunk by `k` to fit: its own size, or
/// twice the fit when that's already about its own size.
fn closer(k: f32) -> f32 {
    let own = 1. / k.max(0.001);
    if own > 1.25 { own } else { 2. }
}

/// How fast a drag was moving when let go, in points a millisecond: over
/// its last 100 ms, nothing if it had stopped.
fn drag_speed(trail: &std::collections::VecDeque<(Instant, Point<Pixels>)>) -> (f32, f32) {
    let Some(&(t1, p1)) = trail.back() else {
        return (0., 0.);
    };
    if t1.elapsed().as_secs_f32() > 0.05 {
        return (0., 0.);
    }
    let Some(&(t0, p0)) = trail
        .iter()
        .find(|(t, _)| t1.duration_since(*t).as_secs_f32() <= 0.1)
    else {
        return (0., 0.);
    };
    let ms = t1.duration_since(t0).as_secs_f32() * 1000.;
    if ms < 8. {
        return (0., 0.);
    }
    let d = p1 - p0;
    (d.x.as_f32() / ms, d.y.as_f32() / ms)
}

impl Lightbox {
    fn shot(&self) -> &Shot {
        &self.set[self.at]
    }

    /// Where the pointer is, as a point in the window.
    fn at(p: Point<Pixels>) -> (f32, f32) {
        (p.x.as_f32(), p.y.as_f32())
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
            cam: Default::default(),
            frame: crate::pan_zoom::Frame {
                x: 0.,
                y: 0.,
                w: 1.,
                h: 1.,
                vw: 1.,
                vh: 1.,
                most: MOST,
            },
            k: 1.,
            drag: None,
            ticked: Instant::now(),
            going: false,
        });
    }

    /// How far the open picture is zoomed and moved, as showing now.
    pub fn lightbox_zoom(&self) -> Option<(f32, (f32, f32))> {
        self.lightbox.as_ref().map(|l| (l.cam.zoom, l.cam.pan))
    }

    /// Zoom the open picture a step in (`1`), out (`-1`) or back to fitting
    /// (`0`), about its middle. False if none is open.
    pub fn zoom_lightbox(&mut self, by: i32, cx: &mut Context<Self>) -> bool {
        let Some(l) = &mut self.lightbox else {
            return false;
        };
        if l.closing.is_none() {
            let about = l.frame.middle();
            match by {
                0 => l.cam.ease_to(&l.frame, 1., about),
                b => l.cam.step(&l.frame, STEP.ln() * b.signum() as f32, about),
            }
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
        l.cam.reset();
        l.drag = None;
        cx.notify();
        true
    }

    /// Send it back to where it came from. True if one was open.
    pub fn close_lightbox(&mut self, cx: &mut Context<Self>) -> bool {
        match &mut self.lightbox {
            Some(l) if l.closing.is_none() => {
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
        let frame = crate::pan_zoom::Frame {
            x: tx,
            y: ty,
            w: tw,
            h: th,
            vw,
            vh,
            most: (MOST / k.max(0.001)).max(1.),
        };
        // Move on to this frame. The window changed (or it just opened):
        // the zoom keeps within what fits now.
        let (zoom, pan, going, dragging) = {
            let l = self.lightbox.as_mut()?;
            if l.frame != frame {
                l.frame = frame;
                l.k = k;
                l.cam.zoom = l.cam.zoom.clamp(1., frame.most);
                l.cam.pan = frame.clamped(l.cam.zoom, l.cam.pan);
            }
            let mut dt = l.ticked.elapsed().as_secs_f32();
            if !l.going {
                dt = dt.min(1. / 60.);
            }
            l.ticked = Instant::now();
            let going = l.closing.is_none() && l.cam.tick(&frame, dt, still);
            l.going = going;
            (l.cam.zoom, l.cam.pan, going, l.drag.is_some())
        };
        if moving || going {
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
            let most = MOST / k.max(0.001);
            caption = format!("{caption} · {}%", (k * zoom.min(most) * 100.).round());
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
                // Zoomed, a drag moves it, giving past its edges; let go
                // while moving, it glides on.
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                    let Some(l) = &mut this.lightbox else { return };
                    let Some(trail) = &mut l.drag else { return };
                    if e.pressed_button != Some(MouseButton::Left) {
                        l.drag = None;
                        l.cam.let_go((0., 0.));
                    } else {
                        let last = trail.back().map_or(e.position, |(_, p)| *p);
                        trail.push_back((Instant::now(), e.position));
                        while trail.len() > 12 {
                            trail.pop_front();
                        }
                        let d = e.position - last;
                        l.cam.pan_by(&l.frame, (d.x.as_f32(), d.y.as_f32()));
                    }
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if let Some(l) = &mut this.lightbox
                            && let Some(trail) = l.drag.take()
                        {
                            l.cam.let_go(drag_speed(&trail));
                            cx.notify();
                        }
                    }),
                )
                // A pinch follows the fingers about where they are. Taken
                // before anything under it, and whether or not the pointer
                // counts as over it: after a key, gpui counts nothing as
                // hovered until the pointer moves, and fingers on a
                // trackpad don't move it.
                .capture_pinch(cx.listener(|this, e: &PinchEvent, _, cx| {
                    let Some(l) = &mut this.lightbox else { return };
                    if l.closing.is_some() {
                        return;
                    }
                    let by = (1. + e.delta).max(0.05).ln();
                    if by != 0. {
                        l.cam.zoom_by(&l.frame, by, Lightbox::at(e.position));
                    }
                    if matches!(e.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        l.cam.lifted();
                    }
                    cx.notify();
                }))
                // ⌘- or ⌃-scrolling zooms: a trackpad's follows the fingers,
                // a mouse wheel's notches ease. Zoomed, scrolling moves it.
                .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, window, cx| {
                    let Some(l) = &mut this.lightbox else { return };
                    if l.closing.is_some() {
                        return;
                    }
                    let about = Lightbox::at(e.position);
                    let zooming = e.modifiers.platform || e.modifiers.control;
                    match e.delta {
                        ScrollDelta::Lines(d) if zooming => {
                            let notches = d.y.clamp(-3., 3.);
                            l.cam.step(&l.frame, NOTCH.ln() * notches, about);
                        }
                        ScrollDelta::Pixels(d) if zooming => {
                            l.cam.zoom_by(&l.frame, d.y.as_f32() * SCROLL_ZOOM, about);
                        }
                        _ if l.cam.zoomed_in() => {
                            let d = e.delta.pixel_delta(window.line_height());
                            l.cam.pan_by(&l.frame, (d.x.as_f32(), d.y.as_f32()));
                        }
                        _ => return,
                    }
                    if matches!(e.touch_phase, TouchPhase::Ended) {
                        l.cam.lifted();
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
                                    // In or out by where it's heading, so a
                                    // double-click mid-zoom turns it round.
                                    let to = if l.cam.heading() > 1.001 {
                                        1.
                                    } else {
                                        closer(l.k)
                                    };
                                    l.drag = None;
                                    l.cam.ease_to(&l.frame, to, Lightbox::at(e.position));
                                } else if l.cam.zoomed_in() {
                                    // A press catches it mid-glide.
                                    l.cam.hold();
                                    l.drag = Some([(Instant::now(), e.position)].into());
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
                                                let to = if l.cam.heading() > 1.001 {
                                                    1.
                                                } else {
                                                    closer(l.k)
                                                };
                                                let about = l.frame.middle();
                                                l.cam.ease_to(&l.frame, to, about);
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
    use super::{closer, pixel_size};

    #[test]
    fn a_double_click_goes_to_its_own_size() {
        // Shrunk to half: its own size is twice the fit; at its own size
        // already, twice that.
        assert_eq!(closer(0.5), 2.);
        assert_eq!(closer(1.), 2.);
        assert_eq!(closer(0.25), 4.);
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
