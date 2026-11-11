//! Particles and sounds, and who an ability may hit: the plugin's `Fx`, over the engine's `Host`.

use horizon_client::mod_api::{
    EntityRef, Host, NativeEffect, NativeServerPlayer, ParticleOptions, ParticleType, Tag,
};
use std::collections::HashMap;

/// The scythe's cyan (`0x29DFEB`) and the dark of its shockwave (`0x0B2A33`), as the 0 to 1
/// channels a dust particle takes.
pub const SCULK_CYAN: [f32; 3] = [41.0 / 255.0, 223.0 / 255.0, 235.0 / 255.0];
pub const SCULK_DARK: [f32; 3] = [11.0 / 255.0, 42.0 / 255.0, 51.0 / 255.0];

/// `SoundSource.PLAYERS` in vanilla's order, as `Host::play_sound` numbers sources.
pub const PLAYERS: usize = 7;

/// Particles, with their count scaled by the config's `particle-multiplier`.
#[derive(Clone, Copy, Debug)]
pub struct Fx {
    multiplier: f64,
}

impl Fx {
    pub fn new(multiplier: f64) -> Self {
        Self {
            multiplier: multiplier.max(0.0),
        }
    }

    /// `count` scaled, and at least one as long as any particle is asked for and the multiplier
    /// is not 0: the plugin never rounds a visible effect away.
    pub fn scaled(&self, count: i32) -> i32 {
        let scaled = (f64::from(count) * self.multiplier + 0.5).floor() as i32;
        if scaled <= 0 && count > 0 && self.multiplier > 0.0 {
            1
        } else {
            scaled.max(0)
        }
    }

    pub fn particle(
        &self,
        host: &mut dyn Host,
        kind: ParticleType,
        at: [f64; 3],
        count: i32,
        spread: f64,
        speed: f64,
    ) {
        self.options(
            host,
            &ParticleOptions::simple(kind),
            at,
            count,
            spread,
            speed,
        );
    }

    pub fn dust(
        &self,
        host: &mut dyn Host,
        rgb: [f32; 3],
        size: f32,
        at: [f64; 3],
        count: i32,
        spread: f64,
        speed: f64,
    ) {
        self.options(
            host,
            &ParticleOptions::dust(rgb, size),
            at,
            count,
            spread,
            speed,
        );
    }

    pub fn options(
        &self,
        host: &mut dyn Host,
        options: &ParticleOptions,
        at: [f64; 3],
        count: i32,
        spread: f64,
        speed: f64,
    ) {
        let count = self.scaled(count);
        if count > 0 {
            host.send_particles(options, None, at, [spread; 3], speed, count);
        }
    }
}

/// A sound for the players around `at`, in the players category as the plugin plays it.
pub fn sound(host: &mut dyn Host, at: [f64; 3], sound: &str, volume: f32, pitch: f32) {
    host.play_sound(sound, at, PLAYERS, volume, pitch);
}

/// Who cast an ability: a player, by UUID and the entity id damage and hooks know them by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caster {
    pub uuid: u128,
    pub entity_id: u32,
}

impl Caster {
    pub fn of(player: &NativeServerPlayer) -> Self {
        Self {
            uuid: player.uuid,
            entity_id: u32::try_from(player.entity_id).unwrap_or(0),
        }
    }
}

/// A living thing an ability may hit.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub id: u32,
    pub entity: EntityRef,
    /// Where its feet are.
    pub pos: [f64; 3],
    /// The UUID when it is a player.
    pub player: Option<u128>,
}

/// The player `uuid`, if they are on the server.
pub fn player(host: &dyn Host, uuid: u128) -> Option<NativeServerPlayer> {
    host.server_players().into_iter().find(|p| p.uuid == uuid)
}

/// The plugin's `canHit`, over every entity in the box from `min` to `max` of `dimension`: a living
/// thing that is not the caster, not dead, not an armor stand, not the caster's own pet, and, for
/// a player, only with `pvp` on and not in creative or spectator; anything else only if it is not
/// invulnerable.
pub fn targets(
    host: &mut dyn Host,
    pvp: bool,
    dimension: &str,
    caster: Option<&Caster>,
    min: [f64; 3],
    max: [f64; 3],
) -> Vec<Target> {
    let ids = host.entities_in(dimension, min, max);
    if ids.is_empty() {
        return Vec::new();
    }
    let players: HashMap<u32, NativeServerPlayer> = host
        .server_players()
        .into_iter()
        .filter_map(|p| Some((u32::try_from(p.entity_id).ok().filter(|id| *id > 0)?, p)))
        .collect();
    let mut out = Vec::new();
    for id in ids {
        if caster.is_some_and(|c| c.entity_id == id) {
            continue;
        }
        let Some(entity) = host.entity(id) else {
            continue;
        };
        if host.living(entity).is_none() {
            continue;
        }
        let Some(pos) = host.entity_position(entity) else {
            continue;
        };
        if let Some(p) = players.get(&id) {
            // 1 is creative and 3 spectator.
            if !pvp || matches!(p.game_mode, 1 | 3) || p.health <= 0.0 {
                continue;
            }
            out.push(Target {
                id,
                entity,
                pos,
                player: Some(p.uuid),
            });
            continue;
        }
        let kind = host.entity_kind(entity).unwrap_or_default();
        if kind == "minecraft:armor_stand" || kind == "minecraft:player" {
            continue;
        }
        let Some(nbt) = host.entity_nbt(entity) else {
            continue;
        };
        let nbt = Tag::Compound(nbt);
        if number(nbt.get("Health")).is_some_and(|health| health <= 0.0)
            || flag(nbt.get("Invulnerable"))
        {
            continue;
        }
        if let Some(c) = caster
            && nbt.get("Owner").and_then(owner) == Some(c.uuid)
        {
            continue;
        }
        out.push(Target {
            id,
            entity,
            pos,
            player: None,
        });
    }
    out
}

/// A number tag as a number (`Health` is a float, but a hand-edited save may hold another kind).
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

fn flag(tag: Option<&Tag>) -> bool {
    number(tag).is_some_and(|v| v != 0.0)
}

/// The UUID an `Owner` tag names: four ints, most significant first.
pub fn owner(tag: &Tag) -> Option<u128> {
    let Tag::IntArray(ints) = tag else {
        return None;
    };
    let [a, b, c, d] = ints.as_slice() else {
        return None;
    };
    Some(
        (u128::from(*a as u32) << 96)
            | (u128::from(*b as u32) << 64)
            | (u128::from(*c as u32) << 32)
            | u128::from(*d as u32),
    )
}

/// Hurts `target` as `attacker` (a player's or mob's entity id) does with `damage_type`.
pub fn hurt(
    host: &mut dyn Host,
    dimension: &str,
    target: &Target,
    amount: f64,
    damage_type: &str,
    attacker: Option<u32>,
) -> bool {
    host.hurt_entity(dimension, target.id, amount as f32, damage_type, attacker)
}

/// Gives `target` the effect `id` for `ticks`. A player gets it as a potion would give it; for
/// anything else the server's `/effect give` does it, which takes whole seconds (rounded up).
pub fn effect(
    host: &mut dyn Host,
    target: &Target,
    id: &str,
    ticks: i32,
    amplifier: u8,
    particles: bool,
) {
    if ticks <= 0 {
        return;
    }
    match target.player {
        Some(uuid) => {
            host.add_effect(
                uuid,
                &NativeEffect {
                    id: id.to_owned(),
                    amplifier,
                    duration: ticks,
                    ambient: false,
                    particles,
                },
            );
        }
        None => {
            if let Some(uuid) = host.entity_uuid(target.entity) {
                let seconds = (ticks + 19) / 20;
                host.run_command(&format!(
                    "effect give {} {id} {seconds} {amplifier} {}",
                    hyphenated(uuid),
                    !particles
                ));
            }
        }
    }
}

/// Sets `target`'s velocity, in blocks a tick: a player's client is told to move, anything else
/// moves on the server.
pub fn set_velocity(host: &mut dyn Host, target: &Target, velocity: [f64; 3]) {
    match target.player {
        Some(uuid) => {
            host.set_player_velocity(uuid, velocity);
        }
        None => {
            host.entity_set_velocity(target.entity, velocity);
        }
    }
}

/// A UUID as `8-4-4-4-12` hexadecimal digits.
pub fn hyphenated(uuid: u128) -> String {
    let h = format!("{uuid:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sound_category_is_the_players() {
        assert_eq!(horizon_client::settings::SOUND_SOURCES[PLAYERS], "player");
    }

    #[test]
    fn the_scythes_two_colours_are_its_hex_values() {
        let channels = |rgb: u32| {
            [
                ((rgb >> 16) & 255) as f32 / 255.0,
                ((rgb >> 8) & 255) as f32 / 255.0,
                (rgb & 255) as f32 / 255.0,
            ]
        };
        assert_eq!(SCULK_CYAN, channels(0x29DFEB));
        assert_eq!(SCULK_DARK, channels(0x0B2A33));
    }

    #[test]
    fn a_particle_count_is_scaled_but_never_rounded_to_nothing() {
        let half = Fx::new(0.5);
        assert_eq!(half.scaled(10), 5);
        assert_eq!(half.scaled(1), 1, "half of one is still shown");
        assert_eq!(half.scaled(3), 2, "1.5 rounds up, as Math.round does");
        assert_eq!(Fx::new(0.0).scaled(10), 0);
        assert_eq!(Fx::new(2.0).scaled(6), 12);
        assert_eq!(Fx::new(1.0).scaled(0), 0);
        assert_eq!(Fx::new(-4.0).scaled(10), 0);
    }

    #[test]
    fn an_owner_tag_is_four_ints_most_significant_first() {
        let uuid = 0x1234_5678_9abc_def0_0fed_cba9_8765_4321_u128;
        let ints = vec![
            0x1234_5678_i32,
            0x9abc_def0_u32 as i32,
            0x0fed_cba9_i32,
            0x8765_4321_u32 as i32,
        ];
        assert_eq!(owner(&Tag::IntArray(ints)), Some(uuid));
        assert_eq!(owner(&Tag::IntArray(vec![1, 2, 3])), None);
        assert_eq!(owner(&Tag::Int(1)), None);
    }

    #[test]
    fn health_and_flags_read_from_whatever_number_tag_holds_them() {
        assert_eq!(number(Some(&Tag::Float(20.0f32.to_bits()))), Some(20.0));
        assert_eq!(number(Some(&Tag::Double(0.5f64.to_bits()))), Some(0.5));
        assert_eq!(number(Some(&Tag::Int(3))), Some(3.0));
        assert_eq!(number(None), None);
        assert_eq!(number(Some(&Tag::String("x".into()))), None);
        assert!(flag(Some(&Tag::Byte(1))));
        assert!(!flag(Some(&Tag::Byte(0))));
        assert!(!flag(None));
    }

    #[test]
    fn a_uuid_is_written_with_its_hyphens() {
        assert_eq!(
            hyphenated(0x1234_5678_9abc_def0_0fed_cba9_8765_4321),
            "12345678-9abc-def0-0fed-cba987654321"
        );
        assert_eq!(hyphenated(1), "00000000-0000-0000-0000-000000000001");
    }
}
