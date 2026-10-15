//! Ctrl+Tab: back to the session you were just in. A tap swaps between
//! the last two; holding Ctrl and pressing Tab again steps further back
//! through the ones you used most recently, in a list that shows only once
//! you've held on a moment. Letting go of Ctrl opens the lit one; Esc
//! stays where you are.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::Dent;
use crate::ds::{LABEL, Typed, UI, motion, radius, space};
use crate::theme::status_square;
use crate::ui::{Arrive, From, Press, floating, icon, kbd, probe};

/// How long Ctrl is held before the list shows: a quick tap never flashes it.
const HOLD: Duration = Duration::from_millis(160);
/// The most sessions it lists.
const ROWS: usize = 9;

pub struct Switcher {
    /// Sessions, most recently used first; the first is the one you're in.
    pub order: Vec<String>,
    /// The lit row.
    pub at: usize,
    pub since: Instant,
}

impl Dent {
    /// Note that `id` was just opened, for Ctrl+Tab's order.
    pub fn used(&mut self, id: &str) {
        self.lately.retain(|r| r != id);
        self.lately.insert(0, id.to_string());
    }

    /// Ctrl+Tab (or with Shift, back the other way).
    pub fn switch_step(&mut self, back: bool, cx: &mut Context<Self>) {
        match &mut self.switcher {
            Some(sw) => {
                let n = sw.order.len();
                sw.at = if back {
                    (sw.at + n - 1) % n
                } else {
                    (sw.at + 1) % n
                };
            }
            None => {
                // Sessions still there, most recent first, then the rest in
                // the sidebar's order.
                let mut order: Vec<String> = self
                    .lately
                    .iter()
                    .filter(|id| self.session(id).is_some_and(|s| !s.closed))
                    .cloned()
                    .collect();
                for s in self.order() {
                    if !order.contains(&s) {
                        order.push(s);
                    }
                }
                // The one you're in leads, so the first step leaves it.
                if let Some(cur) = &self.selected {
                    order.retain(|o| o != cur);
                    order.insert(0, cur.clone());
                }
                order.truncate(ROWS);
                if order.len() < 2 {
                    return;
                }
                let at = if back { order.len() - 1 } else { 1 };
                self.switcher = Some(Switcher {
                    order,
                    at,
                    since: Instant::now(),
                });
                // Show the list if Ctrl is still held a moment from now.
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOLD).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Ctrl let go: open the lit session.
    pub fn switch_finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sw) = self.switcher.take()
            && let Some(id) = sw.order.get(sw.at).cloned()
        {
            self.select(Some(id), window, cx);
        }
        cx.notify();
    }

    pub fn switch_cancel(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.switcher.take().is_some();
        if open {
            cx.notify();
        }
        open
    }

    /// The list, centred near the top of the window, once Ctrl is held.
    pub fn switcher_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sw = self.switcher.as_ref()?;
        if sw.since.elapsed() < HOLD {
            return None;
        }
        let t = self.t;
        let rows: Vec<AnyElement> = sw
            .order
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let s = self.session(id)?;
                let on = i == sw.at;
                let open = id.clone();
                Some(
                    div()
                        .id(SharedString::from(format!("switch-{i}")))
                        .relative()
                        .mx(px(space::S1))
                        .px(px(space::S3))
                        .py(px(space::S1H))
                        .rounded(px(radius::MD))
                        .flex()
                        .items_center()
                        .gap(px(space::S3))
                        .cursor_pointer()
                        .when(on, |d| d.bg(t.selected()))
                        .when(!on, |d| d.hover(|d| d.bg(t.hover())))
                        .on_press(cx.listener(move |this, _, window, cx| {
                            this.switcher = None;
                            this.select(Some(open.clone()), window, cx);
                        }))
                        .child(status_square(s.status, &t))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .ty(UI)
                                .text_color(t.ink)
                                .when(on, |d| d.font_weight(FontWeight::MEDIUM))
                                .child(s.title.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(140.))
                                .truncate()
                                .ty(LABEL)
                                .text_color(t.quiet)
                                .child(if i == 0 {
                                    "Now".to_string()
                                } else {
                                    s.project.clone()
                                }),
                        )
                        .child(probe(&self.probes, format!("switch-{i}")))
                        .into_any_element(),
                )
            })
            .collect();
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(96.))
                .child(
                    floating(&t)
                        .id("switcher")
                        .occlude()
                        .w(px(420.))
                        .max_w(relative(0.9))
                        .py(px(space::S1))
                        .flex()
                        .flex_col()
                        .children(rows)
                        .child(
                            div()
                                .mt(px(space::S1))
                                .px(px(space::S3))
                                .pt(px(space::S2))
                                .pb(px(space::S1))
                                .border_t_1()
                                .border_color(t.hairline)
                                .flex()
                                .items_center()
                                .gap(px(space::S2))
                                .ty(LABEL)
                                .text_color(t.quiet)
                                .child(icon("arrow-left-right", 12., t.quiet))
                                .child("Tab to move on · let go of")
                                .child(kbd("⌃", &t))
                                .child("to open · Esc to stay"),
                        )
                        .arrive("switcher-arrive", From::Above, motion::FAST),
                )
                .into_any_element(),
        )
    }
}
