//! One disc in flight, and what happens to the item it came from.
//!
//! On the way out it flies in a straight line with no gravity, passes through entities, and bounces
//! off hard blocks. Once its range runs out it curves back to the thrower, passing through blocks like
//! a Loyalty trident and hitting entities again; the thrower catches it into the hand it came from. It
//! is drawn with an `item_display` entity, moved and turned once a tick.

use crate::config::Config;
use crate::item::{self, Tier};
use crate::vec3::*;
use horizon_client::mod_api::{
    Compound, EntityRef, Hand, Host, ItemStack, NativeHit, NativeInventoryKind, NativeServerPlayer,
    ParticleOptions, ParticleType, Tag, parse_snbt,
};
use horizon_client::save::item_to_nbt;
use std::collections::{HashMap, HashSet};

/// Extra distance around an entity's hitbox that still counts as a hit.
const HIT_RADIUS: f64 = 0.3;
/// Entities are looked for in the swept box widened by this much, since the engine lists an entity
/// by where it stands and not by its box.
const SEARCH_MARGIN: f64 = 3.0;
/// The tag every flight's display carries, so strays can be found and removed.
pub const FLIGHT_TAG: &str = "throwingdiscs_flight";
/// `SoundSource.PLAYERS` and `BLOCKS` as `Host::play_sound` numbers them.
const PLAYERS: usize = 7;
const BLOCKS: usize = 4;
/// The off hand's slot in a player's inventory (`NativeInventoryKind::Player`).
const OFF_HAND_SLOT: usize = 40;

/// What a disc's tick says happened.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Flying,
    /// The thrower caught it: it goes back into their hand.
    Caught,
    /// Its flight ends where the thrower can have it again: back into their inventory.
    GiveBack,
    /// The thrower is gone or dead: it drops where it is.
    DropHere,
}

pub struct Disc {
    pub owner: u128,
    pub owner_entity: u32,
    pub item: ItemStack,
    hand: Hand,
    /// The hotbar slot a main-hand disc was thrown from.
    slot: usize,
    pub dimension: String,
    display: Option<u32>,
    damage: f64,
    knockback: f64,
    position: V3,
    velocity: V3,
    range_left: f64,
    returning: bool,
    return_speed: f64,
    bounces: i32,
    age: i64,
    spin: f64,
    hit_out: HashSet<u32>,
    hit_return: HashSet<u32>,
}

/// What a tick needs of the world around the disc.
pub struct Around<'a> {
    pub cfg: &'a Config,
    /// The thrower, if they are on the server.
    pub owner: Option<&'a NativeServerPlayer>,
    /// Every player, by entity id, for who may be hit.
    pub players: &'a HashMap<u32, NativeServerPlayer>,
}

impl Disc {
    /// Throws `item` (a disc of `tier`, taken from `hand`) from `owner`, with the display drawn at its
    /// start.
    pub fn launch(
        host: &mut dyn Host,
        cfg: &Config,
        owner: &NativeServerPlayer,
        item: ItemStack,
        tier: Tier,
        hand: Hand,
    ) -> Disc {
        let stats = cfg.stats(tier);
        // Vanilla Sharpness bonus: +1 at level I and +0.5 per level after that, so +3 at level V.
        let sharpness = f64::from(item::level(&item, "minecraft:sharpness"));
        let damage = stats.damage
            + if sharpness > 0.0 {
                0.5 * sharpness + 0.5
            } else {
                0.0
            };
        let knockback = cfg.flight.knockback
            + f64::from(item::level(&item, "minecraft:knockback")) * cfg.flight.knockback_per_level;

        let eye = [
            owner.position[0],
            owner.position[1] + f64::from(owner.eye_height),
            owner.position[2],
        ];
        let dir = normalize(view(owner.yaw, owner.pitch));
        // Start a little in front of the eye, shifted toward the hand that threw it.
        let mut right = cross(dir, [0.0, 1.0, 0.0]);
        if dot(right, right) < 1.0e-6 {
            right = [1.0, 0.0, 0.0];
        }
        let right = normalize(right);
        let side = if hand == Hand::Main { 0.25 } else { -0.25 };
        let start = add(
            add(add(eye, scale(dir, 0.4)), scale(right, side)),
            [0.0, -0.15, 0.0],
        );
        // Against a wall the offset start can be inside the block: start at the eye then.
        let to_start = sub(start, eye);
        let blocked = host
            .pick_block(eye, to_start, len(to_start), false)
            .is_some();
        let position = if blocked { eye } else { start };

        let display = spawn_display(host, &owner.dimension, position, tier, &item);
        let at = owner.position;
        host.play_sound("minecraft:item.trident.throw", at, PLAYERS, 1.0, 1.35);
        host.play_sound(
            "minecraft:entity.player.attack.sweep",
            at,
            PLAYERS,
            0.5,
            1.7,
        );

        Disc {
            owner: owner.uuid,
            owner_entity: u32::try_from(owner.entity_id).unwrap_or(0),
            item,
            hand,
            slot: usize::from(owner.selected_slot),
            dimension: owner.dimension.clone(),
            display,
            damage,
            knockback,
            position,
            velocity: scale(dir, stats.speed),
            range_left: stats.range,
            returning: false,
            return_speed: 0.0,
            bounces: 0,
            age: 0,
            spin: 0.0,
            hit_out: HashSet::new(),
            hit_return: HashSet::new(),
        }
    }

    /// Moves the disc one tick.
    pub fn tick(&mut self, host: &mut dyn Host, around: &Around<'_>) -> Step {
        let cfg = around.cfg;
        self.age += 1;
        let Some(owner) = around.owner.filter(|o| o.health > 0.0) else {
            return Step::DropHere;
        };
        let chunk = [
            (self.position[0].floor() as i32) >> 4,
            (self.position[2].floor() as i32) >> 4,
        ];
        if owner.dimension != self.dimension
            || self.age > cfg.flight.max_lifetime_ticks
            || !host.chunk_is_loaded(&self.dimension, chunk)
        {
            return Step::GiveBack;
        }

        if !self.returning {
            self.tick_outward(host, around);
        } else if self.tick_return(host, around, owner) {
            return Step::Caught;
        }

        self.spin = (self.spin + cfg.flight.spin_degrees_per_tick) % 360.0;
        if let Some(id) = self.display {
            host.move_entity(&self.dimension, id, self.position, self.spin as f32, 0.0);
        }
        if self.age % 5 == 0 {
            host.play_sound(
                "minecraft:entity.player.attack.sweep",
                self.position,
                PLAYERS,
                0.15,
                2.0,
            );
        }
        Step::Flying
    }

    fn tick_outward(&mut self, host: &mut dyn Host, around: &Around<'_>) {
        let cfg = around.cfg;
        let step = len(self.velocity);
        let dir = normalize(self.velocity);
        match host.pick_block(self.position, dir, step, false) {
            None => {
                let next = add(self.position, self.velocity);
                self.hit_entities(host, around, self.position, next, false);
                self.position = next;
                self.range_left -= step;
            }
            Some(hit) => {
                let hit_pos = [hit.x, hit.y, hit.z];
                let travelled = dist(hit_pos, self.position);
                self.hit_entities(host, around, self.position, hit_pos, false);
                self.range_left -= travelled;
                let face = hit.face.map(f64::from);
                if hit.block == "minecraft:decorated_pot" && cfg.break_decorated_pots {
                    shatter_pot(host, &self.dimension, &hit);
                    self.position = add(self.position, self.velocity);
                    self.range_left -= step - travelled;
                } else if is_soft(host, cfg, &hit.block)
                    || self.bounces >= cfg.ricochet.max_bounces
                    // A hit with no face (over far terrain) has nothing to bounce off.
                    || face == [0.0; 3]
                {
                    self.position = sub(hit_pos, scale(dir, 0.15));
                    host.play_sound(
                        "minecraft:item.trident.hit_ground",
                        hit_pos,
                        PLAYERS,
                        0.8,
                        0.9,
                    );
                    self.start_return();
                    return;
                } else {
                    self.ricochet(host, cfg, hit_pos, face);
                }
            }
        }
        if self.range_left <= 0.0 {
            self.start_return();
        }
    }

    fn ricochet(&mut self, host: &mut dyn Host, cfg: &Config, hit_pos: V3, normal: V3) {
        // Reflect the velocity off the block face: v' = v - 2(v.n)n
        let d = dot(self.velocity, normal);
        self.velocity = sub(self.velocity, scale(normal, 2.0 * d));
        let speed =
            (len(self.velocity) * cfg.ricochet.speed_multiplier).min(cfg.ricochet.max_speed);
        self.velocity = scale(normalize(self.velocity), speed);
        self.position = add(hit_pos, scale(normal, 0.15));
        self.range_left += cfg.ricochet.extra_range;
        self.bounces += 1;
        // After a bounce the disc can hit the same mobs again.
        self.hit_out.clear();
        host.play_sound(
            "minecraft:item.trident.hit_ground",
            hit_pos,
            PLAYERS,
            1.0,
            1.5,
        );
        crit(host, hit_pos, 8, 0.05, 0.25);
    }

    fn start_return(&mut self) {
        if self.returning {
            return;
        }
        self.returning = true;
    }

    /// Steers the disc toward the owner's chest. Returns true once the owner has caught it.
    fn tick_return(
        &mut self,
        host: &mut dyn Host,
        around: &Around<'_>,
        owner: &NativeServerPlayer,
    ) -> bool {
        let cfg = around.cfg;
        if self.return_speed == 0.0 {
            self.return_speed = cfg.flight.return_speed.max(len(self.velocity) * 0.6);
        }
        let target = add(owner.position, [0.0, 1.1, 0.0]);
        let to_owner = sub(target, self.position);
        let distance = len(to_owner);
        if distance <= cfg.flight.catch_distance {
            return true;
        }
        self.return_speed =
            (self.return_speed + cfg.flight.return_acceleration).min(cfg.flight.max_return_speed);
        let desired = scale(normalize(to_owner), self.return_speed);
        // Blend the old velocity with the new one so the disc turns in an arc instead of reversing on
        // the spot.
        self.velocity = add(scale(self.velocity, 0.55), scale(desired, 0.45));
        if len(self.velocity) > distance {
            self.velocity = scale(normalize(self.velocity), distance);
        }
        let next = add(self.position, self.velocity);
        self.hit_entities(host, around, self.position, next, true);
        self.position = next;
        dist(target, self.position) <= cfg.flight.catch_distance
    }

    /// Hits every living entity whose hitbox the segment from `from` to `to` passes through.
    fn hit_entities(
        &mut self,
        host: &mut dyn Host,
        around: &Around<'_>,
        from: V3,
        to: V3,
        on_return: bool,
    ) {
        let delta = sub(to, from);
        let length = len(delta);
        if length < 1.0e-4 {
            return;
        }
        let dir = scale(delta, 1.0 / length);
        let margin = 1.0 + SEARCH_MARGIN;
        let min = [
            from[0].min(to[0]) - margin,
            from[1].min(to[1]) - margin,
            from[2].min(to[2]) - margin,
        ];
        let max = [
            from[0].max(to[0]) + margin,
            from[1].max(to[1]) + margin,
            from[2].max(to[2]) + margin,
        ];
        for id in host.entities_in(&self.dimension, min, max) {
            if id == self.owner_entity {
                continue;
            }
            let already = if on_return {
                &self.hit_return
            } else {
                &self.hit_out
            };
            if already.contains(&id) {
                continue;
            }
            let Some(entity) = host.entity(id) else {
                continue;
            };
            if host.living(entity).is_none() {
                continue;
            }
            if let Some(player) = around.players.get(&id) {
                // 3 is spectator.
                if player.game_mode == 3 || !around.cfg.pvp || player.health <= 0.0 {
                    continue;
                }
            } else if host.entity_kind(entity).as_deref() == Some("minecraft:armor_stand") {
                continue;
            }
            let Some([lo, hi]) = host.entity_bounding_box(entity) else {
                continue;
            };
            let (lo, hi) = (sub(lo, [HIT_RADIUS; 3]), add(hi, [HIT_RADIUS; 3]));
            if !inside(from, lo, hi) && ray_box(from, dir, length, lo, hi).is_none() {
                continue;
            }
            if on_return {
                self.hit_return.insert(id);
            } else {
                self.hit_out.insert(id);
            }
            self.strike(host, around, id, entity, dir);
        }
    }

    fn strike(
        &mut self,
        host: &mut dyn Host,
        around: &Around<'_>,
        id: u32,
        entity: EntityRef,
        dir: V3,
    ) {
        let before = host.entity_velocity(entity).unwrap_or([0.0; 3]);
        let hurt = host.hurt_entity(
            &self.dimension,
            id,
            self.damage as f32,
            "minecraft:thrown",
            Some(self.owner_entity),
        );
        let Some(at) = host.entity_position(entity) else {
            return;
        };
        let height = host
            .entity_bounding_box(entity)
            .map_or(1.0, |[lo, hi]| hi[1] - lo[1]);
        let at = add(at, [0.0, height * 0.6, 0.0]);
        if !hurt {
            return;
        }
        // `hurt_entity` applies vanilla knockback; the configured one replaces it.
        let player = around.players.get(&id).map(|p| p.uuid);
        let resistance = knockback_resistance(host, entity, player);
        let resist = 1.0 - resistance.clamp(0.0, 1.0);
        let mut push = [dir[0], 0.0, dir[2]];
        if dot(push, push) > 1.0e-6 {
            push = scale(normalize(push), self.knockback * resist);
        }
        push[1] = (0.2 + self.knockback * 0.3).min(0.4) * resist;
        let velocity = add(before, push);
        match player {
            Some(uuid) => {
                host.set_player_velocity(uuid, velocity);
            }
            None => {
                host.entity_set_velocity(entity, velocity);
            }
        }
        host.play_sound("minecraft:item.trident.hit", at, PLAYERS, 1.0, 1.3);
        crit(host, at, 10, 0.2, 0.3);
    }

    /// The item the disc is, with the damage the throw cost it, for when it is given back.
    pub fn item(&self) -> &ItemStack {
        &self.item
    }

    /// Takes the display away.
    pub fn remove_display(&mut self, host: &mut dyn Host) {
        if let Some(id) = self.display.take() {
            host.remove_entity(&self.dimension, id);
        }
    }

    /// Puts the disc back in the slot it was thrown from. If that slot is taken, any free slot, then
    /// the ground at the owner's feet.
    pub fn give_back(&mut self, host: &mut dyn Host, caught: bool) {
        self.remove_display(host);
        let uuid = self.owner;
        let slot = match self.hand {
            Hand::Off => OFF_HAND_SLOT,
            Hand::Main => self.slot,
        };
        let placed = host
            .set_inventory_slot(
                uuid,
                NativeInventoryKind::Player,
                slot,
                None,
                Some(self.item.clone()),
            )
            .is_ok();
        if !placed && let Some(leftover) = host.give_stack(uuid, self.item.clone()) {
            let at = host
                .server_players()
                .into_iter()
                .find(|p| p.uuid == uuid)
                .map_or(self.position, |p| p.position);
            drop_item(host, &self.dimension, at, &leftover);
        }
        if caught {
            host.play_sound_to(
                uuid,
                "minecraft:item.trident.return",
                "player",
                None,
                1.0,
                1.25,
            );
        }
    }

    /// Puts the disc into the saved inventory of an owner who is no longer on: its slot if it is free,
    /// else the first free one; with none, it drops where it is.
    pub fn give_back_offline(&mut self, host: &mut dyn Host) {
        self.remove_display(host);
        let slot = match self.hand {
            Hand::Off => OFF_HAND_SLOT,
            Hand::Main => self.slot,
        };
        let free = std::iter::once(slot).chain(0..36);
        for slot in free {
            if host
                .set_inventory_slot(
                    self.owner,
                    NativeInventoryKind::Player,
                    slot,
                    None,
                    Some(self.item.clone()),
                )
                .is_ok()
            {
                return;
            }
        }
        drop_item(host, &self.dimension, self.position, &self.item);
    }

    /// Drops the disc where it is. Used when the owner has logged off or died.
    pub fn drop_here(&mut self, host: &mut dyn Host) {
        self.remove_display(host);
        drop_item(host, &self.dimension, self.position, &self.item);
    }
}

/// Spawns the display of a flight: the disc's model lying flat, a little smaller than an item.
fn spawn_display(
    host: &mut dyn Host,
    dimension: &str,
    at: V3,
    tier: Tier,
    item: &ItemStack,
) -> Option<u32> {
    let glint = if item.has_any_enchantments() {
        ",\"minecraft:enchantment_glint_override\":1b"
    } else {
        ""
    };
    let snbt = format!(
        "{{item:{{id:\"{base}\",count:1,components:{{\"minecraft:item_model\":\"{ns}:{model}\"{glint}}}}},\
         item_display:\"fixed\",billboard:\"fixed\",teleport_duration:1,interpolation_duration:1,\
         Tags:[\"{FLIGHT_TAG}\"],\
         transformation:{{translation:[0f,0f,0f],left_rotation:[0.70710678f,0f,0f,0.70710678f],\
         scale:[0.75f,0.75f,0.75f],right_rotation:[0f,0f,0f,1f]}}}}",
        base = item::BASE,
        ns = item::NAMESPACE,
        model = tier.item_id(),
    );
    let Ok((Tag::Compound(nbt), _)) = parse_snbt(&snbt) else {
        return None;
    };
    host.summon(dimension, "minecraft:item_display", at, nbt)
}

/// Whether the disc turns back at `block` instead of bouncing: a block id or a tag of the config's
/// soft blocks.
fn is_soft(host: &dyn Host, cfg: &Config, block: &str) -> bool {
    cfg.soft_blocks
        .iter()
        .any(|entry| match entry.strip_prefix('#') {
            Some(tag) => host.block_name_in_tag(block, tag).unwrap_or(false),
            None => entry == block,
        })
}

/// Breaks the pot the way a vanilla projectile does, so it drops its sherds instead of the whole pot.
fn shatter_pot(host: &mut dyn Host, dimension: &str, hit: &NativeHit) {
    let pos = [hit.block_x, hit.block_y, hit.block_z];
    host.set_block_property(pos, "cracked", "true");
    host.play_sound(
        "minecraft:block.decorated_pot.shatter",
        [
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 0.5,
            f64::from(pos[2]) + 0.5,
        ],
        BLOCKS,
        1.0,
        1.0,
    );
    host.run_command(&format!(
        "execute in {dimension} run setblock {} {} {} minecraft:air destroy",
        pos[0], pos[1], pos[2]
    ));
}

fn crit(host: &mut dyn Host, at: V3, count: i32, spread: f64, speed: f64) {
    host.send_particles(
        &ParticleOptions::simple(ParticleType::Crit),
        None,
        at,
        [spread; 3],
        speed,
        count,
    );
}

/// How much of a knockback the entity shrugs off, 0 to 1.
fn knockback_resistance(host: &mut dyn Host, entity: EntityRef, player: Option<u128>) -> f64 {
    if let Some(uuid) = player {
        return host
            .attribute(uuid, "minecraft:knockback_resistance")
            .map_or(0.0, |(_, value)| value);
    }
    let Some(nbt) = host.entity_nbt(entity) else {
        return 0.0;
    };
    let nbt = Tag::Compound(nbt);
    let Some(Tag::List(attributes)) = nbt.get("attributes") else {
        return 0.0;
    };
    attributes
        .iter()
        .find(|a| {
            matches!(a.get("id"), Some(Tag::String(id))
                if id.to_string_lossy() == "minecraft:knockback_resistance")
        })
        .and_then(|a| number(a.get("base")))
        .unwrap_or(0.0)
}

fn number(tag: Option<&Tag>) -> Option<f64> {
    Some(match tag? {
        Tag::Float(bits) => f64::from(f32::from_bits(*bits)),
        Tag::Double(bits) => f64::from_bits(*bits),
        Tag::Int(v) => f64::from(*v),
        Tag::Short(v) => f64::from(*v),
        Tag::Byte(v) => f64::from(*v),
        Tag::Long(v) => *v as f64,
        _ => return None,
    })
}

/// An item entity of `stack` at `at`.
pub fn drop_item(host: &mut dyn Host, dimension: &str, at: V3, stack: &ItemStack) {
    let mut nbt = Compound::new();
    nbt.insert("Item".into(), item_to_nbt(stack));
    host.summon(dimension, "minecraft:item", at, nbt);
}
