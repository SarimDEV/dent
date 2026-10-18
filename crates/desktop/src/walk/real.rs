use super::*;

/// A short session against the real agent on its smallest model: tiny
/// prompts, so it costs next to nothing. Lenient about wording, strict
/// about the flow.
pub(super) fn real() -> Vec<(f32, Act)> {
    fn haiku(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
        d.defaults.model = Some("haiku".into());
        d.defaults.permission = dent_protocol::Permission::Ask;
        cx.notify();
    }
    fn done(d: &Dent) -> bool {
        status_is(d, Status::Done)
    }
    let mut v: Vec<(f32, Act)> = vec![(
        0.,
        Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
    )];
    v.push((0.2, Call(haiku)));
    v.extend(new_session("Reply with just the word ok."));
    v.extend([
        (0.1, Until("Haiku replies and the turn ends", |d, _| done(d) && says(d, "ok"), 60.)),
        (0.5, Shot("real-reply")),
        (0.2, Type(Field::Composer, "Use the Bash tool to run exactly: echo hi > hi.txt  Then reply with just: done")),
        (0.2, Key("enter")),
        (0.1, Until("it asks before running a command", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 60.)),
        (0.5, Shot("real-approval")),
        (0.2, Click("allow")),
        (0.1, Until("allowed, it runs and finishes", |d, _| done(d) && says(d, "done"), 60.)),
        (0.2, Type(Field::Composer, "Use the AskUserQuestion tool to ask me tea or coffee, with two options. Then reply with just my answer.")),
        (0.2, Key("enter")),
        (0.1, Until("it asks a question with choices", |d, _| has(d, |i| matches!(i, Item::Question { .. })), 60.)),
        (0.5, Shot("real-question")),
        (0.2, Click("opt-0-0")),
        (0.1, Until("the answer reaches it", |d, _| done(d), 60.)),
        (0.2, Type(Field::Composer, "Use the Write tool to create hello.html: a minimal page with an h1 saying Hello from Haiku. Then reply with just: written")),
        (0.2, Key("enter")),
        (0.1, Until("it asks before writing a file", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 60.)),
        (0.2, Click("allow-session")),
        (0.1, Until("the page opens in the viewer", |d, _| done(d) && has(d, |i| matches!(i, Item::Made { .. })), 60.)),
        (1.2, Shot("real-page")),
        (0.2, Call(attach_red)),
        (0.2, Type(Field::Composer, "What colour is this image? Reply with one word.")),
        (0.2, Key("enter")),
        (0.1, Until("it sees the attached picture", |d, _| done(d) && says(d, "ed"), 60.)),
        (0.5, Shot("real-image")),
        // Back to before the picture: the agent shouldn't know it saw one.
        (0.2, Call(|d, w, cx| {
            if let Some(m) = last_sent(d) {
                d.rewind(&m, false, w, cx);
            }
        })),
        (0.1, Until("the chat goes back", |d, _| !has(d, |i| matches!(i, Item::User { text, .. } if text.contains("red.png"))), 10.)),
        (0.2, Type(Field::Composer, "Did I send you an image in this conversation? Reply yes or no.")),
        (0.2, Key("enter")),
        (0.1, Until("after rewinding, it has no memory of the picture", |d, _| done(d) && current(d).is_some_and(|s| matches!(s.items.iter().rev().find(|i| matches!(i, Item::Text(_))), Some(Item::Text(t)) if t.to_lowercase().contains("no"))), 90.)),
        (0.5, Shot("real-rewound")),
        (0.2, Click("fork")),
        (0.1, Until("a fork opens", |d, _| current(d).is_some_and(|s| s.title.starts_with("Fork of")), 10.)),
        (0.2, Type(Field::Composer, "Which drink did I pick earlier in this conversation? One word.")),
        (0.2, Key("enter")),
        (0.1, Until("the fork remembers the conversation it came from", |d, _| done(d) && says(d, "Tea"), 90.)),
        (0.5, Shot("real-fork")),
        // Plan mode: a plan to approve, then the work.
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (0.1, Call(|d, _, cx| {
            d.defaults.model = Some("haiku".into());
            d.defaults.permission = dent_protocol::Permission::Plan;
            cx.notify();
        })),
        (0.2, Type(Field::Starter, "Plan how to add a file plan.txt containing the word yes. Keep the plan to two short lines, then present it.")),
        (0.2, Click("start")),
        (0.1, Until("in plan mode it proposes a plan", |d, _| has(d, |i| matches!(i, Item::Plan { .. })), 90.)),
        (0.5, Shot("real-plan")),
        (0.2, Click("plan-go")),
        (0.1, Until("approving the plan lets it work; it asks before writing", |d, _| has(d, |i| matches!(i, Item::Approval { .. })) || in_project("plan.txt").exists(), 90.)),
        (0.2, ClickIfShown("allow")),
        (0.1, Until("the planned file is written", |d, _| done(d) && in_project("plan.txt").exists(), 90.)),
        (0.3, Key("cmd-n")),
        (0.6, Shot("real-new")),
    ]);
    v
}

/// One long real turn, photographed in both themes every couple of seconds
/// until it settles, then read back from the top: frames to set beside
/// another app's. `DENT_WALK_PROMPT` is what to send and
/// `DENT_WALK_PERMISSION=full` never asks; otherwise an approval is
/// answered after it has been on screen a few seconds.
pub(super) fn compare() -> Vec<(f32, Act)> {
    thread_local! {
        static ASKED: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
        static SETTLED: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    }
    fn theme(d: &mut Dent, dark: bool, w: &mut Window, cx: &mut Context<Dent>) {
        d.forced_dark = Some(dark);
        d.apply_theme(w, cx);
        cx.notify();
    }
    fn scroll(d: &mut Dent, at: fn(usize) -> usize, cx: &mut Context<Dent>) {
        let n = current(d).map_or(0, |s| s.items.len());
        d.transcript.scroll_to(ListOffset {
            item_ix: at(n).min(n.saturating_sub(1)),
            offset_in_item: px(0.),
        });
        cx.notify();
    }
    fn both(v: &mut Vec<(f32, Act)>, name: &str) {
        v.extend([
            (0.0, Call(|d, w, cx| theme(d, false, w, cx))),
            (0.35, Shot(leak(format!("{name}-light")))),
            (0.2, Call(|d, w, cx| theme(d, true, w, cx))),
            (0.35, Shot(leak(format!("{name}-dark")))),
            (0.2, Call(|d, w, cx| theme(d, false, w, cx))),
        ]);
    }
    fn pace(d: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
        let now = Instant::now();
        let asking = has(d, |i| matches!(i, Item::Approval { .. }));
        let ended = status_is(d, Status::Done) && current(d).is_some_and(|s| s.items.len() > 4);
        let Some(walk) = d.walk.as_mut() else { return };
        if asking {
            let since = ASKED.with(|a| *a.get().get_or_insert(now));
            ASKED.with(|a| a.set(Some(since)));
            if now - since > Duration::from_secs(6) {
                ASKED.with(|a| a.set(None));
                walk.steps.insert(walk.ix + 1, (0.1, Click("allow")));
            }
            return;
        }
        ASKED.with(|a| a.set(None));
        if !ended {
            SETTLED.with(|s| s.set(None));
            return;
        }
        let since = SETTLED.with(|s| *s.get().get_or_insert(now));
        SETTLED.with(|s| s.set(Some(since)));
        if now - since < Duration::from_secs(6) {
            return;
        }
        // Settled: read it back from the top, a third at a time.
        walk.steps.truncate(walk.ix + 1);
        let mut v = Vec::new();
        let stops: [(&str, Call); 4] = [
            ("end-1-top", |d, _, cx| scroll(d, |_| 0, cx)),
            ("end-2-third", |d, _, cx| scroll(d, |n| n / 3, cx)),
            ("end-3-two-thirds", |d, _, cx| scroll(d, |n| n * 2 / 3, cx)),
            ("end-4-bottom", |d, _, cx| scroll(d, |n| n, cx)),
        ];
        for (name, go) in stops {
            v.push((0.3, Call(go)));
            both(&mut v, name);
        }
        walk.steps.extend(v);
    }
    fn setup(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
        d.defaults.model = Some("haiku".into());
        d.defaults.permission = match std::env::var("DENT_WALK_PERMISSION").as_deref() {
            Ok("full") => dent_protocol::Permission::Full,
            _ => dent_protocol::Permission::Ask,
        };
        cx.notify();
    }
    let prompt = leak(std::env::var("DENT_WALK_PROMPT").unwrap_or_default());
    let mut v: Vec<(f32, Act)> = vec![
        (
            0.,
            Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
        ),
        (0.2, Call(setup)),
    ];
    v.extend(new_session(prompt));
    for i in 0..600 {
        both(&mut v, &format!("{i:03}"));
        v.push((0.9, Call(pace)));
    }
    v
}
