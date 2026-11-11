//! What the mod holds and what it does with it: the nicknames in force, the work waiting for the
//! next tick, and the lookups in flight.
//!
//! A command only queues what it asks for. The tick does it: the host's nickname, name tag and
//! profile helpers are for where no player's state is swapped in, and a command runs inside the
//! player's own turn. Applying a nickname is the nickname and the name tag first, then the skin of
//! the account the nickname names (looked up in the background), so a nickname is never held up
//! by the services.

use crate::config::{self, Config};
use crate::pool::Pool;
use crate::rng::Rng;
use crate::roster::Roster;
use crate::skins::{Failure, Lookup, Skin, Skins};
use horizon_client::mod_api::{Host, Text};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const GREEN: u32 = 0x55FF55;
pub const RED: u32 = 0xFF5555;
pub const GRAY: u32 = 0xAAAAAA;
pub const AQUA: u32 = 0x55FFFF;

/// How many names `/nick random` tries for one player before it gives up: a name can turn out to
/// be no account, or to be taken as it is drawn.
const MOST_DRAWS: u32 = 12;

/// How many times in a row the services may not answer before a random nickname gives up.
const MOST_SILENCES: u32 = 3;

/// Who ran a command, for the reports that come after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    Console,
    Player { uuid: u128, name: String },
}

impl Actor {
    pub fn label(&self) -> &str {
        match self {
            Self::Console => "console",
            Self::Player { name, .. } => name,
        }
    }
}

/// Whose skin a player has under their nickname.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkinState {
    /// The skin of this account was put on them.
    From(String),
    /// Their own: nothing was put on them, because no account has the name or the services did not
    /// answer.
    Kept,
}

/// A nickname in force.
#[derive(Clone, Debug)]
pub struct Active {
    pub nickname: String,
    /// The name the player logged in with.
    pub real_name: String,
    /// Who set it.
    pub by: String,
    /// The textures the player came with, to give back: None when they had none.
    pub original: Option<(String, Option<String>)>,
    /// Whether a skin was ever put on them: only then is there one to take off.
    pub reskinned: bool,
    /// What they wear now.
    pub skin: SkinState,
    /// The account the nickname names is being looked up.
    pub looking: bool,
}

/// Why a nickname is taken off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClearWhy {
    /// An operator asked.
    Asked,
    /// A player joined whose name is this nickname.
    Collision { joiner: String },
    /// The mod was turned off.
    Disabled,
}

/// Work for the next tick.
#[derive(Clone, Debug)]
pub enum Job {
    Set {
        uuid: u128,
        nickname: String,
        actor: Actor,
    },
    Clear {
        uuid: u128,
        actor: Actor,
        why: ClearWhy,
    },
    Random {
        uuid: u128,
        actor: Actor,
    },
    PoolAdd {
        name: String,
        actor: Actor,
    },
    PoolCheck {
        name: String,
        actor: Actor,
    },
}

impl Job {
    fn uuid(&self) -> Option<u128> {
        match self {
            Self::Set { uuid, .. } | Self::Clear { uuid, .. } | Self::Random { uuid, .. } => {
                Some(*uuid)
            }
            Self::PoolAdd { .. } | Self::PoolCheck { .. } => None,
        }
    }
}

/// A lookup somebody waits for.
enum Task {
    /// The skin for the nickname a player has: the account of that name.
    Skin {
        uuid: u128,
        lower: String,
        actor: Actor,
    },
    /// A random nickname being chosen: the skin of the name drawn decides whether it is used.
    Random {
        uuid: u128,
        candidate: String,
        draws: u32,
        silences: u32,
        actor: Actor,
    },
    PoolAdd {
        lower: String,
        actor: Actor,
    },
    PoolCheck {
        lower: String,
        actor: Actor,
    },
}

impl Task {
    fn uuid(&self) -> Option<u128> {
        match self {
            Self::Skin { uuid, .. } | Self::Random { uuid, .. } => Some(*uuid),
            Self::PoolAdd { .. } | Self::PoolCheck { .. } => None,
        }
    }

    fn waits_for(&self, lower: &str) -> bool {
        match self {
            Self::Skin { lower: l, .. }
            | Self::PoolAdd { lower: l, .. }
            | Self::PoolCheck { lower: l, .. } => l == lower,
            Self::Random { candidate, .. } => candidate.eq_ignore_ascii_case(lower),
        }
    }
}

/// What holds a name, so another cannot take it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// A player on the server logged in with it.
    Player(String),
    /// A player's nickname.
    Nickname { real: String },
    /// A random nickname being chosen is going to use it.
    Pending,
}

/// What to do about a nickname's skin when it is set.
enum Plan {
    /// Look up the account the nickname names.
    Lookup,
    /// The skin is in hand (a random draw found it).
    Have(Skin),
}

pub struct State {
    pub config: Config,
    /// The server's config folder.
    pub dir: PathBuf,
    pub roster: Roster,
    pub active: BTreeMap<u128, Active>,
    pub pool: Pool,
    pub skins: Skins,
    queue: VecDeque<Job>,
    waiting: Vec<Task>,
    /// The names random nicknames being chosen are going to use, in lower case.
    reserved: HashSet<String>,
    rng: Rng,
    ticks: u64,
}

/// A UUID as the server writes it.
pub fn uuid_string(uuid: u128) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (uuid >> 96) as u32,
        (uuid >> 80) as u16,
        (uuid >> 64) as u16,
        (uuid >> 48) as u16,
        uuid & 0xFFFF_FFFF_FFFF
    )
}

/// A line to whoever ran a command, now or later: a player is told in chat, the console in the
/// log.
pub fn report(host: &mut dyn Host, actor: &Actor, text: impl Into<String>, color: u32) {
    let text = text.into();
    match actor {
        Actor::Player { uuid, .. } => {
            if !host.tell(*uuid, Text::colored(text.clone(), color)) {
                host.log("INFO", &format!("Nick: (for {}) {text}", actor.label()));
            }
        }
        Actor::Console => host.log("INFO", &text),
    }
}

/// The audit line of a change.
fn audit(host: &dyn Host, text: String) {
    host.log("INFO", &format!("Nick: {text}"));
}

impl State {
    pub fn new(lookup: Arc<dyn Lookup>, background: bool, rng: Rng) -> Self {
        Self {
            config: Config::default(),
            dir: PathBuf::new(),
            roster: Roster::default(),
            active: BTreeMap::new(),
            pool: Pool::default(),
            skins: Skins::new(lookup, background),
            queue: VecDeque::new(),
            waiting: Vec::new(),
            reserved: HashSet::new(),
            rng,
            ticks: 0,
        }
    }

    /// Reads the config, the pool and the pinned skins from `dir`. All or nothing: a file that does
    /// not read leaves everything as it was. The notes are what to tell the operator.
    pub fn load(&mut self, dir: &Path) -> Result<Vec<String>, String> {
        // Where the files are, whether or not they read.
        dir.clone_into(&mut self.dir);
        let config = config::load(dir)?;
        let pool = Pool::load(dir)?;
        let skipped = self.skins.load_pins(dir)?;
        let mut notes = Vec::new();
        if pool.ignored > 0 {
            notes.push(format!(
                "{} line(s) of nick-pool.txt are not account names and are left out",
                pool.ignored
            ));
        }
        if skipped > 0 {
            notes.push(format!(
                "{skipped} entr{} of nick-skins.json could not be used and {} left out",
                if skipped == 1 { "y" } else { "ies" },
                if skipped == 1 { "is" } else { "are" }
            ));
        }
        self.skins
            .set_ttl(Duration::from_secs(config.skin_cache_minutes * 60));
        self.config = config;
        self.pool = pool;
        Ok(notes)
    }

    /// Reads the files again, as `/nick reload` and the first hook do. A command level that
    /// changed is sent to the players' command trees, and a mod turned off takes every nickname
    /// off.
    pub fn reload(&mut self, host: &mut dyn Host) -> Result<Vec<String>, String> {
        let (was_on, level) = (self.config.enabled, self.config.command_level);
        let dir = host.config_dir()?;
        let notes = self.load(&dir)?;
        if self.config.command_level != level {
            host.refresh_commands();
        }
        if was_on && !self.config.enabled {
            let all: Vec<u128> = self.active.keys().copied().collect();
            for uuid in all {
                self.queue.push_back(Job::Clear {
                    uuid,
                    actor: Actor::Console,
                    why: ClearWhy::Disabled,
                });
            }
        }
        Ok(notes)
    }

    pub fn push(&mut self, job: Job) {
        self.queue.push_back(job);
    }

    /// One of `candidates`, chosen at random.
    pub fn pick(&mut self, candidates: &[u128]) -> Option<u128> {
        (!candidates.is_empty()).then(|| candidates[self.rng.below(candidates.len())])
    }

    /// Whether anything waits for the next tick.
    pub fn busy(&self) -> bool {
        !self.queue.is_empty() || !self.waiting.is_empty()
    }

    /// What holds `lower` (a name in lower case): a player on the server, a nickname or a random
    /// draw under way. The player `except` is not counted: it is their own.
    pub fn holder(&self, lower: &str, except: u128) -> Option<Holder> {
        if let Some(m) = self
            .roster
            .iter()
            .find(|m| m.uuid != except && m.name.eq_ignore_ascii_case(lower))
        {
            return Some(Holder::Player(m.name.clone()));
        }
        if let Some((_, a)) = self
            .active
            .iter()
            .find(|(uuid, a)| **uuid != except && a.nickname.eq_ignore_ascii_case(lower))
        {
            return Some(Holder::Nickname {
                real: a.real_name.clone(),
            });
        }
        self.reserved.contains(lower).then_some(Holder::Pending)
    }

    /// Every name a random draw must leave alone, in lower case.
    fn taken_names(&self) -> HashSet<String> {
        self.roster
            .iter()
            .map(|m| m.name.to_ascii_lowercase())
            .chain(
                self.active
                    .values()
                    .map(|a| a.nickname.to_ascii_lowercase()),
            )
            .chain(self.reserved.iter().cloned())
            .collect()
    }

    // ---- joining and leaving -------------------------------------------------------------

    /// A player joined. A nickname is for a session, so none they had is left; and a nickname that
    /// is this player's own name is taken off, as it would be two players by one name.
    pub fn joined(&mut self, host: &mut dyn Host, uuid: u128, name: &str) {
        let snapshot = host.server_players();
        if snapshot.iter().any(|p| p.uuid == uuid) {
            self.roster.replace(&snapshot);
        } else {
            self.roster.joined(uuid, name);
        }
        self.forget(uuid);
        let clashing: Vec<u128> = self
            .active
            .iter()
            .filter(|(holder, a)| **holder != uuid && a.nickname.eq_ignore_ascii_case(name))
            .map(|(holder, _)| *holder)
            .collect();
        for holder in clashing {
            self.queue.push_back(Job::Clear {
                uuid: holder,
                actor: Actor::Console,
                why: ClearWhy::Collision {
                    joiner: name.to_owned(),
                },
            });
        }
    }

    /// A player is leaving: the engine takes their nickname and name tag off as it sends their
    /// leave line, so the mod only forgets them.
    pub fn left(&mut self, uuid: u128) {
        self.roster.left(uuid);
        self.forget(uuid);
    }

    /// Drops everything held for `uuid`: a nickname, queued work and lookups awaited.
    fn forget(&mut self, uuid: u128) {
        self.active.remove(&uuid);
        self.queue.retain(|job| job.uuid() != Some(uuid));
        self.drop_tasks(|task| task.uuid() == Some(uuid));
    }

    fn drop_tasks(&mut self, gone: impl Fn(&Task) -> bool) {
        let (gone_tasks, kept): (Vec<Task>, Vec<Task>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(gone);
        self.waiting = kept;
        for task in gone_tasks {
            if let Task::Random { candidate, .. } = task {
                self.reserved.remove(&candidate.to_ascii_lowercase());
            }
        }
    }

    // ---- the tick ------------------------------------------------------------------------

    /// The work of one server tick: the queued jobs, then the answers that came.
    pub fn tick(&mut self, host: &mut dyn Host) {
        self.ticks += 1;
        // The whole list once a second, and while there is work that needs it right.
        if self.busy() || self.ticks.is_multiple_of(20) {
            self.roster.replace(&host.server_players());
        }
        while let Some(job) = self.queue.pop_front() {
            self.run(host, job);
        }
        // A lookup answered at once (a pinned or remembered skin) can start another, so go on until
        // nothing new comes.
        loop {
            let arrived = self.skins.poll();
            if arrived.is_empty() {
                break;
            }
            for (lower, result) in arrived {
                self.finish(host, &lower, result);
            }
        }
        if !self.dir.as_os_str().is_empty()
            && let Err(e) = self.skins.flush(&self.dir)
        {
            host.log(
                "WARN",
                &format!("Nick: could not keep the pinned skins: {e}"),
            );
        }
    }

    fn run(&mut self, host: &mut dyn Host, job: Job) {
        match job {
            Job::Set {
                uuid,
                nickname,
                actor,
            } => self.set(host, uuid, &nickname, &actor, Plan::Lookup),
            Job::Clear { uuid, actor, why } => self.clear(host, uuid, &actor, &why),
            Job::Random { uuid, actor } => self.draw(host, uuid, actor, 0, 0),
            Job::PoolAdd { name, actor } => {
                self.waiting.push(Task::PoolAdd {
                    lower: name.to_ascii_lowercase(),
                    actor,
                });
                self.skins.request(&name);
            }
            Job::PoolCheck { name, actor } => {
                self.waiting.push(Task::PoolCheck {
                    lower: name.to_ascii_lowercase(),
                    actor,
                });
                self.skins.request(&name);
            }
        }
    }

    // ---- setting and clearing ------------------------------------------------------------

    /// Gives `uuid` the nickname: the nickname and the name tag now, and then the skin.
    fn set(&mut self, host: &mut dyn Host, uuid: u128, nickname: &str, actor: &Actor, plan: Plan) {
        let Some(member) = self.roster.get(uuid).cloned() else {
            report(
                host,
                actor,
                format!("{nickname} was not given: the player left."),
                RED,
            );
            return;
        };
        let lower = nickname.to_ascii_lowercase();
        // Taken since the command was typed.
        if let Some(holder) = self.holder(&lower, uuid) {
            let by = match holder {
                Holder::Player(real) => format!("{real} is on the server"),
                Holder::Nickname { real } => format!("{real} has it"),
                Holder::Pending => "a random nickname is about to use it".to_owned(),
            };
            report(
                host,
                actor,
                format!(
                    "{} keeps their name: {nickname} is taken ({by}).",
                    member.name
                ),
                RED,
            );
            return;
        }
        if host.text_allowed(nickname) == Some(false) {
            report(
                host,
                actor,
                format!(
                    "{} keeps their name: {nickname} is not allowed by the chat filter.",
                    member.name
                ),
                RED,
            );
            return;
        }
        let first = !self.active.contains_key(&uuid);
        let original = if first {
            host.player_textures(uuid)
        } else {
            None
        };
        // Both take or neither does: the player is on the server, and the name is a tag's shape.
        if !host.set_nickname(uuid, Some(nickname)) || !host.set_name_tag(uuid, Some(nickname)) {
            report(
                host,
                actor,
                format!("Could not give {} the nickname {nickname}.", member.name),
                RED,
            );
            return;
        }
        let random = matches!(plan, Plan::Have(_));
        audit(
            host,
            format!(
                "{} set {} ({}) to {nickname}{}",
                actor.label(),
                member.name,
                uuid_string(uuid),
                if random { " (random)" } else { "" }
            ),
        );
        let entry = self.active.entry(uuid).or_insert_with(|| Active {
            nickname: String::new(),
            real_name: member.name.clone(),
            by: String::new(),
            original,
            reskinned: false,
            skin: SkinState::Kept,
            looking: false,
        });
        entry.nickname = nickname.to_owned();
        entry.by = actor.label().to_owned();
        match plan {
            Plan::Lookup => {
                entry.looking = true;
                self.waiting.push(Task::Skin {
                    uuid,
                    lower,
                    actor: actor.clone(),
                });
                self.skins.request(nickname);
            }
            Plan::Have(skin) => {
                let ok = self.put_on(host, uuid, &skin);
                let real = member.name;
                if ok {
                    report(
                        host,
                        actor,
                        format!("{real} is now nicknamed {nickname}, with {nickname}'s skin."),
                        GREEN,
                    );
                } else {
                    report(
                        host,
                        actor,
                        format!(
                            "{real} is now nicknamed {nickname}; the skin could not be changed."
                        ),
                        GREEN,
                    );
                }
            }
        }
    }

    /// Puts the skin on the player and shows it. Whether it went through.
    fn put_on(&mut self, host: &mut dyn Host, uuid: u128, skin: &Skin) -> bool {
        let value = skin
            .textures
            .as_ref()
            .map(|(value, signature)| (value.as_str(), signature.as_deref()));
        let done = host.set_profile_property(uuid, "textures", value) && host.refresh_player(uuid);
        let real = self.active.get(&uuid).map(|a| a.real_name.clone());
        if let Some(entry) = self.active.get_mut(&uuid) {
            if done {
                entry.reskinned = true;
                entry.skin = SkinState::From(skin.account.clone());
            } else {
                entry.skin = SkinState::Kept;
            }
        }
        if done {
            audit(
                host,
                format!(
                    "{} ({}) now has the skin of {}",
                    real.unwrap_or_default(),
                    uuid_string(uuid),
                    skin.account
                ),
            );
        } else {
            host.log(
                "WARN",
                &format!(
                    "Nick: could not put the skin of {} on {}",
                    skin.account,
                    uuid_string(uuid)
                ),
            );
        }
        done
    }

    /// Takes the player's nickname off, and the skin it gave, back to the one they came with.
    fn clear(&mut self, host: &mut dyn Host, uuid: u128, actor: &Actor, why: &ClearWhy) {
        let Some(entry) = self.active.remove(&uuid) else {
            return;
        };
        self.drop_tasks(|task| task.uuid() == Some(uuid));
        if self.roster.get(uuid).is_none() {
            return;
        }
        let named = host.set_nickname(uuid, None);
        host.set_name_tag(uuid, None);
        let mut skinned = true;
        if entry.reskinned {
            let original = entry
                .original
                .as_ref()
                .map(|(value, signature)| (value.as_str(), signature.as_deref()));
            skinned =
                host.set_profile_property(uuid, "textures", original) && host.refresh_player(uuid);
        }
        let how = match why {
            ClearWhy::Asked => format!("{} cleared", actor.label()),
            ClearWhy::Collision { joiner } => {
                format!("cleared, as {joiner} joined with that name,")
            }
            ClearWhy::Disabled => "cleared, as /nick was turned off,".to_owned(),
        };
        audit(
            host,
            format!(
                "{how} {}'s nickname {} ({})",
                entry.real_name,
                entry.nickname,
                uuid_string(uuid)
            ),
        );
        if !named || !skinned {
            report(
                host,
                actor,
                format!(
                    "{}'s nickname could not be taken off completely.",
                    entry.real_name
                ),
                RED,
            );
        }
        if let ClearWhy::Collision { joiner } = why {
            let told = format!(
                "{joiner} joined with the name of {}'s nickname, so {} is no longer nicknamed {}.",
                entry.real_name, entry.real_name, entry.nickname
            );
            let ops: Vec<u128> = self
                .roster
                .iter()
                .filter(|m| !m.fake && self.config.is_op(m.permission))
                .map(|m| m.uuid)
                .collect();
            for op in ops {
                host.tell(op, Text::colored(told.clone(), AQUA));
            }
        }
    }

    // ---- random nicknames ----------------------------------------------------------------

    /// Draws a name for the player and asks for its skin; `finish` decides whether it is used.
    fn draw(&mut self, host: &mut dyn Host, uuid: u128, actor: Actor, draws: u32, silences: u32) {
        let Some(member) = self.roster.get(uuid).cloned() else {
            return;
        };
        if draws >= MOST_DRAWS {
            report(
                host,
                &actor,
                format!(
                    "No random nickname could be found for {}: the names drawn were taken or no account. Try again.",
                    member.name
                ),
                RED,
            );
            return;
        }
        let taken = self.taken_names();
        let Some(name) = self.pool.draw(&mut self.rng, &taken) else {
            let why = if self.pool.is_empty() {
                "The pool is empty: add names with /nick pool add."
            } else {
                "Every name in the pool is in use or is no account: add names with /nick pool add."
            };
            report(host, &actor, why, RED);
            return;
        };
        self.reserved.insert(name.to_ascii_lowercase());
        self.waiting.push(Task::Random {
            uuid,
            candidate: name.clone(),
            draws: draws + 1,
            silences,
            actor,
        });
        self.skins.request(&name);
    }

    // ---- answers -------------------------------------------------------------------------

    /// What the lookup of `lower` gave, for everyone who waits for it.
    fn finish(&mut self, host: &mut dyn Host, lower: &str, result: Result<Skin, Failure>) {
        let (mine, rest): (Vec<Task>, Vec<Task>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|task| task.waits_for(lower));
        self.waiting = rest;
        for task in mine {
            self.complete(host, task, result.clone());
        }
    }

    fn complete(&mut self, host: &mut dyn Host, task: Task, result: Result<Skin, Failure>) {
        match task {
            Task::Skin { uuid, lower, actor } => self.skin_came(host, uuid, &lower, &actor, result),
            Task::Random {
                uuid,
                candidate,
                draws,
                silences,
                actor,
            } => {
                self.reserved.remove(&candidate.to_ascii_lowercase());
                match result {
                    Ok(skin) => {
                        // A name of the pool: its skin is kept for good.
                        self.skins.pin(&skin);
                        if self.roster.get(uuid).is_some() {
                            let nickname = skin.account.clone();
                            self.set(host, uuid, &nickname, &actor, Plan::Have(skin));
                        }
                    }
                    Err(Failure::NoAccount) => {
                        self.pool.mark_bad(&candidate);
                        self.draw(host, uuid, actor, draws, silences);
                    }
                    Err(Failure::Unavailable) => {
                        self.pool.give_back(&candidate);
                        if silences + 1 >= MOST_SILENCES {
                            let real = self.roster.get(uuid).map(|m| m.name.clone());
                            report(
                                host,
                                &actor,
                                format!(
                                    "Mojang's services are not answering, so no random nickname could be chosen for {}. Try again in a minute.",
                                    real.unwrap_or_default()
                                ),
                                RED,
                            );
                        } else {
                            self.draw(host, uuid, actor, draws, silences + 1);
                        }
                    }
                }
            }
            Task::PoolAdd { lower, actor } => self.pool_add_came(host, &lower, &actor, result),
            Task::PoolCheck { lower, actor } => self.pool_check_came(host, &lower, &actor, result),
        }
    }

    /// The skin for a nickname that names an account.
    fn skin_came(
        &mut self,
        host: &mut dyn Host,
        uuid: u128,
        lower: &str,
        actor: &Actor,
        result: Result<Skin, Failure>,
    ) {
        // The nickname may be another by now, or gone.
        let Some(entry) = self.active.get(&uuid) else {
            return;
        };
        if !entry.nickname.eq_ignore_ascii_case(lower) {
            return;
        }
        let real = entry.real_name.clone();
        let nickname = entry.nickname.clone();
        if let Some(entry) = self.active.get_mut(&uuid) {
            entry.looking = false;
        }
        match result {
            Ok(skin) => {
                if self.pool.contains(&skin.account) {
                    self.skins.pin(&skin);
                }
                // The account's own capitalisation.
                if skin.account != nickname
                    && host.set_nickname(uuid, Some(&skin.account))
                    && host.set_name_tag(uuid, Some(&skin.account))
                    && let Some(entry) = self.active.get_mut(&uuid)
                {
                    entry.nickname = skin.account.clone();
                }
                if self.put_on(host, uuid, &skin) {
                    report(
                        host,
                        actor,
                        format!("{real} now has the skin of {}.", skin.account),
                        GREEN,
                    );
                } else {
                    report(
                        host,
                        actor,
                        format!("{real} keeps their skin: it could not be changed."),
                        RED,
                    );
                }
            }
            Err(failure) => {
                let why = match failure {
                    Failure::NoAccount => format!("no account is called {nickname}"),
                    Failure::Unavailable => format!(
                        "Mojang's services did not answer for {nickname}, so its skin is unknown"
                    ),
                };
                report(
                    host,
                    actor,
                    format!("{real} keeps their skin: {why}."),
                    GRAY,
                );
            }
        }
    }

    fn pool_add_came(
        &mut self,
        host: &mut dyn Host,
        lower: &str,
        actor: &Actor,
        result: Result<Skin, Failure>,
    ) {
        match result {
            Ok(skin) => match self.pool.add(&self.dir, &skin.account) {
                Ok(added) => {
                    self.skins.pin(&skin);
                    let text = if added {
                        format!(
                            "{} is in the pool now, with its skin kept ({} names).",
                            skin.account,
                            self.pool.len()
                        )
                    } else {
                        format!("{} is in the pool already.", skin.account)
                    };
                    report(host, actor, text, if added { GREEN } else { GRAY });
                }
                Err(e) => report(
                    host,
                    actor,
                    format!("{} could not be added: {e}", skin.account),
                    RED,
                ),
            },
            Err(Failure::NoAccount) => report(
                host,
                actor,
                format!("No account is called {lower}: the pool takes only accounts that exist."),
                RED,
            ),
            Err(Failure::Unavailable) => report(
                host,
                actor,
                format!(
                    "Mojang's services did not answer, so {lower} was not checked or added. Try again."
                ),
                RED,
            ),
        }
    }

    fn pool_check_came(
        &mut self,
        host: &mut dyn Host,
        lower: &str,
        actor: &Actor,
        result: Result<Skin, Failure>,
    ) {
        let in_pool = self.pool.find(lower).map(str::to_owned);
        let membership = match &in_pool {
            Some(name) => format!("{name} is in the pool"),
            None => format!("{lower} is not in the pool"),
        };
        match result {
            Ok(skin) => {
                if in_pool.is_some() {
                    self.skins.pin(&skin);
                }
                report(
                    host,
                    actor,
                    format!(
                        "{membership}; the account {} exists and its skin is {}.",
                        skin.account,
                        if in_pool.is_some() {
                            "kept"
                        } else {
                            "not kept (not in the pool)"
                        }
                    ),
                    GRAY,
                );
            }
            Err(Failure::NoAccount) => {
                if in_pool.is_some() {
                    self.pool.mark_bad(lower);
                }
                report(
                    host,
                    actor,
                    format!("{membership}; no account is called {lower}, so it cannot be used."),
                    GRAY,
                );
            }
            Err(Failure::Unavailable) => report(
                host,
                actor,
                format!("{membership}; Mojang's services did not answer, so its skin is unknown."),
                GRAY,
            ),
        }
    }
}
