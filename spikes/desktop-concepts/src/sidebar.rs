//! The sidebar is the inbox: what needs you, then what's done and waiting to
//! be steered or closed, then working sessions, lighter, at the bottom.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::anim;
use crate::app::Dent;
use crate::data::Session;
use crate::ds::{LABEL, Typed, UI, motion, radius, size, space};
use crate::theme::{Status, status_square_at};
use crate::ui::{icon, icon_button, kbd, probe, section_label, tip};

pub const WIDTH: f32 = 288.;

impl Dent {
    pub fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.t;
        let needs = self.needs();
        let done = self.done();
        let working = self.working();
        let closed = self.closed();
        let clear = needs.is_empty() && done.is_empty();
        let new_selected = self.selected.is_none();

        let mut body = div()
            .id("inbox")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb(px(space::S4))
            .flex()
            .flex_col();
        if clear {
            body = body.child(
                div()
                    .mx(px(space::S2))
                    .mt(px(space::S2))
                    .px(px(space::S2))
                    .py(px(space::S3))
                    .flex()
                    .gap(px(space::S3))
                    .child(div().pt(px(4.)).child(icon("check-check", 16., t.quiet)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .ty(UI)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.ink)
                                    .child("All caught up"),
                            )
                            .child(
                                div()
                                    .ty(UI)
                                    .text_color(t.quiet)
                                    .child("Everything is working or closed."),
                            ),
                    ),
            );
        }
        for (label, list, quiet) in [
            ("Needs you", &needs, false),
            ("Done", &done, false),
            ("Working", &working, true),
        ] {
            if list.is_empty() {
                continue;
            }
            body = body.child(section_label(label, Some(list.len()), &t).mt(px(space::S3)));
            for s in list.iter() {
                body = body.child(self.row(s, quiet, cx));
            }
        }
        // Closed sessions are archived, never gone: one row opens them.
        if !closed.is_empty() {
            let open = self.show_closed;
            body = body.child(
                div()
                    .id("closed-toggle")
                    .relative()
                    .mt(px(space::S3))
                    .h(px(size::ROW))
                    .px(px(space::S4))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_closed = !this.show_closed;
                        cx.notify();
                    }))
                    .child(icon(
                        if open {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        },
                        12.,
                        t.faint,
                    ))
                    .child(div().ty(LABEL).text_color(t.quiet).child("CLOSED"))
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.faint)
                            .child(closed.len().to_string()),
                    )
                    .child(probe(&self.probes, "closed-toggle")),
            );
            if open {
                for s in closed.iter() {
                    body = body.child(self.row(s, false, cx));
                }
            }
        }

        div()
            .w(px(WIDTH))
            .flex_none()
            .h_full()
            .key_context("Inbox")
            .track_focus(&self.inbox_focus)
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.hairline)
            .child(
                // The traffic lights sit in this bar; the rest of it drags the window.
                div()
                    .h(px(size::HEADER))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(space::S1))
                    .pl(px(80.))
                    .pr(px(space::S2))
                    .on_mouse_down(MouseButton::Left, |e, window, _| {
                        if e.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(
                        icon_button("search", "search", &t)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_palette(window, cx)),
                            )
                            .tooltip(tip("Search sessions", Some("⌘K"), t)),
                    )
                    .child(
                        icon_button("new-session", "square-pen", &t)
                            .relative()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .when(new_selected, |d| d.bg(t.selected()))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.select(None, window, cx)),
                            )
                            .tooltip(tip("New session", Some("⌘N"), t))
                            .child(probe(&self.probes, "new-session")),
                    ),
            )
            .child(body)
    }

    fn row(&mut self, s: &Session, quiet: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let selected = self.selected.as_deref() == Some(s.id.as_str());
        let hovered = self.hovered_row.as_deref() == Some(s.id.as_str());
        let closable = Self::can_close(s);
        let stopped = self.stopped.contains(&s.id);
        // A row that just finished pops its square once; a row that just
        // arrived rises into place.
        let pop = self
            .finished_at
            .get(&s.id)
            .map(|at| {
                if crate::reduce_motion() {
                    1.
                } else {
                    0.6 + 0.4 * anim::pop(at.elapsed().as_secs_f32())
                }
            })
            .unwrap_or(1.);
        let arrive = self
            .arrived_at
            .get(&s.id)
            .map(|at| anim::ease_out(anim::progress(*at, motion::BASE)))
            .unwrap_or(1.);
        let id = s.id.clone();
        let (close_id, reopen_id) = (s.id.clone(), s.id.clone());
        let hover_id = s.id.clone();
        let mark: AnyElement = if s.closed {
            icon("archive", 12., t.faint).into_any_element()
        } else {
            status_square_at(s.status, &t, pop).into_any_element()
        };
        let title_ink = if quiet { t.quiet } else { t.ink };
        let detail = if s.closed {
            format!("Closed · {}", s.project)
        } else {
            format!("{} · {}", s.project, s.detail)
        };
        let trailing: AnyElement = if hovered && closable {
            div()
                .id(SharedString::from(format!("close-{id}")))
                .size(px(size::CONTROL_SM))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::SM))
                .cursor_pointer()
                .hover(|d| d.bg(t.press()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close(&close_id, window, cx);
                }))
                .tooltip(tip("Close", Some("⌘W"), t))
                .child(icon("archive", 13., t.quiet))
                .into_any_element()
        } else if hovered && s.closed {
            div()
                .id(SharedString::from(format!("reopen-{id}")))
                .size(px(size::CONTROL_SM))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::SM))
                .cursor_pointer()
                .hover(|d| d.bg(t.press()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.reopen(&reopen_id, cx);
                }))
                .tooltip(tip("Reopen", None, t))
                .child(icon("archive-restore", 13., t.quiet))
                .into_any_element()
        } else if stopped && !s.closed {
            div()
                .ty(LABEL)
                .text_color(t.quiet)
                .child("stopped")
                .into_any_element()
        } else {
            let age = match self.working_since.get(&s.id) {
                Some(since) if s.status == Status::Working => clock(since.elapsed().as_secs()),
                _ => s.age.clone(),
            };
            div()
                .ty(LABEL)
                .text_color(t.faint)
                .child(age)
                .into_any_element()
        };
        div()
            .id(SharedString::from(format!("row-{}", s.id)))
            .relative()
            .mx(px(space::S2))
            .px(px(space::S2))
            .py(px(space::S2))
            .rounded(px(radius::MD))
            // Keyboard focus in the inbox: a 2 px ink ring on the selected row.
            .when(selected && self.inbox_focused, |d| {
                d.border_2()
                    .border_color(t.ink)
                    .px(px(space::S2 - 2.))
                    .py(px(space::S2 - 2.))
            })
            .flex()
            .gap(px(space::S1H + 2.))
            .cursor_pointer()
            .opacity(arrive * if quiet && !selected { 0.72 } else { 1. })
            .top(px((1. - arrive) * motion::RISE))
            .when(selected, |d| d.bg(t.selected()))
            .when(!selected, |d| d.hover(|d| d.bg(t.hover())))
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                if *on {
                    this.hovered_row = Some(hover_id.clone());
                } else if this.hovered_row.as_deref() == Some(hover_id.as_str()) {
                    this.hovered_row = None;
                }
                cx.notify();
            }))
            .on_click(
                cx.listener(move |this, _, window, cx| this.select(Some(id.clone()), window, cx)),
            )
            .child(div().h(px(20.)).flex().items_center().child(mark))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .ty(UI)
                                    .text_color(title_ink)
                                    .when(!quiet, |d| d.font_weight(FontWeight::MEDIUM))
                                    .child(s.title.clone()),
                            )
                            .child(div().h(px(20.)).flex().items_center().child(trailing)),
                    )
                    .child(div().truncate().ty(UI).text_color(t.quiet).child(detail)),
            )
            .child(probe(&self.probes, format!("row-{}", s.id)))
            .into_any_element()
    }

    pub fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.t;
        let needs = self.needs().len();
        let online = crate::data::MACHINES.iter().filter(|m| m.online).count();
        div()
            .h(px(size::STATUS))
            .flex_none()
            .px(px(space::S4))
            .flex()
            .items_center()
            .gap(px(space::S4))
            .border_t_1()
            .border_color(t.hairline)
            .ty(LABEL)
            .text_color(t.quiet)
            .child(
                div()
                    .flex()
                    .gap(px(space::S1H))
                    .child("claude")
                    .child(div().text_color(t.ink).child("5h 38%"))
                    .child(div().text_color(t.faint).child("·"))
                    .child("week")
                    .child(div().text_color(t.ink).child("62%")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(space::S1H))
                    .child("codex")
                    .child("week")
                    .child(div().text_color(t.ink).child("18%")),
            )
            .child(div().flex_1())
            .child(format!("{online} machines online"))
            .when(needs > 0, |d| {
                d.child(
                    div()
                        .id("needs-count")
                        .flex()
                        .items_center()
                        .gap(px(space::S1H))
                        .px(px(space::S1H))
                        .h(px(size::CONTROL_SM))
                        .rounded(px(radius::SM))
                        .cursor_pointer()
                        .hover(|d| d.bg(t.hover()))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.next_needs_you(window, cx)),
                        )
                        .child(status_square_at(Status::Needs, &t, 1.))
                        .child(format!("{needs} need you"))
                        .child(kbd("⌘J", &t)),
                )
            })
    }
}

/// Elapsed time as m:ss, in a fixed-width face so it never jitters.
pub fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}
