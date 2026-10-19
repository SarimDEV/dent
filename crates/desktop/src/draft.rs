//! What a prompt holds, as one value: its words, the folded pastes among
//! them (chips, known by their paste number, never by their words), what
//! rides along (files, and notes such as a review comment) and, for a
//! message taken back out of the queue, the place it goes back to.
//!
//! Every draft dent keeps is one of these: a session's unsent draft, one put
//! aside with ⌘S, a message coming back to the box, a message being taken
//! back, and the prefs. Every programmatic edit of a prompt goes through
//! [`put_in`], which keeps the box's chips and records an edit ⌘Z undoes.
//! The lines that carry notes inside a message's text are written and read
//! here only ([`with_notes`], [`split_notes`]).

use std::collections::BTreeMap;
use std::ops::Range;

use gpui_kit::base::input::{InlineToken, InputContent, InputEvent, TextareaState};
use gpui_kit::{Context, SharedString, Window};
use serde::{Deserialize, Serialize};

/// Something riding along with the next message.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Attachment {
    /// An element picked in the preview.
    Element {
        file: String,
        tag: String,
        text: String,
        selector: String,
    },
    /// An error from the preview's console.
    Error { file: String, message: String },
    /// A file or picture, by its path.
    File { path: String },
    /// A review comment on a changed line.
    Comment {
        file: String,
        line: u32,
        text: String,
    },
}

impl Attachment {
    pub fn label(&self) -> String {
        match self {
            Attachment::Element { tag, text, .. } if !text.is_empty() => format!("{tag} “{text}”"),
            Attachment::Element { tag, .. } => tag.clone(),
            Attachment::Error { message, .. } => message.clone(),
            Attachment::Comment { file, line, .. } => {
                format!("{}:{line}", file.rsplit('/').next().unwrap_or(file))
            }
            Attachment::File { path } => path.rsplit('/').next().unwrap_or(path).to_string(),
        }
    }
    pub fn icon(&self) -> &'static str {
        match self {
            Attachment::Element { .. } => "square-dashed-mouse-pointer",
            Attachment::Error { .. } => "circle-alert",
            Attachment::Comment { .. } => "message-square",
            Attachment::File { path }
                if crate::data::Kind::of(path) == Some(crate::data::Kind::Image) =>
            {
                "image"
            }
            Attachment::File { path } if crate::data::file_icon(path).is_some() => {
                crate::data::file_icon(path).unwrap_or("paperclip")
            }
            Attachment::File { .. } => "paperclip",
        }
    }
    /// The file's path, for a file.
    pub fn path(&self) -> Option<&str> {
        match self {
            Attachment::File { path } => Some(path),
            _ => None,
        }
    }
    /// The line this note is in a message's text; files have none (they
    /// travel as the message's attachments).
    fn note_line(&self) -> Option<String> {
        match self {
            Attachment::Element {
                file,
                tag,
                text,
                selector,
            } => Some(format!("› {file} · {tag} “{text}” ({selector})")),
            Attachment::Error { file, message } => Some(format!("› {file} · {message}")),
            Attachment::Comment { file, line, text } => Some(format!("› {file}:{line} · {text}")),
            Attachment::File { .. } => None,
        }
    }
}

/// A folded paste in a draft: where its label sits in the text, and which
/// paste it is.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Chip {
    pub start: usize,
    pub end: usize,
    pub paste: u32,
}

impl Chip {
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// What a prompt holds. See the module comment.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(from = "Stored")]
pub struct Draft {
    /// The words as the box shows them: a chip as its label.
    pub text: String,
    pub chips: Vec<Chip>,
    pub atts: Vec<Attachment>,
    /// A waiting message taken back to edit: the waiting message it goes
    /// back before when sent again. Never saved.
    #[serde(skip)]
    pub place: Option<String>,
}

/// A draft as saved: a plain string (before chips were saved by number), a
/// put-aside draft's `{text, files}`, or a whole draft.
#[derive(Deserialize)]
#[serde(untagged)]
enum Stored {
    Text(String),
    Full {
        #[serde(default)]
        text: String,
        #[serde(default)]
        chips: Vec<Chip>,
        #[serde(default)]
        atts: Vec<Attachment>,
        #[serde(default)]
        files: Vec<String>,
    },
}

impl From<Stored> for Draft {
    fn from(s: Stored) -> Self {
        match s {
            Stored::Text(text) => Draft::words(text),
            Stored::Full {
                text,
                chips,
                mut atts,
                files,
            } => {
                atts.extend(files.into_iter().map(|path| Attachment::File { path }));
                let mut d = Draft {
                    text,
                    chips,
                    atts,
                    place: None,
                };
                d.chips.retain(|c| {
                    c.start <= c.end
                        && c.end <= d.text.len()
                        && d.text.is_char_boundary(c.start)
                        && d.text.is_char_boundary(c.end)
                });
                d
            }
        }
    }
}

/// The token id a chip has in a field.
pub fn chip_id(paste: u32) -> String {
    format!("paste-{paste}")
}

impl Draft {
    /// Words alone.
    pub fn words(text: impl Into<String>) -> Self {
        Draft {
            text: text.into(),
            ..Default::default()
        }
    }

    /// Words and what rides with them.
    pub fn with(text: impl Into<String>, atts: Vec<Attachment>) -> Self {
        Draft {
            text: text.into(),
            atts,
            ..Default::default()
        }
    }

    /// A message as it was sent (its text with any note lines, and its
    /// files), to bring back to a box: the words come back, files come
    /// back attached, and notes (a comment, a picked element) stay behind.
    pub fn of_sent(text: &str, files: &[String]) -> Self {
        let (words, _) = split_notes(text);
        Draft::with(
            words,
            files
                .iter()
                .map(|p| Attachment::File { path: p.clone() })
                .collect(),
        )
    }

    /// What a field holds now, with what rides along.
    pub fn read(field: &TextareaState, atts: Vec<Attachment>) -> Self {
        let text = field.value().to_string();
        let mut chips: Vec<Chip> = field
            .tokens()
            .iter()
            .filter_map(|t| {
                let r = t.range();
                crate::compose::paste_of(t.token().id().as_ref()).map(|paste| Chip {
                    start: r.start,
                    end: r.end,
                    paste,
                })
            })
            .collect();
        chips.sort_by_key(|c| c.start);
        Draft {
            text,
            chips,
            atts,
            place: None,
        }
    }

    /// Nothing typed and nothing riding along.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.atts.is_empty()
    }

    /// The paths of the files it carries.
    pub fn files(&self) -> Vec<String> {
        self.atts
            .iter()
            .filter_map(|a| a.path().map(str::to_string))
            .collect()
    }

    /// What rides along that isn't a file.
    pub fn notes(&self) -> Vec<Attachment> {
        self.atts
            .iter()
            .filter(|a| a.path().is_none())
            .cloned()
            .collect()
    }

    /// The words as they go to the agent: each chip opened to its paste,
    /// trimmed.
    pub fn words_out(&self, pastes: &BTreeMap<u32, String>) -> String {
        let ids: Vec<String> = self.chips.iter().map(|c| chip_id(c.paste)).collect();
        let chips: Vec<(Range<usize>, &str)> = self
            .chips
            .iter()
            .zip(&ids)
            .map(|(c, id)| (c.range(), id.as_str()))
            .collect();
        crate::compose::unfold_tokens(&self.text, &chips, pastes)
            .trim()
            .to_string()
    }

    /// The message's text as sent: its words with each note on a line
    /// after them (so a command stays first).
    pub fn message(&self, pastes: &BTreeMap<u32, String>) -> String {
        with_notes(&self.words_out(pastes), &self.notes())
    }

    /// The words as a list of them reads them: each chip as its chip's
    /// label ("Pasted text · 60 lines").
    pub fn shown(&self, pastes: &BTreeMap<u32, String>) -> String {
        let mut out = String::new();
        let mut at = 0;
        for c in &self.chips {
            if c.start < at {
                continue;
            }
            out.push_str(&self.text[at..c.start]);
            match pastes.get(&c.paste) {
                Some(full) => out.push_str(&crate::compose::chip_label(full)),
                None => out.push_str(&self.text[c.range()]),
            }
            at = c.end;
        }
        out.push_str(&self.text[at..]);
        out
    }

    /// The words with the chips taken out (the words around each closing
    /// up) and the chips' labels: a queued message shows its pastes as
    /// chips beside its words, as the box did.
    pub fn chips_apart(&self, pastes: &BTreeMap<u32, String>) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut labels = Vec::new();
        let mut at = 0;
        for c in &self.chips {
            if c.start < at {
                continue;
            }
            let a = out.clone() + &self.text[at..c.start];
            let a = a.trim_end_matches([' ', '\n']).to_string();
            out = a;
            at = c.end;
            let rest = self.text[at..].trim_start_matches([' ', '\n']);
            at = self.text.len() - rest.len();
            if !out.is_empty() && !rest.is_empty() {
                out.push(' ');
            }
            labels.push(match pastes.get(&c.paste) {
                Some(full) => crate::compose::chip_label(full),
                None => self.text[c.range()].to_string(),
            });
        }
        out.push_str(&self.text[at..]);
        (out, labels)
    }

    /// The field's content: text with its chips as tokens.
    pub fn content(&self, pastes: &BTreeMap<u32, String>) -> InputContent {
        let mut content = InputContent::new(self.text.clone());
        for c in &self.chips {
            let Some(full) = pastes.get(&c.paste) else {
                continue;
            };
            let token = InlineToken::new(chip_id(c.paste), self.text[c.range()].to_string())
                .with_label(crate::compose::chip_label(full));
            if let Ok(next) = content.clone().with_token(c.range(), token) {
                content = next;
            }
        }
        content
    }

    /// The pastes it holds, by number.
    pub fn pastes(&self) -> impl Iterator<Item = u32> + '_ {
        self.chips.iter().map(|c| c.paste)
    }

    /// `more` added beside this draft, a blank line between (after it, or
    /// with `first` before it); what rides with `more` joins what rides
    /// here. Chips keep their pastes.
    pub fn join(&mut self, more: Draft, first: bool) {
        for a in &more.atts {
            if !self.atts.contains(a) {
                self.atts.push(a.clone());
            }
        }
        if more.place.is_some() {
            self.place = more.place.clone();
        }
        if more.text.trim().is_empty() {
            return;
        }
        if self.text.trim().is_empty() {
            self.text = more.text;
            self.chips = more.chips;
            return;
        }
        let mine = (
            std::mem::take(&mut self.text),
            std::mem::take(&mut self.chips),
        );
        let theirs = (more.text, more.chips);
        let ((a, a_chips), (b, b_chips)) = if first {
            (theirs, mine)
        } else {
            (mine, theirs)
        };
        let head = a.trim_end();
        let lead = b.len() - b.trim_start().len();
        let shift = head.len() + 2;
        self.text = format!("{head}\n\n{}", &b[lead..]);
        self.chips = a_chips
            .into_iter()
            .filter(|c| c.end <= head.len())
            .collect();
        self.chips.extend(
            b_chips
                .into_iter()
                .filter(|c| c.start >= lead)
                .map(|c| Chip {
                    start: c.start - lead + shift,
                    end: c.end - lead + shift,
                    paste: c.paste,
                }),
        );
    }

    /// The words between chips, in order, with each chip: what [`put_in`]
    /// types into a field piece by piece.
    fn pieces(&self) -> Vec<Piece> {
        let mut out = Vec::new();
        let mut at = 0;
        for c in &self.chips {
            if c.start < at {
                continue;
            }
            if c.start > at {
                out.push(Piece::Words(self.text[at..c.start].to_string()));
            }
            out.push(Piece::Chip(c.clone()));
            at = c.end;
        }
        if at < self.text.len() {
            out.push(Piece::Words(self.text[at..].to_string()));
        }
        out
    }
}

enum Piece {
    Words(String),
    Chip(Chip),
}

/// Where [`put_in`] puts a draft in a field.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// Instead of everything.
    Replace,
    /// Before what's typed, a blank line between.
    Before,
    /// After what's typed, a blank line between.
    After,
    /// Over this range, the caret after it.
    At(Range<usize>),
}

/// Put draft `d`'s words in a field, chips and all. With `undoable` it's
/// an edit ⌘Z takes back (a piece at a time when it holds chips); without,
/// it's loading a stored draft, which starts the field's history afresh.
/// Only the range that changes is touched, so chips already in the box
/// stay chips. What rides along is the caller's to attach.
pub fn put_in(
    field: &mut TextareaState,
    place: Place,
    d: &Draft,
    pastes: &BTreeMap<u32, String>,
    undoable: bool,
    window: &mut Window,
    cx: &mut Context<TextareaState>,
) {
    let typed = field.value().to_string();
    if !undoable && place == Place::Replace {
        field.set_value(d.content(pastes), window, cx);
        let end = d.text.len();
        field.set_selected_range(end..end, cx);
        cx.emit(InputEvent::Change);
        return;
    }
    // What's replaced, and the separators around the draft.
    let (range, lead, trail) = match place {
        Place::Replace => (0..typed.len(), "", ""),
        _ if typed.trim().is_empty() && !matches!(place, Place::At(_)) => (0..typed.len(), "", ""),
        Place::Before => {
            let lead = typed.len() - typed.trim_start().len();
            (0..lead, "", "\n\n")
        }
        Place::After => {
            let end = typed.trim_end().len();
            (end..typed.len(), "\n\n", "")
        }
        Place::At(r) => (r, "", ""),
    };
    if d.text.is_empty() && range.is_empty() {
        return;
    }
    let mut pieces = d.pieces();
    if !lead.is_empty() {
        pieces.insert(0, Piece::Words(lead.to_string()));
    }
    if !trail.is_empty() {
        pieces.push(Piece::Words(trail.to_string()));
    }
    // The range is replaced by the first piece; each next one goes in at
    // the caret.
    field.set_selected_range(range, cx);
    if pieces.is_empty() {
        field.replace(SharedString::default(), window, cx);
    }
    for p in pieces {
        match p {
            Piece::Words(w) => field.replace(SharedString::from(w), window, cx),
            Piece::Chip(c) => {
                let label = d.text[c.range()].to_string();
                let mut token = InlineToken::new(chip_id(c.paste), label.clone());
                if let Some(full) = pastes.get(&c.paste) {
                    token = token.with_label(crate::compose::chip_label(full));
                }
                if field.replace_with_token(token, window, cx).is_err() {
                    field.replace(SharedString::from(label), window, cx);
                }
                let end = field.selected_range().end;
                field.set_selected_range(end..end, cx);
            }
        }
    }
    cx.emit(InputEvent::Change);
}

/// A message's words with each note on a line after them, so one starting
/// with `/` still reaches the agent as a command.
pub fn with_notes(words: &str, notes: &[Attachment]) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !words.is_empty() {
        lines.push(words.to_string());
    }
    lines.extend(notes.iter().filter_map(Attachment::note_line));
    lines.join("\n")
}

/// A sent message's words and the note lines [`with_notes`] put after
/// them: the trailing run of `› what · detail` lines. A line of your own
/// that starts with `›` in the middle of your words stays yours.
pub fn split_notes(text: &str) -> (&str, Vec<&str>) {
    let lines: Vec<&str> = text.lines().collect();
    let note = |l: &&&str| l.strip_prefix("› ").is_some_and(|r| r.contains(" · "));
    let tail = lines.iter().rev().take_while(note).count();
    let notes = lines[lines.len() - tail..].to_vec();
    // The words end where the first note line starts.
    let end = match notes.first() {
        Some(first) => first.as_ptr() as usize - text.as_ptr() as usize,
        None => text.len(),
    };
    (text[..end].trim_end_matches(['\n', '\r']), notes)
}

/// The command a message you sent runs (`/compact`, `/model opus`): the
/// one rule for "is this a command", for showing, searching, recognising
/// its answer and running dent's own. Only a message that is a command
/// line and nothing else is one; with files or notes it's a message.
pub fn sent_command<'a>(text: &'a str, files: &[String]) -> Option<&'a str> {
    let (words, notes) = split_notes(text);
    (files.is_empty() && notes.is_empty())
        .then(|| crate::data::command_name(words))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(start: usize, end: usize, paste: u32) -> Chip {
        Chip { start, end, paste }
    }

    #[test]
    fn notes_follow_the_words_and_read_back() {
        let atts = [
            Attachment::Comment {
                file: "a.rs".into(),
                line: 3,
                text: "why?".into(),
            },
            Attachment::File {
                path: "/tmp/x.png".into(),
            },
        ];
        let sent = with_notes("/review now", &atts);
        assert_eq!(sent, "/review now\n› a.rs:3 · why?");
        assert_eq!(split_notes(&sent), ("/review now", vec!["› a.rs:3 · why?"]));
        assert_eq!(sent_command("/review now", &[]), Some("review"));
        assert_eq!(sent_command(&sent, &[]), None);
        assert_eq!(sent_command("/review", &["/a.png".into()]), None);
        assert_eq!(sent_command("/Users/me/crash.log why", &[]), None);
        // A line of your own in the middle stays yours.
        let (w, n) = split_notes("first\n› see below · twice\nlast");
        assert_eq!(w, "first\n› see below · twice\nlast");
        assert!(n.is_empty());
        // A sent message comes back as its words and files, notes left.
        let d = Draft::of_sent(&sent, &["/tmp/x.png".into()]);
        assert_eq!(d.text, "/review now");
        assert_eq!(d.files(), ["/tmp/x.png"]);
    }

    #[test]
    fn chips_open_by_number_when_sent() {
        let pastes = BTreeMap::from([(1, "a\nb\n".to_string())]);
        let label = crate::compose::fold_label(1, &pastes[&1]);
        let text = format!("see {label} now");
        let d = Draft {
            chips: vec![chip(4, 4 + label.len(), 1)],
            text,
            ..Default::default()
        };
        assert_eq!(d.words_out(&pastes), "see a\nb\n now");
        // The same words typed by hand, with no chip, go as typed.
        let typed = Draft::words(d.text.clone());
        assert_eq!(typed.words_out(&pastes), d.text);
    }

    #[test]
    fn a_queued_message_shows_its_chips_apart() {
        let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
        let pastes = BTreeMap::from([(1, long)]);
        // At the end, at the start and in the middle alike.
        for (text, chip, words) in [
            ("look at [P]", chip(8, 11, 1), "look at"),
            ("[P]\nwhy?", chip(0, 3, 1), "why?"),
            ("a [P] b", chip(2, 5, 1), "a b"),
        ] {
            let d = Draft {
                text: text.into(),
                chips: vec![chip],
                ..Default::default()
            };
            assert_eq!(
                d.chips_apart(&pastes),
                (
                    words.to_string(),
                    vec!["Pasted text · 60 lines".to_string()]
                )
            );
        }
    }

    #[test]
    fn joining_keeps_chips_on_their_words() {
        let mut d = Draft {
            text: "mine [P1]  ".into(),
            chips: vec![chip(5, 9, 1)],
            ..Default::default()
        };
        let back = Draft {
            text: "  [P2] back".into(),
            chips: vec![chip(2, 6, 2)],
            atts: vec![Attachment::File { path: "/f".into() }],
            place: Some("next".into()),
        };
        d.join(back.clone(), false);
        assert_eq!(d.text, "mine [P1]\n\n[P2] back");
        assert_eq!(&d.text[d.chips[0].range()], "[P1]");
        assert_eq!(&d.text[d.chips[1].range()], "[P2]");
        assert_eq!(d.chips[1].paste, 2);
        assert_eq!(d.files(), ["/f"]);
        assert_eq!(d.place.as_deref(), Some("next"));
        // Before what's there.
        let mut d = Draft {
            text: "mine [P1]".into(),
            chips: vec![chip(5, 9, 1)],
            ..Default::default()
        };
        d.join(back, true);
        assert_eq!(d.text, "  [P2] back\n\nmine [P1]");
        for c in &d.chips {
            assert!(d.text[c.range()].starts_with("[P"));
        }
        // Into an empty draft: as it is.
        let mut e = Draft::default();
        e.join(Draft::words("x"), false);
        assert_eq!(e.text, "x");
    }

    #[test]
    fn saved_drafts_read_in_every_shape() {
        let old: Draft = serde_json::from_str(r#""half a thought""#).unwrap();
        assert_eq!(old, Draft::words("half a thought"));
        let stash: Draft = serde_json::from_str(r#"{"text":"t","files":["/a"]}"#).unwrap();
        assert_eq!(stash.files(), ["/a"]);
        let d = Draft {
            text: "x [P] y".into(),
            chips: vec![chip(2, 5, 7)],
            atts: vec![Attachment::File { path: "/b".into() }],
            place: None,
        };
        let back: Draft = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        // A chip past the text is dropped rather than trusted.
        let bad: Draft =
            serde_json::from_str(r#"{"text":"ab","chips":[{"start":1,"end":9,"paste":1}]}"#)
                .unwrap();
        assert!(bad.chips.is_empty());
    }
}
