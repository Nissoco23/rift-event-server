//! What the mod knows: the rules, each player's death protection, and the deaths it is waiting on.

use crate::cycle::{Going, Standby};
use crate::rng::Rng;
use crate::store::{Saved, SavedProtection};
use horizon_client::mod_api::NativeServerPlayer;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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

/// Where a death cycled player respawns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CycleMode {
    /// At the set distance from where they died.
    DeathPoint,
    /// Within the set distance of the spawn point.
    SpawnPoint,
}

impl CycleMode {
    pub fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "deathpoint" => Some(Self::DeathPoint),
            "spawnpoint" => Some(Self::SpawnPoint),
            _ => None,
        }
    }
}

/// What a death cycled player's death does to their inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CycleInventory {
    /// It drops and they lose it.
    Drop,
    /// It stays with them and nothing drops.
    Keep,
    /// It stays with them and a copy of it drops.
    Both,
}

impl CycleInventory {
    pub fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "drop" => Some(Self::Drop),
            "keep" => Some(Self::Keep),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Drop => "drop",
            Self::Keep => "keep",
            Self::Both => "both",
        }
    }
}

/// The most blocks a respawn can be from its centre: well inside the world.
pub const MAX_DISTANCE: i32 = 100_000;
/// How far a death cycled player respawns, in blocks, when nothing says otherwise.
pub const DEFAULT_CYCLE_DISTANCE: i32 = 150;

/// Death cycling: a player who dies comes straight back somewhere else, under a new name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cycle {
    pub enabled: bool,
    pub mode: CycleMode,
    /// The point `SpawnPoint` is around; none: the world spawn.
    pub spawn: Option<[i32; 3]>,
    pub inventory: CycleInventory,
    pub distance: i32,
}

impl Default for Cycle {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: CycleMode::DeathPoint,
            spawn: None,
            inventory: CycleInventory::Drop,
            distance: DEFAULT_CYCLE_DISTANCE,
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
    pub cycle: Cycle,
    /// Counts each change of a setting a respawn spot depends on, so a spot found before it is
    /// not used after.
    pub revision: u64,
    /// The players death cycling leaves out. Not saved.
    pub exempt: HashSet<u128>,
    /// Each player's respawn spot, found while they live (`cycle.rs`).
    pub standby: HashMap<u128, Standby>,
    /// Players who died and wait for a spot.
    pub waiting: HashSet<u128>,
    /// Players who died, have a spot and are being respawned.
    pub going: HashMap<u128, Going>,
    /// Players who respawned and are to be given a random nickname at the next tick.
    pub rename: Vec<(u128, String)>,
    /// Ticks of the overworld.
    pub tick: u64,
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
            cycle: Cycle::default(),
            revision: 0,
            exempt: HashSet::new(),
            standby: HashMap::new(),
            waiting: HashSet::new(),
            going: HashMap::new(),
            rename: Vec::new(),
            tick: 0,
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
            cycle: self.cycle,
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
        self.cycle = Cycle {
            distance: saved.cycle.distance.clamp(0, MAX_DISTANCE),
            ..saved.cycle
        };
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
