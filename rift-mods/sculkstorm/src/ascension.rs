//! Soulstorm Ascension, sneak + right-click: ported from the plugin's `AscensionAbility`.
//!
//! The user charges for a couple of seconds (slowed, with soul fire gathering), then a shockwave
//! knocks everyone near back and the user is launched up, protected from the fall for a while.

use crate::config::Config;
use crate::fx::{self, Caster, SCULK_CYAN, SCULK_DARK};
use crate::tasks::{Cx, Task};
use horizon_client::mod_api::{Host, NativeEffect, NativeServerPlayer, ParticleType};
use std::f64::consts::PI;
use std::time::Duration;

/// The height of a standing player, which the plugin's box (`getNearbyEntities`) is measured from.
const PLAYER_HEIGHT: f64 = 1.8;

/// The charge before the shockwave.
pub struct Charge {
    pub dimension: String,
    player: u128,
    tick: i32,
    total: i32,
}

impl Charge {
    /// Starts a charge for `p`: slows them for its length and plays the bloom.
    pub fn start(host: &mut dyn Host, cfg: &Config, p: &NativeServerPlayer) -> Self {
        let total = cfg.ascension.charge_ticks;
        host.add_effect(
            p.uuid,
            &NativeEffect {
                id: "minecraft:slowness".into(),
                amplifier: 2,
                duration: total,
                ambient: false,
                particles: false,
            },
        );
        fx::sound(
            host,
            p.position,
            "minecraft:block.sculk_catalyst.bloom",
            1.5,
            0.6,
        );
        Self {
            dimension: p.dimension.clone(),
            player: p.uuid,
            tick: 0,
            total,
        }
    }

    pub fn step(&mut self, cx: &mut Cx<'_>) -> bool {
        let Some(p) = fx::player(&*cx.host, self.player).filter(|p| p.health > 0.0) else {
            // Gone offline, or dead: the charge is off.
            cx.shared.state().set_charging(self.player, false);
            return false;
        };
        if p.dimension != self.dimension {
            // They went through a portal: the charge goes on in their level's ticks.
            self.dimension = p.dimension.clone();
            return true;
        }
        if self.tick >= self.total {
            cx.shared.state().set_charging(self.player, false);
            release(cx, &p);
            return false;
        }
        if self.tick % 2 == 0 {
            visuals(
                cx,
                &p,
                f64::from(self.tick) / f64::from(self.total),
                self.tick,
            );
        }
        if self.tick % 10 == 0 {
            let pitch = 0.8 + self.tick as f32 / self.total as f32;
            fx::sound(
                cx.host,
                p.position,
                "minecraft:entity.warden.heartbeat",
                1.6,
                pitch,
            );
        }
        self.tick += 1;
        true
    }
}

/// Soul fire gathering around the user, more and wider as the charge fills.
fn visuals(cx: &mut Cx<'_>, p: &NativeServerPlayer, progress: f64, tick: i32) {
    let fx = cx.fx;
    let base = p.position;
    let radius = 1.0 + progress * 6.0;
    let flames = 8 + (progress * 16.0) as i32;
    for i in 0..flames {
        let angle = f64::from(tick) * 0.35 + f64::from(i) * (PI * 2.0 / f64::from(flames));
        let r = cx.rng.between(0.6, radius);
        let height = cx.rng.between(0.0, 2.6);
        let at = [
            base[0] + angle.cos() * r,
            base[1] + height,
            base[2] + angle.sin() * r,
        ];
        fx.particle(cx.host, ParticleType::SoulFireFlame, at, 1, 0.05, 0.02);
    }
    // A few sparkles and ink on top of the flames.
    for _ in 0..3 {
        let angle = cx.rng.between(0.0, PI * 2.0);
        let r = cx.rng.between(1.0, radius + 2.0);
        let height = cx.rng.between(0.2, 3.0);
        let at = [
            base[0] + angle.cos() * r,
            base[1] + height,
            base[2] + angle.sin() * r,
        ];
        fx.particle(cx.host, ParticleType::EndRod, at, 1, 0.0, 0.0);
    }
    fx.particle(
        cx.host,
        ParticleType::SquidInk,
        [base[0], base[1] + 1.0, base[2]],
        2,
        0.6,
        0.02,
    );
    fx.particle(
        cx.host,
        ParticleType::SculkSoul,
        [base[0], base[1] + 0.5, base[2]],
        1,
        0.4,
        0.03,
    );
}

/// The shockwave: everything near is hurt, thrown back and darkened, and the user is launched.
fn release(cx: &mut Cx<'_>, p: &NativeServerPlayer) {
    let a = cx.cfg.ascension.clone();
    let fx = cx.fx;
    let center = p.position;
    let caster = Caster::of(p);
    let attacker = (caster.entity_id > 0).then_some(caster.entity_id);

    fx::sound(
        cx.host,
        center,
        "minecraft:entity.warden.sonic_boom",
        2.0,
        0.9,
    );
    fx::sound(
        cx.host,
        center,
        "minecraft:entity.breeze.wind_burst",
        1.5,
        0.7,
    );
    fx.particle(
        cx.host,
        ParticleType::SonicBoom,
        [center[0], center[1] + 1.0, center[2]],
        1,
        0.0,
        0.0,
    );

    // The plugin's box is the user's own, grown by the radius and half of it above and below; the
    // search here is a little wider, and the same two tests are made on what it finds.
    let r = a.shockwave_radius;
    let low = center[1] - r / 2.0;
    let high = center[1] + PLAYER_HEIGHT + r / 2.0;
    let found = fx::targets(
        cx.host,
        cx.cfg.pvp,
        &p.dimension,
        Some(&caster),
        [center[0] - r - 2.0, low - 4.0, center[2] - r - 2.0],
        [center[0] + r + 2.0, high + 4.0, center[2] + r + 2.0],
    );
    for target in found {
        // Its box has to reach into the user's, which is grown vertically by half the radius.
        let Some([bottom, top]) = cx
            .host
            .entity_bounding_box(target.entity)
            .map(|bb| [bb[0][1], bb[1][1]])
        else {
            continue;
        };
        if top < low || bottom > high {
            continue;
        }
        let from_center = [target.pos[0] - center[0], target.pos[2] - center[2]];
        let Some(velocity) = shock_velocity(from_center, r, a.knockback) else {
            continue;
        };
        fx::hurt(
            cx.host,
            &p.dimension,
            &target,
            a.damage,
            "minecraft:player_attack",
            attacker,
        );
        fx::set_velocity(cx.host, &target, velocity);
        fx::effect(cx.host, &target, "minecraft:darkness", 60, 0, true);
    }

    cx.host
        .set_player_velocity(p.uuid, launch_velocity(p.yaw, a.launch_power));
    let until = std::time::Instant::now()
        + Duration::from_secs(u64::try_from(a.fall_protection_seconds).unwrap_or(0));
    cx.shared
        .state()
        .protect_from_fall(p.uuid, caster.entity_id, until);

    cx.spawn.push(Task::Ring(Ring {
        dimension: p.dimension.clone(),
        player: p.uuid,
        center,
        radius: r,
        tick: 0,
    }));
}

/// What the shockwave does to the velocity of something `from_center` away along the ground (x
/// and z), as the plugin works it out: thrown `knockback` blocks a tick straight away from the
/// user and 0.6 up; something right on top of them is thrown along x. None past `radius`.
pub fn shock_velocity(from_center: [f64; 2], radius: f64, knockback: f64) -> Option<[f64; 3]> {
    let mut away = from_center;
    if away[0] * away[0] + away[1] * away[1] < 0.01 {
        away = [1.0, 0.0];
    }
    let length = (away[0] * away[0] + away[1] * away[1]).sqrt();
    if length > radius {
        return None;
    }
    let push = knockback / length;
    Some([away[0] * push, 0.6, away[1] * push])
}

/// The velocity the user is launched with: `power` up, and a small push forward (0.35 along where
/// they face) so it is not a straight vertical launch. Yaw 0 faces south (+z), 90 west (-x), as
/// Minecraft's view vector has it.
pub fn launch_velocity(yaw_degrees: f32, power: f64) -> [f64; 3] {
    let yaw = f64::from(yaw_degrees).to_radians();
    [-yaw.sin() * 0.35, power, yaw.cos() * 0.35]
}

/// The ring that spreads out from where the user stood, and the trail they leave going up.
pub struct Ring {
    pub dimension: String,
    player: u128,
    center: [f64; 3],
    radius: f64,
    tick: i32,
}

const RING_TICKS: i32 = 10;
const TRAIL_TICKS: i32 = 20;

impl Ring {
    pub fn step(&mut self, cx: &mut Cx<'_>) -> bool {
        if self.tick < RING_TICKS {
            let radius = self.radius * f64::from(self.tick + 1) / f64::from(RING_TICKS);
            ring(cx, self.center, radius);
        }
        if self.tick < TRAIL_TICKS
            && let Some(p) = fx::player(&*cx.host, self.player)
        {
            let fx = cx.fx;
            let at = [p.position[0], p.position[1] + 0.2, p.position[2]];
            fx.particle(cx.host, ParticleType::SoulFireFlame, at, 3, 0.25, 0.01);
            fx.particle(cx.host, ParticleType::SculkSoul, at, 1, 0.2, 0.01);
        }
        self.tick += 1;
        self.tick < TRAIL_TICKS
    }
}

/// A dark outer ring and a cyan inner one, at most 64 points round.
fn ring(cx: &mut Cx<'_>, center: [f64; 3], r: f64) {
    let fx = cx.fx;
    let points = (r * 7.0).clamp(12.0, 64.0) as i32;
    for i in 0..points {
        let a = f64::from(i) * (PI * 2.0 / f64::from(points));
        let (cos, sin) = (a.cos(), a.sin());
        let outer = [center[0] + cos * r, center[1] + 0.3, center[2] + sin * r];
        fx.dust(cx.host, SCULK_DARK, 2.2, outer, 1, 0.05, 0.0);
        if i % 3 == 0 {
            fx.particle(cx.host, ParticleType::SquidInk, outer, 1, 0.1, 0.01);
        }
        if i % 2 == 0 {
            let inner = [
                center[0] + cos * r * 0.7,
                center[1] + 0.2,
                center[2] + sin * r * 0.7,
            ];
            fx.dust(cx.host, SCULK_CYAN, 1.4, inner, 1, 0.05, 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn the_user_is_launched_up_and_a_little_the_way_they_face() {
        // South, west and north, with the default launch power.
        assert!(near(launch_velocity(0.0, 2.2), [0.0, 2.2, 0.35]));
        assert!(near(launch_velocity(90.0, 2.2), [-0.35, 2.2, 0.0]));
        assert!(near(launch_velocity(180.0, 2.2), [0.0, 2.2, -0.35]));
        assert!(near(launch_velocity(-90.0, 1.0), [0.35, 1.0, 0.0]));
    }

    #[test]
    fn the_shockwave_throws_things_straight_away_with_the_configured_knockback_and_a_lift() {
        // Three blocks east and four south is five away, in the direction (0.6, 0.8): a push of
        // 1.6 a tick along it.
        let v = shock_velocity([3.0, 4.0], 8.0, 1.6).unwrap();
        assert!(near(v, [0.96, 0.6, 1.28]));
        // Whatever the distance, the push is the same size.
        let far = shock_velocity([-6.0, 0.0], 8.0, 1.6).unwrap();
        assert!(near(far, [-1.6, 0.6, 0.0]));
        let horizontal = (v[0] * v[0] + v[2] * v[2]).sqrt();
        assert!((horizontal - 1.6).abs() < 1e-12);
    }

    #[test]
    fn the_shockwave_reaches_exactly_as_far_as_its_radius() {
        assert!(shock_velocity([8.0, 0.0], 8.0, 1.6).is_some());
        assert!(shock_velocity([8.01, 0.0], 8.0, 1.6).is_none());
        assert!(shock_velocity([6.0, 6.0], 8.0, 1.6).is_none());
        assert!(shock_velocity([5.0, 5.0], 8.0, 1.6).is_some());
    }

    #[test]
    fn something_on_top_of_the_user_is_thrown_along_x_not_nowhere() {
        let v = shock_velocity([0.0, 0.0], 8.0, 1.6).unwrap();
        assert!(near(v, [1.6, 0.6, 0.0]));
        let almost = shock_velocity([0.05, 0.05], 8.0, 1.6).unwrap();
        assert!(near(almost, [1.6, 0.6, 0.0]));
    }
}
