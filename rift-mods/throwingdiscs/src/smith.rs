//! The netherite upgrade: a smithing table turns a diamond disc into a netherite one that keeps its
//! enchantments, name and wear. The recipe's base slot takes any echo shard, so what comes out is
//! checked here.

use crate::item::{self, Tier};
use horizon_client::mod_api::ItemStack;

/// What the table makes of `template`, `base` and `addition`, given the netherite disc the recipe
/// would produce (`fresh`): the upgraded disc for a diamond disc with the right template and ingot,
/// nothing otherwise.
pub fn upgrade(
    template: Option<&ItemStack>,
    base: Option<&ItemStack>,
    addition: Option<&ItemStack>,
    fresh: ItemStack,
) -> Option<ItemStack> {
    let diamond = base.filter(|b| item::tier_of(b) == Some(Tier::Diamond))?;
    let template = template?;
    let addition = addition?;
    if &*template.item != "minecraft:netherite_upgrade_smithing_template"
        || &*addition.item != "minecraft:netherite_ingot"
    {
        return None;
    }
    Some(carry_over(diamond, fresh))
}

/// `fresh` with the diamond disc's enchantments, name and repair cost, and the same share of its
/// durability used, scaled to the higher maximum.
fn carry_over(diamond: &ItemStack, mut netherite: ItemStack) -> ItemStack {
    netherite.set_crafting_enchantments(diamond.enchantments().to_vec());
    if let (Some(old_max), Some(new_max)) =
        (item::max_damage(diamond), item::max_damage(&netherite))
        && old_max > 0
    {
        let share = f64::from(diamond.damage) / f64::from(old_max);
        netherite.damage = (share * f64::from(new_max)).round() as u32;
    }
    if let Some(name) = diamond
        .data()
        .and_then(|d| d.other("minecraft:custom_name"))
        .cloned()
    {
        netherite
            .data_mut()
            .set_other("minecraft:custom_name", Some(name));
    }
    netherite.data_mut().repair_cost = diamond.repair_cost();
    netherite
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::tests::disc;
    use horizon_client::mod_api::Tag;
    use std::sync::Arc;

    fn template() -> ItemStack {
        ItemStack::new("minecraft:netherite_upgrade_smithing_template", 1)
    }

    fn ingot() -> ItemStack {
        ItemStack::new("minecraft:netherite_ingot", 1)
    }

    #[test]
    fn a_diamond_disc_keeps_its_enchantments_name_and_the_share_of_wear() {
        let mut diamond = disc(Tier::Diamond);
        diamond.damage = 780; // about half of 1561
        diamond.set_crafting_enchantments(vec![
            (Arc::from("minecraft:sharpness"), 4),
            (Arc::from("minecraft:mending"), 1),
        ]);
        diamond
            .data_mut()
            .set_other("minecraft:custom_name", Some(Tag::String("Frisbee".into())));
        diamond.data_mut().repair_cost = 7;
        let made = upgrade(
            Some(&template()),
            Some(&diamond),
            Some(&ingot()),
            disc(Tier::Netherite),
        )
        .expect("a diamond disc upgrades");
        assert_eq!(item::tier_of(&made), Some(Tier::Netherite));
        assert_eq!(made.enchantment_level("minecraft:sharpness"), 4);
        assert_eq!(made.enchantment_level("minecraft:mending"), 1);
        assert_eq!(made.custom_name().as_deref(), Some("Frisbee"));
        assert_eq!(made.repair_cost(), 7);
        // 780 / 1561 of 2031, rounded.
        assert_eq!(made.damage, 1015);
    }

    #[test]
    fn nothing_but_a_real_diamond_disc_with_the_right_template_and_ingot_upgrades() {
        let diamond = disc(Tier::Diamond);
        let tried: [(Option<ItemStack>, Option<ItemStack>, Option<ItemStack>); 7] = [
            // A plain echo shard, another tier, and a netherite disc already.
            (
                Some(template()),
                Some(ItemStack::new("minecraft:echo_shard", 1)),
                Some(ingot()),
            ),
            (Some(template()), Some(disc(Tier::Iron)), Some(ingot())),
            (Some(template()), Some(disc(Tier::Netherite)), Some(ingot())),
            // A missing or wrong template or ingot.
            (None, Some(diamond.clone()), Some(ingot())),
            (Some(template()), Some(diamond.clone()), None),
            (
                Some(template()),
                Some(diamond.clone()),
                Some(ItemStack::new("minecraft:diamond", 1)),
            ),
            (
                Some(ItemStack::new("minecraft:iron_ingot", 1)),
                Some(diamond),
                Some(ingot()),
            ),
        ];
        for (template, base, addition) in tried {
            assert_eq!(
                upgrade(
                    template.as_ref(),
                    base.as_ref(),
                    addition.as_ref(),
                    disc(Tier::Netherite)
                ),
                None
            );
        }
    }
}
