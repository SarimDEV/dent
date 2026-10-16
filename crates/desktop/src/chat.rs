//! The main column: a session's conversation and composer, or the new-session
//! page where you start by typing.

use crate::app::Scope;
use crate::ui::Tipped;
use crate::ui::{Arrive, From};
use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::{Duration, Instant};

use crate::ui::Press;
use std::rc::Rc;

use crate::ui::RevealOnHover;

use crate::anim;
use crate::app::{Dent, Menu, Tab, home};
use crate::complete;
use dent_protocol::Permission;

use crate::data::{Item, Kind, Session};
use crate::ds::{
    CODE, DISPLAY, DISPLAY_COMPACT, LABEL, NUMERAL, READING, Typed, UI, motion, radius, size, space,
};
use crate::markdown;
use crate::theme::{self, Status, status_square};
use crate::ui::{
    Ready, action, button, floating, ghost, hint, icon, icon_button, kbd, menu_row, probe, tip,
    tip_owned,
};

impl Dent {
    pub fn main_column(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.current_rc() {
            Some(s) => self.chat(&s, window, cx),
            None => self.starter_page(cx),
        }
    }

    fn header(&self, s: &Session, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let id = s.id.clone();
        let has_viewer = self.shows_panel(s);
        // Short of room the title keeps its space: the project and machine
        // go first, then Fork and Close become icons that say what they do.
        let tight = self.chat_w < 600.;
        // Buttons here must not start a window drag.
        let mut actions = div()
            .flex()
            .items_center()
            .gap(px(space::S1))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if s.items.iter().any(|i| matches!(i, Item::User { .. })) {
            actions = actions.child(self.message_arrows(s, cx));
        }
        if s.stopped && !s.closed {
            let sid = id.clone();
            actions = actions.child(
                button("resume", "Resume", false, &t)
                    .on_press(cx.listener(move |this, _, _, cx| this.resume(&sid, cx))),
            );
        }
        {
            let sid = id.clone();
            let fork = if tight {
                icon_button("fork", "git-fork", &t).with_tip(tip(
                    "Fork: a new session that carries on from here",
                    None,
                    t,
                ))
            } else {
                button("fork", "Fork", false, &t).with_tip(tip(
                    "A new session that carries on from here",
                    None,
                    t,
                ))
            };
            actions = actions.child(
                fork.relative()
                    .on_press(cx.listener(move |this, _, _, cx| this.fork(&sid, None, cx)))
                    .child(probe(&self.probes, "fork")),
            );
        }
        // Close is always there; while it works or waits on you, it's off and
        // says why. The shortcut is in its tooltip and the menu bar.
        if !s.closed {
            let sid = id.clone();
            let close = if tight {
                icon_button("close", "archive", &t)
            } else {
                button("close", "Close", false, &t)
            };
            let close = if Self::can_close(s) {
                close
                    .with_tip(tip("Close", Some("⌘ W"), t))
                    .on_press(cx.listener(move |this, _, window, cx| {
                        this.close(&sid, window, cx);
                    }))
            } else {
                crate::ui::off(close, "Answer it or stop it first", &t)
            };
            actions = actions.child(close.relative().child(probe(&self.probes, "close")));
        }
        // Icon actions sit together, after the text ones.
        // Once the viewer shows a Terminal tab, the tab is the way there.
        let tab_shown = self.panel_open
            && has_viewer
            && (self.shells.contains_key(&s.id) || self.tabs.get(&s.id) == Some(&Tab::Terminal));
        if !tab_shown {
            actions = actions.child(
                icon_button("terminal", "square-terminal", &t)
                    .relative()
                    .on_press(cx.listener(|this, _, window, cx| this.open_shell(window, cx)))
                    .with_tip(tip("Your terminal", Some("⌘ 3"), t))
                    .child(probe(&self.probes, "terminal")),
            );
        }
        if !s.items.is_empty() {
            actions = actions.child(
                icon_button("side-chat", "message-circle-question-mark", &t)
                    .relative()
                    .on_press(cx.listener(|this, _, window, cx| this.open_side_chat(window, cx)))
                    .with_tip(tip("Ask on the side", Some("⌘ ;"), t))
                    .child(probe(&self.probes, "side-chat")),
            );
        }
        // Shown, the viewer has the toggle in its own header, in this same
        // corner of the window.
        if has_viewer && !self.panel_open {
            actions = actions.child(self.panel_toggle(cx));
        }
        // Whole pieces drop out when there's no room; nothing ends mid-word.
        let meta = div()
            .flex_1()
            .min_w_0()
            .h(px(16.))
            .overflow_hidden()
            .flex()
            .flex_wrap()
            .gap_x(px(space::S1H))
            .ty(LABEL)
            .text_color(t.quiet)
            .child(div().flex_none().child(s.project.clone()))
            // The separator travels with the machine, so it never dangles.
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap_x(px(space::S1H))
                    .child(div().text_color(t.faint).child("·"))
                    .child(s.machine.clone()),
            );
        let folded = self.sidebar_hidden;
        div()
            .h(px(size::HEADER))
            .flex_none()
            .px(px(space::S4))
            // The traffic lights sit here when the sidebar is folded away.
            .when(folded, |d| {
                d.pl(px(80.)).child(
                    icon_button("show-sidebar", "panel-left-open", &t)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_press(cx.listener(|this, _, _, cx| {
                            // Hidden by hand, it comes back as it was;
                            // folded for room, it shows over the viewer.
                            if this.sidebar_off {
                                this.toggle_sidebar(cx);
                            } else {
                                this.sidebar_forced = true;
                                this.panel_open = false;
                                cx.notify();
                            }
                        }))
                        .with_tip(tip("Show sessions", None, t)),
                )
            })
            .flex()
            .items_center()
            .gap(px(space::S3))
            .border_b_1()
            .border_color(t.hairline)
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    // The title wins the space; the metadata gives way first.
                    .map(|d| {
                        if self.renaming.as_deref() == Some(s.id.as_str()) {
                            d.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .ty(DISPLAY_COMPACT)
                                    .text_color(t.ink)
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .child(Textarea::new(&self.rename_input)),
                            )
                        } else {
                            let (sid, title) = (s.id.clone(), s.title.clone());
                            d.child(
                                div()
                                    .id("title")
                                    // With its details gone, the title takes all the room there is.
                                    // The title takes what it needs, up to all of it;
                                    // the project and machine get what's left.
                                    .map(|d| {
                                        if tight {
                                            d.flex_1().min_w_0()
                                        } else {
                                            d.flex_initial().min_w_0()
                                        }
                                    })
                                    .truncate()
                                    .with_tip(tip_owned(s.title.clone(), t))
                                    .ty(DISPLAY_COMPACT)
                                    .text_color(t.ink)
                                    // Double-click to rename; a single press still moves the window.
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                            if e.click_count == 2 {
                                                cx.stop_propagation();
                                                this.start_rename(&sid, &title, window, cx);
                                            }
                                        }),
                                    )
                                    .child(s.title.clone()),
                            )
                            .when(!tight, |d| d.child(meta))
                        }
                    }),
            )
            .child(actions)
    }

    /// Shows or hides the viewer. It sits in the window's top-right corner
    /// whether the viewer is shown or not (in the chat's header, then the
    /// viewer's), so pressing it again and again never lands on anything
    /// else.
    pub fn panel_toggle(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.t;
        let open = self.panel_open;
        // One glyph; shown, it reads as pressed, as the Mac's do.
        icon_button("panel", "panel-right", &t)
            .when(open, |d| d.bg(t.selected()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_press(cx.listener(|this, _, _, cx| {
                this.panel_open = !this.panel_open;
                this.end_pick();
                cx.notify();
            }))
            .with_tip(tip(
                if open { "Hide viewer" } else { "Show viewer" },
                Some("⌘ \\"),
                t,
            ))
    }

    /// A tool call opened up: the command or edit, what came back, and the
    /// way to its file. All of it selects and copies.
    /// A call opened up. `key` is the session and the call's id, so what's
    /// shown whole stays with the call, not with a place in the chat.
    fn call_detail(
        &self,
        s: &Session,
        key: &str,
        d: &crate::data::CallDetail,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.t;
        // Long text shows its start, and a line that opens the rest: held
        // to so many lines, and so many characters, so one enormous line
        // can't stall the chat.
        let clip = |text: &str, all: bool| -> String {
            let (kept, ..) = crate::ansi::clip(text, all);
            clipped(text, &kept)
        };
        let block = |id: String, text: String, ink: Hsla| code_lines(id, text, d.edits, ink, &t);
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .px(px(space::S3))
            .py(px(space::S2))
            .rounded(px(radius::MD))
            .bg(t.tint())
            .ty(CODE)
            .text_color(t.ink);
        // The line under a clipped block: how much more, and a press shows
        // it all; shown whole, a press folds it again.
        let rest = |which: &str, text: &str, cx: &mut Context<Self>| -> Option<AnyElement> {
            let fold = format!("call-{which}-all-{key}");
            let all = self.expanded.contains(&fold);
            let (_, lines, chars) = crate::ansi::clip(text, false);
            (lines + chars > 0).then(|| {
                let label = if all {
                    "Show less".to_string()
                } else if lines == 1 {
                    "Show 1 more line".to_string()
                } else if lines > 1 {
                    format!("Show {lines} more lines")
                } else {
                    "Show all".to_string()
                };
                crate::ui::ghost(SharedString::from(fold.clone()), label, &t)
                    .self_start()
                    .relative()
                    .ml(px(-space::S2))
                    .on_press(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&fold) {
                            this.expanded.insert(fold.clone());
                        }
                        cx.notify();
                    }))
                    .child(probe(&self.probes, format!("call-{which}-all")))
                    .into_any_element()
            })
        };
        if !d.input.trim().is_empty() {
            let all = self.expanded.contains(&format!("call-in-all-{key}"));
            let text = clip(&d.input, all);
            col = col
                .child(block(format!("call-in-{key}"), text, t.ink))
                .children(rest("in", &d.input, cx));
        }
        if let Some((failed, out)) = &d.output
            && !out.trim().is_empty()
            && !d.edits
        {
            let all = self.expanded.contains(&format!("call-out-all-{key}"));
            // Colour codes become colours; other escapes go.
            let parsed = crate::ansi::parsed(out);
            let (plain, spans) = (&parsed.0, &parsed.1);
            let (kept, ..) = crate::ansi::clip(plain, all);
            col = col
                .child(
                    div()
                        .ty(LABEL)
                        // Magenta means only "needs you": a failure is said
                        // in words, in ink.
                        .text_color(if *failed { t.ink } else { t.faint })
                        .child(if *failed { "Failed" } else { "Output" }),
                )
                .child(output_lines(
                    format!("call-out-{key}"),
                    plain,
                    spans,
                    &kept,
                    t.quiet,
                    &t,
                ))
                .children(rest("out", plain, cx));
        }
        // Its file: shown in dent when it can be, and its changes.
        let in_diff = d.path.as_ref().and_then(|p| {
            let rel = crate::bridge::relative(std::path::Path::new(&s.cwd), p);
            s.diff.iter().any(|f| f.path == rel).then_some(rel)
        });
        let openable = d
            .path
            .as_ref()
            .filter(|p| crate::data::is_file(std::path::Path::new(p)))
            .cloned();
        if openable.is_some() || in_diff.is_some() {
            let sid = s.id.clone();
            col = col.child(
                div()
                    .flex()
                    .gap(px(space::S1))
                    .when_some(openable, |c, p| {
                        c.child(
                            ghost(
                                SharedString::from(format!("call-open-{key}")),
                                "Open file",
                                &t,
                            )
                            .on_press(cx.listener(move |this, _, _, cx| this.open_file(&p, cx))),
                        )
                    })
                    .when_some(in_diff.filter(|_| d.edits), |c, rel| {
                        c.child(
                            ghost(
                                SharedString::from(format!("call-review-{key}")),
                                "Review changes",
                                &t,
                            )
                            .on_press(
                                cx.listener(move |this, _, _, cx| this.review(&sid, &rel, cx)),
                            ),
                        )
                    }),
            );
        }
        col
    }

    /// Up and down through your own messages, joined as one small pair (Alt+Up
    /// and Alt+Down from the keyboard). An arrow with nowhere to go dims.
    fn message_arrows(&self, s: &Session, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let top = self.transcript.logical_scroll_top().item_ix;
        let (prev, next) = s
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i, Item::User { .. }))
            .fold((false, false), |(p, n), (ix, _)| {
                (p || ix < top, n || ix > top)
            });
        // At the end there's nowhere further down; reading back, down
        // always leads on, to your next message or the end.
        let next = if self.transcript.is_following_tail() {
            false
        } else {
            next || !s.items.is_empty()
        };
        let half = |id: &'static str, up: bool, on: bool, cx: &mut Context<Self>| {
            let (name, label, off, keys) = if up {
                (
                    "chevron-up",
                    "Your previous message",
                    "No earlier message of yours",
                    "⌥ ↑",
                )
            } else {
                (
                    "chevron-down",
                    "Your next message",
                    "No later message of yours",
                    "⌥ ↓",
                )
            };
            div()
                .id(id)
                .relative()
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .when(!up, |d| d.border_l_1().border_color(t.hairline))
                .child(icon(name, 14., if on { t.quiet } else { t.faint }))
                .map(|d| {
                    if on {
                        d.cursor_pointer()
                            .hover(|d| d.bg(t.hover()))
                            .active(|d| d.bg(t.press()))
                            .with_tip(tip(label, Some(keys), t))
                            // A stepper: each click of a quick run steps.
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.jump_to_mine(up, cx);
                            }))
                    } else {
                        d.cursor_not_allowed().with_tip(tip(off, None, t))
                    }
                })
                .child(probe(&self.probes, id))
        };
        // Outlined beside the outlined text buttons; plain among icons.
        let outlined = self.chat_w >= 600.;
        div()
            .flex_none()
            .flex()
            .rounded(px(radius::MD))
            .when(outlined, |d| d.border_1().border_color(t.hairline))
            .overflow_hidden()
            .child(half("mine-prev", true, prev, cx))
            .child(half("mine-next", false, next, cx))
    }

    /// A Copy that says "Copied" for a moment. `text` runs only when
    /// pressed.
    fn copy_ghost(
        &self,
        key: String,
        text: impl Fn() -> String + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let copied = self
            .copied_key
            .as_ref()
            .is_some_and(|(k, at)| *k == key && at.elapsed() < Duration::from_millis(1300));
        ghost(
            SharedString::from(key.clone()),
            if copied { "Copied" } else { "Copy" },
            &self.t,
        )
        .relative()
        .child(probe(&self.probes, key.clone()))
        .on_press(cx.listener(move |this, _, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text()));
            this.copied_key = Some((key.clone(), Instant::now()));
            cx.notify();
            // Back to "Copy" once "Copied" has been read.
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1300))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }))
    }

    /// What a turn changed, as one line that opens Changes on that turn:
    /// "5 files changed +12 −6 Review".
    fn changed_line(
        &self,
        s: &Session,
        ix: usize,
        paths: &[String],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let (added, removed) = turn_counts(s, ix, paths);
        let n = paths.len();
        let (sid, first) = (s.id.clone(), paths[0].clone());
        // The turn it ends: your message before it, if there's a
        // snapshot from just before it to compare with.
        let turn = s.items[..ix]
            .iter()
            .rev()
            .find_map(|i| match i {
                Item::User { id, .. } => Some(id.clone()),
                _ => None,
            })
            .filter(|m| s.checkpoints.contains(m));
        div()
            .id(SharedString::from(format!("changed-{ix}")))
            .relative()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .ty(UI)
            .text_color(t.quiet)
            .cursor_pointer()
            .hover(|d| d.text_color(t.ink))
            // Opens Changes on just this turn, at its first file.
            .on_press(cx.listener(move |this, _, _, cx| {
                this.review_turn(&sid, turn.clone(), Some(first.clone()), cx)
            }))
            .child(icon("git-compare", 13., t.quiet))
            // The files are named just above, by the steps that changed
            // them; this sums the turn up.
            .child(if n == 1 {
                "1 file changed".to_string()
            } else {
                format!("{n} files changed")
            })
            .when(added + removed > 0, |d| {
                d.child(
                    div()
                        .text_color(t.faint)
                        .child(format!("+{added} −{removed}")),
                )
            })
            .child(div().text_color(t.ink).child("Review"))
            .child(probe(&self.probes, format!("changed-{ix}")))
            .into_any_element()
    }

    /// The last reply of a turn, with the turn's time and actions under
    /// it: shown on hover, and always on the latest turn.
    fn with_reply_footer(
        &mut self,
        s: &Session,
        ix: usize,
        (at, took, latest): (u64, u64, bool),
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let group = SharedString::from(format!("reply-{ix}"));
        // Everything the agent said in the turn, as Markdown.
        let start = s.items[..ix]
            .iter()
            .rposition(|i| matches!(i, Item::User { .. }))
            .map_or(0, |u| u + 1);
        let said: Vec<String> = s.items[start..=ix]
            .iter()
            .filter_map(|i| match i {
                Item::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        let errored = s.items[ix..].iter().any(|i| {
            matches!(
                i,
                Item::Note {
                    icon: "circle-alert",
                    ..
                }
            )
        });
        let can_try = latest && !errored && !s.closed && !s.running() && start > 0;
        let asking = can_try && self.trying_again.as_deref() == Some(s.id.as_str());
        let when = if took >= 1000 {
            format!(
                "{} · took {}",
                crate::data::clock(at),
                duration(took / 1000)
            )
        } else {
            crate::data::clock(at)
        };
        let footer = if asking {
            let (s1, s2) = (s.id.clone(), s.id.clone());
            div()
                .mt(px(space::S2))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(space::S2))
                .child(
                    div()
                        .ty(UI)
                        .text_color(t.ink)
                        .child("It changed files. Undo those too before trying again?"),
                )
                .child(div().flex_1())
                // The Mac order: the destructive choice leads, the default
                // trails, so Return never lands on it.
                .child(
                    button("try-again-files", "Undo them and try again", false, &t)
                        .relative()
                        .on_press(
                            cx.listener(move |this, _, _, cx| this.try_again(&s1, Some(true), cx)),
                        )
                        .child(probe(&self.probes, "try-again-files")),
                )
                .child(
                    ghost("try-again-cancel", "Cancel", &t).on_press(cx.listener(
                        |this, _, _, cx| {
                            this.trying_again = None;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    button("try-again-keep", "Try again", true, &t)
                        .relative()
                        .on_press(
                            cx.listener(move |this, _, _, cx| this.try_again(&s2, Some(false), cx)),
                        )
                        .child(probe(&self.probes, "try-again-keep")),
                )
        } else {
            let sid = s.id.clone();
            // What the turn changed leads the line and always shows; the
            // time and actions follow, on hover for an older turn.
            let changed = change_after(&s.items, ix).and_then(|c| match &s.items[c] {
                Item::Changed { paths } => Some(self.changed_line(s, c, paths, cx)),
                _ => None,
            });
            let has_changes = changed.is_some();
            div()
                .flex()
                // A narrow column wraps the time and actions under the
                // changes rather than running past its edge.
                .flex_wrap()
                .items_center()
                .gap_x(px(space::S3))
                .mt(px(space::S1))
                .min_h(px(size::CONTROL_SM))
                .children(changed)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S1))
                        .when(!latest, |d| d.reveal_on_hover(group.clone()))
                        .when(has_changes, |d| {
                            d.child(div().ty(UI).text_color(t.faint).child("·"))
                        })
                        // In line with the reply's text.
                        .child(
                            div()
                                .pr(px(space::S2))
                                .ty(LABEL)
                                .text_color(t.quiet)
                                .child(when),
                        )
                        .child(self.copy_ghost(
                            format!("copy-reply-{ix}"),
                            move || said.join("\n\n"),
                            cx,
                        ))
                        .when(can_try, |d| {
                            d.child(
                                ghost("try-again", "Try again", &t)
                                    .relative()
                                    .with_tip(tip(
                                        "Go back to before your last message and send it again",
                                        None,
                                        t,
                                    ))
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.try_again(&sid, None, cx)
                                    }))
                                    .child(probe(&self.probes, "try-again")),
                            )
                        }),
                )
        };
        div()
            .group(group)
            .flex()
            .flex_col()
            .child(body)
            .child(footer)
            .into_any_element()
    }

    fn chat(&mut self, s: &Session, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        self.sync_transcript(s);
        let count = self.transcript.item_count();
        // Only the items on screen are drawn, however long the session.
        let transcript = list(
            self.transcript.clone(),
            cx.processor(move |this, ix: usize, window, cx| {
                let Some(s) = this.current_rc() else {
                    return div().into_any_element();
                };
                // Each message keeps its place in the chat's selection,
                // drawn or not.
                let _place =
                    crate::select::item(crate::select::area(&format!("chat/{}", s.id)), ix);
                let body = match s.items.get(ix) {
                    Some(item) => this.item(&s, ix, item, window, cx),
                    None => this.working_line(&s),
                };
                // While "Edit from here" asks, what would go fades.
                let going = this.rewinding.as_ref().and_then(|(sid, msg)| {
                    (*sid == s.id).then(|| {
                        s.items
                            .iter()
                            .position(|i| matches!(i, Item::User { id, .. } if id == msg))
                    })?
                });
                let body = if going.is_some_and(|at| ix > at) {
                    div().opacity(0.35).child(body).into_any_element()
                } else {
                    body
                };
                // The last reply in a turn carries the turn's time and actions.
                let body = match turn_end_after(&s.items, ix) {
                    Some(end) => this.with_reply_footer(&s, ix, end, body, cx),
                    None => body,
                };
                let ends_turn = turn_end_after(&s.items, ix).is_some();
                let marker = matches!(s.items.get(ix), Some(Item::TurnEnd { .. }));
                // Just landed: it rises the last few pixels into place.
                let arrive = (!crate::reduce_motion())
                    .then(|| this.row_born.get(&ix))
                    .flatten()
                    .map(|(at, delay)| {
                        anim::ease_out(anim::progress_after(*at, *delay, motion::BASE))
                    });
                // The list sizes each row to its full width, so the column is
                // centred inside it, in line with the composer.
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(size::COLUMN + space::S8 * 2.))
                            .px(px(space::S8))
                            .when(ix == 0, |d| d.pt(px(space::S6)))
                            .when_some(arrive, |d, k| {
                                d.relative().top(px((1. - k) * motion::RISE)).opacity(k)
                            })
                            .pb(px(if ix + 1 == count {
                                space::S8
                            } else if marker || folded_change(&s.items, ix) {
                                0.
                            } else if ends_turn
                                || matches!(s.items.get(ix), Some(Item::User { .. }))
                            {
                                // The time and actions below your message
                                // take the rest of the gap.
                                space::S1
                            } else {
                                // Work sits close: tool lines in a run
                                // almost touch and hang under the words
                                // that led to them; a wider break follows,
                                // so each paragraph and its work read as
                                // one step. Paragraphs of reply breathe.
                                let work = |i: Option<&Item>| {
                                    matches!(
                                        i,
                                        None | Some(
                                            Item::Tools { .. }
                                                | Item::Thinking(_)
                                                | Item::Note { .. }
                                        )
                                    )
                                };
                                let next = s
                                    .items
                                    .get(ix + 1..)
                                    .unwrap_or_default()
                                    .iter()
                                    .find(|i| !matches!(i, Item::TurnEnd { .. }));
                                match (work(s.items.get(ix)), work(next)) {
                                    (true, true) => space::HALF,
                                    (false, true) => space::S3,
                                    (true, false) | (false, false) => space::S4,
                                }
                            }))
                            .child(body),
                    )
                    .into_any_element()
            }),
        )
        .size_full();
        // Read back far enough and a pill offers the way to the latest.
        let bottom = self.transcript.viewport_bounds().bottom();
        let away = !self.transcript.is_following_tail()
            && count > 0
            && self
                .transcript
                .bounds_for_item(count - 1)
                .is_none_or(|b| b.bottom() > bottom + px(space::S8));
        let state = self.transcript.clone();
        // Messages never drawn still copy, from what was said.
        let said = s.items.clone();
        crate::select::source(
            crate::select::area(&format!("chat/{}", s.id)),
            s.items.len(),
            move |ix| said.get(ix).and_then(Item::plain),
        );
        // ⌘F keeps up with a reply still arriving, and the match it went
        // to is brought into view within its message.
        self.refresh_find(false, cx);
        if self
            .find
            .as_ref()
            .is_some_and(|f| f.scope == crate::find::Scope::Chat)
        {
            self.reveal_found(s, window, cx);
        }
        let find = self.find_bar(crate::find::Scope::Chat, cx);
        let transcript = div()
            .relative()
            .flex_1()
            .min_h_0()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    this.chat_context(e.position, window, cx)
                }),
            )
            .child(transcript)
            .child(probe(&self.probes, "transcript"))
            .children(find)
            // A drag past the top or bottom scrolls the chat under it.
            .child(crate::select::region(
                crate::select::area(&format!("chat/{}", s.id)),
                move |dy| state.scroll_by(px(dy)),
            ));
        // Drawn with the composer, above its fade.
        let latest = away.then(|| {
            div()
                .absolute()
                .top(px(-(size::CONTROL + space::S2)))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("to-latest")
                        .relative()
                        .h(px(size::CONTROL))
                        .px(px(space::S3))
                        .flex()
                        .items_center()
                        .gap(px(space::S1H))
                        .rounded_full()
                        // On Carbon a raised fill is barely off the page: lift it a step.
                        .bg(if t.dark {
                            theme::mix_pub(t.raised, t.ink, 0.08)
                        } else {
                            t.raised
                        })
                        .border_1()
                        .border_color(t.hairline)
                        .shadow_md()
                        .ty(UI)
                        .text_color(t.quiet)
                        .cursor_pointer()
                        .hover(|d| d.text_color(t.ink))
                        .active(|d| d.opacity(0.82))
                        .on_press(cx.listener(|this, _, _, cx| {
                            this.transcript.set_follow_mode(FollowMode::Tail);
                            this.transcript.scroll_to_end();
                            cx.notify();
                        }))
                        .child(icon("arrow-down", 13., t.quiet))
                        .child("Latest")
                        .child(probe(&self.probes, "to-latest"))
                        .arrive("to-latest-arrive", From::Below, motion::BASE),
                )
        });
        let footer: AnyElement = if s.closed {
            let id = s.id.clone();
            div()
                .relative()
                .w_full()
                .max_w(px(size::COLUMN + space::S8 * 2.))
                .mx_auto()
                .px(px(space::S8))
                .pb(px(space::S4))
                // Reading back in a closed session gets the way down too.
                .children(latest)
                .child(
                    div()
                        .px(px(space::S4))
                        .py(px(space::S3))
                        .rounded(px(radius::XL))
                        .border_1()
                        .border_color(t.hairline)
                        .flex()
                        .items_center()
                        .gap(px(space::S3))
                        .child(icon("archive", 15., t.quiet))
                        .child(div().flex_1().ty(UI).text_color(t.quiet).child("Closed. It stays searchable, and reopening puts it back in your inbox."))
                        .child(button("reopen", "Reopen", true, &t).on_press(cx.listener(move |this, _, window, cx| {
                            this.reopen(&id, cx);
                            this.composer.update(cx, |c, cx| c.focus(window, cx));
                        }))),
                )
                .into_any_element()
        } else {
            // A short fade so the conversation runs under the composer, not into a cut.
            div()
                .relative()
                .child(
                    div()
                        .absolute()
                        .top(px(-FADE))
                        .left_0()
                        .right_0()
                        .h(px(FADE))
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(t.surface.opacity(0.), 0.),
                            linear_color_stop(t.surface, 1.),
                        )),
                )
                .children(latest)
                .child(self.composer_box(s, cx))
                .into_any_element()
        };
        div()
            .key_context("Chat")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.header(s, cx))
            .child(transcript)
            .child(footer)
            .into_any_element()
    }

    /// The last line while the agent works: what it's doing, and for how
    /// long. The square hangs in the gutter so the words line up with the
    /// reply; the words breathe once it's been a while, and each change of
    /// words hands over in place: the old rise out, the new rise in.
    fn working_line(&self, s: &Session) -> AnyElement {
        let t = self.t;
        let since = self.working_since.get(&s.id).copied();
        // Compacting or retrying says so, in place of the work.
        let special = s
            .activity
            .as_ref()
            .and_then(|a| crate::cards::activity_line(a, crate::cards::now_ms()));
        let (lead, detail) = match special {
            Some(line) => line,
            None => ("Working…".to_string(), s.detail.clone()),
        };
        let kind = match &s.activity {
            Some(dent_protocol::Activity::Compacting { .. }) => "compacting",
            Some(dent_protocol::Activity::Retrying { .. }) => "retrying",
            Some(dent_protocol::Activity::Preparing { .. }) | None => "working",
        };
        // The plan's current step, as the working line would say it.
        let step = s
            .todos
            .iter()
            .any(|(st, what, doing)| st == "in_progress" && (*what == detail || *doing == detail));
        let live = self.live_words(&s.id, kind, &detail, step);
        let still = crate::reduce_motion();
        let k = if still {
            1.
        } else {
            anim::progress(live.at, motion::BASE)
        };
        // The first half the old words leave, the second the new arrive.
        let (words, fade, rise) = if k < 0.5 && !live.was.is_empty() {
            let out = anim::ease_out(k * 2.);
            (live.was.clone(), 1. - out, -out * motion::SWAP)
        } else {
            let k = if live.was.is_empty() {
                k
            } else {
                (k - 0.5) * 2.
            };
            let inn = anim::ease_out(k.clamp(0., 1.));
            (live.words.clone(), inn, (1. - inn) * motion::SWAP)
        };
        let ran = since.map(|at| at.elapsed());
        // A slow breath once it's been a while: still working, not stuck.
        let breath = match ran {
            Some(r) if !still && r.as_millis() as f32 > motion::BREATHE_AFTER => {
                let p = (r.as_millis() as f32 - motion::BREATHE_AFTER) / motion::BREATHE;
                let wave = 0.5 - 0.5 * (p * std::f32::consts::TAU).cos();
                1. - (1. - motion::BREATHE_LOW) * wave
            }
            _ => 1.,
        };
        // The clock runs from the start, so the first seconds look alive.
        let elapsed = ran.map(|r| duration(r.as_secs()));
        // What it's doing says it all; "Working…" only stands in when
        // there are no words yet. Compacting and retrying keep their lead.
        let lead = (kind != "working" || words.is_empty()).then_some(lead);
        div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .ml(px(-(size::SQUARE + space::S2)))
            .ty(UI)
            .text_color(t.quiet)
            .child(status_square(
                if matches!(s.activity, Some(dent_protocol::Activity::Retrying { .. })) {
                    Status::Waiting
                } else {
                    Status::Working
                },
                &t,
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(space::S1H))
                    .opacity(breath)
                    .when_some(lead, |d, lead| d.child(div().flex_none().child(lead)))
                    .when(!words.is_empty(), |d| {
                        d.when(kind != "working", |d| {
                            d.child(div().flex_none().text_color(t.faint).child("·"))
                        })
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .relative()
                                .top(px(rise))
                                .opacity(fade)
                                .text_color(t.secondary())
                                .child(words),
                        )
                    })
                    .when_some(elapsed, |d, e| {
                        d.child(div().flex_none().text_color(t.faint).child("·"))
                            .child(div().flex_none().ty(NUMERAL).text_color(t.faint).child(e))
                    }),
            )
            .into_any_element()
    }

    fn item(
        &mut self,
        s: &Session,
        ix: usize,
        item: &Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        match item {
            // A command, as it was run, until its answer comes.
            Item::User { text, .. } if crate::data::command_name(text).is_some() => {
                crate::cards::command_line(text, "terminal", t.quiet, &t).into_any_element()
            }
            Item::User { id: msg, text, at } => {
                let (atts, body) = crate::data::attachment_lines(text);
                let asking = self
                    .rewinding
                    .as_ref()
                    .is_some_and(|(sid, m)| *sid == s.id && m == msg);
                let busy = s.running();
                let has_files = s.checkpoints.contains(msg);
                let group = SharedString::from(format!("user-{ix}"));
                let (m1, m2, m3, sid) = (msg.clone(), msg.clone(), msg.clone(), s.id.clone());
                div()
                    .group(group.clone())
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .max_w(relative(0.85))
                            .px(px(space::S3 + space::HALF))
                            .py(px(space::S2))
                            .rounded(px(radius::LG + space::HALF))
                            .bg(t.tint())
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .children({
                                // The message's pictures step together.
                                let lines: Vec<&str> =
                                    atts.iter().map(|a| a.trim_start_matches("› ")).collect();
                                let pictures: crate::lightbox::Set = std::rc::Rc::new(
                                    lines
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(j, l)| {
                                            let (place, what) = l.split_once(" · ")?;
                                            (place == "Attached").then_some(())?;
                                            Some(crate::lightbox::Shot {
                                                key: SharedString::from(format!("pic-{msg}-{j}")),
                                                path: picture_path(what, &s.cwd)?,
                                            })
                                        })
                                        .collect(),
                                );
                                lines
                                    .into_iter()
                                    .enumerate()
                                    .map(|(j, l)| {
                                        let key = SharedString::from(format!("pic-{msg}-{j}"));
                                        let at = pictures.iter().position(|p| p.key == key);
                                        sent_attachment(l, at.map(|at| (&pictures, at)), &t)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .when(!body.is_empty(), |d| {
                                // Long messages fold, so the reply stays in view.
                                let open = self.expanded.contains(msg);
                                let (shown, long) = user_shown(&body, open);
                                let key = msg.clone();
                                // A slash command reads as one.
                                let command = crate::data::command_name(&shown).is_some()
                                    && shown.len() < 80;
                                d.child(
                                    div()
                                        .ty(if command { CODE } else { READING })
                                        .text_color(t.ink)
                                        .child({
                                            // Addresses and files you named open
                                            // as they do in a reply.
                                            let links = crate::markdown::plain_links(
                                                &shown,
                                                Some(std::path::Path::new(&s.cwd)),
                                            );
                                            crate::select::text(
                                                SharedString::from(format!("user-text-{ix}")),
                                                shown,
                                                &t,
                                            )
                                            .links(links)
                                            .underline_links(&t)
                                        }),
                                )
                                .when(long, |d| {
                                    d.child(
                                        div()
                                            .id(SharedString::from(format!("fold-{ix}")))
                                            .ty(UI)
                                            .text_color(t.quiet)
                                            .cursor_pointer()
                                            .hover(|d| d.text_color(t.ink))
                                            .on_press(cx.listener(move |this, _, _, cx| {
                                                if !this.expanded.remove(&key) {
                                                    this.expanded.insert(key.clone());
                                                }
                                                cx.notify();
                                            }))
                                            .child(if open {
                                                "Show less"
                                            } else {
                                                "Show full message"
                                            }),
                                    )
                                })
                            }),
                    )
                    .map(|d| {
                        if asking {
                            // Going back asks once, in place.
                            d.pb(px(size::CONTROL_SM)).child(div().ty(UI).text_color(t.quiet).child(if busy {
                                "Stop it first, then go back"
                            } else {
                                "Go back to before this message? It goes back into the reply box."
                            }))
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .items_center()
                                    .gap(px(space::S2))
                                    // The Mac order: the destructive choice leads, the
                                    // default trails, so Return never lands on it.
                                    .when(!busy && has_files, |d| {
                                        d.child(
                                            button(
                                                SharedString::from(format!("rewind-files-{ix}")),
                                                "Go back and undo file changes",
                                                false,
                                                &t,
                                            )
                                            .relative()
                                            .on_press(cx.listener(move |this, _, window, cx| {
                                                this.rewind(&m2, true, window, cx)
                                            }))
                                            .child(
                                                probe(&self.probes, format!("rewind-files-{ix}")),
                                            ),
                                        )
                                        .child(div().flex_1())
                                    })
                                    .child(
                                        ghost(
                                            SharedString::from(format!("rewind-cancel-{ix}")),
                                            "Cancel",
                                            &t,
                                        )
                                        .on_press(
                                            cx.listener(|this, _, _, cx| {
                                                this.rewinding = None;
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .when(!busy, |d| {
                                        d.child(
                                            button(
                                                SharedString::from(format!("rewind-chat-{ix}")),
                                                "Go back",
                                                true,
                                                &t,
                                            )
                                            .relative()
                                            .on_press(cx.listener(move |this, _, window, cx| {
                                                this.rewind(&m1, false, window, cx)
                                            }))
                                            .child(
                                                probe(&self.probes, format!("rewind-chat-{ix}")),
                                            ),
                                        )
                                    }),
                            )
                        } else {
                            let (fsid, fmsg) = (s.id.clone(), msg.clone());
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S1))
                                    // In the gap below the message, part of
                                    // it: reaching for them keeps them shown.
                                    .h(px(size::CONTROL_SM))
                                    .reveal_on_hover(group.clone())
                                    // When you sent it, beside what you can do with it.
                                    .when(*at > 0, |d| {
                                        d.child(
                                            div()
                                                .px(px(space::S2))
                                                .ty(LABEL)
                                                .text_color(t.quiet)
                                                .child(crate::data::clock(*at)),
                                        )
                                    })
                                    .child({
                                        let words = body.join("\n");
                                        self.copy_ghost(
                                            format!("copy-mine-{ix}"),
                                            move || words.clone(),
                                            cx,
                                        )
                                    })
                                    .child(
                                        ghost(
                                            SharedString::from(format!("fork-from-{ix}")),
                                            "Fork from here",
                                            &t,
                                        )
                                        .relative()
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            this.fork(&fsid, Some(fmsg.clone()), cx)
                                        }))
                                        .child(probe(&self.probes, format!("fork-from-{ix}"))),
                                    )
                                    .child(
                                        ghost(
                                            SharedString::from(format!("edit-from-{ix}")),
                                            "Edit from here",
                                            &t,
                                        )
                                        .relative()
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            this.rewinding = Some((sid.clone(), m3.clone()));
                                            cx.notify();
                                        }))
                                        .child(probe(&self.probes, format!("edit-from-{ix}"))),
                                    ),
                            )
                        }
                    })
                    .into_any_element()
            }
            Item::Text(src) => {
                // What it says on the way sits a step back; the answer at
                // the end of a turn reads in full ink.
                let t = if is_answer(&s.items, ix, s.running()) {
                    t
                } else {
                    t.narrating()
                };
                let births = self
                    .births
                    .get(&s.id)
                    .filter(|(i, _)| *i == ix)
                    .map(|(_, b)| b.clone());
                // What its tables, wide blocks and diagrams remember is kept
                // under which reply it is, not its place in the list: a
                // thought or a note can land in front of it.
                let at = format!(
                    "{}/t{}",
                    s.id,
                    s.items[..ix]
                        .iter()
                        .filter(|i| matches!(i, Item::Text(_)))
                        .count()
                );
                // The reply streaming in: only its unfinished end is parsed.
                let blocks = if births.is_some() {
                    self.markdown_live(&s.id, src)
                } else {
                    self.markdown(src)
                };
                match (births, blocks.as_slice()) {
                    // A reply streaming in: each word fades in over 180 ms.
                    (Some(births), [markdown::Block::Para(inline)]) if !crate::reduce_motion() => {
                        let rendered = inline.text.clone();
                        let fade = move |off: usize| {
                            let k = rendered[..off.min(rendered.len())].matches(' ').count();
                            births
                                .get(k)
                                .map(|(_, at)| anim::ease_out(anim::progress(*at, motion::BASE)))
                                .unwrap_or(1.)
                        };
                        let face = markdown::Face::of(READING, theme::body(), t.ink, t.surface);
                        div()
                            // The measure it settles into once finished.
                            .max_w(px(crate::ds::measure()))
                            .ty(READING)
                            .text_color(t.ink)
                            .child(inline.element(
                                &format!("{at}/0"),
                                &t,
                                face,
                                Some(&fade),
                                &self.mdv,
                                window,
                            ))
                            .into_any_element()
                    }
                    _ => {
                        let mut r = markdown::R {
                            md: self.mdv.clone(),
                            owner: cx.entity_id(),
                            window,
                            cx,
                            scope: at.clone(),
                            bg: t.surface,
                            base: Some(std::path::PathBuf::from(&s.cwd)),
                        };
                        div()
                            .relative()
                            .child(markdown::render(blocks.as_slice(), &t, false, &mut r))
                            .child(probe(&self.probes, "reply"))
                            .into_any_element()
                    }
                }
            }
            Item::Tools {
                summary,
                lines,
                counted,
            } => {
                // A lone call opens straight to what it did; a group opens
                // to its calls, each of which opens in turn.
                let lone = lines.len() == 1 && lines[0].detail.is_some();
                let key = if lone {
                    lines[0].id.clone()
                } else {
                    lines.first().map(|l| l.id.clone()).unwrap_or_default()
                };
                let more = lone || lines.len() > 1 || lines.iter().any(|l| l.report.is_some());
                let open = more
                    && if lone {
                        self.open_calls.contains(&key)
                    } else {
                        self.open_tools.contains(&key)
                    };
                let group = SharedString::from(format!("tools-g-{ix}"));
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(SharedString::from(format!("tools-{ix}")))
                            .group(group.clone())
                            .flex()
                            .items_center()
                            .gap(px(space::S1H))
                            .ty(UI)
                            .text_color(t.quiet)
                            .relative()
                            .child(probe(&self.probes, "tool-row"))
                            .when(lone, |d| {
                                let line = lines[0].clone();
                                d.on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.tool_context(e.position, &line, cx)
                                    }),
                                )
                            })
                            .when(more, |d| {
                                d.cursor_pointer()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        let set = if lone {
                                            &mut this.open_calls
                                        } else {
                                            &mut this.open_tools
                                        };
                                        if !set.remove(&key) {
                                            set.insert(key.clone());
                                        }
                                        cx.notify();
                                    }))
                            })
                            // Flush with the reply's text; the chevron trails.
                            .child(if *counted {
                                // A count of work reads as one quiet phrase.
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .group_hover(group.clone(), |d| d.text_color(t.ink))
                                    .child(summary.clone())
                            } else {
                                tool_text(summary, more.then_some(group.clone()), &t)
                            })
                            .when(more, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .child(crate::ui::disclosure("fold", open, 11., t.faint)),
                                )
                            }),
                    )
                    .when(open && lone, |d| {
                        let detail = lines[0].detail.clone();
                        let call = format!("{}/{}", s.id, lines[0].id);
                        d.children(
                            detail.map(|dt| self.call_detail(s, &call, &dt, cx).mt(px(space::S1H))),
                        )
                    })
                    .when(open && !lone, |d| {
                        // A subagent's earlier steps stay folded until asked for.
                        let shown: Vec<(usize, &crate::data::ToolLine)> = lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| {
                                l.fold.as_ref().is_none_or(|k| self.expanded.contains(k))
                            })
                            .collect();
                        let depths: Vec<usize> = shown.iter().map(|(_, l)| l.depth).collect();
                        let rows: Vec<AnyElement> = shown
                            .iter()
                            .enumerate()
                            .map(|(i, &(n, l))| {
                                // Each level gets a column: a rule while that
                                // level carries on below, then this row's branch.
                                // The rails are drawn, so they join from row to
                                // row whatever each row's height.
                                // Rows sit in one card, a hairline between the
                                // group's own calls; a subagent's steps hang
                                // from its call on rails.
                                const PAD: f32 = space::S1;
                                let mut row = div()
                                    .flex()
                                    .px(px(space::S3))
                                    .py(px(PAD))
                                    .ty(UI)
                                    .text_color(t.quiet)
                                    .when(l.depth == 0 && i > 0, |d| {
                                        d.border_t_1().border_color(t.hairline)
                                    });
                                let rail = t.quiet.opacity(0.35);
                                for k in 1..=l.depth {
                                    let more = continues(&depths, i, k);
                                    let own = k == l.depth;
                                    row = row.child(
                                        div()
                                            .w(px(space::S4))
                                            .flex_none()
                                            .relative()
                                            .when(more || own, |d| {
                                                d.child(
                                                    div()
                                                        .absolute()
                                                        .left(px(5.))
                                                        .top_0()
                                                        .w(px(1.))
                                                        .bg(rail)
                                                        .map(|d| {
                                                            if more {
                                                                d.bottom(px(-PAD))
                                                            } else {
                                                                d.h(px(PAD + size::LINE / 2.))
                                                            }
                                                        })
                                                        .top(px(-PAD)),
                                                )
                                            })
                                            .when(own, |d| {
                                                d.child(
                                                    div()
                                                        .absolute()
                                                        .left(px(5.))
                                                        .top(px(size::LINE / 2.))
                                                        .w(px(7.))
                                                        .h(px(1.))
                                                        .bg(rail),
                                                )
                                            }),
                                    );
                                }
                                if let Some((key, hidden)) = l.opens.clone() {
                                    let opened = self.expanded.contains(&key);
                                    let steps = if hidden == 1 { "step" } else { "steps" };
                                    return row
                                        .child(
                                            div()
                                                .id(SharedString::from(format!("steps-{ix}-{n}")))
                                                .relative()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(t.faint)
                                                .cursor_pointer()
                                                .hover(|d| d.text_color(t.ink))
                                                .on_press(cx.listener(move |this, _, _, cx| {
                                                    if !this.expanded.remove(&key) {
                                                        this.expanded.insert(key.clone());
                                                    }
                                                    cx.notify();
                                                }))
                                                .child(if opened {
                                                    format!("Hide {hidden} earlier {steps}")
                                                } else {
                                                    format!("{hidden} earlier {steps}")
                                                })
                                                .child(probe(
                                                    &self.probes,
                                                    format!("steps-{ix}-{n}"),
                                                )),
                                        )
                                        .into_any_element();
                                }
                                match &l.report {
                                    None => {
                                        let call = l.id.clone();
                                        let opened = self.open_calls.contains(&call);
                                        let detail = l.detail.clone().filter(|_| opened);
                                        let gname =
                                            SharedString::from(format!("tool-line-g-{ix}-{n}"));
                                        let line = div()
                                            .id(SharedString::from(format!("tool-line-{ix}-{n}")))
                                            .group(gname.clone())
                                            .flex()
                                            .on_mouse_down(MouseButton::Right, {
                                                let line = l.clone();
                                                cx.listener(
                                                    move |this, e: &MouseDownEvent, _, cx| {
                                                        cx.stop_propagation();
                                                        this.tool_context(e.position, &line, cx)
                                                    },
                                                )
                                            })
                                            .min_w_0()
                                            .child(tool_text(
                                                &l.text,
                                                l.detail.is_some().then_some(gname),
                                                &t,
                                            ))
                                            .when(l.detail.is_some(), |d| {
                                                d.cursor_pointer().on_press(cx.listener(
                                                    move |this, _, _, cx| {
                                                        if !this.open_calls.remove(&call) {
                                                            this.open_calls.insert(call.clone());
                                                        }
                                                        cx.notify();
                                                    },
                                                ))
                                            });
                                        // Opened, its details sit in the row, so the
                                        // tree's rails run on beside them.
                                        row.child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(line)
                                                .children(detail.map(|dt| {
                                                    self.call_detail(
                                                        s,
                                                        &format!("{}/{}", s.id, l.id),
                                                        &dt,
                                                        cx,
                                                    )
                                                    .my(px(space::S1))
                                                })),
                                        )
                                        .into_any_element()
                                    }
                                    Some(report) => row
                                        .child(self.report(s, ix, &l.id, report, window, cx))
                                        .into_any_element(),
                                }
                            })
                            .collect();
                        d.child(
                            div()
                                .mt(px(space::S1H))
                                .py(px(space::HALF))
                                .rounded(px(radius::LG))
                                .border_1()
                                .border_color(t.hairline)
                                .flex()
                                .flex_col()
                                .children(rows),
                        )
                    })
                    .into_any_element()
            }
            Item::Thinking(text) => {
                let nth = s.items[..ix]
                    .iter()
                    .filter(|i| matches!(i, Item::Thinking(_)))
                    .count();
                let key = crate::app::thought_key(&s.id, nth);
                let open = self.open_calls.contains(&key);
                let words = text.split_whitespace().count();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1H))
                    .child(
                        div()
                            .id(SharedString::from(format!("thought-{ix}")))
                            .flex()
                            .items_center()
                            .gap(px(space::S1H))
                            .ty(UI)
                            .text_color(t.quiet)
                            .cursor_pointer()
                            .hover(|d| d.text_color(t.ink))
                            .on_press(cx.listener(move |this, _, _, cx| {
                                if !this.open_calls.remove(&key) {
                                    this.open_calls.insert(key.clone());
                                }
                                cx.notify();
                            }))
                            .child(format!(
                                "Thought · {words} word{}",
                                if words == 1 { "" } else { "s" }
                            ))
                            .child(crate::ui::disclosure("fold", open, 11., t.faint)),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .pl(px(space::S3))
                                .border_l_2()
                                .border_color(t.hairline)
                                .ty(READING)
                                .text_color(t.quiet)
                                .child(crate::select::text(
                                    SharedString::from(format!("thought-text-{ix}")),
                                    text.clone(),
                                    &t,
                                )),
                        )
                    })
                    .into_any_element()
            }
            Item::Made {
                art,
                version,
                review,
            } => self.made_card(s, ix, art, *version, review.as_deref(), cx),
            Item::Approval {
                id: ask,
                title,
                command,
                why,
                diff,
                keeps,
            } => {
                let (a, a2, d) = (s.id.clone(), s.id.clone(), s.id.clone());
                let (k1, k2, k3) = (ask.clone(), ask.clone(), ask.clone());
                let keyed = self.ask_focus.is_focused(window);
                needs_card(&t)
                    .id(SharedString::from(format!("ask-{ask}")))
                    .track_focus(&self.ask_focus)
                    .child(needs_title(title, &t))
                    // Why it wants to, in its own words, before the command.
                    .when(!why.is_empty(), |c| {
                        c.child(div().ty(UI).text_color(t.ink).child(why.clone()))
                    })
                    .when(!command.is_empty(), |c| {
                        c.child(
                            div()
                                .px(px(space::S3))
                                .py(px(space::S2))
                                .rounded(px(radius::SM))
                                .bg(t.raised)
                                .border_1()
                                .border_color(t.hairline)
                                .ty(CODE)
                                .text_color(t.ink)
                                // A long edit scrolls inside the card.
                                .max_h(px(320.))
                                .id(SharedString::from(format!("ask-{ask}")))
                                .overflow_y_scroll()
                                .child(code_lines(
                                    format!("ask-text-{ask}"),
                                    command.clone(),
                                    *diff,
                                    t.ink,
                                    &t,
                                )),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                button("allow", "Allow once", true, &t)
                                    .relative()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a, Some(&k1), Scope::Once, cx)
                                    }))
                                    .child(probe(&self.probes, "allow")),
                            )
                            .child(
                                button("allow-session", "Allow for this session", false, &t)
                                    .relative()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a2, Some(&k2), Scope::Session, cx)
                                    }))
                                    .child(probe(&self.probes, "allow-session")),
                            )
                            .child({
                                let el = button(
                                    "allow-project",
                                    "Always in this project",
                                    false,
                                    &t,
                                )
                                .relative()
                                .child(probe(&self.probes, "allow-project"));
                                if *keeps {
                                    let (a3, k4) = (s.id.clone(), ask.clone());
                                    el.with_tip(tip(
                                        "Saved to the project's own settings, where Claude Code keeps its rules",
                                        None,
                                        t,
                                    ))
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a3, Some(&k4), Scope::Project, cx)
                                    }))
                                } else {
                                    crate::ui::off(
                                        el,
                                        "Claude Code has no rule it can save for this",
                                        &t,
                                    )
                                }
                            })
                            .child(
                                button("deny", "Deny", false, &t)
                                    .relative()
                                    .on_press(cx.listener(move |this, _, window, cx| {
                                        this.deny(&d, Some(&k3), window, cx)
                                    }))
                                    .child(probe(&self.probes, "deny")),
                            ),
                    )
                    .child(ask_keys(
                        keyed,
                        if *keeps {
                            "↩ or 1 allows once · 2 for this session · 3 in this project · 4 denies"
                        } else {
                            "↩ or 1 allows once · 2 for this session · 4 denies"
                        },
                        "Esc, then a number, to answer by key",
                        &t,
                    ))
                    .into_any_element()
            }
            Item::Plan { id: ask, plan } => {
                let (a, d) = (s.id.clone(), s.id.clone());
                let (k1, k3) = (ask.clone(), ask.clone());
                let blocks = self.markdown(plan);
                let mut r = markdown::R {
                    md: self.mdv.clone(),
                    owner: cx.entity_id(),
                    window,
                    cx,
                    scope: format!("{}/plan-{ask}", s.id),
                    bg: t.raised,
                    base: Some(std::path::PathBuf::from(&s.cwd)),
                };
                let plan_view = markdown::render(blocks.as_slice(), &t, false, &mut r);
                let keyed = self.ask_focus.is_focused(window);
                needs_card(&t)
                    .id(SharedString::from(format!("ask-{ask}")))
                    .track_focus(&self.ask_focus)
                    .child(needs_title("Plan ready for review", &t))
                    .child(
                        div()
                            .px(px(space::S4))
                            .py(px(space::S3))
                            .rounded(px(radius::SM))
                            .bg(t.raised)
                            .border_1()
                            .border_color(t.hairline)
                            .child(plan_view),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                button("plan-go", "Start building", true, &t)
                                    .relative()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a, Some(&k1), Scope::Once, cx)
                                    }))
                                    .child(probe(&self.probes, "plan-go")),
                            )
                            .child(
                                button("plan-keep", "Keep planning", false, &t)
                                    .relative()
                                    .on_press(cx.listener(move |this, _, window, cx| {
                                        this.deny(&d, Some(&k3), window, cx)
                                    }))
                                    .child(probe(&self.probes, "plan-keep")),
                            ),
                    )
                    .child(ask_keys(
                        keyed,
                        "↩ or 1 starts building · 2 keeps planning, with what you typed",
                        "Typed text is sent as what to change · Esc, then 1 or 2, to answer by key",
                        &t,
                    ))
                    .into_any_element()
            }
            Item::Question { id: ask, questions } => {
                let picked = self.picks.get(ask).cloned().unwrap_or_default();
                let many = questions.len() > 1;
                // The question the number keys go to: the first unanswered.
                let next = (0..questions.len())
                    .find(|q| picked.get(*q).is_none_or(Option::is_none))
                    .unwrap_or(0);
                let keyed = self.ask_focus.is_focused(window);
                let fixed = questions.iter().any(|q| q.fixed);
                let n = questions.get(next).map_or(0, |q| q.options.len().min(9));
                let multi = questions.get(next).is_some_and(|q| q.multi);
                let hint = match (keyed, multi, fixed, many) {
                    (true, true, ..) => format!("Press 1–{n} to tick, ↩ to continue"),
                    (true, false, ..) => format!("Press 1–{n} to pick"),
                    (false, _, true, _) => "Esc, then a number key, to pick".to_string(),
                    (false, _, false, true) => {
                        "Answer each one, or type an answer in the box".to_string()
                    }
                    (false, _, false, false) => {
                        "Or type your own answer in the box · Esc to pick by number".to_string()
                    }
                };
                needs_card(&t)
                    .id(SharedString::from(format!("ask-{ask}")))
                    .track_focus(&self.ask_focus)
                    .children(questions.iter().enumerate().map(|(qi, q)| {
                        let chosen = picked.get(qi).cloned().flatten();
                        let ticked = self
                            .ticks
                            .get(&format!("{ask}/{qi}"))
                            .cloned()
                            .unwrap_or_default();
                        let flashing = self
                            .flash
                            .as_ref()
                            .filter(|(a, fq, _)| a == ask && *fq == qi)
                            .map(|(.., o)| o.clone());
                        let (sid_go, ask_go) = (s.id.clone(), ask.clone());
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S2))
                                    .child(status_square(Status::Needs, &t))
                                    .child(
                                        div()
                                            .ty(UI)
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(t.ink)
                                            .child(q.text.clone()),
                                    ),
                            )
                            .child(div().flex().flex_col().gap(px(space::S1)).children(
                                q.options.iter().enumerate().map(|(i, (label, desc))| {
                                    let (sid, o2, k) = (s.id.clone(), label.clone(), ask.clone());
                                    let on = chosen.as_deref() == Some(label.as_str())
                                        || flashing.as_deref() == Some(label.as_str())
                                        || ticked.contains(label);
                                    let key = format!("opt-{qi}-{i}");
                                    div()
                                        .id(SharedString::from(key.clone()))
                                        .relative()
                                        .px(px(space::S3))
                                        .py(px(space::S2))
                                        .rounded(px(radius::MD))
                                        .border_1()
                                        .border_color(if on {
                                            t.ink.opacity(0.5)
                                        } else {
                                            t.hairline
                                        })
                                        .bg(if on { t.selected() } else { t.raised })
                                        .cursor_pointer()
                                        .hover(|d| d.border_color(t.ink.opacity(0.3)))
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            this.pick_option(&sid, &k, qi, o2.clone(), cx)
                                        }))
                                        .flex()
                                        .items_start()
                                        .gap(px(space::S2))
                                        .child(
                                            div()
                                                .h(px(20.))
                                                .flex()
                                                .items_center()
                                                .child(kbd(&format!("{}", i + 1), &t)),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .child(
                                                    div()
                                                        .ty(UI)
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(t.ink)
                                                        .child(label.clone()),
                                                )
                                                .when(!desc.is_empty() && desc != label, |d| {
                                                    d.child(
                                                        div()
                                                            .ty(UI)
                                                            .text_color(t.quiet)
                                                            .child(desc.clone()),
                                                    )
                                                }),
                                        )
                                        .child(probe(&self.probes, key))
                                }),
                            ))
                            // Several can be ticked; this sends them.
                            .when(q.multi && chosen.is_none(), |d| {
                                let none = ticked.is_empty();
                                d.child(
                                    div().flex().child(
                                        button(
                                            SharedString::from(format!("ticks-{qi}")),
                                            if none {
                                                "Pick one or more".to_string()
                                            } else {
                                                format!("Continue with {}", ticked.len())
                                            },
                                            !none,
                                            &t,
                                        )
                                        .relative()
                                        .map(|b| {
                                            if none {
                                                b.opacity(0.5).cursor_not_allowed().with_tip(tip(
                                                    "Tick at least one choice first",
                                                    None,
                                                    t,
                                                ))
                                            } else {
                                                b.on_press(cx.listener(move |this, _, _, cx| {
                                                    this.finish_ticks(&sid_go, &ask_go, qi, cx)
                                                }))
                                            }
                                        })
                                        .child(probe(&self.probes, format!("ticks-{qi}"))),
                                    ),
                                )
                            })
                    }))
                    .child(div().ty(LABEL).text_color(t.faint).child(hint))
                    .into_any_element()
            }
            Item::Note { icon: name, text } => {
                // The latest failure, while a usage limit is used up, says
                // which and when it starts over, in plain words.
                let latest_failure = *name == "circle-alert"
                    && !s.items[ix + 1..]
                        .iter()
                        .any(|i| matches!(i, Item::User { .. }));
                let limit =
                    latest_failure
                        .then(|| self.limit_hit())
                        .flatten()
                        .map(|(which, at)| {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_secs() as i64);
                            format!(
                                "Your {which} is used up. It resets at {}, in {}.",
                                crate::data::clock(at as u64 * 1000),
                                crate::data::span((at - now).max(0) as u64)
                            )
                        });
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::S2))
                    .ty(UI)
                    .text_color(t.quiet)
                    .child(
                        div()
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .child(icon(name, 13., t.quiet)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(crate::select::text(
                                SharedString::from(format!("note-{ix}")),
                                text.clone(),
                                &t,
                            ))
                            .when_some(limit, |d, l| d.child(div().text_color(t.ink).child(l))),
                    )
                    // The last thing that went wrong can be tried again: once
                    // the limit it hit starts over, if it hit one.
                    .when(
                        *name == "circle-alert"
                            && s.status != Status::Working
                            && !s.items[ix + 1..]
                                .iter()
                                .any(|i| matches!(i, Item::User { .. })),
                        |d| {
                            let sid = s.id.clone();
                            let retry =
                                ghost(SharedString::from(format!("retry-{ix}")), "Try again", &t)
                                    .relative()
                                    .child(probe(&self.probes, format!("retry-{ix}")));
                            match self.limit_hit() {
                                Some((_, at)) => d.child(crate::ui::off_owned(
                                    retry,
                                    format!(
                                        "Once it resets, at {}",
                                        crate::data::clock(at as u64 * 1000)
                                    ),
                                    &t,
                                )),
                                None => d.child(retry.on_press(
                                    cx.listener(move |this, _, _, cx| this.retry(&sid, cx)),
                                )),
                            }
                        },
                    )
                    .into_any_element()
            }
            Item::Compacted {
                auto,
                before,
                after,
            } => self.compacted_line(ix, *auto, *before, *after),
            Item::Command {
                name,
                args,
                text,
                outcome,
                view,
            } => self.command_card(s, ix, name, args, text, outcome, view.as_ref(), window, cx),
            // Its time and actions sit under the turn's last reply.
            Item::TurnEnd { .. } => div().into_any_element(),
            // Folded into the footer of the reply above it, when there is one.
            Item::Changed { .. } if folded_change(&s.items, ix) => div().into_any_element(),
            Item::Changed { paths } => self.changed_line(s, ix, paths, cx),
            Item::Queued {
                id: msg,
                text,
                attachments,
            } => {
                // The waiting message before this one, which it can go ahead of.
                let ahead = ix.checked_sub(1).and_then(|p| match s.items.get(p) {
                    Some(Item::Queued { id, .. }) => Some(id.clone()),
                    _ => None,
                });
                // Folded pastes show as their chips again, and long text
                // folds as a sent message does.
                let open = self.expanded.contains(msg);
                let refolded = crate::compose::refold(text, &self.pastes);
                let body: Vec<&str> = refolded.lines().collect();
                let (shown, long) = user_shown(&body, open);
                let fold_key = msg.clone();
                let (sid3, msg3) = (s.id.clone(), msg.clone());
                let (sid, msg) = (s.id.clone(), msg.clone());
                let (sid2, msg2) = (s.id.clone(), msg.clone());
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .max_w(relative(0.85))
                            .px(px(space::S4))
                            .py(px(space::S2 + 2.))
                            .rounded(px(radius::LG))
                            .border_1()
                            .border_dashed()
                            .border_color(t.ink.opacity(0.28))
                            .ty(READING)
                            .text_color(t.quiet)
                            .flex()
                            .flex_col()
                            .gap(px(space::S1H))
                            .when(!shown.is_empty(), |d| {
                                d.child(crate::select::text(
                                    SharedString::from(format!("queued-text-{ix}")),
                                    shown,
                                    &t,
                                ))
                            })
                            .when(long, |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("queued-fold-{ix}")))
                                        .ty(UI)
                                        .text_color(t.quiet)
                                        .cursor_pointer()
                                        .hover(|d| d.text_color(t.ink))
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            if !this.expanded.remove(&fold_key) {
                                                this.expanded.insert(fold_key.clone());
                                            }
                                            cx.notify();
                                        }))
                                        .child(if open {
                                            "Show less"
                                        } else {
                                            "Show full message"
                                        }),
                                )
                            })
                            // The files it carries, as they'll go with it.
                            .when(!attachments.is_empty(), |d| {
                                d.child(div().flex().flex_wrap().gap(px(space::S1H)).children(
                                    attachments.iter().map(|p| {
                                        let a = crate::app::Attachment::File { path: p.clone() };
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(space::S1H))
                                            .max_w(px(320.))
                                            .h(px(size::CONTROL_SM))
                                            .px(px(space::S2))
                                            .rounded(px(radius::SM))
                                            .bg(t.selected())
                                            .child(icon(a.icon(), 12., t.quiet))
                                            .child(div().truncate().ty(UI).child(a.label()))
                                    }),
                                ))
                            }),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .ty(LABEL)
                                    .text_color(t.faint)
                                    .child(if ahead.is_some() {
                                        "Queued · sends after the one above"
                                    } else {
                                        "Queued · sends when this reply finishes"
                                    }),
                            )
                            .when_some(ahead, |d, before| {
                                d.child(
                                    ghost(
                                        SharedString::from(format!("move-up-{ix}")),
                                        "Move up",
                                        &t,
                                    )
                                    .relative()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.requeue(&sid3, &msg3, Some(before.clone()), cx)
                                    }))
                                    .child(probe(&self.probes, format!("move-up-{ix}"))),
                                )
                            })
                            .child(
                                ghost(SharedString::from(format!("unqueue-{ix}")), "Edit", &t)
                                    .relative()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.unqueue(&sid, &msg, cx)
                                    }))
                                    .child(probe(&self.probes, format!("unqueue-{ix}"))),
                            )
                            .child(
                                ghost(SharedString::from(format!("steer-{ix}")), "Send now", &t)
                                    .relative()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.steer_queued(&sid2, &msg2, cx)
                                    }))
                                    .child(probe(&self.probes, format!("steer-{ix}"))),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    /// The card for something the agent made: it opens in the viewer.
    fn made_card(
        &mut self,
        s: &Session,
        ix: usize,
        art: &'static str,
        version: usize,
        review: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let Some(a) = self.art(art).cloned() else {
            return div().into_any_element();
        };
        let latest = a.versions.len();
        let showing =
            self.panel_open && self.tab(s) == Tab::Art(art) && self.version(&a) == version;
        let old = version < latest;
        let file = a.path.rsplit('/').next().unwrap_or(a.path).to_string();
        let thumb = (a.kind == Kind::Image)
            .then(|| self.picture(art, version))
            .flatten();
        // This turn's edits include the file: its counts and Review sit on
        // the card, opening Changes at it.
        let counts = review
            .map(|p| turn_counts(s, ix, &[p.to_string()]))
            .filter(|(a, r)| a + r > 0);
        let review_button = review.map(|p| {
            let (sid, path) = (s.id.clone(), p.to_string());
            ghost(
                SharedString::from(format!("review-{art}-{version}")),
                "Review",
                &t,
            )
            .relative()
            .on_press(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.review(&sid, &path, cx);
            }))
            .child(probe(&self.probes, format!("review-{art}-{version}")))
        });
        if old {
            // Older versions fold to one line; the latest keeps the card.
            return div()
                .id(SharedString::from(format!("made-{art}-{version}")))
                .relative()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .ty(CODE)
                .text_color(t.quiet)
                .cursor_pointer()
                .hover(|d| d.text_color(t.ink))
                .on_press(cx.listener(move |this, _, _, cx| this.open_art(art, Some(version), cx)))
                .child(div().text_color(t.faint).child("├"))
                .child(format!("Wrote {file} v{version}"))
                .child(
                    div()
                        .text_color(t.faint)
                        .child(format!("replaced by v{}", version + 1)),
                )
                .when(showing, |d| {
                    d.child(div().ty(UI).text_color(t.ink).child("Showing"))
                })
                .when_some(review_button, |d, b| d.child(b))
                .child(probe(&self.probes, format!("made-{art}-{version}")))
                .into_any_element();
        }
        let note = if a.kind == Kind::Server {
            format!(
                "{} · {} on {}",
                a.title.trim_start_matches("Dev server · "),
                if self.servers_down.contains(art) {
                    "stopped"
                } else {
                    "running"
                },
                crate::data::machine_shown(&s.machine)
            )
        } else {
            format!("{} · v{version}", a.path)
        };
        div()
            .id(SharedString::from(format!("made-{art}-{version}")))
            .relative()
            .flex()
            .items_center()
            .gap(px(space::S3))
            .p(px(space::S2))
            .pr(px(space::S3))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if showing {
                t.ink.opacity(0.3)
            } else {
                t.hairline
            })
            .bg(t.raised)
            .cursor_pointer()
            .hover(|d| d.border_color(t.ink.opacity(0.3)))
            .on_press(cx.listener(move |this, _, _, cx| this.open_art(art, Some(version), cx)))
            .child(
                div()
                    .size(px(40.))
                    .flex_none()
                    .rounded(px(radius::SM))
                    .bg(t.selected())
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .map(|d| match thumb {
                        Some(picture) => {
                            d.child(img(picture).size_full().object_fit(ObjectFit::Cover))
                        }
                        None => {
                            d.child(icon(a.kind.icon(), 18., if old { t.faint } else { t.ink }))
                        }
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .ty(UI)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if old { t.quiet } else { t.ink })
                            // The address is on the line below; the title names the kind.
                            .child(if a.kind == Kind::Server {
                                "Dev server"
                            } else {
                                a.title
                            }),
                    )
                    .child(div().truncate().ty(LABEL).text_color(t.quiet).child(note)),
            )
            .when_some(counts, |d, (added, removed)| {
                d.child(
                    div()
                        .flex_none()
                        .ty(CODE)
                        .text_color(t.faint)
                        .child(format!("+{added} −{removed}")),
                )
            })
            .when_some(review_button, |d, b| d.child(b))
            // "Open" is what a press on the card does; "Showing" is only
            // a state, quieter than the Review beside it.
            .child(
                div()
                    .flex_none()
                    .ty(UI)
                    .text_color(if showing { t.faint } else { t.quiet })
                    .child(if showing { "Showing" } else { "Open" }),
            )
            .child(probe(&self.probes, format!("made-{art}-{version}")))
            .into_any_element()
    }

    fn composer_box(&mut self, s: &Session, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.t;
        let text = self.composer.read(cx).value();
        let empty = text.trim().is_empty() && self.attachments.is_empty();
        let working = s.running();
        // Long enough for a second press meant for Send (one at 11:24 on
        // 15 Oct came 2 s after a send); ⌘. still stops at once.
        let settling = self
            .just_sent
            .is_some_and(|at| at.elapsed().as_secs_f32() < 2.5);
        let id = s.id.clone();
        // While it works and nothing is typed, the button stops it. Right after
        // sending it stays inert, so a double press never lands on Stop.
        // Ink only while it works: waiting on you, the card's own answer is
        // the one loud thing, and Stop steps back to an outline.
        let loud = s.status == Status::Working;
        let action: AnyElement = if working && empty && !settling {
            div()
                .id("send")
                .relative()
                .flex_none()
                .size(px(size::CONTROL))
                .rounded(px(radius::MD))
                .flex()
                .items_center()
                .justify_center()
                .when(loud, |d| d.bg(t.ink).active(|d| d.opacity(0.8)))
                // An outline that still reads as working, set apart from the hint.
                .when(!loud, |d| {
                    d.ml(px(space::S2))
                        .border_1()
                        .border_color(t.ink.opacity(0.35))
                        .hover(|d| d.bg(t.hover()))
                        .active(|d| d.bg(t.press()))
                })
                .cursor_pointer()
                .on_press(cx.listener(move |this, _, window, cx| {
                    this.stop(&id, "the Stop button", window, cx)
                }))
                .with_tip(tip("Stop", Some("⌘ ."), t))
                .child(
                    div()
                        .size(px(9.))
                        .bg(if loud { t.surface } else { t.quiet }),
                )
                .child(probe(&self.probes, "send"))
                .into_any_element()
        } else {
            // New words typed since sending can go at once.
            let ready = if !empty {
                Ready::Yes
            } else if settling {
                Ready::No("Sending…")
            } else {
                Ready::No("Type a message first")
            };
            action(
                div()
                    .id("send")
                    .child(probe(&self.probes, "send"))
                    .relative()
                    .flex_none()
                    .size(px(size::CONTROL))
                    .rounded(px(radius::MD))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(ready == Ready::Yes, |d| {
                        d.on_press(cx.listener(|this, _, window, cx| this.send(window, cx)))
                            .with_tip(tip("Send", Some("↩"), t))
                    }),
                ready,
                &t,
                |d, fg| d.child(icon("arrow-up", 15., fg)),
            )
            .into_any_element()
        };
        let model = s
            .agent
            .split_once(" · ")
            .map_or(s.agent.as_str(), |(_, m)| m)
            .to_string();
        let permission = s.permission.label().to_string();
        let hint_text = if s.status == Status::Needs {
            s.items
                .iter()
                .rev()
                .find_map(|i| match i {
                    Item::Question { .. } => Some("↩ answer"),
                    Item::Approval { .. } => Some("↩ deny with this reason"),
                    Item::Plan { .. } => Some("↩ ask for changes"),
                    _ => None,
                })
                .unwrap_or("↩ send")
        } else if working {
            "↩ queue · ⌘ ↩ steer"
        } else {
            "↩ send"
        };
        // Short of room, things give way one at a time, in this order: the
        // hint, the context reading, "Default ·", effort's word, then the
        // permission's. Each chip keeps its icon and says what it is on hover.
        let room = self.chat_w.min(size::COLUMN + space::S8 * 2.) - space::S8 * 2. - space::S4 * 2.
            + space::S2
            - size::CONTROL
            - space::S2
            // The attach button.
            - size::CONTROL
            - space::S1;
        let effort_word = match s.effort.as_deref() {
            None => "Effort",
            e => effort_name(e),
        };
        let fit = fit_footer(
            room,
            [
                (
                    model.clone(),
                    model.trim_start_matches("Default · ").to_string(),
                ),
                (permission.clone(), String::new()),
                (
                    effort_label(s.effort.as_deref()).to_string(),
                    effort_word.to_string(),
                ),
            ],
            s.context.is_some(),
            hint_text,
        );
        let labels = fit.labels.clone();
        div()
            .w_full()
            .max_w(px(size::COLUMN + space::S8 * 2.))
            .mx_auto()
            .px(px(space::S8))
            .pb(px(space::S4))
            // The plan stands on its own over the composer, not inside it.
            .children(self.plan_strip(s, cx))
            .child(
                div()
                    .relative()
                    .rounded(px(radius::XL))
                    .bg(t.raised)
                    .border_1()
                    .border_color(t.hairline)
                    .px(px(space::S4))
                    .pt(px(space::S3))
                    .pb(px(space::S2))
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .children(self.attachment_chips(cx))
                    .child(self.prompt_field(&self.composer.clone(), cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S1))
                            .ml(px(-space::S2))
                            // Short of room, chips give up words in a fixed
                            // order: the default's "Default ·", then effort's
                            // label, then permission's; each keeps its icon
                            // and says what it is on hover. The model's name
                            // always shows: with no room even for that, the
                            // row wraps instead of squeezing.
                            .when(!fit.fits, |d| d.flex_wrap())
                            .child(self.attach_button(cx))
                            .child(keep(
                                !fit.fits,
                                self.chip(Menu::SessionModel, "claude", &labels[0], false, cx),
                            ))
                            .child(keep(
                                !fit.fits,
                                self.chip(Menu::SessionPermission, "shield", &labels[1], false, cx),
                            ))
                            .child(keep(
                                !fit.fits,
                                self.chip(Menu::SessionEffort, "gauge", &labels[2], false, cx),
                            ))
                            .child(div().flex_1())
                            .when_some(s.context.filter(|_| fit.context), |d, (used, max)| {
                                d.child(self.context_meter(s, used, max, cx))
                            })
                            // A dot only between two things that both show.
                            .when(fit.hint && fit.context && s.context.is_some(), |d| {
                                d.child(div().ty(LABEL).text_color(t.faint).child("·"))
                            })
                            .when(fit.hint, |d| d.child(hint(hint_text, &t)))
                            // At the right even when the row wraps.
                            .child(div().ml_auto().child(action)),
                    )
                    .child(probe(&self.probes, "composer"))
                    .children(self.slash_menu(false, cx))
                    .children(
                        (self.drop_target(cx) == Some(crate::app::DropTarget::Message)).then(
                            || {
                                crate::ui::drop_hint(
                                    "Drop to attach to your message",
                                    &self.probes,
                                    "drop-message",
                                    0.,
                                    &t,
                                )
                            },
                        ),
                    ),
            )
    }

    /// Opens the system's file picker; what you pick rides along with the
    /// next message.
    fn attach_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        icon_button("attach", "paperclip", &t)
            .relative()
            .flex_none()
            .on_press(cx.listener(|this, _, window, cx| this.pick_attachments(window, cx)))
            .with_tip(tip("Attach files", Some("⇧ ⌘ A"), t))
            .child(probe(&self.probes, "attach"))
            .into_any_element()
    }

    /// The agent's plan above the composer while it works: a checklist
    /// card while it's open (the default), one line ("Step 5 of 12 · what's
    /// at hand") when folded. Done steps step back, the current one wears the working
    /// square, the rest wait in the secondary ink.
    fn plan_strip(&self, s: &Session, cx: &mut Context<Self>) -> Option<AnyElement> {
        // A plan is for watching work happen: it shows while the agent works
        // or waits on you, and goes when the turn ends, ticked off or not.
        // (Waiting on you means a question it asked; an error also reads
        // as needing you, but the turn is over.)
        let asking = s.items.iter().rev().any(|i| {
            matches!(
                i,
                Item::Approval { .. } | Item::Question { .. } | Item::Plan { .. }
            )
        });
        let live = s.running() || asking;
        if s.todos.is_empty() || !live {
            return None;
        }
        let t = self.t;
        let total = s.todos.len();
        let done = s.todos.iter().filter(|(st, ..)| st == "completed").count();
        let at = s.todos.iter().position(|(st, ..)| st == "in_progress");
        let now = at.map(|i| {
            let (_, what, doing) = &s.todos[i];
            if doing.is_empty() {
                what.clone()
            } else {
                doing.clone()
            }
        });
        let open = self.plan_open;
        let head = div()
            .id("plan-strip")
            .flex()
            .items_center()
            .gap(px(space::S2))
            .cursor_pointer()
            .on_press(cx.listener(|this, _, _, cx| {
                this.plan_open = !this.plan_open;
                cx.notify();
            }))
            .child(icon("list-checks", 13., t.quiet))
            .child(div().ty(UI).text_color(t.quiet).child(match at {
                Some(i) if !open => format!("Step {} of {total}", i + 1),
                _ => format!("Plan · {done} of {total} done"),
            }))
            .when_some(now.filter(|_| !open), |d, now| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .ty(UI)
                        .text_color(t.ink)
                        .child(now),
                )
            })
            .when(open, |d| d.child(div().flex_1()))
            .child(icon(
                if open { "chevron-down" } else { "chevron-up" },
                11.,
                t.faint,
            ));
        Some(
            div()
                .mb(px(space::S2))
                .px(px(space::S4))
                .when(open, |d| {
                    d.py(px(space::S3))
                        .rounded(px(radius::XL))
                        .border_1()
                        .border_color(t.hairline)
                })
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(head)
                .when(open, |d| {
                    d.children(s.todos.iter().map(|(st, what, _)| {
                        let ink = match st.as_str() {
                            "completed" => t.quiet,
                            "in_progress" => t.ink,
                            _ => t.secondary(),
                        };
                        let mark = match st.as_str() {
                            "completed" => icon("check", 13., t.quiet).into_any_element(),
                            "in_progress" => status_square(Status::Working, &t).into_any_element(),
                            _ => div()
                                .size(px(9.))
                                .rounded_full()
                                .border_1()
                                .border_color(t.faint)
                                .into_any_element(),
                        };
                        div()
                            .flex()
                            .gap(px(space::S2))
                            .ty(UI)
                            .text_color(ink)
                            .child(
                                div()
                                    .w(px(14.))
                                    .h(px(20.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(mark),
                            )
                            .child(div().flex_1().min_w_0().child(what.clone()))
                    }))
                })
                .into_any_element(),
        )
    }

    /// What rides along with the next message, as chips above whichever
    /// prompt is showing: a session's composer or a new session's field.
    fn attachment_chips(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.t;
        // Each chip is known by what it holds, not where it sits, so taking
        // one off doesn't hand its state (a tooltip, a hover) to the next.
        let ident = |i: usize, a: &crate::app::Attachment| match a {
            crate::app::Attachment::File { path } => path.clone(),
            _ => format!("#{i}"),
        };
        // The pictures waiting to go step together in the large view.
        let pictures: crate::lightbox::Set = std::rc::Rc::new(
            self.attachments
                .iter()
                .enumerate()
                .filter_map(|(i, a)| match a {
                    crate::app::Attachment::File { path }
                        if Kind::of(path) == Some(Kind::Image) =>
                    {
                        Some(crate::lightbox::Shot {
                            key: SharedString::from(format!("att-pic-{}", ident(i, a))),
                            path: std::path::PathBuf::from(path),
                        })
                    }
                    _ => None,
                })
                .collect(),
        );
        let chips: Vec<AnyElement> = self
            .attachments
            .clone()
            .into_iter()
            .enumerate()
            .map(|(i, a)| {
                let id = ident(i, &a);
                let key = SharedString::from(format!("att-pic-{id}"));
                let shown = pictures.iter().position(|p| p.key == key);
                // What it is: a picture shows itself, small; anything else
                // its kind's icon.
                let what = div()
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .min_w_0()
                    .h_full()
                    .pl(px(space::S2))
                    .child(match &a {
                        crate::app::Attachment::File { path } if shown.is_some() => div()
                            .size(px(16.))
                            .flex_none()
                            .rounded(px(3.))
                            .overflow_hidden()
                            .child(
                                img(std::path::PathBuf::from(path))
                                    .size_full()
                                    .object_fit(ObjectFit::Cover),
                            )
                            .into_any_element(),
                        _ => icon(a.icon(), 12., t.quiet).into_any_element(),
                    })
                    .child(div().truncate().ty(UI).text_color(t.ink).child(a.label()))
                    .relative()
                    .when(shown.is_some(), |d| {
                        d.child(probe(&self.probes, "pic-chip"))
                    });
                div()
                    .id(SharedString::from(format!("att-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .max_w(px(420.))
                    .h(px(size::CONTROL_SM + 2.))
                    .pr(px(space::S1))
                    .rounded(px(radius::SM))
                    .bg(t.selected())
                    .with_tip(crate::ui::tip_owned(
                        match &a {
                            crate::app::Attachment::File { path } => path.replacen(&home(), "~", 1),
                            _ => a.label(),
                        },
                        t,
                    ))
                    // A picture's whole chip opens it large; the x still
                    // takes it off.
                    .child(match shown {
                        Some(j) => crate::lightbox::picture(&pictures, j, what)
                            .min_w_0()
                            .h_full()
                            .into_any_element(),
                        None => what.into_any_element(),
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("att-x-{id}")))
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .hover(|d| d.bg(t.press()))
                            .on_press(cx.listener(move |this, _, _, cx| {
                                if i < this.attachments.len() {
                                    this.attachments.remove(i);
                                }
                                cx.notify();
                            }))
                            .child(icon("x", 11., t.quiet)),
                    )
                    .into_any_element()
            })
            .collect();
        (!chips.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(space::S1H))
                .children(chips)
                .into_any_element()
        })
    }

    /// Where a prompt is typed. A click can move the caret into or out of
    /// a word to complete, so the menu looks again after one.
    fn prompt_field(
        &self,
        field: &Entity<gpui_kit::base::input::TextareaState>,
        cx: &mut Context<Self>,
    ) -> Div {
        // A folded paste opens to its full text when clicked.
        let me = cx.entity().downgrade();
        let f = field.clone();
        let t = self.t;
        div()
            .ty(READING)
            .text_color(self.t.ink)
            .capture_any_mouse_up(cx.listener(|_, _, _, cx| cx.notify()))
            .child(
                Textarea::new(field)
                    .token(move |tok, _, _| {
                        div()
                            .px(px(space::S1H))
                            .rounded(px(radius::SM))
                            .bg(t.selected())
                            .text_color(t.quiet)
                            .cursor_pointer()
                            .child(tok.token().label().clone())
                    })
                    .on_token_click(move |e, window, cx| {
                        let id = e.token().id().to_string();
                        let at = e.range();
                        let _ = me.update(cx, |this, cx| this.open_paste(&f, &id, at, window, cx));
                    }),
            )
    }

    /// The prompt's menu: commands after a `/` that opens it, files and
    /// folders after an `@`. Opens above the composer, below a new
    /// session's field, which sits near the top.
    fn slash_menu(&mut self, below: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.t;
        let c = self.completion(cx)?;
        let mention = c.token.kind == complete::Kind::Mention;
        let arg = c.token.kind == complete::Kind::Arg;
        let n = c.items.len();
        let at = self.slash_ix.min(n.saturating_sub(1));
        // The highlighted row stays in view as the arrows move it.
        if self.slash_shown != Some(at) {
            self.slash_shown = Some(at);
            self.slash_scroll.scroll_to_item(at);
        }
        let rows: Vec<AnyElement> = c
            .items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let name = item.name.clone();
                let on = i == at;
                let row = div()
                    .id(SharedString::from(format!("slash-{i}")))
                    .relative()
                    .flex_none()
                    .px(px(space::S3))
                    .h(px(size::ROW))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.selected()))
                    // The pointer lights what it's over, as the arrows do.
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.slash_ix != i {
                            this.slash_ix = i;
                            cx.notify();
                        }
                    }))
                    .on_press(cx.listener(move |this, _, window, cx| {
                        this.pick_completion(&name, window, cx);
                    }));
                let row = if mention {
                    // The name first; where it is, quieter, after.
                    let folder = item.name.ends_with('/');
                    let base = complete::base(&item.name);
                    let parent = item.name.trim_end_matches('/');
                    let parent = &parent[..parent.len() - base.len()];
                    row.child(icon(if folder { "folder" } else { "file" }, 13., t.quiet))
                        .child(
                            div()
                                .flex_none()
                                .max_w(relative(0.6))
                                .truncate()
                                .ty(UI)
                                .text_color(t.ink)
                                .child(if folder {
                                    format!("{base}/")
                                } else {
                                    base.to_string()
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .ty(UI)
                                .text_color(t.quiet)
                                .child(parent.trim_end_matches('/').to_string()),
                        )
                } else if arg {
                    // A value: as written, what it means, which is in
                    // effect; one of your own reads as an invitation.
                    // A name that only restates the value (`opus`, "Opus
                    // 5.5") shows once, as the name.
                    let restates = !item.about.is_empty()
                        && item
                            .about
                            .to_lowercase()
                            .starts_with(&item.label.to_lowercase());
                    let name_el = if item.open {
                        div().flex_none().ty(UI).text_color(t.ink).child("Custom…")
                    } else if restates {
                        div()
                            .flex_none()
                            .ty(UI)
                            .text_color(t.ink)
                            .child(item.about.clone())
                    } else {
                        div()
                            .flex_none()
                            .max_w(relative(0.5))
                            .truncate()
                            .ty(CODE)
                            .text_color(t.ink)
                            .child(item.label.clone())
                    };
                    let about = if item.open {
                        format!("Type {} after the command", item.label)
                    } else if restates {
                        String::new()
                    } else {
                        item.about.clone()
                    };
                    row.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(space::S2))
                            .child(name_el)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .ty(UI)
                                    .text_color(t.quiet)
                                    .child(about),
                            ),
                    )
                    .when(item.current, |d| d.child(icon("check", 13., t.ink)))
                    .when(item.more && !item.current, |d| {
                        d.child(icon("chevron-right", 12., t.faint))
                    })
                } else {
                    // The code face and the UI face share a baseline, not a
                    // centre line.
                    row.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(relative(0.45))
                                    .truncate()
                                    .ty(CODE)
                                    .text_color(t.ink)
                                    .child(format!("/{}", item.name)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .ty(UI)
                                    .text_color(t.quiet)
                                    .child(item.about.clone()),
                            ),
                    )
                    // It opens its choices once picked.
                    .when(item.more, |d| d.child(icon("chevron-right", 12., t.faint)))
                };
                row.child(probe(&self.probes, format!("slash-{i}")))
                    .into_any_element()
            })
            .collect();
        let sign = if mention { "@" } else { "/" };
        let note = match (n, c.loading, mention) {
            (0, true, true) => Some("Looking for files…".to_string()),
            (0, _, true) => Some(format!("No file or folder matches {sign}{}", c.token.query)),
            (0, _, false) => Some(format!("No command matches {sign}{}", c.token.query)),
            _ => None,
        };
        let footer = if n == 0 {
            "Esc to close"
        } else if c.loading {
            // dent's own commands are here; the agent's are on their way.
            "Looking up Claude's commands…"
        } else {
            "↑↓ choose · ↩ or Tab to pick · Esc to close"
        };
        // Painted after everything, so what follows the field (the chips,
        // the list below) never draws over it; outside the field's 1px
        // border, so their edges line up exactly.
        let menu = div()
            .absolute()
            .left(px(-1.))
            .right(px(-1.))
            .when(below, |d| d.top(relative(1.)).pt(px(space::S1)))
            .when(!below, |d| d.bottom(relative(1.)).pb(px(space::S1)))
            .child(
                floating(&t)
                    .id("slash-menu")
                    .p(px(space::S1))
                    .flex()
                    .flex_col()
                    .occlude()
                    // Clicks land on the menu, not on what's behind it,
                    // and the prompt keeps its caret.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(c.title.clone().map(|title| {
                        div()
                            .px(px(space::S3))
                            .pt(px(space::S1))
                            .pb(px(space::S1H))
                            .truncate()
                            .ty(UI)
                            .text_color(t.quiet)
                            .child(title)
                    }))
                    .child(
                        div()
                            .id("slash-rows")
                            .flex()
                            .flex_col()
                            .max_h(px(size::ROW * 8.))
                            .overflow_y_scroll()
                            .track_scroll(&self.slash_scroll)
                            .children(rows),
                    )
                    .children(note.map(|note| {
                        div()
                            .px(px(space::S3))
                            .h(px(size::ROW))
                            .flex()
                            .items_center()
                            .ty(UI)
                            .text_color(t.quiet)
                            .child(note)
                    }))
                    .child(
                        div()
                            .mt(px(space::S1))
                            .px(px(space::S3))
                            .py(px(space::S2))
                            .border_t_1()
                            .border_color(t.hairline)
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(footer),
                    )
                    // Settles away from the field it opens from.
                    .arrive(
                        "slash-arrive",
                        if below { From::Above } else { From::Below },
                        motion::FAST,
                    ),
            );
        Some(deferred(menu).with_priority(1).into_any_element())
    }

    fn starter_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let text = self.starter.read(cx).value();
        let empty = text.trim().is_empty() && self.attachments.is_empty();
        let d = self.defaults.clone();
        let project = d
            .project
            .as_deref()
            .map(Self::project_name)
            .unwrap_or_else(|| "No project".into());
        let model = crate::bridge::model_name(&self.models, d.model.as_deref());
        // Everything is already chosen; each choice is one click to change.
        // Sessions from a terminal wait behind one chip at the row's end;
        // its menu floats, so opening it moves nothing.
        let pickup = (!self.recent.is_empty()).then(|| {
            let label = if self.adopting.is_some() {
                "Opening…"
            } else {
                "Pick up from terminal"
            };
            self.chip(Menu::Pickup, "square-terminal", label, true, cx)
        });
        let chips = div()
            .flex()
            .flex_wrap()
            .gap(px(space::S1))
            .mx(px(-space::S2))
            .child(self.chip(Menu::Project, "folder", &project, d.project.is_none(), cx))
            .child(self.chip(Menu::Model, "claude", &model, false, cx))
            .child(self.chip(Menu::Permission, "shield", d.permission.label(), false, cx))
            .child(self.chip(
                Menu::Effort,
                "gauge",
                effort_label(d.effort.as_deref()),
                false,
                cx,
            ));
        // Last, at the row's end: it wraps with the rest and keeps its
        // whole label.
        let chips = chips.children(pickup.map(|p| div().flex_none().ml_auto().child(p)));
        let ready = if self.creating() {
            Ready::No("Starting the session…")
        } else if empty {
            Ready::No("Describe a task first")
        } else {
            Ready::Yes
        };
        let label = if self.creating() {
            "Starting…"
        } else {
            "Start"
        };
        let start = action(
            div()
                .id("start")
                .child(probe(&self.probes, "start"))
                .relative()
                .flex_none()
                .h(px(size::CONTROL))
                .px(px(space::S3))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .rounded(px(radius::MD))
                .ty(UI)
                .font_weight(FontWeight::MEDIUM)
                .when(ready == Ready::Yes, |d| {
                    d.on_press(cx.listener(|this, _, window, cx| this.start(window, cx)))
                }),
            ready,
            &t,
            move |d, fg| {
                // The key shows only when pressing it would do something.
                d.text_color(fg)
                    .child(label)
                    .when(ready == Ready::Yes, |d| {
                        d.child(div().ty(LABEL).opacity(0.7).child("↩"))
                    })
            },
        );
        // The ambient background, when one is chosen, sits behind everything;
        // it veils itself under the title and the settings row (probed here).
        let ambient = crate::ambient::layer(self);
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .children(ambient)
            .child(div().h(px(size::HEADER)).flex_none().on_mouse_down(
                MouseButton::Left,
                |e, window, _| {
                    if e.click_count == 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                },
            ))
            // The title and composer sit in the middle of the page.
            .child(div().flex_1())
            .child(
                div()
                    .w_full()
                    .max_w(px(size::COLUMN + space::S8 * 2.))
                    .mx_auto()
                    .px(px(space::S8))
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(probe(&self.probes, "ambient-ui"))
                    .relative()
                    .child(
                        div()
                            .self_start()
                            .relative()
                            .pb(px(space::S1))
                            .ty(DISPLAY)
                            .text_color(t.ink)
                            .child("New session")
                            .child(probe(&self.probes, "ambient-title")),
                    )
                    .child(
                        div()
                            .relative()
                            .rounded(px(radius::XL))
                            .bg(t.raised)
                            .border_1()
                            .border_color(t.hairline)
                            .px(px(space::S4))
                            .pt(px(space::S3))
                            .pb(px(space::S2))
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .children(self.attachment_chips(cx))
                            .child(self.prompt_field(&self.starter.clone(), cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .ml(px(-space::S2))
                                    .child(self.attach_button(cx))
                                    .child(start),
                            )
                            .child(probe(&self.probes, "starter"))
                            .children(self.slash_menu(true, cx))
                            .children(
                                (self.drop_target(cx) == Some(crate::app::DropTarget::Message))
                                    .then(|| {
                                        crate::ui::drop_hint(
                                            "Drop to attach to your message",
                                            &self.probes,
                                            "drop-message",
                                            0.,
                                            &t,
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .self_start()
                            .relative()
                            .child(chips)
                            .child(probe(&self.probes, "ambient-chips")),
                    ),
            )
            .child(div().flex_1())
            // As much room below as above the header.
            .child(div().h(px(size::HEADER)).flex_none())
            .into_any_element()
    }

    /// One remembered choice; a click opens its menu.
    fn chip(
        &mut self,
        which: Menu,
        icon_name: &str,
        label: &str,
        unset: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let open = self.menu == Some(which);
        let name = format!("chip-{which:?}").to_lowercase();
        // Chips give up width before anything beside them does: the label
        // ends in an ellipsis.
        div()
            .relative()
            .min_w_0()
            .child(
                div()
                    .id(SharedString::from(name.clone()))
                    .min_w_0()
                    .h(px(size::CONTROL_SM + 2.))
                    .px(px(space::S2))
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .when(open, |d| d.bg(t.selected()))
                    .hover(|d| d.bg(t.hover()))
                    .active(|d| d.bg(t.press()))
                    .on_press(cx.listener(move |this, _, window, cx| {
                        if this.menu == Some(which) {
                            this.menu = None;
                            cx.notify();
                        } else {
                            this.open_menu(which, window, cx);
                        }
                    }))
                    .child(icon(
                        icon_name,
                        13.,
                        if icon_name == "claude" {
                            crate::theme::CLAUDE
                        } else {
                            t.quiet
                        },
                    ))
                    .when(!label.is_empty(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .ty(UI)
                                .text_color(if unset { t.quiet } else { t.ink })
                                .child(label.to_string()),
                        )
                    })
                    // An icon-only chip still says what it is.
                    .when(label.is_empty(), |d| {
                        let current = self.current();
                        d.with_tip(tip_owned(
                            match which {
                                Menu::Effort => {
                                    format!(
                                        "Effort: {}",
                                        effort_label(self.defaults.effort.as_deref())
                                    )
                                }
                                Menu::SessionEffort => format!(
                                    "Effort: {}",
                                    effort_label(current.and_then(|s| s.effort.as_deref()))
                                ),
                                Menu::Permission => {
                                    format!("Permissions: {}", self.defaults.permission.label())
                                }
                                Menu::SessionPermission => format!(
                                    "Permissions: {}",
                                    current.map(|s| s.permission).unwrap_or_default().label()
                                ),
                                Menu::Model | Menu::SessionModel => "Model".into(),
                                Menu::Project => "Project".into(),
                                Menu::Pickup => "Pick up from terminal".into(),
                            },
                            t,
                        ))
                    })
                    .child(icon("chevron-down", 11., t.faint))
                    .child(probe(&self.probes, name.clone())),
            )
            .when(open, |d| {
                // As tall as the room between the chip and the window's
                // edge allows; longer lists scroll.
                // It opens on its usual side unless the other has clearly
                // more room, then is as tall as that room allows; longer
                // lists scroll.
                let (above, below) = self.probes.borrow().get(&name).map_or((0., 440.), |b| {
                    (b.top().as_f32(), self.window_h - b.bottom().as_f32())
                });
                let up = if which.upward() {
                    !(above < 320. && below > above)
                } else if which == Menu::Pickup {
                    // Opening upward would cover the composer; a shorter
                    // list that scrolls is better, unless there's no room.
                    below < 200. && above > below
                } else {
                    below < 320. && above > below
                };
                // Below, the status bar is in the way too.
                let fit = (if up {
                    above - space::S8
                } else {
                    below - space::S8 - size::STATUS
                })
                .clamp(160., 440.);
                let list = self.menu_list(which, fit, cx);
                let placed = if up {
                    anchored()
                        .anchor(if which.from_end() {
                            Anchor::BottomRight
                        } else {
                            Anchor::BottomLeft
                        })
                        .snap_to_window_with_margin(px(8.))
                        .child(div().mb(px(6.)).child(list).arrive(
                            ElementId::Name(format!("{name}-arrive").into()),
                            From::Below,
                            motion::FAST,
                        ))
                } else {
                    anchored()
                        .when(which.from_end(), |a| a.anchor(Anchor::TopRight))
                        .snap_to_window_with_margin(px(8.))
                        .child(div().mt(px(size::CONTROL_SM + 2. + 6.)).child(list).arrive(
                            ElementId::Name(format!("{name}-arrive").into()),
                            From::Above,
                            motion::FAST,
                        ))
                };
                // Placed from the chip's top corner, not from below it,
                // where the flow would put it.
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .map(|d| {
                            if which.from_end() {
                                d.right_0()
                            } else {
                                d.left_0()
                            }
                        })
                        .child(deferred(placed).with_priority(1)),
                )
            })
            .into_any_element()
    }

    /// What a menu holds, as data: the mouse and the keyboard both act on
    /// these, so a row does the same thing however it's chosen.
    pub fn menu_entries(&self, which: Menu, cx: &mut Context<Self>) -> Vec<Entry> {
        let d = self.defaults.clone();
        let act = |f: Box<ActFn>| -> Option<Act> { Some(Rc::from(f)) };
        match which {
            Menu::Project => self.project_entries(cx),
            Menu::Pickup => self.pickup_entries(),
            Menu::Model | Menu::SessionModel => {
                let current = self.menu_model(which, cx);
                let all = self.models_or_default();
                // Aliases follow the newest version; pinned versions are
                // full model ids, and wait behind "Older versions", in the
                // same family order as the list above.
                let (latest, mut older): (Vec<dent_protocol::Model>, Vec<dent_protocol::Model>) =
                    all.iter()
                        .cloned()
                        .partition(|m| !m.id.starts_with("claude-"));
                let family = |m: &dent_protocol::Model| {
                    let f = m.name.split(' ').next().unwrap_or_default().to_lowercase();
                    latest.iter().position(|l| l.id == f).unwrap_or(usize::MAX)
                };
                older.sort_by_key(|m| family(m));
                let open = self.older_models || older.iter().any(|m| m.id == current);
                let row = |m: &dent_protocol::Model| {
                    let id = m.id.clone();
                    // Each model is described once: the default names what it
                    // runs, older versions are only their names.
                    let (name, note) = if m.id == "default" {
                        (
                            "Default".to_string(),
                            crate::bridge::runs(&all, m).unwrap_or_default().to_string(),
                        )
                    } else if m.id.starts_with("claude-") {
                        (m.name.clone(), String::new())
                    } else {
                        (m.name.clone(), m.description.clone())
                    };
                    Entry::row(
                        format!("m-{}", m.id),
                        &name,
                        (!note.is_empty()).then_some(note),
                        current == m.id,
                        act(Box::new(move |this, _, cx| {
                            if which == Menu::Model {
                                this.defaults.model = Some(id.clone());
                            } else {
                                this.set_model(id.clone(), cx);
                            }
                        })),
                    )
                };
                let mut v: Vec<Entry> = latest.iter().map(row).collect();
                if v.is_empty() {
                    v.push(Entry::note("Asking Claude Code for its models…"));
                }
                if !older.is_empty() {
                    let at = v.len();
                    v.push(Entry {
                        act: act(Box::new(move |this, _, _| {
                            this.older_models = !this.older_models;
                            // What opened comes into view.
                            if this.older_models {
                                this.menu_scroll.scroll_to_top_of_item(at);
                            }
                        })),
                        ..Entry::of(
                            "m-older".into(),
                            &format!("Older versions · {}", older.len()),
                            EntryKind::Fold(open),
                        )
                    });
                    if open {
                        v.extend(older.iter().map(row));
                    }
                }
                v
            }
            Menu::Permission | Menu::SessionPermission => {
                let current = if which == Menu::Permission {
                    d.permission
                } else {
                    self.current().map(|s| s.permission).unwrap_or_default()
                };
                Permission::ALL
                    .iter()
                    .map(|p| {
                        let p = *p;
                        Entry::row(
                            format!("m-{p:?}").to_lowercase(),
                            p.label(),
                            Some(permission_note(p).to_string()),
                            current == p,
                            act(Box::new(move |this, _, cx| {
                                if which == Menu::Permission {
                                    this.defaults.permission = p;
                                } else {
                                    this.set_permission(p, cx);
                                }
                            })),
                        )
                    })
                    .collect()
            }
            Menu::Effort | Menu::SessionEffort => {
                let current = if which == Menu::Effort {
                    d.effort.clone()
                } else {
                    self.current().and_then(|s| s.effort.clone())
                };
                let model = self.menu_model(
                    if which == Menu::Effort {
                        Menu::Model
                    } else {
                        Menu::SessionModel
                    },
                    cx,
                );
                let takes = self
                    .models
                    .iter()
                    .find(|m| m.id == model)
                    .map(|m| m.efforts.clone())
                    .unwrap_or_default();
                EFFORTS
                    .iter()
                    // A level the model doesn't take isn't offered, unless
                    // it's the one already chosen.
                    .filter(|(value, _)| {
                        value.is_none_or(|v| {
                            takes.is_empty()
                                || takes.iter().any(|e| e == v)
                                || current.as_deref() == Some(v)
                        })
                    })
                    .map(|(value, note)| {
                        let v = value.map(str::to_string);
                        Entry::row(
                            format!("m-effort-{}", value.unwrap_or("default")),
                            effort_name(*value),
                            Some(note.to_string()),
                            current.as_deref() == *value,
                            act(Box::new(move |this, _, cx| {
                                if which == Menu::Effort {
                                    this.defaults.effort = v.clone();
                                } else {
                                    this.set_effort(v.clone(), cx);
                                }
                            })),
                        )
                    })
                    .collect()
            }
        }
    }

    /// Run a menu entry: a row closes the menu; a fold stays open.
    pub fn choose_entry(&mut self, e: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(act) = e.act.clone() else { return };
        if !matches!(e.kind, EntryKind::Fold(_)) && !e.keep {
            self.menu = None;
        }
        act(self, window, cx);
        cx.notify();
    }

    fn menu_list(
        &mut self,
        which: Menu,
        max_h: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = self.t;
        let entries = self.menu_entries(which, cx);
        // A menu where nothing is ticked keeps no room for the tick.
        let ticks = entries.iter().any(|e| e.on);
        let hi = self.menu_ix;
        let rows: Vec<AnyElement> =
            entries
                .into_iter()
                .enumerate()
                .map(|(i, e)| {
                    let lit = hi == Some(i);
                    match &e.kind {
                        EntryKind::Note => menu_note_owned(e.name.clone(), &t),
                        EntryKind::Separator => div()
                            .flex_none()
                            .my(px(space::S1))
                            .mx(px(space::S2))
                            .h(px(1.))
                            .bg(t.hairline)
                            .into_any_element(),
                        EntryKind::Fold(open) => div()
                            .id(SharedString::from(e.key.clone()))
                            .flex_none()
                            .relative()
                            .child(probe(&self.probes, e.key.clone()))
                            .mx(px(space::S1))
                            .mt(px(space::S1))
                            .px(px(space::S2))
                            .h(px(size::ROW))
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .rounded(px(radius::MD))
                            .border_t_1()
                            .border_color(t.hairline)
                            .cursor_pointer()
                            .when(lit, |d| d.bg(t.hover()))
                            .hover(|d| d.bg(t.hover()))
                            .active(|d| d.bg(t.press()))
                            .child(crate::ui::disclosure("fold", *open, 12., t.quiet))
                            .child(
                                div()
                                    .ty(UI)
                                    .text_size(px(12.))
                                    .text_color(t.quiet)
                                    .child(e.name.clone()),
                            )
                            .on_press(cx.listener(move |this, _, window, cx| {
                                this.choose_entry(&e, window, cx)
                            }))
                            .into_any_element(),
                        EntryKind::Row => menu_row(
                            &self.probes,
                            e.key.clone(),
                            &e.name,
                            e.note.clone(),
                            e.on,
                            ticks,
                            e.off.is_some(),
                            &t,
                        )
                        .when(lit && e.off.is_none(), |d| d.bg(t.hover()))
                        // Shown, so you know where it went, but not to be
                        // chosen, with why.
                        .when_some(e.off, |d, why| d.with_tip(tip(why, None, t)))
                        // Its shortcut as plain text, as a Mac menu shows one,
                        // then a slot that holds the remove button on the lit
                        // row: the same width on every row, so nothing shifts.
                        .when_some(e.keycap.clone(), |d, k| {
                            d.child(div().flex_none().ty(UI).text_color(t.quiet).child(k))
                        })
                        .when_some(e.remove.clone(), |d, (tip_text, remove)| {
                            d.child(div().size(px(size::CONTROL_SM + 2.)).flex_none().when(
                                lit,
                                |d| {
                                    d.child(
                                        div()
                                            .id(SharedString::from(format!("{}-remove", e.key)))
                                            .size(px(size::CONTROL_SM + 2.))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(radius::SM))
                                            .cursor_pointer()
                                            .hover(|d| d.bg(t.press()))
                                            .on_press(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                remove(this, window, cx);
                                            }))
                                            .with_tip(tip(tip_text, Some("⌘ ⌫"), t))
                                            .child(icon("x", 12., t.quiet)),
                                    )
                                },
                            ))
                        })
                        .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                            if *h && this.menu_ix != Some(i) {
                                this.menu_ix = Some(i);
                                cx.notify();
                            }
                        }))
                        .on_press(
                            cx.listener(move |this, _, window, cx| {
                                this.choose_entry(&e, window, cx)
                            }),
                        )
                        .into_any_element(),
                    }
                })
                .collect();
        // A list that scrolls fades at an edge with more beyond it, so a
        // row cut by the edge reads as "there's more", not as a mistake.
        let (y, max_y) = (
            self.menu_scroll.offset().y.as_f32(),
            self.menu_scroll.max_offset().y.as_f32(),
        );
        let fade = |top: bool| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(space::S6))
                .map(|d| if top { d.top_0() } else { d.bottom_0() })
                .bg(linear_gradient(
                    if top { 180. } else { 0. },
                    linear_color_stop(t.popover(), 0.),
                    linear_color_stop(t.popover().opacity(0.), 1.),
                ))
        };
        // A menu that searches has its field on top, and room for paths.
        let field = which.searches().then(|| {
            div()
                .px(px(space::S3))
                .py(px(space::S2))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .border_b_1()
                .border_color(t.hairline)
                .child(icon("search", 13., t.quiet))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .ty(UI)
                        .text_color(t.ink)
                        .child(Textarea::new(&self.menu_query)),
                )
        });
        floating(&t)
            .relative()
            .w(px(match which {
                Menu::Pickup => 320.,
                w if w.wide() => 360.,
                _ => 280.,
            }))
            .overflow_hidden()
            .occlude()
            .children(field)
            .child(
                div()
                    .id("menu")
                    // The field above takes its share of the room.
                    .max_h(px(if which.searches() { max_h - 40. } else { max_h }))
                    .overflow_y_scroll()
                    .track_scroll(&self.menu_scroll)
                    .py(px(space::S1))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .when(y < -1., |d| d.child(fade(true)))
            .when(max_y > 1. && y > -max_y + 1., |d| d.child(fade(false)))
    }

    /// Sessions started in a terminal, newest first. One still running
    /// there is listed, so you know where it went, but picking it up now
    /// would run it twice.
    fn pickup_entries(&self) -> Vec<Entry> {
        let row = |i: usize, r: &dent_protocol::RecentSession| {
            let id = r.agent_session.clone();
            let project = Self::project_name(&r.cwd);
            let note = if r.open {
                format!("{project} · In use in a terminal")
            } else {
                format!("{project} · {}", crate::data::ago(r.updated))
            };
            let act: Option<Act> = (!r.open).then(|| {
                Rc::new(
                    move |this: &mut Dent, _: &mut Window, cx: &mut Context<Dent>| {
                        this.adopt(&id, cx)
                    },
                ) as Act
            });
            Entry {
                off: r
                    .open
                    .then_some("Running in a terminal. Close it there to pick it up here."),
                ..Entry::row(format!("adopt-{i}"), &r.title, Some(note), false, act)
            }
        };
        // The ones you can pick up come first; those still running in a
        // terminal wait below a line.
        let all = self.recent.iter().enumerate();
        let free: Vec<Entry> = all
            .clone()
            .filter(|(_, r)| !r.open)
            .map(|(i, r)| row(i, r))
            .collect();
        let busy: Vec<Entry> = all
            .filter(|(_, r)| r.open)
            .map(|(i, r)| row(i, r))
            .collect();
        let line = (!free.is_empty() && !busy.is_empty()).then(Entry::separator);
        free.into_iter().chain(line).chain(busy).collect()
    }

    /// The model a model menu is choosing for: the new session's, or the
    /// open session's.
    fn menu_model(&self, which: Menu, _cx: &mut Context<Self>) -> String {
        if which == Menu::Model {
            self.defaults.model.clone()
        } else {
            self.current().and_then(|s| s.model.clone())
        }
        .unwrap_or_else(|| "default".into())
    }
}

/// A quiet line in a menu that isn't a choice.
/// What a menu entry does when chosen.
pub type ActFn = dyn Fn(&mut Dent, &mut Window, &mut Context<Dent>);
pub type Act = Rc<ActFn>;

/// One thing in a menu.
#[derive(Clone)]
pub struct Entry {
    pub key: String,
    pub name: String,
    pub note: Option<String>,
    /// It's the current choice: a check beside it.
    pub on: bool,
    pub kind: EntryKind,
    pub act: Option<Act>,
    /// The shortcut that chooses it, shown at its end (⌘1).
    pub keycap: Option<String>,
    /// Taking it off the list, from a button on the lit row or ⌘⌫, with
    /// what the button says.
    pub remove: Option<(&'static str, Act)>,
    /// Choosing it leaves the menu open (it changes what the menu shows).
    pub keep: bool,
    /// Listed but not to be chosen now, and why.
    pub off: Option<&'static str>,
}

#[derive(Clone)]
pub enum EntryKind {
    Row,
    /// Shows or hides more rows (open or not); the menu stays.
    Fold(bool),
    /// Words, not a choice.
    Note,
    /// A hairline between groups.
    Separator,
}

impl Entry {
    fn of(key: String, name: &str, kind: EntryKind) -> Self {
        Self {
            key,
            name: name.to_string(),
            note: None,
            on: false,
            kind,
            act: None,
            keycap: None,
            remove: None,
            keep: false,
            off: None,
        }
    }
    pub fn row(key: String, name: &str, note: Option<String>, on: bool, act: Option<Act>) -> Self {
        Self {
            note,
            on,
            act,
            ..Self::of(key, name, EntryKind::Row)
        }
    }
    pub fn separator() -> Self {
        Self::of(String::new(), "", EntryKind::Separator)
    }
    pub fn note(text: &str) -> Self {
        Self::of(String::new(), text, EntryKind::Note)
    }
    pub fn keycap(self, keycap: Option<String>) -> Self {
        Self { keycap, ..self }
    }
    pub fn removable(self, tip: &'static str, act: Act) -> Self {
        Self {
            remove: Some((tip, act)),
            ..self
        }
    }
    pub fn keeps_open(self) -> Self {
        Self { keep: true, ..self }
    }
    /// Whether the keyboard can land on it.
    pub fn choosable(&self) -> bool {
        self.act.is_some()
    }
}

fn menu_note_owned(text: String, t: &theme::Theme) -> AnyElement {
    div()
        // In line with the rows' names, past their check column.
        .pl(px(space::S1 + space::S2 + 14. + space::S2))
        .pr(px(space::S3))
        .py(px(space::S2))
        .ty(UI)
        .text_size(px(12.))
        .text_color(t.quiet)
        .child(text)
        .into_any_element()
}

#[allow(dead_code)]
fn menu_note(text: &'static str, t: &theme::Theme) -> AnyElement {
    div()
        .px(px(space::S3))
        .py(px(space::S2))
        .ty(UI)
        .text_size(px(12.))
        .text_color(t.quiet)
        .child(text)
        .into_any_element()
}

/// Elapsed time in words: 12s, 3m 05s, 1h 02m.
pub fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// How far the conversation fades out above the composer: enough that the
/// "Latest" pill sits on faded text, never on a line you're reading.
const FADE: f32 = 48.;

/// Effort levels, with what each is for.
const EFFORTS: [(Option<&str>, &str); 6] = [
    (None, "Whatever Claude Code picks for the model"),
    (Some("low"), "Fastest; for quick questions"),
    (Some("medium"), "Balanced"),
    (Some("high"), "Thinks harder before acting"),
    (Some("xhigh"), "For tricky problems"),
    (Some("max"), "Thinks longest; uses the most of your limit"),
];

/// An effort level as a menu names it.
pub fn effort_name(e: Option<&str>) -> &'static str {
    match e {
        None => "Default",
        Some("low") => "Low",
        Some("medium") => "Medium",
        Some("high") => "High",
        Some("xhigh") => "Extra high",
        Some("max") => "Max",
        Some(_) => "Custom",
    }
}

/// An effort level on its own, where the word "effort" has to say what it is.
pub fn effort_label(e: Option<&str>) -> &'static str {
    match e {
        None => "Default effort",
        Some("low") => "Low effort",
        Some("medium") => "Medium effort",
        Some("high") => "High effort",
        Some("xhigh") => "Extra high effort",
        Some("max") => "Max effort",
        Some(_) => "Custom effort",
    }
}

/// What each permission mode lets the agent do, in a line.
pub fn permission_note(p: Permission) -> &'static str {
    match p {
        Permission::Ask => "Ask before commands and file changes",
        Permission::Edits => "Auto-approve edits, ask before other actions",
        Permission::Plan => "Read and plan; change nothing",
        Permission::Full => "Allow commands and edits without prompts",
    }
}

/// The tinted card for anything waiting on you.
fn needs_card(t: &theme::Theme) -> Div {
    div()
        .relative()
        .p(px(space::S4))
        .rounded(px(radius::LG))
        .bg(t.wash(t.needs.0))
        .border_1()
        .border_color(t.edge(t.needs.0))
        .flex()
        .flex_col()
        .gap(px(space::S3))
}

fn needs_title(text: &str, t: &theme::Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::S2))
        .child(status_square(Status::Needs, t))
        .child(
            div()
                .ty(UI)
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.ink)
                .child(text.to_string()),
        )
}

/// What rode along with a message, shown above its text.
/// Whether level `k` of the tree carries on below row `i`: another row at
/// that level comes before the tree steps out past it.
fn continues(depths: &[usize], i: usize, k: usize) -> bool {
    for &d in &depths[i + 1..] {
        if d < k {
            return false;
        }
        if d == k {
            return true;
        }
    }
    false
}

impl Dent {
    /// A finished subagent's report under its steps: rendered, and folded
    /// to its first lines until opened.
    fn report(
        &mut self,
        s: &Session,
        ix: usize,
        call: &str,
        report: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let key = format!("report-{call}");
        let open = self.expanded.contains(&key);
        // Folded, it shows its first two blocks whole, never half a table.
        let all = self.markdown(report);
        let blocks: &[markdown::Block] = &all;
        let long = blocks.len() > 2;
        let blocks = if long && !open { &blocks[..2] } else { blocks };
        let fold = cx.listener(move |this, _, _, cx| {
            if !this.expanded.remove(&key) {
                this.expanded.insert(key.clone());
            }
            cx.notify();
        });
        let mut r = markdown::R {
            md: self.mdv.clone(),
            owner: cx.entity_id(),
            window,
            cx,
            scope: format!("{}/report-{call}", s.id),
            bg: t.surface,
            base: Some(std::path::PathBuf::from(&s.cwd)),
        };
        // Inside the tool, quieter than the answer: as an opened thought.
        let body = markdown::render_with(blocks, &t, false, true, &mut r);
        div()
            .flex_1()
            .min_w_0()
            .my(px(space::S1))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(div().ty(LABEL).text_color(t.quiet).child("REPORT"))
            .child(
                div()
                    .pl(px(space::S3))
                    .border_l_2()
                    .border_color(t.hairline)
                    .child(body),
            )
            .when(long, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("report-{ix}-{call}")))
                        .ty(UI)
                        .text_color(t.quiet)
                        .cursor_pointer()
                        .hover(|d| d.text_color(t.ink))
                        .on_press(fold)
                        .child(if open {
                            "Show less"
                        } else {
                            "Show full report"
                        }),
                )
            })
            .into_any_element()
    }
}

/// Where an attached picture is on disk, if it's a picture that's still there.
fn picture_path(shown: &str, cwd: &str) -> Option<std::path::PathBuf> {
    let ext = shown.rsplit('.').next()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return None;
    }
    let path = if let Some(rest) = shown.strip_prefix("~/") {
        std::path::PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else if shown.starts_with('/') {
        std::path::PathBuf::from(shown)
    } else {
        std::path::Path::new(cwd).join(shown)
    };
    crate::data::is_file(&path).then_some(path)
}

/// One attachment line of a sent message; a picture (`shot`, its place in
/// the message's pictures) shows as itself and opens large.
fn sent_attachment(
    line: &str,
    shot: Option<(&crate::lightbox::Set, usize)>,
    t: &theme::Theme,
) -> Div {
    let (place, what) = line.split_once(" · ").unwrap_or((line, ""));
    // A picture shows as itself, with its name under it.
    if let Some((pictures, at)) = shot {
        let path = pictures[at].path.clone();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(space::S1))
            .child(crate::lightbox::picture(
                pictures,
                at,
                div()
                    .max_w(px(320.))
                    .max_h(px(220.))
                    .rounded(px(radius::MD))
                    .border_1()
                    .border_color(t.hairline)
                    .overflow_hidden()
                    .child(
                        img(path.clone())
                            .max_w(px(320.))
                            .max_h(px(220.))
                            .object_fit(ObjectFit::Contain),
                    ),
            ))
            .child(div().ty(LABEL).text_color(t.quiet).child(name));
    }
    div()
        .flex()
        .items_start()
        .gap(px(space::S1H))
        .ty(UI)
        .text_color(t.quiet)
        .child(div().h(px(20.)).flex().items_center().child(icon(
            "corner-down-right",
            12.,
            t.faint,
        )))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().ty(LABEL).text_color(t.quiet).child(place.to_string()))
                .child(div().text_color(t.ink).child(what.to_string())),
        )
}

/// The words of a message you sent as drawn, and whether it's long enough
/// to fold: past 8 lines or 600 characters, folded unless `open`.
pub fn user_shown(body: &[&str], open: bool) -> (String, bool) {
    let full = body.join("\n");
    let long = body.len() > 8 || full.chars().count() > 600;
    if long && !open {
        let mut short: String = body[..body.len().min(8)]
            .join("\n")
            .chars()
            .take(600)
            .collect();
        short.push('…');
        (short, long)
    } else {
        (full, long)
    }
}

/// The parts of `text` that [`crate::ansi::clip`] kept, a line each, with
/// `…` where a line was cut short.
fn clipped(text: &str, kept: &[std::ops::Range<usize>]) -> String {
    let mut out = String::new();
    for (k, r) in kept.iter().enumerate() {
        if k > 0 {
            out.push('\n');
        }
        out.push_str(&text[r.clone()]);
        if cut_short(text, r) {
            out.push('…');
        }
    }
    out
}

/// A kept stretch stops before its line ends.
fn cut_short(text: &str, r: &std::ops::Range<usize>) -> bool {
    !text[r.end..].is_empty() && !text[r.end..].starts_with('\n') && text[r.end..].trim_end() != ""
}

/// Terminal output as the program coloured it, through the theme's 16
/// colours; what `kept` leaves out isn't drawn.
fn output_lines(
    id: String,
    text: &str,
    spans: &[(std::ops::Range<usize>, crate::ansi::Sgr)],
    kept: &[std::ops::Range<usize>],
    ink: Hsla,
    t: &crate::theme::Theme,
) -> Div {
    let run = |len: usize, st: Option<&crate::ansi::Sgr>| {
        let mut f = font(crate::theme::mono());
        let mut color = ink;
        if let Some(st) = st {
            if st.bold {
                f.weight = FontWeight::BOLD;
            }
            if st.italic {
                f.style = FontStyle::Italic;
            }
            color = match st.fg {
                Some(crate::ansi::Colour::Indexed(i)) => crate::shell::indexed(t, i),
                Some(crate::ansi::Colour::Rgb(r, g, b)) => {
                    gpui_kit::rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into()
                }
                None => ink,
            };
            if st.dim {
                color = color.opacity(0.6);
            }
        }
        TextRun {
            len,
            font: f,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    };
    let mut out = String::new();
    let mut runs = Vec::new();
    // Spans are in order and don't overlap; walk them alongside.
    let mut j = 0;
    for (k, r) in kept.iter().enumerate() {
        if k > 0 {
            out.push('\n');
            runs.push(run(1, None));
        }
        let mut at = r.start;
        while j < spans.len() && spans[j].0.end <= r.start {
            j += 1;
        }
        let mut i = j;
        while i < spans.len() && spans[i].0.start < r.end {
            let (s0, s1) = (spans[i].0.start.max(r.start), spans[i].0.end.min(r.end));
            if s0 > at {
                runs.push(run(s0 - at, None));
            }
            runs.push(run(s1 - s0, Some(&spans[i].1)));
            at = s1;
            i += 1;
        }
        if at < r.end {
            runs.push(run(r.end - at, None));
        }
        out.push_str(&text[r.clone()]);
        if cut_short(text, r) {
            out.push('…');
            runs.push(run('…'.len_utf8(), None));
        }
    }
    div()
        .min_w_0()
        .whitespace_normal()
        .child(crate::select::styled(id, out, runs, t))
}

/// Lines of code or output that select and copy; with `diff`, removed
/// ("- ") and added ("+ ") lines take the diff's colours.
pub fn code_lines(id: String, text: String, diff: bool, ink: Hsla, t: &crate::theme::Theme) -> Div {
    // A trailing line break would leave a blank line at the bottom.
    let text = text.trim_end_matches('\n').to_string();
    let mut runs = Vec::new();
    for l in text.split_inclusive('\n') {
        let color = if diff && l.starts_with("- ") {
            t.needs.0
        } else if diff && l.starts_with("+ ") {
            t.done.0
        } else {
            ink
        };
        runs.push(TextRun {
            len: l.len(),
            font: font(crate::theme::mono()),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    div()
        .min_w_0()
        .whitespace_normal()
        .child(crate::select::styled(id, text, runs, t))
}

/// Lines added and removed in `paths`, for the turn whose edits end at
/// item `ix`. The counts are the files as they stand now, so they only
/// describe that turn if no later turn touched the same files; otherwise
/// none are given.
fn turn_counts(s: &Session, ix: usize, paths: &[String]) -> (usize, usize) {
    let later = s.items[ix + 1..].iter().any(|i| match i {
        Item::Changed { paths: p } => p.iter().any(|x| paths.contains(x)),
        Item::Made {
            review: Some(p), ..
        } => paths.contains(p),
        _ => false,
    });
    if later {
        return (0, 0);
    }
    s.diff
        .iter()
        .filter(|f| paths.contains(&f.path))
        .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed))
}

/// How a composer's footer fits its room: which words each chip keeps,
/// and whether the context reading and the hint show.
#[derive(Clone)]
pub struct Fit {
    pub labels: [String; 3],
    pub context: bool,
    pub hint: bool,
    /// False when even the tightest layout is too wide: the row wraps
    /// rather than letting chips run under the send button.
    pub fits: bool,
}

/// Fit chips (each with its full and short label; an empty short label is
/// the icon alone) and the optional readings into `room` points. Things give
/// way one at a time: the hint, each chip's words in turn (model first,
/// permission last), then the context gauge, which opens the meter and so
/// stays longest.
pub fn fit_footer(room: f32, chips: [(String, String); 3], context: bool, hint: &str) -> Fit {
    // Widths from the label lengths: the interface face runs about 7 pt a
    // character; a chip adds its icon, chevron and padding.
    let text = |t: &str| t.chars().count() as f32 * 7.2;
    let chip = |t: &str| if t.is_empty() { 44. } else { 52. + text(t) };
    let gap = space::S1;
    let ctx_w = text("context 9.9%") + space::S2;
    let hint_w = text(hint) + space::S2;
    // The steps, least loss first.
    let mut steps: Vec<([bool; 3], [bool; 3], bool, bool)> = Vec::new();
    // (short?, icon only?) per chip, context, hint
    let full = [false; 3];
    steps.push((full, full, true, true));
    steps.push((full, full, true, false));
    steps.push(([true, false, false], full, true, false));
    steps.push(([true, false, true], full, true, false));
    steps.push(([true, false, true], [false, false, true], true, false));
    steps.push(([true, true, true], [false, true, true], true, false));
    steps.push(([true, true, true], [false, true, true], false, false));
    for (short, icon, ctx, h) in &steps {
        let labels: [String; 3] = std::array::from_fn(|i| {
            if icon[i] {
                String::new()
            } else if short[i] {
                chips[i].1.clone()
            } else {
                chips[i].0.clone()
            }
        });
        let mut w: f32 = labels.iter().map(|l| chip(l)).sum::<f32>() + gap * 2.;
        if *ctx && context {
            w += ctx_w;
        }
        if *h {
            w += hint_w;
        }
        if w <= room {
            return Fit {
                labels,
                context: *ctx && context,
                hint: *h,
                fits: true,
            };
        }
    }
    let (short, icon, ..) = steps[steps.len() - 1];
    Fit {
        labels: std::array::from_fn(|i| {
            if icon[i] {
                String::new()
            } else if short[i] {
                chips[i].1.clone()
            } else {
                chips[i].0.clone()
            }
        }),
        context: false,
        hint: false,
        fits: false,
    }
}

/// The line under an approval or a plan saying how to answer by key: the
/// keys themselves while its card has the keyboard, else how to get there.
fn ask_keys(keyed: bool, on: &'static str, off: &'static str, t: &theme::Theme) -> Div {
    div()
        .ty(LABEL)
        .text_color(if keyed { t.ink } else { t.quiet })
        .child(if keyed { on } else { off })
}

/// The changes line that belongs to the reply at `ix`: the turn's Changed
/// item, when nothing but its end lies between them.
pub fn change_after(items: &[Item], ix: usize) -> Option<usize> {
    turn_end_after(items, ix)?;
    for (j, i) in items.iter().enumerate().skip(ix + 1) {
        match i {
            Item::Changed { .. } => return Some(j),
            Item::TurnEnd { .. } | Item::Note { .. } => {}
            _ => return None,
        }
    }
    None
}

/// Whether the Changed item at `ix` shows in the footer of the reply
/// above it, rather than on a line of its own.
fn folded_change(items: &[Item], ix: usize) -> bool {
    matches!(items.get(ix), Some(Item::Changed { .. }))
        && items[..ix]
            .iter()
            .rposition(|i| matches!(i, Item::Text(_)))
            .is_some_and(|r| change_after(items, r) == Some(ix))
}

/// Whether the reply at `ix` is a turn's answer: nothing but notes follow it
/// before your next message, and its turn has ended. Everything else said in
/// a turn is narration of the work.
pub fn is_answer(items: &[Item], ix: usize, running: bool) -> bool {
    // While the agent works, the turn it's in has no answer yet, whatever
    // you queue or steer into it meanwhile.
    let open = running
        && !items[ix + 1..]
            .iter()
            .any(|i| matches!(i, Item::TurnEnd { .. }));
    for i in items.iter().skip(ix + 1) {
        match i {
            Item::TurnEnd { .. } => return true,
            Item::User { .. } | Item::Queued { .. } => return !open,
            Item::Text(_) | Item::Tools { .. } | Item::Thinking(_) => return false,
            _ => {}
        }
    }
    !running
}

/// For the last reply of a turn that has finished: when it finished, how
/// long it took (ms), and whether it's the session's latest turn.
pub fn turn_end_after(items: &[Item], ix: usize) -> Option<(u64, u64, bool)> {
    if !matches!(items.get(ix), Some(Item::Text(_))) {
        return None;
    }
    for (j, i) in items.iter().enumerate().skip(ix + 1) {
        match i {
            Item::Text(_) | Item::User { .. } | Item::Queued { .. } => return None,
            Item::TurnEnd { at, took } => {
                let latest = !items[j + 1..]
                    .iter()
                    .any(|i| matches!(i, Item::TurnEnd { .. } | Item::User { .. }));
                return Some((*at, *took, latest));
            }
            _ => {}
        }
    }
    None
}

/// A tool line: its verb quiet, what it worked on a step darker, both to
/// ink while `hover` (the row's group) is under the pointer. Too long for
/// the row, the object gives way first.
fn tool_text(text: &str, hover: Option<SharedString>, t: &theme::Theme) -> Div {
    let n = dent_client_core::timeline::verb_len(text);
    let (verb, rest) = text.split_at(n);
    let part = |d: Div, ink: Hsla| match &hover {
        Some(g) => d
            .text_color(ink)
            .group_hover(g.clone(), |d| d.text_color(t.ink)),
        None => d.text_color(ink),
    };
    div()
        .flex()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .when(!verb.is_empty(), |d| {
            d.child(part(div().flex_none(), t.quiet).child(verb.to_string()))
        })
        .when(!rest.trim().is_empty(), |d| {
            d.child(part(div().min_w_0().truncate(), t.secondary()).child(rest.to_string()))
        })
}

/// A chip held at its natural width when `whole`, so a wrapping row moves
/// it to the next line rather than cutting its label short.
fn keep(whole: bool, chip: AnyElement) -> AnyElement {
    if whole {
        div().flex_none().child(chip).into_any_element()
    } else {
        chip
    }
}

#[cfg(test)]
mod answer_tests {
    use super::is_answer;
    use crate::data::Item;

    #[test]
    fn narration_is_never_the_answer_while_its_turn_runs() {
        let text = || Item::Text("words".into());
        let end = Item::TurnEnd { at: 0, took: 0 };
        // A queued message after the latest words: still working, not the answer.
        let q = vec![
            text(),
            Item::Queued {
                id: "q".into(),
                text: "next".into(),
                attachments: Vec::new(),
            },
        ];
        assert!(!is_answer(&q, 0, true));
        // The turn ended: the last words are its answer, narration before isn't.
        let done = vec![text(), Item::Thinking(String::new()), text(), end.clone()];
        assert!(is_answer(&done, 2, false));
        assert!(!is_answer(&done, 0, false));
        // An older turn's answer stays one while a new turn runs.
        let older = vec![
            text(),
            end,
            Item::User {
                id: "u".into(),
                text: "more".into(),
                at: 0,
            },
            text(),
        ];
        assert!(is_answer(&older, 0, true));
        assert!(!is_answer(&older, 3, true));
    }
}
