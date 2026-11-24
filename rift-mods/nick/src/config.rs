//! The mod's configuration: `config/nick.toml`.
//!
//! The file is written, with its comments, when it is missing. Numbers outside what means something
//! are brought into range, and a key the mod does not know is an error, so a typo is told rather
//! than left at its default.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "nick.toml";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Whether `/nick` works. Off, it says so and every nickname in force is taken off.
    pub enabled: bool,
    /// The operator level `/nick` needs (`ops.json`): 1 to 4.
    pub command_level: u8,
    /// The lowest operator level that counts as an operator, for `@a`, `@p` and `@r` to skip and
    /// for `--admin` to mean: 1 to 4.
    pub op_level: u8,
    /// The fewest characters a nickname may have: 1 to 16.
    pub min_length: usize,
    /// The most characters a nickname may have: `min-length` to 16.
    pub max_length: usize,
    /// How many minutes a skin the services gave is remembered before it is asked for again.
    pub skin_cache_minutes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            command_level: 2,
            op_level: 1,
            min_length: 1,
            max_length: 16,
            skin_cache_minutes: 10,
        }
    }
}

impl Config {
    /// The same with every number held to what means something.
    fn held(self) -> Self {
        let min_length = self.min_length.clamp(1, 16);
        Self {
            command_level: self.command_level.clamp(1, 4),
            op_level: self.op_level.clamp(1, 4),
            min_length,
            max_length: self.max_length.clamp(min_length, 16),
            skin_cache_minutes: self.skin_cache_minutes.clamp(1, 24 * 60),
            ..self
        }
    }

    /// Whether a player of operator level `permission` counts as an operator.
    pub fn is_op(&self, permission: u8) -> bool {
        permission >= self.op_level
    }
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Reads a config's text.
pub fn parse(text: &str) -> Result<Config, String> {
    toml::from_str::<Config>(text)
        .map(Config::held)
        .map_err(|e| e.message().to_owned())
}

/// Reads `dir/nick.toml`, writing the default one when there is none.
pub fn load(dir: &Path) -> Result<Config, String> {
    let file = path(dir);
    match std::fs::read_to_string(&file) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            std::fs::write(&file, DEFAULT_FILE).map_err(|e| format!("{}: {e}", file.display()))?;
            Ok(Config::default())
        }
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// What a new `nick.toml` holds.
pub const DEFAULT_FILE: &str = r#"# Operator nicknames. `/nick reload` reads this file, nick-pool.txt and nick-skins.json again.
# The names /nick random draws from are in nick-pool.txt, the skins it has already looked up in
# nick-skins.json.

# Turn this off and /nick says so and every nickname in force is taken off.
enabled = true

# The operator level /nick needs: 2 is a game master. A permissions mod can grant `nick.admin` instead.
command-level = 2

# The lowest operator level that counts as an operator: @a, @p and @r skip them, `--admin` means them.
op-level = 1

# How long a nickname may be. Mojang's own names have 3 to 16 characters; 1 or 2 are for legacy accounts.
min-length = 1
max-length = 16

# How many minutes a skin the services gave is remembered before it is asked for again.
skin-cache-minutes = 10
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_a_new_server_gets_is_the_default_config() {
        assert_eq!(parse(DEFAULT_FILE), Ok(Config::default()));
        assert_eq!(parse(""), Ok(Config::default()));
    }

    #[test]
    fn numbers_are_held_to_what_means_something() {
        let cfg = parse(
            "command-level = 0\nop-level = 9\nmin-length = 0\nmax-length = 99\nskin-cache-minutes = 0\n",
        )
        .unwrap();
        assert_eq!(
            (
                cfg.command_level,
                cfg.op_level,
                cfg.min_length,
                cfg.max_length,
                cfg.skin_cache_minutes
            ),
            (1, 4, 1, 16, 1)
        );
        // A longest below the shortest is the shortest.
        let cfg = parse("min-length = 5\nmax-length = 3\n").unwrap();
        assert_eq!((cfg.min_length, cfg.max_length), (5, 5));
    }

    #[test]
    fn a_mistake_is_told_and_not_taken_for_the_default() {
        assert!(
            parse("comand-level = 2\n")
                .unwrap_err()
                .contains("comand-level")
        );
        assert!(parse("enabled = 'yes'\n").is_err());
        assert!(parse("min-length = [").is_err());
    }

    #[test]
    fn the_level_that_counts_as_an_operator_is_the_configs() {
        let cfg = Config {
            op_level: 3,
            ..Config::default()
        };
        assert!(!cfg.is_op(2) && cfg.is_op(3) && cfg.is_op(4));
        assert!(!Config::default().is_op(0) && Config::default().is_op(1));
    }

    #[test]
    fn a_missing_file_is_written_and_a_broken_one_is_an_error() {
        let dir = std::env::temp_dir().join(format!("nick-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), Ok(Config::default()));
        assert_eq!(std::fs::read_to_string(path(&dir)).unwrap(), DEFAULT_FILE);
        std::fs::write(path(&dir), "enabled = [").unwrap();
        assert!(
            load(&dir)
                .unwrap_err()
                .starts_with(&path(&dir).display().to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
