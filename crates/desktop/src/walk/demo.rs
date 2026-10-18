use super::*;

/// The background a `demo-<id>` walk shows.
pub(super) static DEMO: std::sync::Mutex<&'static str> = std::sync::Mutex::new("prism");

/// The pointer gliding from one offset (from the composer's centre) to
/// another over `secs`, one step every 40 ms.
pub(super) fn glide(from: (f32, f32), to: (f32, f32), secs: f32) -> Vec<(f32, Act)> {
    let steps = (secs / 0.04).max(1.) as usize;
    (1..=steps)
        .map(|i| {
            let k = i as f32 / steps as f32;
            let k = k * k * (3. - 2. * k);
            (
                0.04,
                Hover(
                    "starter",
                    from.0 + (to.0 - from.0) * k,
                    from.1 + (to.1 - from.1) * k,
                ),
            )
        })
        .collect()
}

/// A demo of one ambient background for a recording, with the stand-in's
/// busy afternoon (`DENT_AMBIENT_DEMO=1`): `scripts/walk.sh demo-prism`.
/// It prints `demo: rolling` and `demo: cut` around the part worth filming.
pub(super) fn demo(id: &'static str) -> Vec<(f32, Act)> {
    let _ = id;
    let mut v = vec![
        (0.3, Key("ctrl-cmd-s")),
        (0.3, Key("cmd-n")),
        (
            0.1,
            Call(|_, _, cx| {
                crate::ambient::choose(Some(*DEMO.lock().unwrap_or_else(|e| e.into_inner())));
                cx.notify();
            }),
        ),
        (0.1, Hover("starter", 0., 0.)),
        (1.5, Call(|_, _, _| eprintln!("demo: rolling"))),
    ];
    let pulse = || (0., Call(|d, _, _| d.ambient.borrow_mut().submit()));
    match *DEMO.lock().unwrap_or_else(|e| e.into_inner()) {
        "prism" => {
            v.push((2.0, Wait));
            v.extend(glide((-60., 0.), (-720., 160.), 1.2));
            v.extend(glide((-720., 160.), (-720., -200.), 3.0));
            // Aimed low enough, violet and then blue stay inside the glass.
            v.extend(glide((-720., -200.), (-640., -330.), 2.6));
            v.push((1.6, Wait));
            v.extend(glide((-640., -330.), (-740., -20.), 2.2));
            v.extend(glide((-740., -20.), (0., 0.), 0.8));
            v.push((1.2, Keys("Map every tool call this session made")));
            v.push(pulse());
            v.push((3.5, Wait));
        }
        "attention" => {
            v.push((1.2, Wait));
            v.extend(glide((0., 0.), (-650., -290.), 1.0));
            v.extend(glide((-650., -290.), (640., -290.), 4.0));
            v.push(pulse());
            v.extend(glide((640., -290.), (640., 320.), 3.0));
            v.extend(glide((640., 320.), (-640., 320.), 4.0));
            v.extend(glide((-640., 320.), (0., 0.), 1.0));
            v.push((1.0, Keys("Where did it spend its time?")));
            v.push((3.0, Wait));
        }
        _ => {
            // Stipple and the rest: a rise under the composer, then the
            // pointer lifts the ground as it circles the page.
            v.push((1.0, Wait));
            v.push(pulse());
            v.push((1.5, Wait));
            let mut at = (0., 0.);
            for k in 1..=48 {
                let a = k as f32 / 48. * std::f32::consts::TAU;
                let next = (650. * a.sin(), -320. * a.cos() + 0.);
                v.extend(glide(at, next, 0.2));
                at = next;
            }
            v.extend(glide(at, (0., 0.), 0.8));
            v.push((1.0, Keys("Survey the whole codebase")));
            v.push((3.0, Wait));
        }
    }
    v.push((0., Call(|_, _, _| eprintln!("demo: cut"))));
    v.push((1.0, Wait));
    v
}
