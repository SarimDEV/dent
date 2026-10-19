//! A page in a window of its own: bigger than the viewer, beside the chat or
//! on another screen. It follows new versions and still takes Point.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::Press;

use crate::app::Dent;
use crate::data::Kind;
use crate::ds::{LABEL, Typed, UI, radius, size, space};
use crate::theme::{Status, Theme, status_square};
use crate::ui::{button, icon, icon_button};
use crate::web::Preview;

pub struct PopOut {
    dent: Entity<Dent>,
    art: &'static str,
    t: Theme,
    preview: Option<Rc<RefCell<Preview>>>,
    picking: bool,
}

impl PopOut {
    pub fn new(
        dent: Entity<Dent>,
        art: &'static str,
        t: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // It follows the session: a new version shows here as it lands.
        cx.observe(&dent, |_, _, cx| cx.notify()).detach();
        // Closing the window puts the page back in the viewer.
        let d = dent.clone();
        window.on_window_should_close(cx, move |_, cx| {
            // Close the same way Back to the viewer does: page first.
            if let Some((h, view)) = d.update(cx, |d, cx| {
                cx.notify();
                d.popouts.remove(art)
            }) {
                close(h, view, cx);
            }
            false
        });
        // The page talks back on its own schedule; look in on it.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let Ok(due) = this.update(cx, |this, _| {
                    this.preview.as_ref().is_some_and(|p| {
                        let p = p.borrow();
                        let owed = p.tick();
                        p.pending() || owed
                    })
                }) else {
                    break;
                };
                if due && this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            dent,
            art,
            t,
            preview: std::env::var_os("DENT_NO_PREVIEW")
                .is_none()
                .then(|| Preview::new(window).map(|p| Rc::new(RefCell::new(p))))
                .flatten(),
            picking: false,
        }
    }
}

/// Close a page's window. The web view goes first and the window a moment
/// later: on Linux GTK tears the page down on its next pass, and a page
/// outliving its window is a fatal X error.
pub fn close(h: AnyWindowHandle, view: Entity<PopOut>, cx: &mut App) {
    view.update(cx, |p, _| {
        if let Some(page) = p.preview.take() {
            page.borrow_mut().set_visible(false);
        }
    });
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(120))
            .await;
        let _ = h.update(cx, |_, w, _| w.remove_window());
    })
    .detach();
}

impl Render for PopOut {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // dent's theme is the one theme: light or dark follows it here too.
        self.t = self.dent.read(cx).t;
        let t = self.t;
        let art = self.art;
        let Some(a) = self.dent.read(cx).art(art).cloned() else {
            return div().into_any_element();
        };
        let server = a.kind == Kind::Server;
        let latest = a.versions.len().max(1);
        let title = format!("{} · {}", a.title, a.path);
        window.set_window_title(&title);
        if let Some(p) = &self.preview {
            let mut p = p.borrow_mut();
            if server {
                p.show_server(a.path);
            } else if a.kind == Kind::Media {
                if let Some(f) = crate::viewer::art_file(&a) {
                    p.show_file(&f);
                }
            } else {
                p.show(a.path, latest, &a.versions[latest - 1], a.dir.as_deref());
            }
            p.set_visible(true);
        }
        // Errors and picks go to the session, in the main window.
        if let Some(p) = &self.preview {
            let (events, showing, retired) = {
                let p = p.borrow();
                (p.drain(), p.showing.clone(), p.retired())
            };
            for image in retired {
                let _ = window.drop_image(image);
            }
            if !events.is_empty() {
                if events.iter().any(|e| {
                    matches!(
                        e,
                        crate::web::PageEvent::Pick { .. } | crate::web::PageEvent::PickCancel
                    )
                }) {
                    self.picking = false;
                }
                self.dent
                    .update(cx, |d, cx| d.page_events(art, showing, events, window, cx));
            }
        }
        let errors = self
            .dent
            .read(cx)
            .errors
            .get(&(art, latest))
            .cloned()
            .unwrap_or_default();
        let preview = self.preview.clone();
        let picking = self.picking;
        let header = div()
            .h(px(size::HEADER))
            .flex_none()
            .pl(px(80.))
            .pr(px(space::S2))
            .flex()
            .items_center()
            .gap(px(space::S2))
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
                    .gap(px(space::S2))
                    .child(
                        div()
                            .flex_shrink_0()
                            .ty(UI)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.ink)
                            .child(a.title),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(if server {
                                a.path.to_string()
                            } else {
                                format!("{} · v{latest}", a.path)
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S1))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .id("pop-pick")
                            .h(px(size::CONTROL))
                            .px(px(space::S2))
                            .flex()
                            .items_center()
                            .gap(px(space::S1H))
                            .rounded(px(radius::MD))
                            .cursor_pointer()
                            .when(picking, |d| d.bg(t.ink).text_color(t.surface))
                            .when(!picking, |d| d.text_color(t.ink).hover(|d| d.bg(t.hover())))
                            .on_press(cx.listener(|this, _, _, cx| {
                                this.picking = !this.picking;
                                if let Some(p) = &this.preview {
                                    p.borrow().pick(this.picking);
                                }
                                cx.notify();
                            }))
                            .child(icon(
                                "square-dashed-mouse-pointer",
                                14.,
                                if picking { t.surface } else { t.quiet },
                            ))
                            .child(div().ty(UI).child("Point")),
                    )
                    .child(
                        icon_button("pop-reload", "rotate-cw", &t).on_press(cx.listener(
                            |this, _, _, cx| {
                                if let Some(p) = &this.preview {
                                    p.borrow_mut().reload();
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button("pop-back", "Back to the viewer", false, &t).on_press(cx.listener(
                            move |this, _, _, cx| {
                                this.dent.update(cx, |d, cx| d.bring_back(art, cx));
                            },
                        )),
                    ),
            );
        let area = div().flex_1().min_h_0().child(
            canvas(
                move |bounds, _, _| {
                    if let Some(p) = &preview {
                        p.borrow_mut().place(bounds);
                    }
                },
                |_, _, _, _| {},
            )
            .size_full(),
        );
        let console = errors.first().cloned().map(|e| {
            div()
                .min_h(px(40.))
                .flex_none()
                .px(px(space::S3))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .border_t_1()
                .border_color(t.edge(t.needs.0))
                .bg(t.wash(t.needs.0))
                .child(status_square(Status::Needs, &t))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .ty(UI)
                        .text_color(t.ink)
                        .child(format!("{e} · Ask to fix from the session")),
                )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .font_family(crate::theme::sans())
            .child(header)
            .child(area)
            .children(console)
            .into_any_element()
    }
}
