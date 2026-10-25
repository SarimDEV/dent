//! Who commits are by on a machine, changed from Settings: typed as Git
//! writes it, `Name <email>`, and saved in that machine's Git settings. The
//! row holds the field until the machine says it's done, or why not.

use std::time::Duration;

use dent_protocol::{Machine, Request};
use gpui_kit::{Context, Window};

use crate::app::{Dent, Waiting, put};
use crate::ui::FocusField;

/// How long a machine has to save it before the row says it didn't.
const PATIENCE: Duration = Duration::from_secs(15);

pub struct GitUserEdit {
    pub machine: String,
    /// The request saving it, while it's out.
    pub sent: Option<u64>,
    /// One that went unanswered in time but may yet: its answer still
    /// closes the field.
    pub late: Option<u64>,
    pub error: Option<String>,
}

/// `Name <email>`, or `Name email` with the address last.
pub fn parse(typed: &str) -> Option<(String, String)> {
    let typed = typed.trim();
    let (name, email) = match typed.rsplit_once('<') {
        Some((name, rest)) => (name, rest.strip_suffix('>')?),
        None => typed.rsplit_once(char::is_whitespace)?,
    };
    let (name, email) = (name.trim(), email.trim());
    (!name.is_empty() && email.contains('@')).then(|| (name.to_string(), email.to_string()))
}

impl Dent {
    /// Open the field on `machine`'s row, with who it is now.
    pub fn edit_git_user(
        &mut self,
        machine: &str,
        now: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.git_user_edit = Some(GitUserEdit {
            machine: machine.to_string(),
            sent: None,
            late: None,
            error: None,
        });
        self.git_user_input.update(cx, |i, cx| {
            put(i, now.unwrap_or_default(), window, cx);
            i.focus_field(window, cx);
            i.select_all(window, cx);
        });
        cx.notify();
    }

    /// Send what's typed, if it reads as a name and an address.
    pub fn save_git_user(&mut self, cx: &mut Context<Self>) {
        let typed = self.git_user_input.read(cx).value().to_string();
        let Some(edit) = self.git_user_edit.as_mut().filter(|e| e.sent.is_none()) else {
            return;
        };
        let Some((name, email)) = parse(&typed) else {
            edit.error = Some("Type a name and an email: Ada Lovelace <ada@example.com>".into());
            cx.notify();
            return;
        };
        let machine = edit.machine.clone();
        let n = self
            .live
            .send_to(&machine, Request::SetGitUser { name, email });
        edit.sent = Some(n);
        edit.error = None;
        self.waiting_insert(n, Waiting::GitUser(machine.clone(), n));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PATIENCE).await;
            let _ = this.update(cx, |this, cx| {
                if !this.is_waiting(n) {
                    return;
                }
                let name = this.machine_name(&machine);
                // Never sent (its link is down): taken back, so it can't
                // land later behind your back. Sent: it may yet, and the
                // row takes the answer whenever it comes.
                let why = if !machine.is_empty() && this.live.cancel(&machine, n) {
                    this.waiting_remove(n);
                    format!("{name} isn't connected, so nothing was saved")
                } else {
                    format!("{name} hasn't answered; it may still save it")
                };
                this.git_user_came(&machine, n, Err(why), cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// The machine answered request `n`: saved, so the field closes, or
    /// not, and the row says why.
    pub fn git_user_came(
        &mut self,
        machine: &str,
        n: u64,
        answer: Result<Machine, String>,
        cx: &mut Context<Self>,
    ) {
        let ours = self
            .git_user_edit
            .as_ref()
            // A late one counts only while nothing newer is out.
            .is_some_and(|e| e.sent == Some(n) || (e.sent.is_none() && e.late == Some(n)));
        if let Ok(m) = &answer {
            if machine.is_empty() {
                self.machine_check = Some(m.clone());
            }
            self.setup_came(machine.to_string(), m.clone(), cx);
        }
        if !ours {
            return;
        }
        match answer {
            Ok(_) => self.git_user_edit = None,
            Err(why) => {
                let awaited = self.is_waiting(n);
                if let Some(e) = self.git_user_edit.as_mut() {
                    // Still awaited (it may yet save): its answer counts.
                    e.late = (e.sent == Some(n) && awaited).then_some(n);
                    e.sent = None;
                    e.error = Some(why);
                }
            }
        }
        cx.notify();
    }

    /// Esc: the field closes, unsaved. Whether one was open.
    pub fn cancel_git_user(&mut self, cx: &mut Context<Self>) -> bool {
        // Only where it's drawn: anywhere else, Esc does what it does there.
        if !matches!(
            self.pane,
            Some(crate::settings::Pane::Settings | crate::settings::Pane::Welcome)
        ) {
            self.git_user_edit = None;
            return false;
        }
        let open = self.git_user_edit.take().is_some();
        if open {
            cx.notify();
        }
        open
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn reads_a_name_and_an_address() {
        let ada = Some(("Ada Lovelace".to_string(), "ada@example.com".to_string()));
        assert_eq!(parse("Ada Lovelace <ada@example.com>"), ada);
        assert_eq!(parse("  Ada Lovelace   <ada@example.com>  "), ada);
        assert_eq!(parse("Ada Lovelace ada@example.com"), ada);
        assert_eq!(parse("Ada Lovelace <ada@example.com"), None);
        assert_eq!(parse("ada@example.com"), None);
        assert_eq!(parse("<ada@example.com>"), None);
        assert_eq!(parse("Ada Lovelace"), None);
        assert_eq!(parse(""), None);
    }
}
