//! The viewer: everything a session made or changed, beside the chat. Pages
//! run live, docs render, images show, changes take review comments.

use std::rc::Rc;

use gpui_kit::base::ElementExt;
use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::RevealOnHover;

use crate::app::{Dent, Tab, Viewport};
use crate::data::{Kind, Line, Session};
use crate::ds::{CODE, LABEL, Typed, UI, radius, size, space};
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
    pub fn has_viewer(s: &Session) -> bool {
        !s.arts.is_empty() || !s.diff.is_empty()
    }

    /// Whether the right pane has anything for this session: what it made,
    /// or a side chat, even one just opened.
    pub fn shows_panel(&self, s: &Session) -> bool {
        Self::has_viewer(s)
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
            && Self::has_viewer(s)
            && matches!(self.tab(s), Tab::Art(a) if self.art(a).is_some_and(|a| a.kind.is_page()) && !self.popouts.contains_key(a))
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
        for a in self.open_arts(s).into_iter().filter_map(|a| self.art(a)) {
            tabs.push((
                Tab::Art(a.id),
                if a.kind == Kind::Server {
                    a.title.trim_start_matches("Dev server · ").to_string()
                } else {
                    a.path.rsplit('/').next().unwrap_or(a.path).to_string()
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
        let header = div()
            .h(px(size::HEADER))
            .flex_none()
            .px(px(space::S2))
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
                        .w(px(space::S4))
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
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.show_tab(which, cx)),
                                    )
                                    .when_some(path, |d, p| d.tooltip(crate::ui::tip_owned(p, t)))
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
                                    .child(
                                        div()
                                            .ty(UI)
                                            .text_color(if on { t.ink } else { t.quiet })
                                            .child(middle_ellipsis(&name, 28)),
                                    )
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
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.close_tab(a, cx);
                                                }))
                                                .tooltip(tip("Close tab", None, t))
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
            });
        let body = match tab {
            Tab::Art(id) => match self.art(id).map(|a| a.kind) {
                Some(Kind::Html | Kind::Server) => self.page(id, window, cx),
                Some(Kind::Doc) => self.doc(id, window, cx),
                Some(Kind::Image) => self.image(id, window, cx),
                None => div().into_any_element(),
            },
            Tab::Changes => self.changes(s, window, cx),
            Tab::Terminal => self.shell_view(s, window, cx),
            Tab::Aside => self.side_chat(s, window, cx),
        };
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
                    .child(body)
                    // The viewer's text selects apart from the chat's.
                    .text_selection_scope(crate::select::viewer_scope()),
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
                    d.on_click(cx.listener(move |this, _, _, cx| {
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
        if self.popouts.contains_key(id) {
            return self.popped_out(id, &a, cx);
        }
        let latest = a.versions.len().max(1);
        let v = self.version(&a);
        let detached = self.preview.as_ref().is_some_and(|p| p.borrow().detached());
        if let Some(p) = &self.preview {
            let mut p = p.borrow_mut();
            if server {
                p.show_server(a.path);
            } else {
                p.show(a.path, v, &a.versions[v - 1], a.dir.as_deref());
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
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        this.viewport = vp;
                        cx.notify();
                    }))
                })
                .child(probe(&self.probes, format!("vp-{label}")))
        };
        let machine = self
            .current()
            .map(|s| s.machine.clone())
            .unwrap_or_default();
        let toolbar = self
            .toolbar()
            .when(server, |d| {
                d.pl(px(space::S3)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S1H))
                        .child(status_square(Status::Done, &t))
                        .child(div().ty(LABEL).text_color(t.ink).child("running")),
                )
            })
            .when(!server, |d| d.children(self.versions(&a, cx)))
            .child(div().w(px(1.)).h(px(16.)).mx(px(space::S1)).bg(t.hairline))
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
            .when(!detached && self.panel_w >= 460., |d| {
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
            .child(
                div()
                    .id("pick")
                    .relative()
                    .h(px(size::CONTROL))
                    .on_hover(Self::hint(
                        "Click an element on the page to point the agent at it",
                        cx,
                    ))
                    .px(px(space::S2))
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .when(self.picking, |d| d.bg(t.ink).text_color(t.surface))
                    .when(!self.picking, |d| {
                        d.text_color(t.ink).hover(|d| d.bg(t.hover()))
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_pick(cx)))
                    .child(icon(
                        "square-dashed-mouse-pointer",
                        14.,
                        if self.picking { t.surface } else { t.quiet },
                    ))
                    .child(div().ty(UI).child("Point"))
                    .child(probe(&self.probes, "pick")),
            )
            .child(
                icon_button("reload", "rotate-cw", &t)
                    .on_hover(Self::hint("Reload", cx))
                    .on_click(cx.listener(|this, _, _, cx| {
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
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.pop_out(id, window, cx)),
                        )
                        .child(probe(&self.probes, "pop-out")),
                )
            })
            .child({
                let server_url = a.path.to_string();
                // The latest version is the file itself, beside its styles
                // and pictures; an older one is written out on its own.
                let on_disk = (v == latest)
                    .then(|| art_file(&a))
                    .flatten()
                    .filter(|f| f.is_file());
                let file = (!server && on_disk.is_none()).then(|| {
                    (
                        a.versions[v - 1].clone(),
                        a.path.rsplit('/').next().unwrap_or(a.path).to_string(),
                    )
                });
                icon_button("external", "external-link", &t)
                    .on_hover(Self::hint("Open in your browser", cx))
                    .on_click(move |_, _, cx| match (&on_disk, &file) {
                        (Some(path), _) => cx.open_url(&format!("file://{}", path.display())),
                        (None, Some((bytes, name))) => {
                            let path =
                                std::env::temp_dir().join(format!("dent-preview-v{v}-{name}"));
                            if std::fs::write(&path, &bytes[..]).is_ok() {
                                cx.open_url(&format!("file://{}", path.display()));
                            }
                        }
                        (None, None) => cx.open_url(&server_url),
                    })
            });

        // The web view is a native layer over this area. GPUI draws the frame
        // and, underneath, the page's last snapshot: when a menu hides the
        // web view, the snapshot is what you see.
        let area_preview = self.preview.clone();
        let width = viewport.width().filter(|w| self.panel_w > w + space::S8);
        let area = if detached {
            self.own_window_note(cx)
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
                .child(icon("history", 13., t.quiet))
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
                        .on_click(cx.listener(move |this, _, _, cx| this.open_art(id, None, cx)))
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
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.ask_to_fix(id, v, window, cx)
                        }))
                        .child(probe(&self.probes, "fix")),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(toolbar)
            .children(banner)
            .children(hint)
            .child(area)
            .children(console)
            .into_any_element()
    }

    /// The viewer while the page is in a window of its own.
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
                            .child(ghost("show-popout", "Show window", &t).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.pop_out(id, window, cx)
                                }),
                            ))
                            .child(
                                button("bring-back", "Bring it back", false, &t)
                                    .relative()
                                    .on_click(
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
                ghost("raise-page", "Show the page", &t).on_click(cx.listener(|this, _, _, _| {
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
            md: self.mdv.clone(),
            owner: cx.entity_id(),
            window,
            cx,
            scope: format!("doc/{id}"),
            bg: t.panel,
            // Links and pictures in a document are found from its folder.
            base: art_file(&a).and_then(|f| f.parent().map(|p| p.to_path_buf())),
        };
        let page = markdown::render(blocks.as_slice(), &t, true, &mut r);
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
                            .tooltip(tip("Copy the file's text", None, t))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let src = String::from_utf8_lossy(&bytes).into_owned();
                                cx.write_to_clipboard(ClipboardItem::new_string(src));
                                this.copied_at = Some(std::time::Instant::now());
                                cx.notify();
                            }))
                    })
                    .when_some(art_file(&a), |d, full| {
                        d.child(
                            ghost("open-doc", "Open", &t)
                                .on_click(move |_, _, cx| {
                                    cx.open_url(&format!("file://{}", full.display()))
                                })
                                .tooltip(tip("Open in its app", None, t)),
                        )
                    }),
            )
            .child(
                // Each document keeps its own place.
                div()
                    .id(SharedString::from(format!("doc-{id}")))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .max_w(px(680.))
                            .mx_auto()
                            .px(px(space::S8))
                            .py(px(space::S6))
                            .child(page),
                    ),
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
                .on_click(cx.listener(move |this, _, _, cx| {
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

    fn changes(&mut self, s: &Session, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let selected = self
            .diff_file
            .get(&s.id)
            .copied()
            .unwrap_or(0)
            .min(s.diff.len().saturating_sub(1));
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
                    .on_click(cx.listener(move |this, _, _, cx| {
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
        let diff = list(
            self.diff_list.clone(),
            cx.processor(move |this, ix: usize, _, cx| {
                this.diff_row(&shared, selected, &rows, &comments, ix, cx)
            }),
        )
        .size_full();
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
                        .on_click(cx.listener(|this, _, window, cx| this.send_review(window, cx)))
                        .child(probe(&self.probes, "send-review")),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
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
                                                    .on_click(cx.listener(
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
                                                    .on_click(cx.listener(
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
                                        ghost("open-file", "Open", &t)
                                            .on_click(move |_, _, cx| {
                                                cx.open_url(&format!("file://{}", full.display()))
                                            })
                                            .tooltip(tip("Open in its app", None, t))
                                    })
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
                                        let full = std::path::Path::new(&s.cwd).join(&file.path);
                                        let picture = crate::data::Kind::of(&file.path)
                                            == Some(crate::data::Kind::Image)
                                            && full.exists();
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
                                                let size = std::fs::metadata(&full)
                                                    .map(|m| bytes(m.len()))
                                                    .ok();
                                                let what =
                                                    if picture { "Picture" } else { "Binary file" };
                                                match size {
                                                    Some(b) => format!("{what} · {b}"),
                                                    None => what.to_string(),
                                                }
                                            }))
                                            .into_any_element()
                                    } else {
                                        div()
                                            .id("diff")
                                            .size_full()
                                            .overflow_x_scroll()
                                            .child(
                                                div()
                                                    .h_full()
                                                    .min_w_full()
                                                    .w(px(wide))
                                                    .py(px(space::S1))
                                                    .child(diff),
                                            )
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

    /// The advance of one character of code, measured once.
    fn code_advance(&mut self, window: &mut Window) -> f32 {
        if self.code_em == 0. {
            let font = font(crate::theme::mono());
            let id = window.text_system().resolve_font(&font);
            self.code_em = window
                .text_system()
                .em_advance(id, px(CODE.size))
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
                                    .on_click(cx.listener(move |this, _, window, cx| {
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
                                .into_any_element()
                            }),
                    )
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
                                    .child(ghost("cancel-comment", "Cancel", &t).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.commenting = None;
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        button("add-comment", "Add comment", true, &t)
                                            .relative()
                                            .on_click(cx.listener(|this, _, window, cx| {
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
        let mut list = div()
            .id("asides")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
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
            let question = div().flex().justify_end().child(
                div()
                    .max_w(relative(0.85))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .rounded(px(radius::LG))
                    .bg(t.raised)
                    .border_1()
                    .border_color(t.hairline)
                    .ty(UI)
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
                        md: self.mdv.clone(),
                        owner: cx.entity_id(),
                        window: &mut *window,
                        cx: &mut *cx,
                        scope: format!("{}/aside-{}", s.id, a.id),
                        bg: t.panel,
                        base: Some(std::path::PathBuf::from(&s.cwd)),
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
                                    .tooltip(tip(
                                        "A new session that carries on from this answer",
                                        None,
                                        t,
                                    ))
                                    .on_click(promote)
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
                    div()
                        .px(px(space::S3))
                        .py(px(space::S2))
                        .rounded(px(radius::LG))
                        .bg(t.raised)
                        .border_1()
                        .border_color(t.hairline)
                        .ty(UI)
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
                                        d.on_click(cx.listener(|this, _, window, cx| {
                                            this.ask_aside(window, cx)
                                        }))
                                        .tooltip(tip(
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
            .child(list)
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
fn bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} bytes"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.),
    }
}
