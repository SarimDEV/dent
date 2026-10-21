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
    /// Ask for a new title from what the session is about now.
    Retitle(String),
    /// Start the session's agent again, to load new skills and settings.
    Restart(String),
    Fork(String),
    CloseSession(String),
    Reopen(String),
    /// Pin to the top of the sidebar, or unpin.
    Pin(String),
    /// The sessions picked with Shift or ⌘: close them, or pin or unpin
    /// them, together.
    ClosePicked,
    PinPicked,
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
    /// Put words into the reply box as a quote, under what's there.
    Quote(String),
    /// Keep a message as a skill: in this project's folder, or everywhere.
    SaveSkill(String, Option<String>),
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
            Do::Restart(id) => {
                if self.can(&id, "can't start again on the same conversation", |c| {
                    c.restart
                }) {
                    self.live
                        .send(dent_protocol::Request::RestartAgent { session: id });
                }
            }
            Do::Retitle(id) => {
                self.live
                    .send(dent_protocol::Request::Retitle { session: id });
                self.toast(crate::ToastAction::Info, "Finding a new title…".into());
                cx.notify();
            }
            Do::Fork(id) => self.fork(&id, None, cx),
            Do::CloseSession(id) => {
                self.close(&id, window, cx);
            }
            Do::Reopen(id) => self.reopen(&id, cx),
            Do::Pin(id) => self.toggle_pin(&id, cx),
            Do::ClosePicked => self.close_picked(window, cx),
            Do::PinPicked => {
                // All pinned already: unpin them; otherwise pin the rest.
                let picked = std::mem::take(&mut self.picked);
                let all = picked.iter().all(|id| self.pinned.contains(id));
                for id in &picked {
                    if all == self.pinned.contains(id) {
                        self.toggle_pin(id, cx);
                    }
                }
            }
            Do::OpenWith(path, app) => {
                crate::open::open_with(&path, app, cx);
                self.prefs_changed();
            }
            Do::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Do::OpenLink(url) => crate::select::open_link(&url, window, cx),
            Do::OpenInApp(path) => crate::open::open(&path, cx),
            Do::Quote(words) => self.quote(&words, window, cx),
            Do::SaveSkill(words, project) => self.save_skill(&words, project.as_deref(), cx),
            Do::RunInTerminal(command) => {
                self.open_shell(window, cx);
                if let Some(sh) = self.current().and_then(|s| self.shells.get(&s.id)) {
                    sh.paste(&command);
                }
            }
        }
        cx.notify();
    }

    /// Words from the chat into your reply as a Markdown quote, after
    /// anything you've written, with the caret on a fresh line below it.
    pub fn quote(&mut self, words: &str, window: &mut Window, cx: &mut Context<Self>) {
        let quoted = quote_block(words);
        if quoted.is_empty() {
            return;
        }
        // Only after the draft: its folded pastes stay chips.
        let target = self.selected.clone();
        self.put_draft_in(
            target,
            crate::app::Place::After,
            crate::app::Draft::words(format!("{quoted}\n\n")),
            window,
            cx,
        );
        crate::select::clear(window);
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
            let words = crate::select::copied();
            items.push(("Copy", Do::Copy(words.clone())));
            if self.current().is_some() {
                items.push(("Quote in Reply", Do::Quote(words)));
            }
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
        // The file is the session's machine's: asked of it, and only this
        // Mac's can be shown in Finder.
        let Some(s) = self.current().cloned() else {
            return;
        };
        let far = !dent_client_core::fleet::split(&s.id).0.is_empty();
        let items = tool_items(
            detail,
            |p| {
                matches!(
                    crate::files::stat(&crate::files::Ask::new(&s.id, p, &s.cwd)),
                    crate::files::Look::Loading
                        | crate::files::Look::Here(dent_protocol::FileStat::File { .. })
                        | crate::files::Look::Stale(dent_protocol::FileStat::File { .. })
                        | crate::files::Look::Here(dent_protocol::FileStat::Dir { .. })
                )
            },
            !far,
        );
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
    /// Open, for another machine's file: copied here first, then opened in
    /// its app.
    pub fn open_far_button(
        &self,
        id: &'static str,
        ask: crate::files::Ask,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        crate::ui::ghost(id, "Open", &t)
            .relative()
            .child(crate::ui::probe(&self.probes, id))
            .on_press(cx.listener(move |this, _, _, cx| this.open_outside_far(ask.clone(), cx)))
            .with_tip(crate::ui::tip_owned(
                "Copy it here and open it in its app".to_string(),
                t,
            ))
            .into_any_element()
    }

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

/// `words` as a Markdown quote: each line behind "> ", paragraphs apart as
/// they were.
pub fn quote_block(words: &str) -> String {
    words
        .trim()
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool row's menu: a command copies or goes to your terminal; a file
/// opens, shows in Finder or copies its path (`exists` says whether it's
/// still there); what came back copies whole.
fn tool_items(
    detail: &crate::data::CallDetail,
    exists: impl Fn(&str) -> bool,
    here: bool,
) -> Vec<(&'static str, Do)> {
    let mut items: Vec<(&str, Do)> = Vec::new();
    if let Some(command) = &detail.command {
        items.push(("Copy Command", Do::Copy(command.clone())));
        items.push(("Run in My Terminal", Do::RunInTerminal(command.clone())));
    }
    if let Some(path) = &detail.path {
        if exists(path) {
            items.push(("Open", Do::OpenLink(path.clone())));
            if here {
                items.push(("Show in Finder", Do::Reveal(path.clone())));
            }
        }
        items.push(("Copy Path", Do::CopyPath(path.clone())));
    }
    // Everything kept, not the folded view: plain, without colour codes.
    if let Some((_, out)) = detail.output.as_ref().filter(|(_, o)| !o.trim().is_empty()) {
        items.push(("Copy Output", Do::Copy(crate::ansi::parse(out).0)));
    }
    items
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_keep_their_lines() {
        assert_eq!(super::quote_block(" a\n\nb \n"), "> a\n\n> b");
        assert_eq!(super::quote_block("  "), "");
    }

    #[test]
    fn a_command_row_copies_and_runs_the_bare_command() {
        use super::Do;
        let detail = crate::data::CallDetail {
            input: "$ cargo test".into(),
            command: Some("cargo test".into()),
            output: Some((true, "\x1b[31merror\x1b[0m: failed\n".into())),
            path: None,
            edits: false,
        };
        let items = super::tool_items(&detail, |_| true, true);
        let labels: Vec<_> = items.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            labels,
            ["Copy Command", "Run in My Terminal", "Copy Output"]
        );
        assert!(matches!(&items[0].1, Do::Copy(c) if c == "cargo test"));
        assert!(matches!(&items[1].1, Do::RunInTerminal(c) if c == "cargo test"));
        assert!(matches!(&items[2].1, Do::Copy(c) if c == "error: failed\n"));
        // A file row: no command.
        let file = crate::data::CallDetail {
            input: String::new(),
            command: None,
            output: None,
            path: Some("/p/a.rs".into()),
            edits: true,
        };
        let labels: Vec<_> = super::tool_items(&file, |_| false, true)
            .iter()
            .map(|(l, _)| *l)
            .collect();
        assert_eq!(labels, ["Copy Path"]);
    }
}
