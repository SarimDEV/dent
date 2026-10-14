//! The main column: a session's conversation and composer, or the new-session
//! page where you start by typing.

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;

use crate::ui::RevealOnHover;

use crate::anim;
use crate::app::{Dent, Menu, Tab};
use dent_protocol::Permission;

use crate::data::{Item, Kind, Session};
use crate::ds::{
    CODE, DISPLAY, DISPLAY_COMPACT, LABEL, READING, Typed, UI, motion, radius, size, space,
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
                    .on_click(cx.listener(move |this, _, _, cx| this.resume(&sid, cx))),
            );
        }
        {
            let sid = id.clone();
            let fork = if tight {
                icon_button("fork", "git-fork", &t).tooltip(tip(
                    "Fork: a new session that carries on from here",
                    None,
                    t,
                ))
            } else {
                button("fork", "Fork", false, &t).tooltip(tip(
                    "A new session that carries on from here",
                    None,
                    t,
                ))
            };
            actions = actions.child(
                fork.relative()
                    .on_click(cx.listener(move |this, _, _, cx| this.fork(&sid, None, cx)))
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
                    .tooltip(tip("Close", Some("⌘ W"), t))
                    .on_click(cx.listener(move |this, _, window, cx| {
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
                    .on_click(cx.listener(|this, _, window, cx| this.open_shell(window, cx)))
                    .tooltip(tip("Your terminal", Some("⌘ 3"), t))
                    .child(probe(&self.probes, "terminal")),
            );
        }
        if !s.items.is_empty() {
            actions = actions.child(
                icon_button("side-chat", "message-circle-question-mark", &t)
                    .relative()
                    .on_click(cx.listener(|this, _, window, cx| this.open_side_chat(window, cx)))
                    .tooltip(tip("Ask on the side", Some("⌘ ;"), t))
                    .child(probe(&self.probes, "side-chat")),
            );
        }
        if has_viewer {
            let open = self.panel_open;
            actions = actions.child(
                icon_button(
                    "panel",
                    if open {
                        "panel-right-close"
                    } else {
                        "panel-right-open"
                    },
                    &t,
                )
                .when(open, |d| d.bg(t.selected()))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.panel_open = !this.panel_open;
                    this.end_pick();
                    cx.notify();
                }))
                .tooltip(tip(
                    if open { "Hide viewer" } else { "Show viewer" },
                    Some("⌘ \\"),
                    t,
                )),
            );
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar_forced = true;
                            this.panel_open = false;
                            cx.notify();
                        }))
                        .tooltip(tip("Show sessions", None, t)),
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
                                    .tooltip(tip_owned(s.title.clone(), t))
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

    /// A tool call opened up: the command or edit, what came back, and the
    /// way to its file. All of it selects and copies.
    fn call_detail(
        &self,
        s: &Session,
        key: &str,
        d: &crate::data::CallDetail,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.t;
        const MAX: usize = 60;
        // Long text shows its start; the rest is a line saying how much.
        let clip = |text: &str| -> (String, usize) {
            let n = text.lines().count();
            if n <= MAX {
                (text.trim_end().to_string(), 0)
            } else {
                (
                    text.lines().take(MAX).collect::<Vec<_>>().join("\n"),
                    n - MAX,
                )
            }
        };
        let block = |id: String, text: String, ink: Hsla| code_lines(id, text, d.edits, ink, &t);
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .p(px(space::S3))
            .rounded(px(radius::MD))
            .bg(t.raised)
            .border_1()
            .border_color(t.hairline)
            .ty(CODE)
            .text_color(t.ink);
        if !d.input.trim().is_empty() {
            let (text, more) = clip(&d.input);
            col = col
                .child(block(format!("call-in-{key}"), text, t.ink))
                .when(more > 0, |c| {
                    c.child(
                        div()
                            .text_color(t.faint)
                            .child(format!("… {more} more lines")),
                    )
                });
        }
        if let Some((failed, out)) = &d.output
            && !out.trim().is_empty()
            && !d.edits
        {
            let (text, more) = clip(out);
            col = col
                .child(
                    div()
                        .ty(LABEL)
                        .text_color(if *failed { t.needs.0 } else { t.faint })
                        .child(if *failed { "Failed" } else { "Output" }),
                )
                .child(block(format!("call-out-{key}"), text, t.quiet))
                .when(more > 0, |c| {
                    c.child(
                        div()
                            .text_color(t.faint)
                            .child(format!("… {more} more lines")),
                    )
                });
        }
        // Its file: shown in dent when it can be, and its changes.
        let in_diff = d.path.as_ref().and_then(|p| {
            let rel = crate::bridge::relative(std::path::Path::new(&s.cwd), p);
            s.diff.iter().any(|f| f.path == rel).then_some(rel)
        });
        let openable = d
            .path
            .as_ref()
            .filter(|p| std::path::Path::new(p).is_file())
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
                            .on_click(cx.listener(move |this, _, _, cx| this.open_file(&p, cx))),
                        )
                    })
                    .when_some(in_diff.filter(|_| d.edits), |c, rel| {
                        c.child(
                            ghost(
                                SharedString::from(format!("call-review-{key}")),
                                "Review changes",
                                &t,
                            )
                            .on_click(
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
                            .tooltip(tip(label, Some(keys), t))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.jump_to_mine(up, cx);
                            }))
                    } else {
                        d.cursor_not_allowed().tooltip(tip(off, None, t))
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

    fn chat(&mut self, s: &Session, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
                            .pb(px(if ix + 1 == count {
                                space::S8
                            } else {
                                space::S4
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
        let transcript = div().relative().flex_1().min_h_0().child(transcript);
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.transcript.set_follow_mode(FollowMode::Tail);
                            this.transcript.scroll_to_end();
                            cx.notify();
                        }))
                        .child(icon("arrow-down", 13., t.quiet))
                        .child("Latest")
                        .child(probe(&self.probes, "to-latest")),
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
                        .child(button("reopen", "Reopen", true, &t).on_click(cx.listener(move |this, _, window, cx| {
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

    /// The last line while the agent works: what it's doing, and for how long.
    fn working_line(&self, s: &Session) -> AnyElement {
        let t = self.t;
        let since = self.working_since.get(&s.id).copied();
        div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .ty(UI)
            .text_color(t.quiet)
            .child(status_square(Status::Working, &t))
            .child(div().text_color(t.ink).child(match since {
                Some(at) if at.elapsed().as_secs() >= 1 => {
                    format!("Working for {}", duration(at.elapsed().as_secs()))
                }
                Some(_) => "Working".into(),
                None => "Working".into(),
            }))
            .child(div().text_color(t.faint).child("·"))
            .child(div().flex_1().min_w_0().truncate().child(s.detail.clone()))
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
            Item::User { id: msg, text, at } => {
                let (atts, body): (Vec<&str>, Vec<&str>) =
                    text.lines().partition(|l| l.starts_with("› "));
                let asking = self
                    .rewinding
                    .as_ref()
                    .is_some_and(|(sid, m)| *sid == s.id && m == msg);
                let busy = matches!(s.status, Status::Working | Status::Needs);
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
                            .px(px(space::S4))
                            .py(px(space::S2 + 2.))
                            .rounded(px(radius::LG))
                            .bg(t.raised)
                            .border_1()
                            .border_color(t.hairline)
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .children(
                                atts.iter().map(|a| {
                                    sent_attachment(a.trim_start_matches("› "), &s.cwd, &t)
                                }),
                            )
                            .when(!body.is_empty(), |d| {
                                // Long messages fold, so the reply stays in view.
                                let full = body.join("\n");
                                let long = body.len() > 8 || full.chars().count() > 600;
                                let open = self.expanded.contains(msg);
                                let shown = if long && !open {
                                    let mut short: String = body[..body.len().min(8)]
                                        .join("\n")
                                        .chars()
                                        .take(600)
                                        .collect();
                                    short.push('…');
                                    short
                                } else {
                                    full
                                };
                                let key = msg.clone();
                                // A slash command reads as one.
                                let command = shown.starts_with('/')
                                    && !shown.contains('\n')
                                    && shown.len() < 80;
                                d.child(
                                    div()
                                        .ty(if command { CODE } else { READING })
                                        .text_color(t.ink)
                                        .child(crate::select::text(
                                            SharedString::from(format!("user-text-{ix}")),
                                            shown,
                                            &t,
                                        )),
                                )
                                .when(long, |d| {
                                    d.child(
                                        div()
                                            .id(SharedString::from(format!("fold-{ix}")))
                                            .ty(UI)
                                            .text_color(t.quiet)
                                            .cursor_pointer()
                                            .hover(|d| d.text_color(t.ink))
                                            .on_click(cx.listener(move |this, _, _, cx| {
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
                            d.child(div().ty(UI).text_color(t.quiet).child(if busy {
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
                                            .on_click(cx.listener(move |this, _, window, cx| {
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
                                        .on_click(
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
                                            .on_click(cx.listener(move |this, _, window, cx| {
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
                                    // Shown on hover in the gap below the message,
                                    // so it doesn't push the reply away.
                                    .h(px(size::CONTROL))
                                    .mb(px(-(size::CONTROL + space::S1)))
                                    .reveal_on_hover(group.clone())
                                    // When you sent it, beside what you can do with it.
                                    .when(*at > 0, |d| {
                                        d.child(
                                            div()
                                                .px(px(space::S2))
                                                .ty(LABEL)
                                                .text_color(t.faint)
                                                .child(crate::data::clock(*at)),
                                        )
                                    })
                                    .child(
                                        ghost(
                                            SharedString::from(format!("fork-from-{ix}")),
                                            "Fork from here",
                                            &t,
                                        )
                                        .relative()
                                        .on_click(cx.listener(move |this, _, _, cx| {
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
                                        .on_click(cx.listener(move |this, _, _, cx| {
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
                let births = self
                    .births
                    .get(&s.id)
                    .filter(|(i, _)| *i == ix)
                    .map(|(_, b)| b.clone());
                let blocks = self.markdown(src);
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
                            .ty(READING)
                            .text_color(t.ink)
                            .child(inline.element(
                                &format!("{}/{ix}/0", s.id),
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
                            scope: format!("{}/{ix}", s.id),
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
            Item::Tools { summary, lines } => {
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
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(SharedString::from(format!("tools-{ix}")))
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .ty(CODE)
                            .text_color(t.quiet)
                            .when(more, |d| {
                                d.cursor_pointer().hover(|d| d.text_color(t.ink)).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        let set = if lone {
                                            &mut this.open_calls
                                        } else {
                                            &mut this.open_tools
                                        };
                                        if !set.remove(&key) {
                                            set.insert(key.clone());
                                        }
                                        cx.notify();
                                    }),
                                )
                            })
                            // A lone call sits flush with the text above.
                            .when(more, |d| {
                                d.child(
                                    div()
                                        .w(px(space::S4))
                                        .flex_none()
                                        .flex()
                                        .justify_center()
                                        .child(icon(
                                            if open {
                                                "chevron-down"
                                            } else {
                                                "chevron-right"
                                            },
                                            12.,
                                            t.quiet.opacity(0.7),
                                        )),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(summary.clone()),
                            ),
                    )
                    .when(open && lone, |d| {
                        let detail = lines[0].detail.clone();
                        d.children(detail.map(|dt| {
                            self.call_detail(s, &format!("{ix}-0"), &dt, cx)
                                .ml(px(space::S4))
                                .mt(px(space::S1))
                        }))
                    })
                    .when(open && !lone, |d| {
                        let depths: Vec<usize> = lines.iter().map(|l| l.depth).collect();
                        let rows: Vec<AnyElement> = lines
                            .iter()
                            .enumerate()
                            .map(|(i, l)| {
                                // Each level gets a column: a rule while that
                                // level carries on below, then this row's branch.
                                // The rails are drawn, so they join from row to
                                // row whatever each row's height.
                                let mut row = div().flex().ty(CODE).text_color(t.quiet);
                                let rail = t.quiet.opacity(0.35);
                                for k in 0..=l.depth {
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
                                                                d.bottom_0()
                                                            } else {
                                                                d.h(px(size::LINE / 2.))
                                                            }
                                                        }),
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
                                match &l.report {
                                    None => {
                                        let call = l.id.clone();
                                        let opened = self.open_calls.contains(&call);
                                        let detail = l.detail.clone().filter(|_| opened);
                                        let line = div()
                                            .id(SharedString::from(format!("tool-line-{ix}-{i}")))
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .when(l.detail.is_some(), |d| {
                                                d.cursor_pointer()
                                                    .hover(|d| d.text_color(t.ink))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        if !this.open_calls.remove(&call) {
                                                            this.open_calls.insert(call.clone());
                                                        }
                                                        cx.notify();
                                                    }))
                                            })
                                            .child(l.text.clone());
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
                                                        &format!("{ix}-{i}"),
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
                        d.children(rows)
                    })
                    .into_any_element()
            }
            Item::Thinking(text) => {
                let key = crate::app::thought_key(text);
                let open = self.open_calls.contains(&key);
                let words = text.split_whitespace().count();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .id(SharedString::from(format!("thought-{ix}")))
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .ty(CODE)
                            .text_color(t.quiet)
                            .cursor_pointer()
                            .hover(|d| d.text_color(t.ink))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.open_calls.remove(&key) {
                                    this.open_calls.insert(key.clone());
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w(px(space::S4))
                                    .flex_none()
                                    .flex()
                                    .justify_center()
                                    .child(icon(
                                        if open {
                                            "chevron-down"
                                        } else {
                                            "chevron-right"
                                        },
                                        12.,
                                        t.quiet.opacity(0.7),
                                    )),
                            )
                            .child(format!("Thought · {words} words")),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .ml(px(space::S4))
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
            Item::Made { art, version } => self.made_card(s, art, *version, cx),
            Item::Approval {
                id: ask,
                title,
                command,
                why,
                diff,
            } => {
                let (a, a2, d) = (s.id.clone(), s.id.clone(), s.id.clone());
                let (k1, k2, k3) = (ask.clone(), ask.clone(), ask.clone());
                needs_card(&t)
                    .child(needs_title(title, &t))
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
                    .when(!why.is_empty(), |c| {
                        c.child(div().ty(UI).text_color(t.quiet).child(why.clone()))
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a, Some(&k1), false, cx)
                                    }))
                                    .child(probe(&self.probes, "allow")),
                            )
                            .child(
                                button("allow-session", "Allow for this session", false, &t)
                                    .relative()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a2, Some(&k2), true, cx)
                                    }))
                                    .child(probe(&self.probes, "allow-session")),
                            )
                            .child(
                                button("deny", "Deny", false, &t)
                                    .relative()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.deny(&d, Some(&k3), window, cx)
                                    }))
                                    .child(probe(&self.probes, "deny")),
                            ),
                    )
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
                    scope: format!("{}/{ix}/plan", s.id),
                    bg: t.raised,
                    base: Some(std::path::PathBuf::from(&s.cwd)),
                };
                let plan_view = markdown::render(blocks.as_slice(), &t, false, &mut r);
                needs_card(&t)
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a, Some(&k1), false, cx)
                                    }))
                                    .child(probe(&self.probes, "plan-go")),
                            )
                            .child(
                                button("plan-keep", "Keep planning", false, &t)
                                    .relative()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.deny(&d, Some(&k3), window, cx)
                                    }))
                                    .child(probe(&self.probes, "plan-keep")),
                            ),
                    )
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child("Typed text is sent as what to change"),
                    )
                    .into_any_element()
            }
            Item::Question { id: ask, questions } => {
                let picked = self.picks.get(ask).cloned().unwrap_or_default();
                let many = questions.len() > 1;
                needs_card(&t)
                    .children(questions.iter().enumerate().map(|(qi, q)| {
                        let chosen = picked.get(qi).cloned().flatten();
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
                                    let on = chosen.as_deref() == Some(label.as_str());
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
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.answer(&sid, Some(&k), qi, o2.clone(), cx)
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
                    }))
                    .child(div().ty(LABEL).text_color(t.faint).child(if many {
                        "Answer each one, or type an answer in the box"
                    } else {
                        "Or type your own answer in the box"
                    }))
                    .into_any_element()
            }
            Item::Note { icon: name, text } => div()
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
                .child(div().flex_1().min_w_0().child(crate::select::text(
                    SharedString::from(format!("note-{ix}")),
                    text.clone(),
                    &t,
                )))
                // The last thing that went wrong can be tried again.
                .when(
                    *name == "circle-alert"
                        && s.status != Status::Working
                        && !s.items[ix + 1..]
                            .iter()
                            .any(|i| matches!(i, Item::User { .. })),
                    |d| {
                        let sid = s.id.clone();
                        d.child(
                            ghost(SharedString::from(format!("retry-{ix}")), "Try again", &t)
                                .relative()
                                .on_click(cx.listener(move |this, _, _, cx| this.retry(&sid, cx)))
                                .child(probe(&self.probes, format!("retry-{ix}"))),
                        )
                    },
                )
                .into_any_element(),
            Item::Changed { paths } => {
                // Counts are the files' changes as they stand now, which only
                // describe this turn if it's the latest to touch them.
                let latest = !s.items[ix + 1..]
                    .iter()
                    .any(|i| matches!(i, Item::Changed { paths: p } if p.iter().any(|x| paths.contains(x))));
                let (added, removed) = if latest {
                    s.diff
                        .iter()
                        .filter(|f| paths.contains(&f.path))
                        .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed))
                } else {
                    (0, 0)
                };
                let n = paths.len();
                let (sid, first) = (s.id.clone(), paths[0].clone());
                div()
                    .id(SharedString::from(format!("changed-{ix}")))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .ty(CODE)
                    .text_color(t.quiet)
                    .cursor_pointer()
                    .hover(|d| d.text_color(t.ink))
                    // Opens Changes at the file this turn changed.
                    .on_click(cx.listener(move |this, _, _, cx| this.review(&sid, &first, cx)))
                    .child(icon("git-compare", 13., t.quiet))
                    .child(if n == 1 {
                        format!("Changed {}", paths[0])
                    } else {
                        format!("Changed {n} files")
                    })
                    .when(added + removed > 0, |d| {
                        d.child(
                            div()
                                .text_color(t.faint)
                                .child(format!("+{added} −{removed}")),
                        )
                    })
                    .child(div().ty(UI).text_color(t.ink).child("Review"))
                    .child(probe(&self.probes, format!("changed-{ix}")))
                    .into_any_element()
            }
            Item::Queued { id: msg, text } => {
                let (sid, msg, back) = (s.id.clone(), msg.clone(), text.clone());
                let (sid2, msg2, now) = (s.id.clone(), msg.clone(), text.clone());
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
                            .child(text.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .ty(LABEL)
                                    .text_color(t.faint)
                                    .child("Queued · sends when this reply finishes"),
                            )
                            .child(
                                ghost(SharedString::from(format!("unqueue-{ix}")), "Edit", &t)
                                    .relative()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.unqueue(&sid, &msg, &back, window, cx)
                                    }))
                                    .child(probe(&self.probes, format!("unqueue-{ix}"))),
                            )
                            .child(
                                ghost(SharedString::from(format!("steer-{ix}")), "Send now", &t)
                                    .relative()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.steer_queued(&sid2, &msg2, &now, cx)
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
        art: &'static str,
        version: usize,
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
                .on_click(cx.listener(move |this, _, _, cx| this.open_art(art, Some(version), cx)))
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
                .child(probe(&self.probes, format!("made-{art}-{version}")))
                .into_any_element();
        }
        let note = if a.kind == Kind::Server {
            format!(
                "{} · running on {}",
                a.title.trim_start_matches("Dev server · "),
                s.machine
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
            .on_click(cx.listener(move |this, _, _, cx| this.open_art(art, Some(version), cx)))
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
            .child(
                div()
                    .ty(UI)
                    .text_color(t.quiet)
                    .child(if showing { "Showing" } else { "Open" }),
            )
            .child(probe(&self.probes, format!("made-{art}-{version}")))
            .into_any_element()
    }

    fn composer_box(&mut self, s: &Session, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.t;
        let text = self.composer.read(cx).value();
        let empty = text.trim().is_empty() && self.attachments.is_empty();
        let working = matches!(s.status, Status::Working | Status::Needs);
        let settling = self
            .just_sent
            .is_some_and(|at| at.elapsed().as_secs_f32() < 1.);
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
                .when(!loud, |d| {
                    d.border_1()
                        .border_color(t.ink.opacity(0.16))
                        .hover(|d| d.bg(t.hover()))
                        .active(|d| d.bg(t.press()))
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.stop(&id, window, cx)))
                .tooltip(tip("Stop", Some("⌘ ."), t))
                .child(
                    div()
                        .size(px(9.))
                        .bg(if loud { t.surface } else { t.quiet }),
                )
                .child(probe(&self.probes, "send"))
                .into_any_element()
        } else {
            let ready = if settling {
                Ready::No("Sending…")
            } else if empty {
                Ready::No("Type a message first")
            } else {
                Ready::Yes
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
                        d.on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                            .tooltip(tip("Send", Some("↩"), t))
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
            match s.items.iter().rev().find_map(|i| match i {
                Item::Question { .. } => Some("↩ answer"),
                Item::Approval { .. } => Some("↩ deny with this reason"),
                Item::Plan { .. } => Some("↩ ask for changes"),
                _ => None,
            }) {
                Some(h) => h,
                None => "↩ send",
            }
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
            - space::S2;
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
        let chips: Vec<AnyElement> = self
            .attachments
            .clone()
            .into_iter()
            .enumerate()
            .map(|(i, a)| {
                div()
                    .id(SharedString::from(format!("att-{i}")))
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .max_w(px(420.))
                    .h(px(size::CONTROL_SM + 2.))
                    .pl(px(space::S2))
                    .pr(px(space::S1))
                    .rounded(px(radius::SM))
                    .bg(t.selected())
                    // A picture shows itself, small.
                    .child(match &a {
                        crate::app::Attachment::File { path }
                            if Kind::of(path) == Some(Kind::Image) =>
                        {
                            div()
                                .size(px(16.))
                                .flex_none()
                                .rounded(px(3.))
                                .overflow_hidden()
                                .child(
                                    img(std::path::PathBuf::from(path))
                                        .size_full()
                                        .object_fit(ObjectFit::Cover),
                                )
                                .into_any_element()
                        }
                        _ => icon(a.icon(), 12., t.quiet).into_any_element(),
                    })
                    .child(div().truncate().ty(UI).text_color(t.ink).child(a.label()))
                    .tooltip(crate::ui::tip_owned(
                        match &a {
                            crate::app::Attachment::File { path } => path.replacen(&home(), "~", 1),
                            _ => a.label(),
                        },
                        t,
                    ))
                    .child(
                        div()
                            .id(SharedString::from(format!("att-x-{i}")))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .hover(|d| d.bg(t.press()))
                            .on_click(cx.listener(move |this, _, _, cx| {
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
        div()
            .w_full()
            .max_w(px(size::COLUMN + space::S8 * 2.))
            .mx_auto()
            .px(px(space::S8))
            .pb(px(space::S4))
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
                    .children(self.plan_strip(s, cx))
                    .when(!chips.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap(px(space::S1H)).children(chips))
                    })
                    .child(
                        div()
                            .ty(READING)
                            .text_color(t.ink)
                            .child(Textarea::new(&self.composer)),
                    )
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
                            // always shows.
                            .child(self.chip(Menu::SessionModel, "claude", &labels[0], false, cx))
                            .child(self.chip(
                                Menu::SessionPermission,
                                "shield",
                                &labels[1],
                                false,
                                cx,
                            ))
                            .child(self.chip(Menu::SessionEffort, "gauge", &labels[2], false, cx))
                            .child(div().flex_1())
                            .when_some(s.context.filter(|_| fit.context), |d, (used, max)| {
                                let max = max.unwrap_or(200_000).max(1);
                                let pct = used as f32 / max as f32 * 100.;
                                d.child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .flex_shrink(2.)
                                        .ty(LABEL)
                                        .text_color(if pct > 90. { t.needs.0 } else { t.faint })
                                        .child(if pct < 10. {
                                            format!("context {pct:.1}% · ")
                                        } else {
                                            format!("context {pct:.0}% · ")
                                        }),
                                )
                            })
                            .when(fit.hint, |d| d.child(hint(hint_text, &t)))
                            .child(action),
                    )
                    .child(probe(&self.probes, "composer"))
                    .children(self.slash_menu(cx)),
            )
    }

    /// The agent's plan, folded to one line above the composer.
    fn plan_strip(&self, s: &Session, cx: &mut Context<Self>) -> Option<AnyElement> {
        if s.todos.is_empty()
            || s.todos.iter().all(|(st, ..)| st == "completed") && s.status != Status::Working
        {
            return None;
        }
        let t = self.t;
        let done = s.todos.iter().filter(|(st, ..)| st == "completed").count();
        let now = s
            .todos
            .iter()
            .find(|(st, ..)| st == "in_progress")
            .map(|(_, what, doing)| {
                if doing.is_empty() {
                    what.clone()
                } else {
                    doing.clone()
                }
            });
        let open = self.plan_open;
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .pb(px(space::S1))
                .border_b_1()
                .border_color(t.hairline)
                .child(
                    div()
                        .id("plan-strip")
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.plan_open = !this.plan_open;
                            cx.notify();
                        }))
                        .child(icon("list-checks", 13., t.quiet))
                        .child(div().ty(UI).text_color(t.quiet).child({
                            let total = s.todos.len();
                            match s.todos.iter().position(|(st, ..)| st == "in_progress") {
                                Some(i) if !open => format!("Step {} of {total}", i + 1),
                                _ => format!("Plan · {done} of {total} done"),
                            }
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
                        .child(icon(
                            if open { "chevron-down" } else { "chevron-up" },
                            11.,
                            t.faint,
                        )),
                )
                .when(open, |d| {
                    d.children(s.todos.iter().map(|(st, what, _)| {
                        let ink = match st.as_str() {
                            "completed" => t.faint,
                            "in_progress" => t.ink,
                            _ => t.quiet,
                        };
                        // Done is a check; the current step a filled dot; the
                        // rest an outline.
                        let mark = match st.as_str() {
                            "completed" => icon("check", 12., t.faint).into_any_element(),
                            "in_progress" => div()
                                .size(px(7.))
                                .rounded_full()
                                .bg(t.ink)
                                .into_any_element(),
                            _ => div()
                                .size(px(7.))
                                .rounded_full()
                                .border_1()
                                .border_color(t.quiet)
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

    /// Commands matching a `/word` in the composer, above it.
    fn slash_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.t;
        let (mention, hits) = self.completions(cx);
        if hits.is_empty() {
            return None;
        }
        let at = self.slash_ix.min(hits.len() - 1);
        let rows: Vec<AnyElement> = hits
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let name = c.name.clone();
                div()
                    .id(SharedString::from(format!("slash-{i}")))
                    .relative()
                    .px(px(space::S3))
                    .py(px(space::S1H))
                    .flex()
                    .items_baseline()
                    .gap(px(space::S2))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .when(i == at, |d| d.bg(t.selected()))
                    .hover(|d| d.bg(t.hover()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if mention {
                            this.complete_mention(&name, window, cx);
                        } else {
                            this.complete_slash(&name, window, cx);
                        }
                        this.composer.update(cx, |c, cx| c.focus(window, cx));
                    }))
                    .child(
                        div()
                            .flex_none()
                            .ty(CODE)
                            .text_color(t.ink)
                            .child(if mention {
                                c.name.clone()
                            } else {
                                format!("/{}", c.name)
                            }),
                    )
                    .when(!c.hint.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .ty(CODE)
                                .text_color(t.faint)
                                .child(c.hint.trim_matches(['<', '>', '[', ']']).to_string()),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .ty(UI)
                            .text_color(t.quiet)
                            .child(c.description.clone()),
                    )
                    .child(probe(&self.probes, format!("slash-{i}")))
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(relative(1.))
                .pb(px(space::S1))
                .child(
                    floating(&t)
                        .id("slash-menu")
                        .p(px(space::S1))
                        .flex()
                        .flex_col()
                        .occlude()
                        .children(rows)
                        .child(
                            div()
                                .px(px(space::S3))
                                .pt(px(space::S1))
                                .ty(LABEL)
                                .text_color(t.faint)
                                .child("↑↓ choose · Tab complete"),
                        ),
                )
                .into_any_element(),
        )
    }

    fn starter_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let text = self.starter.read(cx).value();
        let empty = text.trim().is_empty();
        let d = self.defaults.clone();
        let project = d
            .project
            .as_deref()
            .map(Self::project_name)
            .unwrap_or_else(|| "No project".into());
        let model = crate::bridge::model_name(&self.models, d.model.as_deref());
        // Everything is already chosen; each choice is one click to change.
        let chips = div()
            .flex()
            .flex_wrap()
            .gap(px(space::S1))
            .ml(px(-space::S2))
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
        let ready = if self.creating {
            Ready::No("Starting the session…")
        } else if empty {
            Ready::No("Describe a task first")
        } else {
            Ready::Yes
        };
        let label = if self.creating {
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
                    d.on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
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
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
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
            .child(div().flex_1().max_h(px(180.)))
            .child(
                div()
                    .w_full()
                    .max_w(px(size::COLUMN + space::S8 * 2.))
                    .mx_auto()
                    .px(px(space::S8))
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .pb(px(space::S1))
                            .ty(DISPLAY)
                            .text_color(t.ink)
                            .child("New session"),
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
                            .child(
                                div()
                                    .ty(READING)
                                    .text_color(t.ink)
                                    .child(Textarea::new(&self.starter)),
                            )
                            .child(div().flex().justify_end().child(start))
                            .child(probe(&self.probes, "starter")),
                    )
                    .child(chips)
                    .children(self.terminal_sessions(cx)),
            )
            .child(div().flex_1())
            .into_any_element()
    }

    /// Sessions started in a terminal, to carry on here with their history.
    fn terminal_sessions(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.recent.is_empty() {
            return None;
        }
        let t = self.t;
        let rows: Vec<AnyElement> = self
            .recent
            .iter()
            .take(6)
            .enumerate()
            .map(|(i, r)| {
                let id = r.agent_session.clone();
                let open = r.open;
                let opening = self.adopting.as_deref() == Some(r.agent_session.as_str());
                let detail = if opening {
                    format!("{} · Opening…", Self::project_name(&r.cwd))
                } else if open {
                    // Picking it up now would run it twice.
                    format!(
                        "{} · Close it in the terminal to pick it up here",
                        Self::project_name(&r.cwd)
                    )
                } else if r.last.is_empty() || r.last == r.title {
                    Self::project_name(&r.cwd)
                } else {
                    format!("{} · Last: “{}”", Self::project_name(&r.cwd), r.last)
                };
                div()
                    .id(SharedString::from(format!("adopt-{i}")))
                    .relative()
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .rounded(px(radius::MD))
                    .when(!open && !opening, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(t.hover()))
                            .active(|d| d.bg(t.press()))
                            .on_click(cx.listener(move |this, _, _, cx| this.adopt(&id, cx)))
                    })
                    // Running in a terminal: shown, so you know where it
                    // went, but it can't be picked up until it's closed there.
                    .when(open, |d| {
                        d.cursor_not_allowed().tooltip(tip(
                            "Running in a terminal. Close it there to pick it up here.",
                            None,
                            t,
                        ))
                    })
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(icon(
                        "square-terminal",
                        14.,
                        if open { t.faint } else { t.quiet },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .ty(UI)
                                    .text_color(if open { t.quiet } else { t.ink })
                                    .child(r.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .ty(UI)
                                    .text_color(if open { t.faint } else { t.quiet })
                                    .child(detail),
                            ),
                    )
                    .child(if open {
                        // A status square and its word, as status shows everywhere.
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(space::S1H))
                            .child(div().size(px(8.)).bg(t.working.0))
                            .child(div().ty(LABEL).text_color(t.quiet).child("In use"))
                    } else {
                        div()
                            .flex_none()
                            .ty(LABEL)
                            .text_color(t.faint)
                            .child(crate::data::ago(r.updated))
                    })
                    .child(probe(&self.probes, format!("adopt-{i}")))
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .pt(px(space::S6))
                // Rows line up with the choices above; their hover reaches out.
                .ml(px(-space::S3))
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(
                    div()
                        .px(px(space::S3))
                        .pb(px(space::S1))
                        .ty(LABEL)
                        .text_color(t.quiet)
                        .child("PICK UP FROM THE TERMINAL"),
                )
                .children(rows)
                .into_any_element(),
        )
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = if this.menu == Some(which) {
                            None
                        } else {
                            // Each menu opens at its top, the current choice lit
                            // for the keyboard.
                            this.menu_scroll = ScrollHandle::new();
                            this.menu_ix = this.menu_entries(which, cx).iter().position(|e| e.on);
                            Some(which)
                        };
                        cx.notify();
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
                        d.tooltip(tip_owned(
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
                        .anchor(Anchor::BottomLeft)
                        .snap_to_window_with_margin(px(8.))
                        .child(div().mb(px(6.)).child(list))
                } else {
                    anchored()
                        .snap_to_window_with_margin(px(8.))
                        .child(div().mt(px(size::CONTROL_SM + 2. + 6.)).child(list))
                };
                // Placed from the chip's top-left corner, not from below
                // it, where the flow would put it.
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
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
            Menu::Project => {
                // Every project, newest first; the menu scrolls.
                let scratch = crate::scratch_dir().display().to_string();
                let mut v: Vec<Entry> = self
                    .projects
                    .iter()
                    // The scratch folder is "No project", below.
                    .filter(|p| p.path != scratch)
                    .map(|p| {
                        let path = p.path.clone();
                        let on = d.project.as_deref() == Some(p.path.as_str());
                        let note = if on && !d.project_picked {
                            "Guessed from what you typed".to_string()
                        } else {
                            // One line: a long path keeps its start and end.
                            crate::viewer::middle_ellipsis(&p.path.replacen(&home(), "~", 1), 38)
                        };
                        Entry::row(
                            format!("m-{}", p.name),
                            &p.name,
                            Some(note),
                            on,
                            act(Box::new(move |this, _, _| {
                                this.defaults.project = Some(path.clone());
                                this.defaults.project_picked = true;
                            })),
                        )
                    })
                    .collect();
                // Mac menu order: the choices, then the action that opens a panel.
                if !v.is_empty() {
                    v.push(Entry::separator());
                }
                v.push(Entry::row(
                    "m-none".into(),
                    "No project",
                    Some("A scratch folder, ~/dent/scratch".into()),
                    d.project.is_none(),
                    act(Box::new(|this, _, _| {
                        this.defaults.project = None;
                        this.defaults.project_picked = true;
                    })),
                ));
                v.push(Entry::separator());
                v.push(Entry::row(
                    "m-choose".into(),
                    "Choose a folder…",
                    None,
                    false,
                    act(Box::new(|this, _, cx| this.choose_project_folder(cx))),
                ));
                v
            }
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
                        key: "m-older".into(),
                        name: format!("Older versions · {}", older.len()),
                        note: None,
                        on: false,
                        kind: EntryKind::Fold(open),
                        act: act(Box::new(move |this, _, _| {
                            this.older_models = !this.older_models;
                            // What opened comes into view.
                            if this.older_models {
                                this.menu_scroll.scroll_to_top_of_item(at);
                            }
                        })),
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
        if !matches!(e.kind, EntryKind::Fold(_)) {
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
        let hi = self.menu_ix;
        let rows: Vec<AnyElement> = entries
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
                    EntryKind::Fold(open) => {
                        div()
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
                            .child(icon(
                                if *open {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                12.,
                                t.quiet,
                            ))
                            .child(
                                div()
                                    .ty(UI)
                                    .text_size(px(12.))
                                    .text_color(t.quiet)
                                    .child(e.name.clone()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_entry(&e, window, cx)
                            }))
                            .into_any_element()
                    }
                    EntryKind::Row => menu_row(
                        &self.probes,
                        e.key.clone(),
                        &e.name,
                        e.note.clone(),
                        e.on,
                        &t,
                    )
                    .when(lit, |d| d.bg(t.hover()))
                    .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                        if *h && this.menu_ix != Some(i) {
                            this.menu_ix = Some(i);
                            cx.notify();
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.choose_entry(&e, window, cx)),
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
                    linear_color_stop(t.raised, 0.),
                    linear_color_stop(t.raised.opacity(0.), 1.),
                ))
        };
        floating(&t)
            .relative()
            .w(px(280.))
            .overflow_hidden()
            .occlude()
            .child(
                div()
                    .id("menu")
                    .max_h(px(max_h))
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
    fn row(key: String, name: &str, note: Option<String>, on: bool, act: Option<Act>) -> Self {
        Self {
            key,
            name: name.to_string(),
            note,
            on,
            kind: EntryKind::Row,
            act,
        }
    }
    fn separator() -> Self {
        Self {
            key: String::new(),
            name: String::new(),
            note: None,
            on: false,
            kind: EntryKind::Separator,
            act: None,
        }
    }
    fn note(text: &str) -> Self {
        Self {
            key: String::new(),
            name: text.to_string(),
            note: None,
            on: false,
            kind: EntryKind::Note,
            act: None,
        }
    }
    /// Whether the keyboard can land on it.
    pub fn choosable(&self) -> bool {
        self.act.is_some()
    }
}

fn menu_note_owned(text: String, t: &theme::Theme) -> AnyElement {
    div()
        .px(px(space::S3))
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

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
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
            scope: format!("{}/{ix}/report-{call}", s.id),
            bg: t.surface,
            base: Some(std::path::PathBuf::from(&s.cwd)),
        };
        let body = markdown::render(blocks, &t, false, &mut r);
        div()
            .flex_1()
            .min_w_0()
            .my(px(space::S1))
            .pl(px(space::S1))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(div().ty(LABEL).text_color(t.quiet).child("REPORT"))
            .child(body)
            .when(long, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("report-{ix}-{call}")))
                        .ty(UI)
                        .text_color(t.quiet)
                        .cursor_pointer()
                        .hover(|d| d.text_color(t.ink))
                        .on_click(fold)
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
    path.exists().then_some(path)
}

fn sent_attachment(line: &str, cwd: &str, t: &theme::Theme) -> Div {
    let (place, what) = line.split_once(" · ").unwrap_or((line, ""));
    // A picture shows as itself, with its name under it.
    if place == "Attached"
        && let Some(path) = picture_path(what, cwd)
    {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(space::S1))
            .child(
                div()
                    .max_w(px(320.))
                    .max_h(px(220.))
                    .rounded(px(radius::MD))
                    .border_1()
                    .border_color(t.hairline)
                    .overflow_hidden()
                    .child(
                        img(path)
                            .max_w(px(320.))
                            .max_h(px(220.))
                            .object_fit(ObjectFit::Contain),
                    ),
            )
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

/// Lines of code or output that select and copy; with `diff`, removed
/// ("- ") and added ("+ ") lines take the diff's colours.
pub fn code_lines(id: String, text: String, diff: bool, ink: Hsla, t: &crate::theme::Theme) -> Div {
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

/// How a composer's footer fits its room: which words each chip keeps,
/// and whether the context reading and the hint show.
#[derive(Clone)]
pub struct Fit {
    pub labels: [String; 3],
    pub context: bool,
    pub hint: bool,
}

/// Fit chips (each with its full and short label; an empty short label is
/// the icon alone) and the optional readings into `room` points. Things give
/// way one at a time: the hint, the context, then each chip's words in turn,
/// model first and permission last.
pub fn fit_footer(room: f32, chips: [(String, String); 3], context: bool, hint: &str) -> Fit {
    // Widths from the label lengths: the interface face runs about 7 pt a
    // character; a chip adds its icon, chevron and padding.
    let text = |t: &str| t.chars().count() as f32 * 7.2;
    let chip = |t: &str| if t.is_empty() { 44. } else { 52. + text(t) };
    let gap = space::S1;
    let ctx_w = text("context 9.9% · ");
    let hint_w = text(hint) + space::S2;
    // The steps, least loss first.
    let mut steps: Vec<([bool; 3], [bool; 3], bool, bool)> = Vec::new();
    // (short?, icon only?) per chip, context, hint
    let full = [false; 3];
    steps.push((full, full, true, true));
    steps.push((full, full, true, false));
    steps.push((full, full, false, false));
    steps.push(([true, false, false], full, false, false));
    steps.push(([true, false, true], full, false, false));
    steps.push(([true, false, true], [false, false, true], false, false));
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
    }
}
