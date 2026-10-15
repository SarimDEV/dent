//! Your own terminal, in the viewer's Terminal tab: your login shell (zsh or
//! bash on Mac and Linux, PowerShell on Windows) in the session's folder,
//! the way an editor's terminal panel runs it. It's yours alone: the agent
//! never sees it or types into it (0042).
//!
//! Alacritty's terminal does the hard parts (the pty, escape codes, the
//! grid, scrollback, selection); this draws its grid in the code face, one
//! styled line per row, and turns keys into the bytes a shell expects.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Term, TermMode, viewport_to_point};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::Press;

use crate::app::Dent;
use crate::ds::{CODE, Typed, UI, size, space};
use crate::theme::{self, Theme};
use crate::ui::{button, probe};

/// Space around the grid, inside the pane.
const PAD: f32 = space::S4;

/// Tells the app what a shell did: drew something, set its title, ended.
#[derive(Clone)]
pub struct Listener {
    id: String,
    tx: UnboundedSender<(String, Event)>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.tx.unbounded_send((self.id.clone(), event));
    }
}

struct Size {
    cols: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Shell {
    pub term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    /// Columns and rows the pty was last told, and the cell they assume.
    size: Rc<Cell<(u16, u16)>>,
    /// Where the grid was drawn last frame, for the mouse.
    origin: Rc<Cell<Point<Pixels>>>,
    pub exited: bool,
}

impl Shell {
    /// Starts your shell at the size it will be drawn, so it doesn't redraw
    /// its prompt for a resize straight away.
    pub fn spawn(
        id: &str,
        cwd: &str,
        (cols, lines): (u16, u16),
        tx: UnboundedSender<(String, Event)>,
    ) -> std::io::Result<Self> {
        let window = WindowSize {
            num_lines: lines,
            num_cols: cols,
            cell_width: 8,
            cell_height: 16,
        };
        let dir = Path::new(cwd);
        let opts = tty::Options {
            shell: None,
            working_directory: Some(if dir.is_dir() {
                dir.to_path_buf()
            } else {
                std::env::var_os("HOME").map(Into::into).unwrap_or_default()
            }),
            drain_on_exit: false,
            env: HashMap::from([
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("TERM_PROGRAM".into(), "dent".into()),
            ]),
            #[cfg(target_os = "windows")]
            escape_args: false,
        };
        let pty = tty::new(&opts, window, 0)?;
        let listener = Listener {
            id: id.to_string(),
            tx,
        };
        let term = Term::new(
            Config::default(),
            &Size {
                cols: cols.into(),
                lines: lines.into(),
            },
            listener.clone(),
        );
        let term = Arc::new(FairMutex::new(term));
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let sender = event_loop.channel();
        event_loop.spawn();
        Ok(Self {
            term,
            sender,
            size: Rc::new(Cell::new((cols, lines))),
            origin: Rc::default(),
            exited: false,
        })
    }

    /// Bytes for the shell, as if typed.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if std::env::var_os("DENT_DEBUG_SHELL").is_some() {
            eprintln!("shell: write {:?}", String::from_utf8_lossy(&bytes));
        }
        if !bytes.is_empty() {
            let _ = self.sender.send(Msg::Input(bytes));
        }
    }

    /// What you typed goes in at the bottom, wherever you'd scrolled to.
    pub fn type_bytes(&self, bytes: Vec<u8>) {
        {
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
    }

    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text
        };
        self.type_bytes(bytes.into_bytes());
    }

    /// ⌘F: every place `needle` is in the scrollback and on screen, top
    /// to bottom, ignoring case.
    pub fn find_all(&self, needle: &str) -> Vec<Match> {
        if needle.is_empty() {
            return Vec::new();
        }
        let literal: String = needle
            .chars()
            .flat_map(|c| {
                let special = "\\.+*?()|[]{}^$#&-~".contains(c);
                special.then_some('\\').into_iter().chain([c])
            })
            .collect();
        let Ok(mut regex) = RegexSearch::new(&format!("(?i){literal}")) else {
            return Vec::new();
        };
        let term = self.term.lock();
        let start = GridPoint::new(term.topmost_line(), Column(0));
        let end = GridPoint::new(term.bottommost_line(), term.last_column());
        RegexIter::new(start, end, Direction::Right, &term, &mut regex)
            .take(10_000)
            .collect()
    }

    /// What ⌘F's matches were counted against: the scrollback's length
    /// and where the cursor is, which move as anything is printed.
    pub fn find_sig(&self) -> (usize, i32, usize) {
        let term = self.term.lock();
        let cursor = term.grid().cursor.point;
        (term.grid().history_size(), cursor.line.0, cursor.column.0)
    }

    /// Scroll so a match is on screen.
    pub fn show_match(&self, m: &Match) {
        self.term.lock().scroll_to_point(*m.start());
    }

    /// ⌘A: everything in the scrollback and on screen, as in Terminal.
    pub fn select_all(&self) {
        let mut term = self.term.lock();
        let (top, bottom) = (term.topmost_line(), term.bottommost_line());
        let last = term.last_column();
        let mut sel = Selection::new(
            SelectionType::Simple,
            GridPoint::new(top, Column(0)),
            Side::Left,
        );
        sel.update(GridPoint::new(bottom, last), Side::Right);
        term.selection = Some(sel);
    }

    pub fn copy(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|s| !s.is_empty())
    }

    pub fn shutdown(&self) {
        let _ = self.sender.send(Msg::Shutdown);
    }

    /// ⌘K: the screen and everything scrolled past go, as in Terminal;
    /// the shell redraws its prompt.
    pub fn clear(&self) {
        let alt = {
            let mut term = self.term.lock();
            term.grid_mut().clear_history();
            term.mode().contains(TermMode::ALT_SCREEN)
        };
        if !alt {
            self.type_bytes(b"\x0c".to_vec());
        }
    }

    /// The screen as text, for checks.
    pub fn text(&self) -> String {
        let term = self.term.lock();
        let mut out = String::new();
        let mut row = None;
        for cell in term.grid().display_iter() {
            if row.is_some_and(|r| r != cell.point.line) {
                out.push('\n');
            }
            row = Some(cell.point.line);
            if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                out.push(cell.c);
            }
        }
        out
    }

    fn window_size(&self) -> WindowSize {
        let (cols, lines) = self.size.get();
        WindowSize {
            num_lines: lines,
            num_cols: cols,
            cell_width: 8,
            cell_height: 16,
        }
    }
}

fn cell_width(window: &mut Window) -> f32 {
    let id = window.text_system().resolve_font(&font(theme::mono()));
    window
        .text_system()
        .advance(id, px(CODE.size), 'm')
        .map(|s| s.width.as_f32())
        .unwrap_or(CODE.size * 0.6)
}

/// Columns and rows that fit a pane.
fn grid_size(pane: gpui_kit::Size<Pixels>, cell_w: f32) -> (u16, u16) {
    let cols = (((pane.width.as_f32() - 2. * PAD) / cell_w).floor() as u16).max(2);
    let rows = (((pane.height.as_f32() - 2. * PAD) / CODE.line).floor() as u16).max(1);
    (cols, rows)
}

/// The bytes a key sends, the way Terminal.app and xterm send them. ⌘ keys
/// never get here: they stay the app's.
pub fn key_bytes(k: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    let app = mode.contains(TermMode::APP_CURSOR);
    // xterm's modifier number: 1 + shift + 2·alt + 4·ctrl.
    let code = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let arrow = |c: char| -> Vec<u8> {
        if code > 1 {
            format!("\x1b[1;{code}{c}").into_bytes()
        } else if app {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if code > 1 {
            format!("\x1b[{n};{code}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let bytes = match k.key.as_str() {
        "enter" => b"\r".to_vec(),
        "backspace" if m.alt => b"\x1b\x7f".to_vec(),
        "backspace" if m.control => b"\x08".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        // Option+arrow moves by word, as in Terminal.app.
        "left" if m.alt && !m.control && !m.shift => b"\x1bb".to_vec(),
        "right" if m.alt && !m.control && !m.shift => b"\x1bf".to_vec(),
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => arrow('H'),
        "end" => arrow('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if m.control => vec![0],
        key if m.control && key.len() == 1 => {
            let c = key.as_bytes()[0].to_ascii_lowercase();
            let b = match c {
                b'a'..=b'z' => c & 0x1f,
                b'[' => 0x1b,
                b'\\' => 0x1c,
                b']' => 0x1d,
                b'6' => 0x1e,
                b'-' | b'/' => 0x1f,
                b'2' | b'@' => 0,
                _ => return None,
            };
            if m.alt { vec![0x1b, b] } else { vec![b] }
        }
        _ => k.key_char.as_ref()?.as_bytes().to_vec(),
    };
    Some(bytes)
}

/// The 16 colours, tuned to the page: readable on Paper and on Carbon.
fn ansi(t: &Theme, i: usize) -> Hsla {
    const PAPER: [u32; 16] = [
        0x171614, 0xB8402F, 0x3C7A34, 0x8F6A00, 0x2F5EA6, 0x94408A, 0x1F7682, 0x8C877D, 0x6E6A62,
        0xD24A35, 0x4A9440, 0xA97E00, 0x3A72C4, 0xAD4DA1, 0x268C9A, 0x171614,
    ];
    const CARBON: [u32; 16] = [
        0x2E2C28, 0xE5715E, 0x7DC46E, 0xE8B04B, 0x6FA0E8, 0xD07FC4, 0x5EC0CC, 0xC9C4BA, 0x706B63,
        0xF08A78, 0x96D688, 0xF2C46A, 0x8DB6F0, 0xDE9AD3, 0x7ED2DC, 0xECE8DF,
    ];
    rgb(if t.dark { CARBON[i] } else { PAPER[i] }).into()
}

fn rgb_of(c: Rgb) -> Hsla {
    rgb((u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)).into()
}

/// The 256-colour palette past the first 16: a 6×6×6 cube, then greys.
fn indexed(t: &Theme, i: u8) -> Hsla {
    match i {
        0..=15 => ansi(t, i as usize),
        16..=231 => {
            let i = i - 16;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            rgb_of(Rgb {
                r: step(i / 36),
                g: step((i / 6) % 6),
                b: step(i % 6),
            })
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            rgb_of(Rgb { r: v, g: v, b: v })
        }
    }
}

fn color(c: Color, term: &Term<Listener>, t: &Theme, fg: bool) -> Option<Hsla> {
    let set = |i: usize| term.colors()[i].map(rgb_of);
    Some(match c {
        Color::Spec(rgb) => rgb_of(rgb),
        Color::Indexed(i) => set(i as usize).unwrap_or_else(|| indexed(t, i)),
        Color::Named(n) => match n {
            NamedColor::Foreground | NamedColor::BrightForeground => t.ink,
            NamedColor::DimForeground => t.quiet,
            // The pane shows through the default background.
            NamedColor::Background => return if fg { Some(t.panel) } else { None },
            NamedColor::Cursor => t.ink,
            n if (n as usize) < 16 => set(n as usize).unwrap_or_else(|| ansi(t, n as usize)),
            n => {
                // Dim colours: the normal one, quieter.
                let base = (n as usize).saturating_sub(NamedColor::DimBlack as usize) % 8;
                ansi(t, base).opacity(0.7)
            }
        },
    })
}

impl Dent {
    /// Your terminal for this session, started the first time you open it.
    pub fn open_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.current_rc() else {
            return;
        };
        let restart = self.shells.get(&s.id).is_some_and(|sh| sh.exited);
        if restart || !self.shells.contains_key(&s.id) {
            // The pane's size: as last drawn, or worked out from the layout.
            let pane = self.shell_pane.get().unwrap_or_else(|| {
                let h = window.viewport_size().height.as_f32() - size::HEADER - 1. - size::STATUS;
                size(px(self.panel_w - 1.), px(h))
            });
            let grid = grid_size(pane, cell_width(window));
            match Shell::spawn(&s.id, &s.cwd, grid, self.shell_tx.clone()) {
                Ok(sh) => {
                    self.shells.insert(s.id.clone(), sh);
                }
                Err(e) => {
                    self.toast(
                        crate::app::ToastAction::Info,
                        format!("Couldn't start your shell: {e}"),
                    );
                    return;
                }
            }
        }
        self.show_tab(crate::app::Tab::Terminal, cx);
        window.focus(&self.term_focus, cx);
    }

    /// What a shell did, from its own thread.
    pub fn on_shell(&mut self, id: String, event: Event, cx: &mut Context<Self>) {
        let t = self.t;
        let Some(sh) = self.shells.get_mut(&id) else {
            return;
        };
        match event {
            Event::PtyWrite(text) => sh.write(text.into_bytes()),
            Event::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            Event::ClipboardLoad(_, reply) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|c| c.text())
                    .unwrap_or_default();
                sh.write(reply(&text).into_bytes());
            }
            Event::ColorRequest(i, reply) => {
                let c = if i < 256 {
                    indexed(&t, i as u8)
                } else if i == NamedColor::Background as usize {
                    t.panel
                } else {
                    t.ink
                };
                let c = c.to_rgb();
                let px = |v: f32| (v * 255.).round() as u8;
                sh.write(
                    reply(Rgb {
                        r: px(c.r),
                        g: px(c.g),
                        b: px(c.b),
                    })
                    .into_bytes(),
                );
            }
            Event::TextAreaSizeRequest(reply) => {
                let size = sh.window_size();
                sh.write(reply(size).into_bytes());
            }
            Event::Exit | Event::ChildExit(_) => sh.exited = true,
            _ => {}
        }
        cx.notify();
    }

    /// Keys for your terminal while it has focus: everything but ⌘, which
    /// stays the app's (⌘C and ⌘V copy and paste here). Returns whether the
    /// key was used.
    pub fn shell_key(
        &mut self,
        k: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.term_focus.is_focused(window) {
            return false;
        }
        let Some(id) = self.selected.clone() else {
            return false;
        };
        let Some(sh) = self.shells.get(&id) else {
            return false;
        };
        if sh.exited {
            if k.key == "enter" && !k.modifiers.modified() {
                self.open_shell(window, cx);
                return true;
            }
            return false;
        }
        if k.modifiers.platform {
            match k.key.as_str() {
                // With nothing selected, ⌘C leaves the clipboard be.
                "c" => {
                    if let Some(text) = sh.copy() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    true
                }
                "a" => {
                    sh.select_all();
                    cx.notify();
                    true
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        sh.paste(&text);
                    }
                    true
                }
                // ⌘⌫ clears the line, as in Terminal.app.
                "backspace" => {
                    sh.type_bytes(vec![0x15]);
                    true
                }
                _ => false,
            }
        } else {
            let mode = *sh.term.lock().mode();
            match key_bytes(k, mode) {
                Some(bytes) => {
                    sh.type_bytes(bytes);
                    cx.notify();
                    true
                }
                None => false,
            }
        }
    }

    pub fn shell_view(
        &mut self,
        s: &crate::data::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let Some(sh) = self.shells.get(&s.id) else {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(space::S3))
                .ty(UI)
                .text_color(t.quiet)
                .child("Your own shell, in this session's folder. The agent never sees it.")
                .child(
                    button("shell-start", "Open terminal", true, &t)
                        .on_press(cx.listener(|this, _, window, cx| this.open_shell(window, cx))),
                )
                .into_any_element();
        };
        let focused = self.term_focus.is_focused(window);
        let fsize = CODE.size;
        let line_h = CODE.line;
        let mono = font(theme::mono());
        let cell_w = cell_width(window);

        let term = sh.term.lock();
        // What ⌘F found on screen, and which is current.
        let shown = term.grid().display_offset() as i32;
        let (top, bottom) = (-shown, term.screen_lines() as i32 - 1 - shown);
        let found: Vec<(&Match, bool)> = self
            .find
            .as_ref()
            .filter(|f| f.session == s.id && f.scope == crate::find::Scope::Terminal)
            .map(|f| {
                f.cells
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.end().line.0 >= top && m.start().line.0 <= bottom)
                    .map(|(i, m)| (m, f.hits.get(f.at).is_some_and(|h| h.0 == i)))
                    .collect()
            })
            .unwrap_or_default();
        let content = term.renderable_content();
        let offset = content.display_offset;
        let sel = content.selection;
        let mode = content.mode;
        let cursor = content.cursor;
        let lines = term.screen_lines();
        let mut rows: Vec<(String, Vec<TextRun>)> = vec![(String::new(), Vec::new()); lines];
        for cell in content.display_iter {
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let row = (cell.point.line.0 + offset as i32).max(0) as usize;
            let Some((text, runs)) = rows.get_mut(row) else {
                continue;
            };
            let mut fg = color(cell.fg, &term, &t, true).unwrap_or(t.ink);
            let mut bg = color(cell.bg, &term, &t, false);
            if cell.flags.contains(Flags::INVERSE) {
                let was = fg;
                fg = bg.unwrap_or(t.panel);
                bg = Some(was);
            }
            if cell.flags.contains(Flags::DIM) {
                fg = fg.opacity(0.66);
            }
            let at_cursor = focused
                && mode.contains(TermMode::SHOW_CURSOR)
                && offset == 0
                && cursor.shape == CursorShape::Block
                && cell.point == cursor.point;
            if at_cursor {
                bg = Some(t.ink);
                fg = t.panel;
            } else if sel.is_some_and(|r| r.contains(cell.point)) {
                bg = Some(t.selection());
            } else if let Some((_, now)) = found.iter().find(|(m, _)| m.contains(&cell.point)) {
                let (tint, strong) = crate::find::tints(&t);
                bg = Some(if *now { strong } else { tint });
            }
            let c = if cell.flags.contains(Flags::HIDDEN) || cell.c == '\0' {
                ' '
            } else {
                cell.c
            };
            let mut f = mono.clone();
            if cell.flags.contains(Flags::BOLD) {
                f.weight = FontWeight::BOLD;
            }
            if cell.flags.contains(Flags::ITALIC) {
                f.style = FontStyle::Italic;
            }
            let underline = cell
                .flags
                .intersects(Flags::ALL_UNDERLINES)
                .then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(fg),
                    wavy: cell.flags.contains(Flags::UNDERCURL),
                });
            let strike = cell
                .flags
                .contains(Flags::STRIKEOUT)
                .then(|| StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(fg),
                });
            let len = c.len_utf8();
            text.push(c);
            match runs.last_mut() {
                Some(r)
                    if r.color == fg
                        && r.background_color == bg
                        && r.font == f
                        && r.underline == underline
                        && r.strikethrough == strike =>
                {
                    r.len += len;
                }
                _ => runs.push(TextRun {
                    len,
                    font: f,
                    color: fg,
                    background_color: bg,
                    underline,
                    strikethrough: strike,
                }),
            }
        }
        // Not focused, or a bar or underline cursor: drawn over the grid.
        let row = cursor.point.line.0 + offset as i32;
        let drawn_cursor = (mode.contains(TermMode::SHOW_CURSOR)
            && cursor.shape != CursorShape::Hidden
            && row >= 0
            && (row as usize) < lines
            && !(focused && cursor.shape == CursorShape::Block))
            .then(|| {
                let x = PAD + cursor.point.column.0 as f32 * cell_w;
                let y = PAD + row as f32 * line_h;
                let d = div().absolute().left(px(x)).top(px(y));
                match cursor.shape {
                    CursorShape::Beam => d.w(px(1.5)).h(px(line_h)).bg(t.ink),
                    CursorShape::Underline => {
                        d.top(px(y + line_h - 2.)).w(px(cell_w)).h(px(2.)).bg(t.ink)
                    }
                    _ => d
                        .w(px(cell_w))
                        .h(px(line_h))
                        .border_1()
                        .border_color(t.quiet),
                }
            });
        drop(term);

        let exited = sh.exited;
        let (term_a, sender, grid, origin, pane) = (
            sh.term.clone(),
            sh.sender.clone(),
            sh.size.clone(),
            sh.origin.clone(),
            self.shell_pane.clone(),
        );
        let (term_b, origin_b) = (sh.term.clone(), sh.origin.clone());
        let (term_c, origin_c) = (sh.term.clone(), sh.origin.clone());
        let term_d = sh.term.clone();
        let point_at = move |p: Point<Pixels>, o: Point<Pixels>, term: &Term<Listener>| {
            let x = ((p.x - o.x).as_f32() - PAD).max(0.);
            let y = ((p.y - o.y).as_f32() - PAD).max(0.);
            let col = ((x / cell_w) as usize).min(term.columns().saturating_sub(1));
            let row = ((y / line_h) as usize).min(term.screen_lines().saturating_sub(1));
            let side = if (x / cell_w).fract() < 0.5 {
                Side::Left
            } else {
                Side::Right
            };
            (
                viewport_to_point(
                    term.grid().display_offset(),
                    GridPoint::new(row, Column(col)),
                ),
                side,
            )
        };
        div()
            .id("shell")
            .key_context("Terminal")
            .track_focus(&self.term_focus)
            .on_action(cx.listener(|this, _: &crate::app::ClearTerminal, _, cx| {
                if let Some(sh) = this.current().and_then(|s| this.shells.get(&s.id)) {
                    sh.clear();
                }
                cx.notify();
            }))
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(t.panel)
            .cursor(CursorStyle::IBeam)
            .child(
                canvas(
                    move |bounds, window, _| {
                        origin.set(bounds.origin);
                        pane.set(Some(bounds.size));
                        let (cols, rows) = grid_size(bounds.size, cell_w);
                        if grid.get() != (cols, rows) {
                            if std::env::var_os("DENT_DEBUG_SHELL").is_some() {
                                eprintln!("shell: resize {cols}x{rows}");
                            }
                            grid.set((cols, rows));
                            term_a.lock().resize(Size {
                                cols: cols.into(),
                                lines: rows.into(),
                            });
                            let _ = sender.send(Msg::Resize(WindowSize {
                                num_lines: rows,
                                num_cols: cols,
                                cell_width: cell_w as u16,
                                cell_height: line_h as u16,
                            }));
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(PAD))
                    .top(px(PAD))
                    .flex()
                    .flex_col()
                    .children(rows.into_iter().map(|(text, runs)| {
                        div()
                            .h(px(line_h))
                            .whitespace_nowrap()
                            .text_size(px(fsize))
                            .line_height(px(line_h))
                            .child(StyledText::new(text).with_runs(runs))
                    })),
            )
            .children(drawn_cursor)
            .when(exited, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .p(px(space::S3))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .bg(t.panel)
                        .border_t_1()
                        .border_color(t.hairline)
                        .ty(UI)
                        .text_color(t.quiet)
                        .child(
                            div()
                                .flex_1()
                                .child("The shell ended. Return starts a new one."),
                        )
                        .child(button("shell-restart", "New shell", false, &t).on_press(
                            cx.listener(|this, _, window, cx| this.open_shell(window, cx)),
                        )),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    window.focus(&this.term_focus, cx);
                    let mut term = term_b.lock();
                    let (point, side) = point_at(e.position, origin_b.get(), &term);
                    // ⌘-click opens a web address or a file, as Terminal does.
                    if e.modifiers.platform {
                        let line = row_text(&term, point.line);
                        let base = this.current().map(|s| std::path::PathBuf::from(&s.cwd));
                        drop(term);
                        if let Some(url) = link_at_column(&line, point.column.0, base.as_deref()) {
                            crate::select::open_link(&url, window, cx);
                        }
                        return;
                    }
                    let ty = match e.click_count {
                        2 => SelectionType::Semantic,
                        3 => SelectionType::Lines,
                        _ => SelectionType::Simple,
                    };
                    term.selection = Some(Selection::new(ty, point, side));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |_, e: &MouseMoveEvent, _, cx| {
                if e.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let mut term = term_c.lock();
                let (point, side) = point_at(e.position, origin_c.get(), &term);
                if let Some(sel) = term.selection.as_mut() {
                    sel.update(point, side);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |_, _, _, cx| {
                    // A click without a drag selects nothing.
                    let mut term = term_d.lock();
                    if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                        term.selection = None;
                        cx.notify();
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, _, cx| {
                let Some(id) = this.selected.clone() else {
                    return;
                };
                let Some(sh) = this.shells.get(&id) else {
                    return;
                };
                let dy = e.delta.pixel_delta(px(line_h)).y.as_f32();
                let n = (dy / line_h).round() as i32;
                if n == 0 {
                    return;
                }
                let mode = *sh.term.lock().mode();
                if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL)
                {
                    // Full-screen programs (less, vim) scroll with arrow keys.
                    let key = if n > 0 { "\x1b[A" } else { "\x1b[B" };
                    sh.write(key.repeat(n.unsigned_abs() as usize).into_bytes());
                } else {
                    sh.term.lock().scroll_display(Scroll::Delta(n));
                }
                cx.notify();
            }))
            .child(probe(&self.probes, "shell"))
            // Last, so it covers the terminal's text; inset so its border
            // clears the pane's edges.
            .children(
                (self.drop_target(cx) == Some(crate::app::DropTarget::Shell)).then(|| {
                    crate::ui::drop_hint(
                        "Drop to type the paths here",
                        &self.probes,
                        "drop-shell",
                        space::S2,
                        &t,
                    )
                }),
            )
            .into_any_element()
    }
}

/// A path as one shell word: bare when that's safe, else in single quotes.
pub fn shell_quote(p: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c);
    if !p.is_empty() && p.chars().all(safe) {
        p.to_string()
    } else {
        format!("'{}'", p.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::{key_bytes, shell_quote};
    use alacritty_terminal::term::TermMode;
    use gpui_kit::Keystroke;

    fn k(s: &str) -> Vec<u8> {
        key_bytes(&Keystroke::parse(s).unwrap(), TermMode::empty()).unwrap_or_default()
    }

    #[test]
    fn keys_send_what_a_shell_expects() {
        assert_eq!(k("enter"), b"\r");
        assert_eq!(k("ctrl-c"), [3]);
        assert_eq!(k("ctrl-d"), [4]);
        assert_eq!(k("escape"), [0x1b]);
        assert_eq!(k("up"), b"\x1b[A");
        assert_eq!(k("shift-up"), b"\x1b[1;2A");
        assert_eq!(k("alt-left"), b"\x1bb");
        assert_eq!(k("backspace"), [0x7f]);
        assert_eq!(k("pageup"), b"\x1b[5~");
        let app = key_bytes(&Keystroke::parse("up").unwrap(), TermMode::APP_CURSOR).unwrap();
        assert_eq!(app, b"\x1bOA");
    }

    #[test]
    fn dropped_paths_are_one_shell_word() {
        assert_eq!(shell_quote("/tmp/a.txt"), "/tmp/a.txt");
        assert_eq!(shell_quote("/tmp/my file.txt"), "'/tmp/my file.txt'");
        assert_eq!(shell_quote("/tmp/it's"), "'/tmp/it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}

/// One line of the terminal as text, a character per column.
fn row_text(term: &Term<Listener>, line: alacritty_terminal::index::Line) -> String {
    let row = &term.grid()[line];
    (0..term.columns())
        .map(|c| row[Column(c)].c)
        .collect::<String>()
}

/// The link at column `col` of `line`: a web address, or a file named
/// there (found from `base`).
fn link_at_column(line: &str, col: usize, base: Option<&std::path::Path>) -> Option<String> {
    let at = line.char_indices().nth(col).map(|(i, _)| i)?;
    crate::markdown::plain_links(line, base)
        .into_iter()
        .find(|(r, _)| r.contains(&at))
        .map(|(_, url)| url)
}

#[cfg(test)]
mod link_tests {
    use super::link_at_column;

    #[test]
    fn a_click_finds_the_link_under_it() {
        let line = "see https://x.dev/a and src/app.rs:4 here";
        assert_eq!(
            link_at_column(line, 6, None).as_deref(),
            Some("https://x.dev/a")
        );
        let base = std::path::Path::new("/p");
        assert_eq!(
            link_at_column(line, 26, Some(base)).as_deref(),
            Some("/p/src/app.rs:4")
        );
        assert_eq!(link_at_column(line, 1, None), None);
    }
}
