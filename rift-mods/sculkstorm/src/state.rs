//! What the mod keeps between hooks.
//!
//! Hooks take `&self` and can run again inside each other (a block change heard while the mod is
//! changing blocks), so state lives behind a lock that is held for a moment and never across a
//! call into the game.

use crate::config::Config;
use crate::crater::Craters;
use crate::tasks::Task;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ability {
    Ascension,
    Cataclysm,
}

#[derive(Default)]
struct Cooldowns {
    ascension: Option<Instant>,
    cataclysm: Option<Instant>,
}

impl Cooldowns {
    fn slot(&mut self, ability: Ability) -> &mut Option<Instant> {
        match ability {
            Ability::Ascension => &mut self.ascension,
            Ability::Cataclysm => &mut self.cataclysm,
        }
    }
}

/// A player's protection from the fall their ascension ends in.
struct FallGuard {
    uuid: u128,
    entity_id: u32,
    until: Instant,
}

#[derive(Default)]
pub struct State {
    cooldowns: HashMap<u128, Cooldowns>,
    /// Players in the middle of an ascension's charge.
    charging: HashSet<u128>,
    fall: Vec<FallGuard>,
    /// The tick each player's last scythe click was handled in.
    last_click: HashMap<u128, u64>,
    /// Everything that runs tick by tick: charges, flights, fields, debris.
    pub tasks: Vec<Task>,
    /// Cataclysms going, from the cast until the field they leave is gone.
    cataclysms: usize,
    pub craters: Craters,
    /// Dimensions whose leftover debris has been swept up since the server started.
    pub swept: HashSet<String>,
}

impl State {
    /// How long `who` must still wait to use `ability`; None when they can.
    pub fn cooldown_left(&self, who: u128, ability: Ability, now: Instant) -> Option<Duration> {
        let cooldowns = self.cooldowns.get(&who)?;
        let ready = match ability {
            Ability::Ascension => cooldowns.ascension,
            Ability::Cataclysm => cooldowns.cataclysm,
        }?;
        ready
            .checked_duration_since(now)
            .filter(|left| !left.is_zero())
    }

    pub fn start_cooldown(&mut self, who: u128, ability: Ability, until: Instant) {
        *self.cooldowns.entry(who).or_default().slot(ability) = Some(until);
    }

    pub fn is_charging(&self, who: u128) -> bool {
        self.charging.contains(&who)
    }

    pub fn set_charging(&mut self, who: u128, on: bool) {
        if on {
            self.charging.insert(who);
        } else {
            self.charging.remove(&who);
        }
    }

    /// Protects `uuid` (whose entity id is `entity_id`) from fall damage until `until`.
    pub fn protect_from_fall(&mut self, uuid: u128, entity_id: u32, until: Instant) {
        self.fall.retain(|g| g.uuid != uuid);
        self.fall.push(FallGuard {
            uuid,
            entity_id,
            until,
        });
    }

    /// Spends the protection of the player with entity id `entity_id`: whether they had it still.
    pub fn consume_fall_protection(&mut self, entity_id: u32, now: Instant) -> bool {
        let Some(at) = self.fall.iter().position(|g| g.entity_id == entity_id) else {
            return false;
        };
        self.fall.swap_remove(at).until >= now
    }

    /// Whether `who` already clicked with the scythe in `tick` (a click on a block reaches the
    /// game as two: the block's, then the item's). Records the click when it is the first.
    pub fn repeated_click(&mut self, who: u128, tick: u64) -> bool {
        self.last_click.insert(who, tick) == Some(tick)
    }

    /// Whether another cataclysm may start, with `max` allowed at once.
    pub fn can_start_cataclysm(&self, max: usize) -> bool {
        self.cataclysms < max.max(1)
    }

    pub fn cataclysm_started(&mut self) {
        self.cataclysms += 1;
    }

    pub fn cataclysm_ended(&mut self) {
        self.cataclysms = self.cataclysms.saturating_sub(1);
    }

    #[cfg(test)]
    pub(crate) fn cataclysms(&self) -> usize {
        self.cataclysms
    }

    /// What a player's leaving clears: the plugin's `onQuit`.
    pub fn forget(&mut self, who: u128, now: Instant) {
        self.charging.remove(&who);
        self.fall.retain(|g| g.uuid != who);
        self.last_click.remove(&who);
        self.cooldowns.retain(|_, c| {
            c.ascension = c.ascension.filter(|t| *t > now);
            c.cataclysm = c.cataclysm.filter(|t| *t > now);
            c.ascension.is_some() || c.cataclysm.is_some()
        });
    }
}

/// The mod's state and settings, shared by every hook.
pub struct Shared {
    config: RwLock<Arc<Config>>,
    state: Mutex<State>,
    tick: AtomicU64,
    /// Set while the mod puts blocks back that it was told were changed, so it does not hear
    /// itself.
    reverting: AtomicBool,
    /// Whether any crater's area is being watched: `block_changed` is heard for every block
    /// change in the world and does nothing at all when this is off.
    watching: AtomicBool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            config: RwLock::new(Arc::new(Config::default())),
            state: Mutex::new(State::default()),
            tick: AtomicU64::new(0),
            reverting: AtomicBool::new(false),
            watching: AtomicBool::new(false),
        }
    }

    pub fn config(&self) -> Arc<Config> {
        match self.config.read() {
            Ok(config) => Arc::clone(&config),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    pub fn set_config(&self, config: Config) {
        let config = Arc::new(config);
        match self.config.write() {
            Ok(mut slot) => *slot = config,
            Err(poisoned) => *poisoned.into_inner() = config,
        }
    }

    /// The state, for a moment. A panic in an earlier hook leaves it as it was.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The server tick the mod counts: ticks that ran, not ticks that were paused or frozen.
    pub fn tick(&self) -> u64 {
        self.tick.load(Ordering::Relaxed)
    }

    pub fn advance(&self) -> u64 {
        self.tick.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn reverting(&self) -> bool {
        self.reverting.load(Ordering::Relaxed)
    }

    pub fn set_reverting(&self, on: bool) {
        self.reverting.store(on, Ordering::Relaxed);
    }

    pub fn watching(&self) -> bool {
        self.watching.load(Ordering::Relaxed)
    }

    pub fn set_watching(&self, on: bool) {
        self.watching.store(on, Ordering::Relaxed);
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALEX: u128 = 1;
    const BOB: u128 = 2;

    #[test]
    fn a_cooldown_counts_down_and_is_for_one_ability_of_one_player() {
        let mut s = State::default();
        let now = Instant::now();
        assert_eq!(s.cooldown_left(ALEX, Ability::Ascension, now), None);
        s.start_cooldown(ALEX, Ability::Ascension, now + Duration::from_secs(15));
        assert_eq!(
            s.cooldown_left(ALEX, Ability::Ascension, now),
            Some(Duration::from_secs(15))
        );
        assert_eq!(
            s.cooldown_left(ALEX, Ability::Ascension, now + Duration::from_secs(10)),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            s.cooldown_left(ALEX, Ability::Ascension, now + Duration::from_secs(15)),
            None
        );
        assert_eq!(s.cooldown_left(ALEX, Ability::Cataclysm, now), None);
        assert_eq!(s.cooldown_left(BOB, Ability::Ascension, now), None);
    }

    #[test]
    fn a_players_charge_is_set_and_cleared() {
        let mut s = State::default();
        assert!(!s.is_charging(ALEX));
        s.set_charging(ALEX, true);
        assert!(s.is_charging(ALEX) && !s.is_charging(BOB));
        s.set_charging(ALEX, false);
        assert!(!s.is_charging(ALEX));
    }

    #[test]
    fn fall_protection_is_spent_once_and_only_while_it_lasts() {
        let mut s = State::default();
        let now = Instant::now();
        s.protect_from_fall(ALEX, 10, now + Duration::from_secs(12));
        assert!(!s.consume_fall_protection(11, now), "another entity's fall");
        assert!(s.consume_fall_protection(10, now + Duration::from_secs(5)));
        assert!(
            !s.consume_fall_protection(10, now + Duration::from_secs(5)),
            "spent"
        );
        s.protect_from_fall(ALEX, 10, now + Duration::from_secs(12));
        assert!(
            !s.consume_fall_protection(10, now + Duration::from_secs(13)),
            "it ran out"
        );
        // A second ascension replaces the first's protection instead of adding to it.
        s.protect_from_fall(ALEX, 10, now + Duration::from_secs(1));
        s.protect_from_fall(ALEX, 10, now + Duration::from_secs(30));
        assert!(s.consume_fall_protection(10, now + Duration::from_secs(20)));
        assert!(!s.consume_fall_protection(10, now + Duration::from_secs(20)));
    }

    #[test]
    fn the_second_click_of_a_tick_is_a_repeat_and_the_next_tick_is_not() {
        let mut s = State::default();
        assert!(!s.repeated_click(ALEX, 100));
        assert!(s.repeated_click(ALEX, 100));
        assert!(!s.repeated_click(BOB, 100));
        assert!(!s.repeated_click(ALEX, 101));
    }

    #[test]
    fn the_number_of_cataclysms_at_once_is_capped_and_never_below_one() {
        let mut s = State::default();
        for _ in 0..3 {
            assert!(s.can_start_cataclysm(3));
            s.cataclysm_started();
        }
        assert!(!s.can_start_cataclysm(3));
        s.cataclysm_ended();
        assert!(s.can_start_cataclysm(3));
        for _ in 0..10 {
            s.cataclysm_ended();
        }
        assert_eq!(s.cataclysms(), 0, "it never goes below none");
        // A cap of 0 still allows one, and not a second.
        assert!(s.can_start_cataclysm(0));
        s.cataclysm_started();
        assert!(!s.can_start_cataclysm(0));
    }

    #[test]
    fn a_player_who_leaves_takes_their_state_with_them_and_spent_cooldowns_go() {
        let mut s = State::default();
        let now = Instant::now();
        s.set_charging(ALEX, true);
        s.protect_from_fall(ALEX, 10, now + Duration::from_secs(9));
        s.repeated_click(ALEX, 5);
        s.start_cooldown(ALEX, Ability::Ascension, now + Duration::from_secs(9));
        s.start_cooldown(BOB, Ability::Cataclysm, now - Duration::from_secs(1));
        s.forget(ALEX, now);
        assert!(!s.is_charging(ALEX));
        assert!(!s.consume_fall_protection(10, now));
        assert!(!s.repeated_click(ALEX, 5));
        // Their cooldown is still running, so they cannot clear it by logging out; Bob's ended.
        assert!(s.cooldown_left(ALEX, Ability::Ascension, now).is_some());
        assert!(s.cooldown_left(BOB, Ability::Cataclysm, now).is_none());
    }

    #[test]
    fn the_config_is_swapped_whole_and_the_tick_counts_up() {
        let shared = Shared::new();
        assert!(shared.config().pvp);
        shared.set_config(Config {
            pvp: false,
            ..Config::default()
        });
        assert!(!shared.config().pvp);
        assert_eq!(shared.tick(), 0);
        assert_eq!(shared.advance(), 1);
        assert_eq!(shared.advance(), 2);
        assert_eq!(shared.tick(), 2);
    }
}
