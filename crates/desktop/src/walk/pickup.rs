use super::*;

thread_local! {
    /// Where the composer was before the pick-up menu opened.
    pub(super) static PICKUP_TOP: std::cell::Cell<f32> = const { std::cell::Cell::new(f32::NAN) };
}

/// The new-session page with many terminal sessions to pick up: opening
/// one moves nothing.
pub(super) fn pickup() -> Vec<(f32, Act)> {
    vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "terminal sessions are listed",
                |d, _| d.recent.len() >= 6,
                5.,
            ),
        ),
        (1.0, Shot("pickup")),
        (
            0.1,
            Call(|d, _, _| {
                let top = d.probes.borrow().get("starter").map(|b| b.top().as_f32());
                PICKUP_TOP.with(|t| t.set(top.unwrap_or(f32::NAN)));
            }),
        ),
        (0.2, Click("chip-pickup")),
        (0.6, Shot("pickup-open")),
        (
            0.1,
            Until(
                "opening it moves nothing",
                |d, _| {
                    d.probes.borrow().get("starter").is_some_and(|b| {
                        (b.top().as_f32() - PICKUP_TOP.with(|t| t.get())).abs() < 0.5
                    })
                },
                1.,
            ),
        ),
    ]
}
