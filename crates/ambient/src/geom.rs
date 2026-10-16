//! Geometry shared by backgrounds: polygon slicing, contour lines,
//! triangulation, treemaps, label spreading and a perceptual colour map.

use crate::scene::{Pt, Rgba};

/// The part of polygon `pts` whose projection onto `dir` (measured from
/// `origin`, in units of `dir`'s length) lies within `lo..=hi`.
pub fn clip_band(pts: &[Pt], origin: Pt, dir: [f32; 2], lo: f32, hi: f32) -> Vec<Pt> {
    let len2 = dir[0] * dir[0] + dir[1] * dir[1];
    let proj = |p: &Pt| ((p[0] - origin[0]) * dir[0] + (p[1] - origin[1]) * dir[1]) / len2;
    let mut out = pts.to_vec();
    for (bound, keep_above) in [(lo, true), (hi, false)] {
        if !bound.is_finite() || out.is_empty() {
            continue;
        }
        let inside = |p: &Pt| {
            if keep_above {
                proj(p) >= bound
            } else {
                proj(p) <= bound
            }
        };
        let mut next = Vec::with_capacity(out.len() + 2);
        for i in 0..out.len() {
            let a = out[i];
            let b = out[(i + 1) % out.len()];
            let (ia, ib) = (inside(&a), inside(&b));
            if ia {
                next.push(a);
            }
            if ia != ib {
                let (pa, pb) = (proj(&a), proj(&b));
                let k = (bound - pa) / (pb - pa);
                next.push([a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k]);
            }
        }
        out = next;
    }
    out
}

/// Contour segments where the grid `vals` (row-major, `cols` × `rows`)
/// crosses `level`, by marching squares. Coordinates are in grid cells.
pub fn contour(vals: &[f32], cols: usize, rows: usize, level: f32, out: &mut Vec<Pt>) {
    if cols < 2 || rows < 2 || vals.len() < cols * rows {
        return;
    }
    let lerp = |a: f32, b: f32| {
        if (b - a).abs() < 1e-9 {
            0.5
        } else {
            ((level - a) / (b - a)).clamp(0., 1.)
        }
    };
    for y in 0..rows - 1 {
        for x in 0..cols - 1 {
            let (tl, tr) = (vals[y * cols + x], vals[y * cols + x + 1]);
            let (bl, br) = (vals[(y + 1) * cols + x], vals[(y + 1) * cols + x + 1]);
            let code = (tl > level) as u8
                | ((tr > level) as u8) << 1
                | ((br > level) as u8) << 2
                | ((bl > level) as u8) << 3;
            if code == 0 || code == 15 {
                continue;
            }
            let (fx, fy) = (x as f32, y as f32);
            let top = [fx + lerp(tl, tr), fy];
            let right = [fx + 1., fy + lerp(tr, br)];
            let bottom = [fx + lerp(bl, br), fy + 1.];
            let left = [fx, fy + lerp(tl, bl)];
            let centre_high = (tl + tr + br + bl) / 4. > level;
            let mut seg = |a: Pt, b: Pt| {
                out.push(a);
                out.push(b);
            };
            match code {
                1 | 14 => seg(left, top),
                2 | 13 => seg(top, right),
                3 | 12 => seg(left, right),
                4 | 11 => seg(right, bottom),
                6 | 9 => seg(top, bottom),
                7 | 8 => seg(left, bottom),
                5 => {
                    if centre_high {
                        seg(left, bottom);
                        seg(top, right)
                    } else {
                        seg(left, top);
                        seg(right, bottom)
                    }
                }
                10 => {
                    if centre_high {
                        seg(left, top);
                        seg(right, bottom)
                    } else {
                        seg(left, bottom);
                        seg(top, right)
                    }
                }
                _ => {}
            }
        }
    }
}

/// Delaunay triangulation of a few points (Bowyer–Watson; fine for dozens).
pub fn delaunay(pts: &[Pt]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        minx = minx.min(p[0]);
        miny = miny.min(p[1]);
        maxx = maxx.max(p[0]);
        maxy = maxy.max(p[1]);
    }
    let d = (maxx - minx).max(maxy - miny).max(1.) * 20.;
    let (cx, cy) = ((minx + maxx) / 2., (miny + maxy) / 2.);
    let mut all = pts.to_vec();
    all.extend([[cx - d, cy - d], [cx + d, cy - d], [cx, cy + d]]);
    let circum = |t: &[usize; 3]| {
        let (a, b, c) = (all[t[0]], all[t[1]], all[t[2]]);
        let dd = 2. * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
        if dd.abs() < 1e-9 {
            return ([0., 0.], f32::INFINITY);
        }
        let (a2, b2, c2) = (
            a[0] * a[0] + a[1] * a[1],
            b[0] * b[0] + b[1] * b[1],
            c[0] * c[0] + c[1] * c[1],
        );
        let ux = (a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / dd;
        let uy = (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / dd;
        ([ux, uy], (a[0] - ux).powi(2) + (a[1] - uy).powi(2))
    };
    let mut tris: Vec<[usize; 3]> = vec![[n, n + 1, n + 2]];
    for (i, &p) in pts.iter().enumerate() {
        let mut bad = Vec::new();
        tris.retain(|t| {
            let (c, r2) = circum(t);
            let inside = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) < r2;
            if inside {
                bad.push(*t);
            }
            !inside
        });
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for t in &bad {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let shared = bad
                    .iter()
                    .filter(|o| o.contains(&a) && o.contains(&b))
                    .count()
                    > 1;
                if !shared {
                    edges.push((a, b));
                }
            }
        }
        for (a, b) in edges {
            tris.push([a, b, i]);
        }
    }
    tris.retain(|t| t.iter().all(|&v| v < n));
    tris
}

/// The unique edges of a triangulation.
pub fn edges(tris: &[[usize; 3]]) -> Vec<(usize, usize)> {
    let mut e: Vec<(usize, usize)> = tris
        .iter()
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, b)| (a.min(b), a.max(b)))
        .collect();
    e.sort_unstable();
    e.dedup();
    e
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn inset(self, l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect {
            x: self.x + l,
            y: self.y + t,
            w: (self.w - l - r).max(0.),
            h: (self.h - t - b).max(0.),
        }
    }
}

/// Squarified treemap: rectangles for `weights` (in the given order) that
/// tile `area` with aspect ratios near 1.
pub fn treemap(weights: &[f32], area: Rect) -> Vec<Rect> {
    let total: f32 = weights.iter().sum();
    let mut out = vec![
        Rect {
            x: area.x,
            y: area.y,
            w: 0.,
            h: 0.
        };
        weights.len()
    ];
    if total <= 0. || area.w <= 0. || area.h <= 0. {
        return out;
    }
    let scale = area.w * area.h / total;
    let areas: Vec<f32> = weights.iter().map(|w| w * scale).collect();
    let mut free = area;
    let mut i = 0;
    while i < areas.len() {
        let side = free.w.min(free.h);
        let worst = |row: &[f32]| {
            let s: f32 = row.iter().sum();
            let (mx, mn) = row
                .iter()
                .fold((0f32, f32::MAX), |(a, b), &v| (a.max(v), b.min(v)));
            ((side * side * mx) / (s * s)).max((s * s) / (side * side * mn.max(1e-6)))
        };
        let mut j = i + 1;
        while j < areas.len() && worst(&areas[i..=j]) <= worst(&areas[i..j]) {
            j += 1;
        }
        let row = &areas[i..j];
        let s: f32 = row.iter().sum();
        if free.w >= free.h {
            let w = if free.h > 0. { s / free.h } else { 0. };
            let mut y = free.y;
            for (k, a) in row.iter().enumerate() {
                let h = if w > 0. { a / w } else { 0. };
                out[i + k] = Rect { x: free.x, y, w, h };
                y += h;
            }
            free = Rect {
                x: free.x + w,
                w: (free.w - w).max(0.),
                ..free
            };
        } else {
            let h = if free.w > 0. { s / free.w } else { 0. };
            let mut x = free.x;
            for (k, a) in row.iter().enumerate() {
                let w = if h > 0. { a / h } else { 0. };
                out[i + k] = Rect { x, y: free.y, w, h };
                x += w;
            }
            free = Rect {
                y: free.y + h,
                h: (free.h - h).max(0.),
                ..free
            };
        }
        i = j;
    }
    out
}

/// Push labels apart along one axis so each sits at least `gap` from the
/// last, then shift the stack back inside `lo..=hi`. Values are positions.
pub fn spread(ys: &mut [f32], gap: f32, lo: f32, hi: f32) {
    if ys.is_empty() {
        return;
    }
    let mut order: Vec<usize> = (0..ys.len()).collect();
    order.sort_by(|&a, &b| ys[a].total_cmp(&ys[b]));
    for w in 1..order.len() {
        let prev = ys[order[w - 1]];
        let cur = &mut ys[order[w]];
        *cur = cur.max(prev + gap);
    }
    let over = ys[order[order.len() - 1]] - hi;
    for y in ys.iter_mut() {
        if over > 0. {
            *y -= over;
        }
        *y = y.max(lo);
    }
}

/// Google's Turbo colour map (polynomial fit), `t` in 0..1.
pub fn turbo(t: f32) -> Rgba {
    let t = t.clamp(0., 1.);
    let r = 0.135_721_38
        + t * (4.615_392_6
            + t * (-42.660_32 + t * (132.131_08 + t * (-152.942_4 + t * 59.286_38))));
    let g = 0.091_402_61
        + t * (2.194_188_4
            + t * (4.842_966_6 + t * (-14.185_033 + t * (4.277_298_6 + t * 2.829_566))));
    let b = 0.106_673_3
        + t * (12.641_946 + t * (-60.582_05 + t * (110.362_77 + t * (-89.903_11 + t * 27.348_25))));
    Rgba {
        r: r.clamp(0., 1.),
        g: g.clamp(0., 1.),
        b: b.clamp(0., 1.),
        a: 1.,
    }
}

/// Where ray `o + t·v` meets segment `p`–`q`, as `t`, if it does ahead.
pub fn ray_segment(o: Pt, v: [f32; 2], p: Pt, q: Pt) -> Option<f32> {
    let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
    let den = v[0] * ey - v[1] * ex;
    if den.abs() < 1e-9 {
        return None;
    }
    let t = ((p[0] - o[0]) * ey - (p[1] - o[1]) * ex) / den;
    let u = ((p[0] - o[0]) * v[1] - (p[1] - o[1]) * v[0]) / den;
    (t > 1e-4 && (0. ..=1.).contains(&u)).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treemap_tiles_the_area() {
        let r = treemap(
            &[6., 6., 4., 3., 2., 2., 1.],
            Rect {
                x: 0.,
                y: 0.,
                w: 600.,
                h: 400.,
            },
        );
        let area: f32 = r.iter().map(|r| r.w * r.h).sum();
        assert!((area - 240_000.).abs() < 1.);
        assert!(
            r.iter().all(|r| r.x >= -0.01
                && r.y >= -0.01
                && r.x + r.w <= 600.01
                && r.y + r.h <= 400.01)
        );
    }

    #[test]
    fn delaunay_of_a_square_is_two_triangles() {
        let t = delaunay(&[[0., 0.], [10., 0.], [10., 10.], [0., 10.]]);
        assert_eq!(t.len(), 2);
        assert_eq!(edges(&t).len(), 5);
    }

    #[test]
    fn contour_finds_a_ring() {
        let (c, r) = (21, 21);
        let vals: Vec<f32> = (0..c * r)
            .map(|i| {
                let (x, y) = ((i % c) as f32 - 10., (i / c) as f32 - 10.);
                -(x * x + y * y).sqrt()
            })
            .collect();
        let mut seg = Vec::new();
        contour(&vals, c, r, -5., &mut seg);
        assert!(seg.len() > 20);
        assert!(
            seg.iter()
                .all(|p| ((p[0] - 10.).powi(2) + (p[1] - 10.).powi(2)).sqrt() - 5. < 1.)
        );
    }

    #[test]
    fn spread_keeps_gaps() {
        let mut ys = [10., 11., 12., 50.];
        spread(&mut ys, 14., 0., 100.);
        assert!(ys[1] - ys[0] >= 14. && ys[2] - ys[1] >= 14.);
    }

    #[test]
    fn band_clip() {
        let sq = [[0., 0.], [10., 0.], [10., 10.], [0., 10.]];
        let b = clip_band(&sq, [0., 0.], [10., 0.], 0.2, 0.6);
        let xs: Vec<f32> = b.iter().map(|p| p[0]).collect();
        assert!(xs.iter().all(|&x| (1.999..=6.001).contains(&x)));
    }
}
