//! ⌘/ (or ? with no text field in use): every shortcut on one sheet, by
//! what it's for. Esc or a click outside puts it away.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::Dent;
use crate::ds::{LABEL, Typed, UI, motion, radius, space};
use crate::ui::{Arrive, From, floating, kbd, probe};

/// The sheet: sections of (what it does, its keys).
pub const SHEET: &[(&str, &[(&str, &str)])] = &[
    (
        "Sessions",
        &[
            ("New session", "⌘ N"),
            ("Search sessions", "⌘ K"),
            ("Back to the last session", "⌃ Tab"),
            ("Previous or next session", "⌥ ⌘ ↑ ↓"),
            ("Next session that needs you", "⌘ J"),
            ("Close session", "⌘ W"),
            ("Undo closing", "⌘ Z"),
        ],
    ),
    (
        "Writing",
        &[
            ("Send", "↩"),
            ("New line", "⇧ ↩"),
            ("Attach files", "⇧ ⌘ A"),
            ("Paste text unfolded", "⇧ ⌘ V"),
            ("Send now, into the running turn", "⌘ ↩"),
            ("Stop", "⌘ .  Esc"),
            ("Bring back what you sent", "↑"),
            ("Edit the newest waiting message", "⌥ ↑"),
            ("Cycle the permission mode", "⇧ Tab"),
            ("Answer a waiting card", "Esc, then 1–4"),
            ("Deny with the note typed", "⌘ ↩"),
        ],
    ),
    (
        "Reading",
        &[
            ("Find in the session", "⌘ F"),
            ("Next or previous match", "⌘ G  ⇧ ⌘ G"),
            ("Your earlier messages", "⌥ ↑ ↓"),
            ("A page up or down", "PgUp  PgDn"),
            ("The start or end of the chat", "Home  End"),
            ("Bigger or smaller text", "⌘ +  ⌘ −"),
            ("Text as designed", "⌘ 0"),
        ],
    ),
    (
        "Panels",
        &[
            ("Show or hide the sidebar", "⌃ ⌘ S"),
            ("Show or hide the viewer", "⌘ \\"),
            ("Changes", "⌘ 2"),
            ("Your terminal", "⌘ 3"),
            ("Clear your terminal (in it)", "⌘ K"),
            ("Ask on the side", "⌘ ;"),
            ("This sheet", "⌘ /"),
        ],
    ),
];

impl Dent {
    pub fn toggle_keys(&mut self, cx: &mut Context<Self>) {
        self.keys_open = !self.keys_open;
        cx.notify();
    }

    pub fn keys_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.keys_open {
            return None;
        }
        let t = self.t;
        let section = |(name, rows): &(&'static str, &'static [(&'static str, &'static str)])| {
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(
                    div()
                        .pb(px(space::S1))
                        .ty(LABEL)
                        .text_color(t.quiet)
                        .child(name.to_uppercase()),
                )
                .children(rows.iter().map(|(what, keys)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S3))
                        .min_h(px(24.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .ty(UI)
                                .text_color(t.ink)
                                .child(*what),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(space::S1))
                                .children(keys.split("  ").map(|k| kbd(k, &t))),
                        )
                }))
        };
        let (left, right) = SHEET.split_at(2);
        Some(
            div()
                .id("keys-scrim")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(if t.dark {
                    gpui_kit::black().opacity(0.45)
                } else {
                    t.ink.opacity(0.08)
                })
                .flex()
                .justify_center()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.keys_open = false;
                        cx.notify();
                    }),
                )
                .child(
                    floating(&t)
                        .id("keys")
                        .relative()
                        .w(px(680.))
                        .max_w(relative(0.92))
                        .max_h(relative(0.86))
                        .overflow_y_scroll()
                        .rounded(px(radius::XL))
                        .p(px(space::S6))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .pb(px(space::S4))
                                .ty(UI)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.ink)
                                .child("Keyboard shortcuts"),
                        )
                        .child(div().flex().flex_wrap().gap(px(space::S8)).children(
                            [left, right].map(|col| {
                                div()
                                    .flex_1()
                                    .min_w(px(260.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(space::S6))
                                    .children(col.iter().map(section))
                            }),
                        ))
                        .child(probe(&self.probes, "keys"))
                        .arrive("keys-arrive", From::Above, motion::FAST),
                )
                .with_animation(
                    "keys-scrim-in",
                    Animation::new(Duration::from_millis(motion::FAST as u64))
                        .with_easing(crate::anim::ease_out),
                    |d, p| d.opacity(p),
                )
                .into_any_element(),
        )
    }
}
