//! `--devices`: Settings › Devices, driving real daemons on this machine
//! that stand in for other machines. One is on the LAN (pairs with a code),
//! one is on the same Tailscale account (simulated; trusted with no code).
//!
//! `--devices --demo` plays a scripted walkthrough for recording;
//! `--devices --demo away` plays machines going to sleep, crashing and
//! removing this Mac, with stills written to `$STILLS` if set.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;
use spike_link::devices::{self, Cmd, Device, Devices, ReplyState, Snapshot, Status, Via};
use spike_link::proto::now_ms;

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

    fn signal(&self, sig: &str) {
        if let Some(c) = &self.child {
            let _ = Command::new("kill")
                .args([sig, &c.id().to_string()])
                .status();
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
    away: bool,
    stills: Option<PathBuf>,
    /// When the current demo step began, in seconds.
    mark: f32,
    /// The machine whose details are open.
    open: Option<String>,
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(7481)
}

impl DevicesView {
    pub fn new(dark: bool, demo: Option<String>, cx: &mut Context<Self>) -> Self {
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
                    "--agent-ms".into(),
                    "90".into(),
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
            away: demo.as_deref() == Some("away"),
            demo: demo.map(|_| (Instant::now(), 0)),
            stills: std::env::var_os("STILLS").map(PathBuf::from),
            mark: 0.,
            open: None,
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
        if self
            .snap
            .devices
            .iter()
            .any(|d| d.id == id && !d.status.online())
        {
            return;
        }
        if self.prompt.trim().is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.prompt);
        self.send(Cmd::Prompt(id, text));
    }

    fn key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        // A scripted walkthrough types for itself; real keys would land in
        // the recording.
        if self.demo.is_some() {
            return;
        }
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

    fn still(&self, name: &str) {
        if let Some(dir) = &self.stills {
            crate::demo::still(dir.join(format!("{name}.png")));
        }
    }

    /// The scripted walkthrough: what a person would do, step by step.
    fn step_demo(&mut self, window: &mut Window) {
        let Some((start, step)) = self.demo else {
            return;
        };
        if step == 0 && start.elapsed().as_millis() < 50 {
            crate::specimen::reveal::hide_cursor();
        }
        if self.away {
            self.step_away(window);
            return;
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

    /// Machines going away: sleep mid-reply, a crash, and studio removing
    /// this Mac. Each step waits for what a person would wait to see.
    fn step_away(&mut self, window: &mut Window) {
        let Some((start, step)) = self.demo else {
            return;
        };
        let t = start.elapsed().as_secs_f32();
        let mark = self.mark;
        let since = t - mark;
        let status = |v: &Self, name: &str| v.by_name(name).map(|d| d.status);
        let online = |v: &Self, name: &str| status(v, name) == Some(Status::Online);
        let mut next = step;
        match step {
            0 if self.by_name("build-box").is_some() && self.by_name("studio").is_some() => {
                next = 50;
            }
            50 if since >= 0.6 => {
                self.still("away-0-found");
                next = 51;
            }
            51 if since >= 0.4 => {
                if let Some(d) = self.by_name("build-box") {
                    self.send(Cmd::Add(d.id));
                }
                next = 1;
            }
            1 if since >= 0.6 => {
                if let Some(d) = self.by_name("studio") {
                    self.send(Cmd::Add(d.id));
                }
                next = 2;
            }
            2 if self.pairing().is_some() && self.machines[0].code().len() == 6 => {
                // The person reads the code off studio's screen.
                let code = self.machines[0].code();
                let typed = (since / 0.15) as usize;
                let before = self.code.len();
                self.code = code.chars().take(typed.min(6)).collect();
                if before == 3 && self.code.len() == 4 {
                    self.still("away-0-code");
                }
                if typed >= 8 {
                    self.submit_code();
                    next = 3;
                }
            }
            3 if online(self, "studio")
                && online(self, "build-box")
                && self.snap.devices.iter().all(|d| d.health.is_some()) =>
            {
                next = 4;
            }
            4 if since >= 0.2 => {
                self.open = self.by_name("studio").map(|d| d.id);
                next = 40;
            }
            40 if since >= 1.0 => {
                self.still("away-1-details");
                next = 5;
            }
            5 if since >= 1.0 => {
                self.target = self.by_name("studio").map(|d| d.id);
                let text = "Write up the auth changes for the release notes";
                let typed = ((since - 1.0) / 0.02) as usize;
                self.prompt = text.chars().take(typed).collect();
                if typed > text.len() + 4 {
                    self.submit_prompt();
                    next = 6;
                }
            }
            6 if self.snap.replies.last().is_some_and(|r| r.text.len() > 90) => {
                self.machines[0].signal("-USR1");
                next = 7;
            }
            7 if since >= 1.5 => {
                self.still("away-2-asleep");
                next = 8;
            }
            8 if since >= 3.0 => {
                self.machines[0].signal("-CONT");
                next = 9;
            }
            9 if online(self, "studio")
                && self
                    .snap
                    .replies
                    .last()
                    .is_some_and(|r| r.state == ReplyState::Done) =>
            {
                next = 10;
            }
            10 if since >= 1.0 => {
                self.open = None;
                self.still("away-3-woke");
                next = 11;
            }
            11 if since >= 1.0 => {
                self.machines[1].stop();
                next = 12;
            }
            12 if matches!(status(self, "build-box"), Some(Status::Reconnecting { .. })) => {
                next = 13;
            }
            13 if since >= 0.8 => {
                self.open = self.by_name("build-box").map(|d| d.id);
                self.still("away-4-reconnecting");
                next = 14;
            }
            14 if matches!(status(self, "build-box"), Some(Status::Offline { .. })) => {
                next = 15;
            }
            15 if since >= 2.0 => {
                self.still("away-5-offline");
                next = 16;
            }
            16 if since >= 1.0 => {
                self.machines[1].start();
                next = 17;
            }
            17 if online(self, "build-box") => {
                next = 18;
            }
            18 if since >= 2.0 => {
                self.open = None;
                // The person at studio removes this Mac there.
                let me = self.snap.me.1.clone();
                let _ = spike_link::identity::Trust::update(&self.machines[0].state, |p| {
                    p.remove(&me);
                });
                next = 19;
            }
            19 if status(self, "studio") == Some(Status::Removed) => {
                next = 20;
            }
            20 if since >= 1.2 => {
                self.still("away-6-removed");
                next = 21;
            }
            // Held open until the recording (50 s) has finished, so it
            // never captures what's behind the window.
            21 if t >= 50.5 => {
                window.remove_window();
                next = 22;
            }
            _ => {}
        }
        if next != step {
            self.mark = t;
        }
        self.demo = Some((start, next));
        window.request_animation_frame();
    }
}

impl Render for DevicesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.step_demo(window);
        let t = self.theme;
        let width = window.viewport_size().width;
        let narrow = width < px(560.);
        let stack = width < px(380.);
        let mine: Vec<Device> = self
            .snap
            .devices
            .iter()
            .filter(|d| d.trusted_via.is_some() || d.status == Status::Removed)
            .cloned()
            .collect();
        let found: Vec<Device> = self
            .snap
            .devices
            .iter()
            .filter(|d| d.trusted_via.is_none() && d.status != Status::Removed)
            .cloned()
            .collect();
        if self.target.is_none() {
            self.target = mine
                .iter()
                .find(|d| d.status.online())
                .map(|d| d.id.clone());
        }
        let (name, id) = self.snap.me.clone();
        let this_mac = line(
            &t,
            square(&t, Some(t.done.0)),
            name,
            format!("This Mac · {}", short_id(&id)),
            div().into_any_element(),
        )
        .into_any_element();
        let mine_rows: Vec<AnyElement> = mine.iter().map(|d| self.machine(d, stack, cx)).collect();
        let found_rows: Vec<AnyElement> = found.iter().map(|d| self.nearby(d, cx)).collect();
        let gutter = if narrow { space::S4 } else { space::S8 };
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
                    .max_w(px(size::COLUMN))
                    .px(px(gutter))
                    .pt(px(space::S8))
                    .pb(px(space::S12))
                    .flex()
                    .flex_col()
                    .gap(px(space::S8))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .child(div().ty(DISPLAY_COMPACT).child("Devices"))
                            .child(div().ty(UI).text_color(t.quiet).child(
                                "Machines that can run your agents. dent finds them on your network and your Tailscale account.",
                            )),
                    )
                    .child(section(&t, "This Mac", vec![this_mac], None))
                    .child(section(
                        &t,
                        "Your machines",
                        mine_rows,
                        Some("None yet. Add one from the machines found below."),
                    ))
                    .child(section(
                        &t,
                        "Found nearby",
                        found_rows,
                        Some("No other machines found yet. Machines running dent on your network or Tailscale account show up here."),
                    ))
                    .child(self.composer(&mine, cx)),
            )
    }
}

impl DevicesView {
    /// One of your machines: a calm line, with details when opened.
    fn machine(&self, d: &Device, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let now = now_ms();
        let via = via_name(d);
        let (fill, status) = match &d.status {
            Status::Online => (
                Some(t.done.0),
                match d.rtt_ms {
                    Some(0) => format!("Online over {via} · under 1 ms"),
                    Some(r) => format!("Online over {via} · {r} ms"),
                    None => format!("Online over {via}"),
                },
            ),
            Status::Connecting => (Some(t.working.0), "Connecting…".into()),
            Status::Reconnecting { .. } => (Some(t.working.0), "Reconnecting…".into()),
            Status::Asleep { since } => (
                Some(t.idle.0),
                format!("Asleep · went to sleep {}", ago(now, *since)),
            ),
            Status::Stopped {
                restarting: true, ..
            } => (Some(t.working.0), "Restarting dent…".into()),
            Status::Stopped { since, .. } => (
                None,
                format!("dent isn't running · quit {}", ago(now, *since)),
            ),
            Status::Offline { since } if *since > 0 => {
                (None, format!("Offline · last seen {}", ago(now, *since)))
            }
            Status::Offline { .. } => (None, "Offline".into()),
            Status::Removed => (Some(t.needs.0), "Removed this Mac".into()),
            Status::NeedsUpdate { .. } => (Some(t.needs.0), "Needs a dent update".into()),
            Status::Available | Status::NeedsCode { .. } => (None, String::new()),
        };
        let removed = d.status == Status::Removed;
        let open = !removed && self.open.as_deref() == Some(d.id.as_str());
        let id = d.id.clone();
        let header = div()
            .id(SharedString::from(format!("m-{id}")))
            .cursor_pointer()
            .hover(|s| s.bg(t.hover()))
            .when(open, |el| el.bg(t.selected()))
            .on_click(cx.listener(move |v, _, _, cx| {
                v.open = if v.open.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id.clone())
                };
                cx.notify();
            }))
            .child(line(
                &t,
                square(&t, fill),
                d.name.clone(),
                status,
                div()
                    .ty(UI)
                    .text_color(t.faint)
                    .child(match (removed, open) {
                        (true, _) => "",
                        (_, true) => "Hide",
                        _ => "Details",
                    })
                    .into_any_element(),
            ));
        let mut el = div().flex().flex_col().child(header);
        // States that need you get a card, open or not.
        match &d.status {
            Status::Removed => {
                let (a, b) = (d.id.clone(), d.id.clone());
                el = el.child(callout(
                    &t,
                    format!("This Mac can't run anything on {n} now. To use it again, add it with a new code shown on {n}.", n = d.name),
                    vec![
                        button(&t, format!("again-{a}"), "Add again", true)
                            .on_click(cx.listener(move |v, _, _, _| v.send(Cmd::Add(a.clone()))))
                            .into_any_element(),
                        button(&t, format!("dismiss-{b}"), "Dismiss", false)
                            .on_click(cx.listener(move |v, _, _, _| v.send(Cmd::Dismiss(b.clone()))))
                            .into_any_element(),
                    ],
                ));
            }
            Status::NeedsUpdate { theirs, min } => {
                el = el.child(callout(
                    &t,
                    format!(
                        "{} runs a dent that can't talk to this one (it speaks version {theirs} and needs {min} or newer; this Mac speaks {}). Update dent on the older machine.",
                        d.name,
                        spike_link::proto::PROTO
                    ),
                    Vec::new(),
                ));
            }
            _ => {}
        }
        if open {
            el = el.child(self.details(d, narrow, cx));
        }
        el.into_any_element()
    }

    fn details(&self, d: &Device, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let now = now_ms();
        let gb = |b: u64| b as f64 / 1e9;
        let mut rows: Vec<(&str, Vec<String>)> = Vec::new();
        let addr = d.addr().map(|a| a.ip().to_string()).unwrap_or_default();
        rows.push(("Connection", vec![format!("{} · {addr}", via_name(d))]));
        rows.push((
            "Trusted",
            vec![match (d.trusted_via.as_deref(), d.paired_at) {
                (Some("tailnet"), _) => "Your Tailscale account".into(),
                (Some(_), Some(at)) => format!("Paired with a code {}", ago(now, at)),
                (Some(_), None) => "Paired with a code".into(),
                (None, _) => "Not any more".into(),
            }],
        ));
        if let Some(i) = &d.info {
            let mut sys = vec![format!("{} {}", i.os, i.os_version)];
            if !i.model.is_empty() {
                sys.push(format!("{}, {} cores", i.model, i.cores));
            }
            rows.push(("System", sys));
            rows.push((
                "Agents",
                if i.agents.is_empty() {
                    vec!["None installed".into()]
                } else {
                    i.agents
                        .iter()
                        .map(|a| format!("{} {}", a.name, a.version.clone().unwrap_or_default()))
                        .collect()
                },
            ));
        }
        match (&d.health, d.status.online()) {
            (Some((h, _)), true) => {
                rows.push((
                    "Activity",
                    vec![match h.running {
                        0 => "No agents running".into(),
                        1 => "1 agent running".into(),
                        n => format!("{n} agents running"),
                    }],
                ));
                let cores = d.info.as_ref().map(|i| i.cores).unwrap_or(0).max(1);
                rows.push((
                    "Load",
                    vec![format!(
                        "{:.0}% of {cores} cores",
                        (h.load / cores as f32 * 100.).min(999.)
                    )],
                ));
                rows.push((
                    "Memory",
                    vec![format!(
                        "{:.0} of {:.0} GB in use",
                        gb(h.mem_used),
                        gb(h.mem_total)
                    )],
                ));
                rows.push(("Disk", vec![format!("{:.0} GB free", gb(h.disk_free))]));
                if let Some(b) = &h.battery {
                    rows.push((
                        "Power",
                        vec![if b.on_battery {
                            format!("On battery, {}%", b.percent)
                        } else {
                            format!("Plugged in, {}%", b.percent)
                        }],
                    ));
                }
            }
            // Asleep, offline and quit already say when in the status line.
            _ => {
                if let (Status::Reconnecting { .. } | Status::Connecting, Some(at)) =
                    (&d.status, d.last_seen)
                {
                    rows.push(("Last seen", vec![ago(now, at)]));
                }
            }
        }
        let label_w = px(88.);
        let grid = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .children(rows.into_iter().map(|(k, v)| {
                let label = div()
                    .ty(UI)
                    .text_color(t.quiet)
                    .flex_none()
                    .when(!narrow, |e| e.w(label_w))
                    .child(k);
                let value = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .children(v.into_iter().map(|l| div().ty(UI).child(l)));
                if narrow {
                    div().flex().flex_col().child(label).child(value)
                } else {
                    div().flex().gap(px(space::S3)).child(label).child(value)
                }
            }));
        let id = d.id.clone();
        let can_wake = matches!(d.status, Status::Asleep { .. } | Status::Offline { .. })
            && d.info.as_ref().is_some_and(|i| !i.macs.is_empty())
            && d.via() == Via::Lan;
        let (w, r) = (id.clone(), id.clone());
        let actions = div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .when(can_wake, |el| {
                el.child(
                    button(&t, format!("wake-{id}"), "Wake", false)
                        .on_click(cx.listener(move |v, _, _, _| v.send(Cmd::Wake(w.clone())))),
                )
            })
            .child(
                button(&t, format!("remove-{id}"), "Remove", false)
                    .on_click(cx.listener(move |v, _, _, _| v.send(Cmd::Remove(r.clone())))),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(space::S4))
            .pl(px(indent()))
            .pr(px(space::S4))
            .pt(px(space::S3))
            .pb(px(space::S4))
            .bg(t.selected())
            .child(grid)
            .children(
                d.note
                    .clone()
                    .map(|n| div().ty(UI).text_color(t.quiet).child(n)),
            )
            .child(actions)
            .into_any_element()
    }

    /// A machine found but not added.
    fn nearby(&self, d: &Device, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let id = d.id.clone();
        let place = match d.via() {
            Via::Lan => "On your network",
            Via::Tailnet => "On your Tailscale account",
        };
        let action: AnyElement = match d.status {
            Status::NeedsCode { .. } => div().into_any_element(),
            _ => button(&t, format!("add-{id}"), "Add", true)
                .on_click(cx.listener(move |v, _, _, _| v.send(Cmd::Add(id.clone()))))
                .into_any_element(),
        };
        let mut el = div().flex().flex_col().child(line(
            &t,
            square(&t, None),
            d.name.clone(),
            place.into(),
            action,
        ));
        if let Some(n) = &d.same_name_as {
            el = el.child(callout(
                &t,
                format!("This has the same name as {n}, which is offline. If {n} was reinstalled, add this one and remove the old one."),
                Vec::new(),
            ));
        }
        if let Some(n) = &d.note {
            el = el.child(
                div()
                    .pl(px(indent()))
                    .pr(px(space::S4))
                    .pb(px(space::S3))
                    .ty(UI)
                    .text_color(t.quiet)
                    .child(n.clone()),
            );
        }
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
            .gap(px(space::S3))
            .pl(px(indent()))
            .pr(px(space::S4))
            .pb(px(space::S4))
            .child(
                div()
                    .ty(UI)
                    .text_color(t.quiet)
                    .child(format!("Type the 6-digit code shown on {}.", d.name)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::S2))
                    .child(div().flex().gap(px(space::S1H)).children((0..6).map(|i| {
                        let ch = digits.get(i).map(|c| c.to_string()).unwrap_or_default();
                        let active = i == digits.len();
                        div()
                            .w(px(32.))
                            .h(px(40.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(radius::MD))
                            .border_1()
                            .border_color(if active { t.ink } else { t.hairline })
                            .bg(t.surface)
                            .ty(DISPLAY_COMPACT)
                            .child(ch)
                    })))
                    .child(
                        button(&t, "pair".into(), "Pair", self.code.len() == 6)
                            .on_click(cx.listener(|v, _, _, _| v.submit_code())),
                    ),
            )
            .children(error.map(|e| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .child(square(&t, Some(t.needs.0)))
                    .child(div().ty(UI).child(e))
            }))
            .into_any_element()
    }

    fn composer(&self, mine: &[Device], cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let paired: Vec<&Device> = mine.iter().filter(|d| d.trusted_via.is_some()).collect();
        let chips: Vec<AnyElement> = paired
            .iter()
            .map(|d| {
                let id = d.id.clone();
                let away = d.status.away(&d.name).is_some();
                let on = !away && self.target.as_deref() == Some(d.id.as_str());
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
                    .when(on, |c| c.bg(t.selected()))
                    .ty(UI)
                    .text_color(if away { t.faint } else { t.ink })
                    .when(!away, |c| {
                        c.cursor_pointer()
                            .hover(|s| s.bg(t.hover()))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.target = Some(id.clone());
                                cx.notify();
                            }))
                    })
                    .child(square(&t, (!away).then_some(t.done.0)))
                    .child(d.name.clone())
                    .into_any_element()
            })
            .collect();
        let target_away = self
            .target
            .as_ref()
            .and_then(|id| paired.iter().find(|d| &d.id == id))
            .and_then(|d| d.status.away(&d.name));
        let replies: Vec<AnyElement> = self
            .snap
            .replies
            .iter()
            .enumerate()
            .rev()
            .map(|(i, r)| {
                let first = r
                    .first
                    .map(|f| format!(" · first text in {f} ms"))
                    .unwrap_or_default();
                let (fill, meta) = match &r.state {
                    ReplyState::Sending => {
                        (Some(t.working.0), format!("{} · sending…", r.device_name))
                    }
                    ReplyState::Running => (Some(t.working.0), format!("{}{first}", r.device_name)),
                    ReplyState::Paused(why) => (
                        Some(t.idle.0),
                        format!("Paused · {why}. It picks up where it left off."),
                    ),
                    ReplyState::Done => (Some(t.done.0), format!("{}{first}", r.device_name)),
                    ReplyState::Interrupted(why) => {
                        (Some(t.needs.0), format!("Interrupted · {why}"))
                    }
                    ReplyState::NotSent(why) => (Some(t.needs.0), format!("Not sent · {why}")),
                };
                let retry = matches!(r.state, ReplyState::NotSent(_)) && !why_away(&r.state);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .pt(px(space::S4))
                    .border_t_1()
                    .border_color(t.hairline)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S1H))
                            .child(square(&t, fill))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .ty(UI)
                                    .text_color(t.quiet)
                                    .child(meta),
                            )
                            .when(retry, |el| {
                                el.child(
                                    button(&t, format!("retry-{i}"), "Retry", false).on_click(
                                        cx.listener(move |v, _, _, _| v.send(Cmd::Retry(i))),
                                    ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .ty(UI)
                            .font_weight(FontWeight::MEDIUM)
                            .child(r.prompt.clone()),
                    )
                    .when(!r.text.is_empty(), |el| {
                        el.child(div().ty(READING).child(r.text.clone()))
                    })
                    .into_any_element()
            })
            .collect();
        let can_send = self.target.is_some() && target_away.is_none() && !self.prompt.is_empty();
        let body = div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .p(px(space::S4))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(space::S2))
                    .child(div().ty(UI).text_color(t.quiet).child("Run on"))
                    .children(chips)
                    .when(paired.is_empty(), |d| {
                        d.child(
                            div()
                                .ty(UI)
                                .text_color(t.faint)
                                .child("Add a machine first"),
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
                            .child(div().truncate().child(if self.prompt.is_empty() {
                                "Ask the agent on that machine…".to_string()
                            } else {
                                format!("{}▏", self.prompt)
                            })),
                    )
                    .child(
                        button(&t, "send".into(), "Send", can_send)
                            .on_click(cx.listener(|v, _, _, _| v.submit_prompt())),
                    ),
            )
            .children(target_away.map(|why| {
                div().ty(UI).text_color(t.quiet).child(format!(
                    "{why}. Prompts aren't held for later, so send it when it's back."
                ))
            }))
            .children(replies);
        div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(label(&t, "Try a prompt"))
            .child(
                div()
                    .rounded(px(radius::LG))
                    .border_1()
                    .border_color(t.hairline)
                    .bg(t.raised)
                    .child(body),
            )
            .into_any_element()
    }
}

/// A prompt refused because the machine was away has nothing to retry
/// until it's back; one that went unconfirmed does.
fn why_away(s: &ReplyState) -> bool {
    matches!(s, ReplyState::NotSent(w) if w.ends_with("Nothing was sent."))
}

fn via_name(d: &Device) -> &'static str {
    match d.via() {
        Via::Lan => "LAN",
        Via::Tailnet => "Tailscale",
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(6).collect()
}

/// "12 s ago", "3 min ago", "2 h ago", "4 days ago".
fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then) / 1000;
    match s {
        0..5 => "just now".into(),
        5..60 => format!("{s} s ago"),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

/// Where text starts in a row: past the gutter and the status square.
fn indent() -> f32 {
    space::S4 + size::SQUARE + space::S3
}

fn square(t: &Theme, fill: Option<Hsla>) -> Div {
    let d = div().size(px(size::SQUARE)).flex_none().rounded(px(2.));
    match fill {
        Some(c) => d.bg(c),
        None => d.border_1().border_color(t.faint),
    }
}

/// A list row: status square, name over one quiet line, and an action.
fn line(t: &Theme, mark: Div, name: String, sub: String, action: AnyElement) -> Div {
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
                .child(
                    div()
                        .ty(UI)
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(name),
                )
                .child(div().ty(UI).text_color(t.quiet).truncate().child(sub)),
        )
        .child(div().flex_none().child(action))
}

/// A "needs you" card: the approval-card wash and edge, ink text.
fn callout(t: &Theme, text: String, actions: Vec<AnyElement>) -> AnyElement {
    div()
        .mx(px(space::S4))
        .ml(px(indent()))
        .mb(px(space::S3))
        .p(px(space::S3))
        .flex()
        .flex_col()
        .gap(px(space::S3))
        .rounded(px(radius::MD))
        .border_1()
        .border_color(t.edge(t.needs.0))
        .bg(t.wash(t.needs.0))
        .child(div().ty(UI).child(text))
        .when(!actions.is_empty(), |el| {
            el.child(div().flex().gap(px(space::S2)).children(actions))
        })
        .into_any_element()
}

fn label(t: &Theme, text: &str) -> Div {
    div()
        .ty(LABEL)
        .text_color(t.quiet)
        .child(text.to_uppercase())
}

fn section(t: &Theme, title: &str, rows: Vec<AnyElement>, empty: Option<&str>) -> AnyElement {
    let n = rows.len();
    div()
        .flex()
        .flex_col()
        .gap(px(space::S2))
        .child(label(t, title))
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(px(radius::LG))
                .border_1()
                .border_color(t.hairline)
                .bg(t.raised)
                .overflow_hidden()
                .when(n == 0, |d| {
                    d.child(
                        div()
                            .px(px(space::S4))
                            .py(px(space::S4))
                            .ty(UI)
                            .text_color(t.quiet)
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
        .flex_none()
        .px(px(space::S3))
        .h(px(size::CONTROL))
        .flex()
        .items_center()
        .rounded(px(radius::MD))
        .ty(UI)
        .cursor_pointer()
        .when(primary, |d| d.bg(t.ink).text_color(t.surface))
        .when(!primary, |d| {
            // An ink edge, not a hairline, so it holds on the selected tint
            // in both themes.
            d.border_1()
                .border_color(t.ink.opacity(0.18))
                .text_color(t.ink)
                .hover(|s| s.bg(t.hover()))
        })
        .child(label)
}
