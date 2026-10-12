//! Turn status derived purely from stream events, per docs/daemon.md
//! "Turn status and notifications".

use crate::wire::Inbound;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No turn has run yet, or the last one ended with nothing left running.
    Idle,
    /// A turn is in progress (including waiting on a tool it started).
    Working,
    /// Waiting on an approval from us.
    Blocked,
    /// The turn ended but background work it started is still running.
    /// Not worth a notification: the agent starts a new turn when it finishes.
    Background,
    Errored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notify {
    /// Blocked on an approval: time-sensitive.
    Blocked,
    /// Turn truly over, waiting for the next prompt.
    Finished,
    Error,
}

#[derive(Debug, Default)]
pub struct Tracker {
    pending_approvals: usize,
    background: usize,
    in_turn: bool,
    errored: bool,
}

impl Tracker {
    pub fn status(&self) -> Status {
        if self.pending_approvals > 0 {
            Status::Blocked
        } else if self.in_turn {
            Status::Working
        } else if self.background > 0 {
            Status::Background
        } else if self.errored {
            Status::Errored
        } else {
            Status::Idle
        }
    }

    /// We sent a user message: a turn starts (or is queued behind the current one).
    pub fn on_user_sent(&mut self) {
        self.in_turn = true;
        self.errored = false;
    }

    /// We answered an approval.
    pub fn on_approval_answered(&mut self) {
        self.pending_approvals = self.pending_approvals.saturating_sub(1);
    }

    /// Feed one inbound event; returns a notification if one is due.
    pub fn on_event(&mut self, ev: &Inbound) -> Option<Notify> {
        match ev {
            // A turn starting on its own (after background work) also begins with init.
            Inbound::Init { .. } => {
                self.in_turn = true;
                None
            }
            Inbound::CanUseTool { .. } => {
                self.pending_approvals += 1;
                Some(Notify::Blocked)
            }
            Inbound::BackgroundTasks(tasks) => {
                self.background = tasks.len();
                None
            }
            Inbound::Result {
                is_error,
                terminal_reason,
                ..
            } => {
                self.in_turn = false;
                self.pending_approvals = 0;
                let interrupted = terminal_reason.as_deref() == Some("aborted_streaming");
                if *is_error && !interrupted {
                    self.errored = true;
                    Some(Notify::Error)
                } else if interrupted || self.background > 0 {
                    None
                } else {
                    Some(Notify::Finished)
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::parse;

    fn feed(t: &mut Tracker, line: &str) -> Option<Notify> {
        t.on_event(&parse(line).expect("fixture line parses"))
    }

    const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"m"}"#;
    const OK: &str =
        r#"{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed"}"#;

    #[test]
    fn background_work_defers_finished_until_the_follow_up_turn() {
        let mut t = Tracker::default();
        t.on_user_sent();
        feed(&mut t, INIT);
        feed(
            &mut t,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","description":"sleep"}]}"#,
        );
        assert_eq!(feed(&mut t, OK), None);
        assert_eq!(t.status(), Status::Background);

        feed(
            &mut t,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#,
        );
        feed(&mut t, INIT); // agent starts its own turn
        assert_eq!(t.status(), Status::Working);
        assert_eq!(feed(&mut t, OK), Some(Notify::Finished));
        assert_eq!(t.status(), Status::Idle);
    }

    #[test]
    fn approval_blocks_and_answer_unblocks() {
        let mut t = Tracker::default();
        t.on_user_sent();
        let n = feed(
            &mut t,
            r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#,
        );
        assert_eq!(n, Some(Notify::Blocked));
        assert_eq!(t.status(), Status::Blocked);
        t.on_approval_answered();
        assert_eq!(t.status(), Status::Working);
    }

    #[test]
    fn interrupt_is_not_an_error_or_a_notification() {
        let mut t = Tracker::default();
        t.on_user_sent();
        let n = feed(
            &mut t,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_streaming"}"#,
        );
        assert_eq!(n, None);
        assert_eq!(t.status(), Status::Idle);
    }
}
