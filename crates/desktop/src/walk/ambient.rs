use super::*;

/// Which background the ambient walk shows next.
pub(super) static AMBIENT_AT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Each background's frame rate and main-thread cost per frame, as timed.
/// Each entry also holds the frame rate it must reach: GPUI holds a window
/// without focus (as a walk's always is) to about 30 fps, to save energy.
pub(super) static AMBIENT_COSTS: std::sync::Mutex<Vec<(&'static str, f32, f32, f32)>> =
    std::sync::Mutex::new(Vec::new());

pub(super) static COST_FROM: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

/// The process's CPU time so far (user and system), in ms.
pub(super) fn cpu_ms() -> f64 {
    // SAFETY: getrusage only writes the struct it's given.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) } != 0 {
        return 0.;
    }
    let ms = |t: libc::timeval| t.tv_sec as f64 * 1000. + t.tv_usec as f64 / 1000.;
    ms(u.ru_utime) + ms(u.ru_stime)
}

pub(super) static CPU_FROM: std::sync::Mutex<Option<(Instant, f64)>> = std::sync::Mutex::new(None);

/// A working square, and nothing else moving: what it costs. It's drawn at
/// its own rate (`motion::BREATH_FPS`), not the display's, since GPUI
/// redraws the whole window for it. `DENT_BREATH_FPS=0 scripts/walk.sh
/// breath` measures it drawn every frame, for comparison.
pub(super) fn breath() -> Vec<(f32, Act)> {
    let mut v = new_session("make a plan slowly");
    v.extend([
        (
            0.1,
            Until(
                "the session works, its square showing",
                // With Reduce Motion it shows, still, and doesn't breathe.
                |d, _| status_is(d, Status::Working) && (d.breathing() || crate::reduce_motion()),
                10.,
            ),
        ),
        (0.6, Shot("working-square")),
        // The inbox has the keyboard, so no caret blinks (Esc would stop
        // the turn).
        (3.5, Call(|d, w, cx| d.focus_inbox(w, cx))),
        (
            0.5,
            Call(|_, _, _| {
                *CPU_FROM.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some((Instant::now(), cpu_ms()));
            }),
        ),
        (
            0.,
            Idle(
                "a working square alone draws at its own rate, not the display's",
                3.,
                crate::ds::motion::BREATH_FPS + 3.,
            ),
        ),
        (
            0.,
            Call(|_, _, _| {
                if let Some((at, from)) = CPU_FROM.lock().unwrap_or_else(|e| e.into_inner()).take()
                {
                    let secs = at.elapsed().as_secs_f64().max(0.001);
                    eprintln!(
                        "      a working square costs {:.1} ms of CPU a second",
                        (cpu_ms() - from) / secs
                    );
                }
            }),
        ),
        (0.1, Key("cmd-.")),
        (
            0.1,
            Until("and stops", |d, _| !status_is(d, Status::Working), 10.),
        ),
    ]);
    v
}

pub(super) fn start_cost(d: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    d.ambient.borrow_mut().take_cost();
    *COST_FROM.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

pub(super) fn record_cost(d: &mut Dent, window: &mut Window, _: &mut Context<Dent>) {
    let cost = d.ambient.borrow_mut().take_cost();
    let secs = COST_FROM
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .map_or(1., |t| t.elapsed().as_secs_f32().max(0.001));
    let id = crate::ambient::chosen().map_or("none", |s| s.id);
    let fps = cost.frames as f32 / secs;
    let ms = cost.busy.as_secs_f32() * 1000. / cost.frames.max(1) as f32;
    let floor = if window.is_window_active() { 50. } else { 20. };
    eprintln!("      {id:<10} {fps:>6.1} fps (at least {floor})  {ms:>5.2} ms a frame");
    AMBIENT_COSTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, fps, ms, floor));
}

pub(super) fn next_ambient(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let i = AMBIENT_AT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    crate::ambient::choose(dent_ambient::ALL.get(i).map(|s| s.id));
    let _ = d;
    cx.notify();
}

/// Ambient backgrounds (docs/ambient.md): a still of each on the new-session
/// page, then proof that frames stop with none chosen and once a session
/// starts. `scripts/walk.sh ambient --shots DIR [--dark]`.
pub(super) fn ambient() -> Vec<(f32, Act)> {
    let mut v = vec![
        (
            0.,
            Call(|_, _, _| {
                AMBIENT_AT.store(0, std::sync::atomic::Ordering::Relaxed);
                AMBIENT_COSTS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clear();
            }),
        ),
        (0.3, Key("cmd-n")),
    ];
    for s in dent_ambient::ALL {
        let name: &'static str = Box::leak(format!("ambient-{}", s.id).into_boxed_str());
        v.push((0.1, Call(next_ambient)));
        // Backgrounds prefill their history; the longest fade-in is 0.5 s.
        v.push((0.6, Shot(name)));
        // Then a second of frames, timed: the real cost on the main thread.
        v.push((0., Call(start_cost)));
        v.push((1.0, Call(record_cost)));
    }
    v.push((
        0.,
        Until(
            "every background holds its frame rate at under 4 ms a frame",
            |_, _| {
                let costs = AMBIENT_COSTS.lock().unwrap_or_else(|e| e.into_inner());
                // A debug build is too slow to judge: there it only reports.
                costs.len() == dent_ambient::ALL.len()
                    && (cfg!(debug_assertions)
                        || costs
                            .iter()
                            .all(|(_, fps, ms, floor)| *fps >= *floor && *ms <= 4.))
            },
            0.5,
        ),
    ));
    v.extend([
        (
            0.1,
            Call(|d, _, cx| {
                crate::ambient::choose(None);
                d.save_prefs(cx);
                cx.notify();
            }),
        ),
        (0.3, Idle("no background draws no frames", 2., 2.5)),
        (
            0.1,
            Call(|_, _, cx| {
                crate::ambient::choose(Some("prism"));
                cx.notify();
            }),
        ),
        (0.3, Keys("Draw the footer again")),
        (0.6, Shot("ambient-typing")),
        (0.1, Key("enter")),
        (
            0.,
            Until("the new session opens", |d, _| d.selected.is_some(), 10.),
        ),
        (
            0.,
            Until(
                "a started session stops the background",
                |d, _| !crate::ambient::wants_frames(d),
                2.,
            ),
        ),
        (
            0.1,
            Call(|_, _, cx| {
                crate::ambient::choose(None);
                cx.notify();
            }),
        ),
    ]);
    v
}
