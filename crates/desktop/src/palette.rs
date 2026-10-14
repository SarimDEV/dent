//! Cmd+K: find any session, closed ones included, or start one from what you
//! typed when nothing matches.

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::Dent;
use crate::ds::{LABEL, READING, Typed, UI, radius, size, space};
use crate::theme::status_square;
use crate::ui::{floating, icon, kbd, probe};

impl Dent {
    pub fn palette_view(&mut self, at: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let results = self.palette_results(cx);
        let query = self.search.read(cx).value().trim().to_string();
        let at = at.min(results.len().saturating_sub(1));
        let n = results.len();
        let rows: Vec<AnyElement> = results
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let s = self.session(id)?.clone();
                let on = i == at;
                let id = id.clone();
                Some(
                    div()
                        .id(SharedString::from(format!("result-{i}")))
                        .relative()
                        .mx(px(space::S1))
                        .px(px(space::S3))
                        .py(px(space::S2))
                        .rounded(px(radius::MD))
                        .flex()
                        .gap(px(space::S3))
                        .cursor_pointer()
                        .when(on, |d| d.bg(t.selected()))
                        .when(!on, |d| d.hover(|d| d.bg(t.hover())))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.palette = Some(i);
                            let _ = &id;
                            this.palette_open_selected(window, cx);
                        }))
                        .child(div().h(px(20.)).flex().items_center().child(if s.closed {
                            icon("archive", 12., t.faint).into_any_element()
                        } else {
                            status_square(s.status, &t).into_any_element()
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .ty(UI)
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t.ink)
                                        .child(s.title.clone()),
                                )
                                .child(div().truncate().ty(UI).text_color(t.quiet).child(
                                    match self.search_snippet(&s, cx) {
                                        Some(found) => format!("{} · {found}", s.project),
                                        None => format!("{} · {}", s.project, s.detail),
                                    },
                                )),
                        )
                        .when(s.closed, |d| {
                            d.child(
                                div()
                                    .h(px(20.))
                                    .flex()
                                    .items_center()
                                    .ty(LABEL)
                                    .text_color(t.faint)
                                    .child("closed"),
                            )
                        })
                        .child(probe(&self.probes, format!("result-{i}")))
                        .into_any_element(),
                )
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .id("palette-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(t.ink.opacity(if t.dark { 0.3 } else { 0.08 }))
            .flex()
            .justify_center()
            // Sized by its results, not stretched to the window.
            .items_start()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
            )
            .child(
                floating(&t)
                    .id("palette")
                    .mt(px(96.))
                    .w(px(560.))
                    .max_h(px(480.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // Arrows move through results before the text field sees them.
                    .capture_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                        match e.keystroke.key.as_str() {
                            "down" => {
                                this.palette =
                                    Some((this.palette.unwrap_or(0) + 1).min(n.saturating_sub(1)));
                                cx.stop_propagation();
                                cx.notify();
                            }
                            "up" => {
                                this.palette = Some(this.palette.unwrap_or(0).saturating_sub(1));
                                cx.stop_propagation();
                                cx.notify();
                            }
                            _ => {}
                        }
                    }))
                    .child(
                        div()
                            .px(px(space::S4))
                            .py(px(space::S3))
                            .flex()
                            .items_center()
                            .gap(px(space::S3))
                            .border_b_1()
                            .border_color(t.hairline)
                            .child(icon("search", 15., t.quiet))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .ty(READING)
                                    .text_color(t.ink)
                                    .child(Textarea::new(&self.search)),
                            ),
                    )
                    .child(
                        div()
                            .id("results")
                            .max_h(px(400.))
                            .overflow_y_scroll()
                            .py(px(space::S1))
                            .children(rows)
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .mx(px(space::S1))
                                        .px(px(space::S3))
                                        .h(px(size::ROW + 8.))
                                        .rounded(px(radius::MD))
                                        .bg(t.selected())
                                        .flex()
                                        .items_center()
                                        .gap(px(space::S2))
                                        .ty(UI)
                                        .text_color(t.ink)
                                        .child(icon("square-pen", 14., t.quiet))
                                        .child(div().flex_1().truncate().child(
                                            if query.is_empty() {
                                                "No sessions yet".to_string()
                                            } else {
                                                format!("Start a session: {query}")
                                            },
                                        ))
                                        .child(kbd("↩", &t)),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}
