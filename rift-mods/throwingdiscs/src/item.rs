//! The discs: the item the plugin builds, written as the SNBT `Host::item_from_snbt` reads, and how
//! a stack is told to be one.
//!
//! A disc is an echo shard made of vanilla components only (a name, lore, `item_model`, durability,
//! enchantability and `custom_data`), so a stock client shows it with the resource pack and needs no
//! mod. Echo shards have no right-click action and are not furnace fuel. A disc is recognised by its
//! tier in `custom_data`, written where Paper's persistent data container writes it, so a disc from
//! the plugin works here and the other way round.

use crate::config::{Config, TierStats};
use horizon_client::mod_api::{Compound, Host, ItemStack, Tag};

/// The item every disc is made of.
pub const BASE: &str = "minecraft:echo_shard";
pub const NAMESPACE: &str = "throwingdiscs";
/// Where Paper's persistent data container keeps a plugin's values.
const BUKKIT_VALUES: &str = "PublicBukkitValues";
const TIER_KEY: &str = "throwingdiscs:tier";

/// The enchantments a disc can have, with their highest level. Vanilla refuses all of them on an
/// echo shard, so the anvil and `/enchant` here use this list.
pub const ALLOWED_ENCHANTS: [(&str, u32); 4] = [
    ("minecraft:sharpness", 5),
    ("minecraft:knockback", 2),
    ("minecraft:unbreaking", 3),
    ("minecraft:mending", 1),
];

/// The highest level a disc can take of `enchantment`, if it can take it at all.
pub fn allowed_level(enchantment: &str) -> Option<u32> {
    ALLOWED_ENCHANTS
        .iter()
        .find(|(id, _)| *id == enchantment)
        .map(|(_, max)| *max)
}

/// The five tiers. Mojang's design has no wooden or stone disc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    Copper,
    Iron,
    Golden,
    Diamond,
    Netherite,
}

impl Tier {
    pub const ALL: [Tier; 5] = [
        Tier::Copper,
        Tier::Iron,
        Tier::Golden,
        Tier::Diamond,
        Tier::Netherite,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Tier::Copper => "copper",
            Tier::Iron => "iron",
            Tier::Golden => "golden",
            Tier::Diamond => "diamond",
            Tier::Netherite => "netherite",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Tier::Copper => "Copper Throwing Disc",
            Tier::Iron => "Iron Throwing Disc",
            Tier::Golden => "Golden Throwing Disc",
            Tier::Diamond => "Diamond Throwing Disc",
            Tier::Netherite => "Netherite Throwing Disc",
        }
    }

    /// The ingredient of the recipe, which also repairs the disc in an anvil.
    pub fn material(self) -> &'static str {
        match self {
            Tier::Copper => "minecraft:copper_ingot",
            Tier::Iron => "minecraft:iron_ingot",
            Tier::Golden => "minecraft:gold_ingot",
            Tier::Diamond => "minecraft:diamond",
            Tier::Netherite => "minecraft:netherite_ingot",
        }
    }

    /// The item model and the recipe key, for example `iron_throwing_disc`.
    pub fn item_id(self) -> String {
        format!("{}_throwing_disc", self.id())
    }

    /// Accepts `iron`, `iron_throwing_disc`, and `gold` for golden.
    pub fn by_id(id: &str) -> Option<Tier> {
        let lower = id.to_ascii_lowercase();
        let lower = if lower == "gold" { "golden" } else { &lower };
        Tier::ALL
            .into_iter()
            .find(|t| t.id() == lower || t.item_id() == lower)
    }
}

/// A number as the lore shows it: `7`, or `7.5`.
fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// The lore, a line at a time with its colour; an empty line has none.
fn lore(damage: f64) -> Vec<(String, &'static str)> {
    vec![
        (String::new(), ""),
        ("When thrown:".to_owned(), "gray"),
        (
            format!(" {} Projectile Damage", number(damage)),
            "dark_green",
        ),
        (" Pierces and returns".to_owned(), "dark_green"),
        (" Ricochets off hard surfaces".to_owned(), "dark_green"),
    ]
}

/// A disc of `tier` as SNBT: `{id, count, components}`.
pub fn snbt(tier: Tier, stats: &TierStats) -> String {
    let lore: Vec<String> = lore(stats.damage)
        .iter()
        .map(|(text, color)| {
            if text.is_empty() {
                "{text:\"\"}".to_owned()
            } else {
                format!("{{text:\"{text}\",color:\"{color}\",italic:0b}}")
            }
        })
        .collect();
    let fire_proof = if tier == Tier::Netherite {
        "\"minecraft:damage_resistant\":{types:\"#minecraft:is_fire\"},"
    } else {
        ""
    };
    format!(
        "{{id:\"{BASE}\",count:1,components:{{\
         \"minecraft:item_model\":\"{NAMESPACE}:{model}\",\
         \"minecraft:item_name\":{{text:\"{name}\"}},\
         \"minecraft:max_stack_size\":1,\
         \"minecraft:max_damage\":{durability},\
         \"minecraft:damage\":0,\
         \"minecraft:rarity\":\"common\",\
         \"minecraft:enchantable\":{{value:{enchantability}}},\
         \"minecraft:repairable\":{{items:\"{material}\"}},\
         {fire_proof}\
         \"minecraft:lore\":[{lore}],\
         \"minecraft:custom_data\":{{{BUKKIT_VALUES}:{{\"{TIER_KEY}\":\"{id}\"}}}}\
         }}}}",
        model = tier.item_id(),
        name = tier.display_name(),
        durability = stats.durability,
        enchantability = stats.enchantability,
        material = tier.material(),
        lore = lore.join(","),
        id = tier.id(),
    )
}

/// A new disc; None when the world does not read the components.
pub fn create(host: &dyn Host, tier: Tier, config: &Config) -> Option<ItemStack> {
    host.item_from_snbt(&snbt(tier, &config.stats(tier)))
}

/// The tier of `stack` if it is a disc: it carries the tier as a string, whatever it is called.
pub fn tier_of(stack: &ItemStack) -> Option<Tier> {
    if stack.count == 0 || &*stack.item != BASE {
        return None;
    }
    let values = entry(stack.custom_data()?, BUKKIT_VALUES)?;
    match entry_of(values, TIER_KEY)? {
        Tag::String(id) => Tier::by_id(&id.to_string_lossy()),
        _ => None,
    }
}

pub fn is_disc(stack: &ItemStack) -> bool {
    tier_of(stack).is_some()
}

fn entry<'a>(compound: &'a Compound, key: &str) -> Option<&'a Tag> {
    compound
        .iter()
        .find(|(k, _)| k.to_string_lossy() == key)
        .map(|(_, v)| v)
}

fn entry_of<'a>(tag: &'a Tag, key: &str) -> Option<&'a Tag> {
    match tag {
        Tag::Compound(c) => entry(c, key),
        _ => None,
    }
}

/// The disc's durability, from the stack: a disc keeps the durability it was made with when the config
/// changes later.
pub fn max_damage(stack: &ItemStack) -> Option<u32> {
    match stack.data()?.other("minecraft:max_damage")? {
        Tag::Int(n) => u32::try_from(*n).ok(),
        _ => None,
    }
}

/// The level of `enchantment` on the disc.
pub fn level(stack: &ItemStack, enchantment: &str) -> u32 {
    stack.enchantment_level(enchantment)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use horizon_client::mod_api::parse_snbt;

    /// A disc as the engine's own item reader makes it (without the registry check
    /// `Host::item_from_snbt` adds).
    pub fn disc(tier: Tier) -> ItemStack {
        let text = snbt(tier, &Config::default().stats(tier));
        let (tag, used) = parse_snbt(&text).expect("the SNBT is valid");
        assert_eq!(used, text.len(), "all of it was read");
        let Tag::Compound(compound) = tag else {
            panic!("a stack is a compound");
        };
        ItemStack::from_nbt(&compound).expect("the engine reads the stack")
    }

    #[test]
    fn a_disc_is_an_echo_shard_with_the_tiers_durability_and_is_recognised_by_its_tag() {
        for tier in Tier::ALL {
            let stack = disc(tier);
            assert_eq!(&*stack.item, "minecraft:echo_shard");
            assert_eq!(tier_of(&stack), Some(tier));
            assert_eq!(
                max_damage(&stack),
                Some(Config::default().stats(tier).durability as u32),
                "{tier:?}"
            );
            assert_eq!(stack.damage, 0);
        }
    }

    #[test]
    fn other_items_are_not_discs() {
        assert!(!is_disc(&ItemStack::new("minecraft:echo_shard", 1)));
        assert!(!is_disc(&ItemStack::new("minecraft:stick", 1)));
        let mut empty = disc(Tier::Iron);
        empty.count = 0;
        assert!(!is_disc(&empty));
    }

    #[test]
    fn a_disc_from_the_plugin_is_read_by_the_tag_it_writes() {
        let tagged = format!(
            "{{id:\"{BASE}\",count:1,components:{{\"minecraft:custom_data\":\
             {{{BUKKIT_VALUES}:{{\"{TIER_KEY}\":\"golden\"}}}}}}}}"
        );
        let (Tag::Compound(c), _) = parse_snbt(&tagged).unwrap() else {
            panic!()
        };
        assert_eq!(
            tier_of(&ItemStack::from_nbt(&c).unwrap()),
            Some(Tier::Golden)
        );
        // Another tier name, or another key, is no disc.
        for other in [
            format!("{{{BUKKIT_VALUES}:{{\"{TIER_KEY}\":\"wooden\"}}}}"),
            format!("{{{BUKKIT_VALUES}:{{\"other:tier\":\"iron\"}}}}"),
            "{tier:\"iron\"}".to_owned(),
        ] {
            let text = format!(
                "{{id:\"{BASE}\",count:1,components:{{\"minecraft:custom_data\":{other}}}}}"
            );
            let (Tag::Compound(c), _) = parse_snbt(&text).unwrap() else {
                panic!()
            };
            assert!(!is_disc(&ItemStack::from_nbt(&c).unwrap()), "{other}");
        }
    }

    #[test]
    fn tiers_are_found_by_name_item_id_and_the_gold_alias() {
        assert_eq!(Tier::by_id("IRON"), Some(Tier::Iron));
        assert_eq!(Tier::by_id("gold"), Some(Tier::Golden));
        assert_eq!(Tier::by_id("diamond_throwing_disc"), Some(Tier::Diamond));
        assert_eq!(Tier::by_id("wooden"), None);
    }

    #[test]
    fn the_lore_says_the_tiers_damage() {
        assert!(snbt(Tier::Iron, &TierStats::default()).contains("7 Projectile Damage"));
        let half = TierStats {
            damage: 7.5,
            ..TierStats::default()
        };
        assert!(snbt(Tier::Iron, &half).contains("7.5 Projectile Damage"));
    }

    #[test]
    fn only_the_four_enchantments_can_go_on_a_disc() {
        assert_eq!(allowed_level("minecraft:sharpness"), Some(5));
        assert_eq!(allowed_level("minecraft:knockback"), Some(2));
        assert_eq!(allowed_level("minecraft:unbreaking"), Some(3));
        assert_eq!(allowed_level("minecraft:mending"), Some(1));
        assert_eq!(allowed_level("minecraft:fire_aspect"), None);
    }
}
