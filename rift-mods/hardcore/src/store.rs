//! `config/hardcore.toml`: what the mod keeps across a restart, written by the mod itself each
//! time a command changes it.

use crate::state::{Cycle, DEFAULT_RADIUS, DeathMsg};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const FILE_NAME: &str = "hardcore.toml";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub deathmsg: DeathMsg,
    pub deathmsgradius: i32,
    /// Each player's protection, by UUID.
    pub protection: BTreeMap<String, SavedProtection>,
    /// Death cycling (`/cycledeath`).
    pub cycle: Cycle,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            deathmsg: DeathMsg::Leave,
            deathmsgradius: DEFAULT_RADIUS,
            protection: BTreeMap::new(),
            cycle: Cycle::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedProtection {
    pub enabled: bool,
    pub level: u8,
}

impl Default for SavedProtection {
    fn default() -> Self {
        Self {
            enabled: false,
            level: 1,
        }
    }
}

/// A UUID as `8-4-4-4-12` hexadecimal digits.
pub fn uuid_string(uuid: u128) -> String {
    let h = format!("{uuid:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

pub fn parse_uuid(text: &str) -> Option<u128> {
    let digits: String = text.chars().filter(|c| *c != '-').collect();
    (digits.len() == 32)
        .then(|| u128::from_str_radix(&digits, 16).ok())
        .flatten()
}

/// Reads the file in `dir`: the defaults when there is none. A file that cannot be read is moved
/// aside (`hardcore.toml.broken`), so the next save does not overwrite it, and told.
pub fn load(dir: &Path) -> Result<Saved, String> {
    let path = dir.join(FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Saved::default()),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    toml::from_str(&text).map_err(|e| {
        let aside = dir.join(format!("{FILE_NAME}.broken"));
        let moved = std::fs::rename(&path, &aside).is_ok();
        format!(
            "{} is not valid ({}){}",
            path.display(),
            e.message(),
            if moved {
                format!("; it was moved to {}", aside.display())
            } else {
                String::new()
            }
        )
    })
}

/// Writes `saved` to the file in `dir`, whole or not at all.
pub fn save(dir: &Path, saved: &Saved) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let text = toml::to_string(saved).map_err(|e| format!("could not write the file: {e}"))?;
    let path = dir.join(FILE_NAME);
    let temp = dir.join(format!("{FILE_NAME}.tmp"));
    std::fs::write(&temp, text).map_err(|e| format!("could not write {}: {e}", temp.display()))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("could not replace {}: {e}", path.display()))
}
