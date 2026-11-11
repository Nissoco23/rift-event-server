//! The names `/nick random` draws from, and the order it draws them in.
//!
//! The pool is `config/nick-pool.txt`, one name a line, written from the built-in list the first
//! time. Every name is an account that exists, so its skin can be looked up and kept
//! (`skins.rs`). A draw never repeats a name until every other has been drawn, so the same names are
//! not shown again and again, and it skips names that are in use or turned out to be no account.

use crate::names::is_account_name;
use crate::rng::Rng;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "nick-pool.txt";

/// The names a new server starts with: ordinary accounts, looked up and checked for what their
/// skins show before they were put here.
pub const DEFAULT_POOL: &str = include_str!("default-pool.txt");

const HEADER: &str = "\
# The names /nick random draws from, one a line. Each is an account that exists: its skin is looked
# up the first time the name is used and kept in nick-skins.json.
# /nick pool add and remove change this file; /nick reload reads it again.
";

#[derive(Debug, Default)]
pub struct Pool {
    names: Vec<String>,
    /// The names not drawn yet in this round, the next at the end.
    bag: Vec<String>,
    /// Names the services said are no account, in lower case: left out until a reload.
    bad: HashSet<String>,
    /// How many lines of the file were not names.
    pub ignored: usize,
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

impl Pool {
    /// The names in `text`: one a line, `#` starting a comment, a name that is not an account's
    /// shape or that is there twice (in any case) left out.
    pub fn parse(text: &str) -> Self {
        let mut names: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        let mut ignored = 0;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if !is_account_name(line) {
                ignored += 1;
            } else if seen.insert(line.to_ascii_lowercase()) {
                names.push(line.to_owned());
            }
        }
        Self {
            names,
            bag: Vec::new(),
            bad: HashSet::new(),
            ignored,
        }
    }

    /// Reads `dir/nick-pool.txt`, writing the built-in list there when there is no file.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let file = path(dir);
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let pool = Self::parse(DEFAULT_POOL);
                pool.save(dir)?;
                Ok(pool)
            }
            Err(e) => Err(format!("{}: {e}", file.display())),
        }
    }

    /// Writes the pool to its file: all or nothing, so a crash does not leave half a list.
    fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut text = String::from(HEADER);
        for name in &self.names {
            text.push_str(name);
            text.push('\n');
        }
        write_whole(&path(dir), &text)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The pool's spelling of `name`, when it is in the pool (in any case).
    pub fn find(&self, name: &str) -> Option<&str> {
        self.names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(name))
            .map(String::as_str)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// How many names this session found were no account.
    pub fn bad(&self) -> usize {
        self.bad.len()
    }

    /// Puts `name` in the pool and in the file. False when it was there already.
    pub fn add(&mut self, dir: &Path, name: &str) -> Result<bool, String> {
        if self.contains(name) {
            return Ok(false);
        }
        self.names.push(name.to_owned());
        // It joins this round, the last to be drawn.
        self.bag.insert(0, name.to_owned());
        self.bad.remove(&name.to_ascii_lowercase());
        self.save(dir)?;
        Ok(true)
    }

    /// Takes `name` out of the pool and the file. False when it was not there.
    pub fn remove(&mut self, dir: &Path, name: &str) -> Result<bool, String> {
        let before = self.names.len();
        self.names.retain(|n| !n.eq_ignore_ascii_case(name));
        self.bag.retain(|n| !n.eq_ignore_ascii_case(name));
        if self.names.len() == before {
            return Ok(false);
        }
        self.save(dir)?;
        Ok(true)
    }

    /// The services said no account has this name: it is not drawn again until a reload.
    pub fn mark_bad(&mut self, name: &str) {
        self.bad.insert(name.to_ascii_lowercase());
    }

    /// A name that was drawn and not used (the services did not answer) goes back, to be drawn
    /// last in this round.
    pub fn give_back(&mut self, name: &str) {
        if self.contains(name) && !self.bag.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            self.bag.insert(0, name.to_owned());
        }
    }

    /// The next name that is not in `taken` (lower case) and not known to be no account. Every name
    /// comes once before any comes again; None when no name is left to draw at all.
    pub fn draw(&mut self, rng: &mut Rng, taken: &HashSet<String>) -> Option<String> {
        for _ in 0..2 {
            while let Some(name) = self.bag.pop() {
                let lower = name.to_ascii_lowercase();
                if !taken.contains(&lower) && !self.bad.contains(&lower) {
                    return Some(name);
                }
            }
            self.bag = self.names.clone();
            rng.shuffle(&mut self.bag);
        }
        None
    }
}

/// Writes `text` to `file` as a whole: to a file beside it first, then renamed over it.
pub fn write_whole(file: &Path, text: &str) -> Result<(), String> {
    let mut temporary = file.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, text).map_err(|e| format!("{}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, file).map_err(|e| format!("{}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_ascii_lowercase()).collect()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nick-pool-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_file_is_read_a_name_a_line_with_comments_blanks_and_repeats_left_out() {
        let pool = Pool::parse(
            "# names\nAlex\n\n  Steve  # the usual\nalex\nnot a name\nway_too_long_for_a_name\nNotch\n",
        );
        assert_eq!(pool.names(), ["Alex", "Steve", "Notch"]);
        assert_eq!(pool.ignored, 2, "two lines were not names");
        assert_eq!(pool.find("STEVE"), Some("Steve"));
        assert!(pool.contains("notch") && !pool.contains("Herobrine"));
    }

    #[test]
    fn the_built_in_list_is_sizeable_and_every_entry_is_a_distinct_account_name() {
        let pool = Pool::parse(DEFAULT_POOL);
        assert_eq!(pool.ignored, 0, "every line of the list is a name");
        assert!(pool.len() >= 250, "only {} names", pool.len());
        let lower: HashSet<String> = pool
            .names()
            .iter()
            .map(|n| n.to_ascii_lowercase())
            .collect();
        assert_eq!(lower.len(), pool.len(), "no name twice");
        for name in pool.names() {
            assert!(
                crate::names::check(name, 1, 16).is_ok(),
                "{name} could not be a nickname"
            );
        }
    }

    #[test]
    fn a_new_server_gets_the_built_in_list_in_a_file_and_an_edited_file_is_what_is_read() {
        let dir = temp("load");
        let pool = Pool::load(&dir).unwrap();
        assert_eq!(pool.len(), Pool::parse(DEFAULT_POOL).len());
        let text = std::fs::read_to_string(path(&dir)).unwrap();
        assert!(text.starts_with("# The names /nick random draws from"));
        std::fs::write(path(&dir), "Alex\nSteve\n").unwrap();
        assert_eq!(Pool::load(&dir).unwrap().names(), ["Alex", "Steve"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adding_and_removing_change_the_file_and_say_whether_they_did() {
        let dir = temp("edit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(path(&dir), "Alex\nSteve\n").unwrap();
        let mut pool = Pool::load(&dir).unwrap();
        assert_eq!(pool.add(&dir, "Notch"), Ok(true));
        assert_eq!(pool.add(&dir, "notch"), Ok(false), "in any case");
        assert_eq!(
            Pool::load(&dir).unwrap().names(),
            ["Alex", "Steve", "Notch"]
        );
        assert_eq!(pool.remove(&dir, "STEVE"), Ok(true));
        assert_eq!(pool.remove(&dir, "Steve"), Ok(false));
        assert_eq!(Pool::load(&dir).unwrap().names(), ["Alex", "Notch"]);
        assert!(
            !dir.join("nick-pool.txt.tmp").exists(),
            "the write left nothing beside the file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_is_not_drawn_twice_until_every_name_has_been_drawn() {
        let mut pool = Pool::parse("A\nB\nC\nD\nE\n");
        let mut rng = Rng::new(3);
        let mut round: Vec<String> = (0..5)
            .map(|_| pool.draw(&mut rng, &taken(&[])).unwrap())
            .collect();
        round.sort();
        assert_eq!(round, ["A", "B", "C", "D", "E"], "all five, each once");
        // The next draw starts another round.
        let again = pool.draw(&mut rng, &taken(&[])).unwrap();
        assert!(pool.contains(&again));
    }

    #[test]
    fn names_in_use_and_names_that_are_no_account_are_skipped() {
        let mut pool = Pool::parse("A\nB\nC\n");
        let mut rng = Rng::new(9);
        pool.mark_bad("b");
        assert_eq!(pool.bad(), 1);
        for _ in 0..20 {
            let name = pool.draw(&mut rng, &taken(&["a"])).unwrap();
            assert_eq!(name, "C", "A is in use and B is no account");
        }
        assert_eq!(pool.draw(&mut rng, &taken(&["a", "c"])), None);
        assert_eq!(Pool::default().draw(&mut rng, &taken(&[])), None);
    }

    #[test]
    fn a_name_given_back_is_drawn_again_and_a_name_added_joins_the_round() {
        let dir = temp("back");
        std::fs::create_dir_all(&dir).unwrap();
        let mut pool = Pool::parse("A\nB\n");
        let mut rng = Rng::new(5);
        let first = pool.draw(&mut rng, &taken(&[])).unwrap();
        pool.give_back(&first);
        let second = pool.draw(&mut rng, &taken(&[])).unwrap();
        let third = pool.draw(&mut rng, &taken(&[])).unwrap();
        assert_ne!(second, third, "both names come before any repeat");
        pool.add(&dir, "C").unwrap();
        let mut rest = vec![
            pool.draw(&mut rng, &taken(&[])).unwrap(),
            pool.draw(&mut rng, &taken(&[])).unwrap(),
        ];
        rest.sort();
        assert!(rest.contains(&"C".to_owned()), "{rest:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
