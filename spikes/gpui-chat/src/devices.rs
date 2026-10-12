//! `--devices`: Settings › Devices, driving real daemons on this machine
//! that stand in for other machines. One is on the LAN (pairs with a code),
//! one is on the same Tailscale account (simulated; trusted with no code).
//!
//! `--devices --demo` plays a scripted walkthrough for recording.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;
use spike_link::devices::{self, Cmd, Device, Devices, Snapshot, Status, Via};

use crate::ds::{DISPLAY_COMPACT, LABEL, READING, Typed, UI, radius, size, space};
use crate::theme::Theme;

struct Machine {
    child: Option<Child>,
    args: Vec<String>,
    state: PathBuf,
}

impl Machine {
    fn start(&mut self) {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("spike-link")))
            .unwrap_or_else(|| "spike-link".into());
        self.child = Command::new(exe)
            .args(&self.args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn code(&self) -> String {
        std::fs::read_to_string(self.state.join("pairing-code")).unwrap_or_default()
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct DevicesView {
    theme: Theme,
    devices: Devices,
    snap: Snapshot,
    _rt: tokio::runtime::Runtime,
    machines: Vec<Machine>,
    /// Digits typed for the device asking for a code.
    code: String,
    prompt: String,
    target: Option<String>,
    focus: FocusHandle,
    demo: Option<(Instant, usize)>,
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(7481)
}

impl DevicesView {
    pub fn new(dark: bool, demo: bool, cx: &mut Context<Self>) -> Self {
        let root = std::env::temp_dir().join(format!("dent-devices-{}", std::process::id()));
        let (lan, tail) = (port(), port());
        let studio = root.join("studio");
        let build = root.join("build-box");
        let mut machines = vec![
            Machine {
                child: None,
                args: vec![
                    "daemon".into(),
                    "--name".into(),
                    "studio".into(),
                    "--state".into(),
                    studio.display().to_string(),
                    "--lan".into(),
                    format!("0.0.0.0:{lan}"),
                ],
                state: studio,
            },
            Machine {
                child: None,
                args: vec![
                    "daemon".into(),
                    "--name".into(),
                    "build-box".into(),
                    "--state".into(),
                    build.display().to_string(),
                    "--lan".into(),
                    format!("127.0.0.1:{}", port()),
                    "--tailnet".into(),
                    format!("127.0.0.1:{tail}"),
                    "--sim-me".into(),
                    "you@example.com".into(),
                    "--sim-owner".into(),
                    "you@example.com".into(),
                    "--no-mdns".into(),
                ],
                state: build,
            },
        ];
        for m in &mut machines {
            m.start();
        }
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let devices = rt
            .block_on(async {
                devices::start(
                    root.join("laptop"),
                    "laptop",
                    vec![std::net::SocketAddr::from(([127, 0, 0, 1], tail))],
                )
            })
            .expect("devices");
        let mut changed = devices.changed.clone();
        cx.spawn(async move |this, cx| {
            while changed.changed().await.is_ok() {
                if this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Offline times tick.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            theme: if dark {
                Theme::carbon()
            } else {
                Theme::paper()
            },
            snap: devices.state.lock().map(|s| s.clone()).unwrap_or_default(),
            devices,
            _rt: rt,
            machines,
            code: String::new(),
            prompt: String::new(),
            target: None,
            focus: cx.focus_handle(),
            demo: demo.then(|| (Instant::now(), 0)),
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Ok(s) = self.devices.state.lock() {
            self.snap = s.clone();
        }
        cx.notify();
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.devices.cmd.send(cmd);
    }

    fn by_name(&self, name: &str) -> Option<Device> {
        self.snap.devices.iter().find(|d| d.name == name).cloned()
    }

    fn pairing(&self) -> Option<Device> {
        self.snap
            .devices
            .iter()
            .find(|d| matches!(d.status, Status::NeedsCode { .. }))
            .cloned()
    }

    fn submit_code(&mut self) {
        if let Some(d) = self.pairing()
            && self.code.len() == 6
        {
            let code = std::mem::take(&mut self.code);
            self.send(Cmd::Code(d.id, code));
        }
    }

    fn submit_prompt(&mut self) {
        let Some(id) = self.target.clone() else {
            return;
        };
        if self.prompt.trim().is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.prompt);
        self.send(Cmd::Prompt(id, text));
    }

    fn key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let pairing = self.pairing().is_some();
        match k.key.as_str() {
            "backspace" => {
                if pairing {
                    self.code.pop();
                } else {
                    self.prompt.pop();
                }
            }
            "enter" => {
                if pairing {
                    self.submit_code();
                } else {
                    self.submit_prompt();
                }
            }
            _ => {
                if let Some(ch) = k.key_char.as_deref() {
                    if pairing {
                        if ch.len() == 1
                            && ch.chars().all(|c| c.is_ascii_digit())
                            && self.code.len() < 6
                        {
                            self.code.push_str(ch);
                        }
                    } else {
                        self.prompt.push_str(ch);
                    }
                }
            }
        }
        cx.notify();
    }

    /// The scripted walkthrough: what a person would do, step by step.
    fn step_demo(&mut self, window: &mut Window) {
        let Some((start, step)) = self.demo else {
            return;
        };
        if step == 0 && start.elapsed().as_millis() < 50 {
            crate::specimen::reveal::hide_cursor();
        }
        let t = start.elapsed().as_secs_f32();
        let mut next = step;
        let due = |at: f32| t >= at;
        match step {
            0 if due(3.2) && self.by_name("build-box").is_some() => {
                if let Some(d) = self.by_name("build-box") {
                    self.send(Cmd::Add(d.id));
                }
                next = 1;
            }
            1 if due(5.2) && self.by_name("studio").is_some() => {
                if let Some(d) = self.by_name("studio") {
                    self.send(Cmd::Add(d.id));
                }
                next = 2;
            }
            2 if due(6.2) => {
                // The person reads the code off the other machine's screen.
                let code = self.machines[0].code();
                let typed = ((t - 6.2) / 0.16) as usize;
                self.code = code.chars().take(typed.min(6)).collect();
                if typed >= 7 {
                    self.submit_code();
                    next = 3;
                }
            }
            3 if due(9.6) => {
                self.target = self.by_name("studio").map(|d| d.id);
                next = 4;
            }
            4 if due(10.0) => {
                let text = "Summarise what changed in the repo today";
                let typed = ((t - 10.0) / 0.035) as usize;
                self.prompt = text.chars().take(typed).collect();
                if typed > text.len() + 6 {
                    self.submit_prompt();
                    next = 5;
                }
            }
            5 if due(16.5) => {
                self.machines[0].stop();
                next = 6;
            }
            6 if due(22.0) => {
                self.machines[0].start();
                next = 7;
            }
            7 if due(25.0) => {
                window.remove_window();
                next = 8;
            }
            _ => {}
        }
        self.demo = Some((start, next));
        window.request_animation_frame();
    }
}

impl Render for DevicesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.step_demo(window);
        let t = self.theme;
        let me = self.snap.me.clone();
        let connected: Vec<Device> = self
            .snap
            .devices
            .iter()
            .filter(|d| d.trusted_via.is_some())
            .cloned()
            .collect();
        let available: Vec<Device> = self
            .snap
            .devices
            .iter()
            .filter(|d| d.trusted_via.is_none())
            .cloned()
            .collect();
        if self.target.is_none() {
            self.target = connected
                .iter()
                .find(|d| d.status == Status::Online)
                .map(|d| d.id.clone());
        }
        let this_mac = row(
            &t,
            square(&t, Some(t.done.0)),
            me.0.clone(),
            format!("This Mac · {}", &me.1),
            div()
                .ty(LABEL)
                .text_color(t.quiet)
                .child("Online")
                .into_any_element(),
        );
        let connected_rows: Vec<AnyElement> =
            connected.iter().map(|d| self.device_row(d, cx)).collect();
        let available_rows: Vec<AnyElement> =
            available.iter().map(|d| self.device_row(d, cx)).collect();
        div()
            .id("devices")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|v, e, _, cx| v.key(e, cx)))
            .size_full()
            .bg(t.surface)
            .text_color(t.ink)
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .px(px(space::S8))
                    .py(px(space::S6))
                    .flex()
                    .flex_col()
                    .gap(px(space::S6))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S1))
                            .child(div().ty(LABEL).text_color(t.quiet).child("Settings"))
                            .child(div().ty(DISPLAY_COMPACT).child("Devices"))
                            .child(div().ty(UI).text_color(t.quiet).child(
                                "Machines that can run your agents. dent finds them on your network and on your Tailscale account.",
                            )),
                    )
                    .child(section(&t, "This Mac", vec![this_mac], None))
                    .child(section(
                        &t,
                        "Connected",
                        connected_rows,
                        Some("Nothing yet. Add a machine from the list below."),
                    ))
                    .child(section(
                        &t,
                        "Available",
                        available_rows,
                        Some(if connected.is_empty() {
                            "Looking for machines running dent…"
                        } else {
                            "No other machines found. Machines running dent on your network or Tailscale account appear here."
                        }),
                    ))
                    .child(self.composer(&connected, cx)),
            )
    }
}

impl DevicesView {
    fn device_row(&self, d: &Device, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let via = match d.via {
            Via::Lan => "LAN",
            Via::Tailnet => "Tailscale",
        };
        let addr = d.addrs.first().map(|a| a.to_string()).unwrap_or_default();
        let (fill, status) = match &d.status {
            Status::Online => (Some(t.done.0), "Online".to_string()),
            Status::Connecting => (Some(t.working.0), "Connecting…".into()),
            Status::Offline { since } => (
                None,
                format!("Offline · last seen {} s ago", since.elapsed().as_secs()),
            ),
            Status::Available | Status::NeedsCode { .. } => (None, String::new()),
        };
        let sub = match (&d.trusted_via, &d.status) {
            (Some(v), _) => format!(
                "{via} · {addr} · {}",
                if v == "tailnet" {
                    "trusted through your Tailscale account"
                } else {
                    "paired with a code"
                }
            ),
            (None, _) => format!("{via} · {addr}"),
        };
        let id = d.id.clone();
        let action: AnyElement = match &d.status {
            Status::Available if d.trusted_via.is_none() => {
                button(&t, format!("add-{id}"), "Add", true)
                    .on_click(cx.listener(move |v, _, _, _| v.send(Cmd::Add(id.clone()))))
                    .into_any_element()
            }
            Status::NeedsCode { .. } => div().into_any_element(),
            _ => div()
                .flex()
                .items_center()
                .gap(px(space::S3))
                .child(div().ty(LABEL).text_color(t.quiet).child(status))
                .child(
                    button(&t, format!("remove-{id}"), "Remove", false)
                        .on_click(cx.listener(move |v, _, _, _| v.send(Cmd::Remove(id.clone())))),
                )
                .into_any_element(),
        };
        let mut el =
            div()
                .flex()
                .flex_col()
                .child(row(&t, square(&t, fill), d.name.clone(), sub, action));
        if let Status::NeedsCode { error } = &d.status {
            el = el.child(self.code_entry(d, error.clone(), cx));
        }
        el.into_any_element()
    }

    fn code_entry(&self, d: &Device, error: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let digits: Vec<char> = self.code.chars().collect();
        div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .px(px(space::S4))
            .pb(px(space::S3))
            .pl(px(space::S4 + size::SQUARE + space::S3))
            .child(
                div()
                    .ty(UI)
                    .text_color(t.quiet)
                    .child(format!("Enter the 6-digit code shown on {}.", d.name)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .children((0..6).map(|i| {
                        let ch = digits.get(i).map(|c| c.to_string()).unwrap_or_default();
                        let active = i == digits.len();
                        div()
                            .w(px(30.))
                            .h(px(38.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(radius::MD))
                            .border_1()
                            .border_color(if active { t.ink } else { t.hairline })
                            .bg(t.surface)
                            .ty(DISPLAY_COMPACT)
                            .child(ch)
                    }))
                    .child(div().w(px(space::S2)))
                    .child(
                        button(&t, "pair".into(), "Pair", self.code.len() == 6)
                            .on_click(cx.listener(|v, _, _, _| v.submit_code())),
                    ),
            )
            .children(error.map(|e| div().ty(LABEL).text_color(t.needs.0).child(e)))
            .into_any_element()
    }

    fn composer(&self, connected: &[Device], cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let online: Vec<&Device> = connected
            .iter()
            .filter(|d| d.status == Status::Online)
            .collect();
        let chips: Vec<AnyElement> = online
            .iter()
            .map(|d| {
                let id = d.id.clone();
                let on = self.target.as_deref() == Some(d.id.as_str());
                div()
                    .id(SharedString::from(format!("chip-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .px(px(space::S2))
                    .h(px(size::CONTROL_SM))
                    .rounded(px(radius::SM))
                    .border_1()
                    .border_color(if on { t.ink } else { t.hairline })
                    .bg(if on { t.ink.opacity(0.06) } else { t.raised })
                    .ty(LABEL)
                    .cursor_pointer()
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.target = Some(id.clone());
                        cx.notify();
                    }))
                    .child(square(&t, Some(t.done.0)))
                    .child(d.name.clone())
                    .into_any_element()
            })
            .collect();
        let replies: Vec<AnyElement> = self
            .snap
            .replies
            .iter()
            .rev()
            .map(|r| {
                let meta = match r.first {
                    Some(f) if r.done => format!(
                        "{} · first text after {} ms · done",
                        r.device,
                        f.as_millis()
                    ),
                    Some(f) => format!("{} · first text after {} ms", r.device, f.as_millis()),
                    None => format!("{} · sending…", r.device),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .pt(px(space::S3))
                    .border_t_1()
                    .border_color(t.hairline)
                    .child(div().ty(LABEL).text_color(t.quiet).child(meta))
                    .child(
                        div()
                            .ty(UI)
                            .font_weight(FontWeight::MEDIUM)
                            .child(r.prompt.clone()),
                    )
                    .child(div().ty(READING).child(r.text.clone()))
                    .into_any_element()
            })
            .collect();
        let has_target = self.target.is_some();
        div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(div().ty(LABEL).text_color(t.quiet).child("Run a prompt"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .p(px(space::S4))
                    .rounded(px(radius::LG))
                    .border_1()
                    .border_color(t.hairline)
                    .bg(t.raised)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(px(space::S2))
                            .child(div().ty(LABEL).text_color(t.quiet).child("Run on"))
                            .children(chips)
                            .when(online.is_empty(), |d| {
                                d.child(
                                    div()
                                        .ty(LABEL)
                                        .text_color(t.faint)
                                        .child("No machines online"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(size::CONTROL + 4.))
                                    .px(px(space::S3))
                                    .flex()
                                    .items_center()
                                    .rounded(px(radius::MD))
                                    .border_1()
                                    .border_color(t.hairline)
                                    .bg(t.surface)
                                    .ty(UI)
                                    .text_color(if self.prompt.is_empty() {
                                        t.faint
                                    } else {
                                        t.ink
                                    })
                                    .child(if self.prompt.is_empty() {
                                        "Ask the agent on that machine…".to_string()
                                    } else {
                                        format!("{}▏", self.prompt)
                                    }),
                            )
                            .child(
                                button(
                                    &t,
                                    "send".into(),
                                    "Send",
                                    has_target && !self.prompt.is_empty(),
                                )
                                .on_click(cx.listener(|v, _, _, _| v.submit_prompt())),
                            ),
                    )
                    .children(replies),
            )
            .into_any_element()
    }
}

fn square(t: &Theme, fill: Option<Hsla>) -> Div {
    let d = div().size(px(size::SQUARE)).flex_none().rounded(px(2.));
    match fill {
        Some(c) => d.bg(c),
        None => d.border_1().border_color(t.faint),
    }
}

fn row(t: &Theme, mark: Div, name: String, sub: String, action: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(space::S3))
        .px(px(space::S4))
        .py(px(space::S3))
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().ty(UI).font_weight(FontWeight::MEDIUM).child(name))
                .child(div().ty(LABEL).text_color(t.quiet).child(sub)),
        )
        .child(action)
        .into_any_element()
}

fn section(t: &Theme, title: &str, rows: Vec<AnyElement>, empty: Option<&str>) -> AnyElement {
    let n = rows.len();
    div()
        .flex()
        .flex_col()
        .gap(px(space::S2))
        .child(div().ty(LABEL).text_color(t.quiet).child(title.to_string()))
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(px(radius::LG))
                .border_1()
                .border_color(t.hairline)
                .bg(t.raised)
                .when(n == 0, |d| {
                    d.child(
                        div()
                            .px(px(space::S4))
                            .py(px(space::S3))
                            .ty(UI)
                            .text_color(t.faint)
                            .child(empty.unwrap_or_default().to_string()),
                    )
                })
                .children(rows.into_iter().enumerate().map(|(i, r)| {
                    div()
                        .when(i > 0, |d| d.border_t_1().border_color(t.hairline))
                        .child(r)
                })),
        )
        .into_any_element()
}

fn button(t: &Theme, id: String, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .px(px(space::S3))
        .h(px(size::CONTROL_SM + 4.))
        .flex()
        .items_center()
        .rounded(px(radius::MD))
        .ty(UI)
        .cursor_pointer()
        .when(primary, |d| d.bg(t.ink).text_color(t.surface))
        .when(!primary, |d| {
            d.border_1()
                .border_color(t.hairline)
                .text_color(t.quiet)
                .hover(|s| s.text_color(t.ink))
        })
        .child(label)
}
