//! The viewer: everything a session made or changed, beside the chat. Pages
//! run live, docs render, images show, changes take review comments.

use std::sync::Arc;

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::{Dent, Tab, Viewport};
use crate::data::{Kind, Line, Session, Tone};
use crate::ds::{CODE, LABEL, Typed, UI, radius, size, space};
use crate::markdown;
use crate::theme::{Status, status_square};
use crate::ui::{button, ghost, icon, icon_button, kbd, probe};

pub const MIN_WIDTH: f32 = 360.;
/// The chat never gets narrower than this; the viewer and then the sidebar give way.
pub const CHAT_MIN: f32 = 440.;

impl Dent {
    pub fn has_viewer(s: &Session) -> bool {
        !s.arts.is_empty() || !s.diff.is_empty() || !s.terminal.is_empty()
    }

    /// Whether the live page should be on screen this frame.
    pub fn preview_visible(&self) -> bool {
        let Some(s) = self.current() else {
            return false;
        };
        self.panel_open
            && Self::has_viewer(s)
            && matches!(self.tab(s), Tab::Art(a) if self.art(a).is_some_and(|a| a.kind == Kind::Html))
            && self.menu.is_none()
            && self.palette.is_none()
    }

    pub fn panel(
        &mut self,
        s: &Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let tab = self.tab(s);
        let mut tabs: Vec<(Tab, String, &'static str)> = s
            .arts
            .iter()
            .filter_map(|a| self.art(a))
            .map(|a| {
                (
                    Tab::Art(a.id),
                    a.path.rsplit('/').next().unwrap_or(a.path).to_string(),
                    a.kind.icon(),
                )
            })
            .collect();
        if !s.diff.is_empty() {
            tabs.push((
                Tab::Changes,
                format!("Changes {}", s.diff.len()),
                "git-compare",
            ));
        }
        if !s.terminal.is_empty() {
            tabs.push((Tab::Terminal, "Terminal".into(), "square-terminal"));
        }
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
            .child(
                div()
                    .id("tabs")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(space::S1))
                    .overflow_x_scroll()
                    .children(tabs.into_iter().map(|(which, name, ic)| {
                        let on = which == tab;
                        let key = match which {
                            Tab::Art(a) => format!("tab-{a}"),
                            Tab::Changes => "tab-changes".into(),
                            Tab::Terminal => "tab-terminal".into(),
                        };
                        div()
                            .id(SharedString::from(key.clone()))
                            .relative()
                            .flex_none()
                            .h(px(size::CONTROL))
                            .px(px(space::S2))
                            .flex()
                            .items_center()
                            .gap(px(space::S1H))
                            .rounded(px(radius::MD))
                            .cursor_pointer()
                            .when(on, |d| d.bg(t.selected()))
                            .when(!on, |d| d.hover(|d| d.bg(t.hover())))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| this.show_tab(which, cx)))
                            .child(icon(ic, 13., if on { t.ink } else { t.quiet }))
                            .child(
                                div()
                                    .ty(UI)
                                    .text_color(if on { t.ink } else { t.quiet })
                                    .child(name),
                            )
                            .child(probe(&self.probes, key))
                    })),
            );
        let body = match tab {
            Tab::Art(id) => match self.art(id).map(|a| a.kind) {
                Some(Kind::Html) => self.page(id, window, cx),
                Some(Kind::Doc) => self.doc(id, cx),
                Some(Kind::Image) => self.image(id, cx),
                None => div().into_any_element(),
            },
            Tab::Changes => self.changes(s, cx),
            Tab::Terminal => self.terminal(s),
        };
        div()
            .relative()
            .w(px(self.panel_w))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .border_l_1()
            .border_color(t.hairline)
            .child(header)
            .child(body)
            // The left edge resizes the panel.
            .child(
                div()
                    .id("resize")
                    .absolute()
                    .left(px(-3.))
                    .top_0()
                    .bottom_0()
                    .w(px(6.))
                    .cursor_col_resize()
                    .hover(|d| d.bg(t.hairline))
                    .when(self.dragging, |d| d.bg(t.hairline))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.dragging = true;
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
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
        let latest = a.versions.len();
        let v = self.version(&a);
        if let Some(p) = &self.preview {
            p.borrow_mut().show(a.path, v, &a.versions[v - 1]);
        }
        let errors = self.errors.get(&(id, v)).cloned().unwrap_or_default();
        let updated = self
            .updated_at
            .get(id)
            .map(|at| at.elapsed().as_secs_f32())
            .filter(|s| *s < 2.5);
        let viewport = self.viewport;
        let seg = |vp: Viewport,
                   name: &'static str,
                   label: &'static str,
                   says: &'static str,
                   cx: &mut Context<Self>| {
            icon_button(format!("vp-{label}"), name, &t)
                .relative()
                .on_hover(Self::hint(says, cx))
                .when(viewport == vp, |d| d.bg(t.selected()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.viewport = vp;
                    cx.notify();
                }))
                .child(probe(&self.probes, format!("vp-{label}")))
        };
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
                .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(t.hover())))
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
        let toolbar = self
            .toolbar()
            .child(step(
                "prev-version",
                "chevron-left",
                (v > 1).then(|| v - 1),
                "Earlier version",
                cx,
            ))
            .child(
                div()
                    .ty(LABEL)
                    .text_color(t.ink)
                    .child(format!("v{v}"))
                    .child(div().text_color(t.faint).child(format!(" of {latest}")))
                    .flex(),
            )
            .child(step(
                "next-version",
                "chevron-right",
                (v < latest).then(|| v + 1),
                "Later version",
                cx,
            ))
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
                    .child(a.path),
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
            .child(seg(Viewport::Fill, "monitor", "fill", "Full width", cx))
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
            .child({
                let (bytes, name) = (
                    a.versions[v - 1].clone(),
                    a.path.rsplit('/').next().unwrap_or(a.path).to_string(),
                );
                icon_button("external", "external-link", &t)
                    .on_hover(Self::hint("Open in your browser", cx))
                    .on_click(move |_, _, cx| {
                        let path = std::env::temp_dir().join(format!("dent-preview-v{v}-{name}"));
                        if std::fs::write(&path, &bytes[..]).is_ok() {
                            cx.open_url(&format!("file://{}", path.display()));
                        }
                    })
            });

        // The web view is a native layer over this area; GPUI draws its frame.
        let area_preview = self.preview.clone();
        let width = viewport.width();
        let area = div()
            .flex_1()
            .min_h_0()
            .relative()
            .bg(if width.is_some() { t.surface } else { t.panel })
            .child(
                canvas(
                    move |bounds, _, _| {
                        let inner = match width {
                            Some(w) if bounds.size.width.as_f32() > w + space::S8 => Bounds {
                                origin: point(
                                    bounds.origin.x + (bounds.size.width - px(w)) / 2.,
                                    bounds.origin.y + px(space::S4),
                                ),
                                size: size(px(w), bounds.size.height - px(space::S4 * 2.)),
                            },
                            _ => bounds,
                        };
                        if let Some(p) = &area_preview {
                            p.borrow_mut().place(inner);
                        }
                        inner
                    },
                    move |_, inner, window, _| {
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
            )
            .child(probe(&self.probes, "preview"));

        let banner = (v < latest).then(|| {
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

    fn doc(&mut self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let Some(a) = self.art(id).cloned() else {
            return div().into_any_element();
        };
        let src = String::from_utf8_lossy(&a.versions[a.versions.len() - 1]).to_string();
        let blocks = markdown::parse(&src);
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
                            .child(a.path),
                    )
                    .child({
                        let copied = self
                            .copied_at
                            .is_some_and(|at| at.elapsed().as_secs_f32() < 1.5);
                        let src = src.clone();
                        ghost("copy-doc", if copied { "Copied" } else { "Copy" }, &t).on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(src.clone()));
                                this.copied_at = Some(std::time::Instant::now());
                                cx.notify();
                            }),
                        )
                    }),
            )
            .child(
                div()
                    .id("doc")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .max_w(px(680.))
                            .mx_auto()
                            .px(px(space::S8))
                            .py(px(space::S6))
                            .child(markdown::render(&blocks, &t, true)),
                    ),
            )
            .into_any_element()
    }

    fn image(&mut self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let Some(a) = self.art(id).cloned() else {
            return div().into_any_element();
        };
        let bytes = a.versions[a.versions.len() - 1].clone();
        // PNG size from the IHDR chunk.
        let dim = |at: usize| {
            bytes
                .get(at..at + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                .unwrap_or(0)
        };
        let (w, h) = (dim(16), dim(20));
        let actual = self.image_actual;
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(a.path),
                    )
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.faint)
                            .child(format!("{w} × {h}")),
                    )
                    .child(seg("img-fit", "Fit", !actual, false, cx))
                    .child(seg("img-full", "Actual size", actual, true, cx)),
            )
            .child(
                div()
                    .id("image")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .p(px(space::S6))
                    .flex()
                    .when(!actual, |d| d.items_center().justify_center())
                    .bg(t.surface)
                    .child(
                        img(Arc::new(Image::from_bytes(
                            ImageFormat::Png,
                            bytes.to_vec(),
                        )))
                        .map(|i| {
                            if actual {
                                // Screenshots are @2x: actual size is half the pixels.
                                i.flex_none().w(px(w as f32 / 2.)).h(px(h as f32 / 2.))
                            } else {
                                i.max_w_full().max_h_full()
                            }
                        })
                        .object_fit(ObjectFit::Contain)
                        .rounded(px(radius::LG))
                        .border_1()
                        .border_color(t.hairline),
                    ),
            )
            .into_any_element()
    }

    fn changes(&mut self, s: &Session, cx: &mut Context<Self>) -> AnyElement {
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
                            .child(
                                div()
                                    .ty(LABEL)
                                    .text_color(t.faint)
                                    .child(format!("+{} −{}", f.added, f.removed)),
                            ),
                    )
                    .child(div().truncate().ty(LABEL).text_color(t.faint).child(dir))
            }));
        let Some(file) = s.diff.get(selected).cloned() else {
            return div().into_any_element();
        };
        let commenting = self.commenting.clone();
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
        let mut lines: Vec<AnyElement> = Vec::new();
        for (i, l) in file.lines.iter().enumerate() {
            let n = l.new.or(l.old).unwrap_or(0);
            let (bg, bar) = match l.kind {
                Line::Added => (Some(t.added), Some(t.done.0)),
                Line::Removed => (Some(t.removed), Some(t.quiet)),
                Line::Hunk => (Some(t.selected()), None),
                Line::Context => (None, None),
            };
            let path = file.path.clone();
            let commentable = l.kind != Line::Hunk;
            lines.push(
                div()
                    .id(SharedString::from(format!("line-{i}")))
                    .group("line")
                    .relative()
                    .flex()
                    .min_w_full()
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
                            .child(l.text.clone()),
                    )
                    .into_any_element(),
            );
            for (line, text) in comments
                .iter()
                .filter(|(line, _)| *line == n && commentable)
            {
                lines.push(comment_note(*line, text, &t).into_any_element());
            }
            if commenting
                .as_ref()
                .is_some_and(|(f, line)| *f == file.path && *line == n && commentable)
            {
                lines.push(
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
                                .child(ghost("cancel-comment", "Cancel", &t).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.commenting = None;
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    button("add-comment", "Add comment", true, &t)
                                        .relative()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.add_comment(window, cx)
                                        }))
                                        .child(probe(&self.probes, "add-comment")),
                                ),
                        )
                        .into_any_element(),
                );
            }
        }
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
                                    .child(div().ty(LABEL).text_color(t.faint).child(if compact {
                                        format!(
                                            "{} of {count} · +{} −{}",
                                            selected + 1,
                                            file.added,
                                            file.removed
                                        )
                                    } else {
                                        format!("+{} −{}", file.added, file.removed)
                                    })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .relative()
                                    .child(
                                        div()
                                            .id("diff")
                                            .size_full()
                                            .overflow_scroll()
                                            .py(px(space::S1))
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .min_w_full()
                                                    .children(lines),
                                            ),
                                    )
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

    fn terminal(&mut self, s: &Session) -> AnyElement {
        let t = self.t;
        div()
            .id("terminal")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(space::S4))
            .flex()
            .flex_col()
            .children(s.terminal.iter().map(|(tone, line)| {
                let mark = (*tone == Tone::Good).then(|| status_square(Status::Done, &t));
                div()
                    .h(px(size::LINE))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(div().w(px(12.)).flex_none().children(mark))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .ty(CODE)
                            .text_color(match tone {
                                Tone::Quiet => t.quiet,
                                _ => t.ink,
                            })
                            .when(*tone == Tone::Command, |d| {
                                d.font_weight(FontWeight::MEDIUM)
                            })
                            .child(line.clone()),
                    )
            }))
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
