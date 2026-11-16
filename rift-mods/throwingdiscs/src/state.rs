//! What the mod holds between ticks: the discs in flight, the work a hook queued for the next level
//! tick, and the config.

use crate::config::Config;
use crate::flight::Disc;
use crate::rng::Rng;
use horizon_client::mod_api::{Hand, ItemStack, NativeServerPlayer};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

/// Work for the next level tick. A hook runs inside the player's own turn, where the player's
/// inventory is out of reach, so it only queues what it asks for.
#[derive(Clone, Debug)]
pub enum Task {
    /// A player right-clicked a disc in this hand.
    Throw { uuid: u128, hand: Hand },
    /// `/enchant` on the disc in the player's main hand.
    Enchant {
        uuid: u128,
        enchantment: String,
        level: u32,
    },
}

/// A player as the last level tick saw them, for the hooks that run where the server's players are out
/// of reach (`player_command`).
#[derive(Clone, Debug)]
pub struct Member {
    pub player: NativeServerPlayer,
    /// Whether the main hand holds a disc.
    pub holds_disc: bool,
}

pub struct State {
    pub flying: Vec<Disc>,
    pub roster: Vec<Member>,
    pub tasks: VecDeque<Task>,
    /// The server tick each player last threw in: a player throws at most one disc a tick.
    pub last_throw: HashMap<u128, u64>,
    pub tick: u64,
    pub rng: Rng,
    /// Discs that go back to a player when they respawn: those they had in the air when they died
    /// under keepInventory.
    pub returns: HashMap<u128, Vec<ItemStack>>,
    /// The dimensions whose strays (displays a crash left) have been removed.
    pub cleaned: HashSet<String>,
}

pub struct Shared {
    state: Mutex<State>,
    config: RwLock<Arc<Config>>,
}

impl Shared {
    pub fn new(rng: Rng) -> Self {
        Self {
            state: Mutex::new(State {
                flying: Vec::new(),
                roster: Vec::new(),
                tasks: VecDeque::new(),
                last_throw: HashMap::new(),
                tick: 0,
                rng,
                returns: HashMap::new(),
                cleaned: HashSet::new(),
            }),
            config: RwLock::new(Arc::new(Config::default())),
        }
    }

    /// The state. Never held across a call to the host: a hurt can end in a hook of this mod.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn config(&self) -> Arc<Config> {
        Arc::clone(&self.config.read().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn set_config(&self, config: Config) {
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(config);
    }
}
