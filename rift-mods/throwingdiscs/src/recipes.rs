//! The recipes, as the data pack files the mod adds: four shaped recipes (copper, iron, gold and
//! diamond) and the smithing upgrade to netherite. Each result is the disc with its components.
//! A recipe's base slot cannot ask for a disc, so the upgrade's result is checked when the table
//! makes it (`smith.rs`).

use crate::config::Config;
use crate::item::{BASE, NAMESPACE, Tier};
use serde_json::{Value, json};

/// A disc's components as a recipe result writes them.
fn result(tier: Tier, config: &Config) -> Value {
    let stats = config.stats(tier);
    let number = |v: f64| {
        if v.fract() == 0.0 {
            format!("{}", v as i64)
        } else {
            format!("{v}")
        }
    };
    let mut components = json!({
        "minecraft:item_model": format!("{NAMESPACE}:{}", tier.item_id()),
        "minecraft:item_name": { "text": tier.display_name() },
        "minecraft:max_stack_size": 1,
        "minecraft:max_damage": stats.durability,
        "minecraft:damage": 0,
        "minecraft:rarity": "common",
        "minecraft:enchantable": { "value": stats.enchantability },
        "minecraft:repairable": { "items": tier.material() },
        "minecraft:lore": [
            { "text": "" },
            { "text": "When thrown:", "color": "gray", "italic": false },
            { "text": format!(" {} Projectile Damage", number(stats.damage)), "color": "dark_green", "italic": false },
            { "text": " Pierces and returns", "color": "dark_green", "italic": false },
            { "text": " Ricochets off hard surfaces", "color": "dark_green", "italic": false },
        ],
        "minecraft:custom_data": {
            "PublicBukkitValues": { "throwingdiscs:tier": tier.id() }
        },
    });
    if tier == Tier::Netherite {
        components["minecraft:damage_resistant"] = json!({ "types": "#minecraft:is_fire" });
    }
    json!({ "id": BASE, "count": 1, "components": components })
}

/// The files to add to the world's data: nothing when the crafting switch is off.
pub fn files(config: &Config) -> Vec<(String, Vec<u8>)> {
    if !config.crafting {
        return Vec::new();
    }
    let mut files = Vec::new();
    for tier in Tier::ALL {
        if tier == Tier::Netherite {
            continue;
        }
        let recipe = json!({
            "type": "minecraft:crafting_shaped",
            "category": "equipment",
            "group": "throwing_disc",
            "pattern": [" X ", "X X", " X "],
            "key": { "X": tier.material() },
            "result": result(tier, config),
        });
        files.push((key(&tier.item_id()), recipe.to_string().into_bytes()));
    }
    let smithing = json!({
        "type": "minecraft:smithing_transform",
        "template": "minecraft:netherite_upgrade_smithing_template",
        "base": BASE,
        "addition": "minecraft:netherite_ingot",
        "result": result(Tier::Netherite, config),
    });
    files.push((
        key(&format!("{}_smithing", Tier::Netherite.item_id())),
        smithing.to_string().into_bytes(),
    ));
    files
}

/// A recipe's key in `native_data_pack_files`: `recipe/<namespace>:<name>`.
fn key(name: &str) -> String {
    format!("recipe/{NAMESPACE}:{name}")
}

/// The recipe ids a player is shown in their recipe book (`/recipe give`).
pub fn ids() -> Vec<String> {
    Tier::ALL
        .into_iter()
        .map(|t| {
            if t == Tier::Netherite {
                format!("{NAMESPACE}:{}_smithing", t.item_id())
            } else {
                format!("{NAMESPACE}:{}", t.item_id())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("a recipe is JSON")
    }

    #[test]
    fn four_plus_shaped_recipes_and_a_smithing_upgrade_with_the_discs_as_results() {
        let files = files(&Config::default());
        let keys: Vec<&str> = files.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "recipe/throwingdiscs:copper_throwing_disc",
                "recipe/throwingdiscs:iron_throwing_disc",
                "recipe/throwingdiscs:golden_throwing_disc",
                "recipe/throwingdiscs:diamond_throwing_disc",
                "recipe/throwingdiscs:netherite_throwing_disc_smithing",
            ]
        );
        let iron = parse(&files[1].1);
        assert_eq!(iron["pattern"], json!([" X ", "X X", " X "]));
        assert_eq!(iron["key"]["X"], "minecraft:iron_ingot");
        assert_eq!(iron["result"]["id"], "minecraft:echo_shard");
        assert_eq!(
            iron["result"]["components"]["minecraft:custom_data"]["PublicBukkitValues"]["throwingdiscs:tier"],
            "iron"
        );
        assert_eq!(iron["result"]["components"]["minecraft:max_damage"], 250);
        let gold = parse(&files[2].1);
        assert_eq!(gold["key"]["X"], "minecraft:gold_ingot");
        let smithing = parse(&files[4].1);
        assert_eq!(smithing["type"], "minecraft:smithing_transform");
        assert_eq!(smithing["base"], "minecraft:echo_shard");
        assert_eq!(
            smithing["result"]["components"]["minecraft:damage_resistant"]["types"],
            "#minecraft:is_fire"
        );
    }

    #[test]
    fn a_recipes_result_follows_the_configs_stats() {
        let mut config = Config::default();
        config.tiers.diamond.damage = 8.5;
        config.tiers.diamond.durability = 999;
        let files = files(&config);
        let diamond = parse(&files[3].1);
        assert_eq!(diamond["result"]["components"]["minecraft:max_damage"], 999);
        let lore = diamond["result"]["components"]["minecraft:lore"].to_string();
        assert!(lore.contains("8.5 Projectile Damage"));
    }

    #[test]
    fn with_crafting_off_there_are_no_recipes() {
        let config = Config {
            crafting: false,
            ..Config::default()
        };
        assert!(files(&config).is_empty());
    }
}
