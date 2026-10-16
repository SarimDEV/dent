//! Ruled log: a ruled memo page kept like a naturalist's journal. Date,
//! place and weather sit in a header; below it each thing that happened is
//! written in, a letter at a time, against its time in the margin. The page
//! scrolls up smoothly as lines are added.

use std::collections::VecDeque;

use crate::math::{clock, ease};
use crate::scene::{Align, Font, Paint, fit, text_width};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "ruled",
    name: "Ruled log",
    from: "Ruled memo pages, and the naturalist's habit of a dated, continuous record with metadata up top.",
    shows: "What's been happening in dent (session titles, finished turns) written into the page, newest at the bottom, under the date, this machine and how many sessions are running. The page scrolls up smoothly as lines are added.",
    make: |_| Box::new(Ruled::new()),
};

/// Line pitch (pt).
const LH: f32 = 26.;
/// Where the ruled area starts (pt from the top).
const TOP: f32 = 44.;
/// The margin rule's x (pt).
const MX: f32 = 70.;
/// Typing speed (characters per second); each key typed adds one more.
const CPS: f32 = 16.;
/// Pause after a line is finished before the next one starts (s).
const WAIT: f32 = 1.4;
/// Entries kept on the page, and lines waiting to be written.
const KEEP: usize = 60;
/// Scroll easing rate (per second).
const SCROLL: f32 = 7.;
/// How often the date is re-read (s).
const DATE_EVERY: f32 = 30.;

struct Entry {
    text: String,
    len: usize,
    shown: f32,
    time: String,
    accent: bool,
}

impl Entry {
    fn new(text: String, time: String, accent: bool) -> Self {
        Entry {
            len: text.chars().count(),
            text,
            shown: 0.,
            time,
            accent,
        }
    }
}

type Key = (u32, String);

struct Ruled {
    entries: VecDeque<Entry>,
    pending: VecDeque<(f32, String)>,
    seen: Vec<Key>,
    log_sig: Option<(usize, Key)>,
    first: bool,
    wait: f32,
    sc: Option<f32>,
    t: f32,
    date: String,
    date_at: f32,
    place: String,
    weather: String,
}

fn key(minute: f32, text: &str) -> Key {
    (minute.to_bits(), text.to_string())
}

/// Year, month (1..12) and day from days since 1970-01-01 (Hinnant).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (era * 400 + yoe + (m <= 2) as i64, m, d)
}

/// Today's local date as a journal writes it (`15·X·26`). The local offset
/// comes from comparing the reported local minute with the UTC clock.
fn today(local_minute: Option<f32>) -> String {
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return String::new();
    };
    let utc_min = (now.as_secs() / 60) as i64;
    let off = local_minute
        .filter(|m| m.is_finite())
        .map(|m| (m as i64 - utc_min.rem_euclid(1440) + 720).rem_euclid(1440) - 720)
        .unwrap_or(0);
    let (y, m, d) = civil((utc_min + off).div_euclid(1440));
    journal_date(y, m as i64, d as i64)
}

/// A date as a journal writes it: day, month in Roman numerals, two-digit year.
fn journal_date(y: i64, m: i64, d: i64) -> String {
    const ROMAN: [&str; 12] = [
        "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII",
    ];
    format!(
        "{d}·{}·{:02}",
        ROMAN[(m.clamp(1, 12) - 1) as usize],
        y.rem_euclid(100)
    )
}

impl Ruled {
    fn new() -> Self {
        Ruled {
            entries: VecDeque::new(),
            pending: VecDeque::new(),
            seen: Vec::new(),
            log_sig: None,
            first: true,
            wait: 0.,
            sc: None,
            t: 0.,
            date: String::new(),
            date_at: f32::NEG_INFINITY,
            place: String::new(),
            weather: String::new(),
        }
    }

    fn push(&mut self, e: Entry) {
        self.entries.push_back(e);
        if self.entries.len() > KEEP {
            self.entries.pop_front();
            if let Some(sc) = &mut self.sc {
                *sc -= 1.;
            }
        }
    }

    /// Queue every log line not seen before, in order.
    fn sync_log(&mut self, cx: &Ctx) {
        let log = &cx.signals.log;
        let sig = log.last().map(|l| (log.len(), key(l.minute, &l.text)));
        if sig == self.log_sig {
            return;
        }
        self.log_sig = sig;
        let fresh: Vec<(f32, String)> = log
            .iter()
            .filter(|l| {
                !self
                    .seen
                    .iter()
                    .any(|(m, t)| *m == l.minute.to_bits() && *t == l.text)
            })
            .map(|l| (l.minute, l.text.clone()))
            .collect();
        self.seen.clear();
        self.seen.extend(log.iter().map(|l| key(l.minute, &l.text)));
        if self.first {
            // The page starts with what's already happened written in, and
            // the latest line being typed.
            self.first = false;
            let n = fresh.len();
            for (i, (m, text)) in fresh.into_iter().enumerate().skip(n.saturating_sub(KEEP)) {
                let mut e = Entry::new(text, clock(m), false);
                if i + 1 < n {
                    e.shown = e.len as f32;
                }
                self.push(e);
            }
            return;
        }
        self.pending.extend(fresh);
        while self.pending.len() > KEEP {
            self.pending.pop_front();
        }
    }
}

impl Ambient for Ruled {
    fn update(&mut self, cx: &Ctx) {
        self.t = cx.t;
        let s = cx.signals;
        if cx.t - self.date_at >= DATE_EVERY || self.date.is_empty() {
            self.date_at = cx.t;
            self.date = match s.date {
                Some((y, m, d)) => journal_date(y as i64, m as i64, d as i64),
                None => today(s.minute),
            };
        }
        self.place = s.machine.clone().unwrap_or_default();
        // Only a real count is a claim worth printing; stand-in sessions
        // are scenery.
        self.weather = s
            .working
            .map(|n| format!("{n} running"))
            .unwrap_or_default();
        self.sync_log(cx);

        let typing = self.entries.back().is_some_and(|e| e.shown < e.len as f32);
        if cx.input.submit {
            if let Some(cur) = self.entries.back_mut() {
                cur.shown = cur.len as f32;
            }
            let now = clock(s.minute.unwrap_or(0.));
            self.push(Entry::new("New session started".into(), now, true));
            self.wait = 0.;
        } else if typing {
            if let Some(cur) = self.entries.back_mut() {
                cur.shown = (cur.shown + cx.dt * CPS + cx.input.keys as f32).min(cur.len as f32);
            }
        } else {
            self.wait += cx.dt;
            if (self.wait > WAIT || self.entries.is_empty())
                && let Some((m, text)) = self.pending.pop_front()
            {
                self.wait = 0.;
                self.push(Entry::new(text, clock(m), false));
            }
        }

        let vis = ((cx.size.h - TOP) / LH).floor().max(1.);
        let target = (self.entries.len() as f32 - vis).max(0.);
        self.sc = Some(match self.sc {
            None => target,
            Some(sc) => sc + (target - sc) * ease(SCROLL, cx.dt),
        });
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p) = (cx.size.w, cx.size.h, cx.pal);
        let sc = self.sc.unwrap_or(0.).max(0.);
        let off = sc.fract() * LH;
        out.clip(0., TOP - 6., w, h - TOP + 6.);
        let mut rules = Vec::new();
        let mut y = TOP - off;
        while y < h {
            if y >= 0. {
                rules.extend([[0., y.round() + 0.5], [w, y.round() + 0.5]]);
            }
            y += LH;
        }
        out.segments(rules, 1., p.rule);
        let max = w - MX - 30.;
        for (e, it) in self.entries.iter().enumerate().skip(sc.floor() as usize) {
            let y = TOP + (e as f32 - sc + 1.) * LH - 8.;
            if y > h + LH {
                break;
            }
            out.text(
                [MX - 10., y],
                it.time.clone(),
                Font::Mono,
                11.,
                p.soft,
                Align::Right,
            );
            if it.accent {
                out.rect(8., y - 8., 7., 7., Paint::Solid(p.accent));
            }
            let typed: String = it.text.chars().take(it.shown.floor() as usize).collect();
            let txt = fit(&typed, Font::Book, 15., max);
            let tw = text_width(&txt, Font::Book, 15.);
            out.text(
                [MX + 14., y],
                txt,
                Font::Book,
                15.,
                if it.accent { p.accent } else { p.pencil },
                Align::Left,
            );
            if e + 1 == self.entries.len() && (cx.t * 2.).floor() as i64 % 2 == 0 {
                out.rect(MX + 16. + tw, y - 12., 7., 15., Paint::Solid(p.soft));
            }
        }
        out.unclip();
        out.segments(
            vec![[MX + 0.5, 0.], [MX + 0.5, h], [MX + 3.5, 0.], [MX + 3.5, h]],
            1.,
            p.rule2,
        );
        // The header: each field only where it fits, never on the last.
        let mut x_min = 0.;
        for (txt, x) in [
            ("DATE", &self.date, 14.),
            ("PLACE", &self.place, 130.),
            ("WEATHER", &self.weather, 260.),
        ]
        .into_iter()
        .filter(|(_, v, _)| !v.is_empty())
        .map(|(k, v, x)| (format!("{k}  {v}"), x))
        {
            let x = (MX + x).max(x_min);
            let tw = text_width(&txt, Font::Mono, 10.);
            if x + tw >= w - 8. {
                break;
            }
            out.text([x, 26.], txt, Font::Mono, 10., p.soft, Align::Left);
            x_min = x + tw + 18.;
        }
    }
}
