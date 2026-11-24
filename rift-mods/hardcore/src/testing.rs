//! A server in memory, and the mod's real hooks driven over it.
//!
//! `FakeHost` is the part of the engine's `Host` the mod uses, with a record of what it was asked to
//! do. `Rig` boxes the mod as the engine does and runs its hooks through the engine's own
//! `dispatch_*` functions, so what the tests exercise is what the server runs.

use horizon_client::mod_api::{
    Hooks, Host, NativeChunkLoad, NativeColumnBlocks, NativeCommand, NativeHeightmap,
    NativeServerPlayer, Suggestion, Text, dispatch_level_tick_post, dispatch_living_damage_pre_v2,
    dispatch_perform_command, dispatch_perform_console_command, dispatch_player_death_loot,
    dispatch_player_death_message, dispatch_player_died_v2, dispatch_player_logged_out,
    dispatch_player_respawn_position, dispatch_player_respawned, dispatch_register_commands,
    dispatch_tab_complete_v2,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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
    /// The world spawn.
    pub world_spawn: Option<[i32; 3]>,
    /// Chunks asked for, by who asked: the ticks left before each is loaded.
    pub requests: HashMap<(String, [i32; 2]), u32>,
    /// Ticks a chunk takes to load.
    pub chunk_delay: u32,
    /// Every chunk let go.
    pub released: Vec<(String, [i32; 2])>,
    /// Every `respawn_player`.
    pub respawns: Vec<u128>,
    /// Every mod command run as the console.
    pub commands: Vec<String>,
    /// The ground at a column: its height (the first free block) and the block that tops it.
    pub ground: Box<dyn Fn(i32, i32) -> (i32, String)>,
    /// Columns whose chunk is never loaded.
    pub failing: HashSet<[i32; 2]>,
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

    fn world_spawn(&self) -> Option<[i32; 3]> {
        self.world_spawn
    }

    fn chunk_load_request(&mut self, owner: &str, dimension: &str, chunk: [i32; 2]) -> bool {
        assert_eq!(dimension, OVERWORLD);
        if self.failing.contains(&chunk) {
            return false;
        }
        let key = (owner.to_owned(), chunk);
        if self.requests.contains_key(&key) {
            return false;
        }
        self.requests.insert(key, self.chunk_delay);
        true
    }

    fn chunk_load_state(&self, owner: &str, _dimension: &str, chunk: [i32; 2]) -> NativeChunkLoad {
        match self.requests.get(&(owner.to_owned(), chunk)) {
            None => NativeChunkLoad::Failed,
            Some(0) => NativeChunkLoad::Loaded,
            Some(_) => NativeChunkLoad::Pending,
        }
    }

    fn chunk_load_release(&mut self, owner: &str, _dimension: &str, chunk: [i32; 2]) -> bool {
        self.released.push((owner.to_owned(), chunk));
        self.requests.remove(&(owner.to_owned(), chunk)).is_some()
    }

    fn read_columns(
        &self,
        dimension: &str,
        columns: &[(i32, i32)],
        heightmap: NativeHeightmap,
        range: (u32, u32),
    ) -> Result<NativeColumnBlocks, String> {
        assert_eq!(dimension, OVERWORLD);
        assert_eq!(heightmap, NativeHeightmap::MotionBlocking);
        assert_eq!(
            range,
            (2, 2),
            "the mod reads two under the height and two over it"
        );
        let mut out = NativeColumnBlocks {
            palette: vec![
                "minecraft:stone".to_owned(),
                String::new(),
                "minecraft:air".to_owned(),
            ],
            ..Default::default()
        };
        for (x, z) in columns {
            let loaded = self
                .requests
                .iter()
                .any(|((_, chunk), left)| *left == 0 && *chunk == [x >> 4, z >> 4]);
            if !loaded {
                out.bases.push(None);
                out.blocks.extend([2, 2, 2, 2]);
                continue;
            }
            let (height, top) = (self.ground)(*x, *z);
            out.palette[1] = top;
            out.bases.push(Some(height));
            out.blocks.extend([0, 1, 2, 2]);
            out.tints.extend([0, 0, 0]);
        }
        Ok(out)
    }

    fn respawn_player(&mut self, uuid: u128) -> bool {
        self.respawns.push(uuid);
        true
    }

    fn run_mod_command(&mut self, line: &str) -> bool {
        self.commands.push(line.to_owned());
        true
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
                world_spawn: Some([10, 64, 20]),
                requests: HashMap::new(),
                chunk_delay: 0,
                released: Vec::new(),
                respawns: Vec::new(),
                commands: Vec::new(),
                ground: Box::new(|_, _| (70, "minecraft:grass_block".to_owned())),
                failing: HashSet::new(),
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

    /// A tick of the overworld: chunks being loaded come a tick nearer.
    pub fn tick(&mut self) {
        for left in self.host.requests.values_mut() {
            *left = left.saturating_sub(1);
        }
        dispatch_level_tick_post(&mut self.host, &self.mods, OVERWORLD);
    }

    pub fn ticks(&mut self, n: u32) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// What a death does to the inventory and the experience, as the engine's rule starts them
    /// (`keep_inventory` off) and the mod leaves them: kept, dropped, experience dropped and
    /// kept.
    pub fn loot(&mut self, uuid: u128) -> (bool, bool, bool, bool) {
        let name = self
            .host
            .players
            .iter()
            .find(|p| p.uuid == uuid)
            .unwrap()
            .name
            .clone();
        let (mut keep, mut drop, mut xp, mut keep_xp) = (false, true, true, false);
        dispatch_player_death_loot(
            &mut self.host,
            &self.mods,
            uuid,
            &name,
            &mut keep,
            &mut drop,
            &mut xp,
            &mut keep_xp,
        );
        (keep, drop, xp, keep_xp)
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
    fn the_minimum_health_is_gray_with_the_hearts_in_the_accent() {
        let mut r = rig();
        said(r.say(OP, "deathprotect on bob"));
        let line = r.host.styled.last().expect("a second line");
        assert_eq!(line.plain(), "Minimum health: 0.5 hearts");
        let colours: Vec<(String, Option<u32>)> = line
            .spans
            .iter()
            .map(|span| (span.text.clone(), span.style.color))
            .collect();
        assert_eq!(
            colours,
            [
                ("Minimum health: ".to_owned(), Some(rift_chat::GRAY)),
                ("0.5 hearts".to_owned(), Some(rift_chat::ACCENT)),
            ]
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
        assert!(!file.contains("[protection."), "{file}");
    }

    // ---- the command tree ------------------------------------------------------------------

    #[test]
    fn every_usage_is_for_operators_and_has_a_permission_node() {
        let mut r = rig();
        let commands = r.register();
        assert_eq!(commands.len(), 31);
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

#[cfg(test)]
mod cycling {
    use super::*;
    use crate::cycle::owner;

    /// An operator and two players, with a tick seen, and death cycling on.
    fn rig() -> Rig {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        r.join("Bob", BOB, 0);
        r.join("Alice", ALICE, 0);
        r.tick();
        r.say(OP, "cycledeath on").unwrap();
        r
    }

    fn said(lines: Option<Vec<String>>) -> Vec<String> {
        lines.expect("the mod took the command")
    }

    fn reason(lines: &[String]) -> String {
        assert_eq!(lines.len(), 2, "an error is two lines: {lines:?}");
        assert_eq!(
            lines[0],
            "Unknown or incomplete command. See below for error"
        );
        let (_, rest) = lines[1].split_once("<--[").expect("the reason");
        rest.strip_suffix(']').unwrap().to_owned()
    }

    /// The colours a say line was drawn in, as (text, colour).
    fn colours(r: &Rig, line: usize) -> Vec<(String, Option<u32>)> {
        r.host.styled[line]
            .spans
            .iter()
            .map(|s| (s.text.clone(), s.style.color))
            .collect()
    }

    fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
        ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    // ---- the commands ----------------------------------------------------------------------

    #[test]
    fn the_status_shows_whether_it_is_on_and_the_settings_in_gray_with_the_accent() {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        r.join("Bob", BOB, 0);
        r.tick();
        assert_eq!(
            said(r.say(OP, "cycledeath")),
            [
                "Death cycling is off",
                "Mode: deathpoint, 150 blocks from where they died",
                "Inventory: drop, and up to 100 points of experience always drop",
                "Exempt: nobody"
            ]
        );
        for line in 0..4 {
            assert_eq!(colours(&r, line)[0].1, Some(rift_chat::GRAY), "line {line}");
        }
        // The state is in the accent.
        assert_eq!(
            colours(&r, 0),
            [
                ("Death cycling is ".to_owned(), Some(rift_chat::GRAY)),
                ("off".to_owned(), Some(rift_chat::ACCENT)),
            ]
        );
        said(r.say(OP, "cycledeath on"));
        said(r.say(OP, "cycledeath mode spawnpoint"));
        said(r.say(OP, "cycledeath distance 80"));
        said(r.say(OP, "cycledeath setspawn 100 65 -20"));
        said(r.say(OP, "cycledeath inventory both"));
        said(r.say(OP, "cycledeath exempt bob"));
        assert_eq!(
            said(r.say(OP, "cycledeath")),
            [
                "Death cycling is on",
                "Mode: spawnpoint, within 80 blocks of 100, 65, -20",
                "Inventory: both, and up to 100 points of experience always drop",
                "Exempt: Bob"
            ]
        );
    }

    #[test]
    fn the_spawn_point_is_the_world_spawn_until_one_is_set() {
        let mut r = rig();
        said(r.say(OP, "cycledeath mode spawnpoint"));
        assert_eq!(
            said(r.say(OP, "cycledeath"))[1],
            "Mode: spawnpoint, within 150 blocks of the world spawn"
        );
    }

    #[test]
    fn on_and_off_say_what_they_did_and_when_nothing_changed_say_so_in_gray() {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        r.tick();
        assert_eq!(
            said(r.say(OP, "cycledeath on")),
            ["Death cycling has been enabled"]
        );
        assert_eq!(colours(&r, 0)[0].1, Some(rift_chat::WHITE));
        assert_eq!(
            said(r.say(OP, "cycledeath on")),
            ["Death cycling is already enabled"]
        );
        assert_eq!(colours(&r, 0)[0].1, Some(rift_chat::GRAY));
        assert_eq!(
            said(r.say(OP, "cycledeath off")),
            ["Death cycling has been disabled"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath off")),
            ["Death cycling is already disabled"]
        );
    }

    #[test]
    fn the_mode_the_inventory_and_the_distance_are_set_and_a_repeat_is_told() {
        let mut r = rig();
        assert_eq!(
            said(r.say(OP, "cycledeath mode spawnpoint")),
            ["Death cycled players now respawn around the spawn point"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath mode spawnpoint")),
            ["Death cycled players already respawn around the spawn point"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath mode deathpoint")),
            ["Death cycled players now respawn around where they died"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath inventory keep")),
            ["Death cycled players now keep their inventory and drop nothing"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath inventory both")),
            ["Death cycled players now keep their inventory and drop a copy of it"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath inventory both")),
            ["Death cycled players already keep their inventory and drop a copy of it"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath inventory drop")),
            ["Death cycled players now drop their inventory"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath distance 300")),
            ["Death cycle distance set to 300 blocks"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath distance 300")),
            ["The death cycle distance is already 300 blocks"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath distance 1")),
            ["Death cycle distance set to 1 block"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath distance 0")),
            ["Death cycle distance set to 0 blocks"]
        );
    }

    #[test]
    fn a_distance_must_be_a_whole_number_from_zero_to_the_most() {
        let mut r = rig();
        for (typed, why) in [
            ("-1", "The distance cannot be below 0"),
            ("-150", "The distance cannot be below 0"),
            ("100001", "The distance cannot be more than 100000 blocks"),
            (
                "99999999999999999999",
                "The distance must be a whole number of blocks",
            ),
            ("abc", "The distance must be a whole number of blocks"),
            ("1.5", "The distance must be a whole number of blocks"),
        ] {
            let lines = said(r.say(OP, &format!("cycledeath distance {typed}")));
            assert_eq!(reason(&lines), why, "{typed}");
        }
        // Nothing changed, and the most is allowed.
        assert_eq!(
            said(r.say(OP, "cycledeath"))[1],
            "Mode: deathpoint, 150 blocks from where they died"
        );
        assert_eq!(
            said(r.say(OP, "cycledeath distance 100000")),
            ["Death cycle distance set to 100000 blocks"]
        );
    }

    #[test]
    fn a_word_that_is_not_a_mode_or_an_inventory_is_an_error() {
        let mut r = rig();
        let lines = said(r.say(OP, "cycledeath mode nearby"));
        assert_eq!(
            reason(&lines),
            "Usage: /cycledeath mode <deathpoint|spawnpoint>"
        );
        let lines = said(r.say(OP, "cycledeath inventory some"));
        assert_eq!(
            reason(&lines),
            "Usage: /cycledeath inventory <drop|keep|both>"
        );
        for line in [
            "cycledeath mode",
            "cycledeath on now",
            "cycledeath setspawn 1 2",
            "cycledeath exempt",
            "cycledeath wat",
        ] {
            let lines = said(r.say(OP, line));
            assert!(
                reason(&lines).starts_with("Usage: /cycledeath"),
                "{line}: {lines:?}"
            );
        }
    }

    #[test]
    fn the_spawn_point_is_where_you_stand_or_the_coordinates_you_give() {
        let mut r = rig();
        r.player_mut(OP).position = [12.7, 65.0, -30.2];
        assert_eq!(
            said(r.say(OP, "cycledeath setspawn")),
            ["Death cycle spawn point set to 12, 65, -31"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath setspawn 100 70 -200")),
            ["Death cycle spawn point set to 100, 70, -200"]
        );
        // `~` is the one who ran it, as in vanilla.
        assert_eq!(
            said(r.say(OP, "cycledeath setspawn ~ ~2 ~-5")),
            ["Death cycle spawn point set to 12, 67, -36"]
        );
        let lines = said(r.say(OP, "cycledeath setspawn 1 up 3"));
        assert_eq!(
            reason(&lines),
            "The coordinate must be a number, or ~ for yours"
        );
        let lines = said(r.say(OP, "cycledeath setspawn 1 2 99999999"));
        assert_eq!(reason(&lines), "That is outside the world");
        // The console has no position, and no `~`.
        let lines = said(r.console("cycledeath setspawn"));
        assert!(
            reason(&lines).starts_with("The console has no position"),
            "{lines:?}"
        );
        let lines = said(r.console("cycledeath setspawn ~ 1 1"));
        assert_eq!(reason(&lines), "The console has no position for ~");
        assert_eq!(
            said(r.console("cycledeath setspawn 0 64 0")),
            ["Death cycle spawn point set to 0, 64, 0"]
        );
    }

    #[test]
    fn a_player_can_be_exempted_and_exempted_no_more() {
        let mut r = rig();
        r.player_mut(BOB).nickname = Some("Captain".to_owned());
        r.tick();
        assert_eq!(
            said(r.say(OP, "cycledeath exempt bob")),
            ["Captain is now exempt from death cycling"]
        );
        assert_eq!(
            said(r.say(OP, "cycledeath exempt captain")),
            ["Captain is no longer exempt from death cycling"]
        );
        let lines = said(r.say(OP, "cycledeath exempt nobody"));
        assert_eq!(reason(&lines), "Player was not found");
        let lines = said(r.say(OP, "cycledeath exempt @a"));
        assert_eq!(
            reason(&lines),
            "One player only here: name one, or use @s, @p or @r"
        );
        assert_eq!(
            said(r.say(OP, "cycledeath exempt @s")),
            ["Op is now exempt from death cycling"]
        );
    }

    #[test]
    fn an_exempt_player_completes_by_nickname_and_the_other_words_by_the_client() {
        let mut r = rig();
        r.player_mut(BOB).nickname = Some("Captain".to_owned());
        r.tick();
        assert_eq!(
            r.complete(OP, "cycledeath exempt "),
            ["Alice", "Captain", "Op", "@p", "@r", "@s"]
        );
        assert_eq!(r.complete(OP, "cycledeath exempt c"), ["Captain"]);
        assert_eq!(r.complete(OP, "cycledeath mode "), ["Op", "Bob", "Alice"]);
    }

    #[test]
    fn the_cycle_usages_are_for_operators() {
        let mut r = rig();
        let commands = r.register();
        let usages: Vec<&str> = commands.iter().map(|c| c.usage.as_str()).collect();
        for wanted in [
            "cycledeath",
            "cycledeath mode spawnpoint",
            "cycledeath setspawn <pos:block_pos>",
            "cycledeath inventory both",
            "cycledeath distance <blocks:integer>",
            "cycledeath exempt <player:text?>",
        ] {
            assert!(usages.contains(&wanted), "{wanted}");
        }
    }

    // ---- /ondeath while it is on -----------------------------------------------------------

    #[test]
    fn ondeath_says_death_cycling_decides_and_refuses_to_change() {
        let mut r = rig();
        assert_eq!(
            said(r.say(OP, "ondeath")),
            [
                "Players who die are death cycled!",
                "This option is currently being overridden by /cycledeath. Disable it to change this option"
            ]
        );
        // Gray, with the accent on what matters.
        assert_eq!(
            colours(&r, 0),
            [
                ("Players who die are ".to_owned(), Some(rift_chat::GRAY)),
                ("death cycled".to_owned(), Some(rift_chat::ACCENT)),
                ("!".to_owned(), Some(rift_chat::GRAY)),
            ]
        );
        assert_eq!(
            colours(&r, 1),
            [
                (
                    "This option is currently being overridden by ".to_owned(),
                    Some(rift_chat::GRAY)
                ),
                ("/cycledeath".to_owned(), Some(rift_chat::ACCENT)),
                (
                    ". Disable it to change this option".to_owned(),
                    Some(rift_chat::GRAY)
                ),
            ]
        );
        assert_eq!(
            said(r.say(OP, "ondeath admin"))[0],
            "Operators who die are death cycled!"
        );
        for line in ["ondeath kick", "ondeath admin none", "ondeath nonsense"] {
            let lines = said(r.say(OP, line));
            assert!(
                reason(&lines).starts_with("Death cycling is on and overrides this option"),
                "{line}: {lines:?}"
            );
        }
        // It changed nothing: with cycling off again, the settings are the defaults.
        said(r.say(OP, "cycledeath off"));
        assert_eq!(said(r.say(OP, "ondeath")), ["Players who die are kicked"]);
        assert_eq!(colours(&r, 0)[0].1, Some(rift_chat::GRAY));
    }

    #[test]
    fn the_status_of_the_other_commands_is_gray_and_what_they_do_is_white() {
        let mut r = rig();
        said(r.say(OP, "cycledeath off"));
        for line in ["ondeath", "ondeath admin", "deathmsg", "deathmsgradius"] {
            said(r.say(OP, line));
            assert_eq!(colours(&r, 0)[0].1, Some(rift_chat::GRAY), "{line}");
        }
        for line in [
            "ondeath kick",
            "deathmsg death",
            "deathmsgradius 90",
            "deathprotect on",
        ] {
            said(r.say(OP, line));
            assert_eq!(colours(&r, 0)[0].1, Some(rift_chat::WHITE), "{line}");
        }
    }

    // ---- what a death does -----------------------------------------------------------------

    #[test]
    fn the_inventory_follows_the_setting_and_the_experience_always_drops() {
        let mut r = rig();
        // The engine's start: dropped, not kept, experience dropped.
        assert_eq!(r.loot(BOB), (false, true, true, false));
        said(r.say(OP, "cycledeath inventory keep"));
        assert_eq!(r.loot(BOB), (true, false, true, false));
        said(r.say(OP, "cycledeath inventory both"));
        assert_eq!(r.loot(BOB), (true, true, true, false));
        said(r.say(OP, "cycledeath inventory drop"));
        assert_eq!(r.loot(BOB), (false, true, true, false));
    }

    #[test]
    fn an_exempt_player_and_a_world_with_cycling_off_are_left_as_the_engine_has_them() {
        let mut r = rig();
        said(r.say(OP, "cycledeath inventory keep"));
        said(r.say(OP, "cycledeath exempt bob"));
        assert_eq!(r.loot(BOB), (false, true, true, false));
        assert_eq!(r.loot(ALICE), (true, false, true, false));
        said(r.say(OP, "cycledeath off"));
        assert_eq!(r.loot(ALICE), (false, true, true, false));
    }

    // ---- the respawn spot ------------------------------------------------------------------

    #[test]
    fn every_player_gets_a_spot_at_the_set_distance_while_they_live() {
        let mut r = rig();
        r.player_mut(BOB).position = [1000.0, 64.0, -2000.0];
        r.tick();
        r.tick();
        // A chunk was asked for, for each player, by their own name.
        let asked: Vec<&(String, [i32; 2])> = r.host.requests.keys().collect();
        assert_eq!(asked.len(), 3, "{asked:?}");
        let (_, chunk) = r
            .host
            .requests
            .keys()
            .find(|(who, _)| *who == owner(BOB))
            .expect("Bob's chunk");
        // 150 blocks from Bob: the chunk's corner is within a chunk of that ring.
        let corner = [f64::from(chunk[0] * 16), 0.0, f64::from(chunk[1] * 16)];
        let d = distance(corner, [1000.0, 0.0, -2000.0]);
        assert!((d - 150.0).abs() <= 23.0, "{d}");
    }

    #[test]
    fn a_death_respawns_the_player_at_the_spot_before_a_click_and_not_kicked() {
        let mut r = rig();
        r.player_mut(BOB).position = [500.0, 64.0, 500.0];
        r.tick();
        r.tick();
        r.die(BOB, "Bob was slain");
        // Not kicked, not made a spectator: the tick asks the engine to respawn them.
        assert!(r.host.kicks.is_empty() && r.host.game_modes.is_empty());
        r.tick();
        assert_eq!(r.host.respawns, [BOB]);
        let (dimension, at) = r.respawn(BOB, (NETHER, [0.0, 64.0, 0.0]));
        assert_eq!(dimension, OVERWORLD);
        // On the surface the world reports: the first free block above the ground (70), in a
        // block's middle, 150 blocks (within the block it falls in) from where they died.
        assert_eq!(at[1], 70.0);
        assert_eq!((at[0].fract(), at[2].fract()), (0.5, 0.5));
        let d = distance(at, [500.0, 0.0, 500.0]);
        assert!((d - 150.0).abs() <= 1.5, "{d}");
        // The chunk it held is let go, and they get a random nickname at the next tick.
        assert!(
            r.host.released.iter().any(|(who, _)| *who == owner(BOB)),
            "{:?}",
            r.host.released
        );
        r.tick();
        assert_eq!(r.host.commands, ["nick random Bob"]);
        r.tick();
        assert_eq!(r.host.commands.len(), 1, "once");
    }

    #[test]
    fn an_operator_is_cycled_too() {
        let mut r = rig();
        r.ticks(3);
        r.die(OP, "Op died");
        r.tick();
        assert_eq!(r.host.respawns, [OP]);
        assert!(r.host.kicks.is_empty() && r.host.game_modes.is_empty());
        let (dimension, _) = r.respawn(OP, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!(dimension, OVERWORLD);
        r.tick();
        assert_eq!(r.host.commands, ["nick random Op"]);
    }

    #[test]
    fn an_exempt_player_dies_by_the_ondeath_rules() {
        let mut r = rig();
        said(r.say(OP, "cycledeath exempt bob"));
        r.ticks(3);
        // No spot is kept for them.
        assert!(!r.host.requests.keys().any(|(who, _)| *who == owner(BOB)));
        r.die(BOB, "Bob died");
        r.tick();
        assert!(r.host.respawns.is_empty());
        assert_eq!(r.host.kicks, [(BOB, "Bob died".to_owned())]);
    }

    #[test]
    fn ondeath_is_used_again_when_cycling_is_off() {
        let mut r = rig();
        r.ticks(3);
        said(r.say(OP, "cycledeath off"));
        r.tick();
        r.die(BOB, "Bob died");
        assert_eq!(r.host.kicks, [(BOB, "Bob died".to_owned())]);
        // The spots that were held are let go.
        assert!(r.host.requests.is_empty(), "{:?}", r.host.requests);
    }

    #[test]
    fn a_death_before_a_spot_is_ready_waits_for_it_without_a_kick() {
        let mut r = rig();
        r.host.chunk_delay = 5;
        r.ticks(1);
        r.die(BOB, "Bob died");
        r.ticks(3);
        assert!(r.host.respawns.is_empty(), "the chunk is not here yet");
        assert!(r.host.kicks.is_empty());
        r.ticks(4);
        assert_eq!(r.host.respawns, [BOB]);
        let (_, at) = r.respawn(BOB, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!(at[1], 70.0);
    }

    #[test]
    fn a_spot_on_water_or_not_loaded_is_not_used() {
        let mut r = rig();
        // Even columns are lakes: only an odd column can be a spot.
        r.host.ground = Box::new(|x, _| {
            if x % 2 == 0 {
                (63, "minecraft:water".to_owned())
            } else {
                (70, "minecraft:grass_block".to_owned())
            }
        });
        r.ticks(80);
        r.die(BOB, "Bob died");
        r.tick();
        let (_, at) = r.respawn(BOB, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!(at[1], 70.0);
        assert_eq!((at[0] - 0.5).rem_euclid(2.0), 1.0, "{at:?}");
        // The ones that were refused were let go, so nothing stays held.
        assert!(r.host.requests.len() <= 3, "{:?}", r.host.requests);
    }

    #[test]
    fn spots_that_keep_failing_are_tried_again_later() {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        r.tick();
        r.say(OP, "cycledeath on").unwrap();
        r.host.ground = Box::new(|_, _| (63, "minecraft:water".to_owned()));
        r.ticks(25);
        // It tries at most ten times before resting, and holds nothing.
        let held = r.host.requests.len();
        assert_eq!(held, 0);
        let asked = r.host.released.len();
        assert!((10..=20).contains(&asked), "{asked}");
        r.host.ground = Box::new(|_, _| (70, "minecraft:grass_block".to_owned()));
        r.ticks(150);
        r.die(OP, "Op died");
        r.tick();
        assert_eq!(r.host.respawns, [OP]);
    }

    #[test]
    fn a_player_who_moves_far_is_given_a_spot_for_where_they_are_now() {
        let mut r = rig();
        r.player_mut(BOB).position = [0.0, 64.0, 0.0];
        r.ticks(3);
        let first = r
            .host
            .requests
            .keys()
            .find(|(who, _)| *who == owner(BOB))
            .unwrap()
            .1;
        r.player_mut(BOB).position = [3000.0, 64.0, 3000.0];
        r.ticks(3);
        let second = r
            .host
            .requests
            .keys()
            .find(|(who, _)| *who == owner(BOB))
            .unwrap()
            .1;
        assert_ne!(first, second);
        assert!(second[0] > 100, "{second:?}");
        assert!(
            r.host
                .released
                .iter()
                .any(|(who, chunk)| *who == owner(BOB) && *chunk == first),
            "the old chunk is let go"
        );
        // A step is not far.
        r.player_mut(BOB).position = [3005.0, 64.0, 3005.0];
        let held = r.host.requests.clone();
        r.ticks(3);
        assert_eq!(r.host.requests, held);
    }

    #[test]
    fn a_changed_setting_finds_the_spots_again() {
        let mut r = rig();
        r.ticks(3);
        let before = r.host.requests.clone();
        said(r.say(OP, "cycledeath distance 400"));
        r.ticks(3);
        assert_ne!(r.host.requests, before);
        // They are at the new distance from where the players are (the origin, here).
        let (_, chunk) = r
            .host
            .requests
            .keys()
            .find(|(who, _)| *who == owner(BOB))
            .unwrap();
        let d = distance(
            [f64::from(chunk[0] * 16), 0.0, f64::from(chunk[1] * 16)],
            [0.0; 3],
        );
        assert!((d - 400.0).abs() <= 23.0, "{d}");
    }

    #[test]
    fn the_spawn_point_mode_keeps_to_the_distance_from_the_spawn_point() {
        let mut r = rig();
        said(r.say(OP, "cycledeath mode spawnpoint"));
        said(r.say(OP, "cycledeath setspawn 1000 70 -500"));
        said(r.say(OP, "cycledeath distance 60"));
        for _ in 0..30 {
            r.host.requests.clear();
            r.ticks(2);
            for (_, chunk) in r.host.requests.keys() {
                let corner = [f64::from(chunk[0] * 16), 0.0, f64::from(chunk[1] * 16)];
                let d = distance(corner, [1000.0, 0.0, -500.0]);
                assert!(d <= 60.0 + 23.0, "{chunk:?} is {d} from the spawn point");
            }
        }
    }

    #[test]
    fn the_spawn_point_defaults_to_the_world_spawn() {
        let mut r = rig();
        r.host.world_spawn = Some([-3000, 64, 4000]);
        said(r.say(OP, "cycledeath mode spawnpoint"));
        said(r.say(OP, "cycledeath distance 20"));
        r.host.requests.clear();
        r.ticks(2);
        for (_, chunk) in r.host.requests.keys() {
            assert!(
                (chunk[0] * 16 + 3000).abs() <= 40 && (chunk[1] * 16 - 4000).abs() <= 40,
                "{chunk:?}"
            );
        }
    }

    #[test]
    fn a_distance_of_zero_is_the_spot_itself() {
        let mut r = rig();
        said(r.say(OP, "cycledeath distance 0"));
        r.player_mut(BOB).position = [37.6, 64.0, -90.2];
        r.ticks(3);
        r.die(BOB, "Bob died");
        r.tick();
        let (_, at) = r.respawn(BOB, (OVERWORLD, [0.0, 64.0, 0.0]));
        assert_eq!((at[0], at[2]), (37.5, -90.5));
    }

    #[test]
    fn the_engine_is_asked_again_for_a_player_who_has_not_respawned() {
        let mut r = rig();
        r.ticks(3);
        r.die(BOB, "Bob died");
        r.ticks(2);
        assert_eq!(r.host.respawns, [BOB]);
        r.ticks(50);
        assert_eq!(r.host.respawns, [BOB], "not asked again at once");
        r.ticks(60);
        assert_eq!(r.host.respawns, [BOB, BOB]);
    }

    #[test]
    fn a_player_who_leaves_lets_go_of_what_they_held() {
        let mut r = rig();
        r.ticks(3);
        assert!(r.host.requests.keys().any(|(who, _)| *who == owner(BOB)));
        r.leave(BOB);
        assert!(!r.host.requests.keys().any(|(who, _)| *who == owner(BOB)));
        // A player who died and left is not respawned.
        r.die(ALICE, "Alice died");
        r.leave(ALICE);
        r.ticks(3);
        assert!(r.host.respawns.is_empty());
        assert!(r.host.commands.is_empty());
    }

    #[test]
    fn a_bot_is_not_cycled() {
        let mut r = rig();
        r.join("Bot", 0xb07, 0);
        r.player_mut(0xb07).fake = true;
        r.ticks(3);
        assert!(!r.host.requests.keys().any(|(who, _)| *who == owner(0xb07)));
        r.die(0xb07, "Bot died");
        r.tick();
        assert!(r.host.respawns.is_empty());
    }

    // ---- what is kept ----------------------------------------------------------------------

    #[test]
    fn the_settings_are_saved_and_exemptions_are_not() {
        let mut r = rig();
        said(r.say(OP, "cycledeath mode spawnpoint"));
        said(r.say(OP, "cycledeath setspawn 5 70 -9"));
        said(r.say(OP, "cycledeath inventory both"));
        said(r.say(OP, "cycledeath distance 321"));
        said(r.say(OP, "cycledeath exempt bob"));
        let file = r.file().expect("saved");
        assert!(
            file.contains("[cycle]") && file.contains("distance = 321"),
            "{file}"
        );

        let mut r = r.restart();
        r.join("Op", OP, 4);
        r.join("Bob", BOB, 0);
        r.tick();
        assert_eq!(
            said(r.say(OP, "cycledeath")),
            [
                "Death cycling is on",
                "Mode: spawnpoint, within 321 blocks of 5, 70, -9",
                "Inventory: both, and up to 100 points of experience always drop",
                "Exempt: nobody"
            ]
        );
    }

    #[test]
    fn a_saved_distance_out_of_range_is_held_to_it() {
        let mut r = Rig::new();
        r.write_file("[cycle]\nenabled = true\ndistance = 99999999\n");
        r.join("Op", OP, 4);
        r.tick();
        assert_eq!(
            said(r.say(OP, "cycledeath"))[1],
            "Mode: deathpoint, 100000 blocks from where they died"
        );
    }

    #[test]
    fn nothing_is_asked_of_the_world_while_it_is_off() {
        let mut r = Rig::new();
        r.join("Op", OP, 4);
        r.ticks(5);
        assert!(r.host.requests.is_empty() && r.host.released.is_empty());
    }
}
