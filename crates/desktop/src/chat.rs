//! The main column: a session's conversation and composer, or the new-session
//! page where you start by typing.

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::anim;
use crate::app::{Dent, Menu, Tab};
use dent_protocol::Permission;

use crate::data::{Item, Kind, Session};
use crate::ds::{
    CODE, DISPLAY, DISPLAY_COMPACT, LABEL, READING, Typed, UI, motion, radius, size, space,
};
use crate::markdown;
use crate::theme::{self, Status, status_square};
use crate::ui::{button, floating, ghost, hint, icon, icon_button, kbd, menu_row, probe, tip};

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
        let has_viewer = !s.arts.is_empty() || !s.diff.is_empty() || !s.terminal.is_empty();
        // Buttons here must not start a window drag.
        let mut actions = div()
            .flex()
            .items_center()
            .gap(px(space::S1))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if s.stopped && !s.closed {
            let sid = id.clone();
            actions = actions.child(
                button("resume", "Resume", false, &t)
                    .on_click(cx.listener(move |this, _, _, cx| this.resume(&sid, cx))),
            );
        }
        {
            let sid = id.clone();
            actions = actions.child(
                ghost("fork", "Fork", &t)
                    .relative()
                    .on_click(cx.listener(move |this, _, _, cx| this.fork(&sid, None, cx)))
                    .tooltip(tip("A new session that carries on from here", None, t))
                    .child(probe(&self.probes, "fork")),
            );
        }
        if Self::can_close(s) {
            let sid = id.clone();
            actions = actions.child(
                button("close", "Close", false, &t)
                    .relative()
                    .child(kbd("⌘W", &t))
                    .on_click(cx.listener(move |this, _, window, cx| this.close(&sid, window, cx)))
                    .child(probe(&self.probes, "close")),
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
                    this.picking = false;
                    cx.notify();
                }))
                .tooltip(tip(
                    if open { "Hide viewer" } else { "Show viewer" },
                    Some("⌘\\"),
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
                                    .flex_shrink_0()
                                    .max_w(relative(0.7))
                                    .truncate()
                                    .ty(DISPLAY_COMPACT)
                                    .text_color(t.ink)
                                    // Double-click to rename; a single press still moves the window.
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                            if e.click_count == 2 {
                                                cx.stop_propagation();
                                                this.renaming = Some(sid.clone());
                                                this.rename_input.update(cx, |i, cx| {
                                                    i.set_value(title.clone(), window, cx);
                                                    i.focus(window, cx);
                                                });
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .child(s.title.clone()),
                            )
                            .child(meta)
                        }
                    }),
            )
            .child(actions)
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
        .flex_1()
        .min_h_0();
        let footer: AnyElement = if s.closed {
            let id = s.id.clone();
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
                        .top(px(-space::S6))
                        .left_0()
                        .right_0()
                        .h(px(space::S6))
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(t.surface.opacity(0.), 0.),
                            linear_color_stop(t.surface, 1.),
                        )),
                )
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        match item {
            Item::User { id: msg, text } => {
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
                                atts.iter()
                                    .map(|a| sent_attachment(a.trim_start_matches("› "), &t)),
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
                                d.child(div().ty(READING).text_color(t.ink).child(shown))
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
                                    .gap(px(space::S1))
                                    .opacity(0.)
                                    .group_hover(group.clone(), |s| s.opacity(1.))
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
                        div()
                            .ty(READING)
                            .text_color(t.ink)
                            .child(inline.element(&t, theme::body(), t.ink, Some(&fade)))
                            .into_any_element()
                    }
                    _ => markdown::render(blocks.as_slice(), &t, false).into_any_element(),
                }
            }
            Item::Tools { summary, lines } => {
                let key = (s.id.clone(), ix);
                let open = self.open_tools.contains(&key);
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
                            .cursor_pointer()
                            .hover(|d| d.text_color(t.ink))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.open_tools.remove(&key) {
                                    this.open_tools.insert(key.clone());
                                }
                                cx.notify();
                            }))
                            .child(div().text_color(t.faint).child(if open {
                                "┬"
                            } else {
                                "├"
                            }))
                            .child(summary.clone()),
                    )
                    .when(open, |d| {
                        let n = lines.len();
                        d.children(lines.iter().enumerate().map(move |(i, l)| {
                            div()
                                .flex()
                                .gap(px(space::S2))
                                .ty(CODE)
                                .text_color(t.quiet)
                                .child(div().text_color(t.faint).child(if i + 1 == n {
                                    "└"
                                } else {
                                    "├"
                                }))
                                .child(l.clone())
                        }))
                    })
                    .into_any_element()
            }
            Item::Made { art, version } => self.made_card(s, art, *version, cx),
            Item::Approval {
                title,
                command,
                why,
                ..
            } => {
                let (a, a2, d) = (s.id.clone(), s.id.clone(), s.id.clone());
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
                                .child(command.clone()),
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
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.allow(&a, false, cx)
                                        }),
                                    )
                                    .child(probe(&self.probes, "allow")),
                            )
                            .child(
                                button("allow-session", "Allow for this session", false, &t)
                                    .relative()
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.allow(&a2, true, cx)
                                        }),
                                    )
                                    .child(probe(&self.probes, "allow-session")),
                            )
                            .child(
                                button("deny", "Deny", false, &t)
                                    .relative()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.deny(&d, window, cx)
                                    }))
                                    .child(probe(&self.probes, "deny")),
                            ),
                    )
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child("Typed text is sent as the reason"),
                    )
                    .into_any_element()
            }
            Item::Plan { plan, .. } => {
                let (a, d) = (s.id.clone(), s.id.clone());
                let blocks = self.markdown(plan);
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
                            .child(markdown::render(blocks.as_slice(), &t, false)),
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
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.allow(&a, false, cx)
                                        }),
                                    )
                                    .child(probe(&self.probes, "plan-go")),
                            )
                            .child(
                                button("plan-keep", "Keep planning", false, &t)
                                    .relative()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.deny(&d, window, cx)
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
                                    let (sid, o2) = (s.id.clone(), label.clone());
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
                                            this.answer(&sid, qi, o2.clone(), cx)
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
                .child(div().flex_1().min_w_0().child(text.clone()))
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
            format!("{} · on {}", a.path, s.machine)
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
                            .child(a.title),
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
                .bg(t.ink)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.stop(&id, cx)))
                .tooltip(tip("Stop", Some("⌘."), t))
                .child(div().size(px(9.)).bg(t.surface))
                .child(probe(&self.probes, "send"))
                .into_any_element()
        } else {
            let live = !empty && !settling;
            div()
                .id("send")
                .relative()
                .flex_none()
                .size(px(size::CONTROL))
                .rounded(px(radius::MD))
                .flex()
                .items_center()
                .justify_center()
                .bg(if live { t.ink } else { t.ink.opacity(0.12) })
                .when(live, |d| {
                    d.cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                })
                .child(icon(
                    "arrow-up",
                    15.,
                    if live { t.surface } else { t.faint },
                ))
                .child(probe(&self.probes, "send"))
                .into_any_element()
        };
        let model = s.agent.split(" · ").nth(1).unwrap_or(&s.agent).to_string();
        let permission = s.permission.label().to_string();
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
                    .child(icon(a.icon(), 12., t.quiet))
                    .child(div().truncate().ty(UI).text_color(t.ink).child(a.label()))
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
                            .child(self.chip(Menu::SessionModel, "sparkle", &model, false, cx))
                            .child(self.chip(
                                Menu::SessionPermission,
                                "shield",
                                &permission,
                                false,
                                cx,
                            ))
                            // Short of room, the chip keeps its icon.
                            .child(self.chip(
                                Menu::SessionEffort,
                                "gauge",
                                if self.chat_w >= 620. {
                                    effort_label(s.effort.as_deref())
                                } else {
                                    ""
                                },
                                false,
                                cx,
                            ))
                            .child(div().flex_1())
                            .when_some(
                                s.context.filter(|_| self.chat_w >= 680.),
                                |d, (used, max)| {
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
                                },
                            )
                            // The hint gives way before the buttons do.
                            .when(self.chat_w >= 560., |d| {
                                d.child(hint(
                                    if s.status == Status::Needs {
                                        match s.items.iter().rev().find_map(|i| match i {
                                            Item::Question { .. } => Some("↩ answer"),
                                            Item::Approval { .. } | Item::Plan { .. } => {
                                                Some("↩ queue · Deny sends what’s typed")
                                            }
                                            _ => None,
                                        }) {
                                            Some(h) => h,
                                            None => "↩ send",
                                        }
                                    } else if working {
                                        "↩ queue · ⌘↩ steer"
                                    } else {
                                        "↩ send"
                                    },
                                    &t,
                                ))
                            })
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
        let hits = self.slash_matches(cx);
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
                        this.complete_slash(&name, window, cx);
                        this.composer.update(cx, |c, cx| c.focus(window, cx));
                    }))
                    .child(
                        div()
                            .flex_none()
                            .ty(CODE)
                            .text_color(t.ink)
                            .child(format!("/{}", c.name)),
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
            .child(self.chip(Menu::Model, "sparkle", &model, false, cx))
            .child(self.chip(Menu::Permission, "shield", d.permission.label(), false, cx))
            .child(self.chip(
                Menu::Effort,
                "gauge",
                effort_label(d.effort.as_deref()),
                false,
                cx,
            ));
        let start = div()
            .id("start")
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
            .bg(if empty || self.creating {
                t.ink.opacity(0.12)
            } else {
                t.ink
            })
            .text_color(if empty || self.creating {
                t.faint
            } else {
                t.surface
            })
            .when(!empty && !self.creating, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
            })
            .child(if self.creating {
                "Starting…"
            } else {
                "Start"
            })
            .child(div().ty(LABEL).opacity(0.7).child("↩"))
            .child(probe(&self.probes, "start"));
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
                let detail = if r.last.is_empty() || r.last == r.title {
                    Self::project_name(&r.cwd)
                } else {
                    format!("{} · last: {}", Self::project_name(&r.cwd), r.last)
                };
                div()
                    .id(SharedString::from(format!("adopt-{i}")))
                    .relative()
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.hover()))
                    .on_click(cx.listener(move |this, _, _, cx| this.adopt(&id, cx)))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(icon("square-terminal", 14., t.quiet))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .ty(UI)
                                    .text_color(t.ink)
                                    .child(r.title.clone()),
                            )
                            .child(div().truncate().ty(UI).text_color(t.quiet).child(detail)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .ty(LABEL)
                            .text_color(t.faint)
                            .child(crate::data::ago(r.updated)),
                    )
                    .child(probe(&self.probes, format!("adopt-{i}")))
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .pt(px(space::S6))
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
        div()
            .relative()
            .child(
                div()
                    .id(SharedString::from(name.clone()))
                    .h(px(size::CONTROL_SM + 2.))
                    .px(px(space::S2))
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .when(open, |d| d.bg(t.selected()))
                    .hover(|d| d.bg(t.hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = if this.menu == Some(which) {
                            None
                        } else {
                            Some(which)
                        };
                        cx.notify();
                    }))
                    .child(icon(icon_name, 13., t.quiet))
                    .when(!label.is_empty(), |d| {
                        d.child(
                            div()
                                .ty(UI)
                                .text_color(if unset { t.quiet } else { t.ink })
                                .child(label.to_string()),
                        )
                    })
                    // An icon-only chip still says what it is.
                    .when(label.is_empty(), |d| {
                        d.tooltip(tip(
                            match which {
                                Menu::Effort | Menu::SessionEffort => "Effort",
                                Menu::Model | Menu::SessionModel => "Model",
                                Menu::Permission | Menu::SessionPermission => "Permissions",
                                Menu::Project => "Project",
                            },
                            None,
                            t,
                        ))
                    })
                    .child(icon("chevron-down", 11., t.faint))
                    .child(probe(&self.probes, name)),
            )
            .when(open, |d| {
                let list = self.menu_list(which, cx);
                let placed = if which.upward() {
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .snap_to_window_with_margin(px(8.))
                        .child(div().mb(px(6.)).child(list))
                } else {
                    anchored()
                        .snap_to_window_with_margin(px(8.))
                        .child(div().mt(px(30.)).child(list))
                };
                d.child(deferred(placed).with_priority(1))
            })
            .into_any_element()
    }

    fn menu_list(&mut self, which: Menu, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.t;
        let d = self.defaults.clone();
        let rows: Vec<AnyElement> = match which {
            Menu::Project => {
                let mut v: Vec<AnyElement> = self
                    .projects
                    .iter()
                    .take(12)
                    .map(|p| {
                        let path = p.path.clone();
                        let on = d.project.as_deref() == Some(p.path.as_str());
                        let note = if on && !d.project_picked {
                            "Guessed from what you typed".to_string()
                        } else {
                            p.path.replacen(&home(), "~", 1)
                        };
                        menu_row(
                            &self.probes,
                            format!("m-{}", p.name),
                            &p.name,
                            Some(note),
                            on,
                            &t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.defaults.project = Some(path.clone());
                            this.defaults.project_picked = true;
                            this.menu = None;
                            cx.notify();
                        }))
                        .into_any_element()
                    })
                    .collect();
                v.push(
                    menu_row(
                        &self.probes,
                        "m-none",
                        "No project",
                        Some("A scratch folder, ~/dent/scratch".into()),
                        d.project.is_none(),
                        &t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.defaults.project = None;
                        this.defaults.project_picked = true;
                        this.menu = None;
                        cx.notify();
                    }))
                    .into_any_element(),
                );
                v
            }
            Menu::Model | Menu::SessionModel => {
                let current = if which == Menu::Model {
                    d.model.clone()
                } else {
                    self.current().and_then(|s| s.model.clone())
                };
                let current = current.unwrap_or_else(|| "default".into());
                self.models_or_default()
                    .into_iter()
                    .map(|m| {
                        let id = m.id.clone();
                        let note = (!m.description.is_empty()).then(|| m.description.clone());
                        menu_row(
                            &self.probes,
                            format!("m-{}", m.id),
                            &m.name,
                            note,
                            current == m.id,
                            &t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if which == Menu::Model {
                                this.defaults.model = Some(id.clone());
                            } else {
                                this.set_model(id.clone(), cx);
                            }
                            this.menu = None;
                            cx.notify();
                        }))
                        .into_any_element()
                    })
                    .collect()
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
                        menu_row(
                            &self.probes,
                            format!("m-{p:?}").to_lowercase(),
                            p.label(),
                            Some(permission_note(p).to_string()),
                            current == p,
                            &t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if which == Menu::Permission {
                                this.defaults.permission = p;
                            } else {
                                this.set_permission(p, cx);
                            }
                            this.menu = None;
                            cx.notify();
                        }))
                        .into_any_element()
                    })
                    .collect()
            }
            Menu::Effort | Menu::SessionEffort => {
                let current = if which == Menu::Effort {
                    d.effort.clone()
                } else {
                    self.current().and_then(|s| s.effort.clone())
                };
                EFFORTS
                    .iter()
                    .map(|(value, note)| {
                        let v = value.map(str::to_string);
                        menu_row(
                            &self.probes,
                            format!("m-effort-{}", value.unwrap_or("default")),
                            effort_label(*value),
                            Some(note.to_string()),
                            current.as_deref() == *value,
                            &t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if which == Menu::Effort {
                                this.defaults.effort = v.clone();
                            } else {
                                this.set_effort(v.clone(), cx);
                            }
                            this.menu = None;
                            cx.notify();
                        }))
                        .into_any_element()
                    })
                    .collect()
            }
        };
        floating(&t)
            .id("menu")
            .w(px(280.))
            .py(px(space::S1))
            .flex()
            .flex_col()
            .occlude()
            .children(rows)
    }
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

/// Effort levels, with what each is for.
const EFFORTS: [(Option<&str>, &str); 6] = [
    (None, "The model's own default"),
    (Some("low"), "Fastest; for quick questions"),
    (Some("medium"), "Balanced"),
    (Some("high"), "Thinks harder before acting"),
    (Some("xhigh"), "For tricky problems"),
    (Some("max"), "Thinks longest; uses the most"),
];

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
fn sent_attachment(line: &str, t: &theme::Theme) -> Div {
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
