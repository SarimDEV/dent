//! Right-click menus: a short list of actions at the pointer. One is open
//! at a time; a click anywhere else or Esc closes it.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::Press;
use crate::ui::Tipped;
use crate::ui::{Arrive, From};

use crate::app::Dent;
use crate::ds::{Typed, motion, radius, size, space};
use crate::theme::Theme;

#[derive(Clone)]
pub struct ContextMenu {
    pub at: Point<Pixels>,
    pub items: Vec<(SharedString, Do)>,
}

/// What a row does.
#[derive(Clone)]
pub enum Do {
    /// A hairline between groups of actions.
    Separator,
    CloseTab(&'static str),
    CloseOtherTabs(&'static str),
    CopyPath(String),
    Reveal(String),
    Rename(String),
    Fork(String),
    CloseSession(String),
    Reopen(String),
    /// Pin to the top of the sidebar, or unpin.
    Pin(String),
    /// Open a file with this app from now on (`None`: the Mac's default).
    OpenWith(String, Option<String>),
    /// Put text on the clipboard: a link, a command, what's selected.
    Copy(String),
    /// Follow a link as a click would.
    OpenLink(String),
    /// Open a file in its app (your pick for its kind, else the Mac's).
    OpenInApp(String),
    /// Type a command into your terminal, for you to run.
    RunInTerminal(String),
}

const WIDTH: f32 = 200.;

impl Dent {
    pub fn open_context(
        &mut self,
        at: Point<Pixels>,
        items: Vec<(impl Into<SharedString>, Do)>,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        let items = items.into_iter().map(|(l, d)| (l.into(), d)).collect();
        self.context = Some(ContextMenu { at, items });
        cx.notify();
    }

    pub fn run_context(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        self.context = None;
        match what {
            Do::Separator => {}
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
            Do::Pin(id) => self.toggle_pin(&id, cx),
            Do::OpenWith(path, app) => {
                crate::open::open_with(&path, app, cx);
                self.save_prefs();
            }
            Do::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Do::OpenLink(url) => crate::select::open_link(&url, window, cx),
            Do::OpenInApp(path) => crate::open::open(&path, cx),
            Do::RunInTerminal(command) => {
                self.open_shell(window, cx);
                if let Some(sh) = self.current().and_then(|s| self.shells.get(&s.id)) {
                    sh.paste(&command);
                }
            }
        }
        cx.notify();
    }

    /// Right-click in the chat: what's under the pointer decides. A link
    /// offers to open or copy it (a file: also Finder and its app); a
    /// selection offers Copy. Elsewhere, nothing.
    pub fn chat_context(&mut self, at: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let mut items: Vec<(&str, Do)> = Vec::new();
        if let Some(url) = crate::select::link_at(at, window) {
            if url.starts_with('/') {
                let (path, _) = crate::select::at_line(&url);
                items.push(("Open", Do::OpenLink(url.clone())));
                items.push(("Open in App", Do::OpenInApp(path.clone())));
                items.push(("Show in Finder", Do::Reveal(path.clone())));
                items.push(("Copy Path", Do::CopyPath(path)));
            } else {
                items.push(("Open Link", Do::OpenLink(url.clone())));
                items.push(("Copy Link", Do::Copy(url)));
            }
        }
        if crate::select::has_selection() {
            if !items.is_empty() {
                items.push(("", Do::Separator));
            }
            items.push(("Copy", Do::Copy(crate::select::copied())));
        }
        if !items.is_empty() {
            self.open_context(at, items, cx);
        }
    }

    /// Right-click on a tool row: a command copies or goes to your
    /// terminal; a file opens, shows in Finder or copies its path.
    pub fn tool_context(
        &mut self,
        at: Point<Pixels>,
        line: &crate::data::ToolLine,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = line.detail.as_ref() else {
            return;
        };
        let mut items: Vec<(&str, Do)> = Vec::new();
        if line.text.starts_with("$ ") {
            items.push(("Copy Command", Do::Copy(detail.input.clone())));
            items.push((
                "Run in My Terminal",
                Do::RunInTerminal(detail.input.clone()),
            ));
        }
        if let Some(path) = &detail.path {
            if std::path::Path::new(path).exists() {
                items.push(("Open", Do::OpenLink(path.clone())));
                items.push(("Show in Finder", Do::Reveal(path.clone())));
            }
            items.push(("Copy Path", Do::CopyPath(path.clone())));
        }
        if !items.is_empty() {
            self.open_context(at, items, cx);
        }
    }

    /// The open menu, over everything, kept inside the window.
    pub fn context_view(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.context.clone()?;
        let t: Theme = self.t;
        let vw = window.viewport_size();
        let h = menu
            .items
            .iter()
            .map(|(_, w)| {
                if matches!(w, Do::Separator) {
                    space::S2 + 1.
                } else {
                    size::ROW + 4.
                }
            })
            .sum::<f32>()
            + space::S2;
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
                // Takes the press that dismisses it: nothing under it is clicked.
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                    this.context = None;
                    cx.notify();
                }))
                // Placed by an outer box, so the menu itself can arrive.
                .child(
                    div().absolute().left(px(x)).top(px(y)).child(
                        div()
                            .id("context-menu")
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
                            .children(menu.items.into_iter().enumerate().map(
                                |(i, (label, what))| {
                                    if matches!(what, Do::Separator) {
                                        return div()
                                            .id(("context-sep", i))
                                            .my(px(space::S1))
                                            .mx(px(space::S2))
                                            .h(px(1.))
                                            .bg(t.hairline);
                                    }
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
                                        .on_press(cx.listener(move |this, _, window, cx| {
                                            this.run_context(what.clone(), window, cx)
                                        }))
                                        .child(label)
                                },
                            ))
                            .arrive(
                                ElementId::Name(format!("context-{x}-{y}").into()),
                                From::Above,
                                motion::FAST,
                            ),
                    ),
                )
                .into_any_element(),
        )
    }
}

impl Dent {
    /// A file's Open, and beside it Open with: the apps that can open it,
    /// your pick for its kind ticked; picking one opens it there from now
    /// on.
    pub fn open_button(
        &self,
        id: &'static str,
        full: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.t;
        let path = full.display().to_string();
        let opens_in = crate::open::app_for(&path).map(|a| crate::open::app_name(&a));
        let p1 = path.clone();
        div()
            .flex()
            .items_center()
            .child(
                crate::ui::ghost(id, "Open", &t)
                    .relative()
                    .child(crate::ui::probe(&self.probes, id))
                    .on_press(move |_, _, cx| crate::open::open(&p1, cx))
                    .with_tip(crate::ui::tip_owned(
                        match &opens_in {
                            Some(app) => format!("Open in {app}"),
                            None => "Open in its app".to_string(),
                        },
                        t,
                    )),
            )
            .child(
                crate::ui::icon_button(
                    SharedString::from(format!("{id}-with")),
                    "chevron-down",
                    &t,
                )
                .size(px(crate::ds::size::CONTROL_SM))
                .relative()
                .child(crate::ui::probe(&self.probes, format!("{id}-with")))
                .with_tip(crate::ui::tip("Open with…", None, t))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let apps = crate::open::apps(&path);
                        let chosen = crate::open::app_for(&path);
                        let mut items: Vec<(String, Do)> = apps
                            .iter()
                            .enumerate()
                            .map(|(i, a)| {
                                let name = crate::open::app_name(a);
                                let label = match (chosen.as_deref() == Some(a.as_str()), i) {
                                    (true, _) => format!("✓ {name}"),
                                    (false, 0) => format!("{name} (default)"),
                                    _ => name,
                                };
                                (label, Do::OpenWith(path.clone(), Some(a.clone())))
                            })
                            .collect();
                        if apps.is_empty() {
                            items.push(("No app can open this".to_string(), Do::Separator));
                        }
                        if chosen.is_some() {
                            items.push((String::new(), Do::Separator));
                            items.push((
                                "Use the Mac's default".to_string(),
                                Do::OpenWith(path.clone(), None),
                            ));
                        }
                        this.open_context(e.position, items, cx);
                    }),
                ),
            )
    }
}
