//! The main column: a session's conversation and composer, or the new-session
//! page where you start by typing.

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::anim;
use crate::app::{Dent, Menu, Tab};
use crate::data::{self, Item, Kind, Session};
use crate::ds::{
    CODE, DISPLAY, DISPLAY_COMPACT, LABEL, READING, Typed, UI, motion, radius, size, space,
};
use crate::markdown;
use crate::sidebar::clock;
use crate::theme::{self, Status, status_square};
use crate::ui::{button, floating, ghost, icon, icon_button, kbd, menu_row, probe, tip};

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
        if self.stopped.contains(&s.id) && !s.closed {
            let sid = id.clone();
            actions = actions.child(
                button("resume", "Resume", false, &t)
                    .on_click(cx.listener(move |this, _, _, cx| this.resume(&sid, cx))),
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
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(relative(0.7))
                            .truncate()
                            .ty(DISPLAY_COMPACT)
                            .text_color(t.ink)
                            .child(s.title.clone()),
                    )
                    .child(meta),
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
            .child(div().text_color(t.ink).child("Working"))
            .child(div().text_color(t.faint).child("·"))
            .child(div().flex_1().min_w_0().truncate().child(s.detail.clone()))
            .when_some(since, |d, at| {
                d.child(
                    div()
                        .ty(LABEL)
                        .text_color(t.faint)
                        .child(clock(at.elapsed().as_secs())),
                )
            })
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
            Item::User(text) => {
                let (atts, body): (Vec<&str>, Vec<&str>) =
                    text.lines().partition(|l| l.starts_with("› "));
                div()
                    .flex()
                    .justify_end()
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
                                d.child(div().ty(READING).text_color(t.ink).child(body.join("\n")))
                            }),
                    )
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
            Item::Tools {
                summary,
                lines,
                open,
            } => {
                let (sid, open) = (s.id.clone(), *open);
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
                                if let Some(Item::Tools { open, .. }) = this
                                    .sessions
                                    .iter_mut()
                                    .find(|x| x.id == sid)
                                    .map(std::rc::Rc::make_mut)
                                    .and_then(|x| x.items.get_mut(ix))
                                {
                                    *open = !*open;
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
            Item::Approval { command, why } => {
                let (a, d) = (s.id.clone(), s.id.clone());
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
                                    .child("Wants to run a command"),
                            ),
                    )
                    .child(
                        div()
                            .px(px(space::S3))
                            .py(px(space::S2))
                            .rounded(px(radius::SM))
                            .bg(t.raised)
                            .border_1()
                            .border_color(t.hairline)
                            .ty(CODE)
                            .text_color(t.ink)
                            .child(format!("$ {command}")),
                    )
                    .child(div().ty(UI).text_color(t.quiet).child(why.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(space::S2))
                            .child(
                                button("allow", "Allow once", true, &t)
                                    .relative()
                                    .on_click(cx.listener(move |this, _, _, cx| this.allow(&a, cx)))
                                    .child(probe(&self.probes, "allow")),
                            )
                            .child(button("allow-session", "Allow for this session", false, &t))
                            .child(div().flex_1())
                            .child(
                                button("deny", "Deny", false, &t)
                                    .on_click(cx.listener(move |this, _, _, cx| this.deny(&d, cx))),
                            ),
                    )
                    .into_any_element()
            }
            Item::Question { text, options } => div()
                .p(px(space::S4))
                .rounded(px(radius::LG))
                .bg(t.wash(t.needs.0))
                .border_1()
                .border_color(t.edge(t.needs.0))
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(status_square(Status::Needs, &t))
                        .child(div().ty(READING).text_color(t.ink).child(text.clone())),
                )
                .child(div().flex().flex_wrap().gap(px(space::S2)).children(
                    options.iter().enumerate().map(|(i, o)| {
                        let (sid, o2) = (s.id.clone(), o.clone());
                        button(
                            SharedString::from(format!("opt-{i}")),
                            o.clone(),
                            i == 0,
                            &t,
                        )
                        .relative()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.answer(&sid, o2.clone(), cx)),
                        )
                        .child(probe(&self.probes, format!("opt-{i}")))
                    }),
                ))
                .into_any_element(),
            Item::Changed {
                files,
                added,
                removed,
            } => {
                let plural = if *files == 1 { "file" } else { "files" };
                div()
                    .id(SharedString::from(format!("changed-{ix}")))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .ty(CODE)
                    .text_color(t.quiet)
                    .cursor_pointer()
                    .hover(|d| d.text_color(t.ink))
                    .on_click(cx.listener(|this, _, _, cx| this.show_tab(Tab::Changes, cx)))
                    .child(div().text_color(t.faint).child("├"))
                    .child(format!("Changed {files} {plural}"))
                    .child(diffstat(*added, *removed, &t))
                    .child(
                        div()
                            .text_color(t.faint)
                            .child(format!("+{added} −{removed}")),
                    )
                    .child(div().ty(UI).text_color(t.ink).child("Review"))
                    .into_any_element()
            }
            Item::Pr {
                number,
                title,
                checks,
            } => div()
                .px(px(space::S4))
                .py(px(space::S3))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(t.hairline)
                .flex()
                .items_center()
                .gap(px(space::S3))
                .child(icon("git-pull-request", 16., t.quiet))
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
                                .text_color(t.ink)
                                .child(format!("#{number} {title}")),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S1H))
                                .child(status_square(Status::Done, &t))
                                .child(div().ty(UI).text_color(t.quiet).child(checks.clone())),
                        ),
                )
                .child(ghost("open-pr", "Open on GitHub", &t))
                .into_any_element(),
            Item::Note { icon: name, text } => div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .ty(UI)
                .text_color(t.quiet)
                .child(icon(name, 13., t.quiet))
                .child(text.clone())
                .into_any_element(),
            Item::Queued(text) => div()
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
                        .ty(LABEL)
                        .text_color(t.faint)
                        .child("queued · sends when this reply finishes"),
                )
                .into_any_element(),
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
            format!("{} · on {}, forwarded here", a.path, s.machine)
        } else {
            format!("{file} · v{version}")
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
        let working = s.status == Status::Working;
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
        let permission = s.permission.clone();
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
                            .child(div().flex_1())
                            .when(!working, |d| d.child(kbd("↩ send", &t)))
                            .child(action),
                    )
                    .child(probe(&self.probes, "composer")),
            )
    }

    fn starter_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let text = self.starter.read(cx).value();
        let empty = text.trim().is_empty();
        let d = self.defaults.clone();
        // Everything is already chosen; each choice is one click to change.
        let chips = div()
            .flex()
            .flex_wrap()
            .gap(px(space::S1))
            .ml(px(-space::S2))
            .child(self.chip(
                Menu::Project,
                "folder",
                d.project.unwrap_or("No project"),
                d.project.is_none(),
                cx,
            ))
            .child(self.chip(Menu::Machine, "monitor", d.machine, false, cx))
            .child(self.chip(
                Menu::Model,
                "sparkle",
                d.model.split(" · ").nth(1).unwrap_or(d.model),
                false,
                cx,
            ))
            .child(self.chip(Menu::Permission, "shield", d.permission, false, cx))
            .child(self.chip(Menu::Worktree, "git-branch", d.worktree, false, cx));
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
            .bg(if empty { t.ink.opacity(0.12) } else { t.ink })
            .text_color(if empty { t.faint } else { t.surface })
            .when(!empty, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
            })
            .child("Start")
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
                    .child(chips),
            )
            .child(div().flex_1())
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
                    .child(
                        div()
                            .ty(UI)
                            .text_color(if unset { t.quiet } else { t.ink })
                            .child(label.to_string()),
                    )
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
                let mut v: Vec<AnyElement> = data::PROJECTS
                    .iter()
                    .map(|(p, _)| {
                        let p: &'static str = p;
                        let on = d.project == Some(p);
                        let note = (on && !d.project_picked)
                            .then(|| "Guessed from what you typed".to_string());
                        menu_row(&self.probes, format!("m-{p}"), p, note, on, &t)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.defaults.project = Some(p);
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
                        Some("A scratch folder on the machine".into()),
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
            Menu::Machine => {
                let least = data::MACHINES
                    .iter()
                    .filter(|m| m.online)
                    .min_by_key(|m| m.running)
                    .map(|m| m.name);
                let mut machines: Vec<&data::Machine> = data::MACHINES.iter().collect();
                machines.sort_by_key(|m| m.running);
                machines
                    .into_iter()
                    .map(|m| {
                        let name = m.name;
                        let running = match m.running {
                            0 => "Nothing running".to_string(),
                            1 => "1 session running".to_string(),
                            n => format!("{n} sessions running"),
                        };
                        let note = if Some(name) == least {
                            format!("Least busy · {running}")
                        } else {
                            running
                        };
                        menu_row(
                            &self.probes,
                            format!("m-{name}"),
                            name,
                            Some(note),
                            d.machine == name,
                            &t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.defaults.machine = name;
                            this.menu = None;
                            cx.notify();
                        }))
                        .into_any_element()
                    })
                    .collect()
            }
            Menu::Model => data::MODELS
                .iter()
                .map(|m| {
                    let m: &'static str = m;
                    let note = (d.model == m).then(|| "Used last time".to_string());
                    menu_row(&self.probes, format!("m-{m}"), m, note, d.model == m, &t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.defaults.model = m;
                            this.menu = None;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect(),
            Menu::Permission => data::PERMISSIONS
                .iter()
                .map(|(p, note)| {
                    let p: &'static str = p;
                    menu_row(
                        &self.probes,
                        format!("m-{p}"),
                        p,
                        Some(note.to_string()),
                        d.permission == p,
                        &t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.defaults.permission = p;
                        this.menu = None;
                        cx.notify();
                    }))
                    .into_any_element()
                })
                .collect(),
            Menu::SessionModel => {
                let current = self.current().map(|s| s.agent.clone()).unwrap_or_default();
                data::MODELS
                    .iter()
                    .map(|m| {
                        let m: &'static str = m;
                        menu_row(&self.probes, format!("m-{m}"), m, None, current == m, &t)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(id) = this.selected.clone()
                                    && let Some(s) = this
                                        .sessions
                                        .iter_mut()
                                        .find(|s| s.id == id)
                                        .map(std::rc::Rc::make_mut)
                                {
                                    s.agent = m.to_string();
                                }
                                this.defaults.model = m;
                                this.menu = None;
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect()
            }
            Menu::SessionPermission => {
                let current = self
                    .current()
                    .map(|s| s.permission.clone())
                    .unwrap_or_default();
                data::PERMISSIONS
                    .iter()
                    .map(|(p, note)| {
                        let p: &'static str = p;
                        menu_row(
                            &self.probes,
                            format!("m-{p}"),
                            p,
                            Some(note.to_string()),
                            current == p,
                            &t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(id) = this.selected.clone()
                                && let Some(s) = this
                                    .sessions
                                    .iter_mut()
                                    .find(|s| s.id == id)
                                    .map(std::rc::Rc::make_mut)
                            {
                                s.permission = p.to_string();
                            }
                            this.defaults.permission = p;
                            this.menu = None;
                            cx.notify();
                        }))
                        .into_any_element()
                    })
                    .collect()
            }
            Menu::Worktree => data::WORKTREES
                .iter()
                .map(|(w, note)| {
                    let w: &'static str = w;
                    menu_row(
                        &self.probes,
                        format!("m-{w}"),
                        w,
                        Some(note.to_string()),
                        d.worktree == w,
                        &t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.defaults.worktree = w;
                        this.menu = None;
                        cx.notify();
                    }))
                    .into_any_element()
                })
                .collect(),
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

/// A small bar of five squares: how much of the change is additions.
fn diffstat(added: usize, removed: usize, t: &theme::Theme) -> Div {
    let total = (added + removed).max(1) as f32;
    let green = ((added as f32 / total) * 5.).round() as usize;
    div().flex().gap(px(1.)).children((0..5).map(move |i| {
        div()
            .size(px(7.))
            .bg(if i < green { t.done.0 } else { t.hairline })
    }))
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
