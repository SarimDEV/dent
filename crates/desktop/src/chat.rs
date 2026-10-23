//! The main column: a session's conversation and composer, or the new-session
//! page where you start by typing.

use crate::app::Scope;
use crate::ui::FocusField;
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

use crate::data::Fold;
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

    fn header(&mut self, s: &Session, cx: &mut Context<Self>) -> Div {
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
        // Very narrow, the helpers with keys of their own step out (⌥↑ ⌥↓,
        // ⌘;), so the title and the actions that are only here keep room.
        let cramped = self.chat_w < CRAMPED;
        // Nothing to step through or carry on from when its only message
        // was taken back.
        let said = s.items.iter().any(|i| matches!(i, Item::User { .. }));
        if !cramped && said {
            actions = actions.child(self.message_arrows(s, cx));
        }
        // A failure's note offers Continue itself; one name, in one place.
        // A limit's note offers only to continue later, so here it stays.
        let limited = self.limit_hit(&s.agent_id).is_some();
        let note_offers = s
            .items
            .iter()
            .rev()
            .take_while(|i| !matches!(i, Item::User { .. }))
            .any(|i| match i {
                Item::Note {
                    kind: crate::data::NoteKind::Error,
                    text,
                    ..
                } => !(limited && text.to_lowercase().contains("limit")),
                _ => false,
            });
        if s.stopped && !s.closed && said && !note_offers {
            let sid = id.clone();
            actions = actions.child(
                button("resume", "Continue", false, &t)
                    .on_press(cx.listener(move |this, _, _, cx| this.resume(&sid, cx))),
            );
        }
        // The project's scripts, run into your terminal.
        if !s.closed && self.actions.get(&s.id).is_some_and(|a| !a.is_empty()) {
            let label = if tight { "" } else { "Run" };
            actions = actions.child(self.chip(Menu::Actions, "play", label, false, cx));
        }
        // Its pull request, as GitHub has it: a click opens it.
        if let Some(pr) = &s.pr {
            use crate::gitbar::Tone;
            let tone = crate::gitbar::pr_tone(pr);
            let ink = match tone {
                Tone::Trouble => t.needs.0,
                Tone::Good => t.done.0,
                Tone::Running => t.working.0,
                Tone::Merged => t.spectrum[0],
                Tone::Closed => t.removed,
            };
            let icon_name = match tone {
                Tone::Merged => "git-merge",
                Tone::Closed => "git-pull-request-closed",
                _ => "git-pull-request",
            };
            let words = crate::gitbar::pr_words(pr);
            let url = pr.url.clone();
            let badge = div()
                .id("pr-badge")
                .relative()
                .flex_none()
                .h(px(size::CONTROL))
                .px(px(space::S2))
                .flex()
                .items_center()
                .gap(px(space::S1H))
                .rounded(px(radius::MD))
                .cursor_pointer()
                .hover(|d| d.bg(t.hover()))
                .active(|d| d.bg(t.press()))
                .ty(UI)
                .text_color(t.ink)
                .child(icon(icon_name, 13., ink))
                .child(format!("#{}", pr.number))
                // Narrow, the state is its colour and its tooltip.
                .when(!tight, |d| {
                    d.child(div().text_color(t.quiet).child("·"))
                        .child(div().text_color(t.quiet).child(words))
                })
                .with_tip(crate::ui::tip_owned(
                    format!("Pull request #{} · {words}", pr.number),
                    t,
                ))
                .on_press(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
                .child(probe(&self.probes, "pr-badge"));
            actions = actions.child(badge);
        }
        // The branch's next Git step, while the session is open.
        // The pull request as last said live wins over the last status read.
        let st = self.git.get(&s.id).map(|st| dent_protocol::GitStatus {
            pr: s.pr.clone().or_else(|| st.pr.clone()),
            ..st.clone()
        });
        if !s.closed
            && let Some(st) = st.as_ref()
            && let Some(q) = crate::gitbar::quick(st)
        {
            let sid = id.clone();
            let busy = self.git_busy.get(&s.id).copied();
            let confirming = self.git_confirming(&s.id);
            let label = match (busy, confirming) {
                // Asking GitHub for a brief, not running Git.
                (Some(_), _) if matches!(q.press, Some(crate::gitbar::Press::Brief(_))) => {
                    "Gathering…".to_string()
                }
                (Some(a), _) => crate::gitbar::busy(a).to_string(),
                (None, Some(_)) => crate::gitbar::confirm(st),
                _ => q.label.clone(),
            };
            let b = if tight && busy.is_none() && confirming.is_none() {
                icon_button("git-action", q.icon, &t)
            } else {
                button("git-action", &label, confirming.is_some(), &t)
            };
            let b = b.relative().child(probe(&self.probes, "git-action"));
            let b =
                match (&q.press, busy) {
                    (_, Some(_)) => crate::ui::off(b, "Working on it", &t),
                    // Mid-turn the agent may be halfway through a file.
                    _ if s.running() => crate::ui::off(b, "Once the agent's turn ends", &t),
                    (Some(press), None) => {
                        let press = press.clone();
                        b.with_tip(crate::ui::tip_owned(
                            if tight {
                                format!("{label}: {}", q.tip)
                            } else {
                                q.tip.clone()
                            },
                            t,
                        ))
                        .on_press(cx.listener(move |this, _, _, cx| match &press {
                            crate::gitbar::Press::Run(a) => this.run_git(&sid, *a, cx),
                            crate::gitbar::Press::Brief(b) => this.brief_agent(&sid, *b, cx),
                        }))
                    }
                    (None, None) => crate::ui::off_owned(b, q.tip.clone(), &t),
                };
            actions = actions.child(b);
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
            // Only where the agent can carry a conversation on.
            if s.caps.fork {
                actions = actions.child(
                    fork.relative()
                        .on_press(cx.listener(move |this, _, _, cx| this.fork(&sid, None, cx)))
                        .child(probe(&self.probes, "fork")),
                );
            }
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
        if !cramped && !s.items.is_empty() && s.caps.aside {
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
            // A name too long for the row ends in an ellipsis, not mid-glyph.
            .child(div().max_w_full().truncate().child(s.project.clone()))
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
        call: &str,
        d: &crate::data::CallDetail,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.t;
        let key = &format!("{}/{call}", s.id);
        // Long text shows its start, and a line that opens the rest: held
        // to so many lines, and so many characters, so one enormous line
        // can't stall the chat.
        let clip = |text: &str, all: bool| {
            let (kept, ..) = crate::ansi::clip(text, all);
            clipped(text, &kept)
        };
        let block = |id: String, (text, swaps): (String, Vec<_>), ink: Hsla| {
            code_lines(id, text, swaps, d.edits, ink, &t)
        };
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
            let fold = if which == "in" {
                Fold::CallIn(&s.id, call)
            } else {
                Fold::CallOut(&s.id, call)
            };
            let all = self.is_open(&fold);
            let fold = fold.key();
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
                    .on_press(cx.listener(move |this, _, _, cx| {
                        this.toggle_fold(&fold);
                        cx.notify();
                    }))
                    .child(probe(&self.probes, format!("call-{which}-all")))
                    .into_any_element()
            })
        };
        if !d.input.trim().is_empty() {
            let all = self.is_open(&Fold::CallIn(&s.id, call));
            let text = clip(&d.input, all);
            // Its label sits on the text's edge, the ghost's padding
            // hanging out to the left.
            col = col
                .child(block(format!("call-in-{key}"), text, t.ink))
                .children(
                    rest("in", &d.input, cx).map(|r| div().flex().ml(px(-space::S2)).child(r)),
                );
        }
        // The output's "Show more" and the file's actions share one row.
        let mut out_rest = None;
        if let Some((failed, out)) = &d.output
            && !out.trim().is_empty()
            && !d.edits
        {
            let all = self.is_open(&Fold::CallOut(&s.id, call));
            // Colour codes become colours; other escapes go.
            let parsed = crate::ansi::parsed(out);
            let (plain, spans) = (&parsed.0, &parsed.1);
            let (kept, ..) = crate::ansi::clip(plain, all);
            // Copy takes everything kept, not just what's shown.
            let whole = parsed.clone();
            col = col
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .ty(LABEL)
                                // Magenta means only "needs you": a failure is
                                // said in words, in ink.
                                .text_color(if *failed { t.ink } else { t.faint })
                                .child(if *failed { "Failed" } else { "Output" }),
                        )
                        .child(
                            self.copy_ghost(
                                format!("call-copy-{key}"),
                                move || whole.0.clone(),
                                cx,
                            )
                            .mr(px(-space::S2)),
                        ),
                )
                .child(output_lines(
                    format!("call-out-{key}"),
                    plain,
                    spans,
                    &kept,
                    t.quiet,
                    &t,
                ));
            out_rest = rest("out", plain, cx);
        }
        // Its file: shown in dent when it can be, and its changes.
        let in_diff = d.path.as_ref().and_then(|p| {
            let rel = crate::bridge::relative(std::path::Path::new(&s.cwd), p);
            s.diff.iter().any(|f| f.path == rel).then_some(rel)
        });
        // A file on the session's machine: there, or (another machine's)
        // still being asked about.
        let openable = d
            .path
            .as_ref()
            .filter(|p| {
                matches!(
                    crate::files::stat(&crate::files::Ask::new(&s.id, p, &s.cwd)),
                    crate::files::Look::Loading
                        | crate::files::Look::Here(dent_protocol::FileStat::File { .. })
                        | crate::files::Look::Stale(dent_protocol::FileStat::File { .. })
                )
            })
            .cloned();
        let review = in_diff.filter(|_| d.edits);
        if out_rest.is_some() || openable.is_some() || review.is_some() {
            let sid = s.id.clone();
            col = col.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .ml(px(-space::S2))
                    .gap(px(space::S1))
                    .children(out_rest)
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
                    .when_some(review, |c, rel| {
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
        let top = self.top_item();
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
            // All of them, to search and pick from.
            .child(
                div()
                    .id("mine-all")
                    .relative()
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_l_1()
                    .border_color(t.hairline)
                    .child(icon("list", 14., t.quiet))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.hover()))
                    .active(|d| d.bg(t.press()))
                    .with_tip(tip("Your messages", Some("⇧ ⌘ K"), t))
                    .on_click(cx.listener(|this, _, window, cx| this.open_messages(window, cx)))
                    .child(probe(&self.probes, "mine-all")),
            )
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
        crate::ui::ghost_with(
            SharedString::from(key.clone()),
            crate::ui::steady_label(if copied { "Copied" } else { "Copy" }, &["Copy", "Copied"]),
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
            .child(div().text_color(t.secondary()).child("Review"))
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
        let pointed = self.hover_row.map(|r| self.rows.item_of(r)) == Some(ix);
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
                    kind: crate::data::NoteKind::Error,
                    ..
                }
            )
        });
        let can_try = latest && !errored && !s.closed && !s.running() && start > 0 && s.caps.rewind;
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
            // Asked as going back to a message is: the question, then the
            // choices in the Mac order, the destructive one alone on the
            // left and Cancel beside the default on the right, so Return
            // never lands on the destructive one.
            div()
                .mt(px(space::S2))
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .child(
                    div()
                        .ty(UI)
                        .text_color(t.ink)
                        .child("It changed files. Undo those too before trying again?"),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(space::S2))
                        .child(
                            button("try-again-files", "Undo them and try again", false, &t)
                                .relative()
                                .on_press(cx.listener(move |this, _, _, cx| {
                                    this.try_again(&s1, Some(true), cx)
                                }))
                                .child(probe(&self.probes, "try-again-files")),
                        )
                        .child(div().flex_1())
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
                                .on_press(cx.listener(move |this, _, _, cx| {
                                    this.try_again(&s2, Some(false), cx)
                                }))
                                .child(probe(&self.probes, "try-again-keep")),
                        ),
                )
        } else {
            let sid = s.id.clone();
            // What the turn changed leads the line and always shows; the
            // time and actions follow, on hover for an older turn.
            let changed = change_after(&s.items, ix).and_then(|c| match &s.items[c] {
                Item::Changed { paths } => Some(self.changed_line(s, c, paths, cx)),
                _ => None,
            });
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
                        // (A reply drawn a row per block shows them while
                        // the pointer is on any of its blocks.)
                        .when(!latest && !pointed, |d| d.reveal_on_hover(group.clone()))
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
        self.chat_view = (
            self.transcript.viewport_bounds(),
            self.transcript.is_following_tail(),
        );
        let count = self.transcript.item_count();
        // Only the rows on screen are drawn, however long the session: an
        // item each, a reply a row per block (`rows.rs`).
        let transcript = list(
            self.transcript.clone(),
            cx.processor(move |this, row: usize, window, cx| {
                let Some(s) = this.current_rc() else {
                    return div().into_any_element();
                };
                let Some(r) = this.rows.get(row) else {
                    return div().into_any_element();
                };
                // The working line's own row stands past the last item.
                let ix = r.item.unwrap_or(s.items.len());
                // The items, and the working line's row if it has one.
                let count =
                    this.rows.items() + usize::from(this.rows.end_of_items() < this.rows.len());
                // Each message keeps its place in the chat's selection,
                // drawn or not; a reply's blocks keep theirs within it.
                let area = crate::select::area(&format!("chat/{}", s.id));
                let _place = match r.part {
                    Some(p) => {
                        let sigs = this
                            .row_parts
                            .1
                            .get(ix)
                            .map(|(_, sigs, _)| sigs.clone())
                            .unwrap_or_else(|| std::rc::Rc::from([]));
                        crate::select::part(area, ix, p.block, sigs)
                    }
                    None => crate::select::item(area, ix),
                };
                // Find numbers a block's matches on from those before it,
                // drawn or not: set as it paints, since every row on screen
                // is laid out before any is painted.
                let found_from = r.part.map(|p| {
                    let before = this.found_before(ix, p.block);
                    crate::select::at_paint(move |_| crate::select::set_found(ix as u32, before))
                });
                // The working line sits under the reply being written, above
                // any messages queued after it, so they never read as the
                // thing being written.
                let host = working_host(&s);
                // Folded into its turn's line: nothing of its own to draw.
                let fold = this.turn_folds.get(ix).copied().unwrap_or(TurnFold::Shown);
                if fold == TurnFold::Hidden {
                    return div().into_any_element();
                }
                // Closed, the turn's line stands in for the whole item.
                if matches!(fold, TurnFold::Head { open: false, .. }) && !r.first() {
                    return div().into_any_element();
                }
                let drawn = |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| match (
                    s.items.get(ix),
                    r.part,
                ) {
                    (Some(item), Some(p)) => this.reply_part(&s, ix, item, p, window, cx),
                    (Some(item), None) => this.item(&s, ix, item, window, cx),
                    (None, _) => div().into_any_element(),
                };
                let body = match s.items.get(ix) {
                    Some(_) if host == Some(ix) && r.first() => div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S4))
                        .child(this.working_line(&s))
                        .child(drawn(this, window, cx))
                        .into_any_element(),
                    Some(_) => drawn(this, window, cx),
                    None if host.is_some() => div().into_any_element(),
                    None => this.working_line(&s),
                };
                // The turn's line stands where its work began; open, the
                // work follows it.
                let body = match fold {
                    TurnFold::Head { took, user, open } if r.first() => {
                        let line = this.worked_line(&s, took, user, open, cx);
                        if open {
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(space::S2))
                                .child(line)
                                .child(body)
                                .into_any_element()
                        } else {
                            line
                        }
                    }
                    _ => body,
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
                // The last reply in a turn carries the turn's time and
                // actions, under its last block.
                let turn_end = turn_end_after(&s.items, ix);
                let body = match turn_end {
                    Some(end) if r.last() => this.with_reply_footer(&s, ix, end, body, cx),
                    _ => body,
                };
                // Pointing at any of its blocks shows an older turn's actions.
                let body = match turn_end {
                    Some(_) if r.part.is_some() => div()
                        .id(("reply-part", row))
                        .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                            let was = this.hover_row;
                            if *on {
                                this.hover_row = Some(row);
                            } else if was == Some(row) {
                                this.hover_row = None;
                            }
                            if this.hover_row != was {
                                cx.notify();
                            }
                        }))
                        .child(body)
                        .into_any_element(),
                    _ => body,
                };
                let ends_turn = turn_end.is_some() && r.last();
                let marker = matches!(s.items.get(ix), Some(Item::TurnEnd { .. }));
                // Just landed: it rises the last few pixels into place.
                let arrive = (!crate::reduce_motion())
                    .then(|| this.row_born.get(&row))
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
                            .when(row == 0, |d| d.pt(px(space::S6)))
                            .when_some(arrive, |d, k| {
                                d.relative().top(px((1. - k) * motion::RISE)).opacity(k)
                            })
                            .pb(px(if !r.last() {
                                // Between a reply's blocks, as within one.
                                space::S3
                            } else if ix + 1 == count {
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
                                                | Item::Thinking(..)
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
                                // The working line leads the queued run.
                                let next = if host == Some(ix + 1) { None } else { next };
                                match (work(s.items.get(ix)), work(next)) {
                                    (true, true) => space::HALF,
                                    (false, true) => space::S3,
                                    (true, false) | (false, false) => space::S4,
                                }
                            }))
                            .children(found_from)
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
        // A reply's blocks never drawn as they are copy from their words.
        let blocks: Vec<_> = self.row_parts.1.iter().map(|(.., b)| b.clone()).collect();
        crate::select::part_source(
            crate::select::area(&format!("chat/{}", s.id)),
            move |ix, b| {
                let blocks = blocks.get(ix)?.as_ref()?;
                let mut words = Vec::new();
                markdown::drawn(std::slice::from_ref(blocks.get(b)?), &mut words);
                Some(words.join("\n"))
            },
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
            // A row scrolled out from under the pointer says nothing as it
            // goes: an older turn's actions it showed go with the scroll.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                if this.hover_row.take().is_some() {
                    cx.notify();
                }
            }))
            // Find floats at the top; while it's open the chat starts under
            // it, so nothing reads through the bar.
            .when(find.is_some(), |d| {
                d.pt(px(space::S2 + size::CONTROL + space::S2 - space::S1))
            })
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
                        .bg(t.lifted())
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
        let footer: AnyElement = if let Some(card) = self.moved_card(s, cx) {
            div()
                .relative()
                .w_full()
                .max_w(px(size::COLUMN + space::S8 * 2.))
                .mx_auto()
                .px(px(space::S8))
                .pb(px(space::S4))
                .children(latest)
                .child(card)
                .into_any_element()
        } else if s.closed {
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
                .child({
                    // Its worktree is cleaned up only when you say;
                    // reopening brings one back on its branch.
                    // Whether it's still there is its machine's to say.
                    let there = matches!(
                        crate::files::stat(&crate::files::Ask::new(&s.id, &s.cwd, &s.cwd)),
                        crate::files::Look::Here(dent_protocol::FileStat::Dir { .. })
                            | crate::files::Look::Stale(dent_protocol::FileStat::Dir { .. })
                    );
                    let remove = (s.root.is_some() && there).then(|| {
                        let sid = s.id.clone();
                        let force = self.worktree_force.contains(&s.id);
                        // Its own id once it asks again, so its tooltip is the new one.
                        button(
                            if force { "remove-worktree-anyway" } else { "remove-worktree" },
                            if force { "Remove anyway" } else { "Remove worktree" },
                            false,
                            &t,
                        )
                        .relative()
                        .with_tip(tip(
                            if force {
                                "Its changes that aren't committed go too; the branch stays"
                            } else {
                                "Take away its folder; the branch and its commits stay"
                            },
                            None,
                            t,
                        ))
                        .on_press(cx.listener(move |this, _, _, cx| this.remove_worktree(&sid, cx)))
                        .child(probe(&self.probes, "remove-worktree"))
                    });
                    let reopen = button("reopen", "Reopen", true, &t)
                        .relative()
                        .child(probe(&self.probes, "reopen"))
                        .on_press(cx.listener(move |this, _, window, cx| {
                            this.reopen(&id, cx);
                            this.composer.update(cx, |c, cx| c.focus_field(window, cx));
                        }));
                    let words = div()
                        .flex()
                        .items_center()
                        .gap(px(space::S3))
                        .flex_1()
                        .min_w_0()
                        .child(icon("archive", 15., t.quiet))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .ty(UI)
                                .text_color(t.quiet)
                                .child("Closed. It stays searchable, and reopening puts it back in your inbox."),
                        );
                    let buttons = div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(space::S2))
                        .children(remove)
                        .child(reopen);
                    // Narrow, the words take the width and the buttons go
                    // under them, rather than the words standing in a column.
                    let narrow = self.chat_w < 600.;
                    div()
                        .px(px(space::S4))
                        .py(px(space::S3))
                        .rounded(px(radius::XL))
                        .border_1()
                        .border_color(t.hairline)
                        .flex()
                        .gap(px(space::S3))
                        .map(|d| if narrow { d.flex_col() } else { d.items_center() })
                        .child(words)
                        .child(buttons)
                })
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
                .child(if s.old.is_some() {
                    self.old_version_bar(s, cx)
                } else {
                    self.composer_box(s, cx).into_any_element()
                })
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
            .children(self.move_banner(s, cx))
            .children(self.away_banner(s, cx))
            .child(transcript)
            .child(footer)
            .into_any_element()
    }

    /// Under the header while the session's machine is away: what that
    /// means for it, and Retry. Messages written meanwhile wait here.
    /// A move under way, or stopped partway: what it's doing, as the
    /// away banner says a machine's state, with what can be done about it.
    fn move_banner(&self, s: &Session, cx: &mut Context<Self>) -> Option<AnyElement> {
        use dent_client_core::carry::{Ended, Phase};
        use dent_protocol::MoveState as M;
        let t = self.t;
        let run = self.moves.get(&s.id);
        // Stopped, and working here again (it ended there, and the chat
        // says why): choosing a machine starts afresh.
        if run.is_some_and(|r| r.stopped.is_some()) && s.moving.is_none() {
            return None;
        }
        let to = match (run, &s.moving) {
            (Some(r), _) => r.to_name.clone(),
            (None, Some((to, _, M::Pending | M::Departing { .. }))) => to.clone(),
            _ => return None,
        };
        let sent = matches!(&s.moving, Some((_, _, M::Departing { sent: true })));
        let id = s.id.clone();
        // (what, the rest, how far along, stopped)
        let (what, rest, bar, stopped) = match run.map(|r| (r.phase(), &r.stopped)) {
            Some((_, Some(e))) => {
                let why = match e {
                    Ended::Stopped { why, .. } => why.clone(),
                    Ended::NotAccepted(why) => why.clone(),
                    Ended::NoProject => format!("{to} doesn't have this project"),
                };
                (format!("The move to {to} stopped"), why, None, true)
            }
            Some((Phase::Waiting, _)) => (
                format!("Moving to {to}"),
                "once this turn ends".to_string(),
                None,
                false,
            ),
            Some((Phase::Sending { sent, total }, _)) => (
                format!("Moving to {to}"),
                format!("Sending {} of {}", mb(sent), mb(total)),
                Some(if total == 0 {
                    1.
                } else {
                    sent as f32 / total as f32
                }),
                false,
            ),
            Some((Phase::Arriving, _)) => (
                format!("Moving to {to}"),
                "arriving there".to_string(),
                Some(1.),
                false,
            ),
            Some(_) => (format!("Moving to {to}"), String::new(), None, false),
            // Begun here before this app restarted, or by another device.
            None => (
                format!("The move to {to} isn't being carried"),
                "Retry carries on with it".to_string(),
                None,
                true,
            ),
        };
        let tint = if stopped {
            t.signal(Status::Waiting)
                .0
                .opacity(if t.dark { 0.12 } else { 0.10 })
        } else {
            t.tint()
        };
        let (rid, kid, cid) = (id.clone(), id.clone(), id.clone());
        let to_id = self
            .moves
            .get(&s.id)
            .map(|r| r.to.clone())
            .or_else(|| self.move_target(s));
        // Before anything went, it can be called off; after, only kept here.
        let early =
            !sent && run.is_some_and(|r| matches!(r.phase(), Phase::Asking | Phase::Waiting));
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .bg(tint)
                .border_b_1()
                .border_color(t.hairline)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S3))
                        .px(px(space::S4))
                        .py(px(space::S2))
                        .ty(UI)
                        .child(icon(
                            "arrow-left-right",
                            14.,
                            if stopped { t.waiting.0 } else { t.quiet },
                        ))
                        // What, over how far it's got or why it stopped:
                        // a long why wraps, whole, rather than losing its
                        // end.
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t.ink)
                                        .child(what),
                                )
                                .when(!rest.is_empty(), |d| {
                                    d.child(div().text_color(t.quiet).child(rest))
                                }),
                        )
                        .when(stopped && to_id.is_some(), |d| {
                            let to_id = to_id.clone().unwrap_or_default();
                            d.child(
                                button("move-retry", "Retry", true, &t)
                                    .relative()
                                    .child(probe(&self.probes, "move-retry"))
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.start_move(&rid, &to_id, cx)
                                    })),
                            )
                        })
                        .when(stopped || sent, |d| {
                            d.child(
                                ghost("move-keep", "Continue here", &t)
                                    .flex_none()
                                    .relative()
                                    .child(probe(&self.probes, "move-keep"))
                                    .with_tip(tip(
                                        "Keep it on this machine; the other may hold a copy",
                                        None,
                                        t,
                                    ))
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.stop_move(&kid, true, cx)
                                    })),
                            )
                        })
                        .when(early, |d| {
                            d.child(
                                ghost("move-call-off", "Call off", &t)
                                    .flex_none()
                                    .relative()
                                    .child(probe(&self.probes, "move-call-off"))
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.stop_move(&cid, false, cx)
                                    })),
                            )
                        }),
                )
                // How much has gone, while it sends.
                .when_some(bar, |d, k| {
                    d.child(
                        div().h(px(2.)).w_full().child(
                            div()
                                .h_full()
                                .w(relative(k.clamp(0., 1.)))
                                .bg(t.ink.opacity(0.5)),
                        ),
                    )
                })
                .child(probe(&self.probes, "move-banner"))
                .arrive("move-banner", From::Above, motion::BASE)
                .into_any_element(),
        )
    }

    /// Moved: where the reply box was, where it carries on now.
    fn moved_card(&self, s: &Session, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.t;
        let Some((to, Some(there), dent_protocol::MoveState::Departed)) = &s.moving else {
            return None;
        };
        let key = self.moved_to(s, to, there);
        // Called here what it's called now, renamed since or not.
        let name = key.as_deref().map_or(to.clone(), |k| {
            self.machine_name(dent_client_core::fleet::split(k).0)
        });
        // The button names it when the name is short; the line above says
        // it whole.
        let label = if name.chars().count() <= 18 {
            format!("Open on {name}")
        } else {
            "Open it there".to_string()
        };
        let open = key.map(|key| {
            button("open-moved", label, true, &t)
                .relative()
                .child(probe(&self.probes, "open-moved"))
                .on_press(cx.listener(move |this, _, window, cx| {
                    if this.session(&key).is_some() {
                        this.select(Some(key.clone()), window, cx);
                    } else {
                        this.pending_select = Some(key.clone());
                    }
                }))
        });
        Some(
            div()
                .px(px(space::S4))
                .py(px(space::S3))
                .rounded(px(radius::XL))
                .border_1()
                .border_color(t.hairline)
                .flex()
                .items_center()
                .gap(px(space::S3))
                .child(icon("arrow-left-right", 15., t.quiet))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .ty(UI)
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.ink)
                                .child(format!("This session moved to {name}")),
                        )
                        .child(div().text_color(t.quiet).child(
                            "This is how it was when it left. Its folder here stays until you remove it.",
                        )),
                )
                .children(open)
                .child(probe(&self.probes, "moved-card"))
                .into_any_element(),
        )
    }

    fn away_banner(&self, s: &Session, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.t;
        let owner = dent_client_core::fleet::split(&s.id).0;
        let m = self.machines.iter().find(|m| m.id == owner)?;
        let standing = crate::machines_page::Standing::of(m);
        let (strong, rest) = match standing {
            crate::machines_page::Standing::Online | crate::machines_page::Standing::Connecting => {
                return None;
            }
            crate::machines_page::Standing::Asleep => (
                "is asleep",
                "The agent paused there and carries on when it wakes. Messages wait here till then.",
            ),
            // Nothing happens until something changes, so each says what.
            crate::machines_page::Standing::NeedsCode => (
                "needs a code",
                "Pair with it in Settings › Machines. Messages wait here till then.",
            ),
            crate::machines_page::Standing::Blocked => match m.standing() {
                dent_client_core::machine::State::Removed => (
                    "removed this device",
                    "This device was removed there. Pair again in Settings › Machines to use it.",
                ),
                dent_client_core::machine::State::Incompatible { .. } => (
                    "needs an update",
                    "It and this app don't speak the same version. Update dent where it's older.",
                ),
                _ => (
                    "trusts this device only on Tailscale",
                    "Connect to it over Tailscale, or pair with a code to use this route.",
                ),
            },
            _ => (
                "can't be reached",
                "Its sessions keep their place there. Messages wait here till it's back.",
            ),
        };
        let shape = self
            .shapes()
            .get(owner)
            .copied()
            .unwrap_or(crate::glyph::Shape::Circle);
        let id = owner.to_string();
        let tint = t
            .signal(Status::Waiting)
            .0
            .opacity(if t.dark { 0.12 } else { 0.10 });
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::S3))
                .px(px(space::S4))
                .py(px(space::S2))
                .bg(tint)
                .border_b_1()
                .border_color(t.hairline)
                .ty(UI)
                .child(crate::machines_page::holder(
                    shape,
                    Some(if standing == crate::machines_page::Standing::Asleep {
                        t.waiting.0
                    } else {
                        t.idle.0
                    }),
                    22.,
                    false,
                    &t,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .gap(px(space::S1H))
                        // A long machine name gives way, ending in "…",
                        // before it would reach Retry; what it means goes
                        // first. What's become of it always shows.
                        .child(
                            div()
                                .flex_shrink(1.)
                                .min_w(px(48.))
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.ink)
                                .child(crate::data::machine_short(m.name())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.ink)
                                .child(strong),
                        )
                        .child(
                            div()
                                .flex_shrink(10.)
                                .min_w_0()
                                .truncate()
                                .text_color(t.quiet)
                                .child(rest),
                        ),
                )
                .child(
                    ghost("away-retry", "Retry", &t)
                        .flex_none()
                        .on_press(cx.listener(move |this, _, _, cx| {
                            this.live
                                .signal(Some(&id), dent_client_core::machine::Signal::Retry);
                            cx.notify();
                        })),
                )
                .child(probe(&self.probes, "away-banner"))
                .arrive("away-banner", From::Above, motion::BASE)
                .into_any_element(),
        )
    }

    /// The last line while the agent works: what it's doing, and for how
    /// long. The square sits inside the column, on the reply's left edge
    /// like everything else in the chat; the words breathe once it's been
    /// a while, and each change of
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
            Some(dent_protocol::Activity::SettingUp { .. }) => "setting-up",
            Some(dent_protocol::Activity::Preparing { .. })
            | Some(dent_protocol::Activity::Thinking { .. })
            | None => "working",
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
            .relative()
            .child(probe(&self.probes, "working-line"))
            .flex()
            .items_center()
            // Its words start where a note's do.
            .gap(px(space::S2))
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

    /// A reply, as Markdown: whole, or (`part`) one block of it, for a reply
    /// drawn a row per block.
    fn reply_md(
        &mut self,
        s: &Session,
        ix: usize,
        src: &str,
        part: Option<crate::rows::Part>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
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
        // The reply streaming in: only its unfinished end is parsed. A
        // block of one has it from when its rows were worked out.
        let streaming = births.is_some();
        let blocks = match part.and_then(|_| self.part_blocks(&s.id, ix)) {
            Some(b) => b,
            None if streaming => self.markdown_live(&s.id, src),
            None => self.markdown(src),
        };
        match (births, blocks.as_slice()) {
            // A reply streaming in: each word fades in over 180 ms.
            (Some(births), [markdown::Block::Para(inline)])
                if part.is_none() && !crate::reduce_motion() =>
            {
                // Where each word ends, counted as births are (by
                // spaces and line breaks), found by halving.
                let breaks: Vec<usize> = inline
                    .text
                    .match_indices([' ', '\n'])
                    .map(|(i, _)| i)
                    .collect();
                let fade = move |off: usize| {
                    let k = breaks.partition_point(|&b| b < off);
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
                    live: streaming,
                    counted: true,
                    md: self.mdv.clone(),
                    owner: cx.entity_id(),
                    window,
                    cx,
                    scope: at.clone(),
                    bg: t.surface,
                    base: Some(std::path::PathBuf::from(&s.cwd)),
                    session: Some((s.id.clone(), s.cwd.clone())),
                };
                match part {
                    // One block, in a row of its own; the last row takes any
                    // the rows were worked out without (they're worked out
                    // as the reply changes, so that's at most a frame).
                    Some(p) => {
                        let from = p.block.min(blocks.len());
                        let to = if p.block + 1 == p.of {
                            blocks.len()
                        } else {
                            (p.block + 1).min(blocks.len())
                        };
                        div()
                            .relative()
                            .child(markdown::render_part(&blocks[from..to], from, &t, &mut r))
                            .when(p.block == 0, |d| d.child(probe(&self.probes, "reply")))
                            .into_any_element()
                    }
                    None => div()
                        .relative()
                        .child(markdown::render(blocks.as_slice(), &t, false, &mut r))
                        .child(probe(&self.probes, "reply"))
                        .into_any_element(),
                }
            }
        }
    }

    /// Block `p` of chat item `ix`, a reply drawn a row per block.
    fn reply_part(
        &mut self,
        s: &Session,
        ix: usize,
        item: &Item,
        p: crate::rows::Part,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match item {
            Item::Text(src) => self.reply_md(s, ix, src, Some(p), window, cx),
            _ => self.item(s, ix, item, window, cx),
        }
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
            // Sent by dent itself (carrying on): a quiet line, not yours.
            Item::User { text, .. } if dent_protocol::by_dent(text).is_some() => {
                crate::cards::note_line(
                    dent_protocol::by_dent(text).unwrap_or_default(),
                    "rotate-ccw-clock",
                    &t,
                )
                .into_any_element()
            }
            // A command, as it was run, until its answer comes.
            Item::User { text, files, .. } if crate::draft::sent_command(text, files).is_some() => {
                crate::cards::command_line(text, "terminal", t.quiet, &t).into_any_element()
            }
            Item::User {
                id: msg,
                text,
                files,
                at,
            } => {
                let (words, notes) = crate::draft::split_notes(text);
                let body: Vec<&str> = if words.is_empty() {
                    Vec::new()
                } else {
                    words.lines().collect()
                };
                let asking = self
                    .rewinding
                    .as_ref()
                    .is_some_and(|(sid, m)| *sid == s.id && m == msg);
                let busy = s.running();
                let has_files = s.checkpoints.contains(msg);
                let group = SharedString::from(format!("user-{ix}"));
                // A few emoji and nothing else read large, without a bubble.
                let big = files.is_empty() && notes.is_empty() && emoji_only(words);
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
                            .when(!big, |d| d.bg(t.tint()))
                            .when(big, |d| d.text_size(px(EMOJI_SIZE)).line_height(px(EMOJI_SIZE * 1.25)))
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .children({
                                // The message's pictures step together.
                                let pictures: crate::lightbox::Set = std::rc::Rc::new(
                                    files
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(j, f)| {
                                            Some(crate::lightbox::Shot {
                                                key: SharedString::from(format!("pic-{msg}-{j}")),
                                                path: picture_path(&s.id, f, &s.cwd)?,
                                            })
                                        })
                                        .collect(),
                                );
                                let cwd = std::path::Path::new(&s.cwd);
                                files
                                    .iter()
                                    .enumerate()
                                    .map(|(j, f)| {
                                        let key = SharedString::from(format!("pic-{msg}-{j}"));
                                        let at = pictures.iter().position(|p| p.key == key);
                                        let chip = sent_file(
                                            &crate::bridge::relative(cwd, f),
                                            at.map(|at| (&pictures, at)),
                                            &t,
                                        );
                                        if at.is_some() {
                                            return chip.into_any_element();
                                        }
                                        // Any other file opens as a link to it would:
                                        // a video plays, a PDF shows, text reads.
                                        let full = f.clone();
                                        chip.id(SharedString::from(format!("sent-open-{msg}-{j}")))
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.open_file(&full, cx)
                                            }))
                                            .into_any_element()
                                    })
                                    .chain(notes.iter().map(|l| {
                                        sent_note(l.trim_start_matches("› "), &t).into_any_element()
                                    }))
                                    .collect::<Vec<_>>()
                            })
                            .when(!body.is_empty(), |d| {
                                // Long messages fold, so the reply stays in view.
                                let open = self.is_open(&Fold::Message(msg));
                                let (shown, long) = user_shown(&body, open);
                                let key = Fold::Message(msg).key();
                                // A slash command reads as one.
                                let command = crate::draft::sent_command(&shown, &[]).is_some()
                                    && shown.len() < 80;
                                d.child(
                                    div()
                                        .when(!big, |d| d.ty(if command { CODE } else { READING }))
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
                                                this.toggle_fold(&key);
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
                        // Earlier versions of this message, by where it sits.
                        let versions = s
                            .versions
                            .iter()
                            .find(|(m, ..)| m == msg)
                            .map(|&(_, at, cur, total)| (at, cur, total));
                        if asking {
                            // Going back asks once, in place; the versions
                            // stay where they were.
                            let d = d.children(versions.map(|(at, cur, total)| {
                                self.version_stepper(&s.id, at, cur, total, cx)
                            }));
                            d.pb(px(size::CONTROL_SM)).child(div().ty(UI).text_color(t.ink).child(if busy {
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
                            // Earlier versions of this message: step through
                            // them, always shown.
                            let stepper = versions.map(|(at, cur, total)| {
                                self.version_stepper(&s.id, at, cur, total, cx)
                            });
                            d.child(
                                div()
                                    .flex()
                                    // The versions keep the bubble's edge;
                                    // narrow, the actions (shown on hover)
                                    // wrap under them rather than run past
                                    // the edge or leave them stranded.
                                    .flex_row_reverse()
                                    .flex_wrap()
                                    .w_full()
                                    .items_center()
                                    .gap(px(space::S1))
                                    .children(stepper)
                                    .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S1))
                                    // Narrow, they wrap within the column.
                                    .min_w_0()
                                    .flex_wrap()
                                    .justify_end()
                                    // In the gap below the message, part of
                                    // it: reaching for them keeps them shown.
                                    .min_h(px(size::CONTROL_SM))
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
                                    .child({
                                        // A prompt worth keeping becomes a
                                        // skill: it's in the / menu after.
                                        let (words, cwd) = (body.join("\n"), s.cwd.clone());
                                        ghost(
                                            SharedString::from(format!("save-skill-{ix}")),
                                            "Save as skill",
                                            &t,
                                        )
                                        .relative()
                                        .child(probe(&self.probes, format!("save-skill-{ix}")))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                                use crate::context::Do;
                                                cx.stop_propagation();
                                                this.open_context(
                                                    e.position,
                                                    vec![
                                                        (
                                                            "Save for this project",
                                                            Do::SaveSkill(words.clone(), Some(cwd.clone())),
                                                        ),
                                                        (
                                                            "Save for every project",
                                                            Do::SaveSkill(words.clone(), None),
                                                        ),
                                                    ],
                                                    cx,
                                                );
                                            }),
                                        )
                                    })
                                    .when(s.caps.fork, |d| d.child(
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
                                    ))
                                    .when(s.caps.rewind, |d| d.child(
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
                                    )),
                                    ),
                            )
                        }
                    })
                    .into_any_element()
            }
            Item::Text(src) => self.reply_md(s, ix, src, None, window, cx),
            Item::Tools {
                summary,
                lines,
                counted,
                pictures,
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
                        let call = lines[0].id.clone();
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
                                l.fold
                                    .as_ref()
                                    .is_none_or(|k| self.is_open(&Fold::Steps(k)))
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
                                // Past a few levels deep, rails stop
                                // stepping in, so the step keeps its room.
                                let deep = l.depth.min(MOST_RAILS);
                                for k in 1..=deep {
                                    let more = continues(&depths, i, k);
                                    let own = k == deep;
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
                                    let opened = self.is_open(&Fold::Steps(&key));
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
                                                    this.toggle_fold(&key);
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
                                                    self.call_detail(s, &l.id, &dt, cx)
                                                        .my(px(space::S1))
                                                })),
                                        )
                                        .into_any_element()
                                    }
                                    Some(report) => row
                                        .child(
                                            self.report(s, ix, &l.id, l.depth, report, window, cx),
                                        )
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
                    // What the calls brought back as pictures shows whether
                    // the line is open or not: a screenshot is the point.
                    .when(!pictures.is_empty(), |d| {
                        d.child(tool_pictures(&s.id, &s.cwd, ix, pictures, &self.probes, &t))
                    })
                    .into_any_element()
            }
            Item::Thinking(text, took) => {
                let nth = s.items[..ix]
                    .iter()
                    .filter(|i| matches!(i, Item::Thinking(..)))
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
                            .child(if *took >= 1000 {
                                format!("Thought for {}", duration(took / 1000))
                            } else {
                                format!(
                                    "Thought · {words} word{}",
                                    if words == 1 { "" } else { "s" }
                                )
                            })
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
                                // Raised off the card in both themes: on
                                // Carbon the magenta wash is about as light
                                // as a raised fill, so it lifts a step more.
                                .bg(t.lifted())
                                .border_1()
                                .border_color(t.hairline)
                                .ty(CODE)
                                .text_color(t.ink)
                                // A long edit scrolls inside the card.
                                .max_h(px(320.))
                                .id(SharedString::from(format!("ask-{ask}")))
                                .overflow_y_scroll()
                                .restrict_scroll_to_axis()
                                .child(code_lines(
                                    format!("ask-text-{ask}"),
                                    command.clone(),
                                    Vec::new(),
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
                                    .child(key_num("1", true, &t))
                                    .relative()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a, Some(&k1), Scope::Once, cx)
                                    }))
                                    .child(probe(&self.probes, "allow")),
                            )
                            .child(
                                button("allow-session", "Allow for this session", false, &t)
                                    .child(key_num("2", false, &t))
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
                                .when(*keeps && s.caps.project_rules, |b| {
                                    b.child(key_num("3", false, &t))
                                })
                                .relative()
                                .child(probe(&self.probes, "allow-project"));
                                if *keeps && s.caps.project_rules {
                                    let (a3, k4) = (s.id.clone(), ask.clone());
                                    el.with_tip(tip_owned(
                                        format!(
                                            "Saved to the project's own settings, where {} keeps its rules",
                                            s.agent_name
                                        ),
                                        t,
                                    ))
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a3, Some(&k4), Scope::Project, cx)
                                    }))
                                } else {
                                    crate::ui::off_owned(
                                        el,
                                        format!("{} has no rule it can save for this", s.agent_name),
                                        &t,
                                    )
                                }
                            })
                            .child(
                                button("deny", "Deny", false, &t)
                                    .child(key_num("4", false, &t))
                                    .relative()
                                    .on_press(cx.listener(move |this, _, window, cx| {
                                        this.deny(&d, Some(&k3), window, cx)
                                    }))
                                    .child(probe(&self.probes, "deny")),
                            ),
                    )
                    .child(ask_keys(
                        keyed,
                        "Press a number, or ↩ to allow once",
                        "Esc to answer by number",
                        &t,
                    ))
                    .into_any_element()
            }
            Item::Plan { id: ask, plan } => {
                let (a, d) = (s.id.clone(), s.id.clone());
                let (k1, k3) = (ask.clone(), ask.clone());
                // A long plan shows its start until expanded.
                let fold_key = Fold::Plan(ask).key();
                let open = self.is_open(&Fold::Plan(ask));
                let (shown, long) = plan_shown(plan, open);
                let blocks = self.markdown(&shown);
                let mut r = markdown::R {
                    live: false,
                    counted: true,
                    md: self.mdv.clone(),
                    owner: cx.entity_id(),
                    window,
                    cx,
                    scope: format!("{}/plan-{ask}", s.id),
                    bg: t.raised,
                    base: Some(std::path::PathBuf::from(&s.cwd)),
                    session: Some((s.id.clone(), s.cwd.clone())),
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
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .child(plan_view)
                            .when(long, |d| {
                                d.child(
                                    ghost(
                                        SharedString::from(format!("plan-fold-{ask}")),
                                        if open { "Collapse" } else { "Expand plan" },
                                        &t,
                                    )
                                    .self_start()
                                    .relative()
                                    .ml(px(-space::S2))
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.toggle_fold(&fold_key);
                                        cx.notify();
                                    }))
                                    .child(probe(&self.probes, "plan-fold")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                button("plan-go", "Start building", true, &t)
                                    .child(key_num("1", true, &t))
                                    .relative()
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.allow(&a, Some(&k1), Scope::Once, cx)
                                    }))
                                    .child(probe(&self.probes, "plan-go")),
                            )
                            .child(
                                button("plan-keep", "Keep planning", false, &t)
                                    .child(key_num("2", false, &t))
                                    .relative()
                                    .on_press(cx.listener(move |this, _, window, cx| {
                                        this.deny(&d, Some(&k3), window, cx)
                                    }))
                                    .child(probe(&self.probes, "plan-keep")),
                            ),
                    )
                    .child(ask_keys(
                        keyed,
                        "Press a number, or ↩ to start building · what you've typed goes with Keep planning",
                        "What you type is sent as what to change · Esc to answer by number",
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
                let answered = picked.iter().any(Option::is_some);
                let back = if many && answered { " · ⌫ back" } else { "" };
                let hint = match (keyed, multi, fixed, many) {
                    (true, true, ..) => format!("Press 1–{n} to tick, ↩ to continue{back}"),
                    (true, false, ..) => format!("Press 1–{n} to pick{back}"),
                    (false, _, true, _) => "Esc to pick by number".to_string(),
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .ty(LABEL)
                                    .text_color(t.faint)
                                    .child(hint),
                            )
                            // Its own questions only: the agent hears it went
                            // unanswered and carries on.
                            .when(!fixed, |d| {
                                let (sid, ask) = (s.id.clone(), ask.clone());
                                d.child(
                                    ghost(SharedString::from(format!("skip-{ask}")), "Skip", &t)
                                        .relative()
                                        .with_tip(tip("Carry on without an answer", None, t))
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            this.skip_question(&sid, ask.clone(), cx)
                                        }))
                                        .child(probe(&self.probes, "skip-question")),
                                )
                            }),
                    )
                    .into_any_element()
            }
            Item::Secret { id: ask, name, why } => {
                let empty = self.secret_input.read(cx).value().is_empty();
                let give =
                    |place: dent_protocol::SecretPlace, label: &'static str, primary: bool| {
                        let (sid, ask) = (s.id.clone(), ask.clone());
                        let key = format!("secret-{}", if primary { "project" } else { "session" });
                        button(SharedString::from(key.clone()), label, primary, &t)
                            .relative()
                            .map(|b| {
                                if empty {
                                    b.opacity(0.5).cursor_not_allowed().with_tip(tip(
                                        "Type or paste it first",
                                        None,
                                        t,
                                    ))
                                } else {
                                    b.on_press(cx.listener(move |this, _, window, cx| {
                                        this.give_secret(&sid, ask.clone(), place, window, cx)
                                    }))
                                }
                            })
                            .child(probe(&self.probes, key))
                    };
                let (sid, no) = (s.id.clone(), ask.clone());
                needs_card(&t)
                    .id(SharedString::from(format!("ask-{ask}")))
                    .child(needs_title(&format!("{} asks for a secret", s.agent_short), &t))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_baseline()
                            .gap(px(space::S2))
                            .child(div().ty(CODE).text_color(t.ink).child(name.clone()))
                            .when(!why.is_empty(), |d| {
                                d.child(div().ty(UI).text_color(t.quiet).child(why.clone()))
                            }),
                    )
                    .child(
                        div()
                            .relative()
                            .h(px(size::CONTROL + space::S2))
                            .px(px(space::S3))
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .rounded(px(radius::MD))
                            .border_1()
                            .border_color(t.hairline)
                            .bg(t.raised)
                            .child(icon("key-round", 13., t.quiet))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .ty(CODE)
                                    .text_color(t.ink)
                                    .child(gpui_kit::base::input::Input::new(&self.secret_input)),
                            )
                            .child(probe(&self.probes, "secret-field")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(space::S2))
                            .child(give(
                                dent_protocol::SecretPlace::Project,
                                "Save to the project's .env",
                                true,
                            ))
                            .child(give(
                                dent_protocol::SecretPlace::Session,
                                "This session only",
                                false,
                            ))
                            .child(
                                ghost("secret-no", "Don't give it", &t)
                                    .relative()
                                    .on_press(cx.listener(move |this, _, window, cx| {
                                        this.refuse_secret(&sid, no.clone(), window, cx)
                                    }))
                                    .child(probe(&self.probes, "secret-no")),
                            ),
                    )
                    .child(div().ty(LABEL).text_color(t.faint).child(
                        format!(
                            "Kept on this Mac where you choose. {} is told where it is, never what it is.",
                            s.agent_short
                        ),
                    ))
                    .into_any_element()
            }
            Item::Note {
                icon: name, text, ..
            } => {
                // The latest failure, while a usage limit is used up, says
                // which and when it starts over, in plain words.
                let latest_failure = *name == "circle-alert"
                    && !s.items[ix + 1..]
                        .iter()
                        .any(|i| matches!(i, Item::User { .. }));
                // Only a note about a limit becomes one: a crash while some
                // other limit is used up keeps its own words and actions.
                let about_limit = text.to_lowercase().contains("limit");
                let limit = (latest_failure && about_limit)
                    .then(|| self.limit_hit(&s.agent_id))
                    .flatten()
                    .map(|(which, at)| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs() as i64);
                        (
                            format!("Your {which} is used up"),
                            format!(
                                "It resets at {}, in {}.",
                                crate::data::clock(at as u64 * 1000),
                                crate::data::span((at - now).max(0) as u64)
                            ),
                        )
                    });
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::S2))
                    // Its actions' words end on the column's edge, as
                    // the composer's and the cards' do.
                    .mr(px(-space::S2))
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
                            // A used-up limit says so in plain words, which
                            // and when, in place of the agent's own line.
                            .map(|d| match limit {
                                Some((head, when)) => d
                                    .child(div().text_color(t.ink).child(head))
                                    .child(div().child(when)),
                                None => d.child(crate::select::text(
                                    SharedString::from(format!("note-{ix}")),
                                    text.clone(),
                                    &t,
                                )),
                            }),
                    )
                    // An MCP server in trouble: its sheet reconnects it or
                    // signs in.
                    .when(
                        mcp_note(text)
                            && !matches!(s.items.get(ix + 1), Some(Item::Note { text: next, .. }) if mcp_note(next)),
                        |d| {
                        let sid = s.id.clone();
                        d.child(
                            ghost(
                                SharedString::from(format!("mcp-{ix}")),
                                "Manage servers",
                                &t,
                            )
                            .relative()
                            .child(probe(&self.probes, format!("mcp-{ix}")))
                            .on_press(cx.listener(
                                move |this, _, window, cx| {
                                    this.run_command(&sid, "/mcp", window, cx)
                                },
                            )),
                        )
                    },
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
                            match self.limit_hit(&s.agent_id).filter(|_| about_limit) {
                                Some((which, at)) => {
                                    let at = at.max(0) as u64;
                                    let when = crate::data::clock(at * 1000);
                                    let (a, b) = (s.id.clone(), s.id.clone());
                                    // Carrying on by itself once the limit
                                    // resets; the daemon holds it, so it
                                    // goes with the app closed.
                                    let resume: AnyElement = match s.resume_at {
                                        Some(_) => div()
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .px(px(space::S2))
                                                    .text_color(t.ink)
                                                    .child(format!("Continues at {when}")),
                                            )
                                            .child(
                                                ghost(
                                                    SharedString::from(format!("unresume-{ix}")),
                                                    "Cancel",
                                                    &t,
                                                )
                                                .on_press(cx.listener(move |this, _, _, cx| {
                                                    this.resume_at(&a, None, cx)
                                                })),
                                            )
                                            .into_any_element(),
                                        None => ghost(
                                            SharedString::from(format!("resume-{ix}")),
                                            format!("Continue at {when}"),
                                            &t,
                                        )
                                        .relative()
                                        .child(probe(&self.probes, format!("resume-{ix}")))
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            this.resume_at(&b, Some(at), cx)
                                        }))
                                        .into_any_element(),
                                    };
                                    // A limit on one model leaves the others.
                                    let other = which.contains("Opus") || which.contains("Sonnet");
                                    d.child(resume)
                                        .when(other, |d| {
                                            d.child(
                                                ghost(
                                                    SharedString::from(format!("other-model-{ix}")),
                                                    "Use another model",
                                                    &t,
                                                )
                                                .on_press(cx.listener(|this, _, _, cx| {
                                                    this.menu =
                                                        Some(crate::app::Menu::SessionModel);
                                                    cx.notify();
                                                })),
                                            )
                                        })
                                }
                                None => {
                                    let again = s.id.clone();
                                    d.child(
                                        ghost(
                                            SharedString::from(format!("carry-on-{ix}")),
                                            "Continue",
                                            &t,
                                        )
                                        .relative()
                                        .child(probe(&self.probes, format!("carry-on-{ix}")))
                                        .with_tip(crate::ui::tip_owned(
                                            "Pick up from where it was cut off".to_string(),
                                            t,
                                        ))
                                        .on_press(
                                            cx.listener(move |this, _, _, cx| {
                                                this.carry_on(&again, cx)
                                            }),
                                        ),
                                    )
                                    .child(
                                        ghost(
                                            SharedString::from(format!("retry-{ix}")),
                                            "Try again",
                                            &t,
                                        )
                                        .relative()
                                        .child(probe(&self.probes, format!("retry-{ix}")))
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            this.retry(&sid, cx)
                                        })),
                                    )
                                }
                            }
                        },
                    )
                    .into_any_element()
            }
            Item::Compacted {
                auto,
                before,
                after,
            } => self.compacted_line(ix, &s.agent_short, *auto, *before, *after),
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
                // Waiting on this device for its machine (decision 0063): it
                // can't move ahead of anything or go into a turn.
                let outgoing = self.outbox.iter().find(|o| o.id == *msg).cloned();
                // The waiting message before this one, which it can go ahead of.
                let ahead = ix
                    .checked_sub(1)
                    .and_then(|p| match s.items.get(p) {
                        Some(Item::Queued { id, .. }) => Some(id.clone()),
                        _ => None,
                    })
                    .filter(|_| outgoing.is_none());
                // Folded pastes show as their chips again, and long text
                // folds as a sent message does.
                let open = self.is_open(&Fold::Message(msg));
                let (refolded, pasted) = match self.sent_drafts.peek(msg) {
                    Some(d) => {
                        let (words, chips) = d.chips_apart(&self.pastes);
                        // Its notes, as sent, after its words.
                        let notes = crate::draft::split_notes(text).1.join("\n");
                        (
                            [words, notes]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join("\n"),
                            chips,
                        )
                    }
                    None => (text.clone(), Vec::new()),
                };
                let body: Vec<&str> = refolded.lines().collect();
                let (shown, long) = user_shown(&body, open);
                let fold_key = msg.clone();
                let (sid3, msg3) = (s.id.clone(), msg.clone());
                let (sid, msg) = (s.id.clone(), msg.clone());
                let (sid2, msg2) = (s.id.clone(), msg.clone());
                let (sid4, msg4) = (s.id.clone(), msg.clone());
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
                            // Its folded pastes and the files it carries, as
                            // they showed in the box, and first, as a sent
                            // message has them.
                            .when(!attachments.is_empty() || !pasted.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(space::S1H))
                                        .children(pasted.into_iter().map(|label| {
                                            small_chip(ChipFace::Icon("file-text"), label, &t)
                                        }))
                                        .children(attachments.iter().map(|p| {
                                            let a =
                                                crate::app::Attachment::File { path: p.clone() };
                                            small_chip(file_face(p), a.label(), &t)
                                        })),
                                )
                            })
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
                                            this.toggle_fold(&fold_key);
                                            cx.notify();
                                        }))
                                        .child(if open {
                                            "Show less"
                                        } else {
                                            "Show full message"
                                        }),
                                )
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
                                    .child(if let Some(o) = &outgoing {
                                        if o.held {
                                            format!(
                                                "Written {} ago · sends when you say",
                                                crate::outbox::age(o.at, crate::cards::now_ms())
                                            )
                                        } else {
                                            format!(
                                                "Waiting for {} · sends when it's back",
                                                crate::data::machine_short(&s.machine)
                                            )
                                        }
                                    } else if ahead.is_some() {
                                        "Queued".into()
                                    } else if matches!(
                                        s.activity,
                                        Some(dent_protocol::Activity::SettingUp { .. })
                                    ) {
                                        "Queued · sends once the worktree is set up".into()
                                    } else {
                                        "Queued · sends when this reply finishes".into()
                                    }),
                            )
                            // The actions keep together: a narrow column
                            // puts them all under the words, never one alone.
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(px(space::S2))
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
                                        ghost(
                                            SharedString::from(format!("unqueue-{ix}")),
                                            "Edit",
                                            &t,
                                        )
                                        .relative()
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            this.unqueue(&sid, &msg, cx)
                                        }))
                                        .child(probe(&self.probes, format!("unqueue-{ix}"))),
                                    )
                                    // Nothing runs to send it into while the
                                    // worktree is set up: it goes first then.
                                    .children(
                                        // Nor into an agent that can't be steered.
                                        (s.caps.steer
                                            && outgoing.is_none()
                                            && !matches!(
                                                s.activity,
                                                Some(dent_protocol::Activity::SettingUp { .. })
                                            ))
                                        .then(|| {
                                            ghost(
                                                SharedString::from(format!("steer-{ix}")),
                                                "Send now",
                                                &t,
                                            )
                                            .relative()
                                            .on_press(cx.listener(move |this, _, _, cx| {
                                                this.steer_queued(&sid2, &msg2, cx)
                                            }))
                                            .child(probe(&self.probes, format!("steer-{ix}")))
                                        }),
                                    )
                                    // Written long ago: it goes when you say.
                                    .children(outgoing.as_ref().filter(|o| o.held).map(|o| {
                                        let id = o.id.clone();
                                        ghost(
                                            SharedString::from(format!("send-held-{ix}")),
                                            "Send now",
                                            &t,
                                        )
                                        .on_press(
                                            cx.listener(move |this, _, _, cx| {
                                                this.send_held(&id, cx)
                                            }),
                                        )
                                    }))
                                    .child(
                                        ghost(
                                            SharedString::from(format!("remove-{ix}")),
                                            "Remove",
                                            &t,
                                        )
                                        .relative()
                                        .on_press(cx.listener(move |this, _, _, cx| {
                                            this.remove_queued(&sid4, &msg4, cx)
                                        }))
                                        .child(probe(&self.probes, format!("remove-{ix}"))),
                                    ),
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
        // A page the agent showed with dent's tool lives in the chat itself.
        if a.kind == Kind::Html && a.path.starts_with(dent_client_core::timeline::VISUAL_DIR) {
            return self.inline_page(&a, version, cx);
        }
        let latest = a.versions.len();
        let showing =
            self.panel_open && self.tab(s) == Tab::Art(art) && self.version(&a) == version;
        let old = version < latest;
        let file = a.path.rsplit('/').next().unwrap_or(a.path).to_string();
        let thumb = (a.kind == Kind::Image)
            .then(|| self.picture(art, version))
            .flatten();
        // A media file is the session's machine's: how it stands there, and
        // (another machine's) a copy here to make a still from, when it's
        // small enough to copy whole.
        let media = (a.kind == Kind::Media).then(|| crate::files::Ask::new(&s.id, a.path, &s.cwd));
        let media_stat = media.as_ref().map(crate::files::stat);
        if let Some(crate::files::Look::Here(dent_protocol::FileStat::Missing { .. })) = &media_stat
        {
            // Named in a reply, but not there: no card.
            return div().into_any_element();
        }
        let media_local = media.as_ref().and_then(|ask| {
            crate::files::copy(ask, POSTER_MAX)
                .shown()
                .map(|p| p.display().to_string())
        });
        // A video or a PDF shows a still of itself once it's made.
        let still = (a.kind == Kind::Media && crate::data::media_kind(a.path) != Some("Sound"))
            .then(|| {
                media_local
                    .as_deref()
                    .and_then(|p| self.poster(crate::data::intern(p), cx))
            })
            .flatten();
        // A page shows itself above its card, as the viewer last drew it.
        let page = (a.kind == Kind::Html)
            .then(|| self.stills.get(art))
            .flatten()
            .filter(|(v, _)| *v == version)
            .map(|(_, p)| p.clone());
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
        } else if crate::data::visual_title(a.path).is_some() {
            format!("Visual · v{version}")
        } else if a.kind == Kind::Media {
            let size = match &media_stat {
                Some(crate::files::Look::Here(dent_protocol::FileStat::File { stamp, .. }))
                | Some(crate::files::Look::Stale(dent_protocol::FileStat::File {
                    stamp, ..
                })) => crate::viewer::bytes(stamp.size),
                _ => String::new(),
            };
            // How it stood when its machine went away: said so.
            let kept = matches!(
                media_stat,
                Some(crate::files::Look::Stale(
                    dent_protocol::FileStat::File { .. }
                ))
            ) && !crate::files::online(dent_client_core::fleet::split(&s.id).0)
                && media
                    .as_ref()
                    .is_some_and(|ask| crate::files::kept_copy(ask).is_some());
            format!(
                "{} · {} · {size}{}",
                crate::bridge::relative(std::path::Path::new(&s.cwd), a.path),
                crate::data::media_kind(a.path).unwrap_or_default(),
                if kept { " · offline copy" } else { "" },
            )
        } else {
            format!("{} · v{version}", a.path)
        };
        let card = div()
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
                    .map(|d| match (thumb, still) {
                        (Some(picture), _) => {
                            d.child(img(picture).size_full().object_fit(ObjectFit::Cover))
                        }
                        (None, Some(still)) => {
                            d.child(img(still).size_full().object_fit(ObjectFit::Cover))
                        }
                        (None, None) => d.child(icon(
                            crate::data::file_icon(a.path).unwrap_or(a.kind.icon()),
                            18.,
                            if old { t.faint } else { t.ink },
                        )),
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
            .child(probe(&self.probes, format!("made-{art}-{version}")));
        let Some(picture) = page else {
            return card.into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(
                div()
                    .id(SharedString::from(format!("made-still-{art}-{version}")))
                    .relative()
                    .w_full()
                    .h(px(PAGE_STILL_H))
                    .rounded(px(radius::LG))
                    .border_1()
                    .border_color(t.hairline)
                    .bg(t.panel)
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|d| d.border_color(t.ink.opacity(0.3)))
                    .on_press(
                        cx.listener(move |this, _, _, cx| this.open_art(art, Some(version), cx)),
                    )
                    .child(img(picture).size_full().object_fit(ObjectFit::Contain))
                    .child(probe(&self.probes, format!("made-still-{art}"))),
            )
            .child(card)
            .into_any_element()
    }

    /// "Worked for 2m 14s ›": a finished turn's work, folded, opening
    /// in place.
    fn worked_line(
        &self,
        s: &Session,
        took: u64,
        user: usize,
        open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let Some(Item::User { id, .. }) = s.items.get(user) else {
            return div().into_any_element();
        };
        let key = crate::data::Fold::Turn(id).key();
        let words = if took >= 1000 {
            format!("Worked for {}", duration(took / 1000))
        } else {
            "Worked for a moment".to_string()
        };
        let group = SharedString::from(format!("worked-g-{user}"));
        div()
            .id(SharedString::from(format!("worked-{user}")))
            .group(group.clone())
            .relative()
            .flex()
            .items_center()
            .gap(px(space::S1H))
            .ty(UI)
            .text_color(t.quiet)
            .cursor_pointer()
            .on_press(cx.listener(move |this, _, _, cx| {
                // Opening or closing never moves what's above: the line stays
                // where it was and its work unfolds below it, rather than the
                // chat holding to its end and pushing the line out of view.
                let top = this.transcript.logical_scroll_top();
                this.transcript.pause_following_tail();
                this.transcript.scroll_to(top);
                this.toggle_fold(&key);
                cx.notify();
            }))
            .child(
                div()
                    .group_hover(group, |d| d.text_color(t.ink))
                    .child(words),
            )
            .child(
                div()
                    .flex_none()
                    .child(crate::ui::disclosure("worked", open, 11., t.faint)),
            )
            .child(probe(&self.probes, "worked"))
            .into_any_element()
    }

    /// A page the agent showed (`show_visual`), in the chat at its own
    /// height on the chat's own background: live while wholly in view and
    /// nothing floats over it, its still otherwise (`inline.rs`). Under it,
    /// its name and ways to see it bigger.
    fn inline_page(
        &mut self,
        a: &crate::data::Artifact,
        version: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let art = a.id;
        let Some(bytes) = a.versions.get(version.wrapping_sub(1)).cloned() else {
            return div().into_any_element();
        };
        let key = (art, version);
        // No web view can sit in the chat here (Wayland): the page is a card
        // that opens it where one can.
        if self.inline.borrow().broken {
            return div()
                .id(SharedString::from(format!("inline-card-{art}-{version}")))
                .relative()
                .w_full()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S3))
                .py(px(space::S2))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(t.hairline)
                .bg(t.raised)
                .cursor_pointer()
                .hover(|d| d.border_color(t.ink.opacity(0.3)))
                .on_press(cx.listener(move |this, _, _, cx| this.open_art(art, Some(version), cx)))
                .child(icon("app-window", 16., t.quiet))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .ty(UI)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.ink)
                        .child(a.title),
                )
                .child(div().flex_none().ty(UI).text_color(t.quiet).child("Open"))
                .child(probe(&self.probes, "inline-card"))
                .into_any_element();
        }
        let (h, still) = {
            let i = self.inline.borrow();
            (
                crate::inline::height(i.heights.get(&key).copied()),
                i.still(key),
            )
        };
        let page = Rc::new(crate::inline::Page {
            path: a.path,
            version,
            bytes: bytes.clone(),
            dir: a.dir.clone(),
            far: crate::viewer::far_root(a),
        });
        // Where it may show live: the chat's view, less the Latest pill's
        // room over its foot while you read back.
        // (Read before the list draws: its state is busy while rows do.)
        let mut view = self.chat_view.0;
        if !self.chat_view.1 {
            view.size.height -= px(size::CONTROL + space::S2 * 2.);
        }
        let inline = self.inline.clone();
        let name = a.title;
        let open_name = a.path.rsplit('/').next().unwrap_or(a.path).to_string();
        div()
            .w_full()
            .flex()
            .flex_col()
            // Its name and actions sit apart from the page's own last line.
            .gap(px(space::S2))
            .child(
                div()
                    .id(SharedString::from(format!("inline-{art}-{version}")))
                    .relative()
                    .w_full()
                    .h(px(h))
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                let whole = view.size.height > px(0.)
                                    && bounds.top() >= view.top() - px(0.5)
                                    && bounds.bottom() <= view.bottom() + px(0.5);
                                // Taller than the view, it can never be all in
                                // view: it's drawn off screen for its still.
                                let tall = bounds.size.height > view.size.height
                                    && bounds.bottom() > view.top()
                                    && bounds.top() < view.bottom();
                                inline
                                    .borrow_mut()
                                    .want(key, &page, bounds, whole, tall, window);
                                bounds
                            },
                            move |_, bounds, window, _| {
                                // Its last picture underneath, at its own size from
                                // the top: what shows while the live page can't.
                                if let Some(image) = still {
                                    let s = image.size(0);
                                    let scale = window.scale_factor();
                                    let at = Bounds::new(
                                        bounds.origin,
                                        size(
                                            px(s.width.0 as f32 / scale),
                                            px(s.height.0 as f32 / scale),
                                        ),
                                    );
                                    window.with_content_mask(
                                        Some(ContentMask { bounds }),
                                        |window| {
                                            let _ = window.paint_image(
                                                at,
                                                at,
                                                Corners::default(),
                                                image,
                                                0,
                                                false,
                                            );
                                        },
                                    );
                                }
                            },
                        )
                        .size_full(),
                    )
                    .child(probe(&self.probes, "inline-page")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S1))
                    .ml(px(-space::S2))
                    .child(
                        div()
                            .px(px(space::S2))
                            .min_w_0()
                            .truncate()
                            .ty(LABEL)
                            .text_color(t.faint)
                            .child(name),
                    )
                    .child(
                        ghost(
                            SharedString::from(format!("inline-open-{art}-{version}")),
                            "Open beside",
                            &t,
                        )
                        .relative()
                        .with_tip(tip("Show it beside the chat, larger", None, t))
                        .on_press(
                            cx.listener(move |this, _, _, cx| {
                                this.open_art(art, Some(version), cx)
                            }),
                        )
                        .child(probe(&self.probes, "inline-open")),
                    )
                    .child(
                        ghost(
                            SharedString::from(format!("inline-browser-{art}-{version}")),
                            "Open in browser",
                            &t,
                        )
                        .on_press(cx.listener(move |this, _, _, cx| {
                            let path = std::env::temp_dir()
                                .join(format!("dent-visual-v{version}-{open_name}"));
                            // With dent's colours and type, as the chat draws it.
                            let page = crate::inline::browser_copy(&bytes, &this.t);
                            if std::fs::write(&path, page).is_ok() {
                                cx.open_url(&format!("file://{}", path.display()));
                            }
                        })),
                    ),
            )
            .into_any_element()
    }

    /// "‹ 2 / 3 ›" under a message with earlier versions.
    fn version_stepper(
        &self,
        sid: &str,
        at: usize,
        cur: usize,
        total: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let arrow =
            |older: bool, on: bool| {
                let sid = sid.to_string();
                div()
                    .id(SharedString::from(format!(
                        "version-{}-{at}",
                        if older { "older" } else { "newer" }
                    )))
                    .relative()
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .child(icon(
                        if older {
                            "chevron-left"
                        } else {
                            "chevron-right"
                        },
                        12.,
                        if on { t.quiet } else { t.faint },
                    ))
                    .when(on, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(t.hover()))
                            .active(|d| d.bg(t.press()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.step_version(&sid, at, older, cx)
                            }))
                    })
                    .child(probe(
                        &self.probes,
                        format!("version-{}", if older { "older" } else { "newer" }),
                    ))
            };
        div()
            .flex()
            .items_center()
            .ty(LABEL)
            .text_color(t.quiet)
            .child(arrow(true, cur > 1))
            .child(div().px(px(space::S1)).child(format!("{cur} / {total}")))
            .child(arrow(false, cur < total))
            .into_any_element()
    }

    /// In place of the reply box while an earlier version shows: it's read
    /// only, so back to the latest, or carry it on as a new session.
    fn old_version_bar(&self, s: &Session, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let (a, b) = (s.id.clone(), s.id.clone());
        div()
            .w_full()
            .max_w(px(size::COLUMN + space::S8 * 2.))
            .mx_auto()
            .px(px(space::S8))
            .pb(px(space::S4))
            .child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S3))
                    .rounded(px(radius::XL))
                    .border_1()
                    .border_color(t.hairline)
                    .flex()
                    // Narrow, the choices go under the words rather than
                    // squeezing them into a column.
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::S2))
                    .child(icon("rotate-ccw-clock", 14., t.quiet))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(220.))
                            .ty(UI)
                            .text_color(t.quiet)
                            .child("Viewing an earlier version. Replies go to the latest."),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                ghost("fork-version", "Continue from here", &t)
                                    .relative()
                                    .child(probe(&self.probes, "fork-version"))
                                    .on_press(
                                        cx.listener(move |this, _, _, cx| {
                                            this.fork_version(&a, cx)
                                        }),
                                    ),
                            )
                            .child(
                                crate::ui::button("latest-version", "Back to latest", true, &t)
                                    .relative()
                                    .child(probe(&self.probes, "latest-version"))
                                    .on_press(cx.listener(move |this, _, _, cx| {
                                        this.latest_version(&b, cx)
                                    })),
                            ),
                    ),
            )
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
            .is_some_and(|at| at.elapsed() < crate::app::JUST_SENT);
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
        let permission = self.permission_shown(s).label().to_string();
        // On a machine that's away, whatever is typed waits here for it.
        let away = self
            .machines
            .iter()
            .find(|m| m.id == dent_client_core::fleet::split(&s.id).0)
            .is_some_and(|m| m.away().is_some());
        let hint_text = if away {
            "↩ waits for it"
        } else if s.status == Status::Needs {
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
        } else if working && s.caps.steer {
            "↩ queue · ⌘ ↩ steer"
        } else if working {
            "↩ queue"
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
            - space::S1
            // The machine chip, with more than one machine: its glyph,
            // name, chevron and padding.
            - if self.has_machines() {
                let at = dent_client_core::fleet::split(&s.id).0;
                let name = crate::data::machine_short(&self.machine_name(at));
                52. + name.chars().count() as f32 * 7.2 + space::S1
            } else {
                0.
            };
        let effort_now = if self.selected.as_deref() == Some(s.id.as_str()) {
            self.effort_shown(s)
        } else {
            s.effort.clone()
        };
        let effort_word = match effort_now.as_deref() {
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
            .children(self.background_strip(s, cx))
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
                            // Where it runs, with more than one machine:
                            // choosing another moves it there.
                            .children(self.has_machines().then(|| {
                                let at = dent_client_core::fleet::split(&s.id).0;
                                let name = crate::data::machine_short(&self.machine_name(at));
                                keep(true, self.chip(Menu::SessionMachine, "", &name, false, cx))
                            }))
                            .child(keep(
                                !fit.fits,
                                self.chip(
                                    Menu::SessionModel,
                                    crate::agents::mark(&s.agent_id).0,
                                    &labels[0],
                                    false,
                                    cx,
                                ),
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
                    .children(self.stash_menu(false, cx))
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
            // Where it works, under the box: the branch, and a worktree's
            // folder when it has one of its own.
            .children(s.branch.clone().map(|b| {
                let worktree = s.root.is_some();
                div()
                    .relative()
                    .mt(px(space::S2))
                    .px(px(space::S2))
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .ty(LABEL)
                    .text_color(t.quiet)
                    .child(icon(
                        if worktree {
                            "folder-git-2"
                        } else {
                            "git-branch"
                        },
                        12.,
                        t.quiet,
                    ))
                    .child(div().min_w_0().truncate().child(b))
                    .when(worktree, |d| {
                        d.child(div().text_color(t.faint).child("·"))
                            .child(div().flex_none().text_color(t.faint).child("Worktree"))
                    })
                    .child(probe(&self.probes, "branch-strip"))
            }))
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
                Item::Approval { .. }
                    | Item::Question { .. }
                    | Item::Plan { .. }
                    | Item::Secret { .. }
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
            .relative()
            .child(probe(&self.probes, "plan-strip"))
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
                            // Held up on you, the step waits as the session does.
                            "in_progress" => status_square(
                                if s.status == Status::Needs {
                                    Status::Needs
                                } else {
                                    Status::Working
                                },
                                &t,
                            )
                            .into_any_element(),
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

    /// What the agent left running in the background (a dev server, a
    /// watcher, a subagent), over the composer while it runs, each with its
    /// output when there is a file of it, and a Stop.
    fn background_strip(&self, s: &Session, cx: &mut Context<Self>) -> Option<AnyElement> {
        if s.background.is_empty() {
            return None;
        }
        let t = self.t;
        let n = s.background.len();
        let stoppable = s.caps.stop_task;
        let rows = s.background.iter().map(|task| {
            let kind = match task.kind.as_str() {
                "local_bash" => "shell",
                "local_agent" => "subagent",
                "" => "",
                _ => "task",
            };
            let output = task
                .call
                .as_deref()
                .and_then(|call| call_output(s, call))
                .and_then(|out| output_file(&out))
                .filter(|p| {
                    matches!(
                        crate::files::stat(&crate::files::Ask::new(&s.id, p, &s.cwd)),
                        crate::files::Look::Here(dent_protocol::FileStat::File { .. })
                            | crate::files::Look::Stale(dent_protocol::FileStat::File { .. })
                    )
                });
            let (sid, tid) = (s.id.clone(), task.id.clone());
            div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .ty(UI)
                .child(
                    div()
                        .w(px(14.))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(status_square(Status::Working, &t)),
                )
                // What kind it is stays with its name, small, apart from
                // the actions at the end.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_baseline()
                        .gap(px(space::S1H))
                        .child(div().min_w_0().truncate().text_color(t.ink).child(
                            if task.description.trim().is_empty() {
                                "Background work".to_string()
                            } else {
                                task.description.clone()
                            },
                        ))
                        .when(!kind.is_empty(), |d| {
                            d.child(div().flex_none().ty(LABEL).text_color(t.faint).child(kind))
                        }),
                )
                .when_some(output, |d, path| {
                    d.child(
                        ghost(
                            SharedString::from(format!("bg-out-{}", task.id)),
                            "Output",
                            &t,
                        )
                        .on_press(cx.listener(move |this, _, _, cx| this.open_file(&path, cx))),
                    )
                })
                // Only where the agent can stop one piece on its own.
                .when(stoppable, |d| {
                    d.child(
                        ghost(
                            SharedString::from(format!("bg-stop-{}", task.id)),
                            "Stop",
                            &t,
                        )
                        .child(probe(&self.probes, format!("bg-stop-{}", task.id)))
                        .on_press(cx.listener(move |this, _, _, cx| {
                            this.live.send(dent_protocol::Request::StopTask {
                                session: sid.clone(),
                                task: tid.clone(),
                            });
                            cx.notify();
                        })),
                    )
                })
        });
        let all: Vec<String> = s.background.iter().map(|t| t.id.clone()).collect();
        let sid = s.id.clone();
        Some(
            div()
                .id("background-strip")
                .mb(px(space::S2))
                .px(px(space::S4))
                .py(px(space::S2))
                .rounded(px(radius::XL))
                .border_1()
                .border_color(t.hairline)
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(probe(&self.probes, "background-strip"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .ty(UI)
                        .text_color(t.quiet)
                        .child(icon("activity", 13., t.quiet))
                        .child(div().flex_1().child(if n == 1 {
                            "Running in the background".to_string()
                        } else {
                            format!("Running in the background · {n}")
                        }))
                        .when(n > 1 && stoppable, |d| {
                            d.child(ghost("bg-stop-all", "Stop all", &t).on_press(cx.listener(
                                move |this, _, _, cx| {
                                    for task in &all {
                                        this.live.send(dent_protocol::Request::StopTask {
                                            session: sid.clone(),
                                            task: task.clone(),
                                        });
                                    }
                                    cx.notify();
                                },
                            )))
                        }),
                )
                .children(rows)
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
                // The shared chip, its x beside it.
                let what = small_chip(
                    match &a {
                        crate::app::Attachment::File { path } if shown.is_some() => {
                            ChipFace::Picture(std::path::PathBuf::from(path))
                        }
                        _ => ChipFace::Icon(a.icon()),
                    },
                    a.label(),
                    &t,
                )
                .bg(gpui_kit::transparent_black())
                .pr_0()
                .relative()
                .when(shown.is_some(), |d| {
                    d.child(probe(&self.probes, "pic-chip"))
                });
                div()
                    .id(SharedString::from(format!("att-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(space::HALF))
                    .max_w(px(420.))
                    // A narrow box narrows the chip: its name truncates.
                    .min_w_0()
                    .h(px(size::CONTROL_SM))
                    .pr(px(space::HALF))
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
        // Many files scroll in three rows rather than push the box (and
        // its send button) off the window.
        let rows = 3.;
        (!chips.is_empty()).then(|| {
            div()
                .id("att-chips")
                .flex()
                .flex_wrap()
                .gap(px(space::S1H))
                .max_h(px(rows * size::CONTROL_SM + (rows - 1.) * space::S1H))
                .overflow_y_scroll()
                .restrict_scroll_to_axis()
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
                        small_chip(ChipFace::Icon("file-text"), tok.token().label().clone(), &t)
                            .cursor_pointer()
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
        self.over_chat.set(true);
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
                let row = if mention && item.session.is_some() {
                    // Another session: its title, then what it is.
                    row.child(icon("message-square", 13., t.quiet))
                        .child(
                            div()
                                .flex_none()
                                .max_w(relative(0.6))
                                .truncate()
                                .ty(UI)
                                .text_color(t.ink)
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .ty(UI)
                                .text_color(t.quiet)
                                .child(item.about.clone()),
                        )
                } else if mention {
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
            "Looking up the agent's commands…"
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
                            .restrict_scroll_to_axis()
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
        self.want_repo();
        let t = self.t;
        let text = self.starter.read(cx).value();
        let empty = text.trim().is_empty() && self.attachments.is_empty();
        let d = self.defaults.clone();
        let project = match self.bringing_here() {
            Some(c) => c.bring.name.clone(),
            None => d
                .project
                .as_deref()
                .map(Self::project_name)
                .unwrap_or_else(|| "No project".into()),
        };
        let model = crate::bridge::model_name(self.new_models(), d.model.as_deref());
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
        // How it runs sits in the box, as in a session's composer: words
        // give way (model's "Default ·", effort's, then permission's) before
        // the row ever wraps; each chip keeps its icon and says itself on
        // hover. Where it runs is a tray tucked under the box, one line.
        let column = self.chat_w.min(size::COLUMN + space::S8 * 2.) - space::S8 * 2.;
        // With more than one agent ready, which one runs comes first.
        let pick = self.pickable_agents().len() > 1;
        let agent = self.new_agent_info();
        let mut room = column - space::S4 * 2. + space::S2 - size::CONTROL - space::S1 - START_W;
        if pick {
            room -= AGENT_CHIP_W;
        }
        let fit = fit_footer(
            room,
            [
                (
                    model.clone(),
                    model.trim_start_matches("Default · ").to_string(),
                ),
                (d.permission.label().to_string(), String::new()),
                (
                    effort_label(d.effort.as_deref()).to_string(),
                    match d.effort.as_deref() {
                        None => "Effort".to_string(),
                        e => effort_name(e).to_string(),
                    },
                ),
            ],
            false,
            "",
        );
        let agent_chip = pick.then(|| {
            self.chip(
                Menu::Agent,
                crate::agents::mark(&agent.id).0,
                &agent.short,
                false,
                cx,
            )
        });
        let how = [
            self.chip(
                Menu::Model,
                crate::agents::mark(&agent.id).0,
                &fit.labels[0],
                false,
                cx,
            ),
            self.chip(Menu::Permission, "shield", &fit.labels[1], false, cx),
            self.chip(Menu::Effort, "gauge", &fit.labels[2], false, cx),
        ];
        let tray = self.where_tray(&project, d.project.is_none(), pickup, column, cx);
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
                    .children(self.agent_banner(cx))
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
                                    .gap(px(space::S1))
                                    .ml(px(-space::S2))
                                    .child(self.attach_button(cx))
                                    .children(agent_chip.map(|c| keep(true, c)))
                                    .children(how.into_iter().map(|c| keep(true, c)))
                                    .child(div().flex_1())
                                    .child(start),
                            )
                            .child(probe(&self.probes, "starter"))
                            .children(self.slash_menu(true, cx))
                            .children(self.stash_menu(true, cx))
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
                            .w_full()
                            .relative()
                            .child(tray)
                            .child(probe(&self.probes, "ambient-chips")),
                    ),
            )
            .child(div().flex_1())
            // As much room below as above the header.
            .child(div().h(px(size::HEADER)).flex_none())
            .into_any_element()
    }

    /// Where the session runs, in a tray tucked under the box: the project,
    /// in a repository the worktree and the branch it starts from, and the
    /// terminal sessions to pick up at the end. One line at any width: the
    /// pick-up and the worktree give up their words first, then the branch
    /// gives up letters.
    fn where_tray(
        &mut self,
        project: &str,
        no_project: bool,
        pickup: Option<AnyElement>,
        column: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let text = |s: &str| s.chars().count() as f32 * 7.2 + 52.;
        let room = column - space::S4 * 2. - space::S2 * 2.;
        let pickup_w = if pickup.is_some() {
            text("Pick up from terminal")
        } else {
            0.
        };
        let fresh = self.new_worktree();
        // The machine chip, with its shape, counts too.
        let machine_w = if self.has_machines() {
            text(&crate::data::machine_short(
                &self.machine_name(&self.defaults.machine.clone()),
            )) + 16.
        } else {
            0.
        };
        let full = machine_w
            + text(project)
            + if self.repo_now().is_some() {
                text("Current checkout") + if fresh { text("from main") } else { 0. }
            } else {
                0.
            };
        // Least loss first: the pick-up's words, then the worktree's.
        let pickup_bare = full + pickup_w > room;
        let worktree_bare = pickup_bare && full + 44. > room;
        let pickup = pickup.map(|p| div().flex_none().ml_auto().child(p).into_any_element());
        let pickup = if pickup_bare {
            (!self.recent.is_empty()).then(|| {
                div()
                    .flex_none()
                    .ml_auto()
                    .child(self.chip(Menu::Pickup, "square-terminal", "", true, cx))
                    .into_any_element()
            })
        } else {
            pickup
        };
        div()
            .mx(px(space::S4))
            .mt(px(-space::S3))
            .px(px(space::S2))
            .pt(px(space::S1))
            .pb(px(space::S1))
            .flex()
            .items_center()
            .gap(px(space::S1))
            .min_w_0()
            .overflow_hidden()
            .bg(t.tint())
            .border_1()
            .border_t_0()
            .border_color(t.hairline)
            .rounded_b(px(radius::LG))
            .children(self.has_machines().then(|| {
                let name =
                    crate::data::machine_short(&self.machine_name(&self.defaults.machine.clone()));
                keep(true, self.chip(Menu::Machine, "", &name, false, cx))
            }))
            .child(keep(
                true,
                self.chip(Menu::Project, "folder", project, no_project, cx),
            ))
            .children(self.workspace_chips(worktree_bare, cx))
            .children(pickup)
            .into_any_element()
    }

    /// In a repository: where the session works, and for a new worktree
    /// the branch it starts from (decision 0053). `bare`: the worktree chip
    /// is its icon alone; the branch chip always takes what room is left.
    fn workspace_chips(&mut self, bare: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(repo) = self.repo_now().cloned() else {
            return Vec::new();
        };
        let d = self.defaults.clone();
        let mut v = Vec::new();
        let (icon_name, label) = match &d.into {
            Some(path) => (
                "folder-git-2",
                crate::viewer::middle_ellipsis(
                    &repo
                        .worktrees
                        .iter()
                        .find(|w| w.path == *path)
                        .and_then(|w| w.branch.clone())
                        .unwrap_or_else(|| Self::project_name(path)),
                    CHIP_BRANCH,
                ),
            ),
            None if self.new_worktree() => ("git-branch-plus", "New worktree".to_string()),
            None => ("folder-git", "Current checkout".to_string()),
        };
        let label = if bare { String::new() } else { label };
        v.push(keep(
            true,
            self.chip(Menu::Workspace, icon_name, &label, false, cx),
        ));
        if self.new_worktree() {
            let from = d
                .base
                .clone()
                .or(repo.current.clone())
                .unwrap_or_else(|| "HEAD".into());
            // Tight, "from" goes (the icon says it's a branch) and the name
            // gives up letters from the middle, where long names differ least.
            let label = if bare {
                crate::viewer::middle_ellipsis(&from, 12)
            } else {
                format!(
                    "from {}",
                    crate::viewer::middle_ellipsis(&from, CHIP_BRANCH)
                )
            };
            v.push(self.chip(Menu::Base, "git-branch", &label, false, cx));
        }
        v
    }

    pub fn workspace_entries(&self) -> Vec<Entry> {
        let act = |f: Box<ActFn>| -> Option<Act> { Some(Rc::from(f)) };
        let Some(repo) = self.repo_now() else {
            return vec![Entry::note("Not a Git repository")];
        };
        let d = &self.defaults;
        let name = Self::project_name(&repo.root);
        let here = match &repo.current {
            Some(b) => format!("Works in {name}, on {b}"),
            None => format!("Works in {name}"),
        };
        let fresh = if repo.setup {
            "A folder and branch of its own; runs .dent/setup".to_string()
        } else {
            "A folder and branch of its own".to_string()
        };
        // With no commit to start from, there's no worktree to make.
        let empty = repo.branches.is_empty();
        let fresh_on = self.new_worktree();
        let mut v = vec![
            Entry::row(
                "m-here".into(),
                "Current checkout",
                Some(here),
                !fresh_on && d.into.is_none(),
                act(Box::new(|this, _, _| {
                    this.defaults.worktree = false;
                    this.defaults.into = None;
                })),
            ),
            Entry {
                off: empty.then_some("The repository has no commits yet"),
                ..Entry::row(
                    "m-worktree".into(),
                    "New worktree",
                    Some(fresh),
                    fresh_on,
                    act(Box::new(|this, _, _| {
                        this.defaults.worktree = true;
                        this.defaults.into = None;
                    })),
                )
            },
        ];
        if !repo.worktrees.is_empty() {
            v.push(Entry::of("m-sep".into(), "", EntryKind::Separator));
            v.push(Entry::note("Carry on in a worktree"));
            for (i, w) in repo.worktrees.iter().take(8).enumerate() {
                let path = w.path.clone();
                let shown = crate::picker::tilde(&path);
                v.push(Entry::row(
                    format!("m-wt-{i}"),
                    &crate::viewer::middle_ellipsis(
                        w.branch.as_deref().unwrap_or("Detached"),
                        MENU_BRANCH,
                    ),
                    Some(shown),
                    d.into.as_deref() == Some(path.as_str()),
                    act(Box::new(move |this, _, _| {
                        this.defaults.into = Some(path.clone());
                    })),
                ));
            }
        }
        v
    }

    /// The session's project scripts; choosing one runs it in your terminal.
    fn action_entries(&self) -> Vec<Entry> {
        let act = |f: Box<ActFn>| -> Option<Act> { Some(Rc::from(f)) };
        let Some(s) = self.current() else {
            return Vec::new();
        };
        let id = s.id.clone();
        self.actions
            .get(&id)
            .into_iter()
            .flatten()
            .map(|(name, about)| {
                let (id, name2) = (id.clone(), name.clone());
                Entry::row(
                    format!("m-action-{name}"),
                    name,
                    Some(if about.is_empty() {
                        format!("{}/{name}", crate::actions::DIR)
                    } else {
                        about.clone()
                    }),
                    false,
                    act(Box::new(move |this, window, cx| {
                        this.run_action(&id, &name2, window, cx);
                    })),
                )
            })
            .collect()
    }

    /// The branches a new worktree can start from, most recently committed
    /// to first, narrowed by what's typed in the menu's field.
    pub fn base_entries(&self, cx: &App) -> Vec<Entry> {
        let act = |f: Box<ActFn>| -> Option<Act> { Some(Rc::from(f)) };
        let Some(repo) = self.repo_now() else {
            return Vec::new();
        };
        let q = self.branch_query.read(cx).value().trim().to_lowercase();
        let chosen = self.defaults.base.clone().or(repo.current.clone());
        let current = repo.current.clone();
        // The chosen branch and the one checked out lead; the rest follow
        // by their latest commit.
        let lead: Vec<&String> = [&chosen, &current].into_iter().flatten().collect();
        let mut order: Vec<&String> = Vec::new();
        for b in lead.into_iter().chain(repo.branches.iter()) {
            if repo.branches.contains(b) && !order.contains(&b) {
                order.push(b);
            }
        }
        let mut v: Vec<Entry> = order
            .into_iter()
            .filter(|b| q.is_empty() || b.to_lowercase().contains(&q))
            .take(MAX_BRANCHES_SHOWN)
            .map(|b| {
                let (b2, cur) = (b.clone(), current.clone());
                Entry::row(
                    format!("m-base-{b}"),
                    &crate::viewer::middle_ellipsis(b, MENU_BRANCH),
                    (current.as_deref() == Some(b.as_str())).then(|| "Checked out".to_string()),
                    chosen.as_deref() == Some(b.as_str()),
                    act(Box::new(move |this, _, _| {
                        this.defaults.base =
                            (cur.as_deref() != Some(b2.as_str())).then(|| b2.clone());
                    })),
                )
            })
            .collect();
        if v.is_empty() {
            v.push(Entry::note(if q.is_empty() {
                "No branches yet"
            } else {
                "No branch matches"
            }));
        }
        v
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
                    // The machine chip wears the machine's shape.
                    .map(|d| {
                        if matches!(which, Menu::Machine | Menu::SessionMachine) {
                            let at = match which {
                                Menu::SessionMachine => self
                                    .selected
                                    .as_deref()
                                    .map(|id| dent_client_core::fleet::split(id).0.to_string())
                                    .unwrap_or_default(),
                                _ => self.defaults.machine.clone(),
                            };
                            let shape = self
                                .shapes()
                                .get(at.as_str())
                                .copied()
                                .unwrap_or(crate::glyph::Shape::Circle);
                            d.child(crate::glyph::glyph(shape, t.ink, 10.))
                        } else {
                            d.child(icon(
                                icon_name,
                                13.,
                                if icon_name == "claude" {
                                    crate::theme::CLAUDE
                                } else {
                                    t.quiet
                                },
                            ))
                        }
                    })
                    .when(!label.is_empty(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                // A long name (a branch, a project) stops
                                // here, so one chip never takes the row.
                                .max_w(px(CHIP_LABEL_MAX))
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
                                    current
                                        .map(|s| self.permission_shown(s))
                                        .unwrap_or_default()
                                        .label()
                                ),
                                Menu::Model | Menu::SessionModel => "Model".into(),
                                Menu::Project => "Project".into(),
                                Menu::Workspace => "Where it works".into(),
                                Menu::Base => "The branch the worktree starts from".into(),
                                Menu::Actions => "Run a project script in your terminal".into(),
                                Menu::Pickup => "Pick up from terminal".into(),
                                Menu::Stash => "Drafts put aside".into(),
                                Menu::Agent => "The agent it runs".into(),
                                Menu::Machine => "The machine it runs on".into(),
                                Menu::SessionMachine => {
                                    "Where it runs: another machine moves it there".into()
                                }
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
            Menu::Workspace => self.workspace_entries(),
            Menu::Base => self.base_entries(cx),
            Menu::Actions => self.action_entries(),
            Menu::Pickup => self.pickup_entries(),
            Menu::Machine => self.machine_entries(),
            Menu::SessionMachine => self.move_entries(),
            Menu::Stash => self.stash_entries(),
            Menu::Agent => {
                let current = self.agent_id_on(&self.defaults.machine, self.new_agent().as_deref());
                self.pickable_agents()
                    .into_iter()
                    .map(|a| {
                        let id = a.id.clone();
                        Entry::row(
                            format!("agent-{}", a.id),
                            &a.name,
                            Some(crate::agents::version_line(&a)),
                            current == a.id,
                            act(Box::new(move |this, _, cx| {
                                this.defaults.agent = Some(id.clone());
                                // Its own models, so the model chosen is one it has.
                                this.defaults.model = None;
                                this.prefs_changed();
                                cx.notify();
                            })),
                        )
                    })
                    .collect()
            }
            Menu::Model | Menu::SessionModel => {
                let current = self.menu_model(which, cx);
                let all = self.models_or_default(which);
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
                    v.push(Entry::note("Asking the agent for its models…"));
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
                let (current, caps) = match (which, self.current()) {
                    (Menu::SessionPermission, Some(s)) => (s.permission, s.caps.clone()),
                    (Menu::SessionPermission, None) => (Permission::default(), self.new_caps()),
                    _ => (d.permission, self.new_caps()),
                };
                // Only the modes the agent takes.
                Permission::ALL
                    .iter()
                    .filter(|p| caps.permissions.contains(p))
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
                // A level the model doesn't take isn't offered, unless it's
                // the one already chosen.
                let models = self.models_or_default(which);
                let steps = self.effort_steps(&models, &model, current.as_deref());
                EFFORTS
                    .iter()
                    .filter(|(value, _)| steps.contains(value))
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
                        EntryKind::Note => menu_note_owned(e.name.clone(), ticks, &t),
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
                        .child(Textarea::new(if which == Menu::Base {
                            &self.branch_query
                        } else {
                            &self.menu_query
                        })),
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
                    .restrict_scroll_to_axis()
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
    /// Drafts put aside with ⌘S, newest first: each by its first words,
    /// with what it carries.
    fn stash_entries(&self) -> Vec<Entry> {
        let n = self.stashes.len();
        let rows = self.stashes.iter().enumerate().rev().map(|(i, st)| {
            // Folded pastes read as their chips do.
            let text = st.shown(&self.pastes);
            let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let name = if first.chars().count() > 60 {
                format!("{}…", first.chars().take(60).collect::<String>().trim_end())
            } else if first.is_empty() {
                "Files only".to_string()
            } else {
                first.to_string()
            };
            let lines = st.text.lines().count();
            let mut note = Vec::new();
            if lines > 1 {
                note.push(format!("{lines} lines"));
            }
            match st.files().len() {
                0 => {}
                1 => note.push("1 file".into()),
                k => note.push(format!("{k} files")),
            }
            let act: Act = Rc::new(
                move |this: &mut Dent, window: &mut Window, cx: &mut Context<Dent>| {
                    this.unstash(i, window, cx)
                },
            );
            let remove: Act = Rc::new(
                move |this: &mut Dent, _: &mut Window, cx: &mut Context<Dent>| {
                    if i < this.stashes.len() {
                        this.stashes.remove(i);
                        this.prefs_changed();
                    }
                    if this.stashes.len() < 2 {
                        this.menu = None;
                    }
                    this.relight_menu(cx);
                    cx.notify();
                },
            );
            Entry {
                remove: Some(("Throw this draft away", remove)),
                ..Entry::row(
                    format!("stash-{i}"),
                    &name,
                    (!note.is_empty()).then(|| note.join(" · ")),
                    false,
                    Some(act),
                )
            }
        });
        std::iter::once(Entry::note(&format!("{n} drafts put aside")))
            .chain(rows)
            .collect()
    }

    /// The menu of drafts put aside, over the prompt it brings one back to.
    fn stash_menu(&mut self, below: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu != Some(Menu::Stash) {
            return None;
        }
        let list = self.menu_list(Menu::Stash, 320., cx);
        let menu = div()
            .absolute()
            .left(px(-1.))
            .when(below, |d| d.top(relative(1.)).pt(px(space::S1)))
            .when(!below, |d| d.bottom(relative(1.)).pb(px(space::S1)))
            .child(div().child(list).arrive(
                "stash-arrive",
                if below { From::Above } else { From::Below },
                motion::FAST,
            ));
        Some(deferred(menu).with_priority(1).into_any_element())
    }

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

fn menu_note_owned(text: String, ticks: bool, t: &theme::Theme) -> AnyElement {
    let tick = if ticks { 14. + space::S2 } else { 0. };
    div()
        // In line with the rows' names, past their tick column when the
        // menu keeps one.
        .pl(px(space::S1 + space::S2 + tick))
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
pub const EFFORTS: [(Option<&str>, &str); 6] = [
    (None, "Whatever the agent picks for the model"),
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

/// The most subagent rails drawn side by side.
const MOST_RAILS: usize = 6;

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
    #[allow(clippy::too_many_arguments)]
    fn report(
        &mut self,
        s: &Session,
        ix: usize,
        call: &str,
        depth: usize,
        report: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        // A report from inside a subagent's work is one of its steps: the
        // card's own type, in the secondary ink. Only the final report reads
        // at reading size.
        if depth > 0 {
            let plain: String = report
                .lines()
                .map(|l| l.trim_start_matches('#').trim_start())
                .collect::<Vec<_>>()
                .join("\n")
                .replace("**", "")
                .replace('`', "");
            return div()
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
                        .ty(UI)
                        .text_color(t.secondary())
                        .child(crate::select::text(
                            SharedString::from(format!("report-{ix}-{call}-text")),
                            plain.trim().to_string(),
                            &t,
                        )),
                )
                .into_any_element();
        }
        let key = Fold::Report(call).key();
        let open = self.is_open(&Fold::Report(call));
        // Folded, it shows its first two blocks whole, never half a table.
        let all = self.markdown(report);
        let blocks: &[markdown::Block] = &all;
        let long = blocks.len() > 2;
        let blocks = if long && !open { &blocks[..2] } else { blocks };
        let fold = cx.listener(move |this, _, _, cx| {
            this.toggle_fold(&key);
            cx.notify();
        });
        let mut r = markdown::R {
            live: false,
            counted: true,
            md: self.mdv.clone(),
            owner: cx.entity_id(),
            window,
            cx,
            scope: format!("{}/report-{call}", s.id),
            bg: t.surface,
            base: Some(std::path::PathBuf::from(&s.cwd)),
            session: Some((s.id.clone(), s.cwd.clone())),
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
                        // Clear of the quote above it.
                        .mt(px(space::S1))
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

/// Another machine's media is copied here whole for a still only up to
/// this; a bigger one shows without.
const POSTER_MAX: u64 = 16 << 20;

/// A picture session `session` names, to draw on this Mac: the file itself
/// for this Mac's sessions, the copy here of another machine's; `None`
/// when it isn't a picture or isn't here (yet).
fn picture_path(session: &str, shown: &str, cwd: &str) -> Option<std::path::PathBuf> {
    let ext = shown.rsplit('.').next()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return None;
    }
    crate::files::picture(&crate::files::Ask::new(session, shown, cwd)).shown()
}

/// Pictures a group of tool calls brought back, each opening large; they
/// step together in the large view. One shows at a readable size; more
/// sit in one row of small tiles, and past [`TOOL_TILES`] the last tile
/// counts the rest.
fn tool_pictures(
    session: &str,
    cwd: &str,
    ix: usize,
    paths: &[String],
    probes: &std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, Bounds<Pixels>>>>,
    t: &theme::Theme,
) -> Div {
    // Each as it's here: this Mac's own, or the copy of another machine's
    // once it's come.
    // Drawn from copies kept from before, their machine away: marked.
    let kept = paths
        .iter()
        .find_map(|p| crate::files::kept(&crate::files::Ask::new(session, p, cwd)));
    let set: crate::lightbox::Set = std::rc::Rc::new(
        paths
            .iter()
            .enumerate()
            .filter_map(|(j, p)| {
                Some(crate::lightbox::Shot {
                    key: SharedString::from(format!("tool-pic-{session}-{ix}-{j}")),
                    path: crate::files::picture(&crate::files::Ask::new(session, p, cwd))
                        .shown()?,
                })
            })
            .collect(),
    );
    if set.is_empty() {
        return div();
    }

    let frame = || {
        div()
            .rounded(px(radius::MD))
            .border_1()
            .border_color(t.hairline)
            .overflow_hidden()
    };
    if set.len() == 1 {
        // As wide as it is, up to the cap, and narrower with the column. The
        // row sizes to its content, so the picture needs a width of its own
        // (a share of nothing is nothing).
        let wide = crate::data::picture_dims(&set[0].path).map_or(TOOL_PICTURE_W, |(w, h)| {
            (w as f32)
                .min(TOOL_PICTURE_H * w as f32 / h.max(1) as f32)
                .min(TOOL_PICTURE_W)
        });
        return div().mt(px(space::S2)).flex().child(
            crate::lightbox::picture(
                &set,
                0,
                frame()
                    .relative()
                    .max_w_full()
                    .max_h(px(TOOL_PICTURE_H))
                    .child(probe(probes, "tool-picture"))
                    .child(crate::markdown::sized(
                        img(set[0].path.clone()).object_fit(ObjectFit::Contain),
                        &set[0].path,
                        TOOL_PICTURE_H,
                    )),
            )
            .min_w_0()
            .w(px(wide))
            .max_w_full()
            // On the picture's own corner.
            .when_some(kept, |d, m| {
                d.child(crate::ui::kept_mark(
                    format!("tool-kept-{session}-{ix}"),
                    &m,
                    t,
                ))
            }),
        );
    }
    let shown = set.len().min(TOOL_TILES);
    // The rest past the last tile, which then counts them over itself.
    let more = set.len() - shown;
    div()
        .mt(px(space::S2))
        .relative()
        .flex()
        .gap(px(space::S1H))
        .children((0..shown).map(|j| {
            let last = j + 1 == shown && more > 0;
            // A tile gives way evenly when the column narrows, rather than
            // wrapping or being cut off at the edge.
            // The last tile stands for itself and every picture after it.
            let upto = if last { set.len() } else { j + 1 };
            crate::lightbox::pictures(
                &set,
                j,
                upto,
                frame()
                    .relative()
                    .size_full()
                    .bg(t.panel)
                    .child(
                        img(set[j].path.clone())
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                    .when(last, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .bg(gpui_kit::black().opacity(0.55))
                                .flex()
                                .items_center()
                                .justify_center()
                                .ty(LABEL)
                                .text_color(gpui_kit::white())
                                .child(format!("+{}", more + 1)),
                        )
                    }),
            )
            .w(px(TOOL_TILE_W))
            .h(px(TOOL_TILE_H))
            .min_w_0()
            .flex_shrink(1.)
        }))
        // Over the tiles, in the first one's corner.
        .when_some(kept, |d, m| {
            d.child(crate::ui::kept_mark(
                format!("tool-kept-{session}-{ix}"),
                &m,
                t,
            ))
        })
}

/// The most a tool's picture takes in the chat; it opens large on a click.
const TOOL_PICTURE_W: f32 = 420.;
/// How tall a page's picture stands above its card.
const PAGE_STILL_H: f32 = 240.;
const TOOL_PICTURE_H: f32 = 280.;
/// Several pictures: how many tiles show, and each tile's size.
const TOOL_TILES: usize = 5;
const TOOL_TILE_W: f32 = 120.;
const TOOL_TILE_H: f32 = 76.;

/// A file that went with a message you sent (`what`, as the session's
/// folder names it): a picture shows as itself, anything else as the chip
/// it was in the box.
fn sent_file(what: &str, shot: Option<(&crate::lightbox::Set, usize)>, t: &theme::Theme) -> Div {
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
            .child(
                div()
                    .max_w(px(320.))
                    .truncate()
                    .ty(LABEL)
                    .text_color(t.quiet)
                    .child(name),
            );
    }
    let name = what.rsplit('/').next().unwrap_or(what).to_string();
    // Not found as a picture (that would show above), so by its kind.
    div().flex().child(
        small_chip(
            ChipFace::Icon(
                crate::app::Attachment::File {
                    path: what.to_string(),
                }
                .icon(),
            ),
            name,
            t,
        )
        .id(SharedString::from(format!("sent-file-{what}")))
        .with_tip(crate::ui::tip_owned(what.to_string(), *t)),
    )
}

/// A note that went with a message you sent (`place · what`): a review
/// comment, a picked element, a page error.
fn sent_note(line: &str, t: &theme::Theme) -> Div {
    let (place, what) = line.split_once(" · ").unwrap_or((line, ""));
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

/// What a small chip leads with: a picture shows itself, small; anything
/// else its kind's icon.
pub enum ChipFace {
    Icon(&'static str),
    Picture(std::path::PathBuf),
}

/// The one small chip for what goes with a message (a file, a picture, a
/// folded paste), the same in the reply box, a queued message and a sent
/// one: `type.ui` in ink on the selected fill, `size.control.sm` high,
/// `radius.sm`.
pub fn small_chip(face: ChipFace, label: impl Into<SharedString>, t: &theme::Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::S1H))
        .max_w(px(420.))
        .min_w_0()
        .h(px(size::CONTROL_SM))
        .px(px(space::S2))
        .rounded(px(radius::SM))
        .bg(t.selected())
        .ty(UI)
        .text_color(t.ink)
        .child(chip_face(face, t))
        .child(div().min_w_0().truncate().child(label.into()))
}

fn chip_face(face: ChipFace, t: &theme::Theme) -> AnyElement {
    match face {
        ChipFace::Icon(name) => icon(name, 12., t.quiet).into_any_element(),
        ChipFace::Picture(path) => div()
            .size(px(16.))
            .flex_none()
            .rounded(px(3.))
            .overflow_hidden()
            .child(img(path).size_full().object_fit(ObjectFit::Cover))
            .into_any_element(),
    }
}

/// A file's chip face: its picture when it is one, else its kind's icon.
pub fn file_face(path: &str) -> ChipFace {
    if Kind::of(path) == Some(Kind::Image) {
        ChipFace::Picture(std::path::PathBuf::from(path))
    } else {
        ChipFace::Icon(
            crate::app::Attachment::File {
                path: path.to_string(),
            }
            .icon(),
        )
    }
}

/// The queued message the working line sits above, while a session works
/// with messages waiting: the first of the queued run at the end.
pub fn working_host(s: &Session) -> Option<usize> {
    if s.status != Status::Working {
        return None;
    }
    let tail = s
        .items
        .iter()
        .rev()
        .take_while(|i| matches!(i, Item::Queued { .. }))
        .count();
    (tail > 0).then(|| s.items.len() - tail)
}

/// A plan as its card shows it, and whether it's long enough to fold: past
/// 20 lines or 900 characters it shows its first 10 lines (at most 600
/// characters) until it's expanded, so the card's buttons stay in view.
pub fn plan_shown(plan: &str, open: bool) -> (std::borrow::Cow<'_, str>, bool) {
    let long = plan.lines().nth(20).is_some() || plan.chars().count() > 900;
    if !long || open {
        return (plan.into(), long);
    }
    let mut end = plan
        .match_indices('\n')
        .nth(9)
        .map_or(plan.len(), |(i, _)| i);
    if let Some((i, _)) = plan[..end].char_indices().nth(600) {
        end = i;
    }
    let mut preview = plan[..end].trim_end().to_string();
    // A code block cut open is closed, so the rest doesn't read as code.
    let fences = preview
        .lines()
        .filter(|l| l.trim_start().starts_with("```"))
        .count();
    if fences % 2 == 1 {
        preview.push_str("\n```");
    }
    (preview.into(), long)
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
/// `…` where a line was cut short; and, for copying, each `…` and the rest
/// of its line it stands for.
fn clipped(
    text: &str,
    kept: &[std::ops::Range<usize>],
) -> (String, Vec<(std::ops::Range<usize>, String)>) {
    let mut out = String::new();
    let mut swaps = Vec::new();
    for (k, r) in kept.iter().enumerate() {
        if k > 0 {
            out.push('\n');
        }
        out.push_str(&text[r.clone()]);
        if cut_short(text, r) {
            let at = out.len();
            out.push('…');
            swaps.push((at..out.len(), rest_of_line(text, r).to_string()));
        }
    }
    (out, swaps)
}

/// What a kept stretch left off the end of its line.
fn rest_of_line<'a>(text: &'a str, r: &std::ops::Range<usize>) -> &'a str {
    let rest = &text[r.end..];
    &rest[..rest.find('\n').unwrap_or(rest.len())]
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
            // Coloured as the terminal tab colours the same output.
            color = match st.fg {
                Some(crate::ansi::Colour::Indexed(i)) => crate::shell::indexed(t, i),
                Some(crate::ansi::Colour::Rgb(r, g, b)) => {
                    gpui_kit::rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into()
                }
                None => ink,
            };
            if st.dim {
                color = color.opacity(crate::shell::DIM);
            }
        }
        TextRun {
            len,
            font: f,
            color,
            background_color: None,
            // As the terminal tab draws it.
            underline: st.filter(|st| st.underline).map(|_| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: None,
        }
    };
    let mut out = String::new();
    let mut runs = Vec::new();
    let mut swaps = Vec::new();
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
            // Copied, it's the rest of the line again.
            let at = out.len();
            out.push('…');
            runs.push(run('…'.len_utf8(), None));
            swaps.push((at..out.len(), rest_of_line(text, r).to_string()));
        }
    }
    div()
        .min_w_0()
        .whitespace_normal()
        .child(crate::select::styled(id, out, runs, t).swaps(swaps))
}

/// Lines of code or output that select and copy; with `diff`, removed
/// ("- ") and added ("+ ") lines take the diff's colours.
/// `swaps` are stand-ins and what they copy as (a cut line's `…`).
pub fn code_lines(
    id: String,
    text: String,
    swaps: Vec<(std::ops::Range<usize>, String)>,
    diff: bool,
    ink: Hsla,
    t: &crate::theme::Theme,
) -> Div {
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
        .child(crate::select::styled(id, text, runs, t).swaps(swaps))
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
    // The widest it reads ("context full"), with the needs square beside it.
    let ctx_w = text("context full") + space::S2 + 7. + space::S1;
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

/// A note about an MCP server in trouble, worded as now ("The tickets MCP
/// server …") or as older logs have it ("The MCP server tickets …").
fn mcp_note(text: &str) -> bool {
    text.starts_with("The MCP server ")
        || (text.starts_with("The ")
            && [
                " MCP server couldn't connect.",
                " MCP server needs you to sign in.",
            ]
            .iter()
            .any(|end| text.ends_with(end)))
}

/// The number key that answers with a button, faint inside it.
fn key_num(n: &'static str, primary: bool, t: &theme::Theme) -> Div {
    div()
        .ty(NUMERAL)
        .font_weight(FontWeight::NORMAL)
        .text_color(if primary {
            t.surface.opacity(0.55)
        } else {
            t.faint
        })
        .child(n)
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
    // A stopped turn's changes come just before its note.
    if matches!(items.get(ix), Some(Item::Note { .. }))
        && let Some(c) = ix.checked_sub(1)
        && matches!(items.get(c), Some(Item::Changed { .. }))
    {
        return Some(c);
    }
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
        && (items[..ix]
            .iter()
            .rposition(|i| matches!(i, Item::Text(_)))
            .is_some_and(|r| change_after(items, r) == Some(ix))
            || change_after(items, ix + 1) == Some(ix))
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
            Item::Text(_) | Item::Tools { .. } | Item::Thinking(..) => return false,
            _ => {}
        }
    }
    !running
}

/// For the last reply of a turn that has finished: when it finished, how
/// long it took (ms), and whether it's the session's latest turn.
pub fn turn_end_after(items: &[Item], ix: usize) -> Option<(u64, u64, bool)> {
    match items.get(ix) {
        Some(Item::Text(_)) => {}
        // A turn you stopped, or one that failed, says so straight under its
        // words, and its time and actions follow the note.
        Some(Item::Note {
            kind: crate::data::NoteKind::Stopped | crate::data::NoteKind::Error,
            ..
        }) if items[..ix]
            .iter()
            .rev()
            .take_while(|i| !matches!(i, Item::User { .. }))
            .any(|i| matches!(i, Item::Text(_))) => {}
        _ => return None,
    }
    for (j, i) in items.iter().enumerate().skip(ix + 1) {
        match i {
            Item::Text(_)
            | Item::User { .. }
            | Item::Queued { .. }
            | Item::Note {
                kind: crate::data::NoteKind::Stopped | crate::data::NoteKind::Error,
                ..
            } => return None,
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

/// How a finished turn's work sits in the chat: folded into one "Worked
/// for 2m 14s" line where it began, or as it came.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TurnFold {
    Shown,
    /// Folded away under the turn's line.
    Hidden,
    /// The turn's line, standing where its first work was: how long the
    /// turn took (ms), the message that began it (by place), and whether
    /// it's open.
    Head {
        took: u64,
        user: usize,
        open: bool,
    },
}

/// Each item's place in its turn's fold. A finished turn folds what led to
/// its answer: narration, tool groups, thoughts and small notes. Its
/// answer, pages, cards, errors and your messages stay. A turn folds only
/// when there's real work in it (a tool or a thought), and never while it
/// runs. `open` says which turns (by their message's place) are open.
pub fn turn_folds(items: &[Item], open: impl Fn(usize) -> bool) -> Vec<TurnFold> {
    let mut out = vec![TurnFold::Shown; items.len()];
    let mut start = 0;
    while start < items.len() {
        // A turn: your message, up to the next of yours.
        let Some(user) = items[start..]
            .iter()
            .position(|i| matches!(i, Item::User { .. }))
            .map(|p| start + p)
        else {
            break;
        };
        let end = items[user + 1..]
            .iter()
            .position(|i| matches!(i, Item::User { .. }))
            .map_or(items.len(), |p| user + 1 + p);
        start = end;
        let Some((stop, took)) = items[user..end]
            .iter()
            .enumerate()
            .find_map(|(k, i)| match i {
                Item::TurnEnd { took, .. } => Some((user + k, *took)),
                _ => None,
            })
        else {
            continue;
        };
        // Its answer: the last words before it ended.
        let Some(answer) = (user + 1..stop)
            .rev()
            .find(|&k| matches!(items[k], Item::Text(_)))
        else {
            continue;
        };
        let folds = |i: &Item| {
            matches!(
                i,
                Item::Tools { .. }
                    | Item::Thinking(..)
                    | Item::Text(_)
                    | Item::Note {
                        kind: crate::data::NoteKind::Other,
                        ..
                    }
            )
        };
        let work: Vec<usize> = (user + 1..answer).filter(|&k| folds(&items[k])).collect();
        let real = work
            .iter()
            .any(|&k| matches!(items[k], Item::Tools { .. } | Item::Thinking(..)));
        let (Some(&head), true) = (work.first(), real) else {
            continue;
        };
        let open = open(user);
        out[head] = TurnFold::Head { took, user, open };
        if !open {
            for &k in &work[1..] {
                out[k] = TurnFold::Hidden;
            }
        }
    }
    out
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

/// What a tool call printed, found by its id in the session's tool lines.
fn call_output(s: &Session, call: &str) -> Option<String> {
    s.items.iter().find_map(|i| match i {
        Item::Tools { lines, .. } => lines.iter().find(|l| l.id == call).and_then(|l| {
            l.detail
                .as_ref()
                .and_then(|d| d.output.as_ref().map(|(_, o)| o.clone()))
        }),
        _ => None,
    })
}

/// The file background work writes its output to, as the agent's receipt
/// names it ("Output is being written to: /tmp/…/b1.output"); whether it's
/// there is the session's machine's to say.
fn output_file(receipt: &str) -> Option<String> {
    let at = receipt.find("written to:")? + "written to:".len();
    let path = receipt[at..].split_whitespace().next()?;
    Some(path.trim_end_matches(['.', ',']).to_string())
}

/// How large a message of only emoji reads.
const EMOJI_SIZE: f32 = 40.;

/// A message of one to eight emoji and nothing else (spaces aside).
fn emoji_only(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let mut shown = 0;
    for c in t.chars() {
        let u = c as u32;
        match u {
            // Joiners, variation selectors, skin tones and tags ride on
            // the emoji before them.
            0x200D | 0xFE0E | 0xFE0F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F => {}
            _ if c.is_whitespace() => {}
            0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2300..=0x23FF | 0x2B00..=0x2BFF => shown += 1,
            _ => return false,
        }
    }
    (1..=8).contains(&shown)
}

#[cfg(test)]
mod answer_tests {
    use super::{change_after, folded_change, is_answer, turn_end_after};
    use crate::data::Item;

    #[test]
    fn a_stopped_turns_actions_follow_its_note() {
        let note = Item::Note {
            icon: "square",
            text: "Stopped".into(),
            kind: crate::data::NoteKind::Stopped,
        };
        let items = vec![
            Item::Text("words".into()),
            Item::Changed {
                paths: vec!["a.rs".into()],
            },
            note.clone(),
            Item::TurnEnd { at: 1, took: 0 },
        ];
        assert!(turn_end_after(&items, 0).is_none());
        assert!(turn_end_after(&items, 2).is_some());
        assert_eq!(change_after(&items, 2), Some(1));
        assert!(folded_change(&items, 1));
        // A turn stopped before it said anything keeps a plain note.
        let bare = vec![note, Item::TurnEnd { at: 1, took: 0 }];
        assert!(turn_end_after(&bare, 0).is_none());
    }

    #[test]
    fn a_finished_turn_folds_its_work_under_one_line() {
        use super::{TurnFold, turn_folds};
        let user = |id: &str| Item::User {
            id: id.into(),
            text: "do it".into(),
            files: Vec::new(),
            at: 0,
        };
        let text = |t: &str| Item::Text(t.into());
        let tools = || Item::Tools {
            summary: "Ran a command".into(),
            lines: Vec::new(),
            counted: false,
            pictures: Vec::new(),
        };
        let end = Item::TurnEnd { at: 1, took: 9000 };
        let items = vec![
            user("a"),
            text("Looking first."),
            tools(),
            Item::Thinking("hm".into(), 0),
            text("Fixed."),
            end.clone(),
        ];
        let f = turn_folds(&items, |_| false);
        assert_eq!(f[0], TurnFold::Shown);
        assert_eq!(
            f[1],
            TurnFold::Head {
                took: 9000,
                user: 0,
                open: false
            }
        );
        assert_eq!(&f[2..4], &[TurnFold::Hidden, TurnFold::Hidden]);
        // The answer and the turn's end stay.
        assert_eq!(&f[4..], &[TurnFold::Shown, TurnFold::Shown]);
        // Opened, everything shows under its line.
        let f = turn_folds(&items, |_| true);
        assert_eq!(
            f[1],
            TurnFold::Head {
                took: 9000,
                user: 0,
                open: true
            }
        );
        assert!(f[2..].iter().all(|x| *x == TurnFold::Shown));
        // Still running: nothing folds.
        let running = &items[..5];
        assert!(
            turn_folds(running, |_| false)
                .iter()
                .all(|x| *x == TurnFold::Shown)
        );
        // Only words before the answer: nothing worth folding.
        let chat = vec![user("b"), text("Sure."), text("Here it is."), end.clone()];
        assert!(
            turn_folds(&chat, |_| false)
                .iter()
                .all(|x| *x == TurnFold::Shown)
        );
        // A card in the middle stays where it is; work either side folds.
        let mid = vec![
            user("c"),
            tools(),
            Item::Note {
                icon: "circle-alert",
                text: "boom".into(),
                kind: crate::data::NoteKind::Error,
            },
            tools(),
            text("Done."),
            end.clone(),
        ];
        let f = turn_folds(&mid, |_| false);
        assert!(matches!(f[1], TurnFold::Head { .. }));
        assert_eq!(f[2], TurnFold::Shown);
        assert_eq!(f[3], TurnFold::Hidden);
        // Two turns fold apart, each by its own message.
        let two: Vec<Item> = items.iter().cloned().chain(items.iter().cloned()).collect();
        let f = turn_folds(&two, |u| u == 6);
        assert_eq!(
            f[1],
            TurnFold::Head {
                took: 9000,
                user: 0,
                open: false
            }
        );
        assert_eq!(
            f[7],
            TurnFold::Head {
                took: 9000,
                user: 6,
                open: true
            }
        );
        // No answer (stopped before words): nothing to fold to.
        let none = vec![user("d"), tools(), end];
        assert!(
            turn_folds(&none, |_| false)
                .iter()
                .all(|x| *x == TurnFold::Shown)
        );
    }

    #[test]
    fn only_an_mcp_servers_own_notes_offer_its_servers() {
        use super::mcp_note;
        assert!(mcp_note("The tickets MCP server needs you to sign in."));
        assert!(mcp_note("The figma MCP server couldn't connect."));
        // As older logs have it.
        assert!(mcp_note("The MCP server tickets needs you to sign in."));
        assert!(!mcp_note("The request to the MCP server timed out"));
        assert!(!mcp_note("An MCP server couldn't connect."));
    }

    #[test]
    fn a_failed_turns_time_and_copy_follow_its_error() {
        let error = Item::Note {
            icon: "circle-alert",
            text: "Claude Code stopped unexpectedly".into(),
            kind: crate::data::NoteKind::Error,
        };
        let items = vec![
            Item::Text("starting".into()),
            error.clone(),
            Item::TurnEnd { at: 1, took: 0 },
        ];
        assert!(turn_end_after(&items, 0).is_none());
        assert!(turn_end_after(&items, 1).is_some());
        // A failure before any words: no footer at all, as before.
        let bare = vec![error.clone(), Item::TurnEnd { at: 1, took: 0 }];
        assert!(turn_end_after(&bare, 0).is_none());
        // It went on after the error and finished: the footer is the
        // last words', not the error's.
        let after = vec![
            Item::Text("starting".into()),
            error,
            Item::Text("done after all".into()),
            Item::TurnEnd { at: 1, took: 0 },
        ];
        assert!(turn_end_after(&after, 1).is_none());
        assert!(turn_end_after(&after, 2).is_some());
    }

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
        let done = vec![
            text(),
            Item::Thinking(String::new(), 0),
            text(),
            end.clone(),
        ];
        assert!(is_answer(&done, 2, false));
        assert!(!is_answer(&done, 0, false));
        // An older turn's answer stays one while a new turn runs.
        let older = vec![
            text(),
            end,
            Item::User {
                id: "u".into(),
                text: "more".into(),
                files: Vec::new(),
                at: 0,
            },
            text(),
        ];
        assert!(is_answer(&older, 0, true));
        assert!(!is_answer(&older, 3, true));
    }
}

#[cfg(test)]
mod clip_tests {
    #[test]
    fn a_long_plan_shows_its_start() {
        let short = "1. Do it\n2. Check it";
        assert_eq!(super::plan_shown(short, false), (short.into(), false));
        let long: String = (1..=30).map(|i| format!("{i}. Step {i}\n")).collect();
        let (shown, folds) = super::plan_shown(&long, false);
        assert!(folds);
        assert_eq!(shown.lines().count(), 10);
        assert_eq!(super::plan_shown(&long, true).0, long.as_str());
        // A fence cut open is closed.
        let code = format!("Plan:\n```sh\n{}```\n", "ls\n".repeat(30));
        let (shown, _) = super::plan_shown(&code, false);
        assert!(shown.ends_with("\n```"), "{shown:?}");
        // One long line folds by characters.
        let wide = "word ".repeat(400);
        assert!(super::plan_shown(&wide, false).0.chars().count() <= 600);
    }

    /// A line cut short shows `…`, and copies as the whole line.
    #[test]
    fn a_cut_line_copies_whole() {
        let long = format!("cargo test {}", "x".repeat(400));
        let text = format!("{long}\nshort");
        let (kept, ..) = crate::ansi::clip(&text, false);
        let (shown, swaps) = super::clipped(&text, &kept);
        assert!(shown.contains('…'));
        assert_eq!(swaps.len(), 1);
        let (r, with) = &swaps[0];
        assert_eq!(&shown[r.clone()], "…");
        let copied = format!("{}{with}{}", &shown[..r.start], &shown[r.end..]);
        assert_eq!(copied, text);
    }
}

#[cfg(test)]
mod emoji_tests {
    use super::emoji_only;

    #[test]
    fn only_a_few_emoji_read_large() {
        assert!(emoji_only("🎉🚀"));
        assert!(emoji_only(" 👍🏽 "));
        assert!(emoji_only("👨‍👩‍👧"));
        assert!(!emoji_only("Ship it 🚀"));
        assert!(!emoji_only("🎉🎉🎉🎉🎉🎉🎉🎉🎉"));
        assert!(!emoji_only(""));
    }
}

/// Branches the base menu lists at once; typing narrows the rest.
const MAX_BRANCHES_SHOWN: usize = 60;

/// The widest a chip's words get before they end in an ellipsis.
const CHIP_LABEL_MAX: f32 = 240.;

/// Below this chat width the header keeps only what has no other way in.
const CRAMPED: f32 = 480.;
/// A branch's name in a chip, and in a menu row, before it gives up letters
/// from the middle (where long names differ least).
const CHIP_BRANCH: usize = 28;
const MENU_BRANCH: usize = 40;

/// About how wide the new-session page's Start button is, with its key.
const START_W: f32 = 88.;
/// About what the agent chip takes, its icon and a short name, for the
/// row's fitting.
const AGENT_CHIP_W: f32 = 96.;

/// A size in megabytes, as a progress line says it: 1.2 MB.
fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.)
}
