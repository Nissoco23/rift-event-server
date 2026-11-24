//! The skins of accounts: asking for one, what is remembered of the answer, and what a name in the
//! pool keeps for good.
//!
//! A skin is the account's `textures` property as the services signed it, the only form a client
//! accepts. Asking takes the network (a name lookup, then the profile), so it happens on worker
//! threads and the answers are collected on the server's tick. Answers are remembered for a while;
//! the skin of a pool name is also pinned in `config/nick-skins.json` when it is first found, so
//! `/nick random` works with the services down and a skin an account owner changes later does not
//! reach the server unasked.

use crate::pool::write_whole;
use horizon_client::skins::{LookupError, ResolvedProfile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PINS_FILE: &str = "nick-skins.json";

/// How many lookups run at once: the services limit how fast one address may ask.
const WORKERS: usize = 2;

/// A skin: the textures of an account's profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skin {
    /// The account's name, as the account writes it.
    pub account: String,
    pub id: u128,
    /// The `textures` property (value and signature); None for a profile that has none, which a
    /// client draws as its default skin.
    pub textures: Option<(String, Option<String>)>,
}

/// Why there is no skin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The services said no account has the name.
    NoAccount,
    /// They could not be asked, or did not answer usefully.
    Unavailable,
}

/// Where skins come from.
pub trait Lookup: Send + Sync + 'static {
    fn fetch(&self, name: &str) -> Result<Skin, Failure>;
}

/// The skin a profile gives.
pub fn skin_of(profile: ResolvedProfile) -> Skin {
    let textures = profile
        .properties
        .into_iter()
        .find(|(name, _, _)| name == "textures")
        .map(|(_, value, signature)| (value, signature));
    Skin {
        account: profile.name,
        id: profile.id,
        textures,
    }
}

/// What a failed lookup means for a nickname.
pub fn failure_of(error: LookupError) -> Failure {
    match error {
        LookupError::InvalidName | LookupError::NotFound => Failure::NoAccount,
        LookupError::Unavailable => Failure::Unavailable,
    }
}

/// Mojang's services, through the engine's profile lookup.
pub struct Mojang;

impl Lookup for Mojang {
    fn fetch(&self, name: &str) -> Result<Skin, Failure> {
        horizon_client::skins::fetch_profile_by_name(name)
            .map(skin_of)
            .map_err(failure_of)
    }
}

type Task = Box<dyn FnOnce() + Send>;

/// A few threads that run lookups one after another each.
struct Workers {
    tasks: Sender<Task>,
}

impl Workers {
    fn start() -> Self {
        let (tasks, queue) = channel::<Task>();
        let queue = Arc::new(Mutex::new(queue));
        for n in 0..WORKERS {
            let queue = Arc::clone(&queue);
            let _ = std::thread::Builder::new()
                .name(format!("Nick skin lookup {n}"))
                .spawn(move || {
                    loop {
                        let task = match queue.lock() {
                            Ok(queue) => queue.recv(),
                            Err(_) => return,
                        };
                        match task {
                            Ok(task) => task(),
                            // The mod is gone.
                            Err(_) => return,
                        }
                    }
                });
        }
        Self { tasks }
    }

    fn run(&self, task: Task) {
        let _ = self.tasks.send(task);
    }
}

/// An answer on its way back: the name asked for (lower case), what it gave, and whether it is new
/// from the services (and so worth remembering) rather than from memory.
type Answer = (String, Result<Skin, Failure>, bool);

pub struct Skins {
    lookup: Arc<dyn Lookup>,
    workers: Option<Workers>,
    answers: Sender<Answer>,
    arrived: Receiver<Answer>,
    /// What the services answered lately, by lower case name. An unavailable answer is not kept:
    /// the next ask tries again.
    cache: HashMap<String, (Instant, Result<Skin, Failure>)>,
    inflight: HashSet<String>,
    /// The skins of the pool's names, by lower case name, kept in a file.
    pins: BTreeMap<String, Skin>,
    dirty: bool,
    ttl: Duration,
}

impl Skins {
    /// `background`: lookups run on worker threads; without it they run at the call, which a test
    /// uses to have answers at once.
    pub fn new(lookup: Arc<dyn Lookup>, background: bool) -> Self {
        let (answers, arrived) = channel();
        Self {
            lookup,
            workers: background.then(Workers::start),
            answers,
            arrived,
            cache: HashMap::new(),
            inflight: HashSet::new(),
            pins: BTreeMap::new(),
            dirty: false,
            ttl: Duration::from_secs(600),
        }
    }

    pub fn set_ttl(&mut self, ttl: Duration) {
        self.ttl = ttl;
    }

    /// Asks for the skin of the account `name`: the answer comes from `poll`. A pinned or
    /// remembered one comes at once, and a name already being looked up is not asked for twice.
    pub fn request(&mut self, name: &str) {
        let lower = name.to_ascii_lowercase();
        if let Some(skin) = self.pins.get(&lower) {
            let _ = self.answers.send((lower, Ok(skin.clone()), false));
            return;
        }
        if let Some((at, result)) = self.cache.get(&lower)
            && at.elapsed() < self.ttl
        {
            let _ = self.answers.send((lower, result.clone(), false));
            return;
        }
        if !self.inflight.insert(lower.clone()) {
            return;
        }
        let (lookup, answers, asked) = (
            Arc::clone(&self.lookup),
            self.answers.clone(),
            name.to_owned(),
        );
        let task = move || {
            let result = lookup.fetch(&asked);
            let _ = answers.send((lower, result, true));
        };
        match &self.workers {
            Some(workers) => workers.run(Box::new(task)),
            None => task(),
        }
    }

    /// The answers that have come, by lower case name.
    pub fn poll(&mut self) -> Vec<(String, Result<Skin, Failure>)> {
        let mut out = Vec::new();
        while let Ok((lower, result, fresh)) = self.arrived.try_recv() {
            if fresh {
                self.inflight.remove(&lower);
                if !matches!(result, Err(Failure::Unavailable)) {
                    self.cache
                        .insert(lower.clone(), (Instant::now(), result.clone()));
                }
            }
            out.push((lower, result));
        }
        out
    }

    /// Keeps `skin` for good under its account's name.
    pub fn pin(&mut self, skin: &Skin) {
        let lower = skin.account.to_ascii_lowercase();
        if self.pins.get(&lower) != Some(skin) {
            self.pins.insert(lower, skin.clone());
            self.dirty = true;
        }
    }

    pub fn unpin(&mut self, name: &str) {
        if self.pins.remove(&name.to_ascii_lowercase()).is_some() {
            self.dirty = true;
        }
    }

    pub fn pinned(&self, name: &str) -> Option<&Skin> {
        self.pins.get(&name.to_ascii_lowercase())
    }

    pub fn pinned_count(&self) -> usize {
        self.pins.len()
    }

    /// Reads `dir/nick-skins.json`. A file that does not read changes nothing. Returns how many
    /// entries were unusable and left out.
    pub fn load_pins(&mut self, dir: &Path) -> Result<usize, String> {
        let file = path(dir);
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.pins.clear();
                self.dirty = false;
                return Ok(0);
            }
            Err(e) => return Err(format!("{}: {e}", file.display())),
        };
        let entries: BTreeMap<String, Pin> =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        let mut pins = BTreeMap::new();
        let mut skipped = 0;
        for (key, pin) in entries {
            match pin.skin() {
                Some(skin) if skin.account.eq_ignore_ascii_case(&key) => {
                    pins.insert(key.to_ascii_lowercase(), skin);
                }
                _ => skipped += 1,
            }
        }
        self.pins = pins;
        self.dirty = false;
        Ok(skipped)
    }

    /// Writes the pins when they changed.
    pub fn flush(&mut self, dir: &Path) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let entries: BTreeMap<&String, Pin> = self
            .pins
            .iter()
            .map(|(key, skin)| (key, Pin::of(skin)))
            .collect();
        let text = serde_json::to_string_pretty(&entries)
            .map_err(|e| format!("{}: {e}", path(dir).display()))?;
        write_whole(&path(dir), &(text + "\n"))?;
        self.dirty = false;
        Ok(())
    }
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(PINS_FILE)
}

/// A pinned skin as the file holds it.
#[derive(Serialize, Deserialize)]
struct Pin {
    name: String,
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

impl Pin {
    fn of(skin: &Skin) -> Self {
        Self {
            name: skin.account.clone(),
            id: format!("{:032x}", skin.id),
            value: skin.textures.as_ref().map(|(value, _)| value.clone()),
            signature: skin
                .textures
                .as_ref()
                .and_then(|(_, signature)| signature.clone()),
        }
    }

    /// The skin, when the entry is whole: an account name and an id, and a signature only with a
    /// value.
    fn skin(&self) -> Option<Skin> {
        let id = u128::from_str_radix(&self.id.replace('-', ""), 16).ok()?;
        if !crate::names::is_account_name(&self.name) || self.id.replace('-', "").len() != 32 {
            return None;
        }
        let textures = match (&self.value, &self.signature) {
            (Some(value), signature) if !value.is_empty() => {
                Some((value.clone(), signature.clone()))
            }
            (None, None) => None,
            _ => return None,
        };
        Some(Skin {
            account: self.name.clone(),
            id,
            textures,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub fn skin(account: &str) -> Skin {
        Skin {
            account: account.to_owned(),
            id: 0x1234,
            textures: Some((
                format!("value-of-{account}"),
                Some(format!("signed-{account}")),
            )),
        }
    }

    /// Answers from a table, counting what it was asked.
    struct Table {
        accounts: Mutex<HashMap<String, Skin>>,
        down: std::sync::atomic::AtomicBool,
        asked: AtomicUsize,
    }

    impl Table {
        fn new(accounts: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                accounts: Mutex::new(
                    accounts
                        .iter()
                        .map(|a| (a.to_ascii_lowercase(), skin(a)))
                        .collect(),
                ),
                down: std::sync::atomic::AtomicBool::new(false),
                asked: AtomicUsize::new(0),
            })
        }
    }

    impl Lookup for Table {
        fn fetch(&self, name: &str) -> Result<Skin, Failure> {
            self.asked.fetch_add(1, Ordering::SeqCst);
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

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nick-skins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_profile_gives_its_textures_and_a_profile_without_them_gives_none() {
        let profile = ResolvedProfile {
            id: 0x069a_79f4_44e9_4726_a5be_fca9_0e38_aaf5,
            name: "Notch".into(),
            properties: vec![
                ("other".into(), "x".into(), None),
                ("textures".into(), "dmFsdWU=".into(), Some("c2ln".into())),
            ],
        };
        let skin = skin_of(profile.clone());
        assert_eq!(skin.account, "Notch");
        assert_eq!(skin.id, 0x069a_79f4_44e9_4726_a5be_fca9_0e38_aaf5);
        assert_eq!(
            skin.textures,
            Some(("dmFsdWU=".into(), Some("c2ln".into())))
        );
        let bare = skin_of(ResolvedProfile {
            properties: Vec::new(),
            ..profile
        });
        assert_eq!(bare.textures, None);
    }

    #[test]
    fn what_the_services_cannot_say_is_not_a_missing_account() {
        assert_eq!(failure_of(LookupError::NotFound), Failure::NoAccount);
        assert_eq!(failure_of(LookupError::InvalidName), Failure::NoAccount);
        assert_eq!(failure_of(LookupError::Unavailable), Failure::Unavailable);
    }

    #[test]
    fn an_answer_is_remembered_so_the_services_are_asked_once() {
        let table = Table::new(&["Alex"]);
        let mut skins = Skins::new(table.clone(), false);
        skins.request("ALEX");
        assert_eq!(skins.poll(), vec![("alex".to_owned(), Ok(skin("Alex")))]);
        skins.request("alex");
        assert_eq!(skins.poll(), vec![("alex".to_owned(), Ok(skin("Alex")))]);
        assert_eq!(table.asked.load(Ordering::SeqCst), 1);
        // A name that is no account is remembered too.
        skins.request("Nobody");
        assert_eq!(
            skins.poll(),
            vec![("nobody".to_owned(), Err(Failure::NoAccount))]
        );
        skins.request("nobody");
        assert_eq!(
            skins.poll(),
            vec![("nobody".to_owned(), Err(Failure::NoAccount))]
        );
        assert_eq!(table.asked.load(Ordering::SeqCst), 2);
        skins.set_ttl(Duration::ZERO);
        skins.request("alex");
        skins.poll();
        assert_eq!(
            table.asked.load(Ordering::SeqCst),
            3,
            "forgotten after its time"
        );
    }

    #[test]
    fn services_that_did_not_answer_are_asked_again_next_time() {
        let table = Table::new(&["Alex"]);
        table.down.store(true, Ordering::SeqCst);
        let mut skins = Skins::new(table.clone(), false);
        skins.request("Alex");
        assert_eq!(
            skins.poll(),
            vec![("alex".to_owned(), Err(Failure::Unavailable))]
        );
        table.down.store(false, Ordering::SeqCst);
        skins.request("Alex");
        assert_eq!(skins.poll(), vec![("alex".to_owned(), Ok(skin("Alex")))]);
        assert_eq!(table.asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_name_being_looked_up_is_not_asked_for_twice() {
        let table = Table::new(&["Alex"]);
        let mut skins = Skins::new(table.clone(), false);
        skins.request("Alex");
        skins.request("alex");
        assert_eq!(skins.poll().len(), 1, "one answer for both asks");
        assert_eq!(table.asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_pinned_skin_comes_without_asking_and_survives_a_restart() {
        let dir = temp("pins");
        let table = Table::new(&[]);
        let mut skins = Skins::new(table.clone(), false);
        skins.pin(&skin("Alex"));
        skins.pin(&Skin {
            account: "Plain".into(),
            id: 7,
            textures: None,
        });
        assert_eq!(skins.pinned_count(), 2);
        skins.flush(&dir).unwrap();

        let mut again = Skins::new(table.clone(), false);
        assert_eq!(again.load_pins(&dir), Ok(0));
        again.request("alex");
        again.request("PLAIN");
        let answers = again.poll();
        assert_eq!(answers[0].1, Ok(skin("Alex")));
        assert_eq!(
            answers[1].1.as_ref().unwrap().textures,
            None,
            "a profile with no textures stays so"
        );
        assert_eq!(table.asked.load(Ordering::SeqCst), 0);
        // A pin taken away is gone from the file.
        again.unpin("Alex");
        again.flush(&dir).unwrap();
        let mut third = Skins::new(table, false);
        third.load_pins(&dir).unwrap();
        assert!(third.pinned("alex").is_none() && third.pinned("plain").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pins_file_that_does_not_read_changes_nothing_and_a_bad_entry_is_left_out() {
        let dir = temp("broken");
        std::fs::create_dir_all(&dir).unwrap();
        let mut skins = Skins::new(Table::new(&[]), false);
        skins.pin(&skin("Alex"));
        std::fs::write(path(&dir), "{ not json").unwrap();
        assert!(
            skins
                .load_pins(&dir)
                .unwrap_err()
                .contains("nick-skins.json")
        );
        assert!(skins.pinned("alex").is_some(), "the old pins stay");

        std::fs::write(
            path(&dir),
            r#"{
              "good": {"name": "Good", "id": "00000000-0000-0000-0000-000000000001", "value": "v", "signature": "s"},
              "wrongid": {"name": "WrongId", "id": "xyz", "value": "v"},
              "mismatch": {"name": "Other", "id": "00000000000000000000000000000002", "value": "v"},
              "unsigned_without_value": {"name": "Unsigned_Without_Value", "id": "00000000000000000000000000000003", "signature": "s"}
            }"#,
        )
        .unwrap();
        assert_eq!(skins.load_pins(&dir), Ok(3));
        assert_eq!(skins.pinned_count(), 1);
        assert_eq!(skins.pinned("GOOD").unwrap().id, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_written_when_nothing_changed() {
        let dir = temp("clean");
        let mut skins = Skins::new(Table::new(&[]), false);
        skins.flush(&dir).unwrap();
        assert!(!path(&dir).exists());
        skins.pin(&skin("Alex"));
        skins.flush(&dir).unwrap();
        assert!(path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lookups_on_worker_threads_come_back_to_the_poll() {
        let table = Table::new(&["Alex", "Steve"]);
        let mut skins = Skins::new(table, true);
        skins.request("Alex");
        skins.request("Steve");
        skins.request("Nobody");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.len() < 3 && Instant::now() < deadline {
            got.extend(skins.poll());
            std::thread::sleep(Duration::from_millis(2));
        }
        got.sort_by(|a, b| a.0.cmp(&b.0));
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["alex", "nobody", "steve"]);
        assert_eq!(got[1].1, Err(Failure::NoAccount));
    }
}
