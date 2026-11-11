//! What the crater may do to a block, from its name: the plugin's `isTerrain`, `isPlant`,
//! `isLiquid` and the rest of its block tests, over names and block tags in place of materials.

use horizon_client::mod_api::Host;
use std::collections::HashMap;

/// Block tags whose members are natural ground (the plugin's `Tag.DIRT`, `Tag.SAND`, the ore tags
/// and the rest).
const TERRAIN_TAGS: [&str; 12] = [
    "minecraft:dirt",
    "minecraft:base_stone_overworld",
    "minecraft:sand",
    "minecraft:terracotta",
    "minecraft:coal_ores",
    "minecraft:iron_ores",
    "minecraft:copper_ores",
    "minecraft:gold_ores",
    "minecraft:redstone_ores",
    "minecraft:lapis_ores",
    "minecraft:diamond_ores",
    "minecraft:emerald_ores",
];

/// Natural ground the tags above leave out.
const TERRAIN_BLOCKS: [&str; 15] = [
    "minecraft:calcite",
    "minecraft:dripstone_block",
    "minecraft:ice",
    "minecraft:packed_ice",
    "minecraft:gravel",
    "minecraft:clay",
    "minecraft:snow_block",
    "minecraft:sandstone",
    "minecraft:red_sandstone",
    "minecraft:cobbled_deepslate",
    "minecraft:sculk",
    "minecraft:netherrack",
    "minecraft:end_stone",
    "minecraft:soul_sand",
    "minecraft:soul_soil",
];

/// What the crater never breaks, whatever `break-all-blocks` says.
const UNBREAKABLE: [&str; 7] = [
    "minecraft:bedrock",
    "minecraft:barrier",
    "minecraft:light",
    "minecraft:structure_void",
    "minecraft:end_portal_frame",
    "minecraft:nether_portal",
    "minecraft:reinforced_deepslate",
];

/// Solid blocks that are not redstone conductors, which the plugin's `isOccluding` tells from
/// full blocks (`impermeable` and the leaves are asked as tags).
const NOT_OCCLUDING: [&str; 4] = [
    "minecraft:slime_block",
    "minecraft:honey_block",
    "minecraft:tinted_glass",
    "minecraft:spawner",
];

/// What a block's name says of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NameInfo {
    /// Air, cave air or void air.
    pub air: bool,
    /// Water or lava (the plugin's `Block.isLiquid`: a waterlogged block is not one).
    pub liquid: bool,
    /// Natural ground the crater always turns to sculk.
    pub terrain: bool,
    pub log: bool,
    pub leaves: bool,
    /// A flower, a sapling, or anything that can be replaced and is not a liquid or air: grass,
    /// ferns, snow layers, vines.
    pub plant: bool,
    pub unbreakable: bool,
    /// A full block that still is not a redstone conductor (glass, leaves, slime).
    pub see_through: bool,
}

/// `name`'s facts, asking `in_tag` whether it is in each block tag.
pub fn classify(name: &str, mut in_tag: impl FnMut(&str) -> bool) -> NameInfo {
    let air = matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    );
    let liquid = matches!(name, "minecraft:water" | "minecraft:lava");
    let terrain = TERRAIN_BLOCKS.contains(&name) || TERRAIN_TAGS.iter().any(|tag| in_tag(tag));
    let log = in_tag("minecraft:logs");
    let leaves = in_tag("minecraft:leaves");
    let plant = !air
        && (in_tag("minecraft:flowers")
            || in_tag("minecraft:saplings")
            || (!liquid && in_tag("minecraft:replaceable")));
    NameInfo {
        air,
        liquid,
        terrain,
        log,
        leaves,
        plant,
        unbreakable: UNBREAKABLE.contains(&name),
        see_through: leaves || NOT_OCCLUDING.contains(&name) || in_tag("minecraft:impermeable"),
    }
}

/// What each block name seen so far is, asked of the server's tags once.
#[derive(Default)]
pub struct NameCache(HashMap<String, NameInfo>);

impl NameCache {
    pub fn info(&mut self, host: &dyn Host, name: &str) -> NameInfo {
        if let Some(info) = self.0.get(name) {
            return *info;
        }
        let info = classify(name, |tag| {
            host.block_name_in_tag(name, tag).unwrap_or(false)
        });
        self.0.insert(name.to_owned(), info);
        info
    }
}

/// Whether a block's collision boxes (block-local, as `Host::block_collision` gives them) fill
/// the whole block.
pub fn full_cube(boxes: &[[f64; 6]]) -> bool {
    boxes.iter().any(|b| {
        b[0] <= 0.0 && b[1] <= 0.0 && b[2] <= 0.0 && b[3] >= 1.0 && b[4] >= 1.0 && b[5] >= 1.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tags a vanilla server has, for the names the tests use.
    fn tags(name: &str, tag: &str) -> bool {
        let members: &[&str] = match tag {
            "minecraft:dirt" => &["minecraft:dirt", "minecraft:grass_block", "minecraft:mud"],
            "minecraft:base_stone_overworld" => &["minecraft:stone", "minecraft:deepslate"],
            "minecraft:sand" => &["minecraft:sand", "minecraft:red_sand"],
            "minecraft:coal_ores" => &["minecraft:coal_ore", "minecraft:deepslate_coal_ore"],
            "minecraft:logs" => &["minecraft:oak_log", "minecraft:birch_log"],
            "minecraft:leaves" => &["minecraft:oak_leaves"],
            "minecraft:flowers" => &["minecraft:poppy", "minecraft:lilac"],
            "minecraft:saplings" => &["minecraft:oak_sapling"],
            "minecraft:replaceable" => &[
                "minecraft:air",
                "minecraft:short_grass",
                "minecraft:snow",
                "minecraft:water",
                "minecraft:lava",
            ],
            "minecraft:impermeable" => &["minecraft:glass"],
            _ => &[],
        };
        members.contains(&name)
    }

    fn info(name: &str) -> NameInfo {
        classify(name, |tag| tags(name, tag))
    }

    #[test]
    fn natural_ground_is_terrain_by_tag_or_by_name() {
        for name in [
            "minecraft:grass_block",
            "minecraft:stone",
            "minecraft:deepslate",
            "minecraft:red_sand",
            "minecraft:deepslate_coal_ore",
            "minecraft:gravel",
            "minecraft:sculk",
            "minecraft:netherrack",
        ] {
            assert!(info(name).terrain, "{name}");
        }
        for name in [
            "minecraft:oak_planks",
            "minecraft:obsidian",
            "minecraft:oak_log",
        ] {
            assert!(!info(name).terrain, "{name}");
        }
    }

    #[test]
    fn trees_plants_air_and_liquids_are_told_apart() {
        assert!(info("minecraft:oak_log").log && !info("minecraft:oak_log").leaves);
        assert!(info("minecraft:oak_leaves").leaves && info("minecraft:oak_leaves").see_through);
        for name in [
            "minecraft:poppy",
            "minecraft:oak_sapling",
            "minecraft:short_grass",
            "minecraft:snow",
        ] {
            assert!(info(name).plant, "{name}");
        }
        // Air and water can be replaced, but they are not plants to clear.
        assert!(info("minecraft:air").air && !info("minecraft:air").plant);
        assert!(info("minecraft:water").liquid && !info("minecraft:water").plant);
        assert!(info("minecraft:lava").liquid && !info("minecraft:lava").plant);
        assert!(!info("minecraft:stone").plant && !info("minecraft:stone").liquid);
    }

    #[test]
    fn the_plugins_unbreakable_blocks_are_unbreakable_and_glass_does_not_conduct() {
        for name in [
            "minecraft:bedrock",
            "minecraft:barrier",
            "minecraft:light",
            "minecraft:structure_void",
            "minecraft:end_portal_frame",
            "minecraft:nether_portal",
            "minecraft:reinforced_deepslate",
        ] {
            assert!(info(name).unbreakable, "{name}");
        }
        assert!(!info("minecraft:stone").unbreakable);
        assert!(info("minecraft:glass").see_through);
        assert!(info("minecraft:slime_block").see_through);
        assert!(!info("minecraft:stone").see_through);
    }

    #[test]
    fn a_full_cube_is_a_box_that_fills_the_block() {
        assert!(full_cube(&[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]));
        assert!(!full_cube(&[[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]]));
        assert!(!full_cube(&[
            [0.0, 0.0, 0.0, 1.0, 0.5, 1.0],
            [0.25, 0.0, 0.25, 0.75, 1.5, 0.75]
        ]));
        assert!(!full_cube(&[]));
    }
}
