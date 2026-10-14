//! Small shared pieces, all built from design-system tokens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

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

pub fn icon(name: &str, px_size: f32, color: Hsla) -> Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(px_size))
        .flex_none()
        .text_color(color)
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
        Ready::No(why) => el.cursor_not_allowed().tooltip(tip(why, None, *t)),
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
        .tooltip(tip(why, None, *t))
}

/// A quiet keyboard hint. Hints give way first: they shrink far faster
/// than the controls beside them.
pub fn hint(text: &str, t: &Theme) -> Div {
    div()
        .min_w_0()
        .flex_shrink(8.)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .ty(LABEL)
        .text_color(t.faint)
        .child(text.to_string())
}

/// Uppercase section label, with an optional count.
pub fn section_label(text: &str, count: Option<usize>, t: &Theme) -> Div {
    div()
        .h(px(size::ROW))
        .px(px(space::S4))
        .flex()
        .items_end()
        .pb(px(space::S1))
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
        .bg(t.raised)
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
                    d.child(div().ty(UI).text_size(px(12.)).text_color(t.quiet).child(n))
                }),
        )
}

/// A tooltip: what the control does and its key, on a floating surface.
pub struct Tip {
    text: SharedString,
    keys: Option<&'static str>,
    t: Theme,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        div().pt(px(space::S1)).child(
            floating(&t)
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
                .when_some(self.keys, |d, k| d.child(kbd(k, &t))),
        )
    }
}

pub fn tip(
    text: &'static str,
    keys: Option<&'static str>,
    t: Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| {
        cx.new(|_| Tip {
            text: text.into(),
            keys,
            t,
        })
        .into()
    }
}

/// A tooltip with text made at run time, like a file's full path.
pub fn tip_owned(
    text: impl Into<SharedString>,
    t: Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| {
        cx.new(|_| Tip {
            text: text.clone(),
            keys: None,
            t,
        })
        .into()
    }
}
