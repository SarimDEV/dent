//! What the window knows about each remote machine: saved, being added, how
//! it stands now. Settings › Machines draws it; session rows take their
//! machine's name from it.

use dent_client_core::machine::State;
use dent_link::Trust;
use dent_protocol::AgentInfo;
use dent_remote::{Event, Saved};

#[derive(Debug, Clone)]
pub struct MachineView {
    /// Its id, or for one being added, the tag it goes by until its welcome
    /// ([`crate::net::adding_tag`]).
    pub id: String,
    pub saved: Saved,
    pub state: State,
    /// The last ping's round trip.
    pub rtt: Option<u64>,
    /// The route the link took and how this device is trusted there, since
    /// the last welcome.
    pub via: Option<(String, Trust)>,
    pub version: String,
    /// The protocol its daemon speaks, since its last welcome.
    pub proto: Option<u32>,
    pub agents: Vec<AgentInfo>,
    /// Its daemon has said how each agent stands (the welcome says only
    /// what they are).
    pub agents_known: bool,
    /// Why pairing failed, until the next try.
    pub pair_error: Option<String>,
    /// A known route answered with another machine.
    pub impostor: Option<String>,
    /// How it stood when it last went away (asleep, unreachable), kept
    /// while it's tried again: a retry doesn't make it any less away, so
    /// what's shown for it doesn't come and go with each try.
    pub last_away: Option<State>,
}

impl MachineView {
    pub fn saved(s: Saved) -> Self {
        Self {
            id: s.id.clone(),
            saved: s,
            state: State::Connecting,
            rtt: None,
            via: None,
            version: String::new(),
            proto: None,
            agents: Vec::new(),
            agents_known: false,
            pair_error: None,
            impostor: None,
            last_away: None,
        }
    }

    /// How it stands for showing: while it's being tried again after going
    /// away, still as it went away.
    pub fn standing(&self) -> &State {
        match (&self.state, &self.last_away) {
            (State::Connecting | State::Reconnecting, Some(away)) => away,
            (s, _) => s,
        }
    }

    /// Why its sessions can't be working, when it's away; `None` while
    /// it's online or still finding out.
    pub fn away(&self) -> Option<String> {
        let name = self.name();
        match self.standing() {
            State::Online | State::Slow | State::Connecting | State::Reconnecting => None,
            State::Away(dent_link::Goodbye::Sleep) => Some(format!("waiting for {name} to wake")),
            State::Away(dent_link::Goodbye::Awake) => None,
            State::Away(_) => Some(format!("waiting for {name} to come back")),
            _ => Some(format!("{name} can't be reached")),
        }
    }

    /// What to call it: its name, else the route it's being added at.
    pub fn name(&self) -> &str {
        if !self.saved.name.is_empty() {
            &self.saved.name
        } else {
            self.saved.routes.first().map_or(&self.id, |r| r)
        }
    }
}

/// Fold one event from machine `id` into the list.
pub fn apply(list: &mut Vec<MachineView>, id: &str, event: Event) {
    let ix = match list.iter().position(|m| m.id == id) {
        Some(i) => i,
        None => {
            list.push(MachineView::saved(Saved {
                id: id.to_string(),
                ..Saved::default()
            }));
            list.len() - 1
        }
    };
    let m = &mut list[ix];
    match event {
        Event::State(s) => {
            if !matches!(s, State::Online | State::Slow) {
                m.rtt = None;
            }
            match &s {
                State::Connecting | State::Reconnecting | State::Off => {}
                State::Away(dent_link::Goodbye::Awake) => m.last_away = None,
                State::Away(_) | State::Unreachable { .. } => m.last_away = Some(s.clone()),
                _ => m.last_away = None,
            }
            m.state = s;
        }
        Event::Welcome {
            name,
            version,
            proto,
            agents,
            via,
            trust,
            ..
        } => {
            m.saved.name = name;
            m.version = version;
            m.proto = Some(proto);
            // The welcome says what each agent is; how it stands (installed,
            // signed in) comes with the Agents reply, so a reconnect keeps
            // the last one for agents still there.
            let what = |a: &AgentInfo| (a.id.clone(), a.name.clone(), a.caps.clone());
            if m.agents.iter().map(what).ne(agents.iter().map(what)) {
                m.agents = agents;
            }
            m.via = Some((via, trust));
            m.pair_error = None;
            m.impostor = None;
        }
        // Smoothed, so one slow ping doesn't throw the machine across the
        // strip.
        Event::Rtt(ms) => {
            m.rtt = Some(match m.rtt {
                Some(old) => (old * 3 + ms + 2) / 4,
                None => ms,
            })
        }
        Event::Saved(s) => m.saved = s,
        Event::PairFailed(why) => m.pair_error = Some(why),
        Event::DifferentMachine(addr) => m.impostor = Some(addr),
        Event::Out { .. } => {}
    }
}

/// A machine being added now goes by its id; one already saved under that
/// id (added twice) gives way to it.
pub fn rename(list: &mut Vec<MachineView>, from: &str, to: &str) {
    if list.iter().any(|m| m.id == from) {
        list.retain(|m| m.id != to);
    }
    if let Some(m) = list.iter_mut().find(|m| m.id == from) {
        m.id = to.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_being_added_takes_its_id_and_away_reads_plainly() {
        let mut list = vec![MachineView::saved(Saved {
            id: "ab12".into(),
            name: "mini".into(),
            ..Saved::default()
        })];
        apply(&mut list, "+10.0.0.2:7480", Event::State(State::Connecting));
        assert_eq!(list[1].name(), "+10.0.0.2:7480");
        // Added twice: the new link replaces the old entry.
        rename(&mut list, "+10.0.0.2:7480", "ab12");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "ab12");

        apply(&mut list, "ab12", Event::Rtt(3));
        apply(
            &mut list,
            "ab12",
            Event::State(State::Away(dent_link::Goodbye::Sleep)),
        );
        assert_eq!(
            list[0].rtt, None,
            "an old round trip isn't shown for a sleeping machine"
        );
        let m = &mut list[0];
        m.saved.name = "mini".into();
        assert_eq!(m.away().as_deref(), Some("waiting for mini to wake"));
        apply(&mut list, "ab12", Event::State(State::Reconnecting));
        assert_eq!(
            list[0].away().as_deref(),
            Some("waiting for mini to wake"),
            "tried again while asleep, it's still asleep"
        );
        let unreachable = State::Unreachable {
            why: "timeout".into(),
        };
        apply(&mut list, "ab12", Event::State(unreachable.clone()));
        assert_eq!(list[0].away().as_deref(), Some("mini can't be reached"));
        apply(&mut list, "ab12", Event::State(State::Connecting));
        assert_eq!(*list[0].standing(), unreachable, "no flicker between tries");
        apply(&mut list, "ab12", Event::State(State::Online));
        apply(&mut list, "ab12", Event::State(State::Reconnecting));
        assert_eq!(
            list[0].away(),
            None,
            "a link dropping from online is still finding out: sessions keep their state"
        );
    }

    fn mini() -> Vec<MachineView> {
        vec![MachineView::saved(Saved {
            id: "ab12".into(),
            name: "mini".into(),
            ..Saved::default()
        })]
    }

    fn goes(list: &mut Vec<MachineView>, s: State) {
        apply(list, "ab12", Event::State(s));
    }

    fn aways() -> Vec<State> {
        use dent_link::Goodbye;
        vec![
            State::Away(Goodbye::Sleep),
            State::Unreachable {
                why: "timeout".into(),
            },
            State::Away(Goodbye::Quit),
            State::Away(Goodbye::Restart {
                expected_down_ms: 500,
            }),
            State::Away(Goodbye::Update {
                to: "9".into(),
                expected_down_ms: 500,
            }),
        ]
    }

    #[test]
    fn away_holds_across_every_retry() {
        for away in aways() {
            let mut list = mini();
            goes(&mut list, away.clone());
            let said = list[0].away();
            for s in [
                State::Connecting,
                State::Reconnecting,
                State::Connecting,
                State::Reconnecting,
            ] {
                goes(&mut list, s.clone());
                assert_eq!(*list[0].standing(), away, "{away:?} then {s:?}");
                assert_eq!(list[0].away(), said, "{away:?} then {s:?}");
            }
        }
    }

    #[test]
    fn away_never_sticks_after_online() {
        for away in aways() {
            for online in [State::Online, State::Slow] {
                let mut list = mini();
                goes(&mut list, away.clone());
                goes(&mut list, State::Connecting);
                goes(&mut list, online.clone());
                assert_eq!(*list[0].standing(), online);
                assert_eq!(list[0].away(), None);
                for s in [State::Reconnecting, State::Connecting] {
                    goes(&mut list, s.clone());
                    assert_eq!(*list[0].standing(), s, "{away:?}, {online:?}, then {s:?}");
                    assert_eq!(list[0].away(), None);
                }
            }
        }
    }

    #[test]
    fn whatever_comes_after_away_is_shown_as_itself() {
        use dent_link::Goodbye;
        let after = [
            State::Away(Goodbye::Awake),
            State::Online,
            State::Slow,
            State::Removed,
            State::NeedsCode { pairing_open: true },
            State::Incompatible {
                daemon: (1, 2),
                client: (3, 4),
            },
            State::TrustedElsewhere {
                routes: vec!["ts".into()],
            },
        ];
        for away in aways() {
            for s in &after {
                let mut list = mini();
                goes(&mut list, away.clone());
                goes(&mut list, State::Reconnecting);
                goes(&mut list, s.clone());
                assert_eq!(list[0].standing(), s, "{away:?} then {s:?}");
                // And what was away is gone with it: a retry is a retry.
                goes(&mut list, State::Connecting);
                assert_eq!(
                    *list[0].standing(),
                    State::Connecting,
                    "{away:?}, {s:?}, then Connecting"
                );
            }
        }
    }

    #[test]
    fn the_latest_way_away_is_the_one_held() {
        use dent_link::Goodbye;
        let mut list = mini();
        let unreachable = State::Unreachable {
            why: "timeout".into(),
        };
        goes(&mut list, State::Away(Goodbye::Sleep));
        goes(&mut list, State::Connecting);
        goes(&mut list, unreachable.clone());
        goes(&mut list, State::Reconnecting);
        assert_eq!(*list[0].standing(), unreachable);
        goes(&mut list, State::Away(Goodbye::Sleep));
        goes(&mut list, State::Connecting);
        assert_eq!(*list[0].standing(), State::Away(Goodbye::Sleep));
    }

    #[test]
    fn off_is_shown_as_off_while_away() {
        use dent_link::Goodbye;
        let mut list = mini();
        goes(&mut list, State::Away(Goodbye::Sleep));
        goes(&mut list, State::Off);
        assert_eq!(*list[0].standing(), State::Off);
        // As Off reads on its own (not as asleep).
        let mut fresh = mini();
        goes(&mut fresh, State::Off);
        assert_eq!(list[0].away(), fresh[0].away());
    }

    // Turned off, then on again later: is it still "waiting to wake"? The
    // code says yes (Off keeps what was away); this asks for a plain retry.
    #[test]
    fn turned_off_and_on_again_is_a_fresh_try() {
        use dent_link::Goodbye;
        let mut list = mini();
        goes(&mut list, State::Away(Goodbye::Sleep));
        goes(&mut list, State::Off);
        goes(&mut list, State::Connecting);
        assert_eq!(*list[0].standing(), State::Connecting);
    }

    #[test]
    fn a_fresh_machine_trying_isnt_away() {
        let mut list = mini();
        goes(&mut list, State::Connecting);
        assert_eq!(*list[0].standing(), State::Connecting);
        assert_eq!(list[0].away(), None);
        // One never heard of before, first heard trying.
        apply(&mut list, "zz99", Event::State(State::Reconnecting));
        assert_eq!(*list[1].standing(), State::Reconnecting);
    }
}
