use super::*;

pub(super) fn status_is(d: &Dent, s: Status) -> bool {
    current(d).is_some_and(|x| x.status == s)
}

pub(super) fn has(d: &Dent, f: fn(&Item) -> bool) -> bool {
    current(d).is_some_and(|s| s.items.iter().any(f))
}

pub(super) fn says(d: &Dent, needle: &str) -> bool {
    current(d).is_some_and(|s| {
        s.items
            .iter()
            .any(|i| matches!(i, Item::Text(t) if t.contains(needle)))
    })
}

pub(super) fn done_saying(d: &Dent, needle: &str) -> bool {
    status_is(d, Status::Done) && says(d, needle)
}

pub(super) fn streaming(d: &Dent) -> bool {
    status_is(d, Status::Working)
        && current(d).is_some_and(|s| matches!(s.items.last(), Some(Item::Text(t)) if t.len() > 40))
}

/// The scratch project the walk script made.
pub(super) fn use_project(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    d.defaults.project = std::env::var("DENT_WALK_PROJECT").ok();
    cx.notify();
}

/// A small red picture in the project, attached as if dropped on the window.
pub(super) fn attach_red(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    let dir = std::env::var("DENT_WALK_PROJECT").unwrap_or_else(|_| "/tmp".into());
    let path = std::path::Path::new(&dir).join("red.png");
    let img = image::RgbaImage::from_pixel(48, 48, image::Rgba([220, 30, 30, 255]));
    let _ = img.save(&path);
    d.attach_paths(&[path], w, cx);
}

/// The last message you sent in the open session.
pub(super) fn last_sent(d: &Dent) -> Option<String> {
    current(d)?.items.iter().rev().find_map(|i| match i {
        Item::User { id, .. } => Some(id.clone()),
        _ => None,
    })
}

/// A file in the walk's scratch project. Scenes write and commit there, so
/// without one set (a walk started without scripts/walk.sh) they would land
/// in whatever folder the app runs from, the dent checkout included: stop.
pub(super) fn in_project(name: &str) -> std::path::PathBuf {
    match std::env::var("DENT_WALK_PROJECT") {
        Ok(dir) if !dir.is_empty() => std::path::Path::new(&dir).join(name),
        _ => panic!(
            "walk: DENT_WALK_PROJECT is not set; run walks through scripts/walk.sh, \
             which makes a scratch project (scenes write and commit files there)"
        ),
    }
}

/// Kill the daemon, as a crash or an upgrade would.
pub(super) fn kill_daemon(_: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    let state = crate::live::default_state();
    let _ = std::process::Command::new("pkill")
        .args(["-f", &format!("dent-daemon --state {}", state.display())])
        .status();
}

pub(super) fn new_session(text: &'static str) -> Vec<(f32, Act)> {
    vec![
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (0.2, Type(Field::Starter, text)),
        (0.2, Click("start")),
        (
            0.1,
            Until(
                "a new session opens",
                |d, _| current(d).is_some() && !d.creating,
                5.,
            ),
        ),
    ]
}

/// Scroll the conversation so its last reply starts at the top.
pub(super) fn to_reply(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let at = current(d).and_then(|s| s.items.iter().rposition(|i| matches!(i, Item::Text(_))));
    if let Some(ix) = at {
        d.transcript.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: px(0.),
        });
        cx.notify();
    }
}

pub(super) fn to_end(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    d.transcript.scroll_to_end();
    cx.notify();
}

pub(super) fn clipboard_text(cx: &App) -> String {
    cx.read_from_clipboard()
        .and_then(|c| c.text())
        .unwrap_or_default()
}

// ---- Scenes ----

pub(super) fn mine(d: &Dent) -> usize {
    current(d).map_or(0, |s| {
        s.items
            .iter()
            .filter(|i| matches!(i, Item::User { .. }))
            .count()
    })
}

pub(super) fn composer_is(d: &Dent, cx: &App, text: &str) -> bool {
    d.composer.read(cx).value() == text
}

/// Walkthroughs don't save as they go; this saves once, as dent would.
pub(super) fn save_now(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    d.save_prefs(cx);
}

thread_local! {
    /// A number a step notes for later steps to check against.
    pub(super) static MARK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn mark() -> usize {
    MARK.with(|m| m.get())
}

/// The turn is over and a reply follows your last message.
pub(super) fn answered(d: &Dent) -> bool {
    status_is(d, Status::Done)
        && current(d).is_some_and(|s| {
            let last_mine = s.items.iter().rposition(|i| matches!(i, Item::User { .. }));
            let last_reply = s.items.iter().rposition(|i| matches!(i, Item::Text(_)));
            last_reply > last_mine
        })
}

/// Send `text` and wait until it's answered.
pub(super) fn ask_and_wait(text: &'static str) -> Vec<(f32, Act)> {
    vec![
        (0.2, Type(Field::Composer, text)),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                leak(format!("“{text}” is answered")),
                |d, _| answered(d),
                20.,
            ),
        ),
    ]
}

thread_local! {
    /// Whether the current ⌘F match was on screen when last looked.
    pub(super) static SHOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn shown(_: &Dent, _: &App) -> bool {
    SHOWN.with(|v| v.get())
}

pub(super) fn titled(d: &Dent, title: &str) -> bool {
    current(d).is_some_and(|s| s.title == title)
}

/// Letting go of Ctrl, which a walkthrough can't do with a key.
pub(super) fn let_go(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    d.switch_finish(w, cx);
}

pub(super) fn queued_ids(d: &Dent) -> Vec<(usize, String, String)> {
    current(d).map_or(Vec::new(), |s| {
        s.items
            .iter()
            .enumerate()
            .filter_map(|(ix, i)| match i {
                Item::Queued { id, text, .. } => Some((ix, id.clone(), text.clone())),
                _ => None,
            })
            .collect()
    })
}

pub(super) fn sent_order(d: &Dent) -> Vec<String> {
    current(d).map_or(Vec::new(), |s| {
        s.items
            .iter()
            .filter_map(|i| match i {
                Item::User { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    })
}

/// The paste the walk puts on the clipboard: sixty lines of a log.
pub(super) fn copy_long_log(_: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let log: String = (1..=60)
        .map(|n| {
            format!(
                "2026-10-16 09:{:02}:{:02} INFO sync: applied event {n}\n",
                n / 60,
                n % 60
            )
        })
        .collect();
    cx.write_to_clipboard(ClipboardItem::new_string(log));
}

/// The composer's folded pastes, by their labels.
pub(super) fn chips(d: &Dent, cx: &App) -> Vec<String> {
    d.composer
        .read(cx)
        .tokens()
        .iter()
        .map(|t| t.token().label().to_string())
        .collect()
}

/// The waiting messages: their words, and their files by name.
pub(super) fn queued_files(d: &Dent) -> Vec<(String, Vec<String>)> {
    current(d).map_or(Vec::new(), |s| {
        s.items
            .iter()
            .filter_map(|i| match i {
                Item::Queued {
                    text, attachments, ..
                } => Some((
                    text.clone(),
                    attachments
                        .iter()
                        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    })
}

/// A file in the folder outside the project.
pub(super) fn outside(name: &str) -> std::path::PathBuf {
    std::path::Path::new(&std::env::var("DENT_WALK_OUTSIDE").unwrap_or_else(|_| "/tmp".into()))
        .join(name)
}

pub(super) fn queued_now(d: &Dent) -> bool {
    has(d, |i| matches!(i, Item::Queued { .. }))
}
