//! Small shared pieces, all built from design-system tokens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ds::{LABEL, Typed, UI, motion, radius, size, space};
use crate::theme::Theme;

/// Controls that show only while the pointer is over their `group` (a row,
/// a tab, a message): hidden, not just transparent, so nothing unseen can
/// be clicked. The controls must sit inside the group's bounds (in its
/// padding, not hung outside on a negative margin), or they hide as the
/// pointer moves to them.
pub trait RevealOnHover: InteractiveElement + Styled + Sized {
    fn reveal_on_hover(self, group: impl Into<SharedString>) -> Self {
        self.invisible().group_hover(group, |s| s.visible())
    }
    /// The opposite: what makes way for the controls on hover.
    fn hide_on_hover(self, group: impl Into<SharedString>) -> Self {
        self.group_hover(group, |s| s.invisible())
    }
}
impl<T: InteractiveElement + Styled> RevealOnHover for T {}

/// A control: a press on it is its own, not the start of a text selection
/// underneath.
pub trait Control: InteractiveElement + Sized {
    fn control(self) -> Self {
        self.on_mouse_down(MouseButton::Left, |_, _, _| crate::select::claim())
    }
}
impl<T: InteractiveElement> Control for T {}

/// What a click does, it does once per click: the second click of a
/// double-click is ignored, because by then what was under the pointer may
/// have changed (an answered card gives way to the next one, Stop becomes
/// Send, a closed session's header to the next session's). Every control
/// uses this; only steppers, meant to be clicked again and again, keep
/// `on_click`.
pub trait Press: StatefulInteractiveElement + Sized {
    fn on_press(self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click(move |e, window, cx| {
            if e.click_count() <= 1 {
                f(e, window, cx)
            }
        })
    }
}
impl<T: StatefulInteractiveElement> Press for T {}

pub fn icon(name: &str, px_size: f32, color: Hsla) -> Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(px_size))
        .flex_none()
        .text_color(color)
}

/// A fold's chevron: right when folded, down when open, turning between
/// the two on `spring.snappy` so the fold reads as one thing moving. Its
/// spring is keyed `id` within the row; a row scrolled into view starts
/// settled.
pub fn disclosure(id: &'static str, open: bool, px_size: f32, color: Hsla) -> impl IntoElement {
    icon("chevron-right", px_size, color).with_spring(
        id,
        SpringAnimation::new(crate::anim::spring_config(0.25, 0.)).to(open),
        |s, phase| {
            s.with_transformation(Transformation::rotate(radians(
                phase.0 * std::f32::consts::FRAC_PI_2,
            )))
        },
    )
}

/// A 28px square button holding an icon, with a hover tint.
pub fn icon_button(id: impl Into<SharedString>, name: &str, t: &Theme) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .size(px(size::CONTROL))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::MD))
        .cursor_pointer()
        .control()
        .hover(|s| s.bg(t.hover()))
        .active(|s| s.bg(t.press()))
        .child(icon(name, 15., t.quiet))
}

/// A text button. Primary is ink on paper; the rest are quiet outlines.
pub fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    primary: bool,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .flex_none()
        .h(px(size::CONTROL))
        .px(px(space::S3))
        .flex()
        .items_center()
        .gap(px(space::S1H))
        .rounded(px(radius::MD))
        .ty(UI)
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .control()
        .when(primary, |d| {
            d.bg(t.ink)
                .text_color(t.surface)
                .hover(|s| s.opacity(0.9))
                .active(|s| s.opacity(0.8))
        })
        .when(!primary, |d| {
            d.border_1()
                .border_color(t.ink.opacity(0.16))
                .text_color(t.ink)
                .hover(|s| s.bg(t.hover()))
                .active(|s| s.bg(t.press()))
        })
        .child(label.into())
}

/// A button inside a card: the same button with the smaller corners the
/// design system gives controls nested in a rounded surface.
pub fn inner_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    primary: bool,
    t: &Theme,
) -> Stateful<Div> {
    button(id, label, primary, t).rounded(px(radius::SM))
}

/// An on/off switch: a quiet track when on, a wash when off, the knob
/// sliding between (`dur.fast`). Its hit area is a control's height
/// though the track is small. The caller makes `el` (an id, the click)
/// and wraps it in [`off`] while it can't be used.
pub fn switch(mut el: Stateful<Div>, on: bool, t: &Theme) -> impl IntoElement {
    const W: f32 = 26.;
    const H: f32 = 15.;
    const KNOB: f32 = 11.;
    let (off_bg, on_bg) = (t.ink.opacity(0.3), t.quiet);
    // White on Paper, as the Mac draws it; on Carbon the ink.
    let knob = if t.dark { t.ink } else { t.surface };
    let (edge, dark, knob_edge) = (t.hairline, t.dark, t.ink.opacity(0.25));
    let key = el.interactivity().element_id.clone();
    el.flex_none()
        .h(px(size::CONTROL))
        .w(px(W))
        .flex()
        .items_center()
        .cursor_pointer()
        .control()
        .with_spring(
            key.map_or_else(
                || ElementId::Name("switch".into()),
                |k| ElementId::Name(format!("{k}-state").into()),
            ),
            SpringAnimation::new(crate::anim::spring_config(motion::FAST / 1000., 0.)).to(on),
            move |el, phase| {
                let p = phase.0.clamp(0., 1.);
                let inset = (H - KNOB) / 2.;
                el.child(
                    div()
                        .relative()
                        .w(px(W))
                        .h(px(H))
                        .rounded_full()
                        .bg(Hsla::interpolate(off_bg, on_bg, p))
                        // Off, a hairline keeps the track from vanishing.
                        .border_1()
                        .border_color(Hsla::interpolate(edge, edge.opacity(0.), p))
                        .child(
                            div()
                                .absolute()
                                .top(px(inset))
                                .left(px(inset + (W - KNOB - inset * 2.) * p))
                                .size(px(KNOB))
                                .rounded_full()
                                .bg(knob)
                                .when(!dark, |d| d.border_1().border_color(knob_edge)),
                        ),
                )
            },
        )
}

/// A status mark: a dot in a signal colour, or a ring for off, so it
/// reads without colour too.
pub fn dot(color: Option<Hsla>, t: &Theme) -> Div {
    let d = div().size(px(7.)).flex_none().rounded_full();
    match color {
        Some(c) => d.bg(c),
        None => d.border_1().border_color(t.quiet),
    }
}

/// Whether the main action of a form can go, and if not, why.
#[derive(Clone, Copy, PartialEq)]
pub enum Ready {
    Yes,
    /// Unavailable; the reason shows on hover, like "Type a message first".
    No(&'static str),
}

/// The main action of a form (send, start): ink when it can go, a faint
/// wash when it can't. Its colours tween between the two (`dur.fast`), so
/// the change is seen; an unavailable one shows the not-allowed cursor and
/// says why on hover; an available one presses. `fill` draws the content in
/// the colour it's given. The caller adds the click, only when ready.
pub fn action(
    mut el: Stateful<Div>,
    ready: Ready,
    t: &Theme,
    fill: impl FnOnce(Stateful<Div>, Hsla) -> Stateful<Div> + 'static,
) -> impl IntoElement {
    let on = ready == Ready::Yes;
    let (off_bg, on_bg, off_fg, on_fg) = (t.ink.opacity(0.08), t.ink, t.faint, t.surface);
    let key = el.interactivity().element_id.clone();
    let el = el.control();
    let el = match ready {
        Ready::Yes => el.cursor_pointer().active(|s| s.opacity(0.82)),
        Ready::No(why) => el.cursor_not_allowed().with_tip(tip(why, None, *t)),
    };
    el.with_spring(
        key.map_or_else(
            || ElementId::Name("action".into()),
            |k| ElementId::Name(format!("{k}-state").into()),
        ),
        SpringAnimation::new(crate::anim::spring_config(motion::FAST / 1000., 0.)).to(on),
        move |el, phase| {
            let p = phase.0.clamp(0., 1.);
            fill(
                el.bg(Hsla::interpolate(off_bg, on_bg, p)),
                Hsla::interpolate(off_fg, on_fg, p),
            )
        },
    )
}

/// The send of a prompt box (a session's reply, a new session's task, a
/// side question): one square with an arrow, ink when it can go. `what`
/// names it on hover with its key. The caller gives `el` its id, and the
/// press only when ready.
pub fn send(el: Stateful<Div>, ready: Ready, what: &'static str, t: &Theme) -> impl IntoElement {
    action(
        el.relative()
            .flex_none()
            .size(px(size::CONTROL))
            .rounded(px(radius::MD))
            .flex()
            .items_center()
            .justify_center()
            .when(ready == Ready::Yes, |d| {
                d.with_tip(tip(what, Some("↩"), *t))
            }),
        ready,
        t,
        |d, fg| d.child(icon("arrow-up", 15., fg)),
    )
}

/// A small, flat, text-only button for inline actions.
pub fn ghost(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .flex_none()
        .h(px(size::CONTROL_SM))
        .px(px(space::S2))
        .flex()
        .items_center()
        .gap(px(space::S1))
        .rounded(px(radius::SM))
        .ty(UI)
        .text_color(t.quiet)
        .cursor_pointer()
        .control()
        .hover(|s| s.bg(t.hover()).text_color(t.ink))
        .active(|s| s.bg(t.press()).text_color(t.ink))
        .child(label.into())
}

/// A key, boxed like a keycap: ⌘Z, 1.
pub fn kbd(keys: &str, t: &Theme) -> Div {
    div()
        .flex_none()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::SM))
        .border_1()
        .border_color(t.hairline)
        .ty(LABEL)
        .text_color(t.quiet)
        .child(keys.to_string())
}

/// A quiet one-line hint that gives way before anything else does.
/// A control that can't be used right now: dimmed, the not-allowed
/// cursor, and on hover the reason. Shown rather than hidden, so things
/// don't shift about. The caller adds no click.
pub fn off(el: Stateful<Div>, why: &'static str, t: &Theme) -> Stateful<Div> {
    el.opacity(0.45)
        .cursor_not_allowed()
        .with_tip(tip(why, None, *t))
}

/// [`off`] with a reason made at run time.
pub fn off_owned(el: Stateful<Div>, why: String, t: &Theme) -> Stateful<Div> {
    el.opacity(0.45)
        .cursor_not_allowed()
        .with_tip(tip_owned(why, *t))
}

/// A quiet keyboard hint, set like the keys it names. Hints give way
/// first: they shrink far faster than the controls beside them.
pub fn hint(text: &str, t: &Theme) -> Div {
    div()
        .min_w_0()
        .flex_shrink(8.)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .ty(LABEL)
        .text_color(t.quiet)
        .child(text.to_string())
}

/// A quiet sentence that explains what's beside it ("Queued · sends when
/// this reply finishes"). Sentences are read, so they're never mono or
/// faint: those are for keys, paths and counts, and for separators.
pub fn note(text: impl Into<SharedString>, t: &Theme) -> Div {
    div().ty(UI).text_color(t.quiet).child(text.into())
}

/// Uppercase section label, with an optional count, at its natural
/// height. Where it sits (its sides, the space around it) is the
/// caller's, to line up with what it labels.
pub fn section_label(text: &str, count: Option<usize>, t: &Theme) -> Div {
    div()
        .flex()
        .items_end()
        .gap(px(space::S2))
        .ty(LABEL)
        .child(div().text_color(t.quiet).child(text.to_uppercase()))
        .when_some(count, |d, n| {
            d.child(div().text_color(t.faint).child(n.to_string()))
        })
}

thread_local! {
    /// The frame each probe was last drawn in, so a check never clicks
    /// where something used to be.
    pub static PROBE_FRAME: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}
pub static FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Notes where something was laid out this frame, under `name`.
pub fn record(
    probes: &Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    name: String,
    bounds: Bounds<Pixels>,
) {
    let frame = FRAME.load(std::sync::atomic::Ordering::Relaxed);
    PROBE_FRAME.with(|f| f.borrow_mut().insert(name.clone(), frame));
    probes.borrow_mut().insert(name, bounds);
}

/// Records an element's bounds under a name, so the walkthrough can click it
/// through normal hit-testing and checks can measure it.
pub fn probe(
    probes: &Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    name: impl Into<String>,
) -> impl IntoElement {
    let probes = probes.clone();
    let name = name.into();
    canvas(
        move |bounds, _, _| record(&probes, name, bounds),
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A floating surface (elevation 1): menus, toasts.
pub fn floating(t: &Theme) -> Div {
    div()
        .bg(t.popover())
        .border_1()
        .border_color(t.hairline)
        .rounded(px(radius::XL))
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., if t.dark { 0.45 } else { 0.12 }),
            offset: point(px(0.), px(8.)),
            blur_radius: px(24.),
            spread_radius: px(0.),
            inset: false,
        }])
}

/// One row of a menu: a check for the current choice, a name, a quiet note.
pub fn menu_row(
    probes: &Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    id: impl Into<SharedString>,
    name: &str,
    note: Option<String>,
    on: bool,
    t: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    div()
        .id(ElementId::Name(id.clone()))
        .relative()
        .child(probe(probes, id.to_string()))
        // A long menu scrolls; its rows never squeeze to fit.
        .flex_none()
        .min_h(px(size::ROW + 4.))
        .px(px(space::S2))
        .py(px(space::S1))
        .mx(px(space::S1))
        .flex()
        .items_center()
        .gap(px(space::S2))
        .rounded(px(radius::MD))
        .cursor_pointer()
        .control()
        .hover(|s| s.bg(t.hover()))
        .active(|s| s.bg(t.press()))
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .when(on, |d| d.child(icon("check", 14., t.ink))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().ty(UI).text_color(t.ink).child(name.to_string()))
                .when_some(note, |d, n| {
                    d.child(
                        div()
                            .ty(UI)
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(t.quiet)
                            .child(n),
                    )
                }),
        )
}

/// Where a surface comes from as it arrives.
#[derive(Clone, Copy)]
pub enum From {
    /// It settles downwards: menus and tooltips below their control.
    Above,
    /// It settles upwards: menus above their control, toasts, pills.
    Below,
}

/// The Arrive motion for chrome: it fades in and moves the last
/// `motion::NUDGE` px into place on `ease.out`, over `ms` (`dur.fast` for
/// what you open often, like menus; `dur.base` for what comes to you, like
/// a toast). It plays once each time the surface appears under `id`; the
/// offset never moves anything else, and reduce motion shows it settled.
/// It positions the element relative to its place, so put it on one in
/// the flow, never on one placed absolutely.
pub trait Arrive: Styled + IntoElement + 'static {
    fn arrive(self, id: impl Into<ElementId>, from: From, ms: f32) -> AnimationElement<Self> {
        let sign = match from {
            From::Above => -1.,
            From::Below => 1.,
        };
        self.relative().with_animation(
            id,
            Animation::new(Duration::from_millis(ms as u64)).with_easing(crate::anim::ease_out),
            move |d, p| d.opacity(p).top(px(sign * (1. - p) * motion::NUDGE)),
        )
    }
}
impl<T: Styled + IntoElement + 'static> Arrive for T {}

/// How long after a tooltip goes the next one still shows at once: moving
/// along a row of controls reads each one's name without waiting.
const TIP_WARM: Duration = Duration::from_millis(400);
/// The wait before the first tooltip, and before one while warm.
const TIP_COLD_DELAY: Duration = Duration::from_millis(500);
const TIP_WARM_DELAY: Duration = Duration::from_millis(60);

thread_local! {
    /// Tooltips on screen, and when the last one went.
    static TIPS: std::cell::Cell<(usize, Option<Instant>)> = const { std::cell::Cell::new((0, None)) };
    static TIP_IDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Whether a tooltip is showing or has just gone.
fn tips_warm() -> bool {
    let (live, gone) = TIPS.get();
    live > 0 || gone.is_some_and(|at| at.elapsed() < TIP_WARM)
}

/// Every control's tooltip goes through here, so they all wait and appear
/// the same way: the first after a moment, arriving; the next ones, while
/// one is showing or just went, at once and still.
pub trait Tipped: StatefulInteractiveElement + Sized {
    fn with_tip(self, build: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip_show_delay(if tips_warm() {
            TIP_WARM_DELAY
        } else {
            TIP_COLD_DELAY
        })
        .tooltip(build)
    }
}
impl<T: StatefulInteractiveElement> Tipped for T {}

/// A tooltip: what the control does and its key, on a floating surface.
pub struct Tip {
    text: SharedString,
    keys: Option<&'static str>,
    t: Theme,
    /// It belongs to something drawn over the window (the large picture),
    /// so it shows while that's open.
    above: bool,
    /// Its arrival's key, or none when another tooltip was just up and
    /// this one shows still.
    arrive: Option<u64>,
}

impl Tip {
    fn new(
        text: SharedString,
        keys: Option<&'static str>,
        t: Theme,
        above: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let arrive = (!tips_warm()).then(|| {
            let n = TIP_IDS.get() + 1;
            TIP_IDS.set(n);
            n
        });
        TIPS.set((TIPS.get().0 + 1, TIPS.get().1));
        cx.on_release(|_, cx| {
            TIPS.set((TIPS.get().0.saturating_sub(1), Some(Instant::now())));
            // Once warm has passed, controls are drawn again with the
            // first tooltip's wait.
            cx.spawn(async move |cx| {
                cx.background_executor().timer(TIP_WARM).await;
                cx.update(|cx| cx.refresh_windows());
            })
            .detach();
        })
        .detach();
        Self {
            text,
            keys,
            t,
            above,
            arrive,
        }
    }
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        // gpui decides a tooltip's hover by its element's bounds alone, so
        // something covered (by the large picture) would still show its own.
        if !self.above && crate::lightbox::covering() {
            return div().into_any_element();
        }
        let surface = floating(&t)
            .rounded(px(radius::MD))
            .px(px(space::S2))
            .py(px(space::S1))
            .flex()
            .items_center()
            .gap(px(space::S2))
            .ty(UI)
            .text_color(t.ink)
            // Long text, like a full title or path, wraps rather than
            // being cut off in the very tooltip meant to show it.
            .child(
                div()
                    .max_w(px(360.))
                    .whitespace_normal()
                    .child(self.text.clone()),
            )
            .when_some(self.keys, |d, k| d.child(kbd(k, &t)));
        let tip = div().pt(px(space::S1));
        match self.arrive {
            Some(n) => tip
                .child(surface.arrive(
                    ElementId::Name(format!("tip-{n}").into()),
                    From::Above,
                    motion::BASE,
                ))
                .into_any_element(),
            None => tip.child(surface).into_any_element(),
        }
    }
}

pub fn tip(
    text: &'static str,
    keys: Option<&'static str>,
    t: Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| {
        cx.new(|cx| Tip::new(text.into(), keys, t, false, cx))
            .into()
    }
}

/// A tooltip for a control on the large picture, shown over it.
pub fn tip_above(
    text: &'static str,
    keys: Option<&'static str>,
    t: Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|cx| Tip::new(text.into(), keys, t, true, cx)).into()
}

/// A tooltip with text made at run time, like a file's full path.
pub fn tip_owned(
    text: impl Into<SharedString>,
    t: Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| {
        cx.new(|cx| Tip::new(text.clone(), None, t, false, cx))
            .into()
    }
}
