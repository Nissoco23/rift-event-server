//! The scythe: the item the plugin builds, written as the SNBT `Host::item_from_snbt` reads, and
//! how a stack is told to be one.
//!
//! It is a netherite hoe made of vanilla components only (a name, a lore, `item_model`,
//! `custom_model_data` and `custom_data`), so a stock client shows it with the resource pack and
//! needs no mod. It is recognised by its `custom_data` alone, written where Paper's persistent
//! data container writes it, so a scythe from the plugin works here and the other way round, and
//! renaming one at an anvil does not break it.

use horizon_client::mod_api::{Compound, Host, ItemStack, Tag};

pub const DISPLAY_NAME: &str = "Sculkstorm Scythe";

const BASE: &str = "minecraft:netherite_hoe";
const NAME_COLOR: &str = "#29DFEB";
const MODEL: &str = "nissoco:sculkstorm_scythe";
/// For a pack made for versions before `item_model` (the plugin's `LEGACY_CUSTOM_MODEL_DATA`).
const CUSTOM_MODEL_DATA: u32 = 730_102;
/// Where Paper's persistent data container keeps a plugin's values.
const BUKKIT_VALUES: &str = "PublicBukkitValues";
const WEAPON_KEY: &str = "sculkstorm:weapon";
const WEAPON: &str = "SCYTHE";

/// The lore, a line at a time with its colour; an empty line has none.
const LORE: [(&str, &str); 6] = [
    ("A scythe grown from the deep dark.", "dark_gray"),
    ("", ""),
    ("Sneak + Right-Click  »  Soulstorm Ascension", "aqua"),
    (
        "  Gather soul fire, burst outward and launch skyward.",
        "gray",
    ),
    ("Right-Click  »  Sculk Cataclysm", "aqua"),
    (
        "  Hurl a sculk core that blooms into a storm crater.",
        "gray",
    ),
];

/// The scythe as SNBT: `{id, count, components}`.
pub fn snbt() -> String {
    let lore: Vec<String> = LORE
        .iter()
        .map(|(text, color)| {
            if text.is_empty() {
                "{text:\"\"}".to_owned()
            } else {
                format!("{{text:\"{text}\",color:\"{color}\",italic:0b}}")
            }
        })
        .collect();
    format!(
        "{{id:\"{BASE}\",count:1,components:{{\
         \"minecraft:custom_name\":{{text:\"{DISPLAY_NAME}\",color:\"{NAME_COLOR}\",bold:1b,italic:0b}},\
         \"minecraft:lore\":[{lore}],\
         \"minecraft:unbreakable\":{{}},\
         \"minecraft:tooltip_display\":{{hidden_components:[\"minecraft:unbreakable\"]}},\
         \"minecraft:item_model\":\"{MODEL}\",\
         \"minecraft:custom_model_data\":{{floats:[{CUSTOM_MODEL_DATA}.0f]}},\
         \"minecraft:custom_data\":{{{BUKKIT_VALUES}:{{\"{WEAPON_KEY}\":\"{WEAPON}\"}}}}\
         }}}}",
        lore = lore.join(",")
    )
}

/// A new scythe; None when the world has no netherite hoe or does not read the components.
pub fn create(host: &dyn Host) -> Option<ItemStack> {
    host.item_from_snbt(&snbt())
}

/// Whether `stack` is a scythe: it carries the weapon tag as a string, whatever it is called.
pub fn is_scythe(stack: &ItemStack) -> bool {
    stack.count > 0
        && stack
            .custom_data()
            .and_then(|data| entry(data, BUKKIT_VALUES))
            .and_then(|values| values.get(WEAPON_KEY))
            .is_some_and(|tag| matches!(tag, Tag::String(_)))
}

fn entry<'a>(compound: &'a Compound, key: &str) -> Option<&'a Tag> {
    compound
        .iter()
        .find(|(k, _)| k.to_string_lossy() == key)
        .map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::mod_api::parse_snbt;

    /// The scythe as the engine's own item reader makes it (without the registry check
    /// `Host::item_from_snbt` adds).
    fn scythe() -> ItemStack {
        let (tag, used) = parse_snbt(&snbt()).expect("the SNBT is valid");
        assert_eq!(used, snbt().len(), "all of it was read");
        let Tag::Compound(compound) = tag else {
            panic!("a stack is a compound");
        };
        ItemStack::from_nbt(&compound).expect("the engine reads the stack")
    }

    #[test]
    fn the_engine_reads_the_scythe_as_a_netherite_hoe_called_what_the_plugin_calls_it() {
        let stack = scythe();
        assert_eq!(&*stack.item, "minecraft:netherite_hoe");
        assert_eq!(stack.count, 1);
        assert_eq!(stack.custom_name().as_deref(), Some("Sculkstorm Scythe"));
    }

    #[test]
    fn the_scythe_is_recognised_and_other_items_are_not() {
        assert!(is_scythe(&scythe()));
        assert!(!is_scythe(&ItemStack::new("minecraft:netherite_hoe", 1)));
        assert!(!is_scythe(&ItemStack::new("minecraft:stick", 1)));
        let mut empty = scythe();
        empty.count = 0;
        assert!(!is_scythe(&empty));
    }

    #[test]
    fn a_scythe_renamed_or_given_another_base_item_is_still_one() {
        // Only the tag counts: a stack that has it and nothing else is a scythe.
        let tagged = format!(
            "{{id:\"minecraft:stick\",count:1,components:{{\"minecraft:custom_data\":\
             {{{BUKKIT_VALUES}:{{\"{WEAPON_KEY}\":\"anything\"}}}}}}}}"
        );
        let (Tag::Compound(c), _) = parse_snbt(&tagged).unwrap() else {
            panic!()
        };
        assert!(is_scythe(&ItemStack::from_nbt(&c).unwrap()));
        // The tag as another type, or under another key, is not the weapon tag.
        for other in [
            format!("{{{BUKKIT_VALUES}:{{\"{WEAPON_KEY}\":1b}}}}"),
            format!("{{{BUKKIT_VALUES}:{{\"other:weapon\":\"SCYTHE\"}}}}"),
            "{weapon:\"SCYTHE\"}".to_owned(),
        ] {
            let text = format!(
                "{{id:\"minecraft:stick\",count:1,components:{{\"minecraft:custom_data\":{other}}}}}"
            );
            let (Tag::Compound(c), _) = parse_snbt(&text).unwrap() else {
                panic!()
            };
            assert!(!is_scythe(&ItemStack::from_nbt(&c).unwrap()), "{other}");
        }
    }

    #[test]
    fn the_lore_keeps_the_plugins_lines() {
        let text = snbt();
        for line in [
            "A scythe grown from the deep dark.",
            "Sneak + Right-Click  »  Soulstorm Ascension",
            "  Gather soul fire, burst outward and launch skyward.",
            "Right-Click  »  Sculk Cataclysm",
            "  Hurl a sculk core that blooms into a storm crater.",
        ] {
            assert!(text.contains(line), "{line}");
        }
        assert!(text.contains("nissoco:sculkstorm_scythe"));
        assert!(text.contains("730102.0f"));
    }
}
