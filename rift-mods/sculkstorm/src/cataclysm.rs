//! Sculk Cataclysm, right-click: ported from the plugin's `CataclysmAbility`.
//!
//! The scythe fires a sonic boom. Where it lands there is a blast, lightning and a crater (the
//! crater is in `crater/`). The boom is only particles and a ray cast each tick, never an entity.

use crate::config::Config;
use crate::crater;
use crate::fx::{self, Caster, SCULK_CYAN};
use crate::tasks::{Cx, Task};
use horizon_client::mod_api::{Host, NativeHeightmap, NativeServerPlayer, ParticleType};
use std::f64::consts::PI;
use std::time::Instant;

/// How long the field a cataclysm leaves behind lasts, in ticks.
const FIELD_TICKS: i32 = 120;
/// Damage high enough that Resistance IV still dies of it.
const LETHAL_DAMAGE: f64 = 10_000.0;

fn block(coordinate: f64) -> i32 {
    coordinate.floor() as i32
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// The direction a player looks in, as Minecraft's `calculateViewVector` gives it: yaw 0 faces
/// south (+z), 90 west (-x); pitch -90 looks straight up.
pub fn view_vector(yaw_degrees: f32, pitch_degrees: f32) -> [f64; 3] {
    let yaw = f64::from(yaw_degrees).to_radians();
    let pitch = f64::from(pitch_degrees).to_radians();
    [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ]
}

/// The damage the blast does beyond the lethal radius: `damage` at the edge of it, falling to
/// half as much at `radius`.
pub fn falloff_damage(damage: f64, distance: f64, radius: f64) -> f64 {
    damage * (1.0 - 0.5 * distance / radius)
}

/// The velocity the blast throws something with that is `from_center` away along the ground:
/// 0.9 up, and 0.6 straight away (something almost on the impact is not given a direction).
pub fn launch_away(from_center: [f64; 2]) -> [f64; 3] {
    let mut away = from_center;
    let length_squared = away[0] * away[0] + away[1] * away[1];
    if length_squared > 0.01 {
        let scale = 0.6 / length_squared.sqrt();
        away = [away[0] * scale, away[1] * scale];
    }
    [away[0], 0.9, away[1]]
}

/// Where a ray from `from` along `dir` first meets a block within `range`, with blocks that have
/// nothing to collide with (grass, flowers) ignored, as the plugin's `rayTraceBlocks` does.
fn first_block(host: &dyn Host, from: [f64; 3], dir: [f64; 3], range: f64) -> Option<[f64; 3]> {
    let length = dir.iter().map(|c| c * c).sum::<f64>().sqrt();
    if length == 0.0 {
        return None;
    }
    let unit = dir.map(|c| c / length);
    let mut start = from;
    let mut left = range;
    for _ in 0..8 {
        let hit = host.pick_block(start, unit, left, false)?;
        let at = [hit.x, hit.y, hit.z];
        if !host
            .block_collision([hit.block_x, hit.block_y, hit.block_z])
            .is_empty()
        {
            return Some(at);
        }
        // Something to walk through: go on from just past it.
        left -= distance(start, at) + 0.001;
        if left <= 0.0 {
            return None;
        }
        start = [
            at[0] + unit[0] * 0.001,
            at[1] + unit[1] * 0.001,
            at[2] + unit[2] * 0.001,
        ];
    }
    None
}

/// The sonic boom in flight.
pub struct Flight {
    pub dimension: String,
    caster: Option<Caster>,
    pos: [f64; 3],
    dir: [f64; 3],
    speed: f64,
    max_range: f64,
    travelled: f64,
    tick: i32,
}

impl Flight {
    /// Fires the boom from `p`'s eyes along where they look.
    pub fn cast(host: &mut dyn Host, cfg: &Config, p: &NativeServerPlayer) -> Self {
        let dir = view_vector(p.yaw, p.pitch);
        let eye = [
            p.position[0],
            p.position[1] + f64::from(p.eye_height),
            p.position[2],
        ];
        let start = [
            eye[0] + dir[0] * 1.2,
            eye[1] + dir[1] * 1.2,
            eye[2] + dir[2] * 1.2,
        ];
        fx::sound(
            host,
            start,
            "minecraft:entity.warden.sonic_charge",
            1.6,
            1.2,
        );
        fx::sound(host, start, "minecraft:entity.warden.sonic_boom", 2.5, 0.9);
        Self {
            dimension: p.dimension.clone(),
            caster: Some(Caster::of(p)),
            pos: start,
            dir,
            speed: cfg.cataclysm.projectile_speed,
            max_range: cfg.cataclysm.max_range,
            travelled: 0.0,
            tick: 0,
        }
    }

    pub fn step(&mut self, cx: &mut Cx<'_>) -> bool {
        let chunk = [block(self.pos[0]) >> 4, block(self.pos[2]) >> 4];
        if !cx.host.chunk_is_loaded(&self.dimension, chunk) {
            cx.shared.state().cataclysm_ended();
            return false;
        }
        if let Some(hit) = first_block(&*cx.host, self.pos, self.dir, self.speed) {
            impact(cx, &self.dimension, self.caster.as_ref(), hit);
            return false;
        }
        let near = fx::targets(
            cx.host,
            cx.cfg.pvp,
            &self.dimension,
            self.caster.as_ref(),
            self.pos.map(|c| c - 0.9),
            self.pos.map(|c| c + 0.9),
        );
        if !near.is_empty() || self.travelled >= self.max_range {
            impact(cx, &self.dimension, self.caster.as_ref(), self.pos);
            return false;
        }
        boom_visuals(cx, self.pos, self.tick);
        if self.tick % 6 == 0 {
            fx::sound(
                cx.host,
                self.pos,
                "minecraft:entity.warden.sonic_boom",
                0.9,
                1.4,
            );
        }
        for i in 0..3 {
            self.pos[i] += self.dir[i] * self.speed;
        }
        self.travelled += self.speed;
        self.tick += 1;
        true
    }
}

/// The warden's boom particle with a cyan swirl round it: about eleven particles a tick.
fn boom_visuals(cx: &mut Cx<'_>, pos: [f64; 3], tick: i32) {
    let fx = cx.fx;
    fx.particle(cx.host, ParticleType::SonicBoom, pos, 1, 0.0, 0.0);
    for i in 0..6 {
        let a = f64::from(tick) * 0.9 + f64::from(i) * (PI / 3.0);
        let r = 0.45 + 0.1 * (f64::from(tick) * 0.5 + f64::from(i)).sin();
        let at = [
            pos[0] + a.cos() * r,
            pos[1] + (a * 0.5).sin() * 0.25,
            pos[2] + a.sin() * r,
        ];
        fx.dust(cx.host, SCULK_CYAN, 1.3, at, 1, 0.0, 0.0);
    }
    fx.particle(cx.host, ParticleType::SculkSoul, pos, 1, 0.1, 0.01);
    fx.particle(cx.host, ParticleType::SculkChargePop, pos, 1, 0.2, 0.01);
    fx.particle(cx.host, ParticleType::SquidInk, pos, 1, 0.15, 0.01);
}

/// `/sculkstorm blast`: an impact with no caster, to try the scythe out without throwing it.
pub struct Blast {
    pub dimension: String,
    at: [f64; 3],
}

impl Blast {
    pub fn new(dimension: &str, at: [f64; 3]) -> Self {
        Self {
            dimension: dimension.to_owned(),
            at,
        }
    }

    pub fn step(&mut self, cx: &mut Cx<'_>) -> bool {
        impact(cx, &self.dimension, None, self.at);
        false
    }
}

/// Where the boom lands: the field it leaves, the blast, the lightning and the crater. `caster`
/// is None for a blast from the command.
pub fn impact(cx: &mut Cx<'_>, dimension: &str, caster: Option<&Caster>, at: [f64; 3]) {
    let started = Instant::now();
    let c = cx.cfg.cataclysm.clone();
    let fx = cx.fx;
    let radius = c.impact_radius;
    // The damage is credited to the caster only while they are still on.
    let credit = caster.filter(|k| fx::player(&*cx.host, k.uuid).is_some());
    let attacker = credit.and_then(|k| (k.entity_id > 0).then_some(k.entity_id));

    // The field first: it is what frees the cataclysm's slot, so a failure further down still
    // leaves it freed.
    cx.spawn.push(Task::Field(Field {
        dimension: dimension.to_owned(),
        caster: caster.cloned(),
        center: at,
        radius: radius * 1.3,
        tick: 0,
    }));

    fx::sound(cx.host, at, "minecraft:entity.warden.sonic_boom", 3.0, 0.6);
    fx::sound(cx.host, at, "minecraft:entity.generic.explode", 2.5, 0.6);
    fx::sound(
        cx.host,
        at,
        "minecraft:block.sculk_shrieker.shriek",
        2.5,
        0.8,
    );
    let above = [at[0], at[1] + 1.0, at[2]];
    fx.particle(cx.host, ParticleType::SonicBoom, above, 1, 0.0, 0.0);
    fx.particle(cx.host, ParticleType::ExplosionEmitter, at, 1, 0.0, 0.0);
    fx.particle(cx.host, ParticleType::SculkSoul, above, 30, 2.5, 0.08);

    let reach = radius + 2.0;
    let targets = fx::targets(
        cx.host,
        cx.cfg.pvp,
        dimension,
        caster,
        at.map(|c| c - reach),
        at.map(|c| c + reach),
    );
    for target in targets {
        let dist = distance(target.pos, at);
        if dist > radius {
            continue;
        }
        if c.lethal_radius > 0.0 && dist <= c.lethal_radius {
            // The sonic boom ignores armour and protection, so gear does not matter; a totem
            // still works.
            fx::hurt(
                cx.host,
                dimension,
                &target,
                LETHAL_DAMAGE,
                "minecraft:sonic_boom",
                attacker,
            );
        } else {
            let amount = falloff_damage(c.damage, dist, radius);
            match attacker {
                Some(_) => fx::hurt(
                    cx.host,
                    dimension,
                    &target,
                    amount,
                    "minecraft:player_attack",
                    attacker,
                ),
                None => fx::hurt(
                    cx.host,
                    dimension,
                    &target,
                    amount,
                    "minecraft:generic",
                    None,
                ),
            };
        }
        fx::effect(
            cx.host,
            &target,
            "minecraft:darkness",
            c.darkness_seconds * 20,
            0,
            true,
        );
        fx::effect(cx.host, &target, "minecraft:slowness", 60, 1, true);
        let from_center = [target.pos[0] - at[0], target.pos[2] - at[2]];
        fx::set_velocity(cx.host, &target, launch_away(from_center));
    }

    let spread = cx.cfg.terrain.core_radius;
    lightning_storm(
        cx,
        dimension,
        at,
        c.lightning_strikes,
        c.lightning_duration_ticks,
        spread,
    );

    let world = cx.host.world_of_dimension(dimension);
    if cx.cfg.terrain.enabled && !cx.cfg.is_disabled(dimension, world.as_deref()) {
        crater::start(cx, dimension, at);
    }

    if cx.cfg.debug {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        cx.host
            .log("info", &format!("Sculkstorm: impact tick took {ms:.1} ms."));
    }
}

/// Effect-only lightning: three on the impact at once, the rest spread over the duration.
fn lightning_storm(
    cx: &mut Cx<'_>,
    dimension: &str,
    center: [f64; 3],
    strikes: i32,
    duration_ticks: i32,
    spread: i32,
) {
    if strikes <= 0 {
        return;
    }
    let opening = strikes.min(3);
    for _ in 0..opening {
        bolt(cx, dimension, center);
    }
    let rest = strikes - opening;
    if rest > 0 {
        cx.spawn.push(Task::Lightning(Lightning {
            dimension: dimension.to_owned(),
            center,
            rest,
            duration: duration_ticks.max(1),
            spread,
            fired: 0,
            tick: 0,
            delay: 2,
        }));
    }
}

/// One bolt at the ground over `at`, with sparks: no fire, no damage.
fn bolt(cx: &mut Cx<'_>, dimension: &str, at: [f64; 3]) {
    let (x, z) = (block(at[0]), block(at[2]));
    if !cx.host.chunk_is_loaded(dimension, [x >> 4, z >> 4]) {
        return;
    }
    let top = cx
        .host
        .surface_heights(dimension, &[(x, z)], NativeHeightmap::MotionBlocking)
        .ok()
        .and_then(|heights| heights.first().copied().flatten());
    let Some(top) = top else {
        return;
    };
    let ground = [at[0], f64::from(top), at[2]];
    cx.host.lightning_bolt(dimension, ground, true);
    let fx = cx.fx;
    fx.particle(
        cx.host,
        ParticleType::ElectricSpark,
        [ground[0], ground[1] + 1.0, ground[2]],
        8,
        0.6,
        0.3,
    );
}

/// The lightning of a storm after its first three bolts.
pub struct Lightning {
    pub dimension: String,
    center: [f64; 3],
    rest: i32,
    duration: i32,
    spread: i32,
    fired: i32,
    tick: i32,
    /// Ticks to wait before the first (the plugin starts its timer two ticks late).
    delay: i32,
}

/// How many of `rest` bolts are due by the end of tick `tick` of `duration`: the storm keeps to
/// its schedule and catches up if a tick was late.
pub fn bolts_due(rest: i32, tick: i32, duration: i32) -> i32 {
    let due = (f64::from(rest) * f64::from(tick + 1) / f64::from(duration) + 0.5).floor() as i32;
    due.min(rest)
}

impl Lightning {
    pub fn step(&mut self, cx: &mut Cx<'_>) -> bool {
        if self.delay > 0 {
            self.delay -= 1;
            return true;
        }
        let due = bolts_due(self.rest, self.tick, self.duration);
        while self.fired < due {
            let a = cx.rng.between(0.0, PI * 2.0);
            let r = cx.rng.unit().sqrt() * f64::from(self.spread.max(3));
            let at = [
                self.center[0] + a.cos() * r,
                self.center[1],
                self.center[2] + a.sin() * r,
            ];
            bolt(cx, &self.dimension, at);
            self.fired += 1;
        }
        if self.fired >= self.rest {
            return false;
        }
        self.tick += 1;
        self.tick <= self.duration
    }
}

/// Particles hanging over the crater for a while, and darkness on anyone inside once a second.
pub struct Field {
    pub dimension: String,
    caster: Option<Caster>,
    center: [f64; 3],
    radius: f64,
    tick: i32,
}

impl Field {
    pub fn step(&mut self, cx: &mut Cx<'_>) -> bool {
        if self.tick >= FIELD_TICKS {
            cx.shared.state().cataclysm_ended();
            return false;
        }
        let fx = cx.fx;
        if self.tick % 4 == 0 {
            for i in 0..10 {
                let a = cx.rng.between(0.0, PI * 2.0);
                let r = cx.rng.unit().sqrt() * self.radius;
                let height = cx.rng.between(-0.5, 2.5);
                let at = [
                    self.center[0] + a.cos() * r,
                    self.center[1] + height,
                    self.center[2] + a.sin() * r,
                ];
                let kind = if i % 3 == 0 {
                    ParticleType::SculkSoul
                } else {
                    ParticleType::SculkChargePop
                };
                fx.particle(cx.host, kind, at, 1, 0.2, 0.02);
            }
        }
        if self.tick % 20 == 0 {
            let r = self.radius;
            let inside = fx::targets(
                cx.host,
                cx.cfg.pvp,
                &self.dimension,
                self.caster.as_ref(),
                [self.center[0] - r, self.center[1] - 6.0, self.center[2] - r],
                [self.center[0] + r, self.center[1] + 6.0, self.center[2] + r],
            );
            for target in inside {
                fx::effect(cx.host, &target, "minecraft:darkness", 50, 0, false);
                fx::effect(cx.host, &target, "minecraft:slowness", 30, 0, false);
            }
        }
        self.tick += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn the_view_vector_points_where_minecraft_says() {
        assert!(near(view_vector(0.0, 0.0), [0.0, 0.0, 1.0]), "south");
        assert!(near(view_vector(90.0, 0.0), [-1.0, 0.0, 0.0]), "west");
        assert!(near(view_vector(180.0, 0.0), [0.0, 0.0, -1.0]), "north");
        assert!(near(view_vector(-90.0, 0.0), [1.0, 0.0, 0.0]), "east");
        assert!(near(view_vector(0.0, -90.0), [0.0, 1.0, 0.0]), "up");
        assert!(near(view_vector(0.0, 90.0), [0.0, -1.0, 0.0]), "down");
        let slanted = view_vector(37.0, -23.0);
        let length = slanted.iter().map(|c| c * c).sum::<f64>().sqrt();
        assert!((length - 1.0).abs() < 1e-12, "a unit vector");
    }

    #[test]
    fn the_blast_hurts_less_with_distance_to_half_at_the_edge() {
        assert!((falloff_damage(14.0, 0.0, 33.0) - 14.0).abs() < 1e-12);
        assert!((falloff_damage(14.0, 33.0, 33.0) - 7.0).abs() < 1e-12);
        assert!((falloff_damage(14.0, 16.5, 33.0) - 10.5).abs() < 1e-12);
    }

    #[test]
    fn the_blast_throws_things_up_and_away() {
        assert!(near(launch_away([3.0, 4.0]), [0.36, 0.9, 0.48]));
        assert!(near(launch_away([-10.0, 0.0]), [-0.6, 0.9, 0.0]));
        // Almost on the impact: not given a direction, as the plugin leaves it.
        assert!(near(launch_away([0.05, 0.05]), [0.05, 0.9, 0.05]));
        assert!(near(launch_away([0.0, 0.0]), [0.0, 0.9, 0.0]));
    }

    #[test]
    fn the_lightning_keeps_to_its_schedule_and_never_fires_more_than_it_has() {
        // 42 bolts over 100 ticks: 0.42 a tick, rounded as Math.round does.
        assert_eq!(bolts_due(42, 0, 100), 0);
        assert_eq!(bolts_due(42, 1, 100), 1);
        assert_eq!(bolts_due(42, 49, 100), 21);
        assert_eq!(bolts_due(42, 99, 100), 42);
        assert_eq!(bolts_due(42, 500, 100), 42);
        let fired: Vec<i32> = (0..100).map(|t| bolts_due(42, t, 100)).collect();
        assert!(fired.windows(2).all(|w| w[0] <= w[1]), "never goes back");
        assert!(
            fired.windows(2).all(|w| w[1] - w[0] <= 1),
            "at most one a tick here"
        );
        assert_eq!(bolts_due(0, 3, 100), 0);
    }

    #[test]
    fn the_distance_between_two_points_is_the_straight_line() {
        assert!((distance([0.0, 0.0, 0.0], [3.0, 4.0, 12.0]) - 13.0).abs() < 1e-12);
        assert_eq!(distance([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]), 0.0);
        assert_eq!(block(-0.5), -1);
        assert_eq!(block(3.99), 3);
    }
}
