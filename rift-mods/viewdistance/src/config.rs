//! The mod's configuration: `config/viewdistance.toml`.
//!
//! The file is written, with its comments, when it is missing. Numbers outside what means something
//! are brought into range, and a key the mod does not know is an error, so a typo is told rather
//! than left at its default.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "viewdistance.toml";

/// The distances the engine takes for one player (`native_set_player_view_distance`): 2 to 32
/// chunks.
pub const MIN_DISTANCE: i32 = 2;
pub const MAX_DISTANCE: i32 = 32;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Whether the mod limits anyone. Off, every player has the server's own view distance.
    pub enabled: bool,
    /// The most chunks a player who is not an operator is sent in every direction.
    pub non_op_max: i32,
    /// The same for an operator.
    pub op_max: i32,
    /// The lowest operator level (`ops.json`) that counts as an operator here: 1 to 4.
    pub op_level: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            non_op_max: 8,
            op_max: 32,
            op_level: 1,
        }
    }
}

impl Config {
    /// The same with every number held to what means something.
    fn held(self) -> Self {
        Self {
            non_op_max: self.non_op_max.clamp(MIN_DISTANCE, MAX_DISTANCE),
            op_max: self.op_max.clamp(MIN_DISTANCE, MAX_DISTANCE),
            // Level 0 would make everyone an operator.
            op_level: self.op_level.clamp(1, 4),
            ..self
        }
    }

    /// The limit for a player of operator level `permission`.
    pub fn limit_for(&self, permission: u8) -> i32 {
        if permission >= self.op_level {
            self.op_max
        } else {
            self.non_op_max
        }
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

/// Reads `dir/viewdistance.toml`, writing the default one when there is none.
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

/// What a new `viewdistance.toml` holds.
pub const DEFAULT_FILE: &str = r#"# View distance limits. `/viewdistance reload` reads this file again.

# Turn this off and every player has the server's own view distance.
enabled = true

# The most chunks a player who is not an operator is sent around them. The server's own
# `view_distance` (`server.view_distance` in server.toml) is the ceiling: a limit here can only
# lower it. 2 to 32.
non-op-max = 8

# The same for an operator. With the server's `view_distance` at 8 and `max_view_distance` at 32,
# an operator whose client asks for 32 is sent the saved chunks past the 8 the server loads (the far
# ring, docs/hosting.md); a player is not. 2 to 32.
op-max = 32

# The lowest operator level (the `level` in ops.json) that counts as an operator here. 1 to 4.
op-level = 1
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
    fn numbers_are_held_to_what_the_engine_takes() {
        let cfg = parse("non-op-max = 1\nop-max = 99\nop-level = 0\n").unwrap();
        assert_eq!(
            (cfg.non_op_max, cfg.op_max, cfg.op_level),
            (2, 32, 1),
            "2 to 32 chunks, and level 0 is nobody special"
        );
        assert_eq!(parse("op-level = 9\n").unwrap().op_level, 4);
        assert_eq!(parse("non-op-max = 12\n").unwrap().non_op_max, 12);
    }

    #[test]
    fn a_mistake_is_told_and_not_taken_for_the_default() {
        assert!(parse("nonop-max = 4\n").unwrap_err().contains("nonop-max"));
        assert!(parse("non-op-max = 'eight'\n").is_err());
        assert!(parse("enabled = [").is_err());
    }

    #[test]
    fn the_limit_of_a_player_follows_their_operator_level() {
        let cfg = Config {
            op_level: 2,
            ..Config::default()
        };
        assert_eq!(cfg.limit_for(0), 8);
        assert_eq!(cfg.limit_for(1), 8, "below the level that counts");
        assert_eq!(cfg.limit_for(2), 32);
        assert_eq!(cfg.limit_for(4), 32);
    }

    #[test]
    fn a_missing_file_is_written_and_a_broken_one_is_an_error() {
        let dir = std::env::temp_dir().join(format!("viewdistance-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), Ok(Config::default()));
        assert_eq!(
            std::fs::read_to_string(path(&dir)).unwrap(),
            DEFAULT_FILE,
            "written for the operator to edit"
        );
        std::fs::write(path(&dir), "op-max = [").unwrap();
        assert!(
            load(&dir)
                .unwrap_err()
                .starts_with(&path(&dir).display().to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
