//! A server in memory, and the mod's real hooks driven over it.
//!
//! `FakeHost` is the part of the engine's `Host` the mod uses, with a record of what it was asked
//! to do and the engine's rules for it: a game mode or flying speed only for a player who is on,
//! and a game mode that is the player's already is no change. `Rig` boxes the mod as the engine
//! does and runs its hooks through the engine's own `dispatch_*` functions, so what the tests
//! exercise is what the server runs.

use horizon_client::mod_api::{
    Hooks, Host, NativeAuthority, NativeLifecycleContext, NativeServerPlayer, NativeWorldEpoch,
    ServerTickPhase, Text, dispatch_perform_command, dispatch_perform_console_command,
    dispatch_register_commands, dispatch_server_tick,
};
use std::cell::RefCell;

pub const ALICE: u128 = 0xa11ce;
pub const BOB: u128 = 0xb0b;
pub const EVE: u128 = 0xe7e;

pub struct FakeHost {
    pub players: Vec<NativeServerPlayer>,
    /// What the players were told by the mod, by uuid.
    pub told: Vec<(u128, String)>,
    /// What the command's feedback said, in order.
    pub feedback: Vec<String>,
    /// Every `set_flying_speed` the mod made, in order.
    pub flying: Vec<(u128, Option<f32>)>,
    /// Every `set_game_mode` the mod made, in order.
    pub modes: Vec<(u128, i32)>,
    /// A mod that keeps every game mode as it is.
    pub keep_modes: bool,
    logs: RefCell<Vec<String>>,
}

impl FakeHost {
    pub fn logged(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }

    pub fn told_to(&self, uuid: u128) -> Vec<String> {
        self.told
            .iter()
            .filter(|(to, _)| *to == uuid)
            .map(|(_, line)| line.clone())
            .collect()
    }
}

impl Host for FakeHost {
    fn server_players(&self) -> Vec<NativeServerPlayer> {
        self.players.clone()
    }

    fn set_game_mode(&mut self, player: u128, mode: i32) -> bool {
        self.modes.push((player, mode));
        let keep = self.keep_modes;
        match self.players.iter_mut().find(|p| p.uuid == player) {
            Some(p) if p.game_mode != mode && !keep => {
                p.game_mode = mode;
                true
            }
            _ => false,
        }
    }

    fn set_flying_speed(&mut self, player: u128, speed: Option<f32>) -> bool {
        self.flying.push((player, speed));
        self.players.iter().any(|p| p.uuid == player)
    }

    fn tell(&mut self, uuid: u128, line: Text) -> bool {
        if !self.players.iter().any(|p| p.uuid == uuid) {
            return false;
        }
        self.told.push((uuid, line.plain()));
        true
    }

    fn feedback(&mut self, line: Text) {
        self.feedback.push(line.plain());
    }

    fn log(&self, level: &str, line: &str) {
        self.logs.borrow_mut().push(format!("{level}: {line}"));
    }
}

/// The mod boxed as the engine holds it, over a `FakeHost`.
pub struct Rig {
    pub host: FakeHost,
    pub mods: Vec<Box<dyn Hooks>>,
    tick: u64,
}

impl Rig {
    /// An operator `Alice`, and `Bob` and `Eve`, in survival.
    pub fn new() -> Self {
        let mut rig = Self {
            host: FakeHost {
                players: Vec::new(),
                told: Vec::new(),
                feedback: Vec::new(),
                flying: Vec::new(),
                modes: Vec::new(),
                keep_modes: false,
                logs: RefCell::new(Vec::new()),
            },
            mods: vec![crate::hooks()],
            tick: 0,
        };
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.join("Eve", EVE, 0);
        rig
    }

    pub fn join(&mut self, name: &str, uuid: u128, permission: u8) {
        self.host
            .players
            .push(NativeServerPlayer::new(uuid, name, permission));
    }

    pub fn player(&mut self, uuid: u128) -> &mut NativeServerPlayer {
        self.host
            .players
            .iter_mut()
            .find(|p| p.uuid == uuid)
            .unwrap()
    }

    pub fn mode_of(&self, uuid: u128) -> i32 {
        self.host
            .players
            .iter()
            .find(|p| p.uuid == uuid)
            .unwrap()
            .game_mode
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

    /// A player types a line: what the mod answers at once, without the tick.
    pub fn answer(&mut self, uuid: u128, line: &str) -> Vec<String> {
        self.host.feedback.clear();
        let player = self.player(uuid).clone();
        let mut ran = false;
        dispatch_perform_command(&mut self.host, &self.mods, &player, line, &mut ran);
        assert!(ran, "{line} is not the mod's");
        self.host.feedback.clone()
    }

    /// A player types a line, and the tick that does it comes: what it answered at once.
    pub fn run(&mut self, uuid: u128, line: &str) -> Vec<String> {
        let answered = self.answer(uuid, line);
        self.tick();
        answered
    }

    /// The console types a line: what the mod answers at once.
    pub fn console(&mut self, line: &str) -> Vec<String> {
        self.host.feedback.clear();
        let mut ran = false;
        dispatch_perform_console_command(&mut self.host, &self.mods, line, &mut ran);
        assert!(ran, "{line} is not the mod's");
        let answered = self.host.feedback.clone();
        self.tick();
        answered
    }

    /// The console types a line, with no tick after it.
    pub fn console_without_tick(&mut self, line: &str) {
        let mut ran = false;
        dispatch_perform_console_command(&mut self.host, &self.mods, line, &mut ran);
        assert!(ran);
    }

    /// Whether the mod takes the line (a player's).
    pub fn takes(&mut self, uuid: u128, line: &str) -> bool {
        let player = self.player(uuid).clone();
        let mut ran = false;
        dispatch_perform_command(&mut self.host, &self.mods, &player, line, &mut ran);
        ran
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What a refused command answers: the header, then the command as far as the word at fault
    /// (its last ten characters, after dots, when it is longer) and why, in brackets.
    fn refused(context: &str, why: &str) -> [String; 2] {
        [
            "Unknown or incomplete command. See below for error".to_owned(),
            format!("{context}<--[{why}]"),
        ]
    }

    // ---- game modes ----------------------------------------------------------------------

    #[test]
    fn each_shortcut_puts_the_player_named_in_its_mode_at_the_tick() {
        let mut rig = Rig::new();
        for (command, mode, name) in [
            ("gmc", 1, "creative"),
            ("gma", 2, "adventure"),
            ("gmsp", 3, "spectator"),
            ("gms", 0, "survival"),
        ] {
            let line = format!("{command} Bob");
            // Bob is moved off the mode first, so each is a change.
            rig.player(BOB).game_mode = if mode == 1 { 0 } else { 1 };
            rig.host.modes.clear();
            assert_eq!(
                rig.answer(ALICE, &line),
                [format!("Set Bob's game mode to {name}!")]
            );
            assert!(rig.host.modes.is_empty(), "nothing is done in the command");
            rig.tick();
            assert_eq!(rig.host.modes, [(BOB, mode)], "{command}");
            assert_eq!(rig.mode_of(BOB), mode);
        }
        assert_eq!(rig.mode_of(ALICE), 0, "only Bob");
        assert_eq!(rig.mode_of(EVE), 0);
    }

    #[test]
    fn with_no_player_it_is_for_the_one_who_ran_it_and_the_others_are_untouched() {
        let mut rig = Rig::new();
        assert_eq!(rig.run(ALICE, "/gmc"), ["Set your game mode to creative!"]);
        assert_eq!(rig.mode_of(ALICE), 1);
        assert_eq!(rig.mode_of(BOB), 0);
        assert!(
            rig.host.told_to(ALICE).is_empty(),
            "no second line for one's own change"
        );
        assert!(rig.host.told.is_empty());
        assert!(
            rig.host
                .logged()
                .contains(&"INFO: Essentials: Alice set Alice's game mode to creative".to_owned())
        );
    }

    #[test]
    fn a_player_whose_mode_another_changes_is_told_with_vanillas_line() {
        let mut rig = Rig::new();
        rig.run(ALICE, "gmsp bob");
        let told = rig.host.told_to(BOB);
        assert_eq!(told.len(), 1);
        // Vanilla's own line, `gameMode.changed` with the mode's name, which the client words.
        assert_eq!(told, ["gameMode.changed"]);
        assert!(rig.host.told_to(ALICE).is_empty());
    }

    #[test]
    fn the_player_is_found_by_the_name_they_logged_in_with_or_by_their_nickname() {
        let mut rig = Rig::new();
        rig.player(EVE).nickname = Some("Captain".into());
        assert_eq!(
            rig.run(ALICE, "gmc captain"),
            ["Set Eve's game mode to creative!"]
        );
        assert_eq!(rig.mode_of(EVE), 1);
        rig.run(ALICE, "gms EVE");
        assert_eq!(rig.mode_of(EVE), 0);
    }

    #[test]
    fn a_player_not_on_the_server_is_an_error_and_so_is_no_player_from_the_console() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.answer(ALICE, "gmc Zed"),
            refused("gmc Zed", "Player was not found")
        );
        assert_eq!(
            rig.answer(ALICE, "gmsp Zed"),
            refused("gmsp Zed", "Player was not found")
        );
        assert_eq!(
            rig.console("gma"),
            refused("gma", "The console is no player: name one")
        );
        assert!(rig.host.modes.is_empty());
        // The console can name one.
        assert_eq!(
            rig.console("gma Bob"),
            ["Set Bob's game mode to adventure!"]
        );
        assert_eq!(rig.mode_of(BOB), 2);
        assert!(
            rig.host
                .logged()
                .contains(&"INFO: Essentials: console set Bob's game mode to adventure".to_owned())
        );
    }

    #[test]
    fn a_player_already_in_the_mode_is_told_so_and_nothing_is_queued() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.answer(ALICE, "gms Bob"),
            ["Bob is already in survival mode"]
        );
        assert_eq!(
            rig.answer(ALICE, "gms"),
            ["You are already in survival mode"]
        );
        rig.tick();
        assert!(rig.host.modes.is_empty());
    }

    #[test]
    fn a_change_that_fails_at_the_tick_is_told_to_the_one_who_asked() {
        let mut rig = Rig::new();
        // A mod keeps the mode.
        rig.host.keep_modes = true;
        rig.run(ALICE, "gmc Bob");
        assert_eq!(rig.mode_of(BOB), 0);
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Bob's game mode was not changed: they left, or a mod kept it"]
        );
        assert!(rig.host.told_to(BOB).is_empty(), "Bob is told nothing");
        // The player leaves before the tick: the console's report goes to the log.
        rig.host.keep_modes = false;
        rig.host.told.clear();
        rig.console_without_tick("gmc Eve");
        rig.host.players.retain(|p| p.uuid != EVE);
        rig.tick();
        assert!(rig.host.logged().contains(
            &"INFO: Eve's game mode was not changed: they left, or a mod kept it".to_owned()
        ));
    }

    // ---- selectors -----------------------------------------------------------------------

    const DAN: u128 = 0xda4;

    fn crowd() -> Rig {
        let mut rig = Rig::new();
        rig.join("Dan", DAN, 0);
        rig
    }

    #[test]
    fn at_a_changes_everyone_and_says_how_many() {
        let mut rig = crowd();
        assert_eq!(
            rig.answer(ALICE, "gmc @a"),
            ["Set 4 players gamemode to creative!"]
        );
        assert!(rig.host.modes.is_empty(), "nothing is done in the command");
        rig.tick();
        for who in [ALICE, BOB, EVE, DAN] {
            assert_eq!(rig.mode_of(who), 1);
        }
        // The operator who ran it is not told about their own change; the others are.
        assert!(rig.host.told_to(ALICE).is_empty());
        for who in [BOB, EVE, DAN] {
            assert_eq!(rig.host.told_to(who), ["gameMode.changed"]);
        }
    }

    #[test]
    fn those_already_in_the_mode_are_left_out_of_the_count() {
        let mut rig = crowd();
        rig.player(BOB).game_mode = 3;
        rig.player(EVE).game_mode = 3;
        assert_eq!(
            rig.run(ALICE, "gmsp @a"),
            ["Set 2 players gamemode to spectator!"]
        );
        assert_eq!(
            rig.host.modes.len(),
            2,
            "Bob and Eve were left as they were"
        );
        // One left to change is said by name, as a single player is.
        let mut rig = Rig::new();
        rig.player(BOB).game_mode = 1;
        rig.player(EVE).game_mode = 1;
        assert_eq!(
            rig.run(ALICE, "gmc @a"),
            ["Set your game mode to creative!"]
        );
        // None left to change.
        assert_eq!(
            rig.answer(ALICE, "gmc @a"),
            ["All 3 players are already in creative mode"]
        );
    }

    #[test]
    fn at_s_is_whoever_ran_it_and_not_the_console() {
        let mut rig = Rig::new();
        assert_eq!(rig.run(BOB, "gma @s"), ["Set your game mode to adventure!"]);
        assert_eq!(rig.mode_of(BOB), 2);
        assert_eq!(rig.mode_of(ALICE), 0);
        assert_eq!(
            rig.console("gma @s"),
            refused("gma @s", "The console is no player: name one")
        );
    }

    #[test]
    fn at_r_is_one_player_drawn_from_everyone() {
        let mut rig = crowd();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..40 {
            for who in [ALICE, BOB, EVE, DAN] {
                rig.player(who).game_mode = 0;
            }
            let said = rig.run(ALICE, "gmc @r");
            let changed: Vec<u128> = [ALICE, BOB, EVE, DAN]
                .into_iter()
                .filter(|who| rig.mode_of(*who) == 1)
                .collect();
            assert_eq!(changed.len(), 1, "one player: {said:?}");
            seen.insert(changed[0]);
        }
        assert!(seen.len() > 1, "not always the same one: {seen:?}");
        // The console can draw one too, there being players.
        for who in [ALICE, BOB, EVE, DAN] {
            rig.player(who).game_mode = 1;
        }
        assert!(rig.console("gms @r")[0].starts_with("Set "));
    }

    #[test]
    fn at_p_is_the_nearest_player_in_the_same_level_to_whoever_ran_it() {
        let mut rig = crowd();
        rig.player(ALICE).position = [0.0, 64.0, 0.0];
        rig.player(BOB).position = [10.0, 64.0, 0.0];
        rig.player(EVE).position = [3.0, 64.0, 0.0];
        rig.player(DAN).position = [1.0, 64.0, 1.0];
        rig.player(DAN).dimension = "minecraft:the_nether".into();
        // A player is the nearest to themselves, as in vanilla.
        assert_eq!(
            rig.run(ALICE, "gmc @p"),
            ["Set your game mode to creative!"]
        );
        assert_eq!(rig.mode_of(ALICE), 1);
        // From Bob, Eve is nearer than Alice; Dan is in another level.
        rig.player(BOB).game_mode = 1;
        let mut far = Rig::new();
        far.join("Dan", DAN, 0);
        far.player(ALICE).position = [100.0, 64.0, 0.0];
        far.player(BOB).position = [0.0, 64.0, 0.0];
        far.player(EVE).position = [3.0, 64.0, 0.0];
        far.player(DAN).position = [1.0, 64.0, 0.0];
        far.player(DAN).dimension = "minecraft:the_end".into();
        assert_eq!(far.run(BOB, "gmc @p"), ["Set your game mode to creative!"]);
        assert_eq!(far.mode_of(BOB), 1, "Bob is the nearest to Bob");
        assert_eq!(
            far.console("gmc @p"),
            refused("gmc @p", "The console is no player: name one")
        );
    }

    #[test]
    fn a_selector_that_cannot_be_read_says_what_is_supported() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.answer(ALICE, "gmc @e"),
            refused("gmc @e", "Only @a, @p, @r and @s are supported here")
        );
        assert_eq!(
            rig.answer(ALICE, "gmc @a[distance=..5]"),
            refused(
                "...tance=..5]",
                "Selector options are not supported here: use @a, @p, @r, @s, --non-ops or a name"
            )
        );
        rig.tick();
        assert!(rig.host.modes.is_empty());
    }

    #[test]
    fn non_ops_is_every_player_who_is_no_operator() {
        let mut rig = crowd();
        rig.player(DAN).permission = 2;
        assert_eq!(
            rig.run(ALICE, "gmc --non-ops"),
            ["Set 2 players gamemode to creative!"]
        );
        assert_eq!(rig.mode_of(BOB), 1);
        assert_eq!(rig.mode_of(EVE), 1);
        assert_eq!(rig.mode_of(ALICE), 0, "an operator");
        assert_eq!(rig.mode_of(DAN), 0, "an operator of level 2");
        assert_eq!(
            rig.console("gms --NON-OPS"),
            ["Set 2 players gamemode to survival!"]
        );
        assert_eq!(rig.mode_of(BOB), 0);
        // With no one to change.
        let mut ops = Rig::new();
        ops.host.players.retain(|p| p.uuid == ALICE);
        assert_eq!(
            ops.answer(ALICE, "gmc --non-ops"),
            refused(
                "...--non-ops",
                "No player was found: everyone online is an operator"
            )
        );
    }

    // ---- vanilla's /gamemode -------------------------------------------------------------

    #[test]
    fn gamemode_is_the_shortcut_for_its_mode_with_the_same_answers() {
        let mut rig = crowd();
        assert_eq!(
            rig.run(ALICE, "gamemode creative Bob"),
            ["Set Bob's game mode to creative!"]
        );
        assert_eq!(rig.mode_of(BOB), 1);
        assert_eq!(
            rig.run(ALICE, "/gamemode spectator"),
            ["Set your game mode to spectator!"]
        );
        assert_eq!(rig.mode_of(ALICE), 3);
        assert_eq!(
            rig.run(ALICE, "gamemode adventure @a"),
            ["Set 4 players gamemode to adventure!"]
        );
        assert_eq!(
            rig.run(ALICE, "gamemode survival --non-ops"),
            ["Set 3 players gamemode to survival!"]
        );
        assert_eq!(rig.mode_of(BOB), 0);
        assert_eq!(rig.mode_of(ALICE), 2, "an operator");
        assert_eq!(
            rig.console("gamemode creative Eve"),
            ["Set Eve's game mode to creative!"]
        );
    }

    #[test]
    fn a_gamemode_line_the_mod_cannot_read_is_told_and_not_left_to_vanilla() {
        let mut rig = Rig::new();
        let usage = "Usage: /gamemode <survival|creative|adventure|spectator> [targets]";
        assert_eq!(rig.answer(ALICE, "gamemode"), refused("gamemode", usage));
        assert_eq!(
            rig.answer(ALICE, "gamemode flying"),
            refused("...ode flying", usage)
        );
        assert_eq!(
            rig.answer(ALICE, "gamemode creative Zed"),
            refused("...eative Zed", "Player was not found")
        );
        assert_eq!(
            rig.answer(ALICE, "gamemode creative @e"),
            refused("...reative @e", "Only @a, @p, @r and @s are supported here")
        );
        rig.tick();
        assert!(rig.host.modes.is_empty());
    }

    // ---- flying speed --------------------------------------------------------------------

    fn level_of(speed: f32) -> u32 {
        // The inverse of the mapping, from independent numbers: level 1 is vanilla's 0.05 and
        // level 10 is 0.45, in nine equal steps.
        (((speed - 0.05) / (0.40 / 9.0)).round() as u32) + 1
    }

    #[test]
    fn a_speed_from_one_to_ten_sets_the_flying_speed_at_the_tick_with_one_being_vanillas() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.answer(ALICE, "flyspeed 5"),
            ["Set your flight speed to 5!"]
        );
        assert!(rig.host.flying.is_empty(), "nothing is done in the command");
        rig.tick();
        let [(who, Some(speed))] = rig.host.flying[..] else {
            panic!("{:?}", rig.host.flying);
        };
        assert_eq!(who, ALICE);
        assert_eq!(level_of(speed), 5);
        assert!((speed - (0.05 + 4.0 * 0.40 / 9.0)).abs() < 1e-5, "{speed}");

        rig.host.flying.clear();
        rig.run(ALICE, "flyspeed 1");
        assert!(
            (rig.host.flying[0].1.unwrap() - 0.05).abs() < 1e-6,
            "vanilla's"
        );
        rig.run(ALICE, "flyspeed 10");
        assert!(
            (rig.host.flying[1].1.unwrap() - 0.45).abs() < 1e-6,
            "the most"
        );
        assert!(
            rig.host
                .logged()
                .contains(&"INFO: Essentials: Alice set Alice's flight speed to 10".to_owned())
        );
    }

    #[test]
    fn a_speed_above_ten_is_ten_and_a_decimal_goes_to_the_nearest_whole_speed() {
        let mut rig = Rig::new();
        for (typed, level) in [
            ("10", 10),
            ("10.4", 10),
            ("11", 10),
            ("99999", 10),
            ("1.4", 1),
            ("1.5", 2),
            ("2.5", 3),
            ("5.49", 5),
            ("5.51", 6),
            ("9.6", 10),
            ("1e1", 10),
        ] {
            rig.host.flying.clear();
            let said = rig.run(ALICE, &format!("flyspeed {typed}"));
            assert_eq!(
                said,
                [format!("Set your flight speed to {level}!")],
                "{typed}"
            );
            assert_eq!(level_of(rig.host.flying[0].1.unwrap()), level, "{typed}");
        }
    }

    #[test]
    fn a_speed_below_one_or_no_number_is_an_error_and_changes_nothing() {
        let mut rig = Rig::new();
        for (line, context, why) in [
            (
                "flyspeed 0.5",
                "...yspeed 0.5",
                "Flight speed must be at least 1",
            ),
            (
                "flyspeed 0.99",
                "...speed 0.99",
                "Flight speed must be at least 1",
            ),
            (
                "flyspeed 0",
                "flyspeed 0",
                "Flight speed must be at least 1",
            ),
            (
                "flyspeed -3",
                "...lyspeed -3",
                "Flight speed must be at least 1",
            ),
            (
                "flyspeed abc",
                "...yspeed abc",
                "Flight speed must be a number from 1 to 10",
            ),
            (
                "flyspeed NaN",
                "...yspeed NaN",
                "Flight speed must be a number from 1 to 10",
            ),
            (
                "flyspeed inf",
                "...yspeed inf",
                "Flight speed must be a number from 1 to 10",
            ),
        ] {
            assert_eq!(rig.answer(ALICE, line), refused(context, why), "{line}");
        }
        rig.tick();
        assert!(rig.host.flying.is_empty());
    }

    #[test]
    fn flyspeed_needs_a_speed_and_a_player_to_fly() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.answer(ALICE, "flyspeed"),
            refused("flyspeed", "Usage: /flyspeed <1-10>")
        );
        assert_eq!(
            rig.console("flyspeed 5"),
            refused("flyspeed 5", "The console is no player: name one")
        );
        assert!(rig.host.flying.is_empty());
    }

    // ---- who may run them ----------------------------------------------------------------

    #[test]
    fn every_command_is_registered_for_operators_of_level_two_with_a_permission_node() {
        let mut rig = Rig::new();
        let commands = rig.registered();
        assert_eq!(commands.len(), 17);
        assert!(
            commands
                .iter()
                .all(|(_, level, node)| *level == 2 && node.is_some())
        );
        let usages: Vec<&str> = commands.iter().map(|(u, _, _)| u.as_str()).collect();
        for usage in [
            "gmc [targets:players]",
            "gms [targets:players]",
            "gmsp [targets:players]",
            "gma [targets:players]",
            "gmc --non-ops",
            "gamemode creative [targets:players]",
            "gamemode spectator --non-ops",
            "flyspeed <speed:number>",
        ] {
            assert!(usages.contains(&usage), "{usage}");
        }
    }

    #[test]
    fn a_line_of_another_command_is_left_alone() {
        let mut rig = Rig::new();
        assert!(!rig.takes(ALICE, "gamerule keepInventory true"));
        assert!(!rig.takes(ALICE, "gmcx"));
        assert!(rig.takes(ALICE, "gmc"));
    }

    #[test]
    fn a_line_said_wrongly_gets_its_usage() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.answer(ALICE, "gmc Bob Eve"),
            refused("...mc Bob Eve", "Usage: /<gmc|gms|gmsp|gma> [targets]")
        );
    }
}
