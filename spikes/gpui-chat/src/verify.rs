//! `--verify`: drive the stress page with real input events, dispatched
//! through GPUI's normal hit-testing, and check what happened. Wheel events
//! go to the capped table, the wide table and the side panel; clicks go to
//! the copy buttons; the clipboard is read back and restored afterwards.

use std::collections::HashMap;
use std::time::Instant;

use gpui_kit::*;

use super::Specimen;

#[derive(Default)]
pub struct Verify {
    step: usize,
    at: Option<Instant>,
    /// Extra wait before the next step, for input still being sent.
    wait: f32,
    results: Vec<(String, bool, String)>,
    snap: HashMap<&'static str, f32>,
    saved_clipboard: Option<Option<ClipboardItem>>,
    /// The capped table's height on each frame while its rows stream in.
    heights: Vec<f32>,
    tall: usize,
    wide: usize,
    code_key: String,
}

fn center(b: Bounds<Pixels>) -> Point<Pixels> {
    b.center()
}

/// Wheel input the way a trackpad sends it: one event per frame, a
/// started phase, moves, then an end. Returns how long it takes.
fn wheel(window: &Window, at: Point<Pixels>, dx: f32, dy: f32, times: usize) -> f32 {
    wheel_frame(window, at, dx, dy, 0, times);
    times as f32 / 40. + 0.2
}

fn wheel_frame(window: &Window, at: Point<Pixels>, dx: f32, dy: f32, i: usize, n: usize) {
    window.on_next_frame(move |window, cx| {
        let send = |window: &mut Window, cx: &mut App, d: Point<Pixels>, phase| {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: at,
                    delta: ScrollDelta::Pixels(d),
                    modifiers: Modifiers::default(),
                    touch_phase: phase,
                }),
                cx,
            );
        };
        let phase = if i == 0 {
            TouchPhase::Started
        } else {
            TouchPhase::Moved
        };
        send(window, cx, point(px(dx), px(dy)), phase);
        if i + 1 < n {
            wheel_frame(window, at, dx, dy, i + 1, n);
        } else {
            send(window, cx, point(px(0.), px(0.)), TouchPhase::Ended);
        }
    });
}

fn click(window: &Window, at: Point<Pixels>) {
    window.on_next_frame(move |window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
    });
}

impl Verify {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        self.results.push((name.to_string(), ok, detail));
    }
}

impl Specimen {
    fn page_y(&self) -> f32 {
        -self.scroll.offset().y.as_f32()
    }

    fn list_top(&self, key: usize) -> f32 {
        self.table_lists
            .get(&key)
            .map_or(0., |s| -s.scroll_px_offset_for_scrollbar().y.as_f32())
    }

    fn list_max(&self, key: usize) -> f32 {
        self.table_lists
            .get(&key)
            .map_or(0., |s| s.max_offset_for_scrollbar().y.as_f32())
    }

    fn hscroll_x(&self, key: usize) -> f32 {
        self.hscrolls
            .get(&(key, 0))
            .map_or(0., |h| -h.offset().x.as_f32())
    }

    fn probe_at(&self, name: &str) -> Option<Point<Pixels>> {
        self.probes.borrow().get(name).copied().map(center)
    }

    fn shot(&self, name: &str) {
        if let Some(dir) = &self.shots {
            let _ = std::fs::create_dir_all(dir);
            crate::demo::still(dir.join(format!("04-verify-{name}.png")));
        }
    }

    /// Called every frame in `--verify` mode.
    pub fn verify_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut v) = self.verify.take() else {
            return;
        };
        // While streaming: the capped table's height, every frame.
        if self.done_at.is_none()
            && let Some((&key, _)) = self.tables.iter().find(|(_, d)| d.capped())
            && let Some(b) = self.probes.borrow().get(&format!("table-{key}"))
        {
            v.heights.push(b.size.height.as_f32());
        }
        let settled = self
            .done_at
            .is_some_and(|d| d.elapsed().as_secs_f32() > 0.8)
            && !self
                .diagrams
                .values()
                .any(|d| matches!(d, super::Diagram::Pending));
        let ready =
            v.at.is_none_or(|at| at.elapsed().as_secs_f32() > 0.3 + v.wait);
        if settled && ready {
            v.wait = 0.;
            self.step(&mut v, window, cx);
            v.at = Some(Instant::now());
            v.step += 1;
        }
        self.verify = Some(v);
    }

    fn step(&mut self, v: &mut Verify, window: &mut Window, cx: &mut Context<Self>) {
        match v.step {
            0 => {
                v.tall = self
                    .tables
                    .iter()
                    .filter(|(_, d)| d.capped())
                    .map(|(k, _)| *k)
                    .min()
                    .unwrap_or(0);
                v.wide = self
                    .tables
                    .iter()
                    .filter(|(_, d)| d.widths.len() >= 10)
                    .map(|(k, _)| *k)
                    .min()
                    .unwrap_or(0);
                v.saved_clipboard = Some(cx.read_from_clipboard());
                self.scroll.scroll_to_item(v.tall);
            }
            1 => {
                self.shot("table-rest");
                v.snap.insert("page", self.page_y());
                v.snap.insert("top", self.list_top(v.tall));
                if let Some(at) = self.probe_at(&format!("table-body-{}", v.tall)) {
                    v.wait = wheel(window, at, 0., -40., 3);
                }
            }
            2 => {
                let moved = self.list_top(v.tall) - v.snap["top"];
                let page = self.page_y() - v.snap["page"];
                v.check(
                    "Wheel over a capped table scrolls the table, not the chat",
                    moved > 100. && page.abs() < 1.,
                    format!("table moved {moved:.0} pt, chat moved {page:.0} pt"),
                );
                v.snap.insert("page", self.page_y());
                // Start near the end so the handoff is tested at any table size.
                if let Some(state) = self.table_lists.get(&v.tall) {
                    let n = state.item_count();
                    state.scroll_to(ListOffset {
                        item_ix: n.saturating_sub(16),
                        offset_in_item: px(0.),
                    });
                }
                if let Some(at) = self.probe_at(&format!("table-body-{}", v.tall)) {
                    v.wait = wheel(window, at, 0., -200., 30);
                }
            }
            3 => {
                let (top, max) = (self.list_top(v.tall), self.list_max(v.tall));
                let page = self.page_y() - v.snap["page"];
                v.check(
                    "At the table's end, the wheel carries on to the chat",
                    (max - top).abs() < 1. && page > 50.,
                    format!("table at {top:.0} of {max:.0} pt, chat moved {page:.0} pt"),
                );
                v.snap.insert("top", top);
                // The chat moved on past the table; bring it back under the pointer.
                self.scroll.scroll_to_item(v.tall);
            }
            4 => {
                v.snap.insert("page", self.page_y());
                if let Some(at) = self.probe_at(&format!("table-body-{}", v.tall)) {
                    v.wait = wheel(window, at, 0., 40., 3);
                }
            }
            5 => {
                let moved = v.snap["top"] - self.list_top(v.tall);
                let page = self.page_y() - v.snap["page"];
                v.check(
                    "Scrolling back up moves the table before the chat",
                    moved > 100. && page.abs() < 1.,
                    format!("table moved {moved:.0} pt up, chat moved {page:.0} pt"),
                );
                self.scroll.scroll_to_item(v.wide);
            }
            6 => {
                v.snap.insert("page", self.page_y());
                v.snap.insert("x", self.hscroll_x(v.wide));
                if let Some(at) = self.probe_at(&format!("table-{}", v.wide)) {
                    v.wait = wheel(window, at, 0., -40., 3);
                }
            }
            7 => {
                let page = self.page_y() - v.snap["page"];
                let x = self.hscroll_x(v.wide) - v.snap["x"];
                v.check(
                    "Vertical wheel over a wide table scrolls the chat, never the table sideways",
                    page > 100. && x.abs() < 1.,
                    format!("chat moved {page:.0} pt, table moved {x:.0} pt sideways"),
                );
                // The vertical wheel moved the table; bring it back first.
                self.scroll.scroll_to_item(v.wide);
            }
            8 => {
                v.snap.insert("page", self.page_y());
                v.snap.insert("x", self.hscroll_x(v.wide));
                if let Some(at) = self.probe_at(&format!("table-{}", v.wide)) {
                    v.wait = wheel(window, at, -60., 0., 3);
                }
            }
            9 => {
                let page = self.page_y() - v.snap["page"];
                let x = self.hscroll_x(v.wide) - v.snap["x"];
                v.check(
                    "Sideways wheel scrolls the wide table",
                    x > 100. && page.abs() < 1.,
                    format!("table moved {x:.0} pt sideways, chat moved {page:.0} pt"),
                );
                self.shot("wide-scrolled");
                self.scroll.scroll_to_item(v.tall);
            }
            10 => {
                if let Some(at) = self.probe_at(&format!("copy-md-{}", v.tall)) {
                    click(window, at);
                }
            }
            11 => {
                let want = self.tables[&v.tall].markdown();
                let got = cx.read_from_clipboard().and_then(|c| c.text());
                v.check(
                    "Copy puts the table on the clipboard as Markdown",
                    got.as_deref() == Some(want.as_str()),
                    format!("{} lines", got.as_deref().unwrap_or("").lines().count()),
                );
                self.shot("copied");
                if let Some(at) = self.probe_at(&format!("copy-csv-{}", v.tall)) {
                    click(window, at);
                }
            }
            12 => {
                let want = self.tables[&v.tall].csv();
                let got = cx.read_from_clipboard().and_then(|c| c.text());
                v.check(
                    "CSV puts the table on the clipboard as CSV",
                    got.as_deref() == Some(want.as_str()),
                    got.as_deref()
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                );
                let mut keys: Vec<String> = self
                    .probes
                    .borrow()
                    .keys()
                    .filter(|k| k.starts_with("copy-code-"))
                    .cloned()
                    .collect();
                keys.sort();
                v.code_key = keys.last().cloned().unwrap_or_default();
                let block = v.code_key.split('-').nth(2).and_then(|b| b.parse().ok());
                if let Some(b) = block {
                    self.scroll.scroll_to_item(b);
                }
            }
            13 => {
                if let Some(at) = self.probe_at(&v.code_key) {
                    click(window, at);
                }
            }
            14 => {
                let want = self
                    .copy_texts
                    .get(&v.code_key)
                    .map(|f| f())
                    .unwrap_or_default();
                let got = cx.read_from_clipboard().and_then(|c| c.text());
                v.check(
                    "Copy on a code block puts exactly its code on the clipboard",
                    !want.is_empty() && got.as_deref() == Some(want.as_str()),
                    format!(
                        "{} characters; got {:?}",
                        want.len(),
                        got.as_deref()
                            .map(|g| g.chars().take(40).collect::<String>())
                    ),
                );
                self.scroll.scroll_to_item(v.tall);
            }
            15 => {
                if let Some(at) = self.probe_at(&format!("open-{}", v.tall)) {
                    click(window, at);
                }
            }
            16 => {
                v.check(
                    "Open shows the whole table in a side panel",
                    self.panel == Some(v.tall),
                    format!("panel {:?}", self.panel),
                );
                self.shot("panel");
                v.snap.insert("page", self.page_y());
                if let Some(at) = self.probe_at("panel-body") {
                    v.wait = wheel(window, at, 0., -200., 10);
                }
            }
            17 => {
                let top = -self.panel_list.scroll_px_offset_for_scrollbar().y.as_f32();
                let page = self.page_y() - v.snap["page"];
                v.check(
                    "The panel scrolls on its own; the chat behind it stays put",
                    top > 500. && page.abs() < 1.,
                    format!("panel moved {top:.0} pt, chat moved {page:.0} pt"),
                );
                self.shot("panel-scrolled");
                if let Some(at) = self.probe_at("panel-close") {
                    click(window, at);
                }
            }
            18 => {
                v.check(
                    "Close dismisses the panel",
                    self.panel.is_none(),
                    String::new(),
                );
                if let Some(state) = self.table_lists.get(&v.tall) {
                    state.scroll_to(ListOffset::default());
                }
                let found = self.reveal("event number 47");
                v.check(
                    "Search reaches a row the capped table keeps out of view",
                    found.is_some_and(|(k, ix)| k == v.tall && ix == 46),
                    format!("{found:?}"),
                );
            }
            19 => {
                let top = self.list_top(v.tall);
                v.check(
                    "Revealing a row scrolls the table to it",
                    top > 300.,
                    format!("table at {top:.0} pt"),
                );
                self.shot("find");
                // Skip the first frames: probes report the frame before.
                let capped: Vec<f32> = v.heights.iter().copied().skip(2).collect();
                let lo = capped.iter().copied().fold(f32::MAX, f32::min);
                let hi = capped.iter().copied().fold(0., f32::max);
                v.check(
                    "While rows stream in, the capped table keeps its height",
                    capped.len() > 3 && hi - lo < 1.,
                    format!("{} frames, {lo:.1}–{hi:.1} pt", capped.len()),
                );
            }
            _ => {
                if let Some(saved) = v.saved_clipboard.take() {
                    match saved {
                        Some(item) => cx.write_to_clipboard(item),
                        None => cx.write_to_clipboard(ClipboardItem::new_string(String::new())),
                    }
                }
                let ok = v.results.iter().filter(|r| r.1).count();
                for (name, pass, detail) in &v.results {
                    println!("{} {name} ({detail})", if *pass { "PASS" } else { "FAIL" });
                }
                println!("{ok}/{} passed", v.results.len());
                cx.quit();
            }
        }
    }
}
