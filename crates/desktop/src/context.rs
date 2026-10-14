//! Right-click menus: a short list of actions at the pointer. One is open
//! at a time; a click anywhere else or Esc closes it.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::Dent;
use crate::ds::{Typed, radius, size, space};
use crate::theme::Theme;

#[derive(Clone)]
pub struct ContextMenu {
    pub at: Point<Pixels>,
    pub items: Vec<(&'static str, Do)>,
}

/// What a row does.
#[derive(Clone)]
pub enum Do {
    CloseTab(&'static str),
    CloseOtherTabs(&'static str),
    CopyPath(String),
    Reveal(String),
    Rename(String),
    Fork(String),
    CloseSession(String),
    Reopen(String),
}

const WIDTH: f32 = 200.;

impl Dent {
    pub fn open_context(
        &mut self,
        at: Point<Pixels>,
        items: Vec<(&'static str, Do)>,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        self.context = Some(ContextMenu { at, items });
        cx.notify();
    }

    fn run_context(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        self.context = None;
        match what {
            Do::CloseTab(a) => self.close_tab(a, cx),
            Do::CloseOtherTabs(a) => self.close_other_tabs(a, cx),
            Do::CopyPath(p) => cx.write_to_clipboard(ClipboardItem::new_string(p)),
            Do::Reveal(p) => cx.reveal_path(std::path::Path::new(&p)),
            Do::Rename(id) => {
                self.select(Some(id.clone()), window, cx);
                let title = self
                    .sessions
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.title.to_string())
                    .unwrap_or_default();
                self.start_rename(&id, &title, window, cx);
            }
            Do::Fork(id) => self.fork(&id, None, cx),
            Do::CloseSession(id) => {
                self.close(&id, window, cx);
            }
            Do::Reopen(id) => self.reopen(&id, cx),
        }
        cx.notify();
    }

    /// The open menu, over everything, kept inside the window.
    pub fn context_view(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.context.clone()?;
        let t: Theme = self.t;
        let vw = window.viewport_size();
        let h = menu.items.len() as f32 * (size::ROW + 4.) + space::S2;
        let x = menu
            .at
            .x
            .as_f32()
            .min(vw.width.as_f32() - WIDTH - space::S2)
            .max(space::S2);
        let y = menu
            .at
            .y
            .as_f32()
            .min(vw.height.as_f32() - h - space::S2)
            .max(space::S2);
        Some(
            div()
                .id("context-scrim")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                    this.context = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("context-menu")
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(WIDTH))
                        .py(px(space::S1))
                        .flex()
                        .flex_col()
                        .bg(t.raised)
                        .border_1()
                        .border_color(t.hairline)
                        .rounded(px(radius::LG))
                        .shadow_md()
                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .children(
                            menu.items
                                .into_iter()
                                .enumerate()
                                .map(|(i, (label, what))| {
                                    div()
                                        .id(("context-row", i))
                                        .h(px(size::ROW + 4.))
                                        .mx(px(space::S1))
                                        .px(px(space::S2))
                                        .flex()
                                        .items_center()
                                        .rounded(px(radius::MD))
                                        .cursor_pointer()
                                        .ty(crate::ds::UI)
                                        .text_color(t.ink)
                                        .hover(|d| d.bg(t.hover()))
                                        .active(|d| d.bg(t.press()))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.run_context(what.clone(), window, cx)
                                        }))
                                        .child(label)
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}
