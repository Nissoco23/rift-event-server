//! A server in memory, and the mod's real hooks driven over it.
//!
//! `FakeHost` is the part of the engine's `Host` the mod uses, with a record of what it was asked
//! to do and the engine's own rules for it: a nickname and a name tag only for a player who is on,
//! a name tag of a tag's shape, a player's nickname and tag gone when they leave. `FakeLookup` is
//! Mojang's services as a table of accounts. `Rig` boxes the mod as the engine does and runs its
//! hooks through the engine's own `dispatch_*` functions, so what the tests exercise is what the
//! server runs.

use crate::rng::Rng;
use crate::skins::{Failure, Lookup, Skin};
use crate::state::OnJoin;
use crate::{Nick, Shared};
use horizon_client::mod_api::{
    Hooks, Host, NativeAuthority, NativeLifecycleContext, NativeServerPlayer, NativeWorldEpoch,
    ServerTickPhase, Suggestion, Text, dispatch_perform_command, dispatch_perform_console_command,
    dispatch_player_logged_in, dispatch_player_logged_out, dispatch_register_commands,
    dispatch_server_tick, dispatch_tab_complete_v2,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const BOB: u128 = 0xb0b;
pub const ALICE: u128 = 0xa11ce;
pub const EVE: u128 = 0xe7e;
pub const CAROL: u128 = 0xca401;

type Textures = Option<(String, Option<String>)>;

/// What the mod did to a player, in the order it did it.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Nickname(u128, Option<String>),
    Tag(u128, Option<String>),
    Property(u128, Textures),
    Refresh(u128),
}

pub struct FakeHost {
    pub players: Vec<NativeServerPlayer>,
    pub nicknames: HashMap<u128, String>,
    pub tags: HashMap<u128, String>,
    /// The `textures` property each player has; a player joins with their own.
    pub textures: HashMap<u128, Textures>,
    pub calls: Vec<Call>,
    /// What the players were told by the mod, by uuid.
    pub told: Vec<(u128, String)>,
    /// What the command's feedback said, in order.
    pub feedback: Vec<String>,
    /// Words the chat filter refuses.
    pub filter: Vec<String>,
    /// The chat filter is out of reach, as in a player's hook: it answers None.
    pub filter_out_of_reach: bool,
    pub refreshed_commands: usize,
    logs: RefCell<Vec<String>>,
    dir: PathBuf,
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

    fn online(&self, uuid: u128) -> bool {
        self.players.iter().any(|p| p.uuid == uuid)
    }
}

impl Host for FakeHost {
    fn server_players(&self) -> Vec<NativeServerPlayer> {
        self.players.clone()
    }

    fn set_nickname(&mut self, uuid: u128, nickname: Option<&str>) -> bool {
        if !self.online(uuid) {
            return false;
        }
        self.calls
            .push(Call::Nickname(uuid, nickname.map(str::to_owned)));
        match nickname {
            Some(n) => self.nicknames.insert(uuid, n.to_owned()),
            None => self.nicknames.remove(&uuid),
        };
        true
    }

    fn set_name_tag(&mut self, uuid: u128, tag: Option<&str>) -> bool {
        // `native_set_name_tag`: 1 to 16 printable ASCII characters, no spaces.
        if tag.is_some_and(|t| {
            t.is_empty() || t.len() > 16 || !t.bytes().all(|b| b.is_ascii_graphic())
        }) || !self.online(uuid)
        {
            return false;
        }
        self.calls.push(Call::Tag(uuid, tag.map(str::to_owned)));
        match tag {
            Some(t) => self.tags.insert(uuid, t.to_owned()),
            None => self.tags.remove(&uuid),
        };
        true
    }

    fn set_profile_property(
        &mut self,
        player: u128,
        name: &str,
        value: Option<(&str, Option<&str>)>,
    ) -> bool {
        assert_eq!(name, "textures", "the mod changes the skin, nothing else");
        if !self.online(player) {
            return false;
        }
        let value = value.map(|(v, s)| (v.to_owned(), s.map(str::to_owned)));
        self.calls.push(Call::Property(player, value.clone()));
        self.textures.insert(player, value);
        true
    }

    fn refresh_player(&mut self, player: u128) -> bool {
        if !self.online(player) {
            return false;
        }
        self.calls.push(Call::Refresh(player));
        true
    }

    fn player_textures(&self, uuid: u128) -> Option<(String, Option<String>)> {
        self.textures.get(&uuid).cloned().flatten()
    }

    fn tell(&mut self, uuid: u128, line: Text) -> bool {
        if !self.online(uuid) {
            return false;
        }
        self.told.push((uuid, line.plain()));
        true
    }

    fn feedback(&mut self, line: Text) {
        self.feedback.push(line.plain());
    }

    fn text_allowed(&self, text: &str) -> Option<bool> {
        if self.filter_out_of_reach {
            return None;
        }
        let lower = text.to_ascii_lowercase();
        Some(!self.filter.iter().any(|word| lower.contains(word.as_str())))
    }

    fn config_dir(&self) -> Result<PathBuf, String> {
        Ok(self.dir.join("config"))
    }

    fn log(&self, level: &str, line: &str) {
        self.logs.borrow_mut().push(format!("{level}: {line}"));
    }

    fn refresh_commands(&mut self) {
        self.refreshed_commands += 1;
    }
}

/// Mojang's services as a table of accounts, counting what it was asked.
pub struct FakeLookup {
    accounts: Mutex<HashMap<String, Skin>>,
    /// The services do not answer.
    pub down: AtomicBool,
    /// A lookup waits while this is set (for a test that changes things under one).
    pub held: AtomicBool,
    pub asked: Mutex<Vec<String>>,
    next_id: AtomicUsize,
}

impl FakeLookup {
    pub fn new(accounts: &[&str]) -> Arc<Self> {
        let lookup = Arc::new(Self {
            accounts: Mutex::new(HashMap::new()),
            down: AtomicBool::new(false),
            held: AtomicBool::new(false),
            asked: Mutex::new(Vec::new()),
            next_id: AtomicUsize::new(0x1000),
        });
        for account in accounts {
            lookup.add(account);
        }
        lookup
    }

    pub fn add(&self, account: &str) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) as u128;
        self.accounts.lock().unwrap().insert(
            account.to_ascii_lowercase(),
            Skin {
                account: account.to_owned(),
                id,
                textures: Some((
                    format!("skin-of-{account}"),
                    Some(format!("signed-{account}")),
                )),
            },
        );
    }

    pub fn asked_for(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

/// The textures `FakeLookup` gives an account.
pub fn skin_of(account: &str) -> Textures {
    Some((
        format!("skin-of-{account}"),
        Some(format!("signed-{account}")),
    ))
}

/// The textures a player joins with.
pub fn own_skin(name: &str) -> Textures {
    Some((format!("own-{name}"), Some(format!("own-signed-{name}"))))
}

impl Lookup for FakeLookup {
    fn fetch(&self, name: &str) -> Result<Skin, Failure> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.held.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.asked.lock().unwrap().push(name.to_owned());
        if self.down.load(Ordering::SeqCst) {
            return Err(Failure::Unavailable);
        }
        self.accounts
            .lock()
            .unwrap()
            .get(&name.to_ascii_lowercase())
            .cloned()
            .ok_or(Failure::NoAccount)
    }
}

/// The mod boxed as the engine holds it, over a `FakeHost` and a `FakeLookup`.
pub struct Rig {
    pub host: FakeHost,
    pub mods: Vec<Box<dyn Hooks>>,
    pub nick: Arc<Shared>,
    pub lookup: Arc<FakeLookup>,
    tick: u64,
    dir: PathBuf,
}

/// The accounts the tests look up.
pub const ACCOUNTS: [&str; 8] = [
    "Captain", "Rex", "Zed", "Max", "Alex", "Steve", "Notch", "Ada",
];

impl Rig {
    /// A rig whose `config/nick.toml` holds `config` and whose pool is `pool` (one name a line),
    /// with every lookup answered at once.
    pub fn new(config: &str, pool: &str) -> Self {
        Self::build(config, pool, false, false)
    }

    /// The same, with `/nickonjoin` as a server starts it (on): the rigs above have it off, so the
    /// players they put on the server keep their names until a test says otherwise.
    pub fn fresh(config: &str, pool: &str) -> Self {
        Self::build(config, pool, false, true)
    }

    /// The same with lookups on worker threads, as the server has them.
    pub fn new_in_background(config: &str, pool: &str) -> Self {
        Self::build(config, pool, true, false)
    }

    fn build(config: &str, pool: &str, background: bool, on_join: bool) -> Self {
        static RIGS: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nick-rig-{}-{}",
            std::process::id(),
            RIGS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(dir.join("config").join("nick.toml"), config).unwrap();
        std::fs::write(dir.join("config").join("nick-pool.txt"), pool).unwrap();
        let lookup = FakeLookup::new(&ACCOUNTS);
        let module = Nick::with(lookup.clone(), background, Rng::new(7));
        let nick = module.handle();
        if !on_join {
            nick.state.lock().unwrap().on_join = OnJoin::Off;
        }
        Self {
            host: FakeHost {
                players: Vec::new(),
                nicknames: HashMap::new(),
                tags: HashMap::new(),
                textures: HashMap::new(),
                calls: Vec::new(),
                told: Vec::new(),
                feedback: Vec::new(),
                filter: Vec::new(),
                filter_out_of_reach: false,
                refreshed_commands: 0,
                logs: RefCell::new(Vec::new()),
                dir: dir.clone(),
            },
            mods: vec![Box::new(module)],
            nick,
            lookup,
            tick: 0,
            dir,
        }
    }

    /// The pool the tests that need none use up.
    pub fn plain() -> Self {
        Self::new("", "Rex\nZed\nMax\nAda\n")
    }

    /// `Bob`, an operator `Alice` and `Eve`, on the server.
    pub fn with_players() -> Self {
        let mut rig = Self::plain();
        rig.join("Bob", BOB, 0);
        rig.join("Alice", ALICE, 4);
        rig.join("Eve", EVE, 0);
        rig
    }

    /// A player joins: on the server, with a skin of their own, when the mod hears of it.
    pub fn join(&mut self, name: &str, uuid: u128, permission: u8) {
        self.host
            .players
            .push(NativeServerPlayer::new(uuid, name, permission));
        self.host.textures.insert(uuid, own_skin(name));
        dispatch_player_logged_in(&mut self.host, &self.mods, uuid, name);
    }

    /// A player leaves: the mod hears of it as the engine has taken their nickname and tag off.
    pub fn leave(&mut self, uuid: u128) {
        let name = self.player(uuid).name.clone();
        self.host.nicknames.remove(&uuid);
        self.host.tags.remove(&uuid);
        dispatch_player_logged_out(&mut self.host, &self.mods, uuid, &name);
        self.host.players.retain(|p| p.uuid != uuid);
        self.host.textures.remove(&uuid);
    }

    pub fn player(&self, uuid: u128) -> &NativeServerPlayer {
        self.host.players.iter().find(|p| p.uuid == uuid).unwrap()
    }

    pub fn player_mut(&mut self, uuid: u128) -> &mut NativeServerPlayer {
        self.host
            .players
            .iter_mut()
            .find(|p| p.uuid == uuid)
            .unwrap()
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

    /// Ticks until `done`, which has to happen within a few seconds (a lookup on a thread).
    pub fn tick_until(&mut self, what: &str, done: impl Fn(&Rig) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done(self) {
            assert!(
                std::time::Instant::now() < deadline,
                "{what} did not happen"
            );
            self.tick();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Types a line at the console. Whether the mod took it.
    pub fn console(&mut self, line: &str) -> bool {
        let mut ran = false;
        dispatch_perform_console_command(&mut self.host, &self.mods, line, &mut ran);
        ran
    }

    /// A player types a line. Whether the mod took it.
    pub fn say(&mut self, uuid: u128, line: &str) -> bool {
        let player = self.player(uuid).clone();
        let mut ran = false;
        dispatch_perform_command(&mut self.host, &self.mods, &player, line, &mut ran);
        ran
    }

    /// A player types a command, and the tick that does it comes: what it answered at once.
    pub fn run(&mut self, uuid: u128, line: &str) -> Vec<String> {
        self.host.feedback.clear();
        assert!(self.say(uuid, line), "{line} is not the mod's");
        let answered = self.host.feedback.clone();
        self.tick();
        answered
    }

    /// The console types a command, and the tick comes.
    pub fn run_console(&mut self, line: &str) -> Vec<String> {
        self.host.feedback.clear();
        assert!(self.console(line), "{line} is not the mod's");
        let answered = self.host.feedback.clone();
        self.tick();
        answered
    }

    /// What the mod answers at once to a line, without the tick.
    pub fn answer(&mut self, uuid: u128, line: &str) -> Vec<String> {
        self.host.feedback.clear();
        assert!(self.say(uuid, line), "{line} is not the mod's");
        self.host.feedback.clone()
    }

    /// Writes a new config and has the mod read it (the console's `nick reload`).
    pub fn set_config(&mut self, config: &str) {
        std::fs::write(self.dir.join("config").join("nick.toml"), config).unwrap();
        assert!(self.console("nick reload"));
    }

    pub fn config_file(&self, name: &str) -> PathBuf {
        self.dir.join("config").join(name)
    }

    /// The nickname the mod holds for a player.
    pub fn nickname(&self, uuid: u128) -> Option<String> {
        self.nick
            .state
            .lock()
            .unwrap()
            .active
            .get(&uuid)
            .map(|a| a.nickname.clone())
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

    /// What completing `line` (without its slash) offers a player.
    pub fn completions(&mut self, uuid: u128, line: &str) -> Vec<String> {
        let name = self.player(uuid).name.clone();
        let words: Vec<&str> = line.split(' ').collect();
        let arguments: Vec<String> = words[1..].iter().map(|w| (*w).to_owned()).collect();
        let start = line.len() - words.last().map_or(0, |w| w.len());
        let mut suggestions: Vec<Suggestion> = Vec::new();
        dispatch_tab_complete_v2(
            &mut self.host,
            &self.mods,
            uuid,
            &name,
            words[0],
            &arguments,
            [start, line.len()],
            &mut suggestions,
        );
        suggestions.into_iter().map(|s| s.text).collect()
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

    const BOBS_ID: &str = "00000000-0000-0000-0000-000000000b0b";
    const ALICES_ID: &str = "00000000-0000-0000-0000-0000000a11ce";

    fn names(rig: &Rig, uuids: &[u128]) -> Vec<Option<String>> {
        uuids.iter().map(|u| rig.nickname(*u)).collect()
    }

    /// What a refused command answers: the header, then the command as far as the word at fault
    /// (its last ten characters, after dots, when it is longer) and why, in brackets.
    fn refused(context: &str, why: &str) -> [String; 2] {
        [
            "Unknown or incomplete command. See below for error".to_owned(),
            format!("{context}<--[{why}]"),
        ]
    }

    // ---- setting a nickname --------------------------------------------------------------

    #[test]
    fn a_command_only_queues_and_the_tick_names_the_player_then_gives_the_skin() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.answer(ALICE, "/nick Captain"),
            ["Your nickname is now Captain!"]
        );
        assert!(
            rig.host.calls.is_empty(),
            "nothing is done inside the command"
        );
        rig.tick();
        assert_eq!(rig.host.nicknames[&ALICE], "Captain");
        assert_eq!(rig.host.tags[&ALICE], "Captain");
        assert_eq!(rig.host.textures[&ALICE], skin_of("Captain"));
        assert_eq!(
            rig.host.calls,
            [
                Call::Nickname(ALICE, Some("Captain".into())),
                Call::Tag(ALICE, Some("Captain".into())),
                Call::Property(ALICE, skin_of("Captain")),
                Call::Refresh(ALICE),
            ],
            "the name, then the tag, then the skin, then the player's view of it"
        );
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Alice now wears the skin of Captain!"]
        );
        assert_eq!(
            rig.host.logged(),
            [
                format!("INFO: Nick: Alice set Alice ({ALICES_ID}) to Captain"),
                format!("INFO: Nick: Alice ({ALICES_ID}) now has the skin of Captain"),
            ]
        );
    }

    #[test]
    fn a_nickname_for_another_player_tells_the_operator_and_the_player() {
        let mut rig = Rig::with_players();
        assert_eq!(rig.answer(ALICE, "nick Bob Rex"), ["Nicknamed Bob as Rex!"]);
        rig.tick();
        assert_eq!(rig.host.nicknames[&BOB], "Rex");
        assert_eq!(rig.host.tags[&BOB], "Rex");
        assert_eq!(rig.host.textures[&BOB], skin_of("Rex"));
        assert_eq!(rig.host.told_to(ALICE), ["Bob now wears the skin of Rex!"]);
        assert_eq!(
            rig.host.told_to(BOB),
            ["Your nickname is now Rex"],
            "the player is told what they are now, by someone else"
        );
        assert!(!rig.host.nicknames.contains_key(&ALICE), "only Bob");
    }

    #[test]
    fn a_name_that_is_an_account_takes_the_accounts_capitalisation() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick bob captain");
        assert_eq!(rig.host.nicknames[&BOB], "Captain");
        assert_eq!(rig.host.tags[&BOB], "Captain");
        assert_eq!(rig.nickname(BOB).as_deref(), Some("Captain"));
        assert_eq!(
            rig.host.calls[..4],
            [
                Call::Nickname(BOB, Some("captain".into())),
                Call::Tag(BOB, Some("captain".into())),
                Call::Nickname(BOB, Some("Captain".into())),
                Call::Tag(BOB, Some("Captain".into())),
            ],
            "named at once, then as the account spells it"
        );
    }

    #[test]
    fn a_name_no_account_has_keeps_the_players_skin_and_says_so() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick Bob Zork");
        assert_eq!(rig.host.nicknames[&BOB], "Zork");
        assert_eq!(rig.host.textures[&BOB], own_skin("Bob"));
        assert!(
            !rig.host
                .calls
                .iter()
                .any(|c| matches!(c, Call::Property(..) | Call::Refresh(_))),
            "no skin change, so no blink"
        );
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Bob keeps their skin: no account is called Zork"]
        );
    }

    #[test]
    fn services_that_do_not_answer_leave_the_skin_and_say_so() {
        let mut rig = Rig::with_players();
        rig.lookup.down.store(true, Ordering::SeqCst);
        rig.run(ALICE, "nick Bob Captain");
        assert_eq!(
            rig.host.nicknames[&BOB], "Captain",
            "the name is not held up"
        );
        assert_eq!(rig.host.textures[&BOB], own_skin("Bob"));
        assert_eq!(
            rig.host.told_to(ALICE),
            [
                "Bob keeps their skin: Mojang's services did not answer for Captain, so its skin is unknown"
            ]
        );
        // They are asked again, and answer, the next time.
        rig.lookup.down.store(false, Ordering::SeqCst);
        rig.run(ALICE, "nick Bob Captain");
        assert_eq!(rig.host.textures[&BOB], skin_of("Captain"));
    }

    #[test]
    fn a_nickname_of_one_or_two_characters_is_allowed_and_a_longer_minimum_is_the_configs() {
        let mut rig = Rig::with_players();
        for nick in ["X", "ab"] {
            assert_eq!(
                rig.run(ALICE, &format!("nick Bob {nick}")),
                [format!("Nicknamed Bob as {nick}!")]
            );
            assert_eq!(rig.host.nicknames[&BOB], nick);
        }
        rig.set_config("min-length = 3\n");
        assert_eq!(
            rig.answer(ALICE, "nick Eve ab"),
            refused(
                "...ick Eve ab",
                "ab is too short for a nickname: at least 3 characters"
            )
        );
        rig.set_config("max-length = 5\n");
        assert_eq!(
            rig.answer(ALICE, "nick Eve sixsix"),
            refused(
                "...Eve sixsix",
                "sixsix is too long for a nickname: at most 5 characters"
            )
        );
    }

    #[test]
    fn a_nickname_that_does_not_follow_the_conventions_is_refused_and_nothing_happens() {
        let mut rig = Rig::with_players();
        for (line, context, said) in [
            (
                "nick Bob seventeen_chars_x",
                "...en_chars_x",
                "seventeen_chars_x is too long for a nickname: at most 16 characters",
            ),
            (
                "nick Bob Cap-tain",
                "...b Cap-tain",
                "Cap-tain cannot be a nickname: '-' is not allowed, only letters, digits and underscores are",
            ),
            (
                "nick Bob Capt\u{e9}n",
                "...Bob Capt\u{e9}n",
                "Capt\u{e9}n cannot be a nickname: '\u{e9}' is not allowed, only letters, digits and underscores are",
            ),
            (
                "nick Bob Off",
                "...ck Bob Off",
                "Off is a /nick word, so it cannot be a nickname",
            ),
            (
                "nick Bob RANDOM",
                "...Bob RANDOM",
                "RANDOM is a /nick word, so it cannot be a nickname",
            ),
        ] {
            assert_eq!(rig.answer(ALICE, line), refused(context, said), "{line}");
        }
        // Said alone, such a word is the command's: it cannot reach the one-word form.
        let usage = rig.answer(ALICE, "nick Pool");
        assert!(
            usage[1].starts_with("nick Pool<--[Usage: /nick "),
            "{usage:?}"
        );
        rig.tick();
        assert!(rig.host.calls.is_empty());
    }

    #[test]
    fn the_chat_filter_refuses_a_nickname_and_is_asked_again_where_it_is_out_of_reach() {
        let mut rig = Rig::with_players();
        rig.host.filter = vec!["bad".into()];
        assert_eq!(
            rig.answer(ALICE, "nick Bob BadName"),
            refused(
                "...ob BadName",
                "BadName is not allowed by the server's chat filter"
            )
        );
        // Out of reach when the command ran: allowed then, asked again by the tick.
        rig.host.filter_out_of_reach = true;
        assert_eq!(
            rig.answer(ALICE, "nick Bob BadName"),
            ["Nicknamed Bob as BadName!"]
        );
        rig.host.filter_out_of_reach = false;
        rig.tick();
        assert!(rig.host.nicknames.is_empty());
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Bob keeps their name: BadName is not allowed by the chat filter"]
        );
    }

    #[test]
    fn a_nickname_is_refused_when_a_player_or_a_nickname_has_the_name_in_any_case() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.answer(ALICE, "nick Bob alice"),
            refused(
                "...Bob alice",
                "alice is taken: Alice is a player on the server"
            )
        );
        rig.run(ALICE, "nick Bob Captain");
        assert_eq!(
            rig.answer(ALICE, "nick Eve captain"),
            refused(
                "...ve captain",
                "captain is taken: Bob has it as a nickname"
            )
        );
        // The player's own nickname is theirs to change, even to another capitalisation.
        assert_eq!(
            rig.answer(ALICE, "nick Bob CAPTAIN"),
            ["Nicknamed Bob as CAPTAIN!"]
        );
    }

    #[test]
    fn two_nicknames_asked_for_before_the_tick_do_not_both_get_the_name() {
        let mut rig = Rig::with_players();
        rig.answer(ALICE, "nick Bob Rex");
        rig.answer(ALICE, "nick Eve Rex");
        rig.tick();
        assert_eq!(names(&rig, &[BOB, EVE]), [Some("Rex".into()), None]);
        assert!(
            rig.host
                .told_to(ALICE)
                .contains(&"Eve keeps their name: Rex is taken (Bob has it)".to_owned())
        );
    }

    #[test]
    fn a_players_own_name_is_no_nickname_it_is_off() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.answer(ALICE, "nick Bob bob"),
            refused("...ck Bob bob", "Bob does not have a nickname set")
        );
        rig.run(ALICE, "nick Bob Captain");
        assert_eq!(
            rig.answer(ALICE, "nick Bob BOB"),
            ["Removed nicknames from 1 player!"]
        );
        rig.tick();
        assert_eq!(rig.nickname(BOB), None);
        assert!(rig.host.nicknames.is_empty());
    }

    #[test]
    fn a_target_who_is_not_on_the_server_is_an_error_and_so_is_the_console_without_one() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.answer(ALICE, "nick Zed Captain"),
            refused("nick Zed", "Player was not found")
        );
        assert_eq!(
            rig.answer(ALICE, "nick off Zed"),
            refused("...ck off Zed", "Player was not found")
        );
        rig.host.feedback.clear();
        assert!(rig.console("nick Captain"));
        assert_eq!(
            rig.host.feedback,
            refused(
                "...ck Captain",
                "The console has no nickname: name a player"
            )
        );
    }

    // ---- turning it off ------------------------------------------------------------------

    #[test]
    fn off_gives_back_the_name_and_the_skin_the_player_came_with() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick Bob Captain");
        rig.host.calls.clear();
        assert_eq!(
            rig.run(ALICE, "nick off Bob"),
            ["Removed nicknames from 1 player!"]
        );
        assert_eq!(
            rig.host.told_to(BOB).last().map(String::as_str),
            Some("Your nickname has been removed"),
            "the player is told, whoever took it off"
        );
        assert!(rig.host.nicknames.is_empty() && rig.host.tags.is_empty());
        assert_eq!(rig.host.textures[&BOB], own_skin("Bob"));
        assert_eq!(
            rig.host.calls,
            [
                Call::Nickname(BOB, None),
                Call::Tag(BOB, None),
                Call::Property(BOB, own_skin("Bob")),
                Call::Refresh(BOB),
            ]
        );
        assert_eq!(rig.nickname(BOB), None);
        assert_eq!(
            rig.host.logged().last().unwrap(),
            &format!("INFO: Nick: Alice cleared Bob's nickname Captain ({BOBS_ID})")
        );
    }

    #[test]
    fn a_player_who_came_with_no_skin_has_none_again_and_a_nickname_that_changed_no_skin_changes_none()
     {
        let mut rig = Rig::with_players();
        rig.host.textures.insert(BOB, None);
        rig.run(ALICE, "nick Bob Captain");
        assert_eq!(rig.host.textures[&BOB], skin_of("Captain"));
        rig.run(ALICE, "nick off Bob");
        assert_eq!(rig.host.textures[&BOB], None, "none, as they came");

        // No account, so no skin was put on Eve: off puts nothing back and refreshes nothing.
        rig.run(ALICE, "nick Eve Zork");
        rig.host.calls.clear();
        rig.run(ALICE, "nick off Eve");
        assert_eq!(
            rig.host.calls,
            [Call::Nickname(EVE, None), Call::Tag(EVE, None)]
        );
    }

    #[test]
    fn a_second_nickname_does_not_lose_the_skin_the_player_came_with() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick Bob Captain");
        rig.run(ALICE, "nick Bob Rex");
        assert_eq!(rig.host.textures[&BOB], skin_of("Rex"));
        // Then one with no account: the skin on them stays what it was.
        rig.run(ALICE, "nick Bob Zork");
        assert_eq!(rig.host.textures[&BOB], skin_of("Rex"));
        rig.run(ALICE, "nick off Bob");
        assert_eq!(rig.host.textures[&BOB], own_skin("Bob"));
    }

    #[test]
    fn off_works_by_the_real_name_and_by_the_nickname_and_says_when_there_is_none() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.answer(ALICE, "nick off"),
            refused("nick off", "You do not have a nickname set")
        );
        assert_eq!(
            rig.answer(ALICE, "nick off Bob"),
            refused("...ck off Bob", "Bob does not have a nickname set")
        );
        for typed in ["Bob", "bob", "Captain", "captain", "~Captain"] {
            rig.run(ALICE, "nick Bob Captain");
            assert_eq!(rig.nickname(BOB).as_deref(), Some("Captain"));
            rig.run(ALICE, &format!("nick off {typed}"));
            assert_eq!(rig.nickname(BOB), None, "off {typed}");
        }
        // Their own, with none.
        rig.run(ALICE, "nick Alice Rex");
        assert_eq!(
            rig.answer(ALICE, "nick off"),
            ["Removed nicknames from 1 player!"]
        );
    }

    // ---- who: selectors ------------------------------------------------------------------

    #[test]
    fn a_selector_for_many_skips_operators_and_the_admin_flag_means_only_them() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.run(ALICE, "nick random @a"),
            ["Choosing random nickname for 2 players..."]
        );
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Assigned random nicknames to 2 players!"],
            "one answer for the whole batch"
        );
        let (bob, eve) = (rig.nickname(BOB).unwrap(), rig.nickname(EVE).unwrap());
        assert_ne!(bob, eve, "two players, two names");
        assert_eq!(rig.nickname(ALICE), None, "operators are skipped");

        assert_eq!(
            rig.run(ALICE, "nick off @a"),
            ["Removed nicknames from 2 players!"]
        );
        assert_eq!(
            rig.answer(ALICE, "nick off @a"),
            refused("...ick off @a", "None of them has a nickname set")
        );

        rig.run(ALICE, "nick random --admin");
        assert!(rig.nickname(ALICE).is_some());
        assert_eq!(
            names(&rig, &[BOB, EVE]),
            [None, None],
            "--admin is the operators"
        );
        assert_eq!(
            rig.run(ALICE, "nick off --admin"),
            ["Removed nicknames from 1 player!"]
        );
        assert_eq!(rig.nickname(ALICE), None);
    }

    #[test]
    fn at_s_is_the_player_themselves_operator_or_not_and_a_name_works_on_an_operator() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick random @s");
        assert!(rig.nickname(ALICE).is_some(), "@s reaches an operator");
        assert_eq!(
            rig.answer(ALICE, "nick off @s"),
            ["Removed nicknames from 1 player!"]
        );
        rig.tick();
        rig.run(ALICE, "nick Alice Captain");
        assert_eq!(rig.nickname(ALICE).as_deref(), Some("Captain"));
        // An operator can be nicknamed by name from another.
        rig.join("Dan", 0xda4, 4);
        rig.run(0xda4, "nick Alice Rex");
        assert_eq!(rig.nickname(ALICE).as_deref(), Some("Rex"));
    }

    #[test]
    fn at_p_is_the_nearest_player_who_is_not_an_operator_in_the_same_level() {
        let mut rig = Rig::with_players();
        rig.join("Carol", CAROL, 0);
        rig.player_mut(ALICE).position = [0.0, 64.0, 0.0];
        rig.player_mut(BOB).position = [10.0, 64.0, 0.0];
        rig.player_mut(EVE).position = [3.0, 64.0, 0.0];
        rig.player_mut(CAROL).position = [1.0, 64.0, 1.0];
        rig.player_mut(CAROL).dimension = "minecraft:the_nether".into();
        rig.run(ALICE, "nick random @p");
        assert_eq!(
            names(&rig, &[BOB, EVE, CAROL])
                .iter()
                .filter(|n| n.is_some())
                .count(),
            1
        );
        assert!(
            rig.nickname(EVE).is_some(),
            "Eve is the nearest in Alice's level"
        );
        // From the console there is no one for @p to be near.
        rig.host.feedback.clear();
        assert!(rig.console("nick off @p"));
        assert_eq!(
            rig.host.feedback,
            refused("...ick off @p", "The console is no player: name one")
        );
    }

    #[test]
    fn at_r_is_one_player_who_is_not_an_operator() {
        let mut rig = Rig::with_players();
        for _ in 0..6 {
            rig.run(ALICE, "nick random @r");
            assert_eq!(rig.nickname(ALICE), None);
            let named = names(&rig, &[BOB, EVE])
                .iter()
                .filter(|n| n.is_some())
                .count();
            assert_eq!(named, 1);
            rig.run(ALICE, "nick off @a");
        }
    }

    #[test]
    fn a_selector_that_cannot_be_read_says_what_is_supported() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.answer(ALICE, "nick off @e"),
            refused("...ick off @e", "Only @a, @p, @r and @s are supported here")
        );
        assert_eq!(
            rig.answer(ALICE, "nick off @a[distance=..5]"),
            refused(
                "...tance=..5]",
                "Selector options are not supported here: use @a, @p, @r, @s or a name"
            )
        );
        assert_eq!(
            rig.answer(ALICE, "nick @a Captain"),
            refused(
                "nick @a",
                "One player only here: name one, or use @s, @p or @r"
            )
        );
        rig.host.feedback.clear();
        assert!(rig.console("nick @s Captain"));
        assert_eq!(
            rig.host.feedback,
            refused("nick @s", "The console is no player: name one")
        );
        // Only operators online: @a has no one.
        let mut ops = Rig::plain();
        ops.join("Alice", ALICE, 4);
        assert_eq!(
            ops.answer(ALICE, "nick random @a"),
            refused(
                "...random @a",
                "No player was found: operators are skipped, use /nick random --admin to target them"
            )
        );
        let mut none = Rig::plain();
        none.join("Bob", BOB, 0);
        none.host.feedback.clear();
        assert!(none.console("nick off --admin"));
        assert_eq!(
            none.host.feedback,
            refused("...ff --admin", "No operator is online")
        );
    }

    #[test]
    fn the_level_that_counts_as_an_operator_is_the_configs() {
        let mut rig = Rig::new("op-level = 3\n", "Rex\nZed\nMax\nAda\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Dan", 0xda4, 2);
        rig.join("Bob", BOB, 0);
        rig.run(ALICE, "nick random @a");
        assert_eq!(
            names(&rig, &[BOB, 0xda4])
                .iter()
                .filter(|n| n.is_some())
                .count(),
            2
        );
        assert_eq!(rig.nickname(ALICE), None);
    }

    // ---- random --------------------------------------------------------------------------

    #[test]
    fn a_random_nickname_is_a_pool_name_with_its_skin_which_is_kept_for_good() {
        let mut rig = Rig::new("", "Rex\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        assert_eq!(
            rig.run(ALICE, "nick random Bob"),
            ["Choosing random nickname for Bob..."]
        );
        assert_eq!(rig.host.nicknames[&BOB], "Rex");
        assert_eq!(rig.host.textures[&BOB], skin_of("Rex"));
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Assigned random nickname Rex to Bob!"]
        );
        assert_eq!(rig.host.told_to(BOB), ["Your nickname is now Rex"]);
        assert!(rig.host.logged().contains(&format!(
            "INFO: Nick: Alice set Bob ({BOBS_ID}) to Rex (random)"
        )));
        // The skin is in the file for the next time, and the services are not asked again.
        let pins = std::fs::read_to_string(rig.config_file("nick-skins.json")).unwrap();
        assert!(
            pins.contains("\"rex\"") && pins.contains("skin-of-Rex"),
            "{pins}"
        );
        assert_eq!(rig.lookup.asked_for(), ["Rex"]);
        rig.run(ALICE, "nick off Bob");
        rig.run(ALICE, "nick random Bob");
        assert_eq!(rig.nickname(BOB).as_deref(), Some("Rex"));
        assert_eq!(rig.lookup.asked_for(), ["Rex"], "not asked for again");
    }

    #[test]
    fn a_pinned_skin_works_with_the_services_down() {
        let mut rig = Rig::new("", "Rex\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.run(ALICE, "nick random Bob");
        rig.run(ALICE, "nick off Bob");
        rig.lookup.down.store(true, Ordering::SeqCst);
        rig.run(ALICE, "nick random Bob");
        assert_eq!(rig.host.textures[&BOB], skin_of("Rex"));
    }

    #[test]
    fn a_name_is_not_drawn_twice_until_the_whole_pool_has_been_used() {
        let mut rig = Rig::new("", "Rex\nZed\nMax\nAlex\nSteve\nAda\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        let mut drawn = Vec::new();
        for _ in 0..6 {
            rig.run(ALICE, "nick random Bob");
            drawn.push(rig.nickname(BOB).unwrap());
            rig.run(ALICE, "nick off Bob");
        }
        let mut sorted = drawn.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            ["Ada", "Alex", "Max", "Rex", "Steve", "Zed"],
            "{drawn:?}"
        );
        // A new round: any of them.
        rig.run(ALICE, "nick random Bob");
        assert!(drawn.contains(&rig.nickname(BOB).unwrap()));
    }

    #[test]
    fn names_in_use_and_names_that_are_no_account_are_never_drawn() {
        // Rex is a player on the server, Zed another's nickname: only Max is free.
        let mut rig = Rig::new("", "Rex\nZed\nMax\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.join("Rex", 0x2e8, 0);
        rig.join("Eve", EVE, 0);
        rig.run(ALICE, "nick Eve Zed");
        for _ in 0..4 {
            rig.run(ALICE, "nick random Bob");
            assert_eq!(rig.nickname(BOB).as_deref(), Some("Max"));
            rig.run(ALICE, "nick off Bob");
        }
        // A pool name that is no account is found out once and left alone.
        let mut ghosts = Rig::new("", "Ghost\nZed\n");
        ghosts.join("Alice", ALICE, 4);
        ghosts.join("Bob", BOB, 0);
        for _ in 0..4 {
            ghosts.run(ALICE, "nick random Bob");
            assert_eq!(ghosts.nickname(BOB).as_deref(), Some("Zed"));
            ghosts.run(ALICE, "nick off Bob");
        }
        assert_eq!(
            ghosts.answer(ALICE, "nick pool check"),
            [
                "The pool has 2 names: 1 with their skin kept, 1 not looked up yet",
                "1 name found to be no account this session",
            ]
        );
        let asked = ghosts.lookup.asked_for();
        assert_eq!(
            asked.iter().filter(|n| *n == "Ghost").count(),
            1,
            "{asked:?}"
        );
    }

    #[test]
    fn many_random_nicknames_at_once_get_different_names_and_a_pool_too_small_says_so() {
        let mut rig = Rig::new("", "Rex\nZed\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.join("Eve", EVE, 0);
        rig.join("Carol", CAROL, 0);
        rig.run(ALICE, "nick random @a");
        let mut got: Vec<String> = names(&rig, &[BOB, EVE, CAROL])
            .into_iter()
            .flatten()
            .collect();
        got.sort();
        assert_eq!(got, ["Rex", "Zed"], "two names for three players");
        assert_eq!(
            rig.host.told_to(ALICE),
            [
                "Assigned random nicknames to 2 players!",
                "1 player could not be given one: the pool ran out of free names or Mojang's services did not answer",
            ],
            "one answer for the batch, with how many were left out"
        );
    }

    #[test]
    fn a_batch_answers_once_and_each_player_is_told_their_own_nickname() {
        let mut rig = Rig::new("", "Rex\nZed\nMax\nAda\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.join("Eve", EVE, 0);
        rig.join("Carol", CAROL, 0);
        assert_eq!(
            rig.run(ALICE, "nick random @a"),
            ["Choosing random nickname for 3 players..."]
        );
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Assigned random nicknames to 3 players!"]
        );
        for player in [BOB, EVE, CAROL] {
            let told = rig.host.told_to(player);
            let nickname = rig.nickname(player).unwrap();
            assert_eq!(told, [format!("Your nickname is now {nickname}")]);
        }
        rig.host.told.clear();
        assert_eq!(
            rig.run(ALICE, "nick off @a"),
            ["Removed nicknames from 3 players!"]
        );
        for player in [BOB, EVE, CAROL] {
            assert_eq!(rig.host.told_to(player), ["Your nickname has been removed"]);
        }
        // Someone who leaves before their turn is not waited for.
        rig.host.told.clear();
        rig.answer(ALICE, "nick random @a");
        rig.leave(CAROL);
        rig.tick();
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Assigned random nicknames to 2 players!"]
        );
    }

    #[test]
    fn a_refused_command_reads_as_the_command_so_far_and_what_is_wrong_with_it() {
        let mut rig = Rig::with_players();
        // The example of the design: the last ten characters, from the fault.
        let mut ops = Rig::plain();
        ops.join("Alice", ALICE, 4);
        assert_eq!(
            ops.answer(ALICE, "/nick  random   @a"),
            refused(
                "...random @a",
                "No player was found: operators are skipped, use /nick random --admin to target them"
            ),
            "folded to single spaces, with no slash"
        );
        assert_eq!(
            rig.answer(ALICE, "nick Zed Rex"),
            refused("nick Zed", "Player was not found")
        );
    }

    #[test]
    fn an_empty_pool_and_a_used_up_pool_say_so() {
        let mut rig = Rig::new("", "");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.run(ALICE, "nick random Bob");
        assert_eq!(
            rig.host.told_to(ALICE),
            ["The pool is empty: add names with /nick pool add"]
        );
        assert_eq!(rig.nickname(BOB), None);
    }

    #[test]
    fn services_that_do_not_answer_for_three_names_in_a_row_end_the_random_nickname() {
        let mut rig = Rig::new("", "Rex\nZed\nMax\nAda\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.lookup.down.store(true, Ordering::SeqCst);
        rig.run(ALICE, "nick random Bob");
        assert_eq!(rig.nickname(BOB), None);
        assert_eq!(
            rig.host.told_to(ALICE),
            [
                "Mojang's services are not answering, so no random nickname could be chosen for Bob. Try again in a minute"
            ]
        );
        let asked = rig.lookup.asked_for();
        assert_eq!(asked.len(), 3, "{asked:?}");
        assert_eq!(
            asked.iter().collect::<std::collections::HashSet<_>>().len(),
            3,
            "three different names, not one asked three times"
        );
        // Back up, they work, the names that were tried are still in the pool.
        rig.lookup.down.store(false, Ordering::SeqCst);
        rig.run(ALICE, "nick random Bob");
        assert!(rig.nickname(BOB).is_some());
    }

    // ---- info and list -------------------------------------------------------------------

    #[test]
    fn info_gives_the_real_identity_by_nickname_or_by_name() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick Bob Captain");
        let said = [
            format!("Captain is Bob ({BOBS_ID})"),
            "Nicknamed by Alice, skin: the skin of Captain".to_owned(),
        ];
        assert_eq!(rig.answer(ALICE, "nick info Captain"), said);
        assert_eq!(rig.answer(ALICE, "nick info captain"), said);
        assert_eq!(rig.answer(ALICE, "nick info bob"), said);
        assert_eq!(rig.answer(ALICE, "nick info Eve"), ["Eve is not nicknamed"]);
        assert_eq!(
            rig.answer(ALICE, "nick info Zed"),
            refused("...k info Zed", "No player is nicknamed or called Zed")
        );
        // A nickname with no skin of its own says so.
        rig.run(ALICE, "nick Eve Zork");
        assert!(rig.answer(ALICE, "nick info Zork")[1].ends_with("skin: their own"));
    }

    #[test]
    fn list_names_everyone_with_a_nickname() {
        let mut rig = Rig::with_players();
        assert_eq!(rig.answer(ALICE, "nick list"), ["No one is nicknamed"]);
        rig.run(ALICE, "nick Bob Captain");
        rig.console("nick Eve Rex");
        rig.tick();
        assert_eq!(
            rig.answer(ALICE, "nick list"),
            [
                "Nicknames in force: 2".to_owned(),
                format!("Captain = Bob ({BOBS_ID}), by Alice, skin: the skin of Captain"),
                "Rex = Eve (00000000-0000-0000-0000-000000000e7e), by console, skin: the skin of Rex"
                    .to_owned(),
            ]
        );
    }

    // ---- leaving, joining ----------------------------------------------------------------

    #[test]
    fn a_nickname_ends_when_the_player_leaves_and_is_not_there_when_they_come_back() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick Bob Captain");
        rig.leave(BOB);
        assert_eq!(rig.nickname(BOB), None);
        rig.join("Bob", BOB, 0);
        rig.tick();
        assert!(rig.host.nicknames.is_empty());
        assert_eq!(rig.answer(ALICE, "nick info Bob"), ["Bob is not nicknamed"]);
        assert_eq!(rig.host.textures[&BOB], own_skin("Bob"));
        assert_eq!(
            rig.answer(ALICE, "nick off Bob"),
            refused("...ck off Bob", "Bob does not have a nickname set")
        );
    }

    #[test]
    fn work_for_a_player_who_left_before_the_tick_is_dropped() {
        let mut rig = Rig::with_players();
        rig.answer(ALICE, "nick Bob Captain");
        rig.answer(ALICE, "nick random Eve");
        rig.leave(BOB);
        rig.leave(EVE);
        rig.tick();
        assert!(rig.host.calls.is_empty());
        assert!(rig.host.told_to(ALICE).is_empty());
    }

    #[test]
    fn a_player_who_joins_with_the_name_of_a_nickname_takes_it_and_the_operators_are_told() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick Bob Zed");
        rig.host.told.clear();
        rig.join("zed", 0x2ed, 0);
        rig.tick();
        assert_eq!(rig.nickname(BOB), None, "Bob is Bob again");
        assert_eq!(rig.host.textures[&BOB], own_skin("Bob"));
        assert!(rig.host.nicknames.is_empty());
        assert_eq!(
            rig.host.told_to(ALICE),
            ["zed joined with the name of Bob's nickname, so Bob is no longer nicknamed Zed"]
        );
        assert_eq!(rig.host.told_to(BOB), ["Your nickname has been removed"]);
        assert!(rig.host.told_to(EVE).is_empty());
        assert!(rig.host.logged().iter().any(|l| l
            == &format!(
                "INFO: Nick: cleared, as zed joined with that name, Bob's nickname Zed ({BOBS_ID})"
            )));
    }

    #[test]
    fn a_skin_that_comes_after_the_nickname_changed_is_not_put_on() {
        let mut rig = Rig::new_in_background("", "Rex\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.lookup.held.store(true, Ordering::SeqCst);
        rig.run(ALICE, "nick Bob Captain");
        assert_eq!(rig.host.nicknames[&BOB], "Captain");
        rig.run(ALICE, "nick Bob Rex");
        rig.lookup.held.store(false, Ordering::SeqCst);
        rig.tick_until("the skin of Rex", |r| {
            r.host.textures[&BOB] == skin_of("Rex")
        });
        rig.ticks(5);
        assert_eq!(rig.host.nicknames[&BOB], "Rex");
        assert!(
            !rig.host
                .calls
                .contains(&Call::Property(BOB, skin_of("Captain"))),
            "Captain's skin is for a nickname Bob no longer has"
        );
    }

    #[test]
    fn lookups_on_worker_threads_give_the_same_nickname() {
        let mut rig = Rig::new_in_background("", "Rex\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.answer(ALICE, "nick Bob Captain");
        rig.tick_until("the skin", |r| r.host.textures[&BOB] == skin_of("Captain"));
        assert_eq!(rig.host.nicknames[&BOB], "Captain");
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Bob now wears the skin of Captain!"]
        );
    }

    // ---- the pool ------------------------------------------------------------------------

    #[test]
    fn an_account_is_added_to_the_pool_with_its_skin_and_nothing_else_is() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.run(ALICE, "nick pool add Notch"),
            ["Checking that Notch is an account..."]
        );
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Added Notch to the pool, with its skin kept (5 names now)!"]
        );
        let file = std::fs::read_to_string(rig.config_file("nick-pool.txt")).unwrap();
        assert!(file.lines().any(|l| l == "Notch"), "{file}");
        let pins = std::fs::read_to_string(rig.config_file("nick-skins.json")).unwrap();
        assert!(pins.contains("\"notch\""), "{pins}");

        rig.host.told.clear();
        assert_eq!(
            rig.answer(ALICE, "nick pool add NOTCH"),
            ["Notch is in the pool already"]
        );
        assert_eq!(
            rig.answer(ALICE, "nick pool add has/slash"),
            refused(
                "...has/slash",
                "has/slash is not an account name: 1 to 16 letters, digits or underscores"
            )
        );
        rig.run(ALICE, "nick pool add Ghost");
        assert_eq!(
            rig.host.told_to(ALICE),
            ["No account is called ghost: the pool takes only accounts that exist"]
        );
        rig.host.told.clear();
        rig.lookup.down.store(true, Ordering::SeqCst);
        rig.run(ALICE, "nick pool add Steve");
        assert_eq!(
            rig.host.told_to(ALICE),
            ["Mojang's services did not answer, so steve was not checked or added. Try again"]
        );
        let file = std::fs::read_to_string(rig.config_file("nick-pool.txt")).unwrap();
        assert!(!file.contains("Ghost") && !file.contains("Steve"), "{file}");
    }

    #[test]
    fn a_name_is_taken_out_of_the_pool_and_the_file_and_its_skin_is_forgotten() {
        let mut rig = Rig::new("", "Rex\n");
        rig.join("Alice", ALICE, 4);
        rig.join("Bob", BOB, 0);
        rig.run(ALICE, "nick random Bob");
        rig.run(ALICE, "nick off Bob");
        assert_eq!(
            rig.answer(ALICE, "nick pool remove REX"),
            ["Removed Rex from the pool (0 names left)!"]
        );
        let file = std::fs::read_to_string(rig.config_file("nick-pool.txt")).unwrap();
        assert!(!file.lines().any(|l| l == "Rex"), "{file}");
        rig.tick();
        let pins = std::fs::read_to_string(rig.config_file("nick-skins.json")).unwrap();
        assert!(!pins.contains("rex"), "{pins}");
        assert_eq!(
            rig.answer(ALICE, "nick pool remove Rex"),
            refused("...remove Rex", "Rex is not in the pool")
        );
        rig.run(ALICE, "nick random Bob");
        assert_eq!(rig.nickname(BOB), None, "the pool is empty now");
    }

    #[test]
    fn a_pool_check_counts_the_names_and_says_about_one() {
        let mut rig = Rig::with_players();
        assert_eq!(
            rig.answer(ALICE, "nick pool check"),
            ["The pool has 4 names: 0 with their skin kept, 4 not looked up yet"]
        );
        rig.run(ALICE, "nick random Bob");
        let pinned = rig.nickname(BOB).unwrap();
        assert_eq!(
            rig.answer(ALICE, "nick pool check"),
            ["The pool has 4 names: 1 with their skin kept, 3 not looked up yet"]
        );
        // One that is kept is answered at once.
        assert_eq!(
            rig.answer(
                ALICE,
                &format!("nick pool check {}", pinned.to_ascii_lowercase())
            ),
            [format!(
                "{pinned} is in the pool; its skin is kept (the account {pinned})"
            )]
        );
        // One that is not is looked up.
        rig.host.told.clear();
        let other = ["Rex", "Zed", "Max", "Ada"]
            .into_iter()
            .find(|n| *n != pinned)
            .unwrap();
        assert_eq!(
            rig.run(ALICE, &format!("nick pool check {other}")),
            [format!("Checking {other}...")]
        );
        assert_eq!(
            rig.host.told_to(ALICE),
            [format!(
                "{other} is in the pool; the account {other} exists and its skin is kept"
            )]
        );
        rig.host.told.clear();
        rig.run(ALICE, "nick pool check Notch");
        rig.run(ALICE, "nick pool check Ghost");
        assert_eq!(
            rig.host.told_to(ALICE),
            [
                "notch is not in the pool; the account Notch exists and its skin is not kept (not in the pool)",
                "ghost is not in the pool; no account is called ghost, so it cannot be used",
            ]
        );
    }

    // ---- config, reload, registration ----------------------------------------------------

    #[test]
    fn a_reload_reads_everything_again_and_a_file_that_does_not_read_changes_nothing() {
        let mut rig = Rig::with_players();
        std::fs::write(
            rig.config_file("nick-pool.txt"),
            "Notch\nSteve\nnot a name\n",
        )
        .unwrap();
        assert_eq!(
            rig.run_console("nick reload"),
            [
                "Reloaded the nick config: 2 names in the pool, 0 skins kept!",
                "1 line(s) of nick-pool.txt are not account names and are left out",
            ]
        );
        std::fs::write(rig.config_file("nick.toml"), "enabled = [").unwrap();
        let said = rig.run_console("nick reload");
        assert!(
            said[0].starts_with("Could not read the files, keeping the old ones: "),
            "{said:?}"
        );
        assert!(
            rig.host
                .logged()
                .iter()
                .any(|l| l.starts_with("WARN: Nick: could not read the files"))
        );
        assert_eq!(
            rig.answer(ALICE, "nick pool check"),
            ["The pool has 2 names: 0 with their skin kept, 2 not looked up yet"]
        );
    }

    #[test]
    fn switched_off_every_nickname_is_taken_off_and_the_command_says_so_until_it_is_on_again() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick Bob Captain");
        rig.run(ALICE, "nick Eve Rex");
        rig.set_config("enabled = false\n");
        rig.tick();
        assert!(rig.host.nicknames.is_empty() && rig.host.tags.is_empty());
        assert_eq!(rig.host.textures[&BOB], own_skin("Bob"));
        assert_eq!(rig.nickname(BOB), None);
        assert_eq!(
            rig.host.told_to(BOB).last().map(String::as_str),
            Some("Your nickname has been removed")
        );
        let off = "Nicknames are turned off (enabled = false in config/nick.toml): /nick reload reads it again";
        assert_eq!(
            rig.answer(ALICE, "nick Bob Rex"),
            refused("...ck Bob Rex", off)
        );
        assert_eq!(rig.answer(ALICE, "nick list"), refused("nick list", off));
        rig.set_config("enabled = true\n");
        assert_eq!(rig.answer(ALICE, "nick Bob Rex"), ["Nicknamed Bob as Rex!"]);
    }

    #[test]
    fn the_command_is_registered_for_the_configs_level_and_a_new_level_reaches_the_players() {
        let mut rig = Rig::with_players();
        let usages = rig.registered();
        assert_eq!(usages.len(), 16);
        assert!(usages.iter().all(|(usage, level, node)| {
            usage.starts_with("nick") && *level == 2 && node.as_deref() == Some("nick.admin")
        }));
        assert_eq!(rig.host.refreshed_commands, 0);
        rig.set_config("command-level = 3\n");
        assert_eq!(
            rig.host.refreshed_commands, 1,
            "the players' trees are sent again"
        );
        assert!(rig.registered().iter().all(|(_, level, _)| *level == 3));
        // A reload that does not change it sends nothing.
        rig.set_config("command-level = 3\n");
        assert_eq!(rig.host.refreshed_commands, 1);
    }

    #[test]
    fn a_line_of_the_mod_is_taken_and_any_other_is_left_alone() {
        let mut rig = Rig::with_players();
        assert!(!rig.console("gamemode creative"));
        assert!(!rig.say(ALICE, "nickname"));
        assert_eq!(
            rig.answer(ALICE, "nick"),
            refused(
                "nick",
                "Usage: /nick <name> | <player> <name> | off [players|--admin] | random [players|--admin] | info <nickname> | list | pool add|remove|check [name] | reload"
            )
        );
    }

    #[test]
    fn the_first_hook_to_run_writes_the_files_a_new_server_has_none_of() {
        let mut rig = Rig::plain();
        std::fs::remove_file(rig.config_file("nick.toml")).unwrap();
        std::fs::remove_file(rig.config_file("nick-pool.txt")).unwrap();
        rig.join("Bob", BOB, 0);
        assert_eq!(
            std::fs::read_to_string(rig.config_file("nick.toml")).unwrap(),
            crate::config::DEFAULT_FILE
        );
        let pool = std::fs::read_to_string(rig.config_file("nick-pool.txt")).unwrap();
        assert!(
            pool.lines().filter(|l| !l.starts_with('#')).count() >= 250,
            "{pool}"
        );
    }

    // ---- nicknames on join ---------------------------------------------------------------

    const POOL: &str = "Rex\nZed\nMax\nAda\n";

    fn joined_rig() -> Rig {
        let mut rig = Rig::fresh("", POOL);
        rig.join("Alice", ALICE, 4);
        rig
    }

    #[test]
    fn a_server_starts_with_nicknames_on_join_on_and_the_player_is_told_theirs() {
        let mut rig = joined_rig();
        rig.join("Bob", BOB, 0);
        assert_eq!(rig.nickname(BOB), None, "not before the tick");
        rig.tick();
        let nickname = rig.nickname(BOB).expect("a player gets one on joining");
        assert!(POOL.lines().any(|n| n == nickname), "{nickname}");
        assert_eq!(rig.host.nicknames[&BOB], nickname);
        assert_eq!(rig.host.textures[&BOB], skin_of(&nickname));
        assert_eq!(
            rig.host.told_to(BOB),
            [format!(
                "You have automatically been assigned a nickname upon join. Your nickname is {nickname}"
            )]
        );
        // On is for players: the operator who joined first kept their name, and was told nothing.
        assert_eq!(rig.nickname(ALICE), None);
        assert!(rig.host.told_to(ALICE).is_empty());
        assert!(rig.host.logged().contains(&format!(
            "INFO: Nick: the server on join set Bob ({BOBS_ID}) to {nickname} (random)"
        )));
    }

    #[test]
    fn off_gives_no_one_a_nickname_and_all_gives_the_operators_one_too() {
        let mut rig = joined_rig();
        assert_eq!(
            rig.run_console("nickonjoin off"),
            ["Random nicknames on join set to off (no one)!"]
        );
        rig.join("Bob", BOB, 0);
        rig.join("Dan", 0xda4, 4);
        rig.ticks(3);
        assert_eq!(rig.nickname(BOB), None);
        assert_eq!(rig.nickname(0xda4), None);
        assert!(rig.host.nicknames.is_empty() && rig.host.told.is_empty());

        assert_eq!(
            rig.run_console("nickonjoin all"),
            ["Random nicknames on join set to all (everyone, operators too)!"]
        );
        rig.join("Eve", EVE, 0);
        rig.join("Carol", CAROL, 2);
        rig.ticks(3);
        assert!(rig.nickname(EVE).is_some(), "a player");
        assert!(rig.nickname(CAROL).is_some(), "an operator, with all");
        assert_eq!(rig.nickname(BOB), None, "who joined before keeps none");
        let told = rig.host.told_to(CAROL);
        assert_eq!(told.len(), 1);
        assert!(told[0].starts_with(
            "You have automatically been assigned a nickname upon join. Your nickname is "
        ));
    }

    #[test]
    fn on_leaves_the_operators_alone_whoever_asked() {
        let mut rig = joined_rig();
        rig.run_console("nickonjoin on");
        rig.join("Dan", 0xda4, 4);
        rig.join("Bob", BOB, 0);
        rig.ticks(3);
        assert_eq!(rig.nickname(0xda4), None);
        assert!(rig.nickname(BOB).is_some());
    }

    #[test]
    fn a_player_who_rejoins_with_it_off_comes_back_under_their_own_name() {
        let mut rig = joined_rig();
        rig.join("Bob", BOB, 0);
        rig.tick();
        assert!(rig.nickname(BOB).is_some());
        rig.leave(BOB);
        rig.run_console("nickonjoin off");
        rig.join("Bob", BOB, 0);
        rig.ticks(3);
        assert_eq!(rig.nickname(BOB), None);
        assert!(rig.host.nicknames.is_empty() && rig.host.tags.is_empty());
        assert_eq!(rig.host.textures[&BOB], own_skin("Bob"));
    }

    #[test]
    fn a_player_who_rejoins_with_it_on_is_given_a_new_nickname() {
        let mut rig = joined_rig();
        rig.join("Bob", BOB, 0);
        rig.tick();
        let first = rig.nickname(BOB).unwrap();
        rig.leave(BOB);
        assert_eq!(rig.nickname(BOB), None, "it ended with their session");
        rig.host.told.clear();
        rig.join("Bob", BOB, 0);
        rig.tick();
        let second = rig.nickname(BOB).expect("a nickname again");
        assert_ne!(first, second, "a name is not drawn twice in a round");
        assert_eq!(rig.host.nicknames[&BOB], second);
        assert_eq!(
            rig.host.told_to(BOB),
            [format!(
                "You have automatically been assigned a nickname upon join. Your nickname is {second}"
            )]
        );
    }

    #[test]
    fn a_player_the_server_lists_late_is_given_theirs_when_it_does() {
        let mut rig = joined_rig();
        // The hook comes before the player is in the server's list.
        dispatch_player_logged_in(&mut rig.host, &rig.mods, BOB, "Bob");
        rig.ticks(5);
        assert_eq!(rig.nickname(BOB), None);
        rig.host
            .players
            .push(NativeServerPlayer::new(BOB, "Bob", 0));
        rig.host.textures.insert(BOB, own_skin("Bob"));
        rig.tick();
        assert!(rig.nickname(BOB).is_some());
        // One who leaves meanwhile is not waited for.
        dispatch_player_logged_in(&mut rig.host, &rig.mods, EVE, "Eve");
        dispatch_player_logged_out(&mut rig.host, &rig.mods, EVE, "Eve");
        rig.host
            .players
            .push(NativeServerPlayer::new(EVE, "Eve", 0));
        rig.ticks(3);
        assert_eq!(rig.nickname(EVE), None);
    }

    #[test]
    fn nobody_is_nicknamed_on_join_while_the_mod_is_off_or_for_a_bot() {
        let mut rig = Rig::fresh("enabled = false\n", POOL);
        rig.join("Bob", BOB, 0);
        rig.ticks(3);
        assert_eq!(rig.nickname(BOB), None);
        rig.set_config("enabled = true\n");
        let mut bot = NativeServerPlayer::new(CAROL, "Carol", 0);
        bot.fake = true;
        rig.host.players.push(bot);
        dispatch_player_logged_in(&mut rig.host, &rig.mods, CAROL, "Carol");
        rig.join("Eve", EVE, 0);
        rig.ticks(3);
        assert_eq!(
            rig.nickname(CAROL),
            None,
            "a bot has no client to show it to"
        );
        assert!(rig.nickname(EVE).is_some());
    }

    #[test]
    fn a_pool_that_cannot_give_a_name_leaves_the_player_as_they_are_and_says_so_in_the_log() {
        let mut rig = Rig::fresh("", "");
        rig.join("Bob", BOB, 0);
        rig.ticks(3);
        assert_eq!(rig.nickname(BOB), None);
        assert!(rig.host.told_to(BOB).is_empty());
        assert!(
            rig.host
                .logged()
                .iter()
                .any(|l| l
                    == "INFO: Nick: (on join) The pool is empty: add names with /nick pool add")
        );
    }

    #[test]
    fn the_setting_is_asked_with_no_word_and_a_wrong_word_is_refused() {
        let mut rig = joined_rig();
        assert_eq!(
            rig.answer(ALICE, "nickonjoin"),
            ["Random nicknames on join: on (everyone but operators)"]
        );
        assert_eq!(
            rig.answer(ALICE, "/nickonjoin ALL"),
            ["Random nicknames on join set to all (everyone, operators too)!"]
        );
        assert_eq!(
            rig.answer(ALICE, "nickonjoin"),
            ["Random nicknames on join: all (everyone, operators too)"]
        );
        assert_eq!(
            rig.answer(ALICE, "nickonjoin sometimes"),
            refused("...sometimes", "Unknown setting: use off, on or all")
        );
        assert_eq!(
            rig.answer(ALICE, "nickonjoin on now"),
            refused("...oin on now", "Unknown setting: use off, on or all")
        );
        assert!(
            rig.host.logged().contains(
                &"INFO: Nick: Alice set random nicknames on join to all (everyone, operators too)"
                    .to_owned()
            )
        );
        // It is not saved: the next start is on again.
        assert_eq!(rig.nick.state.lock().unwrap().on_join, OnJoin::All);
    }

    #[test]
    fn the_command_is_in_the_players_trees_with_its_three_words() {
        let mut rig = joined_rig();
        let usages: Vec<String> = rig.registered().into_iter().map(|(u, _, _)| u).collect();
        for usage in [
            "nickonjoin",
            "nickonjoin off",
            "nickonjoin on",
            "nickonjoin all",
        ] {
            assert!(usages.iter().any(|u| u == usage), "{usage}");
        }
    }

    // ---- completion ----------------------------------------------------------------------

    #[test]
    fn completion_offers_the_nicknames_for_info_and_the_pools_names_for_the_pool() {
        let mut rig = Rig::with_players();
        rig.run(ALICE, "nick Bob Captain");
        rig.run(ALICE, "nick Eve Rex");
        assert_eq!(rig.completions(ALICE, "nick info "), ["Captain", "Rex"]);
        assert_eq!(rig.completions(ALICE, "nick info c"), ["Captain"]);
        assert_eq!(rig.completions(ALICE, "nick pool remove ma"), ["Max"]);
        assert_eq!(
            rig.completions(ALICE, "nick pool check "),
            ["Ada", "Max", "Rex", "Zed"]
        );
        // Nothing for the rest: the client completes players from its tab list, which carries the
        // nicknames, and a word to type has no list.
        assert!(rig.completions(ALICE, "nick off ").is_empty());
        assert!(rig.completions(ALICE, "nick pool add ").is_empty());
        assert!(rig.completions(ALICE, "gamemode ").is_empty());
    }
}
