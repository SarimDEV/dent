//! Settings › Machines ([remote-plan.md], stage 1): every machine at a
//! glance, then each one's details, then who can use this one.
//!
//! - **The strip:** this Mac on the left; each connected machine placed by
//!   its real round trip on a log scale, so closer is faster; machines that
//!   aren't connected to one side. A live link carries a moving dot.
//! - **The rows:** shape, name, how it stands, its sessions, the way back
//!   when it's away (Retry), and its details on a click.
//! - **Add a machine:** your Tailscale's devices with one click each, or an
//!   address; a code when the machine asks for one.
//!
//! Words are fixed: Online, Asleep, Can't reach, Tailscale, LAN, This Mac.
//!
//! [remote-plan.md]: ../../docs/remote-plan.md

use std::collections::HashMap;

use dent_client_core::fleet;
use dent_client_core::machine::{Signal, State};
use dent_link::{Goodbye, Trust};
use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::{Dent, Waiting};
use crate::ds::{CODE, DISPLAY, LABEL, NUMERAL, Typed, UI, motion, radius, space};
use crate::glyph::{self, Shape};
use crate::machines::MachineView;
use crate::theme::{Status, Theme};
use crate::ui::{self, Arrive, From, Press, probe};

const STRIP_H: f32 = 210.;
/// Across the strip, as parts of its width: this Mac, the round-trip axis
/// (1 ms to 300 ms), the line before machines that aren't connected.
const HOME_X: f32 = 0.087;
const AXIS: (f32, f32) = (0.211, 0.678);
const AWAY_X: f32 = 0.733;
const NODE: f32 = 44.;
const HOME_NODE: f32 = 60.;
const ROW_H: f32 = 60.;

/// A device on this Mac's Tailscale account.
#[derive(Debug, Clone)]
pub struct Peer {
    pub name: String,
    /// Its MagicDNS name, without the final dot.
    pub dns: String,
    pub os: String,
    pub online: bool,
}

/// Add a machine, while open.
#[derive(Debug, Clone, Default)]
pub struct Adding {
    /// The machine being added: the tag it goes by until its welcome, then
    /// its id.
    pub tag: Option<String>,
    /// Your Tailscale's devices: `None` while looking, `Err` when Tailscale
    /// can't be asked.
    pub peers: Option<Result<Vec<Peer>, String>>,
}

/// How a machine stands, in the fixed words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Online,
    Connecting,
    Asleep,
    Unreachable,
    NeedsCode,
    /// Removed this device, wants an update, or trusts this device only on
    /// other routes: nothing happens until something changes.
    Blocked,
}

impl Standing {
    pub fn of(m: &MachineView) -> Self {
        match m.standing() {
            State::Online | State::Slow => Standing::Online,
            State::Off | State::Connecting | State::Reconnecting => Standing::Connecting,
            State::Away(Goodbye::Sleep) => Standing::Asleep,
            State::Away(Goodbye::Awake) => Standing::Connecting,
            State::Away(_) | State::Unreachable { .. } => Standing::Unreachable,
            State::NeedsCode { .. } => Standing::NeedsCode,
            State::Removed | State::Incompatible { .. } | State::TrustedElsewhere { .. } => {
                Standing::Blocked
            }
        }
    }

    fn connected(self) -> bool {
        matches!(self, Standing::Online)
    }

    /// The dot on its holder: none while online or finding out.
    fn dot(self, t: &Theme) -> Option<Hsla> {
        match self {
            Standing::Online | Standing::Connecting => None,
            Standing::Asleep => Some(t.waiting.0),
            _ => Some(t.idle.0),
        }
    }
}

/// What a route is called: Tailscale for the tailnet's addresses and names,
/// LAN for the rest.
pub fn route_kind(addr: &str) -> &'static str {
    let host = addr
        .rsplit_once(':')
        .map_or(addr, |(h, _)| h)
        .trim_matches(['[', ']']);
    let tailnet = host.ends_with(".ts.net")
        || host.starts_with("fd7a:115c:a1e0")
        || host
            .split('.')
            .collect::<Vec<_>>()
            .as_slice()
            .first()
            .and_then(|a| a.parse::<u8>().ok())
            .zip(host.split('.').nth(1).and_then(|b| b.parse::<u8>().ok()))
            .is_some_and(|(a, b)| a == 100 && (64..128).contains(&b));
    if tailnet { "Tailscale" } else { "LAN" }
}

/// A fingerprint in groups of four, as keys are read out.
fn grouped(id: &str) -> String {
    id.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where the round-trip axis puts `ms`, as a part of the strip's width.
fn axis_x(ms: u64) -> f32 {
    let t = ((ms.max(1) as f32).ln() / 300f32.ln()).clamp(0., 1.);
    AXIS.0 + t * (AXIS.1 - AXIS.0)
}

fn cubic(p0: (f32, f32), p3: (f32, f32), t: f32) -> (f32, f32) {
    let mx = (p0.0 + p3.0) / 2.;
    let (p1, p2) = ((mx, p0.1), (mx, p3.1));
    let u = 1. - t;
    let b = |a: f32, b: f32, c: f32, d: f32| {
        u * u * u * a + 3. * u * u * t * b + 3. * u * t * t * c + t * t * t * d
    };
    (b(p0.0, p1.0, p2.0, p3.0), b(p0.1, p1.1, p2.1, p3.1))
}

/// A round holder with the machine's shape, and its state's dot.
pub fn holder(shape: Shape, dot: Option<Hsla>, size: f32, faded: bool, t: &Theme) -> Div {
    let d = (size * 0.28).round();
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(t.raised)
        .border_1()
        .border_color(t.hairline)
        .flex()
        .items_center()
        .justify_center()
        .child(glyph::glyph(
            shape,
            if faded { t.faint } else { t.ink },
            (size * 0.5).round(),
        ))
        .children(dot.map(|c| {
            div()
                .absolute()
                .right(px(-1.))
                .bottom(px(-1.))
                .size(px(d))
                .rounded_full()
                .bg(c)
                .border_2()
                .border_color(t.surface)
        }))
}

/// One placed node of the strip.
struct Node {
    name: String,
    shape: Shape,
    standing: Standing,
    /// Part of the width, and pixels down.
    x: f32,
    y: f32,
    caption: String,
}

/// A shape for every machine: this one (`local`, its id) and each remote
/// one. One still being added has none yet: its address would take a shape
/// and move another machine off its own for as long as the sheet is open.
fn shapes_for(
    local: &str,
    machines: &[MachineView],
    kept: &HashMap<String, Shape>,
) -> HashMap<String, Shape> {
    // This machine goes by its id once its daemon has said it, and by the
    // shape it had last time until then.
    let mut kept = kept.clone();
    if !local.is_empty()
        && !kept.contains_key(local)
        && let Some(sh) = kept.get(THIS).copied()
    {
        kept.insert(local.to_string(), sh);
    }
    let local = if local.is_empty() { THIS } else { local };
    let mut s = glyph::assign_kept(
        std::iter::once(local).chain(
            machines
                .iter()
                .map(|m| m.id.as_str())
                .filter(|id| !id.starts_with('+')),
        ),
        &kept,
    );
    // This machine's own sessions have plain ids: "" is it too.
    if let Some(sh) = s.get(local).copied() {
        s.insert(String::new(), sh);
        s.insert(THIS.to_string(), sh);
    }
    s
}

/// This machine, in the kept shapes, whatever its id.
const THIS: &str = "this";

fn shapes_path() -> std::path::PathBuf {
    dent_remote::store::state_dir().join("shapes.json")
}

/// The shapes machines were given, kept so none changes shape when another
/// is added or between launches.
pub fn load_shapes() -> HashMap<String, Shape> {
    std::fs::read(shapes_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<HashMap<String, usize>>(&b).ok())
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, i)| glyph::SHAPES.get(i).map(|s| (k, *s)))
                .collect()
        })
        .unwrap_or_default()
}

impl Dent {
    /// A shape for every machine: this one and each remote one. A new
    /// machine's is kept from then on.
    pub fn shapes(&self) -> HashMap<String, Shape> {
        let mut kept = self.kept_shapes.borrow_mut();
        let s = shapes_for(&self.machine_id, &self.machines, &kept);
        let grew = s
            .iter()
            .any(|(k, v)| !k.is_empty() && kept.get(k) != Some(v));
        if grew {
            for (k, v) in &s {
                if !k.is_empty() {
                    kept.insert(k.clone(), *v);
                }
            }
            let index: HashMap<&String, usize> = kept
                .iter()
                .filter_map(|(k, v)| glyph::SHAPES.iter().position(|s| s == v).map(|i| (k, i)))
                .collect();
            if let Ok(json) = serde_json::to_vec(&index) {
                let _ = dent_remote::store::write_private(&shapes_path(), &json);
            }
        }
        s
    }

    /// The remote machines added: saved, not one still being added.
    pub fn added_machines(&self) -> impl Iterator<Item = &MachineView> {
        self.machines.iter().filter(|m| !m.id.starts_with('+'))
    }

    /// Whether there's more than this machine.
    pub fn has_machines(&self) -> bool {
        self.added_machines().next().is_some()
    }

    /// What machine `id` is called ("" is this one).
    pub fn machine_name(&self, id: &str) -> String {
        if id.is_empty() {
            return if self.machine.is_empty() {
                "This Mac".into()
            } else {
                self.machine.clone()
            };
        }
        self.machines
            .iter()
            .find(|m| m.id == id)
            .map_or_else(|| id.to_string(), |m| m.name().to_string())
    }

    /// The new-session machine chip's menu: this Mac, then each remote
    /// machine, saying what starting there would do now.
    pub fn machine_entries(&self) -> Vec<crate::chat::Entry> {
        use crate::chat::Entry;
        let pick = |id: String| -> Option<crate::chat::Act> {
            Some(std::rc::Rc::new(
                move |this: &mut Dent, _: &mut Window, cx: &mut Context<Dent>| {
                    this.set_machine(id.clone(), cx);
                },
            ))
        };
        let here = self.defaults.machine.as_str();
        let mut v = vec![Entry::row(
            "machine-this".into(),
            &self.machine_name(""),
            Some("This Mac".into()),
            here.is_empty(),
            pick(String::new()),
        )];
        for m in self.added_machines() {
            let (note, off): (String, Option<&'static str>) = match Standing::of(m) {
                Standing::Online => (
                    m.rtt
                        .map_or("Online".into(), |ms| format!("Online · {ms} ms")),
                    None,
                ),
                Standing::Connecting => ("Connecting…".into(), None),
                // A new session there waits on this Mac and starts when it's
                // back (decision 0063).
                Standing::Asleep => ("Asleep · starts when it wakes".into(), None),
                Standing::Unreachable => ("Can't reach · starts when it's back".into(), None),
                Standing::NeedsCode => (
                    "Needs a code".into(),
                    Some("Pair with it in Settings › Machines"),
                ),
                Standing::Blocked => ("Can't be used".into(), Some("See Settings › Machines")),
            };
            let mut e = Entry::row(
                format!("machine-{}", m.id),
                m.name(),
                Some(note),
                here == m.id,
                pick(m.id.clone()),
            );
            e.off = off;
            v.push(e);
        }
        v
    }

    /// Sessions on machine `id` ("" is this one), by status.
    fn sessions_on(&self, id: &str) -> Vec<(Status, usize)> {
        let mut n: Vec<(Status, usize)> = Vec::new();
        for s in self.sessions.iter().filter(|s| fleet::split(&s.id).0 == id) {
            if s.closed {
                continue;
            }
            match n.iter_mut().find(|(st, _)| *st == s.status) {
                Some((_, c)) => *c += 1,
                None => n.push((s.status, 1)),
            }
        }
        let order = [
            Status::Needs,
            Status::Working,
            Status::Waiting,
            Status::Done,
            Status::Idle,
        ];
        n.sort_by_key(|(s, _)| order.iter().position(|o| o == s));
        n
    }

    pub fn machines_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let shapes = self.shapes();
        let standings: Vec<Standing> = self.machines.iter().map(Standing::of).collect();
        let online = 1 + standings.iter().filter(|s| s.connected()).count();
        let asleep = standings.iter().filter(|s| **s == Standing::Asleep).count();
        let away = standings
            .iter()
            .filter(|s| {
                matches!(
                    s,
                    Standing::Unreachable | Standing::Blocked | Standing::NeedsCode
                )
            })
            .count();
        let total = 1 + self.machines.len();
        let mut summary = vec![format!(
            "{total} machine{}",
            if total == 1 { "" } else { "s" }
        )];
        if total > 1 {
            summary.push(format!("{online} online"));
            if asleep > 0 {
                summary.push(format!("{asleep} asleep"));
            }
            if away > 0 {
                summary.push(format!("{away} can't reach"));
            }
        }
        let head = div()
            .flex()
            .items_end()
            .gap(px(space::S2))
            .pb(px(space::S6))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(div().ty(DISPLAY).text_color(t.ink).child("Machines"))
                    .child(div().ty(UI).text_color(t.quiet).child(summary.join(" · "))),
            )
            .child(
                ui::button("pairing-code", "Pairing code", false, &t)
                    .relative()
                    .child(probe(&self.probes, "pairing-code"))
                    .on_press(cx.listener(|this, _, _, cx| {
                        this.ask(dent_protocol::Request::OpenPairing, Waiting::Pairing);
                        cx.notify();
                    })),
            )
            .child(
                ui::button("add-machine", "Add machine", true, &t)
                    .relative()
                    .child(ui::icon("plus", 13., t.surface))
                    .flex_row_reverse()
                    .child(probe(&self.probes, "add-machine"))
                    .on_press(cx.listener(|this, _, window, cx| this.open_adding(window, cx))),
            );
        let code = self.pairing_card(cx);
        let first = self.machines.is_empty();
        div()
            .relative()
            .flex()
            .flex_col()
            .child(head)
            .children(code)
            .child(self.strip(&shapes, first, window, cx))
            .child(div().h(px(space::S6)))
            .child(ui::section_label("Your machines", None, &t))
            .child(div().h(px(space::S2)))
            .child(self.machine_rows(&shapes, cx))
            .child(div().h(px(space::S6)))
            .child(ui::section_label("Who can use this Mac", None, &t))
            .child(div().h(px(space::S2)))
            .child(self.device_rows(cx))
            .into_any_element()
    }

    /// The code showing on this Mac, for another device to pair with.
    fn pairing_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.t;
        let (code, expires) = self.pairing.clone()?;
        let left_ms = expires.saturating_sub(crate::cards::now_ms());
        // Rounded up: the last seconds still say "1 more minute".
        let left = left_ms.div_ceil(60_000);
        // An expired code stays, faded, with a new one a click away: the
        // other device may still be waiting on it.
        let expired = left_ms == 0;
        // After its third character, whatever the daemon sent.
        let cut = code.char_indices().nth(3).map_or(code.len(), |(i, _)| i);
        let spaced = format!("{} {}", &code[..cut], &code[cut..]);
        Some(
            div()
                .mb(px(space::S6))
                .p(px(space::S4))
                .rounded(px(radius::XL))
                .bg(t.raised)
                .border_1()
                .border_color(t.hairline)
                .flex()
                .items_center()
                .gap(px(space::S4))
                .child(
                    div()
                        .font_family(crate::theme::mono())
                        .text_size(px(28.))
                        .line_height(px(32.))
                        .text_color(if expired { t.faint } else { t.ink })
                        .when(expired, |d| d.line_through())
                        .child(spaced),
                )
                .child(div().flex_1().ty(UI).text_color(t.quiet).child(if expired {
                    "This code has expired. Codes work for a few minutes, once.".to_string()
                } else {
                    format!(
                        "Type it on the other device. Works once, for {} more minute{}.",
                        left,
                        if left == 1 { "" } else { "s" }
                    )
                }))
                .when(expired, |d| {
                    d.child(
                        ui::button("pairing-new", "New code", true, &t).on_press(cx.listener(
                            |this, _, _, cx| {
                                this.ask(dent_protocol::Request::OpenPairing, Waiting::Pairing);
                                cx.notify();
                            },
                        )),
                    )
                })
                .child(ui::ghost("pairing-done", "Done", &t).on_press(cx.listener(
                    |this, _, _, cx| {
                        this.pairing = None;
                        cx.notify();
                    },
                )))
                .arrive(
                    SharedString::from(format!("pairing-{code}")),
                    From::Above,
                    motion::BASE,
                )
                .into_any_element(),
        )
    }

    /// Every machine at a glance.
    fn strip(
        &self,
        shapes: &HashMap<String, Shape>,
        first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let home_y = STRIP_H / 2. - 12.;
        let home_shape = shapes.get("").copied().unwrap_or(Shape::Circle);
        let mut nodes: Vec<Node> = Vec::new();
        let (mut near, mut far) = (0usize, 0usize);
        let far_n = self
            .machines
            .iter()
            .filter(|m| !Standing::of(m).connected())
            .count()
            .max(1);
        for m in &self.machines {
            let st = Standing::of(m);
            let shape = shapes.get(&m.id).copied().unwrap_or(Shape::Circle);
            let (x, y, caption) = if st.connected() {
                let ms = m.rtt.unwrap_or(1);
                let y = [54., 136., 96., 172.][near % 4];
                near += 1;
                (axis_x(ms), y, format!("{ms} ms"))
            } else {
                let step = ((STRIP_H - 110.) / far_n as f32).min(64.);
                let y = 82. + step * far as f32;
                far += 1;
                let words = match st {
                    Standing::Asleep => "Asleep",
                    Standing::Connecting => "Connecting…",
                    Standing::NeedsCode => "Needs a code",
                    _ => "Can't reach",
                };
                (AWAY_X + 0.045, y, words.to_string())
            };
            nodes.push(Node {
                name: m.name().to_string(),
                shape,
                standing: st,
                x,
                y,
                caption,
            });
        }
        let lines: Vec<(f32, f32, Standing)> =
            nodes.iter().map(|n| (n.x, n.y, n.standing)).collect();
        let still = crate::reduce_motion();
        let animate = !still
            && self.window_active
            && self.window_visible
            && lines.iter().any(|l| l.2.connected());
        let ink = t.ink;
        let (tick, hair, waiting) = (t.hairline.opacity(0.6), t.hairline, t.waiting.0);
        let links = canvas(
            |_, _, _| (),
            move |b, _, window: &mut Window, _: &mut App| {
                let (ox, oy) = (b.origin.x.as_f32(), b.origin.y.as_f32());
                let w = b.size.width.as_f32();
                let at = |x: f32, y: f32| point(px(ox + x), px(oy + y));
                if !first {
                    for ms in [1, 10, 100] {
                        let x = axis_x(ms) * w;
                        let mut p = PathBuilder::stroke(px(1.));
                        p.move_to(at(x, 18.));
                        p.line_to(at(x, STRIP_H - 34.));
                        if let Ok(p) = p.build() {
                            window.paint_path(p, tick);
                        }
                    }
                }
                let mut p = PathBuilder::stroke(px(1.));
                p.move_to(at(AWAY_X * w, 18.));
                p.line_to(at(AWAY_X * w, STRIP_H - 16.));
                if let Ok(p) = p.build() {
                    window.paint_path(p, hair);
                }
                let home = (HOME_X * w, home_y);
                let phase = (crate::cards::now_ms() % 1800) as f32 / 1800.;
                for (i, (x, y, st)) in lines.iter().enumerate() {
                    let end = (x * w - NODE / 2., *y);
                    let from = (home.0 + HOME_NODE / 2., home.1);
                    let (color, dash): (Hsla, Option<[Pixels; 2]>) = match st {
                        Standing::Online => (ink.opacity(0.3), None),
                        Standing::Asleep => (waiting.opacity(0.8), Some([px(5.), px(5.)])),
                        _ => continue,
                    };
                    let mut p = PathBuilder::stroke(px(1.3));
                    if let Some(d) = dash {
                        p = p.dash_array(&d);
                    }
                    p.move_to(at(from.0, from.1));
                    let mx = (from.0 + end.0) / 2.;
                    p.cubic_bezier_to(at(end.0, end.1), at(mx, from.1), at(mx, end.1));
                    if let Ok(p) = p.build() {
                        window.paint_path(p, color);
                    }
                    // A live link carries a dot from this Mac out to it.
                    if *st == Standing::Online && !still {
                        let k = (phase + i as f32 * 0.37).fract();
                        let (dx, dy) = cubic(from, end, k);
                        let r = 2.;
                        window.paint_quad(
                            fill(
                                Bounds {
                                    origin: at(dx - r, dy - r),
                                    size: size(px(2. * r), px(2. * r)),
                                },
                                ink.opacity(0.7 * (1. - (2. * k - 1.).abs()).sqrt()),
                            )
                            .corner_radii(px(r)),
                        );
                    }
                }
                if animate {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let _ = window;
        let label = |text: String, mono: bool| {
            div()
                .when(mono, |d| d.ty(NUMERAL))
                .when(!mono, |d| d.ty(LABEL))
                .text_color(t.quiet)
                .whitespace_nowrap()
                .child(text)
        };
        let node_el = |n: &Node, size: f32, below: bool, i: usize| {
            let faded = matches!(n.standing, Standing::Unreachable | Standing::Blocked);
            let words = div()
                .flex()
                .flex_col()
                .when(below, |d| d.items_center())
                .child(
                    div()
                        .ty(UI)
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap()
                        .text_color(if faded { t.quiet } else { t.ink })
                        .child(n.name.clone()),
                )
                .child(label(n.caption.clone(), n.caption.ends_with(" ms")));
            let body = div()
                .ml(px(-size / 2.))
                .mt(px(-size / 2.))
                .flex()
                .when(below, |d| {
                    d.flex_col().items_center().ml(px(-size)).w(px(size * 2.))
                })
                .when(!below, |d| d.items_center())
                .gap(px(if below { space::S1H } else { space::S3 }))
                .child(holder(n.shape, n.standing.dot(&t), size, faded, &t))
                .child(words)
                .with_animation(
                    SharedString::from(format!("strip-node-{}-{i}", n.name)),
                    stagger(i),
                    |d, p| d.opacity(p),
                );
            // A round trip that changes glides the machine to its new place
            // (`spring.snappy`, slowed: it moves a fair way).
            div().absolute().top(px(n.y)).child(body).with_spring(
                SharedString::from(format!("strip-x-{}", n.name)),
                SpringAnimation::new(crate::anim::spring_config(0.5, 0.)).to(n.x),
                |d, x| d.left(relative(x)),
            )
        };
        let home = Node {
            name: if self.machine.is_empty() {
                "This Mac".into()
            } else {
                self.machine.clone()
            },
            shape: home_shape,
            standing: Standing::Online,
            x: HOME_X,
            y: home_y,
            caption: "This Mac".into(),
        };
        let ticks = (!first).then(|| {
            div()
                .absolute()
                .left_0()
                .top(px(STRIP_H - 24.))
                .w_full()
                .children([1u64, 10, 100].map(|ms| {
                    div().absolute().left(relative(axis_x(ms))).child(
                        div()
                            .ml(px(-18.))
                            .w(px(36.))
                            .flex()
                            .justify_center()
                            .child(label(format!("{ms} ms"), true)),
                    )
                }))
                .child(
                    div().absolute().left(relative(AXIS.0)).child(
                        div()
                            .ml(px(-110.))
                            .w(px(80.))
                            .flex()
                            .justify_end()
                            .child(label("Round trip".into(), false)),
                    ),
                )
        });
        let empty = first.then(|| {
            div()
                .absolute()
                .left(relative(AXIS.0))
                .w(relative(AXIS.1 - AXIS.0))
                .top_0()
                .h(px(STRIP_H - 20.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(space::S2))
                .child(
                    div()
                        .ty(crate::ds::DISPLAY_COMPACT)
                        .text_color(t.ink)
                        .child("Only this Mac so far"),
                )
                .child(
                    div()
                        .ty(UI)
                        .text_color(t.quiet)
                        .text_center()
                        .child("Machines you add appear here, placed by how fast they answer."),
                )
                .child(
                    ui::button("add-machine-empty", "Add machine", true, &t)
                        .relative()
                        .child(probe(&self.probes, "add-machine-empty"))
                        .on_press(cx.listener(|this, _, window, cx| this.open_adding(window, cx))),
                )
        });
        div()
            .id("machines-strip")
            .relative()
            .h(px(STRIP_H))
            .w_full()
            .rounded(px(radius::XL))
            .border_1()
            .border_color(t.hairline)
            .overflow_hidden()
            .child(links)
            .children(ticks)
            .child(
                div()
                    .absolute()
                    .left(relative(AWAY_X))
                    .top(px(22.))
                    .pl(px(space::S4))
                    .child(ui::section_label("Not connected", None, &t)),
            )
            .child(node_el(&home, HOME_NODE, true, 0))
            .children(
                nodes
                    .iter()
                    .enumerate()
                    .map(|(i, n)| node_el(n, NODE, false, i + 1)),
            )
            .children(empty)
            .child(probe(&self.probes, "machines-strip"))
            .into_any_element()
    }

    /// One row per machine: this Mac first, then each remote one.
    fn machine_rows(&self, shapes: &HashMap<String, Shape>, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let mut rows: Vec<AnyElement> = Vec::new();
        let local_shape = shapes.get("").copied().unwrap_or(Shape::Circle);
        let local_open = self.machine_open.as_deref() == Some("");
        let name = if self.machine.is_empty() {
            "This Mac".to_string()
        } else {
            self.machine.clone()
        };
        rows.push(self.machine_row(
            "",
            &name,
            Some("This Mac"),
            match &self.connection {
                Ok(()) => "Online".to_string(),
                Err(why) => why.clone(),
            },
            holder(local_shape, None, 32., false, &t),
            None,
            local_open,
            cx,
        ));
        if local_open {
            rows.push(self.details(vec![("Key", grouped(&self.machine_id), true)], None, cx));
        }
        for m in &self.machines {
            let st = Standing::of(m);
            let shape = shapes.get(&m.id).copied().unwrap_or(Shape::Circle);
            let faded = matches!(st, Standing::Unreachable | Standing::Blocked);
            let meta = match (st, m.standing()) {
                (Standing::Online, _) => format!(
                    "Online · {}",
                    m.via
                        .as_ref()
                        .map_or("Tailscale", |(addr, trust)| match trust {
                            Trust::Tailscale { .. } => "Tailscale",
                            _ => route_kind(addr),
                        })
                ),
                (Standing::Connecting, _) => "Connecting…".into(),
                (Standing::Asleep, _) => "Asleep".into(),
                (Standing::NeedsCode, _) => "Needs a code from it".into(),
                (_, State::Removed) => "Removed this device".into(),
                (_, State::Incompatible { .. }) => "Needs an update".into(),
                (_, State::TrustedElsewhere { .. }) => "Trusted on Tailscale only".into(),
                _ => "Can't reach".into(),
            };
            // Asleep or out of reach, with cards it can be woken through:
            // Wake sends the packets from here, then tries again.
            let wakes =
                matches!(st, Standing::Asleep | Standing::Unreachable) && !m.saved.wake.is_empty();
            let retry = (!st.connected() && st != Standing::Connecting).then(|| {
                let id = m.id.clone();
                let (cards, name) = (m.saved.wake.clone(), m.name().to_string());
                ui::inner_button(
                    SharedString::from(format!("retry-{}", m.id)),
                    if wakes { "Wake" } else { "Retry" },
                    false,
                    &t,
                )
                .on_press(cx.listener(move |this, _, _, cx| {
                    if wakes {
                        // Sent off the window's thread; it says how it went.
                        let (cards, name) = (cards.clone(), name.clone());
                        cx.spawn(async move |this, cx| {
                            let sent = cx
                                .background_executor()
                                .spawn(async move { dent_remote::wake::wake(&cards) })
                                .await;
                            let said = match sent {
                                Ok(_) => {
                                    format!("Sent {name} a wake-up; it works from the same network")
                                }
                                Err(e) => format!("Couldn't send {name} a wake-up: {e}"),
                            };
                            let _ = this.update(cx, |this, cx| {
                                this.toast(crate::app::ToastAction::Info, said);
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                    this.live.signal(Some(&id), Signal::Retry);
                    cx.notify();
                }))
                .into_any_element()
            });
            let open = self.machine_open.as_deref() == Some(m.id.as_str());
            rows.push(self.machine_row(
                &m.id,
                m.name(),
                None,
                meta,
                holder(shape, st.dot(&t), 32., faded, &t),
                retry,
                open,
                cx,
            ));
            if open {
                let mut d: Vec<(&str, String, bool)> = Vec::new();
                if let Some((_, trust)) = &m.via {
                    d.push((
                        "Trusted",
                        match trust {
                            Trust::Tailscale { .. } => {
                                "Same Tailscale account · no code needed".into()
                            }
                            Trust::Paired { .. } => "Paired with a code".into(),
                            Trust::Untrusted { .. } => "Not yet".into(),
                        },
                        false,
                    ));
                }
                let routes: Vec<String> = m
                    .saved
                    .all_routes()
                    .into_iter()
                    .map(|r| {
                        let using = m.via.as_ref().is_some_and(|(a, _)| *a == r) && st.connected();
                        format!(
                            "{} {r}{}",
                            route_kind(&r),
                            if using { " · in use" } else { "" }
                        )
                    })
                    .collect();
                if !routes.is_empty() {
                    d.push(("Routes", routes.join("\n"), true));
                }
                if !m.version.is_empty() {
                    d.push(("dent", m.version.clone(), false));
                }
                if !m.agents.is_empty() {
                    d.push((
                        "Agents",
                        m.agents
                            .iter()
                            .map(|a| match &a.version {
                                Some(v) => format!(
                                    "{} {}",
                                    a.name,
                                    v.split_whitespace().next().unwrap_or(v)
                                ),
                                None => a.name.clone(),
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                        false,
                    ));
                }
                d.push(("Key", grouped(&m.id), true));
                rows.push(self.details(d, Some(m.id.clone()), cx));
            }
        }
        let n = rows.len();
        div()
            .rounded(px(radius::XL))
            .border_1()
            .border_color(t.hairline)
            .bg(t.raised)
            .overflow_hidden()
            .children(rows.into_iter().enumerate().map(|(i, r)| {
                div()
                    .when(i + 1 < n, |d| d.border_b_1().border_color(t.hairline))
                    .child(r)
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn machine_row(
        &self,
        id: &str,
        name: &str,
        tag: Option<&str>,
        meta: String,
        holder: Div,
        action: Option<AnyElement>,
        open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let sessions = self.sessions_on(id);
        let key = id.to_string();
        div()
            .id(SharedString::from(format!("machine-row-{id}")))
            .relative()
            .h(px(ROW_H))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .pl(px(space::S4))
            .pr(px(space::S2))
            .cursor_pointer()
            .hover(|s| s.bg(t.hover()))
            .child(holder)
            .child(
                div()
                    .w(px(240.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .ty(UI)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.ink)
                            .child(div().truncate().child(name.to_string()))
                            .children(tag.map(|tg| {
                                div()
                                    .ty(LABEL)
                                    .font_weight(FontWeight::NORMAL)
                                    .px(px(space::S1H))
                                    .rounded(px(radius::SM))
                                    .bg(t.tint())
                                    .text_color(t.quiet)
                                    .child(tg.to_string())
                            })),
                    )
                    .child(div().ty(UI).text_color(t.quiet).truncate().child(meta)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .ty(UI)
                    .text_color(t.quiet)
                    .children(sessions.into_iter().map(|(st, n)| {
                        let word = match st {
                            Status::Needs => "needs you",
                            Status::Working => "working",
                            Status::Waiting => "paused",
                            Status::Done => "done",
                            Status::Idle => "stopped",
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S1H))
                            .whitespace_nowrap()
                            .child(div().size(px(9.)).bg(t.signal(st).0))
                            .child(format!("{n} {word}"))
                    })),
            )
            .children(action)
            .child(
                div()
                    .size(px(crate::ds::size::CONTROL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::disclosure("machine-turn", open, 13., t.quiet)),
            )
            .child(probe(
                &self.probes,
                format!("machine-row-{}", if id.is_empty() { "this" } else { id }),
            ))
            .on_press(cx.listener(move |this, _, _, cx| {
                this.machine_open = if this.machine_open.as_deref() == Some(key.as_str()) {
                    None
                } else {
                    Some(key.clone())
                };
                this.removing = None;
                cx.notify();
            }))
            .into_any_element()
    }

    /// A machine's details, and Remove for a remote one.
    fn details(
        &self,
        lines: Vec<(&str, String, bool)>,
        remove: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let confirming = remove.is_some() && self.removing == remove;
        let key = remove.clone().unwrap_or_else(|| "this".into());
        let actions = remove.map(|id| {
            let name = self
                .machines
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.name().to_string())
                .unwrap_or_default();
            let row = div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .pt(px(space::S2));
            if confirming {
                let gone = id.clone();
                row.child(
                    div()
                        .flex_1()
                        .ty(UI)
                        .text_color(t.ink)
                        .child(format!("Remove {name}? Its sessions stay on it.")),
                )
                .child(
                    ui::ghost("remove-cancel", "Cancel", &t).on_press(cx.listener(
                        |this, _, _, cx| {
                            this.removing = None;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    ui::inner_button("remove-confirm", "Remove", true, &t).on_press(
                        cx.listener(move |this, _, _, cx| this.remove_machine(&gone, cx)),
                    ),
                )
            } else {
                row.child(div().flex_1()).child(
                    ui::ghost(SharedString::from(format!("remove-{id}")), "Remove…", &t).on_press(
                        cx.listener(move |this, _, _, cx| {
                            this.removing = Some(id.clone());
                            cx.notify();
                        }),
                    ),
                )
            }
        });
        div()
            .pl(px(space::S4 + 32. + space::S3))
            .pr(px(space::S4))
            .pb(px(space::S4))
            .flex()
            .flex_col()
            .gap(px(space::S1H))
            .children(lines.into_iter().map(|(k, v, mono)| {
                div()
                    .flex()
                    .items_start()
                    .ty(UI)
                    .child(
                        div()
                            .w(px(120.))
                            .flex_none()
                            .text_color(t.quiet)
                            .child(k.to_string()),
                    )
                    .child(
                        div().flex().flex_col().text_color(t.ink).children(
                            v.lines()
                                .map(|l| div().when(mono, |d| d.ty(CODE)).child(l.to_string())),
                        ),
                    )
            }))
            .children(actions)
            .arrive(
                SharedString::from(format!("details-{key}")),
                From::Above,
                motion::FAST,
            )
            .into_any_element()
    }

    /// The devices this Mac trusts.
    fn device_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let mut rows: Vec<AnyElement> = Vec::new();
        for d in &self.devices {
            let days = crate::cards::now_ms().saturating_sub(d.added) / 86_400_000;
            let when = match days {
                0 => "today".to_string(),
                1 => "yesterday".to_string(),
                n => format!("{n} days ago"),
            };
            let confirming = self.removing.as_deref() == Some(d.id.as_str());
            let id = d.id.clone();
            let action = if confirming {
                let gone = id.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(
                        ui::ghost("device-cancel", "Cancel", &t).on_press(cx.listener(
                            |this, _, _, cx| {
                                this.removing = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        ui::inner_button("device-remove", "Remove", true, &t).on_press(
                            cx.listener(move |this, _, _, cx| {
                                this.ask(
                                    dent_protocol::Request::RemoveDevice { id: gone.clone() },
                                    Waiting::Devices,
                                );
                                this.ask(dent_protocol::Request::Devices, Waiting::Devices);
                                this.removing = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .into_any_element()
            } else {
                ui::ghost(SharedString::from(format!("device-{id}")), "Remove…", &t)
                    .on_press(cx.listener(move |this, _, _, cx| {
                        this.removing = Some(id.clone());
                        cx.notify();
                    }))
                    .into_any_element()
            };
            rows.push(self.simple_row(
                "smartphone",
                d.name.clone(),
                format!("paired with a code {when}"),
                Some(action),
            ));
        }
        rows.push(self.simple_row(
            "shield-check",
            "Your devices on Tailscale".into(),
            "connect with no code".into(),
            None,
        ));
        let n = rows.len();
        div()
            .rounded(px(radius::XL))
            .border_1()
            .border_color(t.hairline)
            .bg(t.raised)
            .overflow_hidden()
            .children(rows.into_iter().enumerate().map(|(i, r)| {
                div()
                    .when(i + 1 < n, |d| d.border_b_1().border_color(t.hairline))
                    .child(r)
            }))
            .into_any_element()
    }

    fn simple_row(
        &self,
        icon: &str,
        strong: String,
        rest: String,
        action: Option<AnyElement>,
    ) -> AnyElement {
        let t = self.t;
        div()
            .h(px(52.))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .pl(px(space::S4))
            .pr(px(space::S3))
            .child(ui::icon(icon, 14., t.ink))
            .child(
                div().flex_1().min_w_0().truncate().ty(UI).child(
                    div()
                        .flex()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.ink)
                                .child(strong),
                        )
                        .child(div().text_color(t.quiet).child(format!(" · {rest}"))),
                ),
            )
            .children(action)
            .into_any_element()
    }

    // ---- Adding ----

    pub fn open_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = Some(Adding::default());
        self.machine_addr
            .update(cx, |i, cx| crate::app::put(i, "", window, cx));
        self.machine_code
            .update(cx, |i, cx| crate::app::put(i, "", window, cx));
        self.machine_addr.update(cx, |i, cx| i.focus(window, cx));
        let task = cx.background_executor().spawn(async { tailnet_peers() });
        cx.spawn(async move |this, cx| {
            let peers = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(a) = &mut this.adding {
                    a.peers = Some(peers);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Close Add a machine. One that never finished being added is let go.
    pub fn close_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Its fields let go, so the next Esc reaches the page.
        window.blur(cx);
        if let Some(Adding { tag: Some(tag), .. }) = self.adding.take()
            && tag.starts_with('+')
        {
            self.live.stop(&tag, false);
            self.machines.retain(|m| m.id != tag);
        }
        cx.notify();
    }

    /// Esc closes Add a machine before anything under it.
    pub fn escape_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.adding.is_none() {
            return false;
        }
        self.close_adding(window, cx);
        true
    }

    fn start_adding(&mut self, addr: String, cx: &mut Context<Self>) {
        let addr = if addr.contains(':') && !addr.starts_with('[') && addr.matches(':').count() > 1
        {
            // A bare IPv6 address.
            format!("[{addr}]:7480")
        } else if addr
            .rsplit_once(':')
            .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        {
            addr
        } else {
            format!("{addr}:7480")
        };
        let tag = crate::net::adding_tag(&addr);
        self.machines.retain(|m| m.id != tag);
        self.machines.push(MachineView::saved(dent_remote::Saved {
            id: tag.clone(),
            routes: vec![addr.clone()],
            ..Default::default()
        }));
        self.live.add(&addr);
        if let Some(a) = &mut self.adding {
            a.tag = Some(tag);
        }
        cx.notify();
    }

    pub fn add_by_address(&mut self, cx: &mut Context<Self>) {
        let addr = self.machine_addr.read(cx).value().trim().to_string();
        if addr.is_empty() || self.adding.is_none() {
            return;
        }
        self.start_adding(addr, cx);
    }

    pub fn pair_adding(&mut self, cx: &mut Context<Self>) {
        let code: String = self
            .machine_code
            .read(cx)
            .value()
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        let Some(tag) = self.adding.as_ref().and_then(|a| a.tag.clone()) else {
            return;
        };
        if code.len() != 6 {
            return;
        }
        if let Some(m) = self.machines.iter_mut().find(|m| m.id == tag) {
            m.pair_error = None;
        }
        self.live.pair(&tag, &code);
        cx.notify();
    }

    fn remove_machine(&mut self, id: &str, cx: &mut Context<Self>) {
        self.live.stop(id, true);
        // Its files' copies go with it.
        crate::files::forget(id);
        self.machines.retain(|m| m.id != id);
        // Its sessions leave the list with it; they carry on there.
        let gone: Vec<String> = self.core.forget(id);
        self.sessions.retain(|s| fleet::split(&s.id).0 != id);
        let _ = gone;
        // Messages waiting for it would go if it were added again, maybe
        // days later.
        if crate::outbox::forget_machine(&mut self.outbox, id) {
            crate::outbox::save(&self.outbox);
        }
        // New sessions no longer go there.
        if self.defaults.machine == id {
            self.set_machine(String::new(), cx);
        }
        self.removing = None;
        self.machine_open = None;
        cx.notify();
    }

    /// Add a machine, over everything, while open.
    pub fn adding_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.t;
        let a = self.adding.as_ref()?;
        let shapes = self.shapes();
        let shapes = &shapes;
        let body: AnyElement = match &a.tag {
            None => div()
                .child(self.adding_start(a, cx))
                .arrive("adding-step-start", From::Below, motion::BASE)
                .into_any_element(),
            Some(tag) => {
                let step = self
                    .machines
                    .iter()
                    .find(|m| m.id == *tag)
                    .map(|m| format!("{:?}-{}", Standing::of(m), tag.starts_with('+')))
                    .unwrap_or_default();
                div()
                    .child(self.adding_progress(tag, shapes, cx))
                    .arrive(
                        SharedString::from(format!("adding-step-{step}")),
                        From::Below,
                        motion::BASE,
                    )
                    .into_any_element()
            }
        };
        Some(
            div()
                .id("adding-scrim")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(if t.dark {
                    gpui_kit::black().opacity(0.45)
                } else {
                    t.ink.opacity(0.08)
                })
                .flex()
                .justify_center()
                .items_start()
                .pt(px(96.))
                .child(
                    ui::floating(&t)
                        .id("adding")
                        .w(px(540.))
                        .max_w(relative(0.92))
                        .p(px(space::S6))
                        .flex()
                        .flex_col()
                        .gap(px(space::S4))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .ty(DISPLAY)
                                        .text_color(t.ink)
                                        .child("Add a machine"),
                                )
                                .child(ui::icon_button("adding-close", "x", &t).on_press(
                                    cx.listener(|this, _, window, cx| {
                                        this.close_adding(window, cx)
                                    }),
                                )),
                        )
                        .child(body)
                        .child(probe(&self.probes, "adding"))
                        .arrive("adding-arrive", From::Above, motion::FAST),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_adding(window, cx)),
                )
                .with_animation(
                    "adding-scrim-in",
                    Animation::new(std::time::Duration::from_millis(motion::FAST as u64))
                        .with_easing(crate::anim::ease_out),
                    |d, p| d.opacity(p),
                )
                .into_any_element(),
        )
    }

    /// Before an address: your Tailscale's devices, or one typed.
    fn adding_start(&self, a: &Adding, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let known: Vec<String> = self
            .machines
            .iter()
            .flat_map(|m| m.saved.all_routes())
            .map(|r| r.rsplit_once(':').map_or(r.clone(), |(h, _)| h.to_string()))
            .collect();
        let peers: AnyElement = match &a.peers {
            None => div()
                .ty(UI)
                .text_color(t.quiet)
                .child("Looking on your Tailscale…")
                .into_any_element(),
            Some(Err(why)) => div()
                .ty(UI)
                .text_color(t.quiet)
                .child(why.clone())
                .into_any_element(),
            Some(Ok(p)) if p.is_empty() => div()
                .ty(UI)
                .text_color(t.quiet)
                .child("No other computers on your Tailscale yet.")
                .into_any_element(),
            Some(Ok(p)) => {
                let n = p.len();
                div()
                    .rounded(px(radius::XL))
                    .border_1()
                    .border_color(t.hairline)
                    .bg(t.surface)
                    .overflow_hidden()
                    .children(p.iter().enumerate().map(|(i, peer)| {
                        let added = known.contains(&peer.dns);
                        let dns = peer.dns.clone();
                        let action =
                            if added {
                                div()
                                    .ty(UI)
                                    .text_color(t.quiet)
                                    .child("Added")
                                    .into_any_element()
                            } else if !peer.online {
                                div()
                                    .ty(UI)
                                    .text_color(t.quiet)
                                    .child("Offline")
                                    .into_any_element()
                            } else {
                                ui::inner_button(
                                    SharedString::from(format!("add-{}", peer.dns)),
                                    "Add",
                                    true,
                                    &t,
                                )
                                .on_press(cx.listener(move |this, _, _, cx| {
                                    this.start_adding(dns.clone(), cx)
                                }))
                                .into_any_element()
                            };
                        div()
                            .h(px(52.))
                            .flex()
                            .items_center()
                            .gap(px(space::S3))
                            .px(px(space::S3))
                            .when(i + 1 < n, |d| d.border_b_1().border_color(t.hairline))
                            .child(ui::icon(
                                if peer.os == "linux" {
                                    "server"
                                } else {
                                    "laptop"
                                },
                                14.,
                                t.quiet,
                            ))
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
                                            .child(peer.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .ty(LABEL)
                                            .text_color(t.quiet)
                                            .child(os_name(&peer.os)),
                                    ),
                            )
                            .child(action)
                    }))
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::S4))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .child(ui::section_label("On your Tailscale", None, &t))
                    .child(peers),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .child(ui::section_label("Or by address", None, &t))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(self.field(&self.machine_addr, true))
                            .child(
                                ui::button("adding-connect", "Connect", false, &t).on_press(
                                    cx.listener(|this, _, _, cx| this.add_by_address(cx)),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn field(&self, f: &Entity<gpui_kit::base::input::TextareaState>, mono: bool) -> Div {
        let t = self.t;
        div()
            .flex_1()
            .min_w_0()
            .h(px(crate::ds::size::CONTROL))
            .px(px(space::S2))
            .flex()
            .items_center()
            .rounded(px(radius::MD))
            .border_1()
            .border_color(t.ink.opacity(0.16))
            .bg(t.raised)
            .when(mono, |d| d.ty(CODE))
            .when(!mono, |d| d.ty(UI))
            .text_color(t.ink)
            .child(Textarea::new(f))
    }

    /// After an address: how the machine answers.
    fn adding_progress(
        &self,
        tag: &str,
        shapes: &HashMap<String, Shape>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let Some(m) = self.machines.iter().find(|m| m.id == tag) else {
            return div().into_any_element();
        };
        let name = m.name().to_string();
        let st = Standing::of(m);
        let back =
            ui::ghost("adding-back", "Back", &t).on_press(cx.listener(|this, _, window, cx| {
                // Your Tailscale's devices were found once; they aren't
                // looked for again.
                let peers = this.adding.as_ref().and_then(|a| a.peers.clone());
                this.close_adding(window, cx);
                this.adding = Some(Adding { tag: None, peers });
                cx.notify();
            }));
        let say = |strong: String, rest: String| {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .ty(UI)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.ink)
                        .child(strong),
                )
                .child(div().ty(UI).text_color(t.quiet).child(rest))
        };
        // Named by its welcome: added.
        if !tag.starts_with('+') && st.connected() {
            let shape = shapes.get(tag).copied().unwrap_or(Shape::Circle);
            let route = m.via.as_ref().map_or("Tailscale", |(a, tr)| match tr {
                Trust::Tailscale { .. } => "Tailscale",
                _ => route_kind(a),
            });
            let rtt = m.rtt.map(|ms| format!(" · {ms} ms")).unwrap_or_default();
            return div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(space::S3))
                .py(px(space::S4))
                .child(holder(shape, None, 56., false, &t))
                .child(
                    say(format!("{name} added"), format!("Online · {route}{rtt}")).items_center(),
                )
                .child(div().w_full().flex().justify_end().child(
                    ui::button("adding-done", "Done", true, &t).on_press(cx.listener(
                        |this, _, _, cx| {
                            this.adding = None;
                            cx.notify();
                        },
                    )),
                ))
                .into_any_element();
        }
        match (m.standing(), st) {
            (_, Standing::NeedsCode) => div()
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(say(
                    format!("{name} needs a code"),
                    "On it, open Settings › Machines › Pairing code, or run dent-daemon pair, and type the code here."
                        .into(),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(self.field(&self.machine_code, true))
                        .child(
                            ui::button("adding-pair", "Pair", true, &t)
                                .on_press(cx.listener(|this, _, _, cx| this.pair_adding(cx))),
                        ),
                )
                .children(m.pair_error.clone().map(|e| div().ty(UI).text_color(t.needs.0).child(e)))
                .child(div().flex().child(back))
                .into_any_element(),
            (State::Unreachable { why }, _) => div()
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(say(
                    format!("Can't reach {name}"),
                    format!("Is dent running there? ({why})"),
                ))
                .child(div().flex().gap(px(space::S2)).child(back).child(
                    ui::button("adding-retry", "Try again", false, &t).on_press(cx.listener({
                        let tag = tag.to_string();
                        move |this, _, _, cx| {
                            this.live.signal(Some(&tag), Signal::Retry);
                            cx.notify();
                        }
                    })),
                ))
                .into_any_element(),
            (State::Incompatible { .. }, _) => div()
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(say(format!("{name} needs an update"), "It and this Mac speak different versions of dent.".into()))
                .child(div().flex().child(back))
                .into_any_element(),
            (State::TrustedElsewhere { .. }, _) => div()
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(say(
                    format!("{name} trusts this Mac on Tailscale only"),
                    "Add it by its Tailscale name instead.".into(),
                ))
                .child(div().flex().child(back))
                .into_any_element(),
            _ => div()
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(say(format!("Connecting to {name}…"), String::new()))
                .child(div().flex().child(back))
                .into_any_element(),
        }
    }
}

/// Arrive, staggered: item `i` waits `stagger` per place before it, up to
/// the cap, then takes `dur.base`.
fn stagger(i: usize) -> Animation {
    let delay = (i as f32 * motion::STAGGER).min(motion::STAGGER_CAP);
    let total = delay + motion::BASE;
    Animation::new(std::time::Duration::from_millis(total as u64)).with_easing(move |t| {
        crate::anim::ease_out(((t * total - delay) / motion::BASE).clamp(0., 1.))
    })
}

fn os_name(os: &str) -> String {
    match os {
        "macOS" | "darwin" => "macOS".into(),
        "linux" => "Linux".into(),
        "windows" => "Windows".into(),
        o => o.to_string(),
    }
}

/// This Mac's Tailscale account's other computers, from the Tailscale CLI.
/// Tagged devices (servers set up by an admin) and phones aren't offered:
/// they're not yours to trust by account, or can't run dent.
fn tailnet_peers() -> Result<Vec<Peer>, String> {
    let candidates = [
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        "/usr/local/bin/tailscale",
        "/opt/homebrew/bin/tailscale",
        "tailscale",
    ];
    let out = candidates
        .iter()
        .find_map(|c| {
            std::process::Command::new(c)
                .args(["status", "--json"])
                .output()
                .ok()
                .filter(|o| o.status.success())
        })
        .ok_or("Tailscale isn't running here. You can still add a machine by its address.")?;
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| "Tailscale said something unexpected.")?;
    Ok(peers_from(&v))
}

fn peers_from(v: &serde_json::Value) -> Vec<Peer> {
    let me = &v["Self"]["UserID"];
    let mut out: Vec<Peer> = v["Peer"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .filter(|p| p["UserID"] == *me && p["Tags"].as_array().is_none_or(|t| t.is_empty()))
        .filter(|p| !matches!(p["OS"].as_str(), Some("iOS" | "android" | "tvOS")))
        .filter_map(|p| {
            // No MagicDNS name (MagicDNS off): nothing to add it by.
            let dns = p["DNSName"].as_str()?.trim_end_matches('.').to_string();
            if dns.is_empty() {
                return None;
            }
            Some(Peer {
                name: p["HostName"].as_str().unwrap_or(&dns).to_string(),
                dns,
                os: p["OS"].as_str().unwrap_or_default().to_string(),
                online: p["Online"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    out.sort_by(|a, b| b.online.cmp(&a.online).then(a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    // Not a glob: GPUI's prelude has a `test` of its own.
    use super::{AXIS, axis_x, peers_from, route_kind, shapes_for};
    use crate::machines::MachineView;

    #[test]
    fn a_machine_being_added_moves_no_other_off_its_shape() {
        let saved = |id: &str| {
            MachineView::saved(dent_remote::Saved {
                id: id.into(),
                ..Default::default()
            })
        };
        let ids = ["8e2e34085ddf", "3f9a2c710b5e", "c27b19e4a0d2"];
        let mut list: Vec<MachineView> = ids.iter().map(|i| saved(i)).collect();
        let before = shapes_for("5e0d44b91a7c", &list, &std::collections::HashMap::new());
        // Whatever shape addresses would hash to.
        for n in 0..12 {
            list.push(saved(&format!("+10.0.0.{n}:7480")));
        }
        let during = shapes_for("5e0d44b91a7c", &list, &std::collections::HashMap::new());
        assert_eq!(during, before);
        assert_eq!(during.get(""), during.get("5e0d44b91a7c"));
    }

    #[test]
    fn routes_are_named_tailscale_or_lan() {
        assert_eq!(route_kind("100.70.46.115:7480"), "Tailscale");
        assert_eq!(route_kind("[fd7a:115c:a1e0::f832:2e74]:7480"), "Tailscale");
        assert_eq!(route_kind("dent-linux.tail30627d.ts.net:7480"), "Tailscale");
        assert_eq!(route_kind("192.168.64.2:7480"), "LAN");
        assert_eq!(
            route_kind("100.20.1.1:7480"),
            "LAN",
            "outside the tailnet's range"
        );
    }

    #[test]
    fn closer_is_faster_on_the_strip() {
        assert!(axis_x(1) < axis_x(3) && axis_x(3) < axis_x(24) && axis_x(24) < axis_x(300));
        assert_eq!(axis_x(0), AXIS.0);
        assert_eq!(axis_x(10_000), AXIS.1);
    }

    #[test]
    fn only_your_own_computers_are_offered() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "Self": {"UserID": 1},
                "Peer": {
                    "a": {"UserID": 1, "HostName": "linux", "DNSName": "linux.t.ts.net.", "OS": "linux", "Online": true},
                    "b": {"UserID": 2, "HostName": "shared", "DNSName": "shared.t.ts.net.", "OS": "macOS", "Online": true},
                    "c": {"UserID": 1, "HostName": "box", "DNSName": "box.t.ts.net.", "OS": "linux", "Online": true, "Tags": ["tag:server"]},
                    "d": {"UserID": 1, "HostName": "phone", "DNSName": "phone.t.ts.net.", "OS": "iOS", "Online": true},
                    "e": {"UserID": 1, "HostName": "old", "DNSName": "old.t.ts.net.", "OS": "macOS", "Online": false},
                    "f": {"UserID": 1, "HostName": "nodns", "DNSName": "", "OS": "linux", "Online": true}
                }
            }"#,
        )
        .unwrap();
        let p = peers_from(&v);
        assert_eq!(
            p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["linux", "old"]
        );
        assert_eq!(p[0].dns, "linux.t.ts.net");
    }

    #[test]
    fn standing_holds_while_tried_again_and_lets_go_online() {
        use super::Standing;
        use crate::machines::apply;
        use dent_client_core::machine::State;
        use dent_link::Goodbye;
        use dent_remote::Event;
        let mut list = vec![MachineView::saved(dent_remote::Saved {
            id: "ab12".into(),
            ..Default::default()
        })];
        let mut goes = |s: State| {
            apply(&mut list, "ab12", Event::State(s));
            Standing::of(&list[0])
        };
        assert_eq!(goes(State::Away(Goodbye::Sleep)), Standing::Asleep);
        assert_eq!(goes(State::Connecting), Standing::Asleep);
        assert_eq!(goes(State::Reconnecting), Standing::Asleep);
        assert_eq!(
            goes(State::Unreachable { why: "x".into() }),
            Standing::Unreachable
        );
        assert_eq!(goes(State::Connecting), Standing::Unreachable);
        assert_eq!(goes(State::Off), Standing::Connecting);
        assert_eq!(goes(State::Away(Goodbye::Awake)), Standing::Connecting);
        assert_eq!(goes(State::Connecting), Standing::Connecting);
        assert_eq!(goes(State::Away(Goodbye::Sleep)), Standing::Asleep);
        assert_eq!(goes(State::Online), Standing::Online);
        assert_eq!(goes(State::Reconnecting), Standing::Connecting);
        assert_eq!(goes(State::Away(Goodbye::Sleep)), Standing::Asleep);
        assert_eq!(goes(State::Removed), Standing::Blocked);
        assert_eq!(goes(State::Connecting), Standing::Connecting);
        assert_eq!(
            goes(State::Unreachable { why: "x".into() }),
            Standing::Unreachable
        );
        assert_eq!(
            goes(State::NeedsCode {
                pairing_open: false
            }),
            Standing::NeedsCode
        );
        assert_eq!(goes(State::Reconnecting), Standing::Connecting);
        assert_eq!(goes(State::Away(Goodbye::Sleep)), Standing::Asleep);
        assert_eq!(
            goes(State::Incompatible {
                daemon: (1, 1),
                client: (2, 2)
            }),
            Standing::Blocked
        );
        assert_eq!(goes(State::Connecting), Standing::Connecting);
    }
}
