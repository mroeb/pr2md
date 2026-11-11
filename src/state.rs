//! `state.json` - what pr2md remembers between syncs (also the cache the notes are rendered from).

use crate::github::{Pr, Repo};
use crate::util::{self, Res};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub login: String,
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
    pub prs: BTreeMap<String, Tracked>,
    pub repos: BTreeMap<String, Repo>,
    /// Newest first.
    pub activity: Vec<Activity>,
    /// Checkboxes the user ticked in Obsidian (block ids).
    pub local_done: BTreeSet<String>,
    /// Checkboxes rendered as ticked last time - used to detect what the user changed.
    pub rendered_done: BTreeSet<String>,
    pub todo_count: usize,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Tracked {
    pub pr: Pr,
    pub fingerprint: String,
    pub fetched_at: i64,
    /// Vault-relative path of the note, `/`-separated.
    pub file: String,
    /// No longer matched by any search; shown under "Recently closed".
    pub archived: bool,
    /// Event id -> unix time first seen (0 = existed before pr2md started tracking).
    pub first_seen: BTreeMap<String, i64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Activity {
    pub time: String,
    pub key: String,
    pub text: String,
}

impl State {
    pub fn load() -> Option<State> {
        let text = std::fs::read_to_string(util::state_path()).ok()?;
        match serde_json::from_str(&text) {
            Ok(s) => Some(s),
            Err(e) => {
                crate::warn!("state.json is unreadable ({e}); starting fresh");
                None
            }
        }
    }

    pub fn save(&self) -> Res<()> {
        util::write_atomic(&util::state_path(), serde_json::to_string(self)?.as_bytes())?;
        Ok(())
    }
}
