//! Small numeric helpers shared by every background: seeded randomness,
//! noise, easing and frame-rate independent smoothing.

/// `v` limited to `lo..=hi`. When the range is empty (a tiny canvas),
/// `lo` wins, so things stay on the leading edge rather than off screen.
pub fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    v.min(hi).max(lo)
}

pub fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

/// The share of the remaining distance to cover this frame when easing at
/// `rate` per second, so smoothing looks the same at 30, 60 or 120 Hz.
pub fn ease(rate: f32, dt: f32) -> f32 {
    1. - (-rate * dt).exp()
}

/// Ease-out cubic, for things that arrive.
pub fn out_cubic(k: f32) -> f32 {
    let k = clamp(k, 0., 1.);
    1. - (1. - k).powi(3)
}

/// The signed difference `a - b` between two angles in degrees, in -180..180.
pub fn wrap_deg(d: f32) -> f32 {
    (d % 360. + 540.) % 360. - 180.
}

/// A small, fast, seeded generator (mulberry32).
#[derive(Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng((seed ^ (seed >> 32)) as u32 ^ 0x9E37_79B9)
    }
    /// Uniform in 0..1.
    pub fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        ((t ^ (t >> 14)) >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Uniform in lo..hi.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
    /// Uniform index below `n` (n > 0).
    pub fn index(&mut self, n: usize) -> usize {
        ((self.f() * n as f32) as usize).min(n.saturating_sub(1))
    }
}

/// A stable pseudo-random value in 0..1 for an integer cell: the same cell
/// always gets the same value, so stipple and jitter never shimmer.
pub fn hash(x: i32, y: i32, s: i32) -> f32 {
    let mut h = (x.wrapping_mul(374_761_393))
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(s.wrapping_mul(1_442_695_041)) as u32;
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

const GRAD: [[f32; 3]; 12] = [
    [1., 1., 0.],
    [-1., 1., 0.],
    [1., -1., 0.],
    [-1., -1., 0.],
    [1., 0., 1.],
    [-1., 0., 1.],
    [1., 0., -1.],
    [-1., 0., -1.],
    [0., 1., 1.],
    [0., -1., 1.],
    [0., 1., -1.],
    [0., -1., -1.],
];

fn perm() -> &'static [u8; 512] {
    static P: std::sync::OnceLock<[u8; 512]> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let mut p: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut r = Rng::new(11);
        for i in (1..256).rev() {
            let j = r.index(i + 1);
            p.swap(i, j);
        }
        std::array::from_fn(|i| p[i & 255])
    })
}

/// 3D simplex noise in roughly -1..1.
pub fn noise3(x: f32, y: f32, z: f32) -> f32 {
    const F3: f32 = 1. / 3.;
    const G3: f32 = 1. / 6.;
    let p = perm();
    let s = (x + y + z) * F3;
    let (i, j, k) = ((x + s).floor(), (y + s).floor(), (z + s).floor());
    let t = (i + j + k) * G3;
    let (x0, y0, z0) = (x - (i - t), y - (j - t), z - (k - t));
    let (i1, j1, k1, i2, j2, k2) = if x0 >= y0 {
        if y0 >= z0 {
            (1, 0, 0, 1, 1, 0)
        } else if x0 >= z0 {
            (1, 0, 0, 1, 0, 1)
        } else {
            (0, 0, 1, 1, 0, 1)
        }
    } else if y0 < z0 {
        (0, 0, 1, 0, 1, 1)
    } else if x0 < z0 {
        (0, 1, 0, 0, 1, 1)
    } else {
        (0, 1, 0, 1, 1, 0)
    };
    let (ii, jj, kk) = (
        (i as i32 & 255) as usize,
        (j as i32 & 255) as usize,
        (k as i32 & 255) as usize,
    );
    let corner = |g: usize, a: f32, b: f32, c: f32| {
        let q = 0.6 - a * a - b * b - c * c;
        if q < 0. {
            return 0.;
        }
        let v = GRAD[g];
        q * q * q * q * (v[0] * a + v[1] * b + v[2] * c)
    };
    let g = |a: usize, b: usize, c: usize| {
        p[ii + a + p[jj + b + p[kk + c] as usize] as usize] as usize % 12
    };
    32. * (corner(g(0, 0, 0), x0, y0, z0)
        + corner(
            g(i1, j1, k1),
            x0 - i1 as f32 + G3,
            y0 - j1 as f32 + G3,
            z0 - k1 as f32 + G3,
        )
        + corner(
            g(i2, j2, k2),
            x0 - i2 as f32 + 2. * G3,
            y0 - j2 as f32 + 2. * G3,
            z0 - k2 as f32 + 2. * G3,
        )
        + corner(
            g(1, 1, 1),
            x0 - 1. + 3. * G3,
            y0 - 1. + 3. * G3,
            z0 - 1. + 3. * G3,
        ))
}

/// A wall-clock time of day, `hh:mm`, from minutes since midnight.
pub fn clock(minutes: f32) -> String {
    let m = minutes.rem_euclid(24. * 60.);
    format!("{:02}:{:02}", (m / 60.) as u32, (m % 60.) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_bounded_and_smooth() {
        let mut prev = noise3(0., 0.3, 0.7);
        for i in 1..2000 {
            let v = noise3(i as f32 * 0.01, 0.3, 0.7);
            assert!((-1.01..=1.01).contains(&v));
            assert!((v - prev).abs() < 0.1, "noise jumped");
            prev = v;
        }
    }

    #[test]
    fn rng_stays_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let v = r.f();
            assert!((0. ..1.).contains(&v));
        }
    }

    #[test]
    fn angles_wrap() {
        assert_eq!(wrap_deg(350. - 10.), -20.);
        assert_eq!(wrap_deg(10. - 350.), 20.);
    }
}
