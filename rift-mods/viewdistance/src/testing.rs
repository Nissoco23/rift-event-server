//! A server in memory, and the mod's real hooks driven over it.
//!
//! `FakeHost` is the part of the engine's `Host` that the mod uses, with a record of what it was
//! asked to do and the engine's rule for what a player ends up with (the smallest of the server's
//! distance and the limit). `Rig` boxes the mod as the engine does and runs its hooks through the
//! engine's own `dispatch_*` functions, so what the tests exercise is what the server runs.

use horizon_client::mod_api::{
    Hooks, Host, NativeAuthority, NativeClientInfo, NativeLifecycleContext, NativeServerPlayer,
    NativeWorldEpoch, ServerTickPhase, Text, dispatch_perform_command,
    dispatch_perform_console_command, dispatch_player_logged_in, dispatch_player_logged_out,
    dispatch_register_commands, dispatch_server_tick,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The server's own view distance in these tests.
const SERVER_DISTANCE: i32 = 10;

pub struct FakeHost {
    pub players: Vec<NativeServerPlayer>,
    /// The limits the engine holds, as the mod set them.
    pub limits: HashMap<u128, i32>,
    /// Every `set_player_view_distance` the mod made, in order.
    pub calls: Vec<(u128, Option<i32>)>,
    /// The engine cannot reach the players (the hook is a guest's packet hook): every call is
    /// refused.
    pub unreachable: bool,
    pub feedback: Vec<String>,
    logs: RefCell<Vec<String>>,
    dir: PathBuf,
}

impl FakeHost {
    pub fn logged(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }

    /// What the player ends up with: the server's distance, under the limit.
    pub fn effective(&self, uuid: u128) -> Option<i32> {
        self.players.iter().any(|p| p.uuid == uuid).then(|| {
            self.limits
                .get(&uuid)
                .map_or(SERVER_DISTANCE, |limit| (*limit).min(SERVER_DISTANCE))
        })
    }
}

impl Host for FakeHost {
    fn server_players(&self) -> Vec<NativeServerPlayer> {
        self.players.clone()
    }

    fn set_player_view_distance(&mut self, uuid: u128, distance: Option<i32>) -> bool {
        self.calls.push((uuid, distance));
        if self.unreachable || !self.players.iter().any(|p| p.uuid == uuid) {
            return false;
        }
        match distance {
            Some(distance) => {
                assert!((2..=32).contains(&distance), "the engine takes 2 to 32");
                self.limits.insert(uuid, distance);
            }
            None => {
                self.limits.remove(&uuid);
            }
        }
        true
    }

    fn player_view_distance(&self, uuid: u128) -> Option<i32> {
        self.effective(uuid)
    }

    fn player_client(&self, _uuid: u128) -> Option<NativeClientInfo> {
        None
    }

    fn config_dir(&self) -> Result<PathBuf, String> {
        Ok(self.dir.join("config"))
    }

    fn log(&self, level: &str, line: &str) {
        self.logs.borrow_mut().push(format!("{level}: {line}"));
    }

    fn feedback(&mut self, line: Text) {
        self.feedback.push(line.plain());
    }
}

/// The mod boxed as the engine holds it, over a `FakeHost`.
pub struct Rig {
    pub host: FakeHost,
    pub mods: Vec<Box<dyn Hooks>>,
    tick: u64,
    dir: PathBuf,
}

pub const BOB: u128 = 0xb0b;
pub const ALICE: u128 = 0xa11ce;

impl Rig {
    /// A rig whose `config/viewdistance.toml` holds `config`.
    pub fn new(config: &str) -> Self {
        static RIGS: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "viewdistance-rig-{}-{}",
            std::process::id(),
            RIGS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(dir.join("config").join("viewdistance.toml"), config).unwrap();
        Self {
            host: FakeHost {
                players: Vec::new(),
                limits: HashMap::new(),
                calls: Vec::new(),
                unreachable: false,
                feedback: Vec::new(),
                logs: RefCell::new(Vec::new()),
                dir: dir.clone(),
            },
            mods: vec![crate::hooks()],
            tick: 0,
            dir,
        }
    }

    /// A player joins: they are on the server when the mod hears of it.
    pub fn join(&mut self, name: &str, uuid: u128, permission: u8) {
        self.host
            .players
            .push(NativeServerPlayer::new(uuid, name, permission));
        dispatch_player_logged_in(&mut self.host, &self.mods, uuid, name);
    }

    /// A player leaves: the mod hears of it while they are still on.
    pub fn leave(&mut self, uuid: u128) {
        let name = self.player(uuid).name.clone();
        dispatch_player_logged_out(&mut self.host, &self.mods, uuid, &name);
        self.host.players.retain(|p| p.uuid != uuid);
        self.host.limits.remove(&uuid);
    }

    pub fn player(&self, uuid: u128) -> &NativeServerPlayer {
        self.host.players.iter().find(|p| p.uuid == uuid).unwrap()
    }

    /// `/op` and `/deop`.
    pub fn set_permission(&mut self, uuid: u128, level: u8) {
        for p in &mut self.host.players {
            if p.uuid == uuid {
                p.permission = level;
            }
        }
    }

    /// One server tick of a running world: its start, then its end.
    pub fn tick(&mut self) {
        for phase in [
            ServerTickPhase::ServerTickStart {
                world_running: true,
            },
            ServerTickPhase::ServerTickEnd {
                world_running: true,
            },
        ] {
            let context = NativeLifecycleContext::new(
                NativeAuthority::DedicatedServer,
                NativeWorldEpoch::new(1),
                self.tick,
            );
            dispatch_server_tick(&mut self.host, &self.mods, &context, phase);
        }
        self.tick += 1;
    }

    pub fn ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// Types a line at the console. Whether the mod took it.
    pub fn console(&mut self, line: &str) -> bool {
        let mut ran = false;
        dispatch_perform_console_command(&mut self.host, &self.mods, line, &mut ran);
        ran
    }

    /// A player types a line. Whether the mod took it.
    pub fn command_as(&mut self, uuid: u128, line: &str) -> bool {
        let player = self.player(uuid).clone();
        let mut ran = false;
        dispatch_perform_command(&mut self.host, &self.mods, &player, line, &mut ran);
        ran
    }

    /// Writes a new config and has the mod read it.
    pub fn set_config(&mut self, config: &str) {
        std::fs::write(self.dir.join("config").join("viewdistance.toml"), config).unwrap();
        assert!(self.console("viewdistance reload"));
    }

    /// What the mod registers for the players' command trees.
    pub fn registered(&mut self) -> Vec<(String, u8, Option<String>)> {
        let mut commands = Vec::new();
        dispatch_register_commands(&mut self.host, &self.mods, &mut commands);
        commands
            .into_iter()
            .map(|c| (c.usage, c.level, c.permission))
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: &str = "";

    #[test]
    fn a_player_and_an_operator_are_limited_within_the_second_they_join() {
        let mut rig = Rig::new(DEFAULTS);
        rig.join("Bob", BOB, 0);
        rig.join("Alice", ALICE, 4);
        assert_eq!(
            rig.host.calls,
            [(BOB, Some(8)), (ALICE, Some(32))],
            "told at login, before any tick"
        );
        assert_eq!(rig.host.effective(BOB), Some(8), "under the server's 10");
        assert_eq!(rig.host.effective(ALICE), Some(10), "the server's own");
    }

    #[test]
    fn a_limit_is_told_to_the_engine_once_not_every_second() {
        let mut rig = Rig::new(DEFAULTS);
        rig.join("Bob", BOB, 0);
        rig.ticks(200);
        assert_eq!(rig.host.calls, [(BOB, Some(8))]);
    }

    #[test]
    fn a_player_made_operator_is_given_the_operators_limit_within_a_second_and_back_on_deop() {
        let mut rig = Rig::new(DEFAULTS);
        rig.join("Bob", BOB, 0);
        rig.join("Alice", ALICE, 0);
        // The first tick is on a second's boundary: nothing has changed yet.
        rig.tick();
        rig.host.calls.clear();

        rig.set_permission(BOB, 4);
        rig.ticks(19);
        assert!(rig.host.calls.is_empty(), "the check is once a second");
        rig.tick();
        assert_eq!(rig.host.calls, [(BOB, Some(32))], "only Bob changed");
        assert_eq!(rig.host.limits[&ALICE], 8);

        rig.set_permission(BOB, 0);
        rig.ticks(20);
        assert_eq!(rig.host.calls.last(), Some(&(BOB, Some(8))));
        assert_eq!(rig.host.limits[&BOB], 8);
    }

    #[test]
    fn the_level_that_counts_as_an_operator_is_the_configs() {
        let mut rig = Rig::new("op-level = 3\n");
        rig.join("Bob", BOB, 2);
        rig.join("Alice", ALICE, 3);
        assert_eq!(rig.host.limits[&BOB], 8, "a game master is below level 3");
        assert_eq!(rig.host.limits[&ALICE], 32);
    }

    #[test]
    fn a_fake_player_is_not_limited() {
        let mut rig = Rig::new(DEFAULTS);
        rig.host.players.push({
            let mut bot = NativeServerPlayer::new(0xb07, "Bot", 0);
            bot.fake = true;
            bot
        });
        rig.join("Bob", BOB, 0);
        rig.ticks(40);
        assert_eq!(rig.host.calls, [(BOB, Some(8))]);
    }

    #[test]
    fn a_player_who_comes_back_is_limited_again() {
        let mut rig = Rig::new(DEFAULTS);
        rig.join("Bob", BOB, 0);
        rig.leave(BOB);
        rig.ticks(40);
        assert!(!rig.host.limits.contains_key(&BOB));
        rig.join("Bob", BOB, 0);
        assert_eq!(rig.host.calls, [(BOB, Some(8)), (BOB, Some(8))]);
    }

    #[test]
    fn a_player_the_engine_could_not_reach_is_limited_at_the_next_second() {
        let mut rig = Rig::new(DEFAULTS);
        rig.host.unreachable = true;
        rig.join("Bob", BOB, 0);
        assert!(rig.host.limits.is_empty(), "the engine refused");
        rig.ticks(20);
        assert_eq!(rig.host.calls.len(), 2, "tried again");
        rig.host.unreachable = false;
        rig.ticks(21);
        assert_eq!(rig.host.limits[&BOB], 8);
        let told = rig.host.calls.len();
        rig.ticks(100);
        assert_eq!(rig.host.calls.len(), told, "and not again once it took");
    }

    #[test]
    fn a_world_that_is_not_running_or_a_tick_that_has_only_begun_does_nothing() {
        let mut rig = Rig::new(DEFAULTS);
        // Joined where the engine could not take the limit, so only a tick can set it.
        rig.host.unreachable = true;
        rig.join("Bob", BOB, 0);
        rig.host.unreachable = false;
        rig.host.calls.clear();
        let hear = |rig: &mut Rig, phase: ServerTickPhase| {
            let context = NativeLifecycleContext::new(
                NativeAuthority::DedicatedServer,
                NativeWorldEpoch::new(1),
                40,
            );
            dispatch_server_tick(&mut rig.host, &rig.mods, &context, phase);
        };
        hear(
            &mut rig,
            ServerTickPhase::ServerTickStart {
                world_running: true,
            },
        );
        hear(
            &mut rig,
            ServerTickPhase::ServerTickEnd {
                world_running: false,
            },
        );
        assert!(rig.host.calls.is_empty());
        hear(
            &mut rig,
            ServerTickPhase::ServerTickEnd {
                world_running: true,
            },
        );
        assert_eq!(rig.host.calls, [(BOB, Some(8))]);
    }

    #[test]
    fn a_reload_applies_the_new_numbers_at_once_and_only_to_those_that_changed() {
        let mut rig = Rig::new(DEFAULTS);
        rig.join("Bob", BOB, 0);
        rig.join("Alice", ALICE, 4);
        rig.host.calls.clear();
        rig.set_config("non-op-max = 6\nop-max = 32\n");
        assert_eq!(rig.host.calls, [(BOB, Some(6))], "Alice's is the same");
        assert_eq!(
            rig.host.feedback.last().unwrap(),
            "Reloaded the view distance config!"
        );
    }

    #[test]
    fn a_config_that_does_not_read_is_told_and_the_old_one_stays() {
        let mut rig = Rig::new(DEFAULTS);
        rig.join("Bob", BOB, 0);
        rig.host.calls.clear();
        rig.set_config("non-op-max = [");
        assert!(
            rig.host
                .feedback
                .last()
                .unwrap()
                .starts_with("Could not read the config, keeping the old one")
        );
        assert!(rig.host.logged().iter().any(|l| l.starts_with("WARN: ")));
        rig.ticks(40);
        assert!(rig.host.calls.is_empty(), "nothing changed");
        assert_eq!(rig.host.limits[&BOB], 8);
    }

    #[test]
    fn switched_off_every_limit_is_taken_back_and_nothing_is_limited_until_it_is_on_again() {
        let mut rig = Rig::new(DEFAULTS);
        rig.join("Bob", BOB, 0);
        rig.join("Alice", ALICE, 4);
        rig.set_config("enabled = false\n");
        let mut released: Vec<_> = rig.host.calls[2..].to_vec();
        released.sort();
        assert_eq!(released, [(BOB, None), (ALICE, None)]);
        assert!(rig.host.limits.is_empty());
        assert_eq!(rig.host.effective(BOB), Some(10), "the server's own again");

        rig.host.calls.clear();
        rig.ticks(60);
        rig.leave(ALICE);
        rig.join("Alice", ALICE, 4);
        assert!(rig.host.calls.is_empty(), "off: nothing is set or released");

        rig.set_config("enabled = true\n");
        assert_eq!(rig.host.limits.len(), 2);
    }

    #[test]
    fn the_status_lists_every_player_with_their_limit() {
        let mut rig = Rig::new(DEFAULTS);
        rig.join("Bob", BOB, 0);
        rig.join("Alice", ALICE, 4);
        assert!(rig.console("viewdistance status"));
        assert_eq!(
            rig.host.feedback,
            [
                "View distance limits: players 8, operators 32 (operator level 1 and up)",
                "Bob (player): asks ?, near 8, limit 8",
                "Alice (operator): asks ?, near 10, limit 32",
            ]
        );
        // From a player too.
        rig.host.feedback.clear();
        assert!(rig.command_as(ALICE, "/viewdistance status"));
        assert_eq!(rig.host.feedback.len(), 3);

        rig.set_config("enabled = false\n");
        rig.host.feedback.clear();
        assert!(rig.console("viewdistance status"));
        assert!(rig.host.feedback[0].starts_with("View distance limits are off"));
        assert!(rig.host.feedback[2].ends_with("limit none yet"));
    }

    #[test]
    fn the_command_answers_a_wrong_line_with_its_usage_and_leaves_other_lines_alone() {
        let mut rig = Rig::new(DEFAULTS);
        assert!(rig.console("viewdistance dance"));
        assert_eq!(
            rig.host.feedback,
            [
                "Unknown or incomplete command. See below for error",
                "...ance dance<--[Usage: /viewdistance status | reload]"
            ]
        );
        assert!(!rig.console("gamemode creative"));
        assert_eq!(
            rig.registered(),
            [
                (
                    "viewdistance status".to_owned(),
                    2,
                    Some("viewdistance.admin".to_owned())
                ),
                (
                    "viewdistance reload".to_owned(),
                    2,
                    Some("viewdistance.admin".to_owned())
                ),
            ]
        );
    }

    #[test]
    fn the_first_hook_to_run_writes_the_config_a_new_server_has_none_of() {
        let mut rig = Rig::new(DEFAULTS);
        let file = rig.dir.join("config").join("viewdistance.toml");
        std::fs::remove_file(&file).unwrap();
        rig.join("Bob", BOB, 0);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            crate::config::DEFAULT_FILE
        );
        assert_eq!(rig.host.limits[&BOB], 8);
    }
}
