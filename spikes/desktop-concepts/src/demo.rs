//! Scripted walkthroughs. Clicks and keys go through the window's normal
//! event dispatch (real hit-testing, real focus), checks read the state, and
//! stills and the recording capture only the window.
//!
//! `--demo tour` is the watchable walkthrough; `--demo checks` runs the edge
//! cases. Both print PASS/FAIL per check and quit.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::{Attachment, Dent, Menu, Tab};
use crate::data::Item;
use crate::theme::Status;

type Pred = fn(&Dent, &App) -> bool;
type Call = fn(&mut Dent, &mut Window, &mut Context<Dent>);

#[derive(Clone, Copy, PartialEq)]
pub enum Field {
    Composer,
    Starter,
    Comment,
}

#[derive(Clone)]
pub enum Act {
    Wait,
    Click(&'static str),
    Key(&'static str),
    Type(Field, &'static str),
    /// Type into the focused field key by key, through normal input.
    Keys(&'static str),
    Page(&'static str),
    Call(Call),
    /// Passes when the predicate holds within the timeout (seconds).
    Until(&'static str, Pred, f32),
    Shot(&'static str),
}

pub struct Demo {
    name: String,
    steps: Vec<(f32, Act)>,
    ix: usize,
    due: Instant,
    waiting_since: Option<Instant>,
    results: Vec<(&'static str, bool)>,
    shots: Option<PathBuf>,
    hold: f32,
}

use Act::*;

impl Demo {
    pub fn new(name: &str, shots: Option<PathBuf>, recording: bool) -> Self {
        let steps = match name {
            "checks" => checks(),
            "stills" => stills(),
            _ => tour(),
        };
        Self {
            name: name.into(),
            steps,
            ix: 0,
            due: Instant::now() + Duration::from_millis(1500),
            waiting_since: None,
            results: Vec::new(),
            shots,
            hold: if recording { 1.5 } else { 0.3 },
        }
    }

    /// Roughly how long it runs, for the recorder.
    pub fn length(&self) -> f32 {
        1.5 + self
            .steps
            .iter()
            .map(|(w, a)| w + if matches!(a, Until(..)) { 2.5 } else { 0. })
            .sum::<f32>()
            + self.hold
    }
}

pub fn tick(me: Entity<Dent>, window: &mut Window, cx: &mut App) {
    let step = me.update(cx, |this, _| {
        let d = this.demo.as_mut()?;
        if Instant::now() < d.due {
            return None;
        }
        Some(d.steps.get(d.ix).cloned().map(|(_, a)| a))
    });
    let Some(step) = step else { return };
    let Some(act) = step else {
        // Done: report and quit.
        let (name, results, hold) = me.update(cx, |this, _| {
            let d = this.demo.take().expect("demo");
            (d.name, d.results, d.hold)
        });
        let failed = results.iter().filter(|(_, ok)| !ok).count();
        for (name, ok) in &results {
            println!("{} {name}", if *ok { "PASS" } else { "FAIL" });
        }
        println!("{name}: {} checks, {failed} failed", results.len());
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f32(hold))
                .await;
            cx.update(|cx| cx.quit());
        })
        .detach();
        return;
    };
    let mut advance = true;
    match act {
        Wait => {}
        Click(name) => {
            if let Some(at) = center(&me, name, cx) {
                // Arrive first, like a pointer would, so hover follows the click.
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                mouse(window, cx, at, true);
                mouse(window, cx, at, false);
            } else {
                note(
                    &me,
                    cx,
                    leak(format!("click target “{name}” on screen")),
                    false,
                );
            }
        }
        Key(k) => {
            if let Ok(ks) = Keystroke::parse(k) {
                window.dispatch_keystroke(ks, cx);
            }
        }
        Type(field, text) => {
            me.update(cx, |this, cx| {
                let input = match field {
                    Field::Composer => this.composer.clone(),
                    Field::Starter => this.starter.clone(),
                    Field::Comment => this.comment_input.clone(),
                };
                input.update(cx, |i, cx| {
                    i.set_value(text, window, cx);
                    i.focus(window, cx);
                });
                cx.notify();
            });
        }
        Keys(text) => {
            for ch in text.chars() {
                let key = if ch == ' ' {
                    "space".to_string()
                } else {
                    ch.to_lowercase().to_string()
                };
                let ks = Keystroke {
                    modifiers: Modifiers {
                        shift: ch.is_uppercase(),
                        ..Default::default()
                    },
                    key,
                    key_char: Some(ch.to_string()),
                };
                window.dispatch_keystroke(ks, cx);
            }
        }
        Page(js) => {
            me.update(cx, |this, _| {
                if let Some(p) = &this.preview {
                    p.borrow().script(js);
                }
            });
        }
        Call(f) => me.update(cx, |this, cx| f(this, window, cx)),
        Until(name, pred, timeout) => {
            let ok = pred(me.read(cx), cx);
            let since = me.update(cx, |this, _| {
                *this
                    .demo
                    .as_mut()
                    .expect("demo")
                    .waiting_since
                    .get_or_insert_with(Instant::now)
            });
            if ok || since.elapsed().as_secs_f32() > timeout {
                note(&me, cx, name, ok);
                me.update(cx, |this, _| {
                    this.demo.as_mut().expect("demo").waiting_since = None
                });
            } else {
                advance = false;
            }
        }
        Shot(name) => {
            let dir = me.read(cx).demo.as_ref().and_then(|d| d.shots.clone());
            if let Some(dir) = dir {
                let _ = std::fs::create_dir_all(&dir);
                still(dir.join(format!("{name}.png")));
            }
        }
    }
    if advance {
        me.update(cx, |this, _| {
            let d = this.demo.as_mut().expect("demo");
            d.ix += 1;
            let wait = d.steps.get(d.ix).map(|(w, _)| *w).unwrap_or(0.);
            d.due = Instant::now() + Duration::from_secs_f32(wait);
        });
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn note(me: &Entity<Dent>, cx: &mut App, name: &'static str, ok: bool) {
    me.update(cx, |this, _| {
        if let Some(d) = this.demo.as_mut() {
            d.results.push((name, ok));
        }
    });
    eprintln!("{} {name}", if ok { "pass" } else { "FAIL" });
}

fn center(me: &Entity<Dent>, name: &str, cx: &App) -> Option<Point<Pixels>> {
    me.read(cx).probes.borrow().get(name).map(|b| b.center())
}

fn mouse(window: &mut Window, cx: &mut App, at: Point<Pixels>, down: bool) {
    let ev = if down {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        })
    } else {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
        })
    };
    window.dispatch_event(ev, cx);
}

// ---- Helpers for checks ----

fn session<'a>(d: &'a Dent, id: &str) -> Option<&'a crate::data::Session> {
    d.sessions.iter().find(|s| s.id == id)
}
fn status(d: &Dent, id: &str) -> Option<Status> {
    session(d, id).map(|s| s.status)
}
fn latest_new(d: &Dent) -> Option<&crate::data::Session> {
    d.sessions.iter().find(|s| s.id.starts_with("new-"))
}

// ---- Scenes ----

/// The watchable walkthrough, with stills at each state worth showing.
fn tour() -> Vec<(f32, Act)> {
    vec![
        // A page the agent made, with an error in it.
        (
            0.5,
            Until(
                "the preview reports the page's error",
                |d, _| d.errors.get(&("pricing", 2)).is_some_and(|e| !e.is_empty()),
                4.,
            ),
        ),
        (
            0.0,
            Until(
                "a page error puts its session in Needs you",
                |d, _| status(d, "pricing") == Some(Status::Needs),
                1.,
            ),
        ),
        (0.6, Shot("01-pricing-error")),
        (0.8, Click("fix")),
        (
            0.3,
            Until(
                "Ask to fix sends the error to the agent",
                |d, _| {
                    session(d, "pricing").is_some_and(|s| {
                        s.items
                            .iter()
                            .any(|i| matches!(i, Item::User(t) if t.contains("formatPrices")))
                    })
                },
                2.,
            ),
        ),
        (
            0.0,
            Until(
                "the fix arrives as v3 and the preview follows it",
                |d, _| {
                    d.preview
                        .as_ref()
                        .is_some_and(|p| p.borrow().showing.as_ref().is_some_and(|(_, v)| *v == 3))
                },
                12.,
            ),
        ),
        (
            1.0,
            Until(
                "v3 has no errors",
                |d, _| d.errors.get(&("pricing", 3)).is_none_or(|e| e.is_empty()),
                2.,
            ),
        ),
        (0.2, Shot("02-pricing-fixed")),
        // Point at the button, then ask for a change.
        (0.8, Click("pick")),
        (0.6, Page("window.__dentShow('.featured .cta')")),
        (
            0.9,
            Page("document.querySelector('.featured .cta').click()"),
        ),
        (
            0.2,
            Until(
                "pointing at an element attaches it to the message",
                |d, _| {
                    d.attachments
                        .iter()
                        .any(|a| matches!(a, Attachment::Element { tag, .. } if tag == "button"))
                },
                3.,
            ),
        ),
        (
            0.4,
            Type(
                Field::Composer,
                "Make this the main button: bigger, and say it's a 14-day trial.",
            ),
        ),
        (0.5, Shot("03-pointed")),
        (0.6, Key("enter")),
        (
            0.0,
            Until(
                "the change arrives as v4",
                |d, _| d.art("pricing").is_some_and(|a| a.versions.len() == 4),
                12.,
            ),
        ),
        (1.4, Shot("04-pricing-v4")),
        (0.6, Click("vp-phone")),
        (1.2, Shot("05-phone-width")),
        (0.6, Click("vp-fill")),
        (0.6, Click("prev-version")),
        (
            1.0,
            Until(
                "an older version opens with a way back",
                |d, _| d.viewing.get("pricing") == Some(&3),
                2.,
            ),
        ),
        (0.2, Shot("06-older-version")),
        (1.0, Click("latest")),
        // An approval, answered from the chat.
        (0.8, Click("row-login")),
        (0.8, Shot("07-approval")),
        (0.6, Click("allow")),
        (
            0.0,
            Until(
                "Allow runs the command and the session finishes",
                |d, _| status(d, "login") == Some(Status::Done),
                10.,
            ),
        ),
        (0.8, Shot("08-allowed")),
        // Review the changes with a comment.
        (0.6, Click("tab-changes")),
        (0.8, Click("gutter-8")),
        (
            0.4,
            Type(Field::Comment, "Make the 60 s window a named constant."),
        ),
        (0.6, Click("add-comment")),
        (
            0.3,
            Until(
                "the comment joins the review",
                |d, _| d.review.len() == 1,
                2.,
            ),
        ),
        (0.4, Shot("09-review")),
        (0.8, Click("send-review")),
        (
            0.0,
            Until(
                "sending the review starts the agent on it",
                |d, _| status(d, "login") == Some(Status::Working),
                2.,
            ),
        ),
        // A plan and a screenshot.
        (1.2, Click("row-ledger")),
        (1.0, Shot("10-doc")),
        (0.8, Click("row-swiftui")),
        (1.0, Shot("11-image")),
        // Close, undo.
        (0.8, Click("row-flaky")),
        (0.8, Key("cmd-w")),
        (
            0.2,
            Until(
                "Cmd+W closes a done session and offers Undo",
                |d, _| session(d, "flaky").is_some_and(|s| s.closed) && d.toast.is_some(),
                2.,
            ),
        ),
        (0.3, Shot("12-closed")),
        (1.2, Click("toast-action")),
        (
            0.2,
            Until(
                "Undo brings it back",
                |d, _| session(d, "flaky").is_some_and(|s| !s.closed),
                2.,
            ),
        ),
        // A new session, started by typing.
        (0.8, Key("cmd-n")),
        (0.6, Keys("Fix the pricing page footer on small screens")),
        (
            0.9,
            Until(
                "the project is guessed from the words",
                |d, _| d.defaults.project == Some("marketing-site"),
                2.,
            ),
        ),
        (0.2, Shot("13-new-session")),
        (0.6, Click("chip-machine")),
        (
            0.5,
            Until(
                "the machine menu opens",
                |d, _| d.menu == Some(Menu::Machine),
                1.,
            ),
        ),
        (0.2, Shot("14-machine-menu")),
        (1.0, Key("escape")),
        (0.5, Click("start")),
        (
            0.2,
            Until(
                "Start opens the session, working",
                |d, _| {
                    latest_new(d).is_some_and(|s| s.status == Status::Working)
                        && d.selected.as_deref().is_some_and(|s| s.starts_with("new-"))
                },
                2.,
            ),
        ),
        (1.6, Shot("15-started")),
        (3.0, Wait),
    ]
}

/// Edge cases, run fast: things that would break in daily use.
fn checks() -> Vec<(f32, Act)> {
    vec![
        // Needs-you sessions can't be closed.
        (0.2, Click("row-login")),
        (0.3, Key("cmd-w")),
        (
            0.2,
            Until(
                "a session that needs you can't be closed",
                |d, _| session(d, "login").is_some_and(|s| !s.closed),
                1.,
            ),
        ),
        // Closing opens the next thing to clear, in inbox order.
        (0.2, Click("row-ledger")),
        (0.3, Key("cmd-w")),
        (
            0.2,
            Until(
                "closing opens the next session in the inbox",
                |d, _| d.selected.as_deref() == Some("flaky"),
                1.,
            ),
        ),
        (0.2, Call(|d, w, cx| d.toast_action(w, cx))),
        // A double press on Send never stops the agent.
        (0.3, Key("cmd-n")),
        (0.2, Type(Field::Starter, "Tidy the README")),
        (0.2, Key("enter")),
        (0.3, Type(Field::Composer, "Also fix the typos")),
        (0.1, Click("send")),
        (0.05, Click("send")),
        (
            0.3,
            Until(
                "a double press on Send queues once and doesn't stop",
                |d, _| {
                    latest_new(d).is_some_and(|s| {
                        s.status == Status::Working
                            && s.items
                                .iter()
                                .filter(|i| matches!(i, Item::Queued(_)))
                                .count()
                                == 1
                    })
                },
                1.,
            ),
        ),
        // Queued messages wait below the reply and run after it.
        (
            0.0,
            Until(
                "the queued message runs after the reply finishes",
                |d, _| {
                    latest_new(d).is_some_and(|s| {
                        !s.items.iter().any(|i| matches!(i, Item::Queued(_)))
                            && matches!(s.items.last(), Some(Item::Text(_)))
                            && s.items
                                .iter()
                                .filter(|i| matches!(i, Item::User(_)))
                                .count()
                                == 2
                    })
                },
                15.,
            ),
        ),
        (
            0.0,
            Until(
                "…and every reply lands before the message after it",
                |d, _| {
                    latest_new(d).is_some_and(|s| {
                        let users: Vec<usize> = s
                            .items
                            .iter()
                            .enumerate()
                            .filter(|(_, i)| matches!(i, Item::User(_)))
                            .map(|(i, _)| i)
                            .collect();
                        users.len() == 2
                            && s.items[users[0]..users[1]]
                                .iter()
                                .any(|i| matches!(i, Item::Text(_)))
                    })
                },
                1.,
            ),
        ),
        // Stop really stops.
        (0.3, Type(Field::Composer, "One more thing")),
        (0.1, Key("enter")),
        (
            1.3,
            Call(|d, _, cx| {
                if let Some(id) = d.selected.clone() {
                    d.stop(&id, cx);
                }
            }),
        ),
        (
            0.0,
            Call(|d, _, _| {
                let len = latest_new(d)
                    .map(|s| {
                        s.items
                            .iter()
                            .map(|i| if let Item::Text(t) = i { t.len() } else { 0 })
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                STOPPED_LEN.store(len, std::sync::atomic::Ordering::Relaxed);
            }),
        ),
        (
            1.2,
            Until(
                "nothing arrives after Stop",
                |d, _| {
                    latest_new(d).is_some_and(|s| {
                        s.items
                            .iter()
                            .map(|i| if let Item::Text(t) = i { t.len() } else { 0 })
                            .sum::<usize>()
                            == STOPPED_LEN.load(std::sync::atomic::Ordering::Relaxed)
                            && s.status == Status::Idle
                    })
                },
                0.5,
            ),
        ),
        (
            0.0,
            Until(
                "Stop offers Resume",
                |d, _| {
                    d.toast
                        .as_ref()
                        .is_some_and(|t| matches!(t.action, crate::app::ToastAction::Resume(_)))
                },
                0.5,
            ),
        ),
        (0.2, Click("toast-action")),
        (
            0.2,
            Until(
                "Resume starts it again",
                |d, _| latest_new(d).is_some_and(|s| s.status == Status::Working),
                1.,
            ),
        ),
        // A long draft keeps the composer inside the window.
        (
            0.3,
            Type(
                Field::Composer,
                "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\nline 11\nline 12\nline 13\nline 14",
            ),
        ),
        (
            0.5,
            Until(
                "a 14-line draft stays inside the window, above the status bar",
                |d, _| {
                    let p = d.probes.borrow();
                    let (_, _, _, h) = crate::window_rect();
                    p.get("composer").is_some_and(|b| {
                        (b.bottom().as_f32()) <= h - 28. && b.size.height.as_f32() < 340.
                    })
                },
                1.,
            ),
        ),
        (
            0.0,
            Until(
                "…and the conversation keeps room to read",
                |d, _| {
                    d.probes
                        .borrow()
                        .get("composer")
                        .is_some_and(|b| b.origin.y.as_f32() > 300.)
                },
                0.5,
            ),
        ),
        (0.2, Type(Field::Composer, "")),
        // Drafts belong to their session.
        (0.2, Type(Field::Composer, "draft for this one")),
        (0.2, Click("row-pricing")),
        (
            0.3,
            Until(
                "switching sessions keeps each draft",
                |d, cx| {
                    d.drafts.values().any(|v| v == "draft for this one")
                        && d.composer.read(cx).value().is_empty()
                },
                1.,
            ),
        ),
        // The project guess follows the words until you pick one.
        (0.2, Key("cmd-n")),
        (0.2, Keys("rate limit the login page")),
        (
            0.6,
            Until(
                "the project follows what you type",
                |d, _| d.defaults.project == Some("auth-service"),
                1.,
            ),
        ),
        (0.2, Click("chip-project")),
        (0.3, Click("m-billing")),
        (0.2, Keys(", and the pricing page")),
        (
            0.6,
            Until(
                "…until you pick one yourself",
                |d, _| d.defaults.project == Some("billing"),
                1.,
            ),
        ),
        (0.2, Click("chip-model")),
        (
            0.3,
            Until(
                "menus close on Escape",
                |d, _| d.menu == Some(Menu::Model),
                1.,
            ),
        ),
        (0.1, Key("escape")),
        (
            0.2,
            Until("…and Escape closed it", |d, _| d.menu.is_none(), 1.),
        ),
        // Cmd+K finds closed sessions too, and opening one reopens it.
        (0.3, Key("cmd-k")),
        (0.3, Keys("onboarding")),
        (
            0.4,
            Until(
                "search finds a closed session",
                |d, cx| d.palette_results(cx).first().map(String::as_str) == Some("onboard"),
                1.,
            ),
        ),
        (0.2, Key("enter")),
        (
            0.3,
            Until(
                "…and opening it from search reopens it",
                |d, _| {
                    d.selected.as_deref() == Some("onboard")
                        && session(d, "onboard").is_some_and(|s| !s.closed)
                        && d.palette.is_none()
                },
                1.,
            ),
        ),
        // Esc puts focus in the inbox; arrows move, E closes, and focus stays.
        (0.2, Call(|d, w, cx| d.select(Some("ledger".into()), w, cx))),
        (0.3, Key("escape")),
        (
            0.2,
            Until(
                "Esc from an empty composer focuses the inbox",
                |d, _| d.inbox_focused,
                1.,
            ),
        ),
        (0.2, Key("down")),
        (
            0.2,
            Until(
                "Down moves to the next session",
                |d, _| d.selected.as_deref() == Some("flaky") && d.inbox_focused,
                1.,
            ),
        ),
        (0.2, Key("e")),
        (
            0.3,
            Until(
                "E closes it and stays in the inbox",
                |d, _| session(d, "flaky").is_some_and(|s| s.closed) && d.inbox_focused,
                1.,
            ),
        ),
        (0.2, Key("cmd-z")),
        (
            0.3,
            Until(
                "Cmd+Z undoes the close from the inbox",
                |d, _| session(d, "flaky").is_some_and(|s| !s.closed),
                1.,
            ),
        ),
        (0.2, Key("alt-cmd-up")),
        (
            0.2,
            Until(
                "Alt+Cmd+Up moves between sessions from anywhere",
                |d, _| d.selected.is_some() && d.selected.as_deref() != Some("flaky"),
                1.,
            ),
        ),
        // The preview stays hidden when it isn't the page on screen.
        (0.2, Call(|d, w, cx| d.select(Some("ledger".into()), w, cx))),
        (
            0.4,
            Until(
                "the live page hides behind other tabs",
                |d, _| !d.preview_visible(),
                1.,
            ),
        ),
        (
            0.2,
            Call(|d, w, cx| d.select(Some("pricing".into()), w, cx)),
        ),
        (
            0.4,
            Until(
                "…and shows again on its own tab",
                |d, _| {
                    d.preview_visible()
                        && d.current().is_some_and(|s| d.tab(s) == Tab::Art("pricing"))
                },
                1.,
            ),
        ),
    ]
}

/// Still frames in the current theme and size: no checks.
fn stills() -> Vec<(f32, Act)> {
    vec![
        (1.5, Shot("a-pricing")),
        (0.3, Click("tab-changes")),
        (0.6, Shot("a2-pricing-changes")),
        (0.3, Click("row-login")),
        (0.8, Shot("b-approval")),
        (0.3, Click("tab-changes")),
        (0.6, Shot("c-changes")),
        (0.3, Click("row-ledger")),
        (0.8, Shot("d-doc")),
        (0.3, Key("cmd-n")),
        (0.3, Keys("Fix the pricing page footer on small screens")),
        (0.9, Shot("e-new")),
    ]
}

/// How much reply text there was the moment Stop was pressed.
static STOPPED_LEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Record this window only, through the `RECORDER` helper
/// (tools/record-window.swift). There is no screen-recording fallback: a
/// recording must never contain anything but the dent window.
pub fn record(path: PathBuf, secs: f32) {
    let Ok(tool) = std::env::var("RECORDER") else {
        eprintln!("--record needs RECORDER set to a build of tools/record-window.swift");
        return;
    };
    std::thread::spawn(move || {
        // Let the window reach the screen first.
        std::thread::sleep(Duration::from_millis(600));
        let _ = std::process::Command::new(tool)
            .arg(std::process::id().to_string())
            // Generous: it stops by itself when the window closes.
            .arg(format!("{:.1}", secs * 2. + 30.))
            .arg(&path)
            .status();
    });
}

/// The window only, never what's around it: by window number when the
/// `WINID` helper (tools/winid.swift) is given, else the window's rectangle.
pub fn still(path: PathBuf) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        let id = std::env::var("WINID").ok().and_then(|tool| {
            let out = std::process::Command::new(tool)
                .arg(std::process::id().to_string())
                .output()
                .ok()?;
            Some(String::from_utf8(out.stdout).ok()?.trim().to_string()).filter(|s| !s.is_empty())
        });
        let mut cmd = std::process::Command::new("screencapture");
        match id {
            Some(id) => cmd.args(["-x", "-o", &format!("-l{id}")]),
            None => {
                let (x, y, w, h) = crate::window_rect();
                cmd.args(["-x", "-R", &format!("{x},{y},{w},{h}")])
            }
        };
        let _ = cmd.arg(&path).status();
    });
}
