//! ⌘F: find in the open session. Every match on screen is tinted and the
//! current one stands out; Return and ⇧Return (or ⌘G and ⇧⌘G) step
//! through them, scrolling the chat to each, and Esc puts the bar away.

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::Dent;
use crate::data::Item;
use crate::ds::{LABEL, Typed, UI, motion, radius, size, space};
use crate::ui::{Arrive, From, Press, Tipped, floating, icon, icon_button, probe, tip};

/// What the bar found, kept in step with the session as it grows.
pub struct Find {
    pub session: String,
    /// Each match: its item, and which match it is within that item.
    pub hits: Vec<(usize, usize)>,
    /// The current match.
    pub at: usize,
    /// What the hits were counted against: the query, the item count and
    /// the last item's length, so a reply streaming in is searched too.
    sig: (String, usize, usize),
}

impl Dent {
    /// ⌘F: open the bar on the session, or take the keyboard back to it
    /// with its text selected to type over.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.find.as_ref().is_none_or(|f| f.session != id) {
            self.find = Some(Find {
                session: id,
                hits: Vec::new(),
                at: 0,
                sig: (String::new(), usize::MAX, 0),
            });
        }
        self.find_input.update(cx, |f, cx| {
            let n = f.value().len();
            f.set_selected_range(0..n, cx);
            f.focus(window, cx);
        });
        self.refresh_find(true, cx);
        cx.notify();
    }

    /// Put the bar away; the chat keeps its place and the composer gets
    /// the keyboard.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.find.take().is_none() {
            return false;
        }
        crate::select::set_find(None);
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
        true
    }

    /// Count the matches again if the query or the session changed. With
    /// `jump`, the current match is the first one at or below what's on
    /// screen, and the chat scrolls to it.
    pub fn refresh_find(&mut self, jump: bool, cx: &mut Context<Self>) {
        let Some(f) = &self.find else { return };
        let Some(s) = self.session(&f.session) else {
            self.find = None;
            crate::select::set_find(None);
            return;
        };
        let query = self.find_input.read(cx).value().to_string();
        let sig = (
            query.clone(),
            s.items.len(),
            s.items.last().and_then(Item::plain).map_or(0, |t| t.len()),
        );
        if sig != f.sig {
            let hits: Vec<(usize, usize)> = if query.is_empty() {
                Vec::new()
            } else {
                s.items
                    .iter()
                    .enumerate()
                    .flat_map(|(ix, item)| {
                        let n = item
                            .plain()
                            .map_or(0, |t| crate::select::matches(&t, &query).len());
                        (0..n).map(move |k| (ix, k))
                    })
                    .collect()
            };
            // The first item you can see, at least partly.
            let view = self.transcript.viewport_bounds();
            let top = (0..s.items.len())
                .find(|ix| {
                    self.transcript
                        .bounds_for_item(*ix)
                        .is_some_and(|b| b.bottom() > view.top() + px(space::S6))
                })
                .unwrap_or_else(|| self.transcript.logical_scroll_top().item_ix);
            let f = self.find.as_mut().expect("find");
            let query_changed = f.sig.0 != sig.0;
            f.hits = hits;
            f.sig = sig;
            if query_changed || jump {
                // From where you're reading, so typing doesn't throw you
                // back to the start of a long session.
                f.at = f.hits.iter().position(|h| h.0 >= top).unwrap_or(0);
                self.reveal_find();
            } else {
                f.at = f.at.min(f.hits.len().saturating_sub(1));
            }
        }
        self.paint_find(cx);
    }

    /// Step to the next match, or the previous, round the ends.
    pub fn step_find(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            self.open_find(window, cx);
            return;
        }
        let Some(f) = self.find.as_mut() else { return };
        let n = f.hits.len();
        if n == 0 {
            return;
        }
        f.at = if back {
            (f.at + n - 1) % n
        } else {
            (f.at + 1) % n
        };
        self.reveal_find();
        self.paint_find(cx);
        cx.notify();
    }

    fn reveal_find(&mut self) {
        if let Some(&(ix, _)) = self.find.as_ref().and_then(|f| f.hits.get(f.at)) {
            self.transcript.scroll_to_reveal_item(ix);
        }
    }

    fn paint_find(&self, cx: &App) {
        let t = self.t;
        crate::select::set_find(self.find.as_ref().map(|f| crate::select::Find {
            area: crate::select::area(&format!("chat/{}", f.session)),
            needle: self.find_input.read(cx).value().to_string(),
            current: f.hits.get(f.at).map(|&(ix, k)| (ix as u32, k)),
            tint: t.waiting.0.opacity(if t.dark { 0.28 } else { 0.30 }),
            strong: t.waiting.0.opacity(if t.dark { 0.70 } else { 0.62 }),
        }));
    }

    /// The bar, over the chat's top right corner.
    pub fn find_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.find.as_ref()?;
        if Some(&f.session) != self.selected.as_ref() {
            return None;
        }
        let t = self.t;
        let typed = !self.find_input.read(cx).value().is_empty();
        let n = f.hits.len();
        let count = if !typed {
            String::new()
        } else if n == 0 {
            "No matches".into()
        } else {
            format!("{} of {n}", f.at + 1)
        };
        let step = |id: &'static str, name: &'static str, says: &'static str, keys, back: bool| {
            icon_button(id, name, &t)
                .size(px(size::CONTROL_SM))
                .when(n == 0, |d| d.opacity(0.45).cursor_not_allowed())
                .with_tip(tip(says, Some(keys), t))
                .on_press(cx.listener(move |this, _, window, cx| this.step_find(back, window, cx)))
        };
        Some(
            div()
                .absolute()
                .top(px(space::S2))
                .right(px(space::S4))
                .child(
                    floating(&t)
                        .id("find-bar")
                        .occlude()
                        .rounded(px(radius::LG))
                        .h(px(size::CONTROL + space::S2))
                        .pl(px(space::S3))
                        .pr(px(space::S1))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(icon("search", 13., t.quiet))
                        .child(
                            div()
                                .w(px(200.))
                                .ty(UI)
                                .text_color(t.ink)
                                .child(Textarea::new(&self.find_input)),
                        )
                        .child(
                            div()
                                .min_w(px(72.))
                                .flex()
                                .justify_end()
                                .ty(LABEL)
                                .text_color(if typed && n == 0 { t.needs.0 } else { t.quiet })
                                .child(count),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(step(
                                    "find-prev",
                                    "chevron-up",
                                    "Previous match",
                                    "⇧ ⌘ G",
                                    true,
                                ))
                                .child(step(
                                    "find-next",
                                    "chevron-down",
                                    "Next match",
                                    "⌘ G",
                                    false,
                                )),
                        )
                        .child(
                            icon_button("find-close", "x", &t)
                                .size(px(size::CONTROL_SM))
                                .with_tip(tip("Close", Some("Esc"), t))
                                .on_press(cx.listener(|this, _, window, cx| {
                                    this.close_find(window, cx);
                                })),
                        )
                        .child(probe(&self.probes, "find-bar"))
                        .arrive("find-arrive", From::Above, motion::FAST),
                )
                .into_any_element(),
        )
    }
}

/// Typing in the bar: count again and go to the nearest match.
pub fn query_changed(this: &mut Dent, cx: &mut Context<Dent>) {
    this.refresh_find(false, cx);
    cx.notify();
}
