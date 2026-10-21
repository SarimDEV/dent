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
    pub agents: Vec<AgentInfo>,
    /// Why pairing failed, until the next try.
    pub pair_error: Option<String>,
    /// A known route answered with another machine.
    pub impostor: Option<String>,
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
            agents: Vec::new(),
            pair_error: None,
            impostor: None,
        }
    }

    /// Why its sessions can't be working, when it's away; `None` while
    /// it's online or still finding out.
    pub fn away(&self) -> Option<String> {
        let name = self.name();
        match &self.state {
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
            m.state = s;
        }
        Event::Welcome {
            name,
            version,
            agents,
            via,
            trust,
            ..
        } => {
            m.saved.name = name;
            m.version = version;
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
        m.state = State::Reconnecting;
        assert_eq!(
            m.away(),
            None,
            "still finding out: sessions keep their state"
        );
        m.state = State::Unreachable {
            why: "timeout".into(),
        };
        assert_eq!(m.away().as_deref(), Some("mini can't be reached"));
    }
}
