//! The mod's configuration: `config/throwingdiscs.toml`, with the keys and defaults of the Paper
//! plugin's `config.yml`, and a switch for the crafting and smithing recipes.
//!
//! The file is written, with its comments, when it is missing. Values outside what the flight can
//! use are brought into range, so a typo cannot make a disc stand still or cross the world in a tick.

use crate::item::Tier;
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "throwingdiscs.toml";

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    pub tiers: Tiers,
    pub flight: Flight,
    pub ricochet: Ricochet,
    /// The disc bounces off every block except these: block ids, or block tags starting with `#`.
    pub soft_blocks: Vec<String>,
    /// Whether the disc breaks decorated pots in its path and keeps flying.
    pub break_decorated_pots: bool,
    /// Whether discs can hurt players.
    pub pvp: bool,
    /// Durability used per throw. Unbreaking applies.
    pub durability_per_throw: i32,
    /// Whether the crafting and smithing recipes exist. Off, discs come from `/discs give` alone.
    pub crafting: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Tiers {
    pub copper: TierStats,
    pub iron: TierStats,
    pub golden: TierStats,
    pub diamond: TierStats,
    pub netherite: TierStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct TierStats {
    /// Damage per hit, in half-hearts, before Sharpness.
    pub damage: f64,
    pub durability: i32,
    pub enchantability: i32,
    /// Blocks the disc flies before it turns back.
    pub range: f64,
    /// Blocks per tick.
    pub speed: f64,
}

impl Default for TierStats {
    fn default() -> Self {
        Self {
            damage: 7.0,
            durability: 250,
            enchantability: 14,
            range: 11.0,
            speed: 1.05,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Flight {
    /// The push per hit with no Knockback enchantment: 0.25 moves a mob about a block.
    pub knockback: f64,
    /// Added per Knockback level.
    pub knockback_per_level: f64,
    /// Blocks a tick when the disc starts coming back.
    pub return_speed: f64,
    /// Added to the return speed every tick.
    pub return_acceleration: f64,
    pub max_return_speed: f64,
    /// The player catches the disc within this many blocks.
    pub catch_distance: f64,
    /// A disc still flying after this many ticks goes back into the inventory.
    pub max_lifetime_ticks: i64,
    pub spin_degrees_per_tick: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Ricochet {
    pub max_bounces: i32,
    /// Each bounce multiplies the speed by this.
    pub speed_multiplier: f64,
    pub max_speed: f64,
    /// Each bounce adds this many blocks of range.
    pub extra_range: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tiers: Tiers::default(),
            flight: Flight::default(),
            ricochet: Ricochet::default(),
            soft_blocks: [
                "#minecraft:leaves",
                "#minecraft:wool",
                "minecraft:hay_block",
                "minecraft:slime_block",
                "minecraft:honey_block",
                "minecraft:moss_block",
                "minecraft:pale_moss_block",
                "minecraft:sponge",
                "minecraft:wet_sponge",
                "minecraft:snow_block",
                "minecraft:powder_snow",
                "minecraft:target",
            ]
            .map(str::to_owned)
            .to_vec(),
            break_decorated_pots: true,
            pvp: true,
            durability_per_throw: 1,
            crafting: true,
        }
    }
}

impl Default for Tiers {
    fn default() -> Self {
        let stats = |damage, durability, enchantability, range, speed| TierStats {
            damage,
            durability,
            enchantability,
            range,
            speed,
        };
        Self {
            copper: stats(6.0, 190, 13, 10.0, 1.0),
            iron: stats(7.0, 250, 14, 11.0, 1.05),
            golden: stats(6.0, 32, 22, 12.0, 1.2),
            diamond: stats(8.0, 1561, 10, 12.0, 1.1),
            netherite: stats(9.0, 2031, 15, 13.0, 1.15),
        }
    }
}

impl Default for Flight {
    fn default() -> Self {
        Self {
            knockback: 0.25,
            knockback_per_level: 0.5,
            return_speed: 1.1,
            return_acceleration: 0.06,
            max_return_speed: 2.2,
            catch_distance: 1.4,
            max_lifetime_ticks: 400,
            spin_degrees_per_tick: 40.0,
        }
    }
}

impl Default for Ricochet {
    fn default() -> Self {
        Self {
            max_bounces: 6,
            speed_multiplier: 1.2,
            max_speed: 2.0,
            extra_range: 6.0,
        }
    }
}

impl Config {
    /// The stats of `tier`.
    pub fn stats(&self, tier: Tier) -> TierStats {
        match tier {
            Tier::Copper => self.tiers.copper,
            Tier::Iron => self.tiers.iron,
            Tier::Golden => self.tiers.golden,
            Tier::Diamond => self.tiers.diamond,
            Tier::Netherite => self.tiers.netherite,
        }
    }

    /// The same with every number brought into what the flight can use.
    pub fn sanitized(mut self) -> Self {
        for tier in Tier::ALL {
            let stats = match tier {
                Tier::Copper => &mut self.tiers.copper,
                Tier::Iron => &mut self.tiers.iron,
                Tier::Golden => &mut self.tiers.golden,
                Tier::Diamond => &mut self.tiers.diamond,
                Tier::Netherite => &mut self.tiers.netherite,
            };
            stats.damage = stats.damage.clamp(0.0, 1000.0);
            stats.durability = stats.durability.clamp(2, 32_767);
            stats.enchantability = stats.enchantability.clamp(1, 100);
            stats.range = stats.range.clamp(1.0, 128.0);
            stats.speed = stats.speed.clamp(0.1, 4.0);
        }
        let f = &mut self.flight;
        f.knockback = f.knockback.clamp(0.0, 10.0);
        f.knockback_per_level = f.knockback_per_level.clamp(0.0, 10.0);
        f.return_speed = f.return_speed.clamp(0.1, 4.0);
        f.return_acceleration = f.return_acceleration.clamp(0.0, 2.0);
        f.max_return_speed = f.max_return_speed.clamp(f.return_speed, 4.0);
        f.catch_distance = f.catch_distance.clamp(0.5, 8.0);
        f.max_lifetime_ticks = f.max_lifetime_ticks.clamp(20, 24_000);
        f.spin_degrees_per_tick = f.spin_degrees_per_tick.clamp(0.0, 170.0);
        let r = &mut self.ricochet;
        r.max_bounces = r.max_bounces.clamp(0, 64);
        r.speed_multiplier = r.speed_multiplier.clamp(0.1, 4.0);
        r.max_speed = r.max_speed.clamp(0.1, 4.0);
        r.extra_range = r.extra_range.clamp(0.0, 128.0);
        self.durability_per_throw = self.durability_per_throw.clamp(0, 100);
        self
    }
}

/// The file's config, read from text.
pub fn parse(text: &str) -> Result<Config, String> {
    toml::from_str::<Config>(text)
        .map(Config::sanitized)
        .map_err(|e| e.to_string())
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Reads `throwingdiscs.toml` in `dir`, writing the default one first when there is none.
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

/// What a new `throwingdiscs.toml` holds.
pub const DEFAULT_FILE: &str = r##"# Throwing discs configuration. `/discs reload` reads this file again.
# Mojang has published one number: the iron disc deals 7 damage. The other values are estimates.
# Discs that already exist keep their old durability and lore.

# Whether the crafting and smithing recipes exist. Off, discs come from /discs give alone.
crafting = true

# Whether discs can hurt players.
pvp = true

# Durability used per throw. Unbreaking applies.
durability-per-throw = 1

# Whether the disc breaks decorated pots in its path and keeps flying.
break-decorated-pots = true

# The disc bounces off every block except these. Hitting one sends the disc straight back.
# Block ids, or block tags that start with #.
soft-blocks = [
  "#minecraft:leaves",
  "#minecraft:wool",
  "minecraft:hay_block",
  "minecraft:slime_block",
  "minecraft:honey_block",
  "minecraft:moss_block",
  "minecraft:pale_moss_block",
  "minecraft:sponge",
  "minecraft:wet_sponge",
  "minecraft:snow_block",
  "minecraft:powder_snow",
  "minecraft:target",
]

# Damage per hit is the tier's damage plus Sharpness: +1 at level I and +0.5 per level after that.
[tiers.copper]
damage = 6.0           # damage per hit, in half-hearts
durability = 190
enchantability = 13
range = 10.0           # blocks the disc flies before it turns back
speed = 1.0            # blocks per tick

[tiers.iron]
damage = 7.0           # Mojang's published value
durability = 250
enchantability = 14
range = 11.0
speed = 1.05

[tiers.golden]
damage = 6.0
durability = 32
enchantability = 22
range = 12.0
speed = 1.2

[tiers.diamond]
damage = 8.0
durability = 1561
enchantability = 10
range = 12.0
speed = 1.1

[tiers.netherite]
damage = 9.0
durability = 2031
enchantability = 15
range = 13.0
speed = 1.15

[flight]
knockback = 0.25               # push per hit with no Knockback enchantment: 0.25 moves a mob about 1 block
knockback-per-level = 0.5      # added per Knockback level, so I = 0.75 and II = 1.25
return-speed = 1.1             # blocks per tick when the disc starts coming back
return-acceleration = 0.06     # added to the return speed every tick
max-return-speed = 2.2
catch-distance = 1.4           # the player catches the disc within this many blocks
max-lifetime-ticks = 400       # a disc still flying after this many ticks goes back into the inventory
spin-degrees-per-tick = 40.0

[ricochet]
max-bounces = 6
speed-multiplier = 1.2         # each bounce multiplies the speed by this
max-speed = 2.0
extra-range = 6.0              # each bounce adds this many blocks of range
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_a_new_server_gets_is_the_default() {
        assert_eq!(parse(DEFAULT_FILE).unwrap(), Config::default());
    }

    #[test]
    fn a_missing_key_keeps_its_default_and_a_wrong_one_is_an_error() {
        let config = parse("[tiers.iron]\ndamage = 9.5\n").unwrap();
        assert_eq!(config.tiers.iron.damage, 9.5);
        assert_eq!(config.tiers.iron.durability, 250, "the rest of the tier");
        assert_eq!(config.tiers.copper, Config::default().tiers.copper);
        assert!(parse("[tiers.iron]\ndamage = \"lots\"\n").is_err());
    }

    #[test]
    fn numbers_outside_what_the_flight_can_use_are_brought_into_range() {
        let config = parse(
            "[tiers.iron]\nspeed = 0.0\nrange = 100000.0\ndurability = 1\n\
             [flight]\ncatch-distance = 100.0\nmax-lifetime-ticks = 1\n",
        )
        .unwrap();
        assert_eq!(config.tiers.iron.speed, 0.1);
        assert_eq!(config.tiers.iron.range, 128.0);
        assert_eq!(config.tiers.iron.durability, 2);
        assert_eq!(config.flight.catch_distance, 8.0);
        assert_eq!(config.flight.max_lifetime_ticks, 20);
    }

    #[test]
    fn tiers_scale_up_the_default_damage_ladder() {
        let c = Config::default();
        let damage = |t| c.stats(t).damage;
        assert!(damage(Tier::Copper) <= damage(Tier::Iron));
        assert!(damage(Tier::Iron) < damage(Tier::Diamond));
        assert!(damage(Tier::Diamond) < damage(Tier::Netherite));
    }
}
