//! Moving a session to another machine ([decision 0064]): the source's
//! states and fence (`state`), packing (`pack`) and accepting (`accept`).
//!
//! [decision 0064]: ../../../../docs/decisions/0064-moving-sessions.md

pub mod git;
pub mod state;

/// A session's move as kept, if it has one.
pub fn current(store: &crate::store::Store, session: &str) -> Option<state::Move> {
    serde_json::from_str(&store.get(&move_key(session)).ok()??).ok()
}

/// Refuse what the move fences, for what's done outside the session's own
/// task (forking it, a side question).
pub fn check(store: &crate::store::Store, session: &str) -> anyhow::Result<()> {
    match state::fence(current(store, session).as_ref()) {
        Some(why) => anyhow::bail!(why),
        None => Ok(()),
    }
}

/// Where a session's move is kept in the store.
pub fn move_key(session: &str) -> String {
    format!("move/{session}")
}
