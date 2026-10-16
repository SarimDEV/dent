//! Attention map: the open project as a stippled specimen plate, one block
//! per file sized by its weight and grouped by folder. False-colour squares
//! are pinned over the files the agent reads or edits, joined by fine leader
//! lines in the order it touched them, and a heat loupe follows the pointer.

use std::collections::VecDeque;

use crate::geom::{Rect, spread, treemap, turbo};
use crate::math::{Rng, ease, hash, noise3};
use crate::scene::{Align, Font, Paint, Pt, Rgba, fit, text_width};
use crate::{Ambient, Ctx, DrawCtx, File, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "attention",
    name: "Attention map",
    from: "Stippled specimen plates with false-colour detail windows pinned over them and joined by fine leader lines.",
    shows: "The open project as a stippled map, one block per file sized by its lines. Squares mark the files the agent is reading or editing, linked in the order it touched them. Point anywhere for a heat loupe.",
    make: |seed| Box::new(Attention::new(seed)),
};

/// Where the map starts (pt), and the margins it leaves.
const OX: f32 = 24.;
const OY: f32 = 64.;
const MARGIN_W: f32 = 48.;
const MARGIN_H: f32 = 96.;
/// Treemap paddings (pt): around everything, the group header, inside a
/// group, and the gap between siblings.
const PAD_OUTER: f32 = 2.;
const PAD_HEADER: f32 = 16.;
const PAD_GROUP: f32 = 2.;
const GAP: f32 = 10.;
/// Files mapped; the rest still count in the totals.
const MAX_FILES: usize = 80;
/// Stipple pitch (pt), widened on big screens to keep the cell count down.
const STIPPLE: f32 = 4.;
const MAX_CELLS: f32 = 80_000.;
/// Stipple dot size at the base pitch (pt).
const DOT: f32 = 1.5;
/// Each frame re-stipples one file in this many, round robin.
const EVERY: usize = 15;
/// Density falls off over this distance from a file's edge (pt).
const EDGE: f32 = 28.;
/// Heat a touch adds, and how fast heat settles back (per second).
const BUMP: f32 = 0.3;
const SETTLE: f32 = 0.08;
/// Squares kept, and how long one takes to fade in (seconds).
const MAX_HITS: usize = 7;
const FADE: f32 = 0.5;
/// A square's false-colour field: sample rows, and stops per row.
const FR: usize = 24;
const FC: usize = 7;
/// Finest row of a square (pt).
const ROW_PX: f32 = 3.;
/// The loupe: side (pt), sample rows and stops per row.
const LOUPE: f32 = 72.;
const LR: usize = 18;
const LC: usize = 7;
/// Group labels need this much width (pt); the totals need this much screen.
const GROUP_LABEL_W: f32 = 70.;
const TOTALS_W: f32 = 340.;
/// Square borders are deliberately white, as on the plates.
const WHITE: Rgba = Rgba {
    r: 1.,
    g: 1.,
    b: 1.,
    a: 0.9,
};

struct Leaf {
    /// In map coordinates (origin at `OX`, `OY`).
    r: Rect,
    name: String,
    group: usize,
    base: f32,
    heat: f32,
    /// Absolute positions.
    dots: Vec<Pt>,
    stale: bool,
}

struct Group {
    r: Rect,
    name: String,
    label: String,
}

struct Hit {
    li: usize,
    name: String,
    fx: f32,
    fy: f32,
    size: f32,
    t0: f32,
    edit: bool,
    /// `FR` × `FC` colours.
    field: Vec<Rgba>,
}

struct Loupe {
    x: f32,
    y: f32,
    k: f32,
}

struct Attention {
    seed: i32,
    rng: Rng,
    key: (u32, u32, u64),
    leaves: Vec<Leaf>,
    groups: Vec<Group>,
    lines: f64,
    files: usize,
    st: f32,
    frame: usize,
    hits: VecDeque<Hit>,
    loupe: Option<Loupe>,
    loupe_field: Vec<Rgba>,
}

impl Attention {
    fn new(seed: u64) -> Self {
        Attention {
            seed: (seed % 100) as i32,
            rng: Rng::new(seed ^ 0xA77E),
            key: (0, 0, 0),
            leaves: Vec::new(),
            groups: Vec::new(),
            lines: 0.,
            files: 0,
            st: STIPPLE,
            frame: 0,
            hits: VecDeque::with_capacity(MAX_HITS + 1),
            loupe: None,
            loupe_field: Vec::with_capacity(LR * LC),
        }
    }

    fn layout(&mut self, w: f32, h: f32, files: &[File]) {
        let mut fp: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |b: &[u8]| {
            for &x in b {
                fp = (fp ^ x as u64).wrapping_mul(0x100_0000_01b3);
            }
        };
        for f in files {
            mix(f.group.as_bytes());
            mix(f.name.as_bytes());
            mix(&f.weight.to_bits().to_le_bytes());
        }
        let key = (w.to_bits(), h.to_bits(), fp);
        if key == self.key {
            return;
        }
        self.key = key;
        let ok = |f: &&File| f.weight.is_finite() && f.weight > 0.;
        self.lines = files.iter().filter(ok).map(|f| f.weight as f64).sum();
        self.files = files.len();

        // The heaviest files, grouped; groups and files heaviest first.
        let mut top: Vec<&File> = files.iter().filter(ok).collect();
        top.sort_by(|a, b| b.weight.total_cmp(&a.weight));
        top.truncate(MAX_FILES);
        let mut groups: Vec<(&str, f32, Vec<&File>)> = Vec::new();
        for f in top {
            match groups.iter_mut().find(|g| g.0 == f.group) {
                Some(g) => {
                    g.1 += f.weight;
                    g.2.push(f);
                }
                None => groups.push((&f.group, f.weight, vec![f])),
            }
        }
        groups.sort_by(|a, b| b.1.total_cmp(&a.1));

        let area = Rect {
            x: 0.,
            y: 0.,
            w: (w - MARGIN_W).max(10.),
            h: (h - MARGIN_H).max(10.),
        };
        let area = area.inset(PAD_OUTER, PAD_OUTER, PAD_OUTER, PAD_OUTER);
        let g_rects = treemap(&groups.iter().map(|g| g.1).collect::<Vec<_>>(), area);
        let old: Vec<(String, f32)> = self.leaves.drain(..).map(|l| (l.name, l.heat)).collect();
        self.groups.clear();
        let half = GAP / 2.;
        for (gi, ((name, sum, fs), gr)) in groups.iter().zip(g_rects).enumerate() {
            let gr = gr.inset(half, half, half, half);
            self.groups.push(Group {
                r: gr,
                name: name.to_string(),
                label: format!("{} {:.1}k", name.to_uppercase(), sum / 1000.),
            });
            let inner = gr.inset(PAD_GROUP, PAD_HEADER, PAD_GROUP, PAD_GROUP);
            let f_rects = treemap(&fs.iter().map(|f| f.weight).collect::<Vec<_>>(), inner);
            for (f, r) in fs.iter().zip(f_rects) {
                let i = self.leaves.len() as i32;
                let base = 0.1 + hash(i, 5, self.seed).powf(1.8) * 0.85;
                let heat = old.iter().find(|o| o.0 == f.name).map_or(base, |o| o.1);
                let r = r.inset(half, half, half, half);
                self.leaves.push(Leaf {
                    r,
                    name: f.name.clone(),
                    group: gi,
                    base,
                    heat,
                    dots: Vec::new(),
                    stale: true,
                });
            }
        }
        let leaves = &self.leaves;
        self.hits
            .retain_mut(|hit| match leaves.iter().position(|l| l.name == hit.name) {
                Some(li) => {
                    hit.li = li;
                    true
                }
                None => false,
            });
        self.st = STIPPLE.max((area.w * area.h / MAX_CELLS).sqrt());
    }

    fn density(l: &Leaf, x: f32, y: f32) -> f32 {
        let d = (x - l.r.x)
            .min(l.r.x + l.r.w - x)
            .min(y - l.r.y)
            .min(l.r.y + l.r.h - y);
        l.heat * (d / EDGE).min(1.) + 0.14 * noise3(x * 0.02, y * 0.02, 3.)
    }

    fn stipple(l: &mut Leaf, st: f32) {
        l.dots.clear();
        l.stale = false;
        let (x1, y1) = (l.r.x + l.r.w, l.r.y + l.r.h);
        let mut y = l.r.y;
        while y < y1 {
            let mut x = l.r.x;
            while x < x1 {
                let (gx, gy) = ((x / st) as i32, (y / st) as i32);
                if Self::density(l, x, y) > hash(gx, gy, 31) {
                    l.dots
                        .push([OX + x + hash(gy, gx, 7) * 2., OY + y + hash(gx, gy, 8) * 2.]);
                }
                x += st;
            }
            y += st;
        }
    }

    /// The file a touched path means: by name (preferring one whose group
    /// is in the path), else one chosen by the path's hash.
    fn file_for(&self, path: &str) -> Option<usize> {
        if self.leaves.is_empty() {
            return None;
        }
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let named = |l: &&Leaf| l.name == name;
        let in_group = self
            .leaves
            .iter()
            .position(|l| named(&l) && path.contains(&self.groups[l.group].name));
        let any = || self.leaves.iter().position(|l| named(&l));
        in_group.or_else(any).or_else(|| {
            let h = path.bytes().fold(0x811c_9dc5u32, |h, b| {
                (h ^ b as u32).wrapping_mul(0x0100_0193)
            });
            Some(h as usize % self.leaves.len())
        })
    }

    fn touch(&mut self, li: usize, edit: bool, t: f32) {
        let l = &mut self.leaves[li];
        l.heat = (l.heat + BUMP).min(1.);
        let name = l.name.clone();
        self.hits.retain(|h| h.li != li);
        let r = &mut self.rng;
        let (fx, fy, size) = (0.15 + r.f() * 0.7, 0.2 + r.f() * 0.6, 30. + r.f() * 50.);
        let sd = (r.f() * 1000.).floor();
        let base = if edit { 0.35 } else { 0.1 };
        let field = (0..FR * FC)
            .map(|i| {
                let (u, v) = (
                    (i % FC) as f32 / (FC - 1) as f32,
                    (i / FC) as f32 / (FR - 1) as f32,
                );
                turbo(base + 0.25 * (u + 1. - v) + 0.3 * noise3(u * 4., v * 4., sd))
            })
            .collect();
        self.hits.push_back(Hit {
            li,
            name,
            fx,
            fy,
            size,
            t0: t,
            edit,
            field,
        });
        while self.hits.len() > MAX_HITS {
            self.hits.pop_front();
        }
    }

    /// Where a square sits: its top-left and side, whole points.
    fn place(&self, hit: &Hit) -> Option<(f32, f32, f32)> {
        let l = self.leaves.get(hit.li)?;
        let sz = hit.size.min((l.r.w.min(l.r.h) + 20.).max(18.)).round();
        let x = (OX + l.r.x + hit.fx * l.r.w - sz / 2.).round();
        let y = (OY + l.r.y + hit.fy * l.r.h - sz / 2.).round();
        Some((x, y, sz))
    }
}

/// `n` with thousands separators.
fn grouped(n: f64) -> String {
    let s = format!("{:.0}", n.max(0.));
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A false-colour field as rows of rects, each row a run of two-stop
/// horizontal gradients. Edges land on whole points so rows never seam.
#[allow(clippy::too_many_arguments)]
fn field(
    out: &mut Scene,
    x: f32,
    y: f32,
    side: f32,
    rows: usize,
    src: &[Rgba],
    src_rows: usize,
    cols: usize,
    a: f32,
) {
    for k in 0..rows {
        let y0 = y + (k as f32 * side / rows as f32).floor();
        let y1 = y + ((k + 1) as f32 * side / rows as f32).floor();
        let sr = ((k as f32 + 0.5) / rows as f32 * src_rows as f32) as usize;
        let row = &src[sr.min(src_rows - 1) * cols..][..cols];
        for c in 0..cols - 1 {
            let x0 = x + (c as f32 * side / (cols - 1) as f32).round();
            let x1 = x + ((c + 1) as f32 * side / (cols - 1) as f32).round();
            let paint = Paint::Linear {
                from: [x0, y0],
                to: [x1, y0],
                a: row[c].alpha(a),
                b: row[c + 1].alpha(a),
            };
            out.rect(x0, y0, x1 - x0, y1 - y0, paint);
        }
    }
}

impl Ambient for Attention {
    fn update(&mut self, cx: &Ctx) {
        self.layout(cx.size.w, cx.size.h, &cx.signals.files);
        let k = ease(SETTLE, cx.dt);
        for l in &mut self.leaves {
            l.heat += (l.base - l.heat) * k;
        }
        for t in &cx.signals.touches {
            if let Some(li) = self.file_for(&t.path) {
                self.touch(li, t.edit, cx.t);
            }
        }
        self.frame = self.frame.wrapping_add(1);
        let (st, frame) = (self.st, self.frame);
        for (i, l) in self.leaves.iter_mut().enumerate() {
            if l.stale || (i + frame) % EVERY == 0 {
                Self::stipple(l, st);
            }
        }

        // The loupe samples the stipple's density under the pointer.
        self.loupe = cx.input.pointer.map(|p| Loupe {
            x: (p.x - LOUPE / 2.).round(),
            y: (p.y - LOUPE / 2.).round(),
            k: p.k,
        });
        self.loupe_field.clear();
        if let Some(lp) = &self.loupe {
            for r in 0..LR {
                let y = lp.y + (r as f32 + 0.5) / LR as f32 * LOUPE - OY;
                for c in 0..LC {
                    let x = lp.x + c as f32 / (LC - 1) as f32 * LOUPE - OX;
                    let inside = |l: &&Leaf| {
                        x >= l.r.x && x < l.r.x + l.r.w && y >= l.r.y && y < l.r.y + l.r.h
                    };
                    let v = self
                        .leaves
                        .iter()
                        .find(inside)
                        .map_or(0.02, |l| Self::density(l, x, y) * 1.1);
                    self.loupe_field.push(turbo(v));
                }
            }
        }
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p) = (cx.size.w, cx.size.h, cx.pal);
        let dot = DOT * self.st / STIPPLE;
        let pts: Vec<Pt> = self
            .leaves
            .iter()
            .flat_map(|l| l.dots.iter().copied())
            .collect();
        out.dots(dot, p.pencil, pts);

        // Bottom-left, opposite the totals: the top-left corner is where the
        // window's own controls sit when the sidebar is hidden.
        out.text(
            [OX, h - 14.],
            "WHERE THE AGENT IS LOOKING.",
            Font::Mono,
            11.,
            p.pencil,
            Align::Left,
        );
        if w > TOTALS_W {
            let t = format!("{} LINES · {} FILES", grouped(self.lines), self.files);
            out.text(
                [w - 24., h - 14.],
                t,
                Font::Mono,
                11.,
                p.pencil,
                Align::Right,
            );
        }
        for g in &self.groups {
            if g.r.w > GROUP_LABEL_W {
                out.text(
                    [OX + g.r.x, OY + g.r.y + 11.],
                    fit(&g.label, Font::Mono, 10., g.r.w),
                    Font::Mono,
                    10.,
                    p.soft,
                    Align::Left,
                );
            }
        }

        // Leader lines in touch order, each growing toward its square as it fades in.
        let fade = |hit: &Hit| ((cx.t - hit.t0) / FADE).clamp(0., 1.);
        let placed: Vec<(&Hit, (f32, f32, f32))> = self
            .hits
            .iter()
            .filter_map(|hit| Some((hit, self.place(hit)?)))
            .collect();
        let mut lead: Vec<Pt> = Vec::new();
        for (hit, (x, y, sz)) in &placed {
            let c = [x + sz / 2., y + sz / 2.];
            match lead.last() {
                Some(&prev) => {
                    let a = fade(hit);
                    lead.push([
                        prev[0] + (c[0] - prev[0]) * a,
                        prev[1] + (c[1] - prev[1]) * a,
                    ]);
                    if a < 1. {
                        break;
                    }
                }
                None => lead.push(c),
            }
        }
        out.line(lead, 0.7, p.pencil.alpha(0.55));

        // Labels sit under their squares; any whose boxes share columns are
        // spread apart vertically.
        let mut labels: Vec<(f32, f32, f32, String)> = placed
            .iter()
            .map(|(hit, (x, y, sz))| {
                let text = format!("{} · {}", hit.name, if hit.edit { "edit" } else { "read" });
                (
                    *x - 3.,
                    *x + text_width(&text, Font::Mono, 10.) + 3.,
                    y + sz + 13.,
                    text,
                )
            })
            .collect();
        let mut order: Vec<usize> = (0..labels.len()).collect();
        order.sort_by(|&a, &b| labels[a].0.total_cmp(&labels[b].0));
        let mut s = 0;
        while s < order.len() {
            let mut e = s + 1;
            let mut reach = labels[order[s]].1;
            while e < order.len() && labels[order[e]].0 < reach {
                reach = reach.max(labels[order[e]].1);
                e += 1;
            }
            let mut ys: Vec<f32> = order[s..e].iter().map(|&i| labels[i].2).collect();
            spread(&mut ys, 14., OY, h - 4.);
            for (&i, y) in order[s..e].iter().zip(ys) {
                labels[i].2 = y;
            }
            s = e;
        }

        for ((hit, (x, y, sz)), (lx, _, ly, text)) in placed.iter().zip(labels) {
            let a = fade(hit);
            let rows = ((sz / ROW_PX).ceil() as usize).clamp(1, FR);
            field(out, *x, *y, *sz, rows, &hit.field, FR, FC, a);
            let (x0, y0, x1, y1) = (x + 0.5, y + 0.5, x + sz - 0.5, y + sz - 0.5);
            out.outline(
                vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                1.,
                WHITE.alpha(a),
            );
            out.label(
                [lx + 3., ly],
                text,
                p.pencil.alpha(a),
                p.bg.alpha(a),
                Align::Left,
            );
        }

        if let (Some(lp), Some(_)) = (&self.loupe, cx.pointer)
            && self.loupe_field.len() == LR * LC
        {
            field(out, lp.x, lp.y, LOUPE, LR, &self.loupe_field, LR, LC, lp.k);
            let (x0, y0, x1, y1) = (
                lp.x + 0.5,
                lp.y + 0.5,
                lp.x + LOUPE - 0.5,
                lp.y + LOUPE - 0.5,
            );
            out.outline(
                vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                1.,
                WHITE.alpha(lp.k),
            );
            out.label(
                [lp.x, lp.y - 6.],
                "heat",
                p.soft.alpha(lp.k),
                p.bg.alpha(lp.k),
                Align::Left,
            );
        }
    }
}
