//! Cmd+K: find any session, closed ones included, or start one from what you
//! typed when nothing matches.

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::Press;

use crate::app::Dent;
use std::time::Duration;

use crate::ds::{DISPLAY_COMPACT, LABEL, Typed, UI, motion, radius, size, space};
use crate::theme::status_square;
use crate::ui::{Arrive, From, floating, icon, kbd, probe};

impl Dent {
    pub fn palette_view(
        &mut self,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let results = self.palette_results(cx);
        let files = self.palette_files(cx);
        let query = self.search.read(cx).value().trim().to_string();
        let n = results.len();
        let all = n + files.len();
        let at = at.min(if query.is_empty() {
            all.saturating_sub(1)
        } else {
            all
        });
        let vw = window.viewport_size();
        let width = (vw.width.as_f32() - 2. * space::S8).min(560.);
        let top = (vw.height.as_f32() * 0.12).clamp(space::S4, 96.);
        let max_h = (vw.height.as_f32() - top - space::S8).min(480.);
        // The highlighted row stays in view as the arrows move it.
        if self.palette_shown != Some(at) {
            self.palette_shown = Some(at);
            self.palette_scroll.scroll_to_item(at);
        }
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
                        .when(!on, |d| {
                            d.hover(|d| d.bg(t.hover())).active(|d| d.bg(t.press()))
                        })
                        .on_press(cx.listener(move |this, _, window, cx| {
                            this.palette_open(id.clone(), window, cx);
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
                                .child(
                                    div().truncate().ty(UI).text_color(t.quiet).child(
                                        match self.search_snippet(&s, cx) {
                                            Some(found) => {
                                                // The word found, in ink, where it was found.
                                                let line = format!("{} · {found}", s.project);
                                                let q = self.search.read(cx).value().to_lowercase();
                                                let w =
                                                    q.split_whitespace().next().unwrap_or_default();
                                                let lower = line.to_lowercase();
                                                let start = s.project.len() + 3;
                                                match lower[start..].find(w).map(|i| i + start) {
                                                    Some(at)
                                                        if !w.is_empty()
                                                            && line.is_char_boundary(at)
                                                            && line
                                                                .is_char_boundary(at + w.len()) =>
                                                    {
                                                        let base = font(crate::theme::sans());
                                                        let mut hit = base.clone();
                                                        hit.weight = FontWeight::MEDIUM;
                                                        let run = |len, f: Font, c| TextRun {
                                                            len,
                                                            font: f,
                                                            color: c,
                                                            background_color: None,
                                                            underline: None,
                                                            strikethrough: None,
                                                        };
                                                        StyledText::new(line.clone())
                                                            .with_runs(vec![
                                                                run(at, base.clone(), t.quiet),
                                                                run(w.len(), hit, t.ink),
                                                                run(
                                                                    line.len() - at - w.len(),
                                                                    base,
                                                                    t.quiet,
                                                                ),
                                                            ])
                                                            .into_any_element()
                                                    }
                                                    _ => line.into_any_element(),
                                                }
                                            }
                                            None => format!("{} · {}", s.project, s.detail)
                                                .into_any_element(),
                                        },
                                    ),
                                ),
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
                        // Return opens whichever row is lit; its keycap says so.
                        .when(on, |d| {
                            d.child(div().h(px(20.)).flex().items_center().child(kbd("↩", &t)))
                        })
                        .child(probe(&self.probes, format!("result-{i}")))
                        .into_any_element(),
                )
            })
            .collect();
        // Files of the session you're in, under the sessions.
        let project = self
            .current()
            .map(|s| s.project.clone())
            .unwrap_or_default();
        let file_rows: Vec<AnyElement> = files
            .iter()
            .enumerate()
            .map(|(j, path)| {
                let i = n + j;
                let on = i == at;
                let (dir, name) = match path.rsplit_once('/') {
                    Some((d, f)) => (d.to_string(), f.to_string()),
                    None => (String::new(), path.clone()),
                };
                div()
                    .id(SharedString::from(format!("result-{i}")))
                    .relative()
                    .mx(px(space::S1))
                    .px(px(space::S3))
                    .h(px(size::ROW + 8.))
                    .rounded(px(radius::MD))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.selected()))
                    .when(!on, |d| {
                        d.hover(|d| d.bg(t.hover())).active(|d| d.bg(t.press()))
                    })
                    .on_press(cx.listener(move |this, _, window, cx| {
                        this.palette = Some(i);
                        this.palette_open_selected(window, cx);
                    }))
                    .child(icon("file", 14., t.quiet))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(space::S2))
                            .child(div().flex_none().ty(UI).text_color(t.ink).child(name))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .ty(LABEL)
                                    .text_color(t.quiet)
                                    .child(dir),
                            ),
                    )
                    .when(on, |d| d.child(kbd("↩", &t)))
                    .child(probe(&self.probes, format!("result-{i}")))
                    .into_any_element()
            })
            .collect();
        let file_head = (!file_rows.is_empty()).then(|| {
            div()
                .px(px(space::S4))
                .pt(px(space::S3))
                .pb(px(space::S1))
                .ty(LABEL)
                .text_color(t.quiet)
                .child(format!("Files in {project}"))
        });
        let empty = rows.is_empty() && file_rows.is_empty();
        div()
            .id("palette-scrim")
            // Takes the press that dismisses it: nothing under it is clicked.
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(if t.dark {
                // Darker, not lighter: the ink is light in Carbon.
                gpui_kit::black().opacity(0.45)
            } else {
                t.ink.opacity(0.08)
            })
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
                    // Fits the window, short or narrow ones too.
                    .mt(px(top))
                    .w(px(width))
                    .max_h(px(max_h))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
                                    // The query is set in the display face (type.display, compact).
                                    .ty(DISPLAY_COMPACT)
                                    .font_family(crate::theme::serif())
                                    .text_color(t.ink)
                                    .child(Textarea::new(&self.search)),
                            ),
                    )
                    .child(
                        div()
                            .id("results")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.palette_scroll)
                            .py(px(space::S1))
                            .children(rows)
                            .children(file_head)
                            .children(file_rows)
                            // What you typed can always start a session, matches or not.
                            .when(empty || !query.is_empty(), |d| {
                                let on = at >= all;
                                d.child(
                                    div()
                                        .id("palette-start")
                                        .mx(px(space::S1))
                                        .px(px(space::S3))
                                        .h(px(size::ROW + 8.))
                                        .rounded(px(radius::MD))
                                        .cursor_pointer()
                                        .when(on, |d| d.bg(t.selected()))
                                        .when(!on, |d| d.hover(|d| d.bg(t.hover())))
                                        .on_press(cx.listener(move |this, _, window, cx| {
                                            this.palette = Some(all);
                                            this.palette_open_selected(window, cx);
                                        }))
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
                                        .when(on, |d| d.child(kbd("↩", &t))),
                                )
                            }),
                    )
                    .arrive("palette-arrive", From::Above, motion::FAST),
            )
            // The dim comes up with it, so opening never flashes.
            .with_animation(
                "palette-scrim-in",
                Animation::new(Duration::from_millis(motion::FAST as u64))
                    .with_easing(crate::anim::ease_out),
                |d, p| d.opacity(p),
            )
            .into_any_element()
    }
}
