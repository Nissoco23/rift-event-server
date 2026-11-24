//! Debris thrown out of the crater.
//!
//! The plugin throws real falling blocks and cancels the event that would let them land as
//! blocks. A native mod cannot stop a falling block from placing itself, and has no way to take
//! one out of the world, so the debris here are `block_display` entities that the mod flies with
//! a falling block's own physics (gravity 0.04, then the air's drag of 0.98) and removes where
//! they would land. A client draws a block display exactly as it draws the block; nothing is
//! placed, nothing hurts, and nothing is left if a debris is lost (it is tagged, and swept up when
//! the world opens).

use crate::tasks::Cx;
use horizon_client::mod_api::{Compound, Host, Tag};

/// The tag every debris carries, so a sweep can tell them from anyone else's displays.
pub const TAG: &str = "sculkstorm_debris";

/// How long a debris lives if it never lands, in ticks (the plugin's four seconds).
const LIFETIME: u32 = 80;
/// `FallingBlockEntity`'s gravity, and the drag every entity's motion takes each tick.
const GRAVITY: f64 = 0.04;
const DRAG: f64 = 0.98;

/// A block in flight.
#[derive(Clone, Debug)]
pub struct Debris {
    pub dimension: String,
    entity: u32,
    /// The bottom centre of the block, as a falling block's position is.
    pos: [f64; 3],
    vel: [f64; 3],
    age: u32,
}

/// One tick of a falling block's flight: gravity, the move, then the drag.
pub fn fly(pos: [f64; 3], vel: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let mut v = vel;
    v[1] -= GRAVITY;
    let p = [pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]];
    (p, v.map(|c| c * DRAG))
}

/// A block state as `/setblock` writes it, in the two parts a `block_state` compound has:
/// `minecraft:oak_log[axis=y]` is `minecraft:oak_log` with `axis` = `y`.
pub fn split_state(state: &str) -> (&str, Vec<(&str, &str)>) {
    let Some((name, rest)) = state.split_once('[') else {
        return (state, Vec::new());
    };
    let properties = rest
        .trim_end_matches(']')
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .collect();
    (name, properties)
}

/// The compound a `block_display` is summoned from for the block `state`.
pub fn display_nbt(state: &str) -> Compound {
    let (name, properties) = split_state(state);
    let mut block = vec![("Name", Tag::String(name.into()))];
    if !properties.is_empty() {
        block.push((
            "Properties",
            Tag::compound(
                properties
                    .into_iter()
                    .map(|(k, v)| (k, Tag::String(v.into())))
                    .collect::<Vec<_>>(),
            ),
        ));
    }
    let tag = Tag::compound([
        ("block_state", Tag::compound(block)),
        // Moved every tick, so each move eases over one: smooth on a client, and on time.
        ("teleport_duration", Tag::Int(1)),
        ("Tags", Tag::List(vec![Tag::String(TAG.into())])),
    ]);
    match tag {
        Tag::Compound(compound) => compound,
        _ => unreachable!("Tag::compound makes a compound"),
    }
}

/// Whether a block at `pos` (a block's `corner`, with its collision `boxes` in block-local
/// coordinates) holds the point `at`.
pub fn inside(boxes: &[[f64; 6]], corner: [i32; 3], at: [f64; 3]) -> bool {
    let local = [
        at[0] - f64::from(corner[0]),
        at[1] - f64::from(corner[1]),
        at[2] - f64::from(corner[2]),
    ];
    boxes
        .iter()
        .any(|b| (0..3).all(|axis| local[axis] >= b[axis] && local[axis] <= b[axis + 3]))
}

/// Whether the flight from `from` to `to` meets a block, looked at every half block along it.
fn lands(host: &dyn Host, from: [f64; 3], to: [f64; 3]) -> bool {
    let length = (0..3)
        .map(|i| (to[i] - from[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let steps = (length / 0.5).ceil().max(1.0) as u32;
    (1..=steps).any(|s| {
        let t = f64::from(s) / f64::from(steps);
        let at = [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
            from[2] + (to[2] - from[2]) * t,
        ];
        let corner = [
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        ];
        inside(&host.block_collision(corner), corner, at)
    })
}

impl Debris {
    /// Throws the block `state` from `pos` with `vel`; None where nothing can be summoned (a chunk
    /// that is not loaded).
    pub fn launch(
        host: &mut dyn Host,
        dimension: &str,
        state: &str,
        pos: [f64; 3],
        vel: [f64; 3],
    ) -> Option<Self> {
        // A display's origin is its block's corner, a falling block's the bottom centre.
        let origin = [pos[0] - 0.5, pos[1], pos[2] - 0.5];
        let entity = host.summon(
            dimension,
            "minecraft:block_display",
            origin,
            display_nbt(state),
        )?;
        Some(Self {
            dimension: dimension.to_owned(),
            entity,
            pos,
            vel,
            age: 0,
        })
    }

    /// Flies it one tick; false once it has landed or run out of time and is gone.
    pub fn step(&mut self, cx: &mut Cx<'_>) -> bool {
        if self.age >= LIFETIME {
            cx.host.remove_entity(&self.dimension, self.entity);
            return false;
        }
        let (next, vel) = fly(self.pos, self.vel);
        let landed = vel[1] < 0.0 && lands(&*cx.host, self.pos, next);
        if landed {
            cx.host.remove_entity(&self.dimension, self.entity);
            return false;
        }
        self.pos = next;
        self.vel = vel;
        self.age += 1;
        cx.host.move_entity(
            &self.dimension,
            self.entity,
            [next[0] - 0.5, next[1], next[2] - 0.5],
            0.0,
            0.0,
        );
        true
    }

    /// Takes it out of the world now.
    pub fn remove(&self, host: &mut dyn Host) {
        host.remove_entity(&self.dimension, self.entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_falls_as_a_falling_block_does() {
        // Worked by hand: gravity takes 0.04 off the speed, the move follows, then the drag.
        // tick 1: v = -0.04, y = -0.04, then v = -0.0392
        // tick 2: v = -0.0792, y = -0.1192, then v = -0.077616
        // tick 3: v = -0.117616, y = -0.236816
        let (p1, v1) = fly([0.0; 3], [0.0; 3]);
        assert!((p1[1] + 0.04).abs() < 1e-12);
        assert!((v1[1] + 0.0392).abs() < 1e-12);
        let (p2, v2) = fly(p1, v1);
        assert!((p2[1] + 0.1192).abs() < 1e-12);
        assert!((v2[1] + 0.077616).abs() < 1e-12);
        let (p3, _) = fly(p2, v2);
        assert!((p3[1] + 0.236816).abs() < 1e-12);
    }

    #[test]
    fn a_block_thrown_sideways_loses_a_fiftieth_of_its_speed_a_tick() {
        let (p, v) = fly([5.0, 70.0, 5.0], [1.0, 0.5, -1.0]);
        assert!((p[0] - 6.0).abs() < 1e-12 && (p[2] - 4.0).abs() < 1e-12);
        assert!((v[0] - 0.98).abs() < 1e-12 && (v[2] + 0.98).abs() < 1e-12);
        assert!((p[1] - 70.46).abs() < 1e-12);
        assert!((v[1] - 0.46 * 0.98).abs() < 1e-12);
    }

    #[test]
    fn a_state_splits_into_its_name_and_properties() {
        assert_eq!(split_state("minecraft:stone"), ("minecraft:stone", vec![]));
        assert_eq!(
            split_state("minecraft:oak_log[axis=y]"),
            ("minecraft:oak_log", vec![("axis", "y")])
        );
        assert_eq!(
            split_state(
                "minecraft:oak_stairs[facing=east,half=bottom,shape=straight,waterlogged=false]"
            ),
            (
                "minecraft:oak_stairs",
                vec![
                    ("facing", "east"),
                    ("half", "bottom"),
                    ("shape", "straight"),
                    ("waterlogged", "false")
                ]
            )
        );
    }

    #[test]
    fn the_display_is_the_block_with_its_properties_tagged_and_eased_over_a_tick() {
        let tag = Tag::Compound(display_nbt("minecraft:oak_log[axis=x]"));
        let block = tag.get("block_state").unwrap();
        assert_eq!(
            block.get("Name"),
            Some(&Tag::String("minecraft:oak_log".into()))
        );
        assert_eq!(
            block.get("Properties").and_then(|p| p.get("axis")),
            Some(&Tag::String("x".into()))
        );
        assert_eq!(tag.get("teleport_duration"), Some(&Tag::Int(1)));
        assert_eq!(
            tag.get("Tags"),
            Some(&Tag::List(vec![Tag::String(TAG.into())]))
        );
        // A block with no properties has no Properties compound.
        let plain = Tag::Compound(display_nbt("minecraft:sculk"));
        assert!(
            plain
                .get("block_state")
                .unwrap()
                .get("Properties")
                .is_none()
        );
    }

    #[test]
    fn a_point_is_inside_a_block_only_where_the_blocks_boxes_are() {
        let full = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        let slab = [[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
        assert!(inside(&full, [10, 64, 10], [10.5, 64.9, 10.5]));
        assert!(!inside(&full, [10, 64, 10], [10.5, 65.1, 10.5]));
        assert!(inside(&slab, [10, 64, 10], [10.5, 64.4, 10.5]));
        assert!(!inside(&slab, [10, 64, 10], [10.5, 64.6, 10.5]));
        assert!(!inside(&[], [10, 64, 10], [10.5, 64.4, 10.5]));
    }
}
