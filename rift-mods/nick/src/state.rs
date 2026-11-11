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
use horizon_client::mod_api::Host;
use rift_chat::{count, escape, mini, plain};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
        /// The command that asked for several at once, which answers when all are done.
        batch: Option<u64>,
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

    fn batch(&self) -> Option<u64> {
        match self {
            Self::Random { batch, .. } => *batch,
            _ => None,
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
        batch: Option<u64>,
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

    fn batch(&self) -> Option<u64> {
        match self {
            Self::Random { batch, .. } => *batch,
            _ => None,
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

/// A command that asked for random nicknames for several players: it answers once, when every
/// player has had theirs or has not.
struct Batch {
    actor: Actor,
    total: usize,
    ok: usize,
    failed: usize,
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
    batches: HashMap<u64, Batch>,
    next_batch: u64,
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

/// A line of markup (`rift_chat`) to whoever ran a command, now or later: a player is told in
/// chat, the console in the log.
pub fn report(host: &mut dyn Host, actor: &Actor, markup: &str) {
    match actor {
        Actor::Player { uuid, .. } => {
            if !host.tell(*uuid, mini(markup)) {
                host.log(
                    "INFO",
                    &format!("Nick: (for {}) {}", actor.label(), plain(markup)),
                );
            }
        }
        Actor::Console => host.log("INFO", &plain(markup)),
    }
}

/// A line of markup to a player of the server: what happened to them.
fn tell(host: &mut dyn Host, uuid: u128, markup: &str) {
    host.tell(uuid, mini(markup));
}

/// `text` as markup in the accent colour, escaped.
pub fn accent(text: &str) -> String {
    format!("<accent>{}</accent>", escape(text))
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
            batches: HashMap::new(),
            next_batch: 0,
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

    /// Opens a batch of `total` random nicknames for `actor`; the jobs name its id.
    pub fn new_batch(&mut self, actor: &Actor, total: usize) -> u64 {
        self.next_batch += 1;
        self.batches.insert(
            self.next_batch,
            Batch {
                actor: actor.clone(),
                total,
                ok: 0,
                failed: 0,
            },
        );
        self.next_batch
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
        let (gone, kept): (VecDeque<Job>, VecDeque<Job>) = std::mem::take(&mut self.queue)
            .into_iter()
            .partition(|job| job.uuid() == Some(uuid));
        self.queue = kept;
        for job in gone {
            self.batch_left(job.batch());
        }
        self.drop_tasks(|task| task.uuid() == Some(uuid));
    }

    /// A player of a batch is gone: the batch no longer waits for them.
    fn batch_left(&mut self, batch: Option<u64>) {
        if let Some(batch) = batch.and_then(|id| self.batches.get_mut(&id)) {
            batch.total = batch.total.saturating_sub(1);
        }
    }

    fn drop_tasks(&mut self, gone: impl Fn(&Task) -> bool) {
        let (gone_tasks, kept): (Vec<Task>, Vec<Task>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(gone);
        self.waiting = kept;
        for task in gone_tasks {
            if let Task::Random { candidate, .. } = &task {
                self.reserved.remove(&candidate.to_ascii_lowercase());
            }
            self.batch_left(task.batch());
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
        self.finish_batches(host);
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
            } => {
                self.set(host, uuid, &nickname, &actor, Plan::Lookup);
            }
            Job::Clear { uuid, actor, why } => self.clear(host, uuid, &actor, &why),
            Job::Random { uuid, actor, batch } => self.draw(host, uuid, actor, batch, 0, 0),
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

    /// Gives `uuid` the nickname: the nickname and the name tag now, and then the skin. None when
    /// it was not given (the reason has been told); for a skin in hand (`Plan::Have`), whether
    /// it went on.
    fn set(
        &mut self,
        host: &mut dyn Host,
        uuid: u128,
        nickname: &str,
        actor: &Actor,
        plan: Plan,
    ) -> Option<bool> {
        let Some(member) = self.roster.get(uuid).cloned() else {
            report(
                host,
                actor,
                &format!(
                    "<red>The nickname {} was not given: the player left</red>",
                    accent(nickname)
                ),
            );
            return None;
        };
        let real = accent(&member.name);
        let lower = nickname.to_ascii_lowercase();
        // Taken since the command was typed.
        if let Some(holder) = self.holder(&lower, uuid) {
            let by = match holder {
                Holder::Player(real) => format!("{} is on the server", accent(&real)),
                Holder::Nickname { real } => format!("{} has it", accent(&real)),
                Holder::Pending => "a random nickname is about to use it".to_owned(),
            };
            report(
                host,
                actor,
                &format!(
                    "<red>{real} keeps their name: {} is taken ({by})</red>",
                    accent(nickname)
                ),
            );
            return None;
        }
        if host.text_allowed(nickname) == Some(false) {
            report(
                host,
                actor,
                &format!(
                    "<red>{real} keeps their name: {} is not allowed by the chat filter</red>",
                    accent(nickname)
                ),
            );
            return None;
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
                &format!(
                    "<red>Could not give {real} the nickname {}</red>",
                    accent(nickname)
                ),
            );
            return None;
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
        // The player is told what they are now, unless they are the one who asked.
        if !matches!(actor, Actor::Player { uuid: by, .. } if *by == uuid) {
            tell(
                host,
                uuid,
                &format!("<gray>Your nickname is now {}</gray>", accent(nickname)),
            );
        }
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
                Some(true)
            }
            Plan::Have(skin) => Some(self.put_on(host, uuid, &skin)),
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

    /// Takes the player's nickname off, and the skin it gave, back to the one they came with, and
    /// tells them.
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
        tell(host, uuid, "<gray>Your nickname has been removed</gray>");
        if !named || !skinned {
            report(
                host,
                actor,
                &format!(
                    "<red>{}'s nickname could not be taken off completely</red>",
                    accent(&entry.real_name)
                ),
            );
        }
        if let ClearWhy::Collision { joiner } = why {
            let told = format!(
                "<gray>{} joined with the name of {}'s nickname, so {} is no longer nicknamed {}</gray>",
                accent(joiner),
                accent(&entry.real_name),
                accent(&entry.real_name),
                accent(&entry.nickname)
            );
            let ops: Vec<u128> = self
                .roster
                .iter()
                .filter(|m| !m.fake && self.config.is_op(m.permission))
                .map(|m| m.uuid)
                .collect();
            for op in ops {
                tell(host, op, &told);
            }
        }
    }

    // ---- random nicknames ----------------------------------------------------------------

    /// Draws a name for the player and asks for its skin; `finish` decides whether it is used.
    fn draw(
        &mut self,
        host: &mut dyn Host,
        uuid: u128,
        actor: Actor,
        batch: Option<u64>,
        draws: u32,
        silences: u32,
    ) {
        let Some(member) = self.roster.get(uuid).cloned() else {
            self.batch_left(batch);
            return;
        };
        let real = accent(&member.name);
        if draws >= MOST_DRAWS {
            let why = format!(
                "No random nickname could be found for {real}: the names drawn were taken or no account. Try again"
            );
            self.drew(host, uuid, &actor, batch, Err(Some(why)));
            return;
        }
        let taken = self.taken_names();
        let Some(name) = self.pool.draw(&mut self.rng, &taken) else {
            let why = if self.pool.is_empty() {
                "The pool is empty: add names with <accent><click:run_command:'/nick pool add '>/nick pool add</click></accent>"
            } else {
                "Every name in the pool is in use or is no account: add names with <accent><click:run_command:'/nick pool add '>/nick pool add</click></accent>"
            };
            self.drew(host, uuid, &actor, batch, Err(Some(why.to_owned())));
            return;
        };
        self.reserved.insert(name.to_ascii_lowercase());
        self.waiting.push(Task::Random {
            uuid,
            candidate: name.clone(),
            draws: draws + 1,
            silences,
            actor,
            batch,
        });
        self.skins.request(&name);
    }

    /// How a random nickname ended: given (and whether its skin went on), or not (with the reason
    /// to tell, unless it has been told). One asked for alone is answered now; one of a batch is
    /// counted, and the batch answers when all have ended.
    fn drew(
        &mut self,
        host: &mut dyn Host,
        uuid: u128,
        actor: &Actor,
        batch: Option<u64>,
        result: Result<(String, bool), Option<String>>,
    ) {
        if let Some(batch) = batch.and_then(|id| self.batches.get_mut(&id)) {
            match result {
                Ok(_) => batch.ok += 1,
                Err(_) => batch.failed += 1,
            }
            return;
        }
        let real = self
            .roster
            .get(uuid)
            .map_or_else(|| uuid_string(uuid), |m| m.name.clone());
        match result {
            Ok((nickname, skin_ok)) => {
                report(
                    host,
                    actor,
                    &format!(
                        "<white>Assigned random nickname {} to {}!</white>",
                        accent(&nickname),
                        accent(&real)
                    ),
                );
                if !skin_ok {
                    report(host, actor, "<gray>Their skin could not be changed</gray>");
                }
            }
            Err(Some(why)) => report(host, actor, &format!("<red>{why}</red>")),
            Err(None) => {}
        }
    }

    /// Answers the batches whose players have all had their draw end: how many got a nickname.
    fn finish_batches(&mut self, host: &mut dyn Host) {
        let done: Vec<u64> = self
            .batches
            .iter()
            .filter(|(_, b)| b.ok + b.failed >= b.total)
            .map(|(id, _)| *id)
            .collect();
        for id in done {
            let Some(batch) = self.batches.remove(&id) else {
                continue;
            };
            if batch.ok > 0 {
                report(
                    host,
                    &batch.actor,
                    &format!(
                        "<white>Assigned random nicknames to {}!</white>",
                        accent(&count(batch.ok, "player"))
                    ),
                );
            }
            if batch.failed > 0 {
                report(
                    host,
                    &batch.actor,
                    &format!(
                        "<red>{} could not be given one: the pool ran out of free names or Mojang's services did not answer</red>",
                        accent(&count(batch.failed, "player"))
                    ),
                );
            }
        }
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
                batch,
            } => {
                self.reserved.remove(&candidate.to_ascii_lowercase());
                match result {
                    Ok(skin) => {
                        // A name of the pool: its skin is kept for good.
                        self.skins.pin(&skin);
                        if self.roster.get(uuid).is_none() {
                            self.batch_left(batch);
                            return;
                        }
                        let nickname = skin.account.clone();
                        let ended = match self.set(host, uuid, &nickname, &actor, Plan::Have(skin))
                        {
                            Some(skin_ok) => Ok((nickname, skin_ok)),
                            // Told by `set`.
                            None => Err(None),
                        };
                        self.drew(host, uuid, &actor, batch, ended);
                    }
                    Err(Failure::NoAccount) => {
                        self.pool.mark_bad(&candidate);
                        self.draw(host, uuid, actor, batch, draws, silences);
                    }
                    Err(Failure::Unavailable) => {
                        self.pool.give_back(&candidate);
                        if silences + 1 >= MOST_SILENCES {
                            let real = self.roster.get(uuid).map(|m| m.name.clone());
                            let why = format!(
                                "Mojang's services are not answering, so no random nickname could be chosen for {}. Try again in a minute",
                                accent(&real.unwrap_or_default())
                            );
                            self.drew(host, uuid, &actor, batch, Err(Some(why)));
                        } else {
                            self.draw(host, uuid, actor, batch, draws, silences + 1);
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
        let real = accent(&entry.real_name);
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
                        &format!(
                            "<white>{real} now wears the skin of {}!</white>",
                            accent(&skin.account)
                        ),
                    );
                } else {
                    report(
                        host,
                        actor,
                        &format!("<red>{real} keeps their skin: it could not be changed</red>"),
                    );
                }
            }
            Err(failure) => {
                let nickname = accent(&nickname);
                let why = match failure {
                    Failure::NoAccount => format!("no account is called {nickname}"),
                    Failure::Unavailable => format!(
                        "Mojang's services did not answer for {nickname}, so its skin is unknown"
                    ),
                };
                report(
                    host,
                    actor,
                    &format!("<gray>{real} keeps their skin: {why}</gray>"),
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
                            "<white>Added {} to the pool, with its skin kept ({} now)!</white>",
                            accent(&skin.account),
                            accent(&count(self.pool.len(), "name"))
                        )
                    } else {
                        format!(
                            "<gray>{} is in the pool already</gray>",
                            accent(&skin.account)
                        )
                    };
                    report(host, actor, &text);
                }
                Err(e) => report(
                    host,
                    actor,
                    &format!(
                        "<red>{} could not be added: {}</red>",
                        accent(&skin.account),
                        escape(&e)
                    ),
                ),
            },
            Err(Failure::NoAccount) => report(
                host,
                actor,
                &format!(
                    "<red>No account is called {}: the pool takes only accounts that exist</red>",
                    accent(lower)
                ),
            ),
            Err(Failure::Unavailable) => report(
                host,
                actor,
                &format!(
                    "<red>Mojang's services did not answer, so {} was not checked or added. Try again</red>",
                    accent(lower)
                ),
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
            Some(name) => format!("{} is in the pool", accent(name)),
            None => format!("{} is not in the pool", accent(lower)),
        };
        match result {
            Ok(skin) => {
                if in_pool.is_some() {
                    self.skins.pin(&skin);
                }
                report(
                    host,
                    actor,
                    &format!(
                        "<gray>{membership}; the account {} exists and its skin is {}</gray>",
                        accent(&skin.account),
                        if in_pool.is_some() {
                            "kept"
                        } else {
                            "not kept (not in the pool)"
                        }
                    ),
                );
            }
            Err(Failure::NoAccount) => {
                if in_pool.is_some() {
                    self.pool.mark_bad(lower);
                }
                report(
                    host,
                    actor,
                    &format!(
                        "<gray>{membership}; no account is called {}, so it cannot be used</gray>",
                        accent(lower)
                    ),
                );
            }
            Err(Failure::Unavailable) => report(
                host,
                actor,
                &format!(
                    "<gray>{membership}; Mojang's services did not answer, so its skin is unknown</gray>"
                ),
            ),
        }
    }
}
