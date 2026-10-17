//! ⌘/ (or ? with no text field in use): every shortcut on one sheet, by
//! what it's for. Esc or a click outside puts it away.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::Dent;
use crate::ds::{LABEL, Typed, UI, motion, radius, space};
use crate::ui::{Arrive, From, floating, kbd, probe};

/// Where a key means what it does. Keys bound through gpui name its
/// key context; the rest are read by `Dent::intercept` (or a field, or the
/// card's capture), in the state each names.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum When {
    Anywhere,
    /// The window, before a text field's own binding.
    Dent,
    Chat,
    /// A text field inside the chat (the reply box).
    ChatInput,
    Terminal,
    /// Any text field.
    Input,
    Inbox,
    /// The reply box or a new session's field.
    Prompt,
    /// The reply box or a new session's field, empty.
    EmptyPrompt,
    /// The reply box, empty or showing a message brought back unchanged.
    Recall,
    /// The reply box empty, or no text field.
    NoText,
    /// The prompt's completion menu shows.
    Completion,
    /// A waiting approval or plan's card has the keyboard.
    Card,
    /// A question waits and no text field has the keyboard.
    Question,
    /// No text field has the keyboard.
    NoField,
}

impl When {
    /// The gpui key context a binding for it uses.
    fn context(self) -> Option<&'static str> {
        match self {
            When::Dent => Some("Dent"),
            When::Chat => Some("Chat"),
            When::ChatInput => Some("Chat > Input"),
            When::Terminal => Some("Terminal"),
            When::Input => Some("Input"),
            When::Inbox => Some("Inbox"),
            _ => None,
        }
    }
}

/// The sections of the shortcut sheet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Sessions,
    Writing,
    Reading,
    Panels,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Sessions,
        Section::Writing,
        Section::Reading,
        Section::Panels,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Section::Sessions => "Sessions",
            Section::Writing => "Writing",
            Section::Reading => "Reading",
            Section::Panels => "Panels",
        }
    }
}

type Bind = fn(&'static str, Option<&'static str>) -> KeyBinding;

/// One key's meaning: the key (as gpui writes it), where it means this,
/// the command (one name per meaning), how it's bound (`None`: read by
/// `Dent::intercept`, a field or the card's capture), and its rows on the
/// shortcut sheet and in docs/desktop.md's table.
pub struct Key {
    pub keys: &'static str,
    pub when: When,
    /// What it does there; the test that no key means two things in one
    /// place reads it.
    #[allow(dead_code)]
    pub does: &'static str,
    pub bind: Option<Bind>,
    pub sheet: Option<(Section, &'static str, &'static str)>,
    pub doc: Option<(&'static str, &'static str)>,
}

const fn key(keys: &'static str, when: When, does: &'static str) -> Key {
    Key {
        keys,
        when,
        does,
        bind: None,
        sheet: None,
        doc: None,
    }
}

impl Key {
    const fn bound(mut self, b: Bind) -> Key {
        self.bind = Some(b);
        self
    }
    const fn sheet(mut self, s: Section, what: &'static str, shown: &'static str) -> Key {
        self.sheet = Some((s, what, shown));
        self
    }
    const fn doc(mut self, what: &'static str, keys: &'static str) -> Key {
        self.doc = Some((what, keys));
        self
    }
}

macro_rules! to {
    ($a:expr) => {
        |k: &'static str, c: Option<&'static str>| KeyBinding::new(k, $a, c)
    };
}

use crate::app::{
    AttachFiles, ChatBottom, ChatPageDown, ChatPageUp, ChatTop, ClearTerminal, CloseSession,
    Escape, FindInSession, FindNext, FindPrev, FullScreen, Hide, HideOthers, InboxDown, InboxOpen,
    InboxUp, Minimize, NewSession, NextNeedsYou, Quit, Search, SelectNext, SelectPrev, ShowChanges,
    ShowFirst, ShowKeys, ShowTerminal, SideChat, SteerSend, StopSession, SwitchNext, SwitchPrev,
    TextActual, TextBigger, TextSmaller, TogglePanel, ToggleSidebar, UndoClose,
};
use Section::*;
use When::*;
use gpui_kit::base::input;

/// Every key dent gives a meaning, once. `bind_keys` binds from it, the
/// shortcut sheet is drawn from it and docs/desktop.md's table is checked
/// against it; a test fails if a key means two things in one place.
pub const KEYS: &[Key] = &[
    // Sessions.
    key("cmd-n", Anywhere, "new session")
        .bound(to!(NewSession))
        .sheet(Sessions, "New session", "⌘ N")
        .doc("New session", "Cmd/Ctrl+N"),
    key("cmd-k", Anywhere, "search sessions")
        .bound(to!(Search))
        .sheet(Sessions, "Search sessions", "⌘ K")
        .doc("Command palette", "Cmd/Ctrl+K"),
    key("ctrl-tab", Anywhere, "back to the last session")
        .bound(to!(SwitchNext))
        .sheet(Sessions, "Back to the last session", "⌃ Tab")
        .doc(
            "Back to the last session; further back while held",
            "Ctrl+Tab (Ctrl+Shift+Tab the other way)",
        ),
    key("ctrl-shift-tab", Anywhere, "forward to the next session").bound(to!(SwitchPrev)),
    key("alt-cmd-up", Anywhere, "previous session")
        .bound(to!(SelectPrev))
        .sheet(Sessions, "Previous or next session", "⌥ ⌘ ↑ ↓"),
    key("alt-cmd-down", Anywhere, "next session").bound(to!(SelectNext)),
    key("cmd-j", Anywhere, "next session that needs you")
        .bound(to!(NextNeedsYou))
        .sheet(Sessions, "Next session that needs you", "⌘ J")
        .doc("Next blocked session", "Cmd/Ctrl+J"),
    key("cmd-w", Anywhere, "close session")
        .bound(to!(CloseSession))
        .sheet(Sessions, "Close session", "⌘ W")
        .doc("Close session", "Cmd/Ctrl+W"),
    key("cmd-z", Dent, "undo closing")
        .bound(to!(UndoClose))
        .sheet(Sessions, "Undo closing", "⌘ Z")
        .doc("Undo close", "Cmd/Ctrl+Z"),
    key("cmd-z", EmptyPrompt, "undo closing"),
    // Writing.
    key("enter", Prompt, "send")
        .sheet(Writing, "Send", "↩")
        .doc(
            "Send; while the agent works, queue it; while it asks, answer or deny with it",
            "Return",
        ),
    key("shift-enter", Input, "new line").sheet(Writing, "New line", "⇧ ↩"),
    key("alt-enter", Input, "new line").bound(|k, c| {
        KeyBinding::new(
            k,
            input::Enter {
                secondary: false,
                shift: true,
            },
            c,
        )
    }),
    key("shift-cmd-a", Anywhere, "attach files")
        .bound(to!(AttachFiles))
        .sheet(Writing, "Attach files", "⇧ ⌘ A")
        .doc("Attach files to the next message", "Shift+Cmd+A"),
    key("shift-cmd-v", Prompt, "paste text unfolded")
        .sheet(Writing, "Paste text unfolded", "⇧ ⌘ V")
        .doc("Paste text unfolded", "Shift+Cmd+V"),
    key(
        "cmd-v",
        Prompt,
        "paste, folding long text and attaching files",
    ),
    key("cmd-s", Prompt, "put the draft aside, or bring it back")
        .sheet(Writing, "Put the draft aside, or bring it back", "⌘ S")
        .doc(
            "Put the draft aside; in an empty box, bring one back",
            "Cmd+S",
        ),
    key("cmd-enter", Anywhere, "steer")
        .bound(to!(SteerSend))
        .sheet(Writing, "Send now, into the running turn", "⌘ ↩")
        .doc("Send now, into the running turn (steer)", "Cmd/Ctrl+Return"),
    key("cmd-.", Anywhere, "stop")
        .bound(to!(StopSession))
        .sheet(Writing, "Stop", "⌘ .  Esc")
        .doc("Stop the agent", "Cmd/Ctrl+., or Esc in the reply box"),
    key("escape", Anywhere, "back out (stop, put away, leave)").bound(to!(Escape)),
    key("up", Recall, "bring back what you sent")
        .sheet(Writing, "Bring back what you sent", "↑")
        .doc("Bring back what I sent (in an empty composer)", "Up / Down"),
    key("down", Recall, "step back toward the newest"),
    key("shift-tab", Prompt, "step the permission mode")
        .sheet(Writing, "Cycle the permission mode", "⇧ Tab")
        .doc(
            "Step the permission mode (Ask, Edits, Plan; never Full)",
            "Shift+Tab in the reply box or a new session",
        ),
    key("enter", Card, "allow once, or start building").sheet(
        Writing,
        "Answer a waiting card",
        "Esc, then 1–4",
    ),
    key("1", Card, "allow once, or start building"),
    key("2", Card, "allow for the session, or keep planning"),
    key("3", Card, "always allow in this project"),
    key("4", Card, "deny with what's typed"),
    key("1", Question, "pick option 1"),
    key("2", Question, "pick option 2"),
    key("3", Question, "pick option 3"),
    key("4", Question, "pick option 4"),
    key("5", Question, "pick option 5"),
    key("6", Question, "pick option 6"),
    key("7", Question, "pick option 7"),
    key("8", Question, "pick option 8"),
    key("9", Question, "pick option 9"),
    key("tab", Completion, "pick"),
    key("enter", Completion, "pick, or send what's typed in full"),
    key("up", Completion, "previous row"),
    key("down", Completion, "next row"),
    // Reading.
    key("cmd-f", Anywhere, "find")
        .bound(to!(FindInSession))
        .sheet(Reading, "Find in the session", "⌘ F")
        .doc(
            "Find in what you're reading; next / previous match",
            "Cmd+F; Cmd+G / Shift+Cmd+G",
        ),
    key("cmd-g", Anywhere, "next match")
        .bound(to!(FindNext))
        .sheet(Reading, "Next or previous match", "⌘ G  ⇧ ⌘ G"),
    key("shift-cmd-g", Anywhere, "previous match").bound(to!(FindPrev)),
    key("alt-up", NoText, "your previous message")
        .sheet(Reading, "Your earlier messages", "⌥ ↑ ↓")
        .doc("Previous / next of my messages", "Alt+Up / Alt+Down"),
    key("alt-down", NoText, "your next message"),
    key("pageup", Chat, "chat a page up")
        .bound(to!(ChatPageUp))
        .sheet(Reading, "A page up or down", "PgUp  PgDn"),
    key("pagedown", Chat, "chat a page down").bound(to!(ChatPageDown)),
    key("home", Chat, "chat to its start")
        .bound(to!(ChatTop))
        .sheet(Reading, "The start or end of the chat", "Home  End"),
    key("end", Chat, "chat to its end").bound(to!(ChatBottom)),
    key("pageup", ChatInput, "chat a page up").bound(to!(ChatPageUp)),
    key("pagedown", ChatInput, "chat a page down").bound(to!(ChatPageDown)),
    key("home", ChatInput, "chat to its start").bound(to!(ChatTop)),
    key("end", ChatInput, "chat to its end").bound(to!(ChatBottom)),
    key("cmd-=", Anywhere, "bigger text")
        .bound(to!(TextBigger))
        .sheet(Reading, "Bigger or smaller text", "⌘ +  ⌘ −")
        .doc("Bigger / smaller text; as designed", "Cmd+= / Cmd+−; Cmd+0"),
    key("cmd-+", Anywhere, "bigger text").bound(to!(TextBigger)),
    key("cmd--", Anywhere, "smaller text").bound(to!(TextSmaller)),
    key("cmd-0", Anywhere, "text as designed")
        .bound(to!(TextActual))
        .sheet(Reading, "Text as designed", "⌘ 0"),
    // Panels.
    key("ctrl-cmd-s", Anywhere, "show or hide the sidebar")
        .bound(to!(ToggleSidebar))
        .sheet(Panels, "Show or hide the sidebar", "⌃ ⌘ S"),
    key("cmd-\\", Anywhere, "show or hide the viewer")
        .bound(to!(TogglePanel))
        .sheet(Panels, "Show or hide the viewer", "⌘ \\")
        .doc("Toggle the viewer", "Cmd/Ctrl+\\\\"),
    key("cmd-1", Anywhere, "viewer: first artifact")
        .bound(to!(ShowFirst))
        .doc(
            "Viewer: first artifact / changes / terminal",
            "Cmd/Ctrl+1 / 2 / 3",
        ),
    key("cmd-2", Anywhere, "viewer: changes")
        .bound(to!(ShowChanges))
        .sheet(Panels, "Changes", "⌘ 2"),
    key("cmd-3", Anywhere, "viewer: your terminal")
        .bound(to!(ShowTerminal))
        .sheet(Panels, "Your terminal", "⌘ 3"),
    key("ctrl-`", Anywhere, "viewer: your terminal").bound(to!(ShowTerminal)),
    key("cmd-k", Terminal, "clear your terminal")
        .bound(to!(ClearTerminal))
        .sheet(Panels, "Clear your terminal (in it)", "⌘ K"),
    key("cmd-;", Anywhere, "ask on the side")
        .bound(to!(SideChat))
        .sheet(Panels, "Ask on the side", "⌘ ;"),
    key("cmd-/", Anywhere, "this sheet")
        .bound(to!(ShowKeys))
        .sheet(Panels, "This sheet", "⌘ /")
        .doc(
            "Every shortcut on one sheet",
            "Cmd+/ (or ? with no text field in use)",
        ),
    key("?", NoField, "this sheet"),
    // The app and window.
    key("cmd-q", Anywhere, "quit").bound(to!(Quit)),
    key("cmd-h", Anywhere, "hide").bound(to!(Hide)),
    key("alt-cmd-h", Anywhere, "hide others").bound(to!(HideOthers)),
    key("cmd-m", Anywhere, "minimize").bound(to!(Minimize)),
    key("ctrl-cmd-f", Anywhere, "full screen").bound(to!(FullScreen)),
    // The Mac's Control keys in every text box, as anywhere else: the
    // field brings ⌃A and ⌃E, these are the rest that get used.
    key("ctrl-f", Input, "caret right").bound(to!(input::MoveRight)),
    key("ctrl-b", Input, "caret left").bound(to!(input::MoveLeft)),
    key("ctrl-n", Input, "caret down").bound(to!(input::MoveDown)),
    key("ctrl-p", Input, "caret up").bound(to!(input::MoveUp)),
    key("ctrl-d", Input, "delete forward").bound(to!(input::Delete)),
    key("ctrl-h", Input, "delete back").bound(to!(input::Backspace)),
    key("ctrl-k", Input, "delete to the line's end").bound(to!(input::DeleteToEndOfLine)),
    // With the inbox focused (Esc from an empty composer), it works like mail.
    key("up", Inbox, "previous session in the list").bound(to!(InboxUp)),
    key("down", Inbox, "next session in the list").bound(to!(InboxDown)),
    key("enter", Inbox, "open the session").bound(to!(InboxOpen)),
];

/// Every binding the registry makes.
pub fn bindings() -> Vec<KeyBinding> {
    KEYS.iter()
        .filter_map(|k| k.bind.map(|b| b(k.keys, k.when.context())))
        .collect()
}

/// The shortcut sheet, by section: (what it does, its keys).
pub fn sheet() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    Section::ALL
        .iter()
        .map(|s| {
            (
                s.name(),
                KEYS.iter()
                    .filter_map(|k| k.sheet.filter(|(at, ..)| at == s))
                    .map(|(_, what, shown)| (what, shown))
                    .collect(),
            )
        })
        .collect()
}

/// docs/desktop.md's table of default shortcuts, as the registry has it.
#[cfg(test)]
pub fn doc_table() -> String {
    let mut out = String::from("| Action | Default |\n| --- | --- |\n");
    for (what, keys) in KEYS.iter().filter_map(|k| k.doc) {
        out.push_str(&format!("| {what} | {keys} |\n"));
    }
    out
}

impl Dent {
    pub fn toggle_keys(&mut self, cx: &mut Context<Self>) {
        self.keys_open = !self.keys_open;
        cx.notify();
    }

    pub fn keys_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.keys_open {
            return None;
        }
        let t = self.t;
        let section = |(name, rows): &(&'static str, Vec<(&'static str, &'static str)>)| {
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(
                    div()
                        .pb(px(space::S1))
                        .ty(LABEL)
                        .text_color(t.quiet)
                        .child(name.to_uppercase()),
                )
                .children(rows.iter().map(|(what, keys)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S3))
                        .min_h(px(24.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .ty(UI)
                                .text_color(t.ink)
                                .child(*what),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(space::S1))
                                .children(keys.split("  ").map(|k| kbd(k, &t))),
                        )
                }))
        };
        let sheet = sheet();
        let (left, right) = sheet.split_at(2);
        Some(
            div()
                .id("keys-scrim")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(if t.dark {
                    gpui_kit::black().opacity(0.45)
                } else {
                    t.ink.opacity(0.08)
                })
                .flex()
                .justify_center()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.keys_open = false;
                        cx.notify();
                    }),
                )
                .child(
                    floating(&t)
                        .id("keys")
                        .relative()
                        .w(px(680.))
                        .max_w(relative(0.92))
                        .max_h(relative(0.86))
                        .overflow_y_scroll()
                        .rounded(px(radius::XL))
                        .p(px(space::S6))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .pb(px(space::S4))
                                .ty(UI)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.ink)
                                .child("Keyboard shortcuts"),
                        )
                        .child(div().flex().flex_wrap().gap(px(space::S8)).children(
                            [left, right].map(|col| {
                                div()
                                    .flex_1()
                                    .min_w(px(260.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(space::S6))
                                    .children(col.iter().map(section))
                            }),
                        ))
                        .child(probe(&self.probes, "keys"))
                        .arrive("keys-arrive", From::Above, motion::FAST),
                )
                .with_animation(
                    "keys-scrim-in",
                    Animation::new(Duration::from_millis(motion::FAST as u64))
                        .with_easing(crate::anim::ease_out),
                    |d, p| d.opacity(p),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{KEYS, When, doc_table, sheet};

    #[test]
    fn no_key_means_two_things_in_one_place() {
        let mut seen: std::collections::HashMap<(&str, When), &str> = Default::default();
        for k in KEYS {
            if let Some(was) = seen.insert((k.keys, k.when), k.does) {
                assert_eq!(
                    was, k.does,
                    "{} in {:?} means both {was:?} and {:?}",
                    k.keys, k.when, k.does
                );
            }
        }
    }

    #[test]
    fn the_docs_table_is_the_registry() {
        let doc = include_str!("../../../docs/desktop.md");
        let table = doc_table();
        assert!(
            doc.contains(&table),
            "docs/desktop.md's shortcut table differs from keys::KEYS; it should read:\n{table}"
        );
    }

    #[test]
    fn every_sheet_row_and_binding_parses() {
        for k in KEYS {
            if k.bind.is_some() {
                assert!(
                    gpui_kit::Keystroke::parse(k.keys.split_whitespace().next().unwrap()).is_ok(),
                    "{}",
                    k.keys
                );
            }
        }
        let rows: usize = sheet().iter().map(|(_, r)| r.len()).sum();
        assert_eq!(rows, KEYS.iter().filter(|k| k.sheet.is_some()).count());
    }
}
