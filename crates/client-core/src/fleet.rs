//! Sessions from several machines in one client ([decision 0058]). Each
//! machine keeps its own ids; the client qualifies a remote machine's with
//! that machine's id, so two daemons' sessions never meet, and strips the
//! qualifier again on the way out. This machine's own sessions keep their
//! plain ids.
//!
//! [decision 0058]: ../../docs/decisions/0058-independent-machines.md

use dent_protocol::{Out, Request};

/// Between a machine's id and a session's. Session ids never hold one.
pub const SEP: char = '/';

/// The id a client keeps for `machine`'s session `id`; this machine is "".
pub fn qualify(machine: &str, id: &str) -> String {
    if machine.is_empty() {
        id.to_string()
    } else {
        format!("{machine}{SEP}{id}")
    }
}

/// The machine a client's session id belongs to ("" for this one), and the
/// id that machine knows it by.
pub fn split(key: &str) -> (&str, &str) {
    key.split_once(SEP).unwrap_or(("", key))
}

/// Qualify every session id in a message from `machine`.
pub fn qualify_out(machine: &str, out: &mut Out) {
    if machine.is_empty() {
        return;
    }
    for s in out.sessions_mut() {
        *s = qualify(machine, s);
    }
}

/// Which machine a request is for, by the sessions it names, with their ids
/// made plain again. `None` when it names sessions on different machines.
/// One that names no session is for whoever sends it: "".
pub fn route(req: &mut Request) -> Option<String> {
    let mut machine: Option<String> = None;
    for s in req.sessions_mut() {
        let (m, id) = split(s);
        match &machine {
            Some(prev) if prev != m => return None,
            Some(_) => {}
            None => machine = Some(m.to_string()),
        }
        *s = id.to_string();
    }
    Some(machine.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dent_protocol::{Body, Event, Permission};

    #[test]
    fn this_machines_ids_stay_plain() {
        assert_eq!(qualify("", "s1"), "s1");
        assert_eq!(split("s1"), ("", "s1"));
        assert_eq!(split(&qualify("ab12", "s1")), ("ab12", "s1"));
    }

    #[test]
    fn a_remote_feed_is_qualified_and_requests_go_back_plain() {
        let mut out = Out::Events {
            events: vec![Event {
                seq: 1,
                session: "s2".into(),
                at: 0,
                body: Body::Forked {
                    from: "s1".into(),
                    upto: None,
                },
            }],
        };
        qualify_out("ab12", &mut out);
        let Out::Events { events } = &out else {
            unreachable!()
        };
        assert_eq!(events[0].session, "ab12/s2");
        assert!(matches!(&events[0].body, Body::Forked { from, .. } if from == "ab12/s1"));

        let mut req = Request::SetPermission {
            session: "ab12/s2".into(),
            permission: Permission::default(),
        };
        assert_eq!(route(&mut req).as_deref(), Some("ab12"));
        assert!(matches!(&req, Request::SetPermission { session, .. } if session == "s2"));
        let mut local = Request::Close {
            session: "s9".into(),
        };
        assert_eq!(route(&mut local).as_deref(), Some(""));
        assert_eq!(route(&mut Request::Projects).as_deref(), Some(""));
    }

    fn created(seq: u64, session: &str) -> Out {
        Out::Events {
            events: vec![Event {
                seq,
                session: session.into(),
                at: seq,
                body: Body::Created {
                    agent: "claude".into(),
                    cwd: "/r".into(),
                    model: None,
                    permission: Permission::default(),
                    title: "t".into(),
                },
            }],
        }
    }

    #[test]
    fn two_machines_with_the_same_ids_and_their_own_cursors() {
        let mut s = crate::Sessions::default();
        s.apply(&created(5, "s1"));
        let mut far = created(1, "s1");
        qualify_out("ab12", &mut far);
        assert_eq!(s.apply_from("ab12", &far), vec!["ab12/s1".to_string()]);
        // The far machine's low sequence numbers don't count as old here.
        assert_eq!(s.list.len(), 2);
        assert_eq!((s.last_seq, s.feed("ab12").last_seq), (5, 1));
        // This machine live, the far one still catching up: its sessions'
        // changes are history, not news.
        s.apply(&Out::CaughtUp { seq: 5 });
        assert!(s.live_for("s1") && !s.live_for("ab12/s1"));
        s.apply_from("ab12", &Out::CaughtUp { seq: 1 });
        assert!(s.feed("ab12").live && s.live_for("ab12/s1"));
        // Its log started again: only its sessions go.
        assert_eq!(s.forget("ab12"), vec!["ab12/s1".to_string()]);
        assert_eq!(s.list.keys().collect::<Vec<_>>(), ["s1"]);
        assert_eq!(s.feed("ab12").last_seq, 0);
    }
}
