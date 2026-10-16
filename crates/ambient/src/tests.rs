//! Fast checks every registered background must pass. They run in well
//! under a second; the long stress pass is `just ambient-stress`.

use super::*;

/// Drive `spec` for `frames` frames at `size` with hostile input: every key,
/// Enter every frame, the pointer jumping on and off. Returns the scene of
/// the last frame in each theme.
pub(crate) fn hammer(spec: &'static Spec, size: Size, frames: usize, hz: f32) -> (Runner, usize) {
    let mut r = Runner::new(spec, 7);
    let mut rng = math::Rng::new(3);
    let mut prims = 0;
    for f in 0..frames {
        for _ in 0..10 {
            r.key();
        }
        r.submit();
        let p = (rng.f() > 0.15)
            .then(|| [rng.range(-0.1, 1.1) * size.w, rng.range(-0.1, 1.1) * size.h]);
        r.pointer(p);
        r.tick(1. / hz, size, Signals::default());
        if f % 7 == 0 || f + 1 == frames {
            for pal in [Palette::paper(), Palette::carbon()] {
                let scene = r.draw(&pal);
                prims = prims.max(scene.prims.len());
                check(spec, size, scene);
            }
        }
    }
    (r, prims)
}

pub(crate) fn check(spec: &Spec, size: Size, scene: &Scene) {
    let far = 4. * (size.w + size.h) + 100.;
    let inb = |p: &Pt| p[0].abs() <= far && p[1].abs() <= far;
    let mut points = 0;
    for p in &scene.prims {
        let ok = match p {
            Prim::Rect { x, y, .. } => inb(&[*x, *y]),
            Prim::Dots { pts, .. }
            | Prim::Stroke { pts, .. }
            | Prim::Segments { pts, .. }
            | Prim::Fill { pts, .. } => {
                points += pts.len();
                pts.iter().all(inb)
            }
            Prim::Circle { c, r, .. } => inb(c) && *r < far,
            Prim::Text { at, .. } => inb(at),
            _ => true,
        };
        assert!(
            ok,
            "{}: geometry far out of bounds at {size:?}: {p:?}",
            spec.id
        );
    }
    assert!(
        scene.prims.len() < 30_000,
        "{}: {} primitives",
        spec.id,
        scene.prims.len()
    );
    assert!(points < 400_000, "{}: {points} points", spec.id);
}

#[test]
fn every_background_survives_hostile_input() {
    for spec in ALL {
        for size in [Size { w: 320., h: 200. }, Size { w: 390., h: 640. }] {
            let (r, _) = hammer(spec, size, 150, 60.);
            assert!(!r.failed(), "{} panicked at {size:?}", spec.id);
        }
    }
}

#[test]
fn every_background_draws_at_desktop_size() {
    for spec in ALL {
        let (r, prims) = hammer(spec, Size { w: 1280., h: 760. }, 12, 60.);
        assert!(!r.failed(), "{} panicked", spec.id);
        assert!(prims > 0, "{} drew nothing", spec.id);
    }
}

#[test]
fn ids_are_unique_and_findable() {
    for (i, s) in ALL.iter().enumerate() {
        assert!(
            ALL.iter().skip(i + 1).all(|o| o.id != s.id),
            "duplicate id {}",
            s.id
        );
        assert!(find(s.id).is_some());
    }
}

#[test]
fn tiny_and_degenerate_sizes_are_safe() {
    for spec in ALL {
        for size in [
            Size { w: 1., h: 1. },
            Size { w: 0., h: 0. },
            Size { w: 40., h: 900. },
        ] {
            let (r, _) = hammer(spec, size, 20, 60.);
            assert!(!r.failed(), "{} panicked at {size:?}", spec.id);
        }
    }
}

/// The long pass: big screens, several frame rates, timing. On demand only
/// (`just ambient-stress`), in release.
#[test]
#[ignore]
fn stress() {
    let sizes = [
        Size { w: 320., h: 200. },
        Size { w: 390., h: 844. },
        Size { w: 1440., h: 900. },
        Size { w: 2560., h: 1600. },
        Size { w: 3840., h: 2160. },
    ];
    println!(
        "{:<12} {:>10} {:>12} {:>12}",
        "background", "size", "avg ms", "worst ms"
    );
    for spec in ALL {
        for size in sizes {
            for hz in [30., 60., 120.] {
                let (r, _) = hammer(spec, size, 600, hz);
                assert!(!r.failed(), "{} panicked at {size:?} {hz} Hz", spec.id);
            }
            // Timing: update + draw for 300 frames after warm-up.
            let mut r = Runner::new(spec, 1);
            for _ in 0..120 {
                r.tick(1. / 60., size, Signals::default());
                r.draw(&Palette::paper());
            }
            let (mut sum, mut worst) = (0f64, 0f64);
            for _ in 0..300 {
                let t0 = std::time::Instant::now();
                r.tick(1. / 60., size, Signals::default());
                r.draw(&Palette::paper());
                let ms = t0.elapsed().as_secs_f64() * 1000.;
                sum += ms;
                worst = worst.max(ms);
            }
            println!(
                "{:<12} {:>10} {:>12.2} {:>12.2}",
                spec.id,
                format!("{}x{}", size.w, size.h),
                sum / 300.,
                worst
            );
        }
    }
}
