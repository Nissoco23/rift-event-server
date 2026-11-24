//! The mod's configuration: `config/sculkstorm.toml`, with the keys and defaults of the Paper
//! plugin's `config.yml`.
//!
//! The file is written, with its comments, when it is missing. A value the plugin raised to a
//! floor is raised the same way here, and a few are held under a ceiling too, so a typo cannot
//! ask the server for a crater the size of the world.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "sculkstorm.toml";

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    /// Worlds where the scythe does not work: by world name (`world`) or dimension
    /// (`minecraft:overworld`).
    pub disabled_worlds: Vec<String>,
    /// Whether the abilities can hit players.
    pub pvp: bool,
    pub ascension: Ascension,
    pub cataclysm: Cataclysm,
    pub terrain: Terrain,
    /// Logs block counts and timings for each cataclysm.
    pub debug: bool,
    /// 0.5 is half the particles.
    pub particle_multiplier: f64,
    pub resource_pack: ResourcePack,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Ascension {
    pub cooldown_seconds: i64,
    /// The wind-up, in ticks.
    pub charge_ticks: i32,
    pub shockwave_radius: f64,
    pub damage: f64,
    pub knockback: f64,
    /// The upward velocity the user is launched with.
    pub launch_power: f64,
    pub fall_protection_seconds: i64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Cataclysm {
    pub cooldown_seconds: i64,
    /// Blocks a tick.
    pub projectile_speed: f64,
    pub max_range: f64,
    /// The radius entities are hurt in.
    pub impact_radius: f64,
    /// The damage between `lethal-radius` and `impact-radius`, falling off with distance.
    pub damage: f64,
    /// Everything inside dies, armour or not (a totem still works). 0 turns it off.
    pub lethal_radius: f64,
    pub darkness_seconds: i32,
    /// Only the effect: no fire and no damage.
    pub lightning_strikes: i32,
    pub lightning_duration_ticks: i32,
    /// How many cataclysms can go at once, on the whole server.
    pub max_active: i32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Terrain {
    pub enabled: bool,
    /// 33 is about 66 blocks across.
    pub core_radius: i32,
    pub crater_depth: i32,
    /// The number of sculk roots.
    pub tendrils: i32,
    pub tendril_length: i32,
    /// Whether builds break too (never bedrock, portals, spawners or anything with data), or only
    /// natural blocks.
    pub break_all_blocks: bool,
    /// The crater is built over a few seconds so it does not spike a tick.
    pub columns_per_tick: i32,
    /// Lower is smoother and slower.
    pub max_ms_per_tick: f64,
    /// Flying blocks for each impact.
    pub max_debris: i32,
    /// Put the land back afterwards.
    pub restore: bool,
    pub restore_delay_seconds: i64,
    pub restore_blocks_per_tick: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ResourcePack {
    pub enabled: bool,
    pub url: String,
    pub sha1: String,
    pub required: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            disabled_worlds: Vec::new(),
            pvp: true,
            ascension: Ascension::default(),
            cataclysm: Cataclysm::default(),
            terrain: Terrain::default(),
            debug: false,
            particle_multiplier: 1.0,
            resource_pack: ResourcePack::default(),
        }
    }
}

impl Default for Ascension {
    fn default() -> Self {
        Self {
            cooldown_seconds: 15,
            charge_ticks: 40,
            shockwave_radius: 8.0,
            damage: 8.0,
            knockback: 1.6,
            launch_power: 2.2,
            fall_protection_seconds: 12,
        }
    }
}

impl Default for Cataclysm {
    fn default() -> Self {
        Self {
            cooldown_seconds: 30,
            projectile_speed: 1.6,
            max_range: 80.0,
            impact_radius: 33.0,
            damage: 14.0,
            lethal_radius: 13.0,
            darkness_seconds: 8,
            lightning_strikes: 45,
            lightning_duration_ticks: 100,
            max_active: 3,
        }
    }
}

impl Default for Terrain {
    fn default() -> Self {
        Self {
            enabled: true,
            core_radius: 33,
            crater_depth: 10,
            tendrils: 24,
            tendril_length: 50,
            break_all_blocks: true,
            columns_per_tick: 80,
            max_ms_per_tick: 4.0,
            max_debris: 80,
            restore: true,
            restore_delay_seconds: 90,
            restore_blocks_per_tick: 400,
        }
    }
}

impl Config {
    /// The values as the plugin reads them: one under its floor is raised to it, and one over a
    /// ceiling is lowered to it.
    pub fn sanitized(mut self) -> Self {
        let a = &mut self.ascension;
        a.cooldown_seconds = a.cooldown_seconds.clamp(0, 86_400);
        a.charge_ticks = a.charge_ticks.clamp(10, 1_200);
        a.shockwave_radius = finite(a.shockwave_radius, 8.0).clamp(0.5, 64.0);
        a.damage = finite(a.damage, 8.0).clamp(0.0, 1_000_000.0);
        a.knockback = finite(a.knockback, 1.6).clamp(0.0, 4.0);
        a.launch_power = finite(a.launch_power, 2.2).clamp(0.0, 3.9);
        a.fall_protection_seconds = a.fall_protection_seconds.clamp(0, 3_600);

        let c = &mut self.cataclysm;
        c.cooldown_seconds = c.cooldown_seconds.clamp(0, 86_400);
        c.projectile_speed = finite(c.projectile_speed, 1.6).clamp(0.3, 8.0);
        c.max_range = finite(c.max_range, 80.0).clamp(1.0, 1_024.0);
        c.impact_radius = finite(c.impact_radius, 33.0).clamp(1.0, 128.0);
        c.damage = finite(c.damage, 14.0).clamp(0.0, 1_000_000.0);
        c.lethal_radius = finite(c.lethal_radius, 13.0).clamp(0.0, 128.0);
        c.darkness_seconds = c.darkness_seconds.clamp(0, 600);
        c.lightning_strikes = c.lightning_strikes.clamp(0, 500);
        c.lightning_duration_ticks = c.lightning_duration_ticks.clamp(1, 1_200);
        c.max_active = c.max_active.clamp(1, 32);

        let t = &mut self.terrain;
        t.core_radius = t.core_radius.clamp(3, 120);
        t.crater_depth = t.crater_depth.clamp(0, 64);
        t.tendrils = t.tendrils.clamp(0, 96);
        t.tendril_length = t.tendril_length.clamp(0, 160);
        t.columns_per_tick = t.columns_per_tick.clamp(1, 2_048);
        t.max_ms_per_tick = finite(t.max_ms_per_tick, 4.0).clamp(0.5, 50.0);
        t.max_debris = t.max_debris.clamp(0, 400);
        t.restore_delay_seconds = t.restore_delay_seconds.clamp(1, 86_400);
        t.restore_blocks_per_tick = t.restore_blocks_per_tick.clamp(1, 20_000);

        self.particle_multiplier = finite(self.particle_multiplier, 1.0).clamp(0.0, 16.0);
        self
    }

    /// Whether `dimension` (`minecraft:overworld`), whose world is called `world`, is one the
    /// scythe is turned off in.
    pub fn is_disabled(&self, dimension: &str, world: Option<&str>) -> bool {
        self.disabled_worlds
            .iter()
            .any(|w| w == dimension || world.is_some_and(|name| w == name))
    }

    /// The pack's hash if it is a SHA-1 (40 hexadecimal digits), else none, as the plugin takes
    /// it.
    pub fn pack_sha1(&self) -> Option<&str> {
        let sha1 = self.resource_pack.sha1.trim();
        (sha1.len() == 40 && sha1.bytes().all(|b| b.is_ascii_hexdigit())).then_some(sha1)
    }
}

fn finite(value: f64, default: f64) -> f64 {
    if value.is_finite() { value } else { default }
}

/// The config in `text`, or why it does not read.
pub fn parse(text: &str) -> Result<Config, String> {
    toml::from_str::<Config>(text)
        .map(Config::sanitized)
        .map_err(|e| e.to_string())
}

/// `dir`'s `sculkstorm.toml`.
pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Reads the config in `dir`, writing the default (with its comments) first when there is none.
/// A file that does not read is an error, and the caller keeps what it had.
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

/// What a new `sculkstorm.toml` holds.
pub const DEFAULT_FILE: &str = r#"# Sculkstorm configuration. `/sculkstorm reload` reads this file again.

# Worlds where the scythe does not work, by world name ("world") or dimension ("minecraft:the_end").
disabled-worlds = []

# Whether the abilities can hit players.
pvp = true

# Logs block counts and timings for each cataclysm.
debug = false

# 0.5 is half the particles.
particle-multiplier = 1.0

# Sneak + right-click.
[ascension]
cooldown-seconds = 15
charge-ticks = 40           # the wind-up (2 s)
shockwave-radius = 8.0
damage = 8.0
knockback = 1.6
launch-power = 2.2          # the upward velocity the user is launched with
fall-protection-seconds = 12

# Right-click.
[cataclysm]
cooldown-seconds = 30
projectile-speed = 1.6      # blocks a tick
max-range = 80.0
impact-radius = 33.0        # the radius entities are hurt in
damage = 14.0               # the damage between lethal-radius and impact-radius, falling off with distance
# Everything inside this radius dies, armour or not (a totem still works). 0 turns it off.
lethal-radius = 13.0
darkness-seconds = 8
lightning-strikes = 45      # only the effect: no fire and no damage
lightning-duration-ticks = 100
# How many cataclysms can go at once, on the whole server.
max-active = 3

[terrain]
enabled = true
core-radius = 33            # 33 is about 66 blocks across
crater-depth = 10
tendrils = 24               # the number of sculk roots
tendril-length = 50
# true: builds break too (never bedrock, portals, spawners, or anything with data such as chests and signs).
# false: only natural blocks such as dirt, stone and sand.
break-all-blocks = true
# The crater is built over a few seconds so it does not spike a tick.
columns-per-tick = 80
max-ms-per-tick = 4.0       # lower is smoother and slower
max-debris = 80             # flying blocks for each impact
# Put the land back afterwards. Crater blocks drop nothing while they wait.
restore = true
restore-delay-seconds = 90
restore-blocks-per-tick = 400

# Sends the resource pack (the scythe's model) when a player joins.
[resource-pack]
enabled = false
url = ""
sha1 = ""
required = false
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_a_new_server_gets_reads_as_the_defaults() {
        assert_eq!(parse(DEFAULT_FILE).unwrap(), Config::default());
    }

    #[test]
    fn the_defaults_are_the_plugins() {
        let c = Config::default();
        assert_eq!(c.ascension.cooldown_seconds, 15);
        assert_eq!(c.ascension.charge_ticks, 40);
        assert_eq!(c.cataclysm.impact_radius, 33.0);
        assert_eq!(c.cataclysm.lethal_radius, 13.0);
        assert_eq!(c.cataclysm.max_active, 3);
        assert_eq!(c.terrain.core_radius, 33);
        assert_eq!(c.terrain.restore_delay_seconds, 90);
        assert_eq!(c.terrain.restore_blocks_per_tick, 400);
        assert!(c.pvp && c.terrain.enabled && c.terrain.break_all_blocks && c.terrain.restore);
        assert!(!c.resource_pack.enabled);
    }

    #[test]
    fn a_missing_key_keeps_its_default_and_a_given_one_wins() {
        let c = parse("pvp = false\n[ascension]\ndamage = 3\n").unwrap();
        assert!(!c.pvp);
        assert_eq!(c.ascension.damage, 3.0);
        assert_eq!(c.ascension.knockback, 1.6);
        assert_eq!(c.cataclysm, Cataclysm::default());
    }

    #[test]
    fn values_under_the_plugins_floors_are_raised_to_them() {
        let c = parse(
            "[ascension]\ncharge-ticks = 1\n[cataclysm]\nprojectile-speed = 0.0\nmax-active = 0\n\
             [terrain]\ncore-radius = 0\ncolumns-per-tick = 0\nmax-ms-per-tick = 0.0\n\
             restore-delay-seconds = 0\nrestore-blocks-per-tick = 0\n",
        )
        .unwrap();
        assert_eq!(c.ascension.charge_ticks, 10);
        assert_eq!(c.cataclysm.projectile_speed, 0.3);
        assert_eq!(c.cataclysm.max_active, 1);
        assert_eq!(c.terrain.core_radius, 3);
        assert_eq!(c.terrain.columns_per_tick, 1);
        assert_eq!(c.terrain.max_ms_per_tick, 0.5);
        assert_eq!(c.terrain.restore_delay_seconds, 1);
        assert_eq!(c.terrain.restore_blocks_per_tick, 1);
    }

    #[test]
    fn a_size_that_would_swallow_the_server_is_held_under_a_ceiling() {
        let c = parse("[terrain]\ncore-radius = 100000\ntendril-length = 100000\n").unwrap();
        assert_eq!(c.terrain.core_radius, 120);
        assert_eq!(c.terrain.tendril_length, 160);
    }

    #[test]
    fn a_negative_multiplier_is_none_and_a_bad_file_is_an_error() {
        assert_eq!(
            parse("particle-multiplier = -3.0")
                .unwrap()
                .particle_multiplier,
            0.0
        );
        assert!(parse("pvp = \"yes\"").is_err());
        assert!(parse("[ascension\n").is_err());
    }

    #[test]
    fn a_world_is_disabled_by_name_or_by_dimension() {
        let c = parse("disabled-worlds = [\"creative\", \"minecraft:the_end\"]").unwrap();
        assert!(c.is_disabled("minecraft:overworld", Some("creative")));
        assert!(c.is_disabled("minecraft:the_end", Some("world_the_end")));
        assert!(!c.is_disabled("minecraft:overworld", Some("world")));
        assert!(!c.is_disabled("minecraft:overworld", None));
    }

    #[test]
    fn only_a_forty_digit_hash_is_sent_with_the_pack() {
        let mut c = Config::default();
        c.resource_pack.sha1 = "da39a3ee5e6b4b0d3255bfef95601890afd80709".into();
        assert_eq!(
            c.pack_sha1(),
            Some("da39a3ee5e6b4b0d3255bfef95601890afd80709")
        );
        c.resource_pack.sha1 = "short".into();
        assert_eq!(c.pack_sha1(), None);
        c.resource_pack.sha1 = "z".repeat(40);
        assert_eq!(c.pack_sha1(), None);
    }

    #[test]
    fn load_writes_the_default_once_and_reads_it_back() {
        let dir = std::env::temp_dir().join(format!("sculkstorm-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir).unwrap(), Config::default());
        assert_eq!(std::fs::read_to_string(path(&dir)).unwrap(), DEFAULT_FILE);
        std::fs::write(path(&dir), "pvp = false\n").unwrap();
        assert!(!load(&dir).unwrap().pvp);
        std::fs::write(path(&dir), "pvp = [").unwrap();
        assert!(load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
