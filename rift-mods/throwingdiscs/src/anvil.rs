//! What an anvil does with a disc: books and a second disc of the same tier. Renaming alone and
//! repair with the tier's own material are left to vanilla, which handles them through the
//! `repairable` component.

use crate::item;
use horizon_client::item_data::{Levels, set_level};
use horizon_client::mod_api::{ItemStack, Tag};
use std::sync::Arc;

/// The most levels an anvil charges before it says "Too Expensive!".
const MOST_LEVELS: i32 = 39;

/// What the anvil shows for the inputs.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Vanilla's own result stands.
    Vanilla,
    /// No result.
    Nothing,
    /// This result, for this many levels.
    Result(Box<ItemStack>, i32),
}

/// The anvil's answer for `left` and `right` (and the name typed): the disc with the book's or the
/// other disc's enchantments, a repair when it is a disc of the same tier, and the rename.
pub fn combine(left: &ItemStack, right: Option<&ItemStack>, rename: Option<&str>) -> Outcome {
    let left_tier = item::tier_of(left);
    let right_tier = right.and_then(item::tier_of);
    let Some(left_tier) = left_tier else {
        // A disc as the second item of anything else is not a material.
        return if right_tier.is_some() {
            Outcome::Nothing
        } else {
            Outcome::Vanilla
        };
    };
    let Some(right) = right.filter(|r| r.count > 0) else {
        return Outcome::Vanilla;
    };
    if &*right.item == left_tier.material() {
        return Outcome::Vanilla;
    }

    let (incoming, combine_durability): (Vec<(Arc<str>, u32)>, bool) =
        if &*right.item == "minecraft:enchanted_book" {
            (right.stored_enchantments().to_vec(), false)
        } else if right_tier == Some(left_tier) {
            (right.enchantments().to_vec(), true)
        } else {
            return Outcome::Nothing;
        };

    let mut result = left.clone();
    let mut current: Levels = left.enchantments().to_vec();
    let mut cost = 0;
    let mut changed = false;
    for (id, add) in &incoming {
        let Some(max) = item::allowed_level(id) else {
            continue;
        };
        let have = current.iter().find(|(e, _)| e == id).map_or(0, |(_, l)| *l);
        // Vanilla rule: two equal levels combine to the next level, otherwise the higher one wins.
        let level = if have == *add {
            max.min(have + 1)
        } else {
            have.max(max.min(*add))
        };
        if level != have {
            set_level(&mut current, id, level);
            changed = true;
        }
        cost += level as i32 * if &**id == "minecraft:mending" { 4 } else { 2 };
    }
    result.set_crafting_enchantments(current);

    if combine_durability
        && let Some(max) = item::max_damage(left)
        && left.damage > 0
    {
        // Vanilla repair: both discs' remaining durability plus a 12% bonus.
        let remaining = i64::from(max) - i64::from(left.damage) + i64::from(max)
            - i64::from(right.damage)
            + i64::from(max) * 12 / 100;
        result.damage = (i64::from(max) - remaining).max(0) as u32;
        cost += 2;
        changed = true;
    }

    if let Some(name) = rename.filter(|n| !n.trim().is_empty()) {
        result
            .data_mut()
            .set_other("minecraft:custom_name", Some(Tag::String(name.into())));
        cost += 1;
        changed = true;
    }

    if !changed {
        return Outcome::Nothing;
    }
    let prior = left.repair_cost() as i32;
    result.data_mut().repair_cost = (prior * 2 + 1) as u32;
    Outcome::Result(Box::new(result), (cost + prior).clamp(1, MOST_LEVELS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Tier;
    use crate::item::tests::disc;

    fn book(enchantments: &[(&str, u32)]) -> ItemStack {
        let mut book = ItemStack::new("minecraft:enchanted_book", 1);
        book.set_crafting_enchantments(
            enchantments
                .iter()
                .map(|(id, l)| (Arc::from(*id), *l))
                .collect(),
        );
        book
    }

    fn result(outcome: Outcome) -> (ItemStack, i32) {
        match outcome {
            Outcome::Result(stack, cost) => (*stack, cost),
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn a_book_puts_its_allowed_enchantments_on_the_disc_and_leaves_the_rest() {
        let left = disc(Tier::Iron);
        let book = book(&[
            ("minecraft:sharpness", 3),
            ("minecraft:fire_aspect", 2),
            ("minecraft:mending", 1),
        ]);
        let (stack, cost) = result(combine(&left, Some(&book), None));
        assert_eq!(stack.enchantment_level("minecraft:sharpness"), 3);
        assert_eq!(stack.enchantment_level("minecraft:mending"), 1);
        assert_eq!(
            stack.enchantment_level("minecraft:fire_aspect"),
            0,
            "not for a disc"
        );
        // 3 levels of sharpness at 2 each, and mending at 4.
        assert_eq!(cost, 3 * 2 + 4);
        assert_eq!(stack.repair_cost(), 1, "the next anvil use costs more");
    }

    #[test]
    fn two_equal_levels_make_the_next_and_the_highest_level_is_capped() {
        let mut left = disc(Tier::Iron);
        left.set_crafting_enchantments(vec![(Arc::from("minecraft:sharpness"), 4)]);
        let (stack, _) = result(combine(
            &left,
            Some(&book(&[("minecraft:sharpness", 4)])),
            None,
        ));
        assert_eq!(stack.enchantment_level("minecraft:sharpness"), 5);
        // Sharpness stops at V: another V changes nothing, so there is no result.
        assert_eq!(
            combine(&stack, Some(&book(&[("minecraft:sharpness", 5)])), None),
            Outcome::Nothing
        );
        // A lower level does not lower the disc's.
        let outcome = combine(&stack, Some(&book(&[("minecraft:sharpness", 2)])), None);
        assert_eq!(
            outcome,
            Outcome::Nothing,
            "nothing changes, so there is no result"
        );
    }

    #[test]
    fn a_second_disc_of_the_same_tier_repairs_and_brings_its_enchantments() {
        let mut left = disc(Tier::Iron);
        left.damage = 200;
        let mut right = disc(Tier::Iron);
        right.damage = 100;
        right.set_crafting_enchantments(vec![(Arc::from("minecraft:knockback"), 2)]);
        let (stack, cost) = result(combine(&left, Some(&right), None));
        // 50 left and 150 left, and 12% of 250: 230 of 250 remain.
        assert_eq!(stack.damage, 20);
        assert_eq!(stack.enchantment_level("minecraft:knockback"), 2);
        assert_eq!(cost, 2 * 2 + 2);
        // A different tier is no material.
        assert_eq!(
            combine(&left, Some(&disc(Tier::Diamond)), None),
            Outcome::Nothing
        );
    }

    #[test]
    fn a_disc_is_not_a_material_for_anything_else_and_vanilla_keeps_the_rest() {
        let iron = disc(Tier::Iron);
        let sword = ItemStack::new("minecraft:iron_sword", 1);
        assert_eq!(combine(&sword, Some(&iron), None), Outcome::Nothing);
        assert_eq!(
            combine(&sword, Some(&book(&[("minecraft:sharpness", 1)])), None),
            Outcome::Vanilla
        );
        // The disc's own ingot, and a rename alone, are vanilla's.
        let ingot = ItemStack::new("minecraft:iron_ingot", 1);
        assert_eq!(combine(&iron, Some(&ingot), None), Outcome::Vanilla);
        assert_eq!(combine(&iron, None, Some("Frisbee")), Outcome::Vanilla);
    }

    #[test]
    fn renaming_with_a_book_is_part_of_the_result() {
        let (stack, cost) = result(combine(
            &disc(Tier::Iron),
            Some(&book(&[("minecraft:unbreaking", 1)])),
            Some("Frisbee"),
        ));
        assert_eq!(stack.custom_name().as_deref(), Some("Frisbee"));
        assert_eq!(cost, 2 + 1);
    }

    #[test]
    fn the_anvil_never_charges_more_than_it_allows() {
        let mut left = disc(Tier::Iron);
        left.data_mut().repair_cost = 63;
        let (_, cost) = result(combine(
            &left,
            Some(&book(&[("minecraft:sharpness", 5)])),
            None,
        ));
        assert_eq!(cost, 39);
    }
}
