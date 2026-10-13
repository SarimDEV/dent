//! Desktop concepts: the session inbox, starting a session by typing, and the
//! viewer (live pages, docs, images, changes with review comments) in GPUI,
//! with fake agents. Nothing here reaches a real agent or the network.
//!
//! `cargo run -p spike-desktop-concepts -- [--dark] [--size WxH] [--pace N]
//!   [--reduce-motion] [--demo tour|checks] [--shots DIR] [--record FILE.mov]`

mod anim;
mod app;
mod chat;
mod data;
mod demo;
mod ds;
mod markdown;
mod palette;
mod sidebar;
mod theme;
mod ui;
mod viewer;
mod web;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::base::input::{InputEditorStyle, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use app::{
    CloseSession, Dent, Escape, InboxClose, InboxDown, InboxOpen, InboxUp, NewSession,
    NextNeedsYou, Search, SelectNext, SelectPrev, ShowChanges, ShowFirst, ShowTerminal,
    StopSession, Tab, ToastAction, TogglePanel, UndoClose,
};
use ds::{Typed, UI, radius, size, space};
use theme::Theme;

pub static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static REDUCE_MOTION: AtomicBool = AtomicBool::new(false);
pub static WINDOW: OnceLock<(f32, f32, f32, f32)> = OnceLock::new();

pub fn reduce_motion() -> bool {
    REDUCE_MOTION.load(Ordering::Relaxed)
}

pub fn window_rect() -> (f32, f32, f32, f32) {
    WINDOW.get().copied().unwrap_or((40., 40., 1440., 860.))
}

/// Inputs take the page's colours: ink text, quiet placeholder, ink caret.
pub fn style_input(input: &Entity<TextareaState>, t: &Theme, cx: &mut App) {
    let style = InputEditorStyle {
        foreground: t.ink,
        muted_foreground: t.faint,
        background: t.raised.opacity(0.),
        border: t.hairline.opacity(0.),
        selection: t.ink.opacity(if t.dark { 0.28 } else { 0.16 }),
        caret: t.ink,
        ..Default::default()
    };
    input.update(cx, |i, _| i.set_editor_style(style));
}

impl Render for Dent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dt = self.last_frame.elapsed();
        self.last_frame = Instant::now();
        let _ = dt;
        self.drain_page(window, cx);
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed() > Duration::from_secs(8))
        {
            self.toast = None;
        }
        if self.demo.is_some() {
            let me = cx.entity();
            window.defer(cx, move |window, cx| demo::tick(me, window, cx));
        }
        if self.animating() || self.toast.is_some() || self.demo.is_some() {
            window.request_animation_frame();
        }
        // The composer says what Enter will do in this session's state.
        let want = if self
            .current()
            .is_some_and(|s| s.status == theme::Status::Working)
        {
            "Queue a message, or steer with ⌘↩"
        } else {
            "Reply"
        };
        if self.placeholder != want {
            self.placeholder = want;
            self.composer
                .update(cx, |c, cx| c.set_placeholder(want, window, cx));
        }
        self.inbox_focused = self.inbox_focus.is_focused(window);
        let visible = self.preview_visible();
        if let Some(p) = &self.preview {
            p.borrow_mut().set_visible(visible);
        }
        let t = self.t;
        // Lay out from the window: the chat keeps its minimum, the viewer
        // shrinks first, and only then does the sidebar fold away.
        let w = window.viewport_size().width.as_f32();
        let wants_panel = self.panel_open && self.current().is_some_and(Self::has_viewer);
        if !wants_panel {
            self.sidebar_forced = false;
        }
        let room = w - sidebar::WIDTH - viewer::CHAT_MIN;
        self.sidebar_hidden = wants_panel && room < viewer::MIN_WIDTH && !self.sidebar_forced;
        let side = if self.sidebar_hidden {
            0.
        } else {
            sidebar::WIDTH
        };
        self.panel_w = self
            .panel_width
            .min(w - side - viewer::CHAT_MIN)
            .max(viewer::MIN_WIDTH);
        let sidebar = (!self.sidebar_hidden).then(|| self.sidebar(cx));
        let main = self.main_column(window, cx);
        let panel = self
            .current()
            .cloned()
            .filter(|s| self.panel_open && Self::has_viewer(s))
            .map(|s| self.panel(&s, window, cx));
        let toast =
            self.toast.clone().map(|toast| {
                let label = match toast.action {
                    ToastAction::Undo(_) => "Undo",
                    ToastAction::Resume(_) => "Resume",
                };
                div()
                    .absolute()
                    .bottom(px(size::STATUS + space::S2))
                    .w(px(sidebar::WIDTH - space::S4))
                    .map(|d| {
                        if self.sidebar_hidden {
                            d.left(px(space::S4))
                        } else {
                            d.left(px(space::S2))
                        }
                    })
                    .child(
                        ui::floating(&t)
                            .rounded(px(radius::LG))
                            .pl(px(space::S3))
                            .pr(px(space::S1))
                            .h(px(40.))
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(ui::icon(
                                match toast.action {
                                    ToastAction::Undo(_) => "archive",
                                    ToastAction::Resume(_) => "square",
                                },
                                14.,
                                t.quiet,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .ty(UI)
                                    .text_color(t.ink)
                                    .child(toast.text.clone()),
                            )
                            .when(matches!(toast.action, ToastAction::Undo(_)), |d| {
                                d.child(ui::kbd("⌘Z", &t))
                            })
                            .child(
                                ui::button("toast-action", label, false, &t)
                                    .relative()
                                    .border_0()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toast_action(window, cx)
                                    }))
                                    .child(ui::probe(&self.probes, "toast-action")),
                            ),
                    )
            });
        let menu_open = self.menu.is_some();
        div()
            .id("root")
            .key_context("Dent")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .text_color(t.ink)
            .ty(UI)
            .on_action(
                cx.listener(|this, _: &NewSession, window, cx| this.select(None, window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseSession, window, cx| {
                if let Some(id) = this.selected.clone() {
                    this.close(&id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &UndoClose, window, cx| {
                if this
                    .toast
                    .as_ref()
                    .is_some_and(|t| matches!(t.action, ToastAction::Undo(_)))
                {
                    this.toast_action(window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &NextNeedsYou, window, cx| this.next_needs_you(window, cx)),
            )
            .on_action(cx.listener(|this, _: &TogglePanel, _, cx| {
                this.panel_open = !this.panel_open;
                this.picking = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowFirst, _, cx| {
                if let Some(a) = this.current().and_then(|s| s.arts.first().copied()) {
                    this.show_tab(Tab::Art(a), cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowChanges, _, cx| this.show_tab(Tab::Changes, cx)))
            .on_action(
                cx.listener(|this, _: &ShowTerminal, _, cx| this.show_tab(Tab::Terminal, cx)),
            )
            .on_action(cx.listener(|this, _: &StopSession, _, cx| {
                if let Some(id) = this.selected.clone() {
                    this.stop(&id, cx);
                }
            }))
            // Esc reaches the app before a text field swallows it.
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if k.key == "escape" && !k.modifiers.modified() {
                    window.dispatch_action(Box::new(Escape), cx);
                    cx.stop_propagation();
                    let _ = this;
                }
            }))
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                // Esc backs out one thing at a time; with nothing open it moves
                // to the inbox, where arrows, Enter and E work.
                if this.palette.is_some() {
                    this.close_palette(window, cx);
                } else if this.menu.take().is_some() {
                } else if this.picking {
                    this.toggle_pick(cx);
                } else if this.commenting.take().is_some() {
                } else {
                    window.focus(&this.inbox_focus, cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Search, window, cx| this.open_palette(window, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| {
                this.step_selection(-1, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                this.step_selection(1, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InboxUp, window, cx| {
                this.step_selection(-1, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InboxDown, window, cx| {
                this.step_selection(1, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InboxOpen, window, cx| {
                this.composer.update(cx, |c, cx| c.focus(window, cx))
            }))
            .on_action(cx.listener(|this, _: &InboxClose, window, cx| {
                if let Some(id) = this.selected.clone() {
                    this.close(&id, window, cx);
                    window.focus(&this.inbox_focus, cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                if this.dragging {
                    let w = window.viewport_size().width.as_f32();
                    let side = if this.sidebar_hidden {
                        0.
                    } else {
                        sidebar::WIDTH
                    };
                    let max = (w - side - viewer::CHAT_MIN).max(viewer::MIN_WIDTH);
                    this.panel_width = (w - e.position.x.as_f32()).clamp(viewer::MIN_WIDTH, max);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging {
                        this.dragging = false;
                        cx.notify();
                    }
                }),
            )
            // A drag released outside the window still ends.
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging {
                        this.dragging = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .children(sidebar)
                    .child(main)
                    .children(panel),
            )
            .child(self.status_bar(cx))
            .children(toast)
            // Clicking anywhere outside an open menu closes it.
            .when(menu_open, |d| {
                d.child(
                    div()
                        .id("menu-scrim")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                cx.notify();
                            }),
                        ),
                )
            })
            .when(self.dragging, |d| d.cursor_col_resize())
            .children(self.palette.map(|at| self.palette_view(at, cx)))
    }
}

fn load_fonts(cx: &mut App) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut fonts = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "ttf")
                && let Ok(bytes) = std::fs::read(&p)
            {
                fonts.push(std::borrow::Cow::Owned(bytes));
            }
        }
    }
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("fonts: {e}");
    }
}

struct Opts {
    dark: bool,
    pace: f32,
    demo: Option<String>,
    shots: Option<PathBuf>,
    record: Option<PathBuf>,
    size: (f32, f32),
}

fn opts() -> Opts {
    let mut o = Opts {
        dark: false,
        pace: 1.,
        demo: None,
        shots: None,
        record: None,
        size: (1440., 860.),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dark" => o.dark = true,
            "--reduce-motion" => REDUCE_MOTION.store(true, Ordering::Relaxed),
            "--pace" => o.pace = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.),
            "--demo" => o.demo = it.next(),
            "--shots" => o.shots = it.next().map(Into::into),
            "--record" => o.record = it.next().map(Into::into),
            "--size" => {
                if let Some((w, h)) = it.next().and_then(|v| {
                    let (w, h) = v.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                }) {
                    o.size = (w, h);
                }
            }
            other => eprintln!("ignoring unknown flag {other}"),
        }
    }
    o
}

fn main() {
    PROCESS_START.get_or_init(Instant::now);
    let o = opts();
    let _ = WINDOW.set((40., 40., o.size.0, o.size.1));
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            load_fonts(cx);
            app::bind_keys(cx);
            let (x, y, w, h) = window_rect();
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(x), px(y)),
                    size: size(px(w), px(h)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("dent".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                focus: true,
                ..Default::default()
            };
            let (dark, pace) = (o.dark, o.pace);
            let demo = o
                .demo
                .clone()
                .map(|name| demo::Demo::new(&name, o.shots.clone(), o.record.is_some()));
            let length = demo.as_ref().map(|d| d.length());
            let record = o.record.clone();
            let (_, view) = gpui_kit::open_window(options, cx, move |window, cx| {
                let view = cx.new(|cx| Dent::new(dark, pace, demo, window, cx));
                let composer = view.read(cx).composer.clone();
                composer.update(cx, |c, cx| c.focus(window, cx));
                view
            })
            .expect("open window");
            if let (Some(path), Some(secs)) = (record, length) {
                demo::record(path, secs);
            }
            // The working squares step at 8 fps, and the timers tick, even when
            // nothing else changes.
            let weak = view.downgrade();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs_f32(1. / ds::motion::PIXEL_FPS))
                        .await;
                    if weak.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
        });
}
