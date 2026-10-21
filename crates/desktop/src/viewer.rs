//! The viewer: everything a session made or changed, beside the chat. Pages
//! run live, docs render, images show, changes take review comments.

use crate::ui::Tipped;
use std::rc::Rc;

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::Press;

use crate::ui::RevealOnHover;

use crate::app::{Dent, Tab, Viewport};
use crate::data::{FileDiff, Kind, Line, Session};
use crate::ds::{CODE, LABEL, READING, Typed, UI, radius, size, space};
use crate::markdown;
use crate::theme::{Status, status_square};
use crate::ui::{Ready, action, button, ghost, icon, icon_button, kbd, probe, tip};

/// Where a file the agent made is on disk: paths are kept as it wrote them,
/// so `~/…` and relative ones are resolved here.
pub fn art_file(a: &crate::data::Artifact) -> Option<std::path::PathBuf> {
    if a.kind == Kind::Server {
        return None;
    }
    let p = a.path;
    if let Some(rest) = p.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(rest));
    }
    if p.starts_with('/') {
        return Some(p.into());
    }
    a.dir.as_ref().map(|d| d.join(p))
}

/// The session an artifact is from (qualified with its machine): made and
/// media ones are `session:path`, opened ones `session/open/path`.
pub fn art_session(id: &str) -> &str {
    if let Some(i) = id.find("/open/") {
        return &id[..i];
    }
    id.split_once(':').map_or(id, |(s, _)| s)
}

/// Whether an artifact is another machine's file, not opened from a copy
/// here: never looked for on this Mac's disk.
pub fn art_far(a: &crate::data::Artifact) -> bool {
    !a.id.contains("/open/")
        && !dent_client_core::fleet::split(art_session(a.id))
            .0
            .is_empty()
}

/// Where another machine's artifact finds its own files there: its
/// session, and the folder it's in. `None` for this Mac's.
pub fn far_root(a: &crate::data::Artifact) -> Option<crate::web::FarRoot> {
    if !art_far(a) {
        return None;
    }
    let cwd = a
        .dir
        .as_ref()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let full = if a.path.starts_with('/') || a.path.starts_with("~/") {
        a.path.to_string()
    } else {
        format!("{cwd}/{}", a.path)
    };
    let dir = full
        .rsplit_once('/')
        .map_or(cwd.clone(), |(d, _)| d.to_string());
    Some(crate::web::FarRoot {
        session: art_session(a.id).to_string(),
        cwd,
        dir,
    })
}

/// What another machine's artifact is, to ask of the file layer; `None`
/// for this Mac's.
pub fn far_ask(a: &crate::data::Artifact) -> Option<crate::files::Ask> {
    if !art_far(a) {
        return None;
    }
    let cwd = a
        .dir
        .as_ref()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    Some(crate::files::Ask::new(art_session(a.id), a.path, &cwd))
}

/// A long name cut in the middle, so its start and its extension both show.
pub fn middle_ellipsis(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep = max - 1;
    let head: String = s.chars().take(keep - keep / 2).collect();
    let tail: String = s.chars().skip(n - keep / 2).collect();
    format!("{head}…{tail}")
}

/// A file's path as the viewer shows it: relative to the session's folder
/// when it's inside, from `~` when it's in home.
pub fn shown_path(a: &crate::data::Artifact, cwd: &str) -> String {
    if a.path.starts_with('/') {
        crate::bridge::relative(std::path::Path::new(cwd), a.path)
    } else {
        a.path.to_string()
    }
}

/// A picture's size in pixels, read from the file itself whatever its
/// format; remembered per version, as an SVG has to be parsed for it.
type Sizes = std::collections::HashMap<(&'static str, usize), (u32, u32)>;

fn picture_size(id: &'static str, v: usize, bytes: &[u8]) -> (u32, u32) {
    thread_local! {
        static SIZES: std::cell::RefCell<Sizes> =
            std::cell::RefCell::default();
    }
    if let Some(s) = SIZES.with(|m| m.borrow().get(&(id, v)).copied()) {
        return s;
    }
    let raster = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok());
    let size = raster
        .or_else(|| {
            resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default())
                .ok()
                .map(|t| {
                    (
                        t.size().width().round() as u32,
                        t.size().height().round() as u32,
                    )
                })
        })
        .unwrap_or((0, 0));
    SIZES.with(|m| m.borrow_mut().insert((id, v), size));
    size
}

/// Half the width of the grip on the panel's left edge.
const GRIP: f32 = 4.;

pub const MIN_WIDTH: f32 = 360.;

/// A row of the diff list.
#[derive(Clone, Copy)]
pub enum Row {
    Line(usize),
    Note(usize),
    Compose,
}
/// The chat never gets narrower than this; the viewer and then the sidebar give way.
pub const CHAT_MIN: f32 = 440.;

impl Dent {
    /// Whether the viewer has anything for this session: what it made,
    /// what it changed, or a file you opened.
    pub fn has_viewer(&self, s: &Session) -> bool {
        !s.arts.is_empty()
            || !s.diff.is_empty()
            || self.opened.get(&s.id).is_some_and(|o| !o.is_empty())
    }

    /// Whether the right pane has anything for this session: the viewer,
    /// or a side chat, even one just opened.
    pub fn shows_panel(&self, s: &Session) -> bool {
        self.has_viewer(s)
            || !s.asides.is_empty()
            || matches!(self.tabs.get(&s.id), Some(Tab::Aside | Tab::Terminal))
    }

    /// Whether the live page should be on screen this frame.
    pub fn preview_visible(&self) -> bool {
        let Some(s) = self.current() else {
            return false;
        };
        // A page in its own window never hides for menus: nothing covers it.
        let detached = self.preview.as_ref().is_some_and(|p| p.borrow().detached());
        self.panel_open
            && self.has_viewer(s)
            && matches!(self.tab(s), Tab::Art(a) if self.art(a).is_some_and(|a| a.kind.is_page()) && !self.popouts.contains_key(a) && !self.servers_down.contains(a))
            && (detached || (self.menu.is_none() && self.palette.is_none() && !self.dragging))
    }

    pub fn panel(
        &mut self,
        s: &Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let tab = self.tab(s);
        // Changes, Terminal and Side chat stay put on the left; the files
        // the agent made follow, newest last.
        let mut tabs: Vec<(Tab, String, &'static str, Option<&'static str>)> = Vec::new();
        if !s.diff.is_empty() {
            tabs.push((
                Tab::Changes,
                format!("Changes {}", s.diff.len()),
                "git-compare",
                None,
            ));
        }
        if self.shells.contains_key(&s.id) || tab == Tab::Terminal {
            tabs.push((Tab::Terminal, "Terminal".into(), "square-terminal", None));
        }
        if !s.asides.is_empty() || tab == Tab::Aside {
            tabs.push((
                Tab::Aside,
                "Side chat".into(),
                "message-circle-question-mark",
                None,
            ));
        }
        let arts: Vec<_> = self
            .open_arts(s)
            .into_iter()
            .filter_map(|a| self.art(a))
            .collect();
        // Files are named by their own name, with as much of their folder
        // as tells two of the same name apart.
        let files: Vec<&str> = arts
            .iter()
            .filter(|a| a.kind != Kind::Server && crate::data::visual_title(a.path).is_none())
            .map(|a| a.path)
            .collect();
        let mut names = tab_names(&files).into_iter();
        for a in arts {
            tabs.push((
                Tab::Art(a.id),
                if a.kind == Kind::Server {
                    a.title.trim_start_matches("Dev server · ").to_string()
                } else if let Some(v) = crate::data::visual_title(a.path) {
                    v.to_string()
                } else {
                    names.next().unwrap_or_default()
                },
                a.kind.icon(),
                Some(crate::data::intern(&self.shown(a))),
            ));
        }
        // Keep the tab you're looking at in view as tabs come and go.
        let key = (tab, tabs.len(), self.panel_w as u32);
        if self.revealed_tab != Some(key) && !self.dragging {
            self.revealed_tab = Some(key);
            self.reveal_pending = true;
        }
        if self.reveal_pending {
            // Once the strip has been laid out it knows where each tab is
            // (before scrolling): scroll just enough to show this one whole.
            let ix = tabs.iter().position(|(w, ..)| *w == tab);
            let view = self.tab_scroll.bounds();
            match ix.and_then(|ix| self.tab_scroll.bounds_for_item(ix)) {
                Some(b) if view.size.width > px(0.) => {
                    let mut x = self.tab_scroll.offset().x;
                    if b.right() + x > view.right() {
                        x = view.right() - b.right();
                    }
                    if b.left() + x < view.left() {
                        x = view.left() - b.left();
                    }
                    let x = x.min(px(0.)).max(-self.tab_scroll.max_offset().x);
                    let shown = b.right() + x <= view.right() + px(0.5)
                        && b.left() + x >= view.left() - px(0.5);
                    self.tab_scroll.set_offset(point(x, px(0.)));
                    // The strip's own measure lags a frame behind new tabs:
                    // look again until the tab fits, a few frames at most.
                    self.reveal_tries += 1;
                    // Settled means it fit two frames running: a new tab's
                    // own width lands a frame after it does.
                    if (shown && self.reveal_tries >= 2) || self.reveal_tries > 8 {
                        self.reveal_pending = false;
                        self.reveal_tries = 0;
                    }
                    window.request_animation_frame();
                }
                _ => window.request_animation_frame(),
            }
        }
        let many = tabs.iter().filter(|(_, _, _, p)| p.is_some()).count() > 1;
        // Names give up letters (from the middle) to share the strip before
        // it has to scroll: 28 at most, 10 at least.
        let room = (self.panel_w - TAB_STRIP_CHROME) / tabs.len().max(1) as f32;
        let fit = ((room - TAB_CHROME) / TAB_LETTER).floor().max(0.) as usize;
        let letters = fit.clamp(10, 28);
        // Too narrow even for that: files not showing are their icon alone
        // (the tooltip has the path), so no tab is cut off at an edge.
        let bare = fit < 10;
        let header = div()
            .h(px(size::HEADER))
            .flex_none()
            .pl(px(space::S2))
            // The toggle at the end lines up with the chat header's.
            .pr(px(space::S4))
            .flex()
            .items_center()
            .gap(px(space::S1))
            .border_b_1()
            .border_color(t.hairline)
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child({
                // An edge with more tabs past it fades, so a cut-off tab reads
                // as "there's more", not as a broken one.
                let off = self.tab_scroll.offset().x.as_f32();
                let max = self.tab_scroll.max_offset().x.as_f32();
                let fade = |left: bool| {
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w(px(space::S8))
                        .map(|d| if left { d.left_0() } else { d.right_0() })
                        .bg(linear_gradient(
                            if left { 90. } else { 270. },
                            linear_color_stop(t.panel, 0.),
                            linear_color_stop(t.panel.opacity(0.), 1.),
                        ))
                };
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id(SharedString::from(format!("tabs-{}", s.id)))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(space::S1))
                            // swipe: a mouse wheel scrolls the tabs sideways,
                            // as there's nothing else here for it to move.
                            .overflow_x_scroll()
                            .track_scroll(&self.tab_scroll)
                            .children(tabs.into_iter().map(|(which, name, ic, path)| {
                                let on = which == tab;
                                let key = match which {
                                    Tab::Art(a) => format!("tab-{a}"),
                                    Tab::Changes => "tab-changes".into(),
                                    Tab::Terminal => "tab-terminal".into(),
                                    Tab::Aside => "tab-aside".into(),
                                };
                                let group = SharedString::from(format!("g-{key}"));
                                let art = match which {
                                    Tab::Art(a) => Some(a),
                                    _ => None,
                                };
                                div()
                                    .id(SharedString::from(key.clone()))
                                    .group(group.clone())
                                    .relative()
                                    .flex_none()
                                    .h(px(size::CONTROL))
                                    .pl(px(space::S2))
                                    .pr(px(if art.is_some() { space::S1 } else { space::S2 }))
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S1H))
                                    .rounded(px(radius::MD))
                                    .cursor_pointer()
                                    .when(on, |d| d.bg(t.selected()))
                                    .when(!on, |d| d.hover(|d| d.bg(t.hover())))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_press(
                                        cx.listener(move |this, _, _, cx| this.show_tab(which, cx)),
                                    )
                                    .when_some(path, |d, p| d.with_tip(crate::ui::tip_owned(p, t)))
                                    // A middle click closes a file's tab, as in a browser.
                                    .when_some(art, |d, a| {
                                        d.on_mouse_down(
                                            MouseButton::Middle,
                                            cx.listener(move |this, _, _, cx| {
                                                this.close_tab(a, cx)
                                            }),
                                        )
                                    })
                                    .child(icon(ic, 13., if on { t.ink } else { t.quiet }))
                                    .when(!(bare && !on && art.is_some()), |d| {
                                        d.child(
                                            div()
                                                .ty(UI)
                                                .text_color(if on { t.ink } else { t.quiet })
                                                .child(middle_ellipsis(&name, letters)),
                                        )
                                    })
                                    .when_some(art, |d, a| {
                                        d.child(
                                            div()
                                                .id(SharedString::from(format!("close-{key}")))
                                                .flex_none()
                                                .size(px(18.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(radius::SM))
                                                .when(!on, |d| d.reveal_on_hover(group.clone()))
                                                .hover(|d| d.bg(t.hover()))
                                                .active(|d| d.bg(t.press()))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .on_press(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.close_tab(a, cx);
                                                }))
                                                .with_tip(tip("Close tab", None, t))
                                                .child(icon("x", 12., t.quiet)),
                                        )
                                    })
                                    .when_some(art.zip(path), |d, (a, p)| {
                                        let _ = p;
                                        let full = self
                                            .art(a)
                                            .and_then(art_file)
                                            .map(|f| f.display().to_string());
                                        d.on_mouse_down(
                                            MouseButton::Right,
                                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                                use crate::context::Do;
                                                let mut items =
                                                    vec![("Close tab", Do::CloseTab(a))];
                                                if many {
                                                    items.push((
                                                        "Close other tabs",
                                                        Do::CloseOtherTabs(a),
                                                    ));
                                                }
                                                if let Some(f) = &full {
                                                    items.push(("", Do::Separator));
                                                    items.push((
                                                        "Copy path",
                                                        Do::CopyPath(f.clone()),
                                                    ));
                                                    items.push((
                                                        "Show in Finder",
                                                        Do::Reveal(f.clone()),
                                                    ));
                                                }
                                                this.open_context(e.position, items, cx);
                                            }),
                                        )
                                    })
                                    .child(probe(&self.probes, key))
                            })),
                    )
                    .when(off < -1., |d| d.child(fade(true)))
                    .when(max > 1. && off > -max + 1., |d| d.child(fade(false)))
            })
            .child(self.panel_toggle(cx));
        let body = match tab {
            Tab::Art(id) => match self.art(id).map(|a| a.kind) {
                Some(Kind::Html | Kind::Server | Kind::Media) => self.page(id, window, cx),
                Some(Kind::Doc) => self.doc(id, window, cx),
                Some(Kind::Image) => self.image(id, window, cx),
                Some(Kind::Text) => self.text(id, window, cx),
                None => div().into_any_element(),
            },
            Tab::Changes => self.changes(s, window, cx),
            Tab::Terminal => self.shell_view(s, window, cx),
            Tab::Aside => self.side_chat(s, window, cx),
        };
        // ⌘F over what it searches here, and the match it went to in view.
        self.reveal_found(s, window, cx);
        let find = self
            .viewer_scope(s)
            .filter(|here| !matches!(here, crate::find::Scope::Page(_)))
            .and_then(|here| self.find_bar(here, cx));
        let body = div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(body)
            .children(find);
        // The panel clips its contents; its grip sits outside that, half
        // over the chat, so it is easy to catch.
        div()
            .relative()
            .w(px(self.panel_w))
            .flex_none()
            .h_full()
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .bg(t.panel)
                    .border_l_1()
                    .border_color(t.hairline)
                    .child(header)
                    .child(body),
            )
            // The left edge resizes the panel. A page stays clear of the
            // grip's inner half: a native web view would take the press.
            .child(
                div()
                    .id("resize")
                    .absolute()
                    .left(px(-GRIP))
                    .top_0()
                    .bottom_0()
                    .w(px(GRIP * 2.))
                    .cursor_col_resize()
                    .hover(|d| d.bg(t.hairline))
                    .when(self.dragging, |d| d.bg(t.hairline))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.dragging = true;
                            cx.notify();
                        }),
                    )
                    .child(probe(&self.probes, "resize")),
            )
            .into_any_element()
    }

    /// An artifact's path as shown in the viewer, for the open session.
    fn shown(&self, a: &crate::data::Artifact) -> String {
        shown_path(a, self.current().map_or("", |s| s.cwd.as_str()))
    }

    /// Earlier and later versions of a file the agent wrote more than once:
    /// "‹ v2 of 3 ›". One version needs no stepper.
    fn versions(&self, a: &crate::data::Artifact, cx: &mut Context<Self>) -> Option<Div> {
        let t = self.t;
        let latest = a.versions.len();
        if latest < 2 {
            return None;
        }
        let v = self.version(a);
        let id = a.id;
        // Only versions it has the text of: a file that was only ever edited
        // has its latest whole, not what came before.
        let has = |k: usize| a.versions.get(k - 1).is_some_and(|b| !b.is_empty());
        let prev = (1..v).rev().find(|k| has(*k));
        let next = (v + 1..=latest).find(|k| has(*k));
        if prev.is_none() && next.is_none() {
            return None;
        }
        let step = |label: &'static str,
                    name: &'static str,
                    to: Option<usize>,
                    says: &'static str,
                    cx: &mut Context<Self>| {
            let enabled = to.is_some();
            div()
                .id(label)
                .relative()
                .size(px(size::CONTROL))
                .on_hover(Self::hint(says, cx))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::SM))
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(|d| d.bg(t.hover()))
                        .active(|d| d.bg(t.press()))
                })
                .when(!enabled, |d| d.cursor_not_allowed())
                .when_some(to, |d, to| {
                    d.on_press(cx.listener(move |this, _, _, cx| {
                        this.open_art(id, Some(to), cx);
                    }))
                })
                .child(icon(
                    name,
                    14.,
                    if enabled {
                        t.quiet
                    } else {
                        t.faint.opacity(0.5)
                    },
                ))
                .child(probe(&self.probes, label))
        };
        Some(
            div()
                .flex()
                .items_center()
                .child(step(
                    "prev-version",
                    "chevron-left",
                    prev,
                    "Earlier version",
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .ty(LABEL)
                        .text_color(t.ink)
                        .child(format!("v{v}"))
                        .child(div().text_color(t.faint).child(format!(" of {latest}"))),
                )
                .child(step(
                    "next-version",
                    "chevron-right",
                    next,
                    "Later version",
                    cx,
                )),
        )
    }

    /// Hovering a preview control says what it does, in the toolbar itself.
    fn hint(
        text: &'static str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        cx.listener(move |this: &mut Self, on: &bool, _, cx| {
            this.toolbar_hint = if *on {
                Some(text)
            } else {
                this.toolbar_hint.filter(|h| *h != text)
            };
            cx.notify();
        })
    }

    fn toolbar(&self) -> Div {
        div()
            .h(px(size::TOOLBAR))
            .flex_none()
            .px(px(space::S2))
            .flex()
            .items_center()
            .gap(px(space::S1))
            .border_b_1()
            .border_color(self.t.hairline)
    }

    fn page(
        &mut self,
        id: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let Some(a) = self.art(id).cloned() else {
            return div().into_any_element();
        };
        let server = a.kind == Kind::Server;
        // A video, a sound or a PDF: no versions, widths or pointing.
        let media = a.kind == Kind::Media;
        if self.popouts.contains_key(id) {
            return self.popped_out(id, &a, cx);
        }
        let latest = a.versions.len().max(1);
        let v = self.version(&a);
        let detached = self.preview.as_ref().is_some_and(|p| p.borrow().detached());
        if server {
            self.watch_server(id, a.path, cx);
        }
        let down = server && self.servers_down.contains(id);
        if let Some(p) = &self.preview {
            let mut p = p.borrow_mut();
            if server {
                p.show_server(a.path);
            } else if let Some(far) = far_root(&a) {
                // Another machine's: its video plays from there a range at a
                // time, and a page's own pictures and styles come from there.
                if media {
                    p.show_far_file(a.path.rsplit('/').next().unwrap_or(a.path), far);
                } else {
                    p.show_far(a.path, v, &a.versions[v - 1], far);
                }
            } else if media {
                if let Some(f) = art_file(&a) {
                    p.show_file(&f);
                }
            } else {
                p.show(a.path, v, &a.versions[v - 1], a.dir.as_deref());
                // Its picture, for its card in the chat.
                if let Some((src, bytes, format)) = p.take_still()
                    && src
                        == (crate::web::Source::File {
                            name: a.path.to_string(),
                            version: v,
                        })
                {
                    self.stills.insert(
                        id,
                        (v, std::sync::Arc::new(Image::from_bytes(format, bytes))),
                    );
                }
            }
        }
        let errors = self.errors.get(&(id, v)).cloned().unwrap_or_default();
        let updated = self
            .updated_at
            .get(id)
            .map(|at| at.elapsed().as_secs_f32())
            .filter(|s| *s < 2.5);
        let viewport = self.viewport;
        let dragging = self.dragging || self.dragging_sidebar;
        // A width wider than the panel can't be shown: its button is off,
        // says why, and Full width is what's selected meanwhile.
        let panel_w = self.panel_w;
        let fits = move |vp: Viewport| vp.width().is_none_or(|w| panel_w > w + space::S8);
        let shown_vp = if fits(viewport) {
            viewport
        } else {
            Viewport::Fill
        };
        let seg = |vp: Viewport,
                   name: &'static str,
                   label: &'static str,
                   says: &'static str,
                   cx: &mut Context<Self>| {
            let ok = fits(vp);
            icon_button(format!("vp-{label}"), name, &t)
                .relative()
                .on_hover(Self::hint(
                    if ok { says } else { "Needs a wider panel" },
                    cx,
                ))
                .when(shown_vp == vp, |d| d.bg(t.selected()))
                .map(|d| {
                    if ok {
                        d
                    } else {
                        crate::ui::off(d, "Needs a wider panel", &t)
                    }
                })
                .when(ok, |d| {
                    d.on_press(cx.listener(move |this, _, _, cx| {
                        this.viewport = vp;
                        cx.notify();
                    }))
                })
                .child(probe(&self.probes, format!("vp-{label}")))
        };
        let machine = self
            .current()
            .map(|s| crate::data::machine_shown(&s.machine))
            .unwrap_or_default();
        let toolbar = self
            .toolbar()
            .when(server, |d| {
                d.pl(px(space::S3)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S1H))
                        .child(status_square(
                            if down { Status::Idle } else { Status::Done },
                            &t,
                        ))
                        .child(div().ty(LABEL).text_color(t.ink).child(if down {
                            "stopped"
                        } else {
                            "running"
                        })),
                )
            })
            .when(!server && !media, |d| {
                d.children(self.versions(&a, cx))
                    .child(div().w(px(1.)).h(px(16.)).mx(px(space::S1)).bg(t.hairline))
            })
            .when(server, |d| {
                d.child(div().w(px(1.)).h(px(16.)).mx(px(space::S1)).bg(t.hairline))
            })
            .child(match self.toolbar_hint {
                Some(h) => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .ty(UI)
                    .text_color(t.ink)
                    .child(h),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .ty(LABEL)
                    .text_color(t.quiet)
                    .child(if server {
                        format!(
                            "{} · on {machine}",
                            a.title.trim_start_matches("Dev server · ")
                        )
                    } else {
                        self.shown(&a)
                    }),
            })
            .when_some(updated, |d, secs| {
                let k = if secs < 2. {
                    1.
                } else {
                    1. - (secs - 2.) / 0.5
                };
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S1))
                        .opacity(k)
                        .ty(LABEL)
                        .text_color(t.quiet)
                        .child(status_square(Status::Done, &t))
                        .child(format!("updated to v{latest}")),
                )
            })
            // Widths frame the page inside the viewer; a page in its own
            // window is sized by its window.
            .when(!detached && !media && self.panel_w >= 460., |d| {
                d.child(seg(Viewport::Fill, "monitor", "fill", "Full width", cx))
                    .child(seg(
                        Viewport::Tablet,
                        "tablet",
                        "tablet",
                        "Tablet width, 768",
                        cx,
                    ))
                    .child(seg(
                        Viewport::Phone,
                        "smartphone",
                        "phone",
                        "Phone width, 390",
                        cx,
                    ))
                    .child(div().w(px(1.)).h(px(16.)).mx(px(space::S1)).bg(t.hairline))
            })
            .when(!media, |d| {
                d.child(
                    div()
                        .id("pick")
                        .relative()
                        .h(px(size::CONTROL))
                        // Off, its tooltip says why; the toolbar keeps the address.
                        .when(!down, |d| {
                            d.on_hover(Self::hint(
                                "Click an element on the page to point the agent at it",
                                cx,
                            ))
                        })
                        .px(px(space::S2))
                        .flex()
                        .items_center()
                        .gap(px(space::S1H))
                        .rounded(px(radius::MD))
                        .cursor_pointer()
                        .when(self.picking, |d| d.bg(t.ink).text_color(t.surface))
                        .when(!self.picking && !down, |d| {
                            d.text_color(t.ink).hover(|d| d.bg(t.hover()))
                        })
                        .map(|d| {
                            if down {
                                crate::ui::off(d.text_color(t.ink), "The dev server stopped", &t)
                            } else {
                                d.on_press(cx.listener(|this, _, _, cx| this.toggle_pick(cx)))
                            }
                        })
                        .child(icon(
                            "square-dashed-mouse-pointer",
                            14.,
                            if self.picking { t.surface } else { t.quiet },
                        ))
                        .child(div().ty(UI).child("Point"))
                        .child(probe(&self.probes, "pick")),
                )
            })
            .child(
                icon_button("reload", "rotate-cw", &t)
                    .on_hover(Self::hint("Reload", cx))
                    .on_press(cx.listener(move |this, _, _, cx| {
                        // A stopped server is checked again rather than
                        // loaded into an error page.
                        if down {
                            let up = crate::app::server_answers(a.path);
                            this.server_seen(id, up, cx);
                            return;
                        }
                        if let Some(p) = &this.preview {
                            p.borrow_mut().reload();
                        }
                        cx.notify();
                    })),
            )
            .when(!detached, |d| {
                d.child(
                    icon_button("pop-out", "picture-in-picture-2", &t)
                        .relative()
                        .on_hover(Self::hint("Open in its own window", cx))
                        .on_press(
                            cx.listener(move |this, _, window, cx| this.pop_out(id, window, cx)),
                        )
                        .child(probe(&self.probes, "pop-out")),
                )
            })
            .child({
                let server_url = a.path.to_string();
                // The latest version is the file itself, beside its styles
                // and pictures; an older one is written out on its own.
                // Another machine's file opens from a copy here, made when
                // it's asked for, never just because this is drawn.
                let far = far_ask(&a);
                let on_disk = (v == latest && far.is_none())
                    .then(|| art_file(&a))
                    .flatten()
                    .filter(|f| f.is_file()); // this Mac's
                let file = (!server && on_disk.is_none()).then(|| {
                    (
                        a.versions
                            .get(v - 1)
                            .cloned()
                            .unwrap_or_else(|| Rc::from(&b""[..])),
                        a.path.rsplit('/').next().unwrap_or(a.path).to_string(),
                    )
                });
                let in_app = media.then(|| on_disk.clone()).flatten();
                icon_button("external", "external-link", &t)
                    .on_hover(Self::hint(
                        if in_app.is_some() {
                            "Open in its app"
                        } else {
                            "Open in your browser"
                        },
                        cx,
                    ))
                    .on_press(cx.listener(move |this, _, _, cx| match (&on_disk, &file) {
                        _ if far.is_some() && (media || v == latest) => {
                            if let Some(ask) = far.clone() {
                                this.open_outside_far(ask, cx);
                            }
                        }
                        (Some(path), _) if in_app.is_some() => {
                            // this Mac's: the file itself
                            crate::open::open(&path.display().to_string(), cx)
                        }
                        (Some(path), _) => cx.open_url(&format!("file://{}", path.display())),
                        (None, Some((bytes, name))) => {
                            let path =
                                std::env::temp_dir().join(format!("dent-preview-v{v}-{name}"));
                            if std::fs::write(&path, &bytes[..]).is_ok() {
                                cx.open_url(&format!("file://{}", path.display()));
                            }
                        }
                        (None, None) => cx.open_url(&server_url),
                    }))
            });

        // The web view is a native layer over this area. GPUI draws the frame
        // and, underneath, the page's last snapshot: when a menu hides the
        // web view, the snapshot is what you see.
        let area_preview = self.preview.clone();
        let width = viewport.width().filter(|w| self.panel_w > w + space::S8);
        let area = if detached {
            self.own_window_note(cx)
        } else if down {
            self.server_stopped(id, &a, cx)
        } else {
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .flex()
                .justify_center()
                .pl(px(GRIP))
                .bg(if width.is_some() { t.surface } else { t.panel })
                .when(width.is_some(), |d| d.py(px(space::S4)))
                .child(
                    div()
                        .h_full()
                        .when_some(width, |d, w| d.w(px(w)))
                        .when(width.is_none(), |d| d.w_full())
                        .child(
                            canvas(
                                move |bounds, _, _| {
                                    if let Some(p) = &area_preview {
                                        p.borrow_mut().place(bounds);
                                    }
                                    (bounds, area_preview)
                                },
                                move |_, (inner, preview), window, _| {
                                    if let Some(snap) = preview
                                        .and_then(|p| p.borrow().snapshot(inner.size, dragging))
                                    {
                                        // While resizing, the last picture stays at its
                                        // own size, pinned top left and clipped, rather
                                        // than leaving the area blank.
                                        let at = Bounds::new(inner.origin, snap.size);
                                        window.with_content_mask(
                                            Some(ContentMask { bounds: inner }),
                                            |window| {
                                                let _ = window.paint_image(
                                                    at,
                                                    at,
                                                    Corners::default(),
                                                    snap.image,
                                                    0,
                                                    false,
                                                );
                                            },
                                        );
                                    }
                                    if width.is_some() {
                                        // A hairline frame around a narrow viewport.
                                        window.paint_quad(outline(
                                            inner.dilate(px(1.)),
                                            t.hairline,
                                            BorderStyle::Solid,
                                        ));
                                    }
                                },
                            )
                            .size_full(),
                        ),
                )
                .child(probe(&self.probes, "preview"))
                .into_any_element()
        };

        let banner = (!server && v < latest).then(|| {
            div()
                .h(px(36.))
                .flex_none()
                .px(px(space::S3))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .border_b_1()
                .border_color(t.hairline)
                .bg(t.selected())
                .child(icon("rotate-ccw-clock", 13., t.quiet))
                .child(
                    div()
                        .flex_1()
                        .ty(UI)
                        .text_color(t.ink)
                        .child(format!("Viewing v{v}, an earlier version.")),
                )
                .child(
                    ghost("latest", format!("Back to v{latest}"), &t)
                        .relative()
                        .on_press(cx.listener(move |this, _, _, cx| this.open_art(id, None, cx)))
                        .child(probe(&self.probes, "latest")),
                )
        });
        let hint = self.picking.then(|| {
            div()
                .h(px(36.))
                .flex_none()
                .px(px(space::S3))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .border_b_1()
                .border_color(t.hairline)
                .child(icon("mouse-pointer-click", 13., t.quiet))
                .child(
                    div()
                        .flex_1()
                        .ty(UI)
                        .text_color(t.ink)
                        .child("Click anything on the page to point the agent at it."),
                )
                .child(kbd("esc to cancel", &t))
        });
        let console = errors.first().cloned().map(|e| {
            let more = errors.len().saturating_sub(1);
            div()
                .min_h(px(44.))
                .flex_none()
                .px(px(space::S3))
                .py(px(space::S2))
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
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .ty(UI)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.ink)
                                .child(if more > 0 {
                                    format!("{} errors on this page", errors.len())
                                } else {
                                    "An error on this page".into()
                                }),
                        )
                        .child(div().truncate().ty(CODE).text_color(t.quiet).child(e)),
                )
                .child(
                    button("fix", "Ask to fix", true, &t)
                        .relative()
                        .on_press(cx.listener(move |this, _, window, cx| {
                            this.ask_to_fix(id, v, window, cx)
                        }))
                        .child(probe(&self.probes, "fix")),
                )
        });
        // ⌘F in the page: a strip of its own under the toolbar, as the
        // page is drawn over anything that would float on it.
        let find = self.find_bar(crate::find::Scope::Page(id), cx).map(|bar| {
            div()
                .flex_none()
                .relative()
                .h(px(size::CONTROL + space::S2 * 3.))
                .border_b_1()
                .border_color(t.hairline)
                .child(bar)
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(toolbar)
            .children(find)
            .children(banner)
            .children(hint)
            .child(area)
            .children(console)
            .into_any_element()
    }

    /// The viewer while the page is in a window of its own.
    /// Keeps checking the dev server on screen: when it stops answering
    /// its page gives way to a note, and when it's back the page reloads.
    /// One check runs at a time, for the server shown; it ends when that
    /// server is no longer shown.
    fn watch_server(&mut self, id: &'static str, url: &'static str, cx: &mut Context<Self>) {
        if self.server_watch.as_ref().is_some_and(|(w, _)| *w == id) {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            loop {
                let up = cx
                    .background_executor()
                    .spawn(async move { crate::app::server_answers(url) })
                    .await;
                let shown = this
                    .update(cx, |this, cx| {
                        let shown = this.current().is_some_and(|s| this.tab(s) == Tab::Art(id))
                            && !this.popouts.contains_key(id);
                        if !shown {
                            this.server_watch = None;
                            return false;
                        }
                        this.server_seen(id, up, cx);
                        true
                    })
                    .unwrap_or(false);
                if !shown {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
            }
        });
        self.server_watch = Some((id, task));
    }

    /// What a check of a dev server found.
    fn server_seen(&mut self, id: &'static str, up: bool, cx: &mut Context<Self>) {
        let was_down = self.servers_down.contains(id);
        if up == !was_down {
            return;
        }
        if up {
            self.servers_down.remove(id);
            if let Some(p) = &self.preview {
                p.borrow_mut().reload();
            }
        } else {
            self.servers_down.insert(id);
        }
        // A hint said over the toolbar no longer fits what it shows.
        self.toolbar_hint = None;
        cx.notify();
    }

    /// In place of a dev server's page once it stops answering.
    fn server_stopped(
        &mut self,
        id: &'static str,
        a: &crate::data::Artifact,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let url = a.path;
        // The address as the agent gave it, as on its tab.
        let shown = a.title.trim_start_matches("Dev server · ");
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(space::S3))
            .px(px(space::S6))
            .bg(t.panel)
            .child(icon("unplug", 20., t.quiet))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .ty(UI)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.ink)
                            .child("The dev server stopped."),
                    )
                    .child(
                        div()
                            .ty(UI)
                            .text_color(t.quiet)
                            .text_center()
                            .child(format!(
                                "Nothing answers at {shown}. The page comes back when it does."
                            )),
                    ),
            )
            .child(
                button("server-retry", "Check again", false, &t)
                    .relative()
                    .on_press(cx.listener(move |this, _, _, cx| {
                        let up = crate::app::server_answers(url);
                        this.server_seen(id, up, cx);
                    }))
                    .child(probe(&self.probes, "server-retry")),
            )
            .child(probe(&self.probes, "server-stopped"))
            .into_any_element()
    }

    fn popped_out(
        &mut self,
        id: &'static str,
        a: &crate::data::Artifact,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                self.toolbar().pl(px(space::S3)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .ty(LABEL)
                        .text_color(t.quiet)
                        .child(self.shown(a)),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(space::S3))
                    .px(px(space::S6))
                    .child(icon("picture-in-picture-2", 20., t.quiet))
                    .child(
                        div()
                            .ty(UI)
                            .text_color(t.ink)
                            .child("This page is open in its own window."),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(space::S2))
                            .child(ghost("show-popout", "Show window", &t).on_press(
                                cx.listener(move |this, _, window, cx| {
                                    this.pop_out(id, window, cx)
                                }),
                            ))
                            .child(
                                button("bring-back", "Bring it back", false, &t)
                                    .relative()
                                    .on_press(
                                        cx.listener(move |this, _, _, cx| this.bring_back(id, cx)),
                                    )
                                    .child(probe(&self.probes, "bring-back")),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// On Wayland the page lives in its own window; the viewer says so.
    fn own_window_note(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(space::S3))
            .px(px(space::S6))
            .child(icon("app-window", 20., t.quiet))
            .child(
                div()
                    .ty(UI)
                    .text_color(t.ink)
                    .child("The page opens in its own window here."),
            )
            .child(
                ghost("raise-page", "Show the page", &t).on_press(cx.listener(|this, _, _, _| {
                    if let Some(p) = &this.preview {
                        p.borrow().raise();
                    }
                })),
            )
            .into_any_element()
    }

    fn doc(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let Some(a) = self.art(id).cloned() else {
            return div().into_any_element();
        };
        // The version you chose, the latest unless you stepped back.
        let v = self.version(&a);
        let blocks = self.doc_blocks(id, v);
        let mut r = markdown::R {
            live: false,
            counted: false,
            md: self.mdv.clone(),
            owner: cx.entity_id(),
            window,
            cx,
            scope: format!("doc/{id}"),
            bg: t.panel,
            // Links and pictures in a document are found from its folder.
            base: art_file(&a).and_then(|f| f.parent().map(|p| p.to_path_buf())),
            session: Some((
                art_session(a.id).to_string(),
                a.dir
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default(),
            )),
        };
        let area = crate::select::area(&format!("doc/{id}"));
        let page = {
            let _place = crate::select::item(area, 0);
            markdown::render(blocks.as_slice(), &t, true, &mut r)
        };
        let scroll = self.doc_scrolls.entry(id).or_default().clone();
        let by = scroll.clone();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                self.toolbar()
                    .pl(px(space::S3))
                    .children(self.versions(&a, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(self.shown(&a)),
                    )
                    .child({
                        let copied = self
                            .copied_at
                            .is_some_and(|at| at.elapsed().as_secs_f32() < 1.5);
                        // The text is read when you copy, not each frame.
                        let bytes = a.versions[v - 1].clone();
                        ghost("copy-doc", if copied { "Copied" } else { "Copy text" }, &t)
                            .with_tip(tip("Copy the file's text", None, t))
                            .on_press(cx.listener(move |this, _, _, cx| {
                                let src = String::from_utf8_lossy(&bytes).into_owned();
                                cx.write_to_clipboard(ClipboardItem::new_string(src));
                                this.copied_at = Some(std::time::Instant::now());
                                cx.notify();
                            }))
                    })
                    .when_some(art_file(&a).filter(|_| !art_far(&a)), |d, full| {
                        d.child(self.open_button("open-doc", full, cx))
                    })
                    .when_some(far_ask(&a), |d, ask| {
                        d.child(self.open_far_button("open-doc", ask, cx))
                    }),
            )
            .child(
                // The region stays put over what's on screen, outside what
                // scrolls.
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        // Each document keeps its own place.
                        div()
                            .id(SharedString::from(format!("doc-{id}")))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .restrict_scroll_to_axis()
                            .track_scroll(&scroll)
                            .child(
                                div()
                                    .max_w(px(680.))
                                    .mx_auto()
                                    .px(px(space::S8))
                                    .py(px(space::S6))
                                    .child(page),
                            ),
                    )
                    .child(crate::select::region(area, move |dy| {
                        let (o, max) = (by.offset(), by.max_offset());
                        by.set_offset(point(o.x, (o.y - px(dy)).clamp(-max.y, px(0.))));
                    }))
                    .child(probe(&self.probes, "doc")),
            )
            .into_any_element()
    }

    fn image(&mut self, id: &'static str, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let Some(a) = self.art(id).cloned() else {
            return div().into_any_element();
        };
        let v = self.version(&a);
        let (w, h) = picture_size(id, v, &a.versions[v - 1]);
        let actual = self.image_actual;
        // Actual size is one image pixel per screen pixel.
        let scale = window.scale_factor().max(1.);
        let seg = |key: &'static str,
                   label: &'static str,
                   on: bool,
                   value: bool,
                   cx: &mut Context<Self>| {
            ghost(key, label, &t)
                .when(on, |d| d.bg(t.selected()).text_color(t.ink))
                .on_press(cx.listener(move |this, _, _, cx| {
                    this.image_actual = value;
                    cx.notify();
                }))
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                self.toolbar()
                    .pl(px(space::S3))
                    .children(self.versions(&a, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(self.shown(&a)),
                    )
                    .when(w > 0, |d| {
                        d.child(
                            div()
                                .ty(LABEL)
                                .text_color(t.faint)
                                .child(format!("{w} × {h}")),
                        )
                    })
                    .child(seg("img-fit", "Fit", !actual, false, cx))
                    .child(seg("img-full", "Actual size", actual, true, cx)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("image-{id}")))
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .restrict_scroll_to_axis()
                    .p(px(space::S6))
                    .flex()
                    .when(!actual, |d| d.items_center().justify_center())
                    .bg(t.surface)
                    .children(self.picture(id, v).map(|picture| {
                        img(picture)
                            .map(|i| {
                                if actual && w > 0 {
                                    i.flex_none()
                                        .w(px(w as f32 / scale))
                                        .h(px(h as f32 / scale))
                                } else {
                                    i.max_w_full().max_h_full()
                                }
                            })
                            .object_fit(ObjectFit::Contain)
                            .rounded(px(radius::LG))
                            .border_1()
                            .border_color(t.hairline)
                    })),
            )
            .into_any_element()
    }

    /// Which of the changed files is shown.
    pub fn diff_selected(&self, s: &Session) -> usize {
        self.diff_file
            .get(&s.id)
            .copied()
            .unwrap_or(0)
            .min(s.diff.len().saturating_sub(1))
    }

    /// The rows of a file's diff: one per line, plus a row for each
    /// comment and the open comment box.
    pub fn diff_rows(&self, file: &FileDiff) -> (Vec<Row>, Vec<(u32, String)>) {
        let comments: Vec<(u32, String)> = self
            .review
            .iter()
            .filter_map(|a| match a {
                crate::app::Attachment::Comment {
                    file: f,
                    line,
                    text,
                } if *f == file.path => Some((*line, text.clone())),
                _ => None,
            })
            .collect();
        // One row per line, plus a row for each comment and the open comment box.
        let mut rows: Vec<Row> = Vec::with_capacity(file.lines.len());
        for (i, l) in file.lines.iter().enumerate() {
            rows.push(Row::Line(i));
            if l.kind == Line::Hunk {
                continue;
            }
            let n = l.new.or(l.old).unwrap_or(0);
            for (k, _) in comments
                .iter()
                .enumerate()
                .filter(|(_, (line, _))| *line == n)
            {
                rows.push(Row::Note(k));
            }
            if self
                .commenting
                .as_ref()
                .is_some_and(|(f, line)| *f == file.path && *line == n)
            {
                rows.push(Row::Compose);
            }
        }
        (rows, comments)
    }

    fn changes(&mut self, s: &Session, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let selected = self.diff_selected(s);
        let sid = s.id.clone();
        let total_add: usize = s.diff.iter().map(|f| f.added).sum();
        let total_del: usize = s.diff.iter().map(|f| f.removed).sum();
        // A narrow panel gives the diff all the room; files step with arrows.
        let compact = self.panel_w < 560.;
        let count = s.diff.len();
        let files = div()
            .w(px(200.))
            .flex_none()
            .border_r_1()
            .border_color(t.hairline)
            .py(px(space::S2))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(space::S3))
                    .pb(px(space::S2))
                    .ty(LABEL)
                    .text_color(t.quiet)
                    .child(format!(
                        "{} {} · +{total_add} −{total_del}",
                        s.diff.len(),
                        if s.diff.len() == 1 { "FILE" } else { "FILES" }
                    )),
            )
            .children(s.diff.iter().enumerate().map(|(i, f)| {
                let sid = sid.clone();
                let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
                let dir = f
                    .path
                    .rsplit_once('/')
                    .map(|(d, _)| d.to_string())
                    .unwrap_or_default();
                div()
                    .id(SharedString::from(format!("file-{i}")))
                    .mx(px(space::S1))
                    .px(px(space::S2))
                    .py(px(space::S1))
                    .rounded(px(radius::MD))
                    .flex()
                    .flex_col()
                    .cursor_pointer()
                    .when(i == selected, |d| d.bg(t.selected()))
                    .when(i != selected, |d| d.hover(|d| d.bg(t.hover())))
                    .on_press(cx.listener(move |this, _, _, cx| {
                        this.diff_file.insert(sid.clone(), i);
                        this.commenting = None;
                        cx.notify();
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
                                    .truncate()
                                    .ty(UI)
                                    .text_color(t.ink)
                                    .child(name),
                            )
                            .child(div().ty(LABEL).text_color(t.faint).child(counts(f))),
                    )
                    .child(div().truncate().ty(LABEL).text_color(t.faint).child(dir))
            }));
        let Some(shared) = self.current_rc() else {
            return div().into_any_element();
        };
        let Some(file) = s.diff.get(selected) else {
            return div().into_any_element();
        };
        let (rows, comments) = self.diff_rows(file);
        let hash = |v: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            use std::hash::Hasher;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            v(&mut h);
            h.finish()
        };
        use std::hash::Hash;
        let file_key = hash(&|h| (&s.id, selected).hash(h));
        let key = hash(&|h| (file_key, rows.len(), &self.commenting, comments.len()).hash(h));
        if key != self.diff_key {
            // A new file starts at the top; a comment opening or landing keeps your place.
            let keep =
                (file_key == self.diff_file_key).then(|| self.diff_list.logical_scroll_top());
            self.diff_key = key;
            self.diff_file_key = file_key;
            self.diff_list.reset(rows.len());
            if let Some(top) = keep {
                self.diff_list.scroll_to(top);
            }
        }
        // Long lines scroll sideways: the list is as wide as the longest line.
        let longest = file
            .lines
            .iter()
            .map(|l| l.text.chars().count())
            .max()
            .unwrap_or(0) as f32;
        let wide = 64. + 14. + longest * self.code_advance(window) + space::S4;
        // Notes and the comment box fit the visible diff, not the longest line.
        self.diff_view_w = self.panel_w - if compact { 0. } else { 200. } - space::S2;
        let (rows, comments) = (Rc::new(rows), Rc::new(comments));
        let area = crate::select::area(&format!(
            "diff/{}/{}",
            s.id,
            s.diff.get(selected).map_or("", |f| f.path.as_str())
        ));
        let diff = list(
            self.diff_list.clone(),
            cx.processor(move |this, ix: usize, _, cx| {
                let _place = crate::select::item(area, ix);
                this.diff_row(&shared, selected, &rows, &comments, ix, cx)
            }),
        )
        .size_full();
        let lines = self.diff_list.clone();
        let review = (!self.review.is_empty()).then(|| {
            let n = self.review.len();
            div()
                .h(px(48.))
                .flex_none()
                .px(px(space::S3))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .border_t_1()
                .border_color(t.hairline)
                .bg(t.raised)
                .child(icon("message-square", 14., t.quiet))
                .child(div().flex_1().ty(UI).text_color(t.ink).child(if n == 1 {
                    "1 comment in your review".to_string()
                } else {
                    format!("{n} comments in your review")
                }))
                .child(
                    button("send-review", "Send to agent", true, &t)
                        .relative()
                        .on_press(cx.listener(|this, _, window, cx| this.send_review(window, cx)))
                        .child(probe(&self.probes, "send-review")),
                )
        });
        // Showing one turn: says so, with the way back to everything.
        let turn_strip =
            self.changes_turn.contains_key(&s.id).then(|| {
                let sid = s.id.clone();
                div()
                    .flex_none()
                    .h(px(size::CONTROL + space::S1))
                    .px(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .border_b_1()
                    .border_color(t.hairline)
                    .bg(t.raised)
                    .child(icon("rotate-ccw-clock", 13., t.quiet))
                    .child(
                        div()
                            .flex_1()
                            .ty(UI)
                            .text_color(t.ink)
                            .child("What this turn changed"),
                    )
                    .child(
                        ghost("all-changes", "Show all changes", &t)
                            .relative()
                            .on_press(cx.listener(move |this, _, _, cx| {
                                this.review_turn(&sid, None, None, cx)
                            }))
                            .child(probe(&self.probes, "all-changes")),
                    )
            });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .children(turn_strip)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!compact, |d| d.child(files))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .h(px(size::TOOLBAR))
                                    .flex_none()
                                    .px(px(space::S3))
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S2))
                                    .border_b_1()
                                    .border_color(t.hairline)
                                    .when(compact, |d| {
                                        let (a, b) = (sid.clone(), sid.clone());
                                        d.ml(px(-space::S2))
                                            .child(
                                                icon_button("prev-file", "chevron-left", &t)
                                                    .on_press(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.diff_file.insert(
                                                                a.clone(),
                                                                selected.saturating_sub(1),
                                                            );
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                icon_button("next-file", "chevron-right", &t)
                                                    .on_press(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.diff_file.insert(
                                                                b.clone(),
                                                                (selected + 1).min(count - 1),
                                                            );
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .ty(LABEL)
                                            .text_color(t.ink)
                                            .child(file.path.clone()),
                                    )
                                    .child({
                                        let full = std::path::Path::new(&s.cwd).join(&file.path);
                                        self.open_button("open-file", full, cx)
                                    })
                                    .child(self.discard_file(s, &file.path, cx))
                                    .child(div().ty(LABEL).text_color(t.faint).child(
                                        match (compact, file.lines.is_empty()) {
                                            (true, _) => {
                                                format!(
                                                    "{} of {count} · {}",
                                                    selected + 1,
                                                    counts(file)
                                                )
                                            }
                                            // The pane below says what it is.
                                            (false, true) => String::new(),
                                            (false, false) => counts(file),
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .relative()
                                    .child(if file.lines.is_empty() {
                                        // Nothing to compare line by line: a
                                        // picture shows itself, anything else says so.
                                        // From the session's machine: this Mac's file,
                                        // or the copy of another's.
                                        let ask = crate::files::Ask::new(&s.id, &file.path, &s.cwd);
                                        let shown = (crate::data::Kind::of(&file.path)
                                            == Some(crate::data::Kind::Image))
                                        .then(|| crate::files::picture(&ask).shown())
                                        .flatten();
                                        let picture = shown.is_some();
                                        let full = shown.unwrap_or_default();
                                        let file_size = match crate::files::stat(&ask) {
                                            crate::files::Look::Here(
                                                dent_protocol::FileStat::File { stamp, .. },
                                            )
                                            | crate::files::Look::Stale(
                                                dent_protocol::FileStat::File { stamp, .. },
                                            ) => Some(stamp.size),
                                            _ => None,
                                        };
                                        div()
                                            .id("diff")
                                            .size_full()
                                            .p(px(space::S6))
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .justify_center()
                                            .gap(px(space::S3))
                                            .when(picture, |d| {
                                                d.child(
                                                    div()
                                                        .max_w_full()
                                                        .max_h(px(360.))
                                                        .rounded(px(radius::MD))
                                                        .border_1()
                                                        .border_color(t.hairline)
                                                        .overflow_hidden()
                                                        .child(
                                                            img(full.clone())
                                                                .max_w_full()
                                                                .max_h(px(360.))
                                                                .object_fit(ObjectFit::Contain),
                                                        ),
                                                )
                                            })
                                            // What it is and how big; the
                                            // list beside says it changed.
                                            .child(div().ty(UI).text_color(t.quiet).child({
                                                let size = file_size.map(bytes);
                                                let what =
                                                    if picture { "Picture" } else { "Binary file" };
                                                match size {
                                                    Some(b) => format!("{what} · {b}"),
                                                    None => what.to_string(),
                                                }
                                            }))
                                            .into_any_element()
                                    } else {
                                        // The region stays over what's on
                                        // screen, outside what scrolls.
                                        div()
                                            .relative()
                                            .size_full()
                                            .child(crate::swipe::lock(
                                                Some(lines.clone()),
                                                Some(self.diff_side.clone()),
                                            ))
                                            .child(
                                                div()
                                                    .id("diff")
                                                    .size_full()
                                                    .overflow_x_scroll()
                                                    .track_scroll(&self.diff_side)
                                                    .restrict_scroll_to_axis()
                                                    .child(
                                                        div()
                                                            .h_full()
                                                            .min_w_full()
                                                            .w(px(wide))
                                                            .py(px(space::S1))
                                                            .child(diff),
                                                    ),
                                            )
                                            .child(crate::select::region(area, move |dy| {
                                                lines.scroll_by(px(dy))
                                            }))
                                            .child(probe(&self.probes, "diff"))
                                            .into_any_element()
                                    })
                                    // Long lines scroll sideways; the edge fades to say so.
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .right_0()
                                            .w(px(space::S6))
                                            .bg(linear_gradient(
                                                90.,
                                                linear_color_stop(t.panel.opacity(0.), 0.),
                                                linear_color_stop(t.panel, 1.),
                                            )),
                                    ),
                            ),
                    ),
            )
            .children(review)
            .into_any_element()
    }

    /// A text file opened from a link: read-only, numbered and highlighted
    /// like the diff, at the line the link named, which stays marked.
    fn text(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let Some(a) = self.art(id).cloned() else {
            return div().into_any_element();
        };
        let v = self.version(&a);
        let src: Rc<str> = String::from_utf8_lossy(&a.versions[v - 1]).into();
        let lines: Rc<Vec<(usize, usize)>> = Rc::new({
            let mut at = 0;
            src.split('\n')
                .map(|l| {
                    let r = (at, at + l.trim_end_matches('\r').len());
                    at += l.len() + 1;
                    r
                })
                .collect()
        });
        let marked = self.marked_line.get(id).copied();
        // The line count is part of it: an edited file is read again from
        // disk, and may have changed there under the same version.
        if self.text_key != Some((id, v, lines.len())) {
            self.text_key = Some((id, v, lines.len()));
            self.text_list.reset(lines.len());
            // A few lines above the marked one, so it reads in context.
            if let Some(l) = marked {
                self.text_list.scroll_to(ListOffset {
                    item_ix: (l as usize)
                        .saturating_sub(4)
                        .min(lines.len().saturating_sub(1)),
                    offset_in_item: px(0.),
                });
            }
        }
        let lang: SharedString = a
            .path
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_string()
            .into();
        let longest = lines
            .iter()
            .map(|(s, e)| src[*s..*e].chars().count())
            .max()
            .unwrap_or(0);
        let wide = 64. + longest as f32 * self.code_advance(window) + space::S4 * 2.;
        let area = crate::select::area(&format!("text/{id}"));
        let body = {
            let (src, lines, lang) = (src.clone(), lines.clone(), lang.clone());
            list(
                self.text_list.clone(),
                cx.processor(move |_, ix: usize, _, _| {
                    let _place = crate::select::item(area, ix);
                    let (s, e) = lines[ix];
                    let text: SharedString = src[s..e].to_string().into();
                    let n = ix as u32 + 1;
                    let here = marked == Some(n);
                    div()
                        .flex()
                        .w_full()
                        .h(px(size::LINE))
                        .border_l_2()
                        .border_color(if here { t.ink } else { transparent_black() })
                        .when(here, |d| d.bg(t.selected()))
                        .child(
                            div()
                                .w(px(64.))
                                .flex_none()
                                .flex()
                                .justify_end()
                                .pr(px(space::S4))
                                .ty(CODE)
                                .text_color(if here { t.ink } else { t.faint })
                                .child(n.to_string()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .pr(px(space::S4))
                                .whitespace_nowrap()
                                .ty(CODE)
                                .text_color(t.ink)
                                .child(if text.is_empty() {
                                    text.into_any_element()
                                } else {
                                    let spans = crate::highlight::spans(&lang, &text);
                                    crate::select::styled(
                                        SharedString::from(format!("text-{ix}")),
                                        text.clone(),
                                        crate::highlight::runs(&spans, &text, &t),
                                        &t,
                                    )
                                    .into_any_element()
                                }),
                        )
                        .into_any_element()
                }),
            )
            .size_full()
        };
        let scroll = self.text_list.clone();
        let bytes = a.versions[v - 1].clone();
        let copied = self
            .copied_at
            .is_some_and(|at| at.elapsed().as_secs_f32() < 1.5);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                self.toolbar()
                    .pl(px(space::S3))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(match marked {
                                Some(l) => format!("{} · line {l}", self.shown(&a)),
                                None => self.shown(&a).to_string(),
                            }),
                    )
                    .child(
                        ghost("copy-text", if copied { "Copied" } else { "Copy text" }, &t)
                            .with_tip(tip("Copy the file's text", None, t))
                            .on_press(cx.listener(move |this, _, _, cx| {
                                let src = String::from_utf8_lossy(&bytes).into_owned();
                                cx.write_to_clipboard(ClipboardItem::new_string(src));
                                this.copied_at = Some(std::time::Instant::now());
                                cx.notify();
                            })),
                    )
                    .when_some(art_file(&a).filter(|_| !art_far(&a)), |d, full| {
                        d.child(self.open_button("open-text", full, cx))
                    })
                    .when_some(far_ask(&a), |d, ask| {
                        d.child(self.open_far_button("open-text", ask, cx))
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(crate::swipe::lock(
                        Some(scroll.clone()),
                        Some(self.text_side.clone()),
                    ))
                    .child(
                        div()
                            .id(SharedString::from(format!("text-{id}")))
                            .size_full()
                            .overflow_x_scroll()
                            .track_scroll(&self.text_side)
                            .restrict_scroll_to_axis()
                            .child(
                                div()
                                    .h_full()
                                    .min_w_full()
                                    .w(px(wide))
                                    .py(px(space::S1))
                                    .child(body),
                            ),
                    )
                    .child(crate::select::region(area, move |dy| {
                        scroll.scroll_by(px(dy))
                    }))
                    .child(probe(&self.probes, "text-view")),
            )
            .into_any_element()
    }

    /// The advance of one character of code, measured once.
    fn code_advance(&mut self, window: &mut Window) -> f32 {
        if self.code_em == 0. {
            let font = font(crate::theme::mono());
            let id = window.text_system().resolve_font(&font);
            self.code_em = window
                .text_system()
                .em_advance(id, px(CODE.zoomed().size))
                .map(|p| p.as_f32())
                .unwrap_or(7.2);
        }
        self.code_em
    }

    /// One row of the diff: a line, a comment on it, or the comment box.
    fn diff_row(
        &mut self,
        s: &Session,
        selected: usize,
        rows: &[Row],
        comments: &[(u32, String)],
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let Some(file) = s.diff.get(selected) else {
            return div().into_any_element();
        };
        match rows.get(ix) {
            Some(Row::Line(i)) => {
                let i = *i;
                let Some(l) = file.lines.get(i) else {
                    return div().into_any_element();
                };
                let n = l.new.or(l.old).unwrap_or(0);
                let (bg, bar) = match l.kind {
                    Line::Added => (Some(t.added), Some(t.done.0)),
                    Line::Removed => (Some(t.removed), Some(t.quiet)),
                    Line::Hunk => (Some(t.selected()), None),
                    Line::Context => (None, None),
                };
                let path = file.path.clone();
                let commentable = l.kind != Line::Hunk;
                // Which hunk this header starts, counted from 0.
                let hunk = (l.kind == Line::Hunk).then(|| {
                    file.lines[..i]
                        .iter()
                        .filter(|x| x.kind == Line::Hunk)
                        .count()
                });
                div()
                    .id(SharedString::from(format!("line-{i}")))
                    .group("line")
                    .relative()
                    .flex()
                    .w_full()
                    .h(px(size::LINE))
                    .when_some(bg, |d, c| d.bg(c))
                    .border_l_2()
                    .border_color(bar.unwrap_or(transparent_black()))
                    .child(
                        div()
                            .id(SharedString::from(format!("gutter-{i}")))
                            .relative()
                            .w(px(64.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(space::S1))
                            .pr(px(space::S2))
                            .ty(CODE)
                            .text_color(t.faint)
                            .when(commentable, |d| {
                                d.cursor_pointer()
                                    .on_press(cx.listener(move |this, _, window, cx| {
                                        this.start_comment(path.clone(), n, window, cx)
                                    }))
                                    .child(
                                        div()
                                            .invisible()
                                            .group_hover("line", |s| s.visible())
                                            .child(icon("message-square-plus", 12., t.quiet)),
                                    )
                            })
                            .child(match l.kind {
                                Line::Hunk => String::new(),
                                _ => n.to_string(),
                            })
                            .child(probe(&self.probes, format!("gutter-{i}"))),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .ty(CODE)
                            .text_color(t.faint)
                            .child(match l.kind {
                                Line::Added => "+",
                                Line::Removed => "−",
                                _ => "",
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .pr(px(space::S4))
                            .whitespace_nowrap()
                            .ty(CODE)
                            .text_color(if l.kind == Line::Hunk { t.quiet } else { t.ink })
                            .child(if l.kind == Line::Hunk || l.text.is_empty() {
                                l.text.clone().into_any_element()
                            } else {
                                // Each line on its own: enough for keywords,
                                // strings and comments, cached per line.
                                let lang = file.path.rsplit('.').next().unwrap_or_default();
                                let spans = crate::highlight::spans(lang, &l.text);
                                crate::select::styled(
                                    SharedString::from(format!("diff-text-{i}")),
                                    l.text.clone(),
                                    crate::highlight::runs(&spans, &l.text, &t),
                                    &t,
                                )
                                // Lines of a file copy one to a line, with
                                // the blank lines above kept.
                                .lead({
                                    let blanks = file.lines[..i]
                                        .iter()
                                        .rev()
                                        .take_while(|b| b.kind != Line::Hunk && b.text.is_empty())
                                        .count();
                                    "\n".repeat(blanks)
                                })
                                .into_any_element()
                            }),
                    )
                    .when_some(hunk, |d, h| {
                        d.child(self.discard_hunk(s, &file.path, h, cx))
                    })
                    .into_any_element()
            }
            Some(Row::Note(k)) => match comments.get(*k) {
                // Notes stay inside the visible width, not the longest line's.
                // The list sizes its rows; the note inside keeps to the visible width.
                Some((line, text)) => div()
                    .w_full()
                    .flex()
                    .child(
                        div()
                            .w(px(self.diff_view_w))
                            .flex_none()
                            .child(comment_note(*line, text, &t)),
                    )
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Some(Row::Compose) => div()
                .w_full()
                .flex()
                .child(
                    div().w(px(self.diff_view_w)).flex_none().child(
                        div()
                            .mx(px(space::S3))
                            .my(px(space::S2))
                            .p(px(space::S3))
                            .rounded(px(radius::LG))
                            .bg(t.raised)
                            .border_1()
                            .border_color(t.hairline)
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .ty(UI)
                                    .text_color(t.ink)
                                    .child(Textarea::new(&self.comment_input)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S2))
                                    .child(
                                        div()
                                            .flex_1()
                                            .ty(LABEL)
                                            .text_color(t.faint)
                                            .child("comments collect into one review"),
                                    )
                                    .child(ghost("cancel-comment", "Cancel", &t).on_press(
                                        cx.listener(|this, _, _, cx| {
                                            this.commenting = None;
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        button("add-comment", "Add comment", true, &t)
                                            .relative()
                                            .on_press(cx.listener(|this, _, window, cx| {
                                                this.add_comment(window, cx)
                                            }))
                                            .child(probe(&self.probes, "add-comment")),
                                    ),
                            ),
                    ),
                )
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// Side questions about the session, answered with everything said so
    /// far, kept out of the conversation; any answer can become a fork.
    fn side_chat(
        &mut self,
        s: &Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let started = s
            .items
            .iter()
            .any(|i| matches!(i, crate::data::Item::Text(_)));
        let area = crate::select::area(&format!("aside/{}", s.id));
        let by = self.aside_scroll.clone();
        // A drag past the edge scrolls, and ⌘F brings a match into view.
        let region = crate::select::region(area, move |dy| {
            let (o, max) = (by.offset(), by.max_offset());
            by.set_offset(point(o.x, (o.y - px(dy)).clamp(-max.y, px(0.))));
        });
        let mut list = div()
            .id("asides")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .track_scroll(&self.aside_scroll)
            .p(px(space::S4))
            .flex()
            .flex_col()
            .gap(px(space::S6));
        if s.asides.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .gap(px(space::S3))
                    .child(div().pt(px(3.)).child(icon("message-circle-question-mark", 16., t.quiet)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S1))
                            .child(
                                div()
                                    .ty(UI)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.ink)
                                    .child("Ask on the side"),
                            )
                            .child(div().ty(UI).text_color(t.quiet).child(if started {
                                "Answered with everything said so far. Nothing here goes into the conversation."
                            } else {
                                "Once the session has replied, ask about it here without adding to it."
                            })),
                    ),
            );
        }
        for (i, a) in s.asides.iter().enumerate() {
            let _place = crate::select::item(area, i);
            let question = div().flex().justify_end().child(
                // Your question as your messages read in the chat.
                div()
                    .max_w(relative(0.85))
                    .px(px(space::S3 + space::HALF))
                    .py(px(space::S2))
                    .rounded(px(radius::LG + space::HALF))
                    .bg(t.tint())
                    .ty(READING)
                    .text_color(t.ink)
                    .child(a.question.clone()),
            );
            let answer = match &a.answer {
                None => div()
                    .ty(UI)
                    .text_color(t.quiet)
                    .child("Thinking…")
                    .into_any_element(),
                Some((text, true, _)) => div()
                    .flex()
                    .gap(px(space::S2))
                    .child(div().pt(px(3.)).child(icon("circle-alert", 13., t.needs.0)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .ty(UI)
                            .text_color(t.quiet)
                            .child(text.clone()),
                    )
                    .into_any_element(),
                Some((text, false, agent)) => {
                    let blocks = self.markdown(text);
                    let (sid, aid) = (s.id.clone(), a.id.clone());
                    let promote =
                        cx.listener(move |this, _, _, cx| this.promote_aside(&sid, &aid, cx));
                    let mut r = markdown::R {
                        live: false,
                        counted: false,
                        md: self.mdv.clone(),
                        owner: cx.entity_id(),
                        window: &mut *window,
                        cx: &mut *cx,
                        scope: format!("{}/aside-{}", s.id, a.id),
                        bg: t.panel,
                        base: Some(std::path::PathBuf::from(&s.cwd)),
                        session: Some((s.id.clone(), s.cwd.clone())),
                    };
                    let body = markdown::render(blocks.as_slice(), &t, false, &mut r);
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S2))
                        .child(body)
                        .when(agent.is_some(), |d| {
                            d.child(
                                div().flex().child(
                                    button(
                                        SharedString::from(format!("promote-{i}")),
                                        "Make it a fork",
                                        false,
                                        &t,
                                    )
                                    .relative()
                                    .with_tip(tip(
                                        "A new session that carries on from this answer",
                                        None,
                                        t,
                                    ))
                                    .on_press(promote)
                                    .child(probe(&self.probes, format!("promote-{i}"))),
                                ),
                            )
                        })
                        .into_any_element()
                }
            };
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .child(question)
                    .child(answer),
            );
        }
        let input =
            div()
                .flex_none()
                .p(px(space::S3))
                .border_t_1()
                .border_color(t.hairline)
                .child(
                    // The chat's reply box, smaller: the reading face, its
                    // corners and its surface.
                    div()
                        .px(px(space::S3))
                        .py(px(space::S2))
                        .rounded(px(radius::XL))
                        .bg(t.raised)
                        .border_1()
                        .border_color(t.hairline)
                        .ty(READING)
                        .text_color(t.ink)
                        .relative()
                        .flex()
                        .items_end()
                        .gap(px(space::S2))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Textarea::new(&self.aside_input)),
                        )
                        .child({
                            let ready = if self.aside_input.read(cx).value().trim().is_empty() {
                                Ready::No("Type a question first")
                            } else {
                                Ready::Yes
                            };
                            action(
                                div()
                                    .id("aside-send")
                                    .size(px(size::CONTROL))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(radius::MD))
                                    .when(ready == Ready::Yes, |d| {
                                        d.on_press(cx.listener(|this, _, window, cx| {
                                            this.ask_aside(window, cx)
                                        }))
                                        .with_tip(tip(
                                            "Ask",
                                            Some("↩"),
                                            t,
                                        ))
                                    }),
                                ready,
                                &t,
                                |d, fg| d.child(icon("arrow-up", 15., fg)),
                            )
                        })
                        .child(probe(&self.probes, "aside-input")),
                );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // The region stays over what's on screen, outside what scrolls.
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(list)
                    .child(region)
                    .child(probe(&self.probes, "asides")),
            )
            .child(input)
            .into_any_element()
    }
}

fn comment_note(line: u32, text: &str, t: &crate::theme::Theme) -> Div {
    div()
        .mx(px(space::S3))
        .my(px(space::S1))
        .px(px(space::S3))
        .py(px(space::S2))
        .rounded(px(radius::LG))
        .bg(t.raised)
        .border_1()
        .border_color(t.hairline)
        .flex()
        .gap(px(space::S2))
        .child(icon("message-square", 13., t.quiet))
        .child(
            div()
                .ty(LABEL)
                .text_color(t.quiet)
                .child(format!("line {line}")),
        )
        .child(
            div()
                .flex_1()
                .ty(UI)
                .text_color(t.ink)
                .child(text.to_string()),
        )
}

/// A changed file's size in lines, or what it is when it has none.
fn counts(f: &crate::data::FileDiff) -> String {
    if f.lines.is_empty() {
        match crate::data::Kind::of(&f.path) {
            Some(crate::data::Kind::Image) => "picture".into(),
            _ => "binary".into(),
        }
    } else {
        format!("+{} −{}", f.added, f.removed)
    }
}

/// A file size in words people use: "812 bytes", "2.4 KB", "3.1 MB".
pub fn bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} bytes"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.),
    }
}

impl Dent {
    /// A file's Discard: asks in place, then puts the file back as it was
    /// (before this turn, or at the last commit). Off, saying why, when
    /// what's shown can't be put back.
    fn discard_file(&self, s: &Session, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let asking = self
            .discarding
            .as_ref()
            .is_some_and(|(sid, p, h)| *sid == s.id && p == path && h.is_none());
        let (sid, p) = (s.id.clone(), path.to_string());
        if asking {
            return div()
                .flex()
                .items_center()
                .gap(px(space::S1))
                // The Mac order: the destructive choice leads, Cancel trails.
                .child(
                    crate::ui::button("discard-file-yes", "Discard changes", false, &t)
                        .relative()
                        .on_press(
                            cx.listener(move |this, _, _, cx| this.discard(&sid, &p, None, cx)),
                        )
                        .child(probe(&self.probes, "discard-file-yes")),
                )
                .child(
                    ghost("discard-file-cancel", "Cancel", &t).on_press(cx.listener(
                        |this, _, _, cx| {
                            this.discarding = None;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element();
        }
        let el = ghost("discard-file", "Discard", &t)
            .relative()
            .child(probe(&self.probes, "discard-file"));
        match self.can_discard(s) {
            Ok(()) => el
                .with_tip(crate::ui::tip_owned(
                    if self.changes_turn.contains_key(&s.id) {
                        "Put this file back as it was before this turn"
                    } else {
                        "Put this file back as it was at the last commit"
                    },
                    t,
                ))
                .on_press(cx.listener(move |this, _, _, cx| {
                    this.discarding = Some((sid.clone(), p.clone(), None));
                    cx.notify();
                }))
                .into_any_element(),
            Err(why) => crate::ui::off(el, why, &t).into_any_element(),
        }
    }
}

impl Dent {
    /// A hunk's Discard, at the right of what's in view on its header row:
    /// shown on hover, it asks in place, then puts just that hunk back.
    fn discard_hunk(
        &self,
        s: &Session,
        path: &str,
        hunk: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let asking = self
            .discarding
            .as_ref()
            .is_some_and(|(sid, p, h)| *sid == s.id && p == path && *h == Some(hunk));
        let (sid, p) = (s.id.clone(), path.to_string());
        let control = if asking {
            div()
                .flex()
                .items_center()
                .gap(px(space::S1))
                .child(
                    ghost(
                        SharedString::from(format!("discard-hunk-yes-{hunk}")),
                        "Discard this change",
                        &t,
                    )
                    .relative()
                    .text_color(t.ink)
                    .on_press(
                        cx.listener(move |this, _, _, cx| this.discard(&sid, &p, Some(hunk), cx)),
                    )
                    .child(probe(&self.probes, format!("discard-hunk-yes-{hunk}"))),
                )
                .child(
                    ghost(
                        SharedString::from(format!("discard-hunk-cancel-{hunk}")),
                        "Cancel",
                        &t,
                    )
                    .on_press(cx.listener(|this, _, _, cx| {
                        this.discarding = None;
                        cx.notify();
                    })),
                )
                .into_any_element()
        } else {
            match self.can_discard(s) {
                Ok(()) => ghost(
                    SharedString::from(format!("discard-hunk-{hunk}")),
                    "Discard",
                    &t,
                )
                .relative()
                .invisible()
                .group_hover("line", |d| d.visible())
                .with_tip(crate::ui::tip("Put this change back as it was", None, t))
                .on_press(cx.listener(move |this, _, _, cx| {
                    this.discarding = Some((sid.clone(), p.clone(), Some(hunk)));
                    cx.notify();
                }))
                .child(probe(&self.probes, format!("discard-hunk-{hunk}")))
                .into_any_element(),
                // Off: the file's Discard says why; nothing here.
                Err(_) => div().into_any_element(),
            }
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(self.diff_view_w))
            .h_full()
            .flex()
            .justify_end()
            .items_center()
            .pr(px(space::S2))
            .child(control)
            .into_any_element()
    }
}

/// The tab strip's own room besides its tabs: the panel toggle and padding.
const TAB_STRIP_CHROME: f32 = 56.;
/// A tab's room besides its name: icon, padding, the close button, the gap.
const TAB_CHROME: f32 = 56.;
/// About how wide a letter of a tab's name is.
const TAB_LETTER: f32 = 7.;

/// Each path's tab name: its file name, and when another has the same
/// name, as many of its folders as it takes to tell them apart
/// (`take.mp4 · clips`, `take.mp4 · raw`).
fn tab_names(paths: &[&str]) -> Vec<String> {
    let parts: Vec<Vec<&str>> = paths
        .iter()
        .map(|p| p.split('/').filter(|c| !c.is_empty()).rev().collect())
        .collect();
    let name = |i: usize| parts[i].first().copied().unwrap_or(paths[i]);
    (0..paths.len())
        .map(|i| {
            let same: Vec<usize> = (0..paths.len())
                .filter(|&j| j != i && name(j) == name(i) && paths[j] != paths[i])
                .collect();
            if same.is_empty() {
                return name(i).to_string();
            }
            // The fewest folders, nearest first, that no other of the same
            // name ends with too.
            let folders = &parts[i][1..];
            let n = (1..=folders.len())
                .find(|&n| {
                    same.iter()
                        .all(|&j| parts[j].get(1..=n) != Some(&folders[..n]))
                })
                .unwrap_or(folders.len());
            let dir: Vec<&str> = folders[..n].iter().rev().copied().collect();
            format!("{} · {}", name(i), dir.join("/"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::tab_names;

    #[test]
    fn two_files_of_one_name_are_told_apart_by_their_folders() {
        assert_eq!(
            tab_names(&["/w/clips/take.mp4", "/w/raw/take.mp4", "/w/notes.md"]),
            ["take.mp4 · clips", "take.mp4 · raw", "notes.md"]
        );
        // The same nearest folder: one more is shown.
        assert_eq!(
            tab_names(&["/a/out/x.png", "/b/out/x.png"]),
            ["x.png · a/out", "x.png · b/out"]
        );
        // One folder inside the other.
        assert_eq!(
            tab_names(&["/w/x.png", "/w/sub/x.png"]),
            ["x.png · w", "x.png · sub"]
        );
        assert_eq!(tab_names(&["/only.txt"]), ["only.txt"]);
    }
}
