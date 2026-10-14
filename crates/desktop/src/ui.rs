//! Small shared pieces, all built from design-system tokens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ds::{LABEL, Typed, UI, radius, size, space};
use crate::theme::Theme;

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
        .when(primary, |d| {
            d.bg(t.ink).text_color(t.surface).hover(|s| s.opacity(0.9))
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
        .hover(|s| s.bg(t.hover()).text_color(t.ink))
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
pub fn hint(text: &str, t: &Theme) -> Div {
    div()
        .min_w_0()
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
        .min_h(px(size::ROW + 4.))
        .px(px(space::S2))
        .py(px(space::S1))
        .mx(px(space::S1))
        .flex()
        .items_center()
        .gap(px(space::S2))
        .rounded(px(radius::MD))
        .cursor_pointer()
        .hover(|s| s.bg(t.hover()))
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
                .child(self.text.clone())
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
