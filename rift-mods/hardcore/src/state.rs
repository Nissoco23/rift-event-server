//! What the mod knows: the rules, each player's death protection, and the deaths it is waiting on.

use crate::rng::Rng;
use crate::store::{Saved, SavedProtection};
use horizon_client::mod_api::NativeServerPlayer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What the others are told when a player dies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeathMsg {
    /// Nothing.
    None,
    /// The death message, as vanilla shows it.
    Death,
    /// A "left the game" line in its place, and the real join and leave lines are hidden.
    Leave,
}

impl DeathMsg {
    pub fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "death" => Some(Self::Death),
            "leave" => Some(Self::Leave),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Death => "death",
            Self::Leave => "leave",
        }
    }
}

/// What happens to a player who dies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDeath {
    /// They respawn as a spectator, where they died.
    Spectate,
    /// They are disconnected with their death message.
    Kick,
    /// Vanilla: the death screen and a respawn.
    None,
}

impl OnDeath {
    pub fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "spectate" => Some(Self::Spectate),
            "kick" => Some(Self::Kick),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// The lowest health a player is held to, when they have turned it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Protection {
    pub enabled: bool,
    /// Health points, 1 to 20: half a heart each.
    pub level: u8,
}

impl Default for Protection {
    fn default() -> Self {
        Self {
            enabled: false,
            level: 1,
        }
    }
}

/// Where a player who is to spectate died.
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    pub dimension: String,
    pub position: [f64; 3],
}

pub struct State {
    pub deathmsg: DeathMsg,
    /// Blocks a death message travels; below 0, everyone hears it.
    pub radius: i32,
    pub protection: HashMap<u128, Protection>,
    /// For players, and for operators. They are not saved: a restart sets both back.
    pub on_death: OnDeath,
    pub on_death_admin: OnDeath,
    /// The players as the last tick or command saw them, for the hooks that cannot reach the
    /// server's players.
    pub roster: Vec<NativeServerPlayer>,
    /// The players who died and are to spectate, until they respawn.
    pub spectate: HashMap<u128, Spot>,
    /// What the engine was last told about the join and leave lines, none until it was told.
    pub lines_applied: Option<bool>,
    pub rng: Rng,
}

/// The length of the death message's reach when nothing says otherwise.
pub const DEFAULT_RADIUS: i32 = 150;

impl State {
    pub fn new(rng: Rng) -> Self {
        Self {
            deathmsg: DeathMsg::Leave,
            radius: DEFAULT_RADIUS,
            protection: HashMap::new(),
            on_death: OnDeath::Kick,
            on_death_admin: OnDeath::Spectate,
            roster: Vec::new(),
            spectate: HashMap::new(),
            lines_applied: None,
            rng,
        }
    }

    pub fn protection_of(&self, uuid: u128) -> Protection {
        self.protection.get(&uuid).copied().unwrap_or_default()
    }

    /// The player `uuid`, as the roster has them.
    pub fn member(&self, uuid: u128) -> Option<&NativeServerPlayer> {
        self.roster.iter().find(|p| p.uuid == uuid)
    }

    /// What is saved: the rules and the protection of every player who differs from the default.
    pub fn saved(&self) -> Saved {
        Saved {
            deathmsg: self.deathmsg,
            deathmsgradius: self.radius,
            protection: self
                .protection
                .iter()
                .filter(|(_, p)| **p != Protection::default())
                .map(|(uuid, p)| {
                    (
                        crate::store::uuid_string(*uuid),
                        SavedProtection {
                            enabled: p.enabled,
                            level: p.level,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Takes up what was saved. An entry that is not a player's UUID or has a level outside 1 to 20
    /// is skipped.
    pub fn restore(&mut self, saved: Saved) {
        self.deathmsg = saved.deathmsg;
        self.radius = saved.deathmsgradius.max(-1);
        self.protection = saved
            .protection
            .into_iter()
            .filter_map(|(key, p)| {
                let uuid = crate::store::parse_uuid(&key)?;
                (1..=20).contains(&p.level).then_some((
                    uuid,
                    Protection {
                        enabled: p.enabled,
                        level: p.level,
                    },
                ))
            })
            .collect();
    }
}
