//! Where a session's move stands on the machine it leaves, and what that
//! lets it do ([decision 0064]).
//!
//! `none → pending → departing → departed`, or back to none (cancelled,
//! failed, kept here). Every step is a compare-and-set on the move's id,
//! kept in the store (`move/<session>`) so it outlives a restart, and
//! said as a [`Body::Move`](dent_protocol::Body::Move) event for clients.
//!
//! [decision 0064]: ../../../../docs/decisions/0064-moving-sessions.md

use dent_protocol::MoveState;

/// A session's move, as kept.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Move {
    pub id: String,
    /// The machine it goes to.
    pub to: String,
    pub state: MoveState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_session: Option<String>,
}

/// What can happen to a move.
#[derive(Debug)]
pub enum Step {
    /// Asked to move: `busy` while a turn (or work it left running) goes on.
    Start { id: String, to: String, busy: bool },
    /// The turn it waited for ended.
    Quiet,
    /// Something may now have been sent to the other machine.
    Sending { id: String },
    /// The other machine has it, as this session there.
    Arrived { id: String, to_session: String },
    /// The other machine said it took nothing.
    NotAccepted { id: String, why: String },
    /// Called off (only before anything was sent).
    Cancel { id: String },
    /// Kept here, whatever was sent.
    ContinueHere { id: String },
    /// You sent a message while it waited: the move gives way.
    Message,
}

/// The move after `step`, or why `step` can't happen now. `None` is no
/// move. Asking for what's already so (the same id, the same step) is
/// answered as done, so a client retrying after a lost answer is safe.
pub fn next(now: Option<&Move>, step: Step) -> Result<Option<Move>, String> {
    use MoveState as S;
    let live = now.filter(|m| matches!(m.state, S::Pending | S::Departing { .. } | S::Departed));
    let same = |id: &str| live.is_some_and(|m| m.id == id);
    let with = |state: S| live.map(|m| Move { state, ..m.clone() });
    match step {
        Step::Start { id, to, busy } => match live {
            Some(m) if m.id == id => Ok(Some(m.clone())),
            Some(m) if m.state == S::Departed => Err(format!("it moved to {} already", m.to)),
            Some(m) => Err(format!("it's moving to {} already", m.to)),
            None => Ok(Some(Move {
                id,
                to,
                state: if busy { S::Pending } else { S::Departing { sent: false } },
                to_session: None,
            })),
        },
        Step::Quiet => Ok(match live {
            Some(m) if m.state == S::Pending => with(S::Departing { sent: false }),
            _ => now.cloned(),
        }),
        Step::Sending { id } => match live {
            Some(m) if m.id == id && matches!(m.state, S::Departing { .. }) => {
                Ok(with(S::Departing { sent: true }))
            }
            _ => Err("it isn't ready to go".into()),
        },
        Step::Arrived { id, to_session } => match live {
            Some(m) if m.id == id && m.state == S::Departed => Ok(Some(m.clone())),
            Some(m) if m.id == id && matches!(m.state, S::Departing { .. }) => Ok(Some(Move {
                state: S::Departed,
                to_session: Some(to_session),
                ..m.clone()
            })),
            _ => Err("no such move here".into()),
        },
        // Only from code that ran before the other machine wrote anything:
        // the session is whole here, and works again.
        Step::NotAccepted { id, why } if same(&id) => match live.map(|m| &m.state) {
            Some(S::Departed) => Err("it arrived already".into()),
            _ => Ok(with(S::Failed { why })),
        },
        Step::Cancel { id } if same(&id) => match live.map(|m| &m.state) {
            Some(S::Pending | S::Departing { sent: false }) => Ok(with(S::Cancelled)),
            Some(S::Departing { sent: true }) => Err(
                "the other machine may have it already; keep it here with Continue here, or retry the move".into(),
            ),
            _ => Err("it moved already".into()),
        },
        Step::ContinueHere { id } if same(&id) => match live.map(|m| &m.state) {
            Some(S::Departed) => Err("it moved already".into()),
            Some(S::Departing { sent: true }) => Ok(with(S::ContinuedHere)),
            _ => Ok(with(S::Cancelled)),
        },
        Step::Message => Ok(match live {
            Some(m) if m.state == S::Pending => with(S::Cancelled),
            _ => now.cloned(),
        }),
        Step::NotAccepted { .. } | Step::Cancel { .. } | Step::ContinueHere { .. } => {
            Err("no such move here".into())
        }
    }
}

/// Why something that would start agent work or change the session's
/// folder can't happen now: the move holds it.
pub fn fence(now: Option<&Move>) -> Option<String> {
    match now.map(|m| (&m.state, &m.to)) {
        Some((MoveState::Departing { .. }, to)) => Some(format!(
            "It's moving to {to}; this waits until the move finishes, or Continue here"
        )),
        Some((MoveState::Departed, to)) => Some(format!("It moved to {to}; carry on there")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MoveState as S;

    fn at(state: S) -> Move {
        Move {
            id: "m1".into(),
            to: "vm".into(),
            state,
            to_session: None,
        }
    }

    #[test]
    fn a_move_goes_pending_departing_departed() {
        let m = next(
            None,
            Step::Start {
                id: "m1".into(),
                to: "vm".into(),
                busy: true,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(m.state, S::Pending);
        let m = next(Some(&m), Step::Quiet).unwrap().unwrap();
        assert_eq!(m.state, S::Departing { sent: false });
        let m = next(Some(&m), Step::Sending { id: "m1".into() })
            .unwrap()
            .unwrap();
        assert_eq!(m.state, S::Departing { sent: true });
        let m = next(
            Some(&m),
            Step::Arrived {
                id: "m1".into(),
                to_session: "s9".into(),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(m.state, S::Departed);
        assert_eq!(m.to_session.as_deref(), Some("s9"));
        // An idle session is fenced at once.
        let idle = next(
            None,
            Step::Start {
                id: "m2".into(),
                to: "vm".into(),
                busy: false,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(idle.state, S::Departing { sent: false });
    }

    #[test]
    fn a_retry_with_the_same_id_is_answered_as_done() {
        let m = at(S::Departing { sent: true });
        assert_eq!(
            next(
                Some(&m),
                Step::Start {
                    id: "m1".into(),
                    to: "vm".into(),
                    busy: false
                }
            )
            .unwrap()
            .unwrap(),
            m
        );
        let gone = at(S::Departed);
        assert_eq!(
            next(
                Some(&gone),
                Step::Arrived {
                    id: "m1".into(),
                    to_session: "s9".into()
                }
            )
            .unwrap()
            .unwrap(),
            gone
        );
        // Another move while one runs, or after it went: refused.
        assert!(
            next(
                Some(&m),
                Step::Start {
                    id: "m2".into(),
                    to: "vm".into(),
                    busy: false
                }
            )
            .is_err()
        );
        assert!(
            next(
                Some(&gone),
                Step::Start {
                    id: "m2".into(),
                    to: "vm".into(),
                    busy: false
                }
            )
            .is_err()
        );
    }

    #[test]
    fn once_anything_may_have_gone_only_continue_here_undoes_it() {
        let sent = at(S::Departing { sent: true });
        assert!(next(Some(&sent), Step::Cancel { id: "m1".into() }).is_err());
        let kept = next(Some(&sent), Step::ContinueHere { id: "m1".into() })
            .unwrap()
            .unwrap();
        assert_eq!(kept.state, S::ContinuedHere);
        // Before anything went, calling it off is just that.
        let quiet = at(S::Departing { sent: false });
        assert_eq!(
            next(Some(&quiet), Step::Cancel { id: "m1".into() })
                .unwrap()
                .unwrap()
                .state,
            S::Cancelled
        );
        // And a move that arrived can't be undone here.
        assert!(
            next(
                Some(&at(S::Departed)),
                Step::ContinueHere { id: "m1".into() }
            )
            .is_err()
        );
        // Another move's id changes nothing.
        assert!(next(Some(&sent), Step::Cancel { id: "m9".into() }).is_err());
    }

    #[test]
    fn a_message_while_it_waits_calls_it_off_but_not_once_fenced() {
        let waiting = at(S::Pending);
        assert_eq!(
            next(Some(&waiting), Step::Message).unwrap().unwrap().state,
            S::Cancelled
        );
        let fenced = at(S::Departing { sent: false });
        assert_eq!(next(Some(&fenced), Step::Message).unwrap().unwrap(), fenced);
        assert!(fence(Some(&fenced)).is_some());
        assert!(fence(Some(&waiting)).is_none());
        assert!(fence(Some(&at(S::Departed))).is_some());
        assert!(fence(None).is_none());
    }

    #[test]
    fn not_accepted_puts_it_back_unless_it_arrived() {
        let sent = at(S::Departing { sent: true });
        let back = next(
            Some(&sent),
            Step::NotAccepted {
                id: "m1".into(),
                why: "no room".into(),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            back.state,
            S::Failed {
                why: "no room".into()
            }
        );
        assert!(fence(Some(&back)).is_none());
        assert!(
            next(
                Some(&at(S::Departed)),
                Step::NotAccepted {
                    id: "m1".into(),
                    why: "x".into()
                }
            )
            .is_err()
        );
    }
}
