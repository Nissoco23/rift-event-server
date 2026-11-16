//! A server in memory, and the mod's real hooks driven over it.
//!
//! `FakeHost` is the part of the engine's `Host` the mod uses, with a record of what it was asked to
//! do. `Rig` boxes the mod as the engine does and runs its hooks through the engine's own
//! `dispatch_*` functions, so what the tests exercise is what the server runs.

use horizon_client::mod_api::{
    Hooks, Host, NativeCommand, NativeServerPlayer, Suggestion, Text, dispatch_level_tick_post,
    dispatch_living_damage_pre_v2, dispatch_perform_command, dispatch_perform_console_command,
    dispatch_player_death_message, dispatch_player_died_v2, dispatch_player_logged_out,
    dispatch_player_respawn_position, dispatch_player_respawned, dispatch_register_commands,
    dispatch_tab_complete_v2,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const OVERWORLD: &str = "minecraft:overworld";
pub const NETHER: &str = "minecraft:the_nether";
pub const BOB: u128 = 0xb0b;
pub const ALICE: u128 = 0xa11ce;
pub const OP: u128 = 0x0b;

pub struct FakeHost {
    pub players: Vec<NativeServerPlayer>,
    /// What the commands answered, as plain text.
    pub feedback: Vec<String>,
    /// The same, as the engine would draw them.
    pub styled: Vec<Text>,
    /// Every kick: the player and the reason, as plain text.
    pub kicks: Vec<(u128, String)>,
    /// Every game mode a player was put in.
    pub game_modes: Vec<(u128, i32)>,
    /// Every time the join and leave lines were switched.
    pub lines: Vec<bool>,
    /// The engine cannot reach the hub: the join and leave lines are refused.
    pub hub_away: bool,
    logs: RefCell<Vec<String>>,
    dir: PathBuf,
}

impl FakeHost {
    pub fn logged(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }
}

impl Host for FakeHost {
    fn server_players(&self) -> Vec<NativeServerPlayer> {
        self.players.clone()
    }

    fn kick(&mut self, player: u128, reason: &Text) -> bool {
        self.kicks.push((player, reason.plain()));
        true
    }

    fn set_game_mode(&mut self, player: u128, mode: i32) -> bool {
        self.game_modes.push((player, mode));
        true
    }

    fn set_membership_lines(&mut self, show: bool) -> bool {
        if self.hub_away {
            return false;
        }
        self.lines.push(show);
        true
    }

    fn config_dir(&self) -> Result<PathBuf, String> {
        Ok(self.dir.join("config"))
    }

    fn log(&self, level: &str, line: &str) {
        self.logs.borrow_mut().push(format!("{level}: {line}"));
    }

    fn feedback(&mut self, line: Text) {
        self.feedback.push(line.plain());
        self.styled.push(line);
    }
}

/// The mod boxed as the engine holds it, over a `FakeHost`.
pub struct Rig {
    pub host: FakeHost,
    pub mods: Vec<Box<dyn Hooks>>,
    dir: PathBuf,
    next_entity: i32,
}

impl Rig {
    pub fn new() -> Self {
        static RIGS: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hardcore-rig-{}-{}",
            std::process::id(),
            RIGS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        Self::over(dir)
    }

    fn over(dir: PathBuf) -> Self {
        Self {
            host: FakeHost {
                players: Vec::new(),
                feedback: Vec::new(),
                styled: Vec::new(),
                kicks: Vec::new(),
                game_modes: Vec::new(),
                lines: Vec::new(),
                hub_away: false,
                logs: RefCell::new(Vec::new()),
                dir: dir.clone(),
            },
            mods: vec![crate::hooks()],
            dir,
            next_entity: 100,
        }
    }

    /// The server stopped and started again over the same folder: the mod is new, the players
    /// are those who join next.
    pub fn restart(self) -> Self {
        Self::over(self.dir)
    }

    /// The file the mod saves, as it is now.
    pub fn file(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("config").join(crate::store::FILE_NAME)).ok()
    }

    pub fn write_file(&self, text: &str) {
        std::fs::write(self.dir.join("config").join(crate::store::FILE_NAME), text).unwrap();
    }

    pub fn file_exists(&self, name: &str) -> bool {
        self.dir.join("config").join(name).exists()
    }

    /// A player is on the server.
    pub fn join(&mut self, name: &str, uuid: u128, permission: u8) {
        self.next_entity += 1;
        let mut player = NativeServerPlayer::new(uuid, name, permission);
        player.entity_id = self.next_entity;
        self.host.players.push(player);
    }

    pub fn player_mut(&mut self, uuid: u128) -> &mut NativeServerPlayer {
        self.host
            .players
            .iter_mut()
            .find(|p| p.uuid == uuid)
            .unwrap()
    }

    pub fn leave(&mut self, uuid: u128) {
        let name = self.player_mut(uuid).name.clone();
        dispatch_player_logged_out(&mut self.host, &self.mods, uuid, &name);
        self.host.players.retain(|p| p.uuid != uuid);
    }

    /// The engine asks for the commands, once, when a world opens.
    pub fn register(&mut self) -> Vec<NativeCommand> {
        let mut commands = Vec::new();
        dispatch_register_commands(&mut self.host, &self.mods, &mut commands);
        commands
    }

    /// A tick of the overworld.
    pub fn tick(&mut self) {
        dispatch_level_tick_post(&mut self.host, &self.mods, OVERWORLD);
    }

    /// A player types a line: what the mod answered, none when it was not for the mod.
    pub fn say(&mut self, uuid: u128, line: &str) -> Option<Vec<String>> {
        let player = self.host.players.iter().find(|p| p.uuid == uuid).cloned()?;
        self.host.feedback.clear();
        self.host.styled.clear();
        let mut ran = false;
        dispatch_perform_command(&mut self.host, &self.mods, &player, line, &mut ran);
        ran.then(|| std::mem::take(&mut self.host.feedback))
    }

    pub fn console(&mut self, line: &str) -> Option<Vec<String>> {
        self.host.feedback.clear();
        let mut ran = false;
        dispatch_perform_console_command(&mut self.host, &self.mods, line, &mut ran);
        ran.then(|| std::mem::take(&mut self.host.feedback))
    }

    /// What a hit takes off, after the mod: `amount` of `damage_type` on a player at `health`.
    pub fn hit(&self, uuid: u128, health: f32, amount: f32, damage_type: &str, totem: bool) -> f32 {
        let mut amount = amount;
        dispatch_living_damage_pre_v2(
            &self.mods,
            100,
            Some(uuid),
            "minecraft:player",
            damage_type,
            0,
            0,
            health,
            totem,
            &mut amount,
        );
        amount
    }

    /// A player dies of `message`.
    pub fn die(&mut self, uuid: u128, message: &str) {
        let name = self
            .host
            .players
            .iter()
            .find(|p| p.uuid == uuid)
            .unwrap()
            .name
            .clone();
        dispatch_player_died_v2(
            &mut self.host,
            &self.mods,
            uuid,
            &name,
            &Text::literal(message),
        );
    }

    /// What a death tells the others: the line, and the distance it keeps to.
    pub fn death_line(&mut self, uuid: u128) -> (Option<Text>, Option<f64>) {
        let name = self
            .host
            .players
            .iter()
            .find(|p| p.uuid == uuid)
            .unwrap()
            .name
            .clone();
        let mut line = Some(Text::literal(format!("{name} was slain")));
        let mut radius = None;
        dispatch_player_death_message(
            &mut self.host,
            &self.mods,
            uuid,
            &name,
            &mut line,
            &mut radius,
        );
        (line, radius)
    }

    /// A dead player respawns: where the mod sends them from `spawn`, then the respawn.
    pub fn respawn(&mut self, uuid: u128, spawn: (&str, [f64; 3])) -> (String, [f64; 3]) {
        let name = self
            .host
            .players
            .iter()
            .find(|p| p.uuid == uuid)
            .unwrap()
            .name
            .clone();
        let (mut dimension, mut pos) = (spawn.0.to_owned(), spawn.1);
        let (mut yaw, mut pitch) = (0.0, 0.0);
        dispatch_player_respawn_position(
            &mut self.host,
            &self.mods,
            uuid,
            &name,
            &mut dimension,
            &mut pos,
            &mut yaw,
            &mut pitch,
        );
        dispatch_player_respawned(&mut self.host, &self.mods, uuid, &name);
        (dimension, pos)
    }

    /// What a player completes when `line` is typed up to the cursor.
    /// The engine's own suggestions (the real names, for a word that completes a player) go in first.
    pub fn complete(&mut self, uuid: u128, line: &str) -> Vec<String> {
        let name = self
            .host
            .players
            .iter()
            .find(|p| p.uuid == uuid)
            .unwrap()
            .name
            .clone();
        let mut words: Vec<String> = line.trim_start().split(' ').map(str::to_owned).collect();
        let label = words.remove(0);
        let typing_at = line.rfind(' ').map_or(0, |i| i + 1);
        let range = (typing_at, line.len());
        let mut suggestions: Vec<Suggestion> = self
            .host
            .players
            .iter()
            .map(|p| Suggestion {
                range,
                text: p.name.clone(),
                tooltip: None,
                int_value: None,
            })
            .collect();
        dispatch_tab_complete_v2(
            &mut self.host,
            &self.mods,
            uuid,
            &name,
            &label,
            &words,
            [range.0, range.1],
            &mut suggestions,
        );
        suggestions.into_iter().map(|s| s.text).collect()
    }
}

#[cfg(test)]
mod scenarios {
    use super::*;
    use crate::command::hearts;

    /// An operator, and two players, with a tick seen.
    fn rig() -> Rig {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        r.join("Bob", BOB, 0);
        r.join("Alice", ALICE, 0);
        r.tick();
        r
    }

    fn said(lines: Option<Vec<String>>) -> Vec<String> {
        lines.expect("the mod took the command")
    }

    /// What a refused command says as its reason: the second line's text in `<--[` and `]`.
    fn reason(lines: &[String]) -> String {
        assert_eq!(lines.len(), 2, "an error is two lines: {lines:?}");
        assert_eq!(
            lines[0],
            "Unknown or incomplete command. See below for error"
        );
        let (_, rest) = lines[1]
            .split_once("<--[")
            .expect("the context and the reason");
        rest.strip_suffix(']')
            .expect("the reason ends the line")
            .to_owned()
    }

    // ---- /deathprotect ---------------------------------------------------------------------

    #[test]
    fn switching_your_own_protection_says_what_it_is_and_the_floor() {
        let mut r = rig();
        assert_eq!(
            said(r.say(OP, "deathprotect")),
            [
                "Your death protection has been enabled",
                "Minimum health: 0.5 hearts"
            ]
        );
        assert_eq!(
            said(r.say(OP, "/deathprotect")),
            ["Your death protection has been disabled"]
        );
    }

    #[test]
    fn switching_another_players_names_them() {
        let mut r = rig();
        assert_eq!(
            said(r.say(OP, "deathprotect bob")),
            [
                "Bob death protection has been enabled",
                "Minimum health: 0.5 hearts"
            ]
        );
        // The name is typed in any case.
        assert_eq!(
            said(r.say(OP, "deathprotect BOB")),
            ["Bob death protection has been disabled"]
        );
    }

    #[test]
    fn a_nicked_player_is_found_by_either_name_and_shown_by_the_nickname() {
        let mut r = rig();
        r.player_mut(BOB).nickname = Some("Captain".to_owned());
        assert_eq!(
            said(r.say(OP, "deathprotect captain")),
            [
                "Captain death protection has been enabled",
                "Minimum health: 0.5 hearts"
            ]
        );
        assert_eq!(
            said(r.say(OP, "deathprotect bob")),
            ["Captain death protection has been disabled"]
        );
    }

    #[test]
    fn a_nickname_that_is_another_players_name_means_the_one_wearing_it() {
        let mut r = rig();
        r.player_mut(ALICE).nickname = Some("Bob".to_owned());
        said(r.say(OP, "deathprotect bob"));
        assert_eq!(r.hit(ALICE, 20.0, 50.0, "minecraft:generic", false), 19.0);
        assert_eq!(r.hit(BOB, 20.0, 50.0, "minecraft:generic", false), 50.0);
    }

    #[test]
    fn an_unknown_player_is_an_error() {
        let mut r = rig();
        let lines = said(r.say(OP, "deathprotect nobody"));
        assert_eq!(reason(&lines), "Player was not found");
        assert!(lines[1].contains("nobody<--["), "{lines:?}");
        let lines = said(r.say(OP, "deathprotect on nobody"));
        assert_eq!(reason(&lines), "Player was not found");
        let lines = said(r.say(OP, "deathprotect level 4 nobody"));
        assert_eq!(reason(&lines), "Player was not found");
    }

    #[test]
    fn on_and_off_set_it_where_the_toggle_would_flip_it() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on bob"));
        // On again stays on, where the toggle would have turned it off, and says so.
        assert_eq!(
            said(r.say(OP, "deathprotect on bob")),
            ["Bob death protection is already enabled"]
        );
        assert_eq!(r.hit(BOB, 20.0, 50.0, "minecraft:generic", false), 19.0);
        assert_eq!(
            said(r.say(OP, "deathprotect off bob")),
            ["Bob death protection has been disabled"]
        );
        assert_eq!(
            said(r.say(OP, "deathprotect off bob")),
            ["Bob death protection is already disabled"]
        );
        assert_eq!(r.hit(BOB, 20.0, 50.0, "minecraft:generic", false), 50.0);
        // With no one named, it is for the one who typed it.
        assert_eq!(
            said(r.say(BOB, "deathprotect on")),
            [
                "Your death protection has been enabled",
                "Minimum health: 0.5 hearts"
            ]
        );
    }

    #[test]
    fn many_players_are_counted() {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        for i in 0..28 {
            r.join(&format!("P{i}"), 0x1000 + i, 0);
        }
        r.tick();
        assert_eq!(
            said(r.say(OP, "deathprotect on @a")),
            ["Death protection has been enabled for 29 players"]
        );
        for uuid in [OP, 0x1000, 0x101b] {
            assert_eq!(r.hit(uuid, 20.0, 99.0, "minecraft:generic", false), 19.0);
        }
        assert_eq!(
            said(r.say(OP, "deathprotect off @a")),
            ["Death protection has been disabled for 29 players"]
        );
        assert_eq!(r.hit(0x1000, 20.0, 99.0, "minecraft:generic", false), 99.0);
    }

    #[test]
    fn a_player_already_in_the_state_is_told_so() {
        let mut r = rig();
        // Nobody has it on: off says so, and so for yourself.
        assert_eq!(
            said(r.say(OP, "deathprotect off")),
            ["Your death protection is already disabled"]
        );
        said(r.say(OP, "deathprotect on"));
        assert_eq!(
            said(r.say(OP, "deathprotect on")),
            ["Your death protection is already enabled"]
        );
        assert_eq!(
            said(r.say(OP, "deathprotect on @a")),
            [
                "Death protection has been enabled for 2 players",
                "1 already had it enabled"
            ]
        );
        assert_eq!(
            said(r.say(OP, "deathprotect on @a")),
            ["Death protection is already enabled for 3 players"]
        );
        assert_eq!(
            said(r.say(OP, "deathprotect off @a")),
            ["Death protection has been disabled for 3 players"]
        );
        assert_eq!(
            said(r.say(OP, "deathprotect off @a")),
            ["Death protection is already disabled for 3 players"]
        );
    }

    #[test]
    fn saying_it_is_already_set_is_all_gray() {
        let mut r = rig();
        for line in [
            "deathprotect off",
            "deathprotect off bob",
            "deathprotect off @a",
        ] {
            said(r.say(OP, line));
            let text = r.host.styled.last().expect("a line");
            assert!(text.plain().contains("already"), "{text:?}");
            assert!(
                text.spans
                    .iter()
                    .all(|span| span.style.color == Some(rift_chat::GRAY)),
                "{line}: {text:?}"
            );
        }
    }

    #[test]
    fn the_minimum_health_is_told_in_gray() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on bob"));
        let line = r.host.styled.last().expect("a second line");
        assert_eq!(line.plain(), "Minimum health: 0.5 hearts");
        assert!(
            line.spans
                .iter()
                .all(|span| span.style.color == Some(rift_chat::GRAY)),
            "{line:?}"
        );
    }

    #[test]
    fn a_selector_that_finds_one_player_answers_as_for_one() {
        let mut r = rig();
        assert_eq!(
            said(r.say(OP, "deathprotect on @s")),
            [
                "Your death protection has been enabled",
                "Minimum health: 0.5 hearts"
            ]
        );
        // The nearest player to the operator is the operator, as in vanilla.
        assert_eq!(
            said(r.say(OP, "deathprotect off @p")),
            ["Your death protection has been disabled"]
        );
        // Out of three, one is picked, and the answer names them.
        let lines = said(r.say(OP, "deathprotect on @r"));
        assert!(
            lines[0].ends_with("death protection has been enabled"),
            "{lines:?}"
        );
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn the_toggle_takes_one_player_only() {
        let mut r = rig();
        let lines = said(r.say(OP, "deathprotect @a"));
        assert_eq!(
            reason(&lines),
            "One player only here: name one, or use @s, @p or @r"
        );
        assert_eq!(r.hit(BOB, 20.0, 50.0, "minecraft:generic", false), 50.0);
        let lines = said(r.say(OP, "deathprotect @e"));
        assert_eq!(reason(&lines), "Only @a, @p, @r and @s are supported here");
        let lines = said(r.say(OP, "deathprotect @a[distance=..5]"));
        assert_eq!(
            reason(&lines),
            "Selector options are not supported here: use @a, @p, @r, @s or a name"
        );
    }

    #[test]
    fn the_nearest_player_is_in_the_same_dimension() {
        let mut r = rig();
        r.player_mut(OP).position = [0.0, 64.0, 0.0];
        r.player_mut(BOB).position = [3.0, 64.0, 0.0];
        r.player_mut(ALICE).dimension = NETHER.to_owned();
        r.tick();
        // The operator is the nearest to themselves; from Bob, Bob is.
        said(r.say(BOB, "deathprotect on @p"));
        assert_eq!(r.hit(BOB, 20.0, 50.0, "minecraft:generic", false), 19.0);
        assert_eq!(r.hit(OP, 20.0, 50.0, "minecraft:generic", false), 50.0);
    }

    #[test]
    fn the_console_must_name_a_player() {
        let mut r = rig();
        let lines = said(r.console("deathprotect"));
        assert_eq!(
            reason(&lines),
            "The console has no death protection: name a player"
        );
        let lines = said(r.console("deathprotect on @s"));
        assert_eq!(reason(&lines), "The console is no player: name one");
        assert_eq!(
            said(r.console("deathprotect on Bob")),
            [
                "Bob death protection has been enabled",
                "Minimum health: 0.5 hearts"
            ]
        );
        assert_eq!(
            said(r.console("deathprotect on @a")),
            [
                "Death protection has been enabled for 2 players",
                "1 already had it enabled"
            ]
        );
    }

    #[test]
    fn a_level_is_half_hearts_from_one_to_twenty() {
        for (level, text) in [
            (1, "0.5 hearts"),
            (2, "1 heart"),
            (3, "1.5 hearts"),
            (4, "2 hearts"),
            (19, "9.5 hearts"),
            (20, "10 hearts"),
        ] {
            assert_eq!(hearts(level), text);
        }
    }

    #[test]
    fn the_level_is_the_floor_a_hit_cannot_take_health_below() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on bob"));
        assert_eq!(
            said(r.say(OP, "deathprotect level 4 bob")),
            ["Bob minimum health has been set to 2 hearts"]
        );
        // From 20 health, the most a hit can take is 16.
        assert_eq!(r.hit(BOB, 20.0, 100.0, "minecraft:generic", false), 16.0);
        assert_eq!(
            said(r.say(OP, "deathprotect bob")),
            ["Bob death protection has been disabled"]
        );
        said(r.say(OP, "deathprotect bob"));
        assert_eq!(
            said(r.say(OP, "deathprotect")),
            [
                "Your death protection has been enabled",
                "Minimum health: 0.5 hearts"
            ]
        );
        // The toggle tells the floor that was set.
        said(r.say(OP, "deathprotect off bob"));
        assert_eq!(
            said(r.say(OP, "deathprotect bob")),
            [
                "Bob death protection has been enabled",
                "Minimum health: 2 hearts"
            ]
        );
    }

    #[test]
    fn a_level_with_protection_off_says_so() {
        let mut r = rig();
        assert_eq!(
            said(r.say(OP, "deathprotect level 5")),
            [
                "Your minimum health has been set to 2.5 hearts",
                "Death protection is off for you: /deathprotect on turns it on"
            ]
        );
        said(r.say(OP, "deathprotect on"));
        assert_eq!(
            said(r.say(OP, "deathprotect level 20")),
            ["Your minimum health has been set to 10 hearts"]
        );
        assert_eq!(
            said(r.say(OP, "deathprotect level 3 alice")),
            [
                "Alice minimum health has been set to 1.5 hearts",
                "Death protection is off for Alice"
            ]
        );
        assert_eq!(
            said(r.say(OP, "deathprotect level 1 @a")),
            [
                "Minimum health has been set to 0.5 hearts for 3 players",
                "Death protection is off for 2 of them"
            ]
        );
    }

    #[test]
    fn a_level_outside_one_to_twenty_is_refused_and_changes_nothing() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on"));
        said(r.say(OP, "deathprotect level 10"));
        for (typed, why) in [
            ("0", "The level must be from 1 to 20 (half a heart each)"),
            ("21", "The level must be from 1 to 20 (half a heart each)"),
            ("-4", "The level must be from 1 to 20 (half a heart each)"),
            ("abc", "The level must be a whole number from 1 to 20"),
            ("2.5", "The level must be a whole number from 1 to 20"),
        ] {
            let lines = said(r.say(OP, &format!("deathprotect level {typed}")));
            assert_eq!(reason(&lines), why, "level {typed}");
        }
        assert_eq!(r.hit(OP, 20.0, 100.0, "minecraft:generic", false), 10.0);
        let lines = said(r.say(OP, "deathprotect level"));
        assert!(
            reason(&lines).starts_with("Usage: /deathprotect"),
            "{lines:?}"
        );
        let lines = said(r.say(OP, "deathprotect on bob alice"));
        assert!(
            reason(&lines).starts_with("Usage: /deathprotect"),
            "{lines:?}"
        );
    }

    #[test]
    fn the_commands_of_other_mods_are_not_taken() {
        let mut r = rig();
        for line in [
            "gamemode creative",
            "deathprotection",
            "ondeaths",
            "nick Bob",
            "",
        ] {
            assert_eq!(r.say(OP, line), None, "{line}");
        }
    }

    // ---- the hit ---------------------------------------------------------------------------

    #[test]
    fn a_hit_takes_the_player_down_to_the_floor_and_no_further() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on"));
        let hit =
            |r: &Rig, health, amount| r.hit(OP, health, amount, "minecraft:mob_attack", false);
        // More damage than health: down to half a heart.
        assert_eq!(hit(&r, 20.0, 30.0), 19.0);
        assert_eq!(hit(&r, 6.0, 6.0), 5.0);
        // A hit that leaves them above the floor is not touched.
        assert_eq!(hit(&r, 20.0, 5.0), 5.0);
        assert_eq!(hit(&r, 6.0, 5.0), 5.0);
        // At the floor, a hit takes nothing; below it, nor is anyone healed.
        assert_eq!(hit(&r, 1.0, 3.0), 0.0);
        said(r.say(OP, "deathprotect level 10"));
        assert_eq!(hit(&r, 4.0, 2.0), 0.0);
        assert_eq!(hit(&r, 10.5, 2.0), 0.5);
    }

    #[test]
    fn a_totem_in_either_hand_is_left_to_pop() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on"));
        assert_eq!(r.hit(OP, 20.0, 30.0, "minecraft:generic", true), 30.0);
        assert_eq!(r.hit(OP, 20.0, 30.0, "minecraft:generic", false), 19.0);
    }

    #[test]
    fn kill_and_the_void_are_not_held_back() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on"));
        for kind in ["minecraft:generic_kill", "minecraft:out_of_world"] {
            assert_eq!(r.hit(OP, 20.0, 1000.0, kind, false), 1000.0, "{kind}");
        }
        assert_eq!(r.hit(OP, 20.0, 1000.0, "minecraft:fall", false), 19.0);
    }

    #[test]
    fn only_a_player_with_protection_on_is_held() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on"));
        assert_eq!(r.hit(BOB, 20.0, 30.0, "minecraft:generic", false), 30.0);
        // A hit on a player the server has not yet named is not held either.
        let mut amount = 30.0;
        dispatch_living_damage_pre_v2(
            &r.mods,
            100,
            None,
            "minecraft:player",
            "minecraft:generic",
            0,
            0,
            20.0,
            false,
            &mut amount,
        );
        assert_eq!(amount, 30.0);
        // A mob is not a player.
        let mut amount = 30.0;
        dispatch_living_damage_pre_v2(
            &r.mods,
            100,
            Some(OP),
            "minecraft:zombie",
            "minecraft:generic",
            0,
            0,
            20.0,
            false,
            &mut amount,
        );
        assert_eq!(amount, 30.0);
    }

    // ---- /ondeath --------------------------------------------------------------------------

    #[test]
    fn players_are_kicked_and_operators_spectate_unless_told_otherwise() {
        let mut r = rig();
        assert_eq!(said(r.say(OP, "ondeath")), ["Players who die are kicked"]);
        assert_eq!(
            said(r.say(OP, "ondeath admin")),
            ["Operators who die are put in spectator mode"]
        );
        assert_eq!(
            said(r.say(OP, "ondeath none")),
            ["Players who die will now be left alone"]
        );
        assert_eq!(
            said(r.say(OP, "ondeath admin kick")),
            ["Operators who die will now be kicked"]
        );
        assert_eq!(
            said(r.say(OP, "ondeath spectate")),
            ["Players who die will now be put in spectator mode"]
        );
        assert_eq!(
            said(r.say(OP, "ondeath")),
            ["Players who die are put in spectator mode"]
        );
        let lines = said(r.say(OP, "ondeath explode"));
        assert_eq!(
            reason(&lines),
            "Usage: /ondeath [admin] <spectate|kick|none>"
        );
    }

    #[test]
    fn a_player_who_dies_is_kicked_with_their_death_message() {
        let mut r = rig();
        r.die(BOB, "Bob was slain by Zombie");
        assert_eq!(r.host.kicks, [(BOB, "Bob was slain by Zombie".to_owned())]);
        assert!(r.host.game_modes.is_empty());
    }

    #[test]
    fn an_operator_who_dies_spectates_where_they_died() {
        let mut r = rig();
        r.player_mut(OP).dimension = NETHER.to_owned();
        r.player_mut(OP).position = [10.5, 70.0, -20.5];
        r.tick();
        r.die(OP, "Op fell from a high place");
        assert!(r.host.kicks.is_empty());
        // The death screen's respawn puts them at the death spot, as a spectator.
        let (dimension, at) = r.respawn(OP, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!((dimension.as_str(), at), (NETHER, [10.5, 70.0, -20.5]));
        assert_eq!(r.host.game_modes, [(OP, 3)]);
        // The next respawn is an ordinary one.
        let (dimension, at) = r.respawn(OP, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!((dimension.as_str(), at), (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!(r.host.game_modes, [(OP, 3)]);
    }

    #[test]
    fn what_a_death_does_follows_the_rules_for_that_kind_of_player() {
        let mut r = rig();
        said(r.say(OP, "ondeath spectate"));
        said(r.say(OP, "ondeath admin kick"));
        r.die(BOB, "Bob died");
        r.die(OP, "Op died");
        assert_eq!(r.host.kicks, [(OP, "Op died".to_owned())]);
        r.respawn(BOB, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!(r.host.game_modes, [(BOB, 3)]);

        said(r.say(OP, "ondeath none"));
        r.die(ALICE, "Alice died");
        r.respawn(ALICE, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!(r.host.kicks.len(), 1);
        assert_eq!(r.host.game_modes.len(), 1);
    }

    #[test]
    fn a_player_who_left_before_respawning_does_not_spectate_next_time() {
        let mut r = rig();
        r.die(OP, "Op died");
        r.leave(OP);
        r.join("Op", OP, 4);
        r.respawn(OP, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert!(r.host.game_modes.is_empty());
    }

    #[test]
    fn a_bot_that_dies_is_left_alone() {
        let mut r = rig();
        r.join("Bot", 0xb07, 0);
        r.player_mut(0xb07).fake = true;
        r.tick();
        r.die(0xb07, "Bot died");
        assert!(r.host.kicks.is_empty());
    }

    // ---- /deathmsg and /deathmsgradius -----------------------------------------------------

    fn names(line: &Text) -> String {
        format!("{line:?}")
    }

    #[test]
    fn a_death_tells_the_others_a_player_left_by_default() {
        let mut r = rig();
        let (line, radius) = r.death_line(BOB);
        let line = line.expect("a line");
        assert_eq!(line.plain(), "multiplayer.player.left");
        assert!(names(&line).contains("Bob"), "{line:?}");
        assert_eq!(radius, Some(150.0));
    }

    #[test]
    fn the_leave_line_names_a_player_by_their_nickname() {
        let mut r = rig();
        r.player_mut(BOB).nickname = Some("Captain".to_owned());
        r.tick();
        let (line, _) = r.death_line(BOB);
        let line = line.unwrap();
        assert!(
            names(&line).contains("Captain") && !names(&line).contains("Bob"),
            "{line:?}"
        );
    }

    #[test]
    fn death_leaves_the_death_message_and_none_silences_it() {
        let mut r = rig();
        assert_eq!(
            said(r.say(OP, "deathmsg death")),
            [
                "Death messages set to death",
                "A death shows its death message"
            ]
        );
        let (line, radius) = r.death_line(BOB);
        assert_eq!(line.unwrap().plain(), "Bob was slain");
        assert_eq!(radius, Some(150.0));
        assert_eq!(
            said(r.say(OP, "deathmsg none")),
            ["Death messages set to none", "A death tells nobody"]
        );
        assert_eq!(r.death_line(BOB).0, None);
        assert_eq!(
            said(r.say(OP, "deathmsg")),
            ["Death messages are set to none", "A death tells nobody"]
        );
        let lines = said(r.say(OP, "deathmsg shout"));
        assert_eq!(reason(&lines), "Usage: /deathmsg <none|death|leave>");
        assert_eq!(r.death_line(BOB).0, None);
    }

    #[test]
    fn the_message_keeps_to_the_radius() {
        let mut r = rig();
        assert_eq!(
            said(r.say(OP, "deathmsgradius 40")),
            ["Death messages now reach 40 blocks"]
        );
        assert_eq!(r.death_line(BOB).1, Some(40.0));
        assert_eq!(
            said(r.say(OP, "deathmsgradius 1")),
            ["Death messages now reach 1 block"]
        );
        assert_eq!(
            said(r.say(OP, "deathmsgradius 0")),
            ["Death messages now reach only the player who died"]
        );
        assert_eq!(r.death_line(BOB).1, Some(0.0));
        assert_eq!(
            said(r.say(OP, "deathmsgradius -1")),
            ["Death messages now reach everyone"]
        );
        assert_eq!(r.death_line(BOB).1, None);
        assert_eq!(
            said(r.say(OP, "deathmsgradius")),
            ["Death messages reach everyone"]
        );
    }

    #[test]
    fn a_radius_that_is_not_a_distance_is_refused() {
        let mut r = rig();
        said(r.say(OP, "deathmsgradius 40"));
        for typed in ["abc", "-2", "1.5", "99999999999999999999"] {
            let lines = said(r.say(OP, &format!("deathmsgradius {typed}")));
            assert_eq!(
                reason(&lines),
                "The radius must be a whole number of blocks, or -1 for everyone",
                "{typed}"
            );
        }
        assert_eq!(r.death_line(BOB).1, Some(40.0));
    }

    #[test]
    fn the_real_join_and_leave_lines_are_off_while_a_death_shows_a_leave_line() {
        let mut r = rig();
        r.tick();
        r.tick();
        // The default is leave: the engine is told once.
        assert_eq!(r.host.lines, [false]);
        said(r.say(OP, "deathmsg death"));
        assert_eq!(r.host.lines, [false, true]);
        r.tick();
        said(r.say(OP, "deathmsg none"));
        assert_eq!(r.host.lines, [false, true]);
        said(r.say(OP, "deathmsg leave"));
        assert_eq!(r.host.lines, [false, true, false]);
    }

    #[test]
    fn the_engine_is_asked_again_while_it_cannot_take_the_lines() {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        r.host.hub_away = true;
        r.tick();
        r.tick();
        assert!(r.host.lines.is_empty());
        r.host.hub_away = false;
        r.tick();
        r.tick();
        assert_eq!(r.host.lines, [false]);
    }

    // ---- what is kept ----------------------------------------------------------------------

    #[test]
    fn protection_and_the_death_message_rules_survive_a_restart_and_ondeath_does_not() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on bob"));
        said(r.say(OP, "deathprotect level 7 bob"));
        said(r.say(OP, "deathprotect level 3 alice"));
        said(r.say(OP, "deathmsg death"));
        said(r.say(OP, "deathmsgradius 60"));
        said(r.say(OP, "ondeath none"));
        said(r.say(OP, "ondeath admin none"));
        let file = r.file().expect("a file was written");
        assert!(file.contains("deathmsg = \"death\""), "{file}");

        let mut r = r.restart();
        r.join("Op", OP, 4);
        r.join("Bob", BOB, 0);
        r.join("Alice", ALICE, 0);
        r.tick();
        assert_eq!(r.hit(BOB, 20.0, 100.0, "minecraft:generic", false), 13.0);
        // Alice never turned hers on.
        assert_eq!(r.hit(ALICE, 20.0, 100.0, "minecraft:generic", false), 100.0);
        let (line, radius) = r.death_line(BOB);
        assert_eq!(line.unwrap().plain(), "Bob was slain");
        assert_eq!(radius, Some(60.0));
        assert_eq!(r.host.lines, [true]);
        // What a player does on death is back to its default: kick, and spectate for operators.
        assert_eq!(said(r.say(OP, "ondeath")), ["Players who die are kicked"]);
        assert_eq!(
            said(r.say(OP, "ondeath admin")),
            ["Operators who die are put in spectator mode"]
        );
        // Alice's level was kept, so turning it on gives it.
        assert_eq!(
            said(r.say(OP, "deathprotect alice")),
            [
                "Alice death protection has been enabled",
                "Minimum health: 1.5 hearts"
            ]
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_is_set_aside_and_told() {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        r.write_file("deathmsg = [ this is not toml");
        r.tick();
        assert!(
            r.host
                .logged()
                .iter()
                .any(|l| l.starts_with("WARN: Hardcore: could not read")),
            "{:?}",
            r.host.logged()
        );
        assert!(r.file_exists("hardcore.toml.broken"));
        // The defaults hold, and the next change writes a fresh file.
        assert_eq!(
            r.death_line(OP).0.unwrap().plain(),
            "multiplayer.player.left"
        );
        said(r.say(OP, "deathmsg none"));
        assert!(r.file().unwrap().contains("deathmsg = \"none\""));
    }

    #[test]
    fn a_player_back_to_the_default_is_not_kept() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on bob"));
        said(r.say(OP, "deathprotect off bob"));
        said(r.say(OP, "deathmsgradius 150"));
        let file = r.file().unwrap();
        assert!(!file.contains("enabled"), "{file}");
    }

    // ---- the command tree ------------------------------------------------------------------

    #[test]
    fn every_usage_is_for_operators_and_has_a_permission_node() {
        let mut r = rig();
        let commands = r.register();
        assert_eq!(commands.len(), 19);
        for c in &commands {
            assert_eq!(c.level, 2, "{}", c.usage);
            assert_eq!(
                c.permission.as_deref(),
                Some("hardcore.admin"),
                "{}",
                c.usage
            );
        }
        let usages: Vec<&str> = commands.iter().map(|c| c.usage.as_str()).collect();
        for wanted in [
            "deathprotect <target:text?>",
            "deathprotect on [targets:text?]",
            "deathprotect level <level:integer> [targets:text?]",
            "ondeath admin kick",
            "deathmsg leave",
            "deathmsgradius <radius:integer>",
        ] {
            assert!(usages.contains(&wanted), "{wanted}");
        }
    }

    #[test]
    fn a_target_completes_to_the_nickname_or_else_the_name() {
        let mut r = rig();
        r.player_mut(BOB).nickname = Some("Captain".to_owned());
        r.tick();
        // The engine offered the real names; the mod's replace them.
        assert_eq!(
            r.complete(OP, "deathprotect "),
            ["Alice", "Captain", "Op", "@p", "@r", "@s"]
        );
        assert_eq!(r.complete(OP, "deathprotect ca"), ["Captain"]);
        assert_eq!(r.complete(OP, "deathprotect @"), ["@p", "@r", "@s"]);
        // For several players, `@a` too.
        for line in [
            "deathprotect on ",
            "deathprotect off ",
            "deathprotect level 5 ",
        ] {
            let found = r.complete(OP, line);
            assert!(found.contains(&"@a".to_owned()), "{line}: {found:?}");
            assert!(found.contains(&"Captain".to_owned()) && !found.contains(&"Bob".to_owned()));
        }
    }

    #[test]
    fn nothing_else_is_completed_by_the_mod() {
        let mut r = rig();
        // Where the mod has nothing to add, the engine's suggestions stay.
        for line in [
            "ondeath ",
            "deathmsg ",
            "deathprotect level ",
            "deathprotect on bob ",
            "gamemode ",
        ] {
            assert_eq!(r.complete(OP, line), ["Op", "Bob", "Alice"], "{line}");
        }
    }
}
