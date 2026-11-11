//! The crater's layout, ported from the plugin's `SculkSpreader.plan`: which columns the crater
//! touches and what it does to each, from a seed and the config's sizes.
//!
//! A crater is a bowl dug into the ground (the core) with sculk and catalysts on its floor and
//! some deepslate pits, a sculk rim, roots going outward, and random specks between them. Nothing
//! here touches the world: it is the plan the job in `job.rs` carries out column by column.

use crate::rng::Rng;
use std::collections::HashMap;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Core,
    Rim,
    Root,
    Speck,
}

/// What stands on a column's floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    None,
    Catalyst,
    Sculk,
    Vein,
}

/// One column of the crater. `depth`, `pit` and the floor's `extra` matter for the core, `tip`
/// for a root.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub x: i32,
    pub z: i32,
    pub kind: Kind,
    pub depth: i32,
    pub pit: bool,
    pub extra: Extra,
    pub tip: bool,
    /// How far it is from the impact, which is the order the job works in: inside out.
    pub dist: f64,
}

/// The square, in block columns, that holds the whole crater.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
}

impl Area {
    pub fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }
}

/// The sizes a plan is made from (the config's `terrain` keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub core: i32,
    pub depth: i32,
    pub roots: i32,
    pub root_len: i32,
    pub max_debris: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub columns: Vec<Column>,
    pub area: Area,
    /// The chance that a solid block the crater carves flies out as debris, so that about
    /// `max-debris` do over the whole crater.
    pub debris_chance: f64,
    pub debris: u32,
}

/// Java's `Math.round`: halves go up.
fn round(x: f64) -> i32 {
    (x + 0.5).floor() as i32
}

type Columns = HashMap<(i32, i32), Column>;

/// The plan for an impact over column `cx, cz`.
pub fn plan(rng: &mut Rng, cx: i32, cz: i32, p: &Params) -> Plan {
    let (core, crater_depth) = (p.core.max(3), p.depth.max(0));
    let mut cols = Columns::new();

    // A few sine waves on the radius, so the crater is not a perfect circle.
    let (p1, p2, p3) = (
        rng.between(0.0, 6.3),
        rng.between(0.0, 6.3),
        rng.between(0.0, 6.3),
    );
    let reach = (f64::from(core) * 1.3).ceil() as i32 + 4;
    let mut core_columns = 0usize;
    for dx in -reach..=reach {
        for dz in -reach..=reach {
            let dist = f64::from(dx * dx + dz * dz).sqrt();
            let a = f64::from(dz).atan2(f64::from(dx));
            let edge = f64::from(core)
                * (1.0
                    + 0.12 * (3.0 * a + p1).sin()
                    + 0.08 * (5.0 * a + p2).sin()
                    + 0.05 * (9.0 * a + p3).sin());
            let key = (cx + dx, cz + dz);
            if dist <= edge - rng.between(0.0, 1.5) {
                let f = dist / edge;
                // The plugin's noise digs a block here and there even at a depth of 0; asking for
                // none gets none.
                let noise = rng.between(-0.8, 0.8);
                let depth = if crater_depth == 0 {
                    0
                } else {
                    round(f64::from(crater_depth) * (1.0 - f * f) + noise)
                };
                let pit = rng.chance(14);
                let extra = match rng.below(100) {
                    0..=8 => Extra::Catalyst,
                    9..=14 => Extra::Sculk,
                    15..=24 => Extra::Vein,
                    _ => Extra::None,
                };
                cols.insert(
                    key,
                    Column {
                        x: key.0,
                        z: key.1,
                        kind: Kind::Core,
                        depth: depth.max(0),
                        pit,
                        extra,
                        tip: false,
                        dist,
                    },
                );
                core_columns += 1;
            } else if dist <= edge + 3.5 && rng.unit() < 0.85 - (dist - edge) * 0.2 {
                let extra = if rng.chance(12) {
                    Extra::Vein
                } else {
                    Extra::None
                };
                cols.entry(key).or_insert(Column {
                    x: key.0,
                    z: key.1,
                    kind: Kind::Rim,
                    depth: 0,
                    pit: false,
                    extra,
                    tip: false,
                    dist,
                });
            }
        }
    }

    // Roots.
    for t in 0..p.roots.max(0) {
        let angle = f64::from(t) * (PI * 2.0 / f64::from(p.roots)) + rng.between(-0.2, 0.2);
        let len = f64::from(p.root_len.max(0)) * rng.between(0.55, 1.15);
        let reach = f64::from(core) * 0.8;
        let root = Root {
            cx,
            cz,
            px: angle.cos() * reach,
            pz: angle.sin() * reach,
            angle,
            len,
            width: 2.6,
            depth: 0,
        };
        trace_root(&mut cols, rng, root);
    }

    // Random specks in the gaps between the roots.
    let root_len = f64::from(p.root_len.max(0));
    let outer = f64::from(core) + root_len;
    let specks = (PI * (outer * outer - f64::from(core * core)) * 0.035) as i64;
    for _ in 0..specks {
        let a = rng.between(0.0, PI * 2.0);
        // The power bunches them closer in.
        let r = f64::from(core) + rng.unit().powf(1.6) * root_len;
        let x = cx + round(a.cos() * r);
        let z = cz + round(a.sin() * r);
        let extra = if rng.below(3) == 0 {
            Extra::Vein
        } else {
            Extra::None
        };
        cols.entry((x, z)).or_insert(Column {
            x,
            z,
            kind: Kind::Speck,
            depth: 0,
            pit: false,
            extra,
            tip: false,
            dist: r,
        });
    }

    // Inside out. The tie-break only makes the order the same for the same seed.
    let mut columns: Vec<Column> = cols.into_values().collect();
    columns.sort_by(|a, b| {
        a.dist
            .total_cmp(&b.dist)
            .then(a.x.cmp(&b.x))
            .then(a.z.cmp(&b.z))
    });

    let bound = outer.ceil() as i32 + 6;
    let area = Area {
        min_x: cx - bound,
        min_z: cz - bound,
        max_x: cx + bound,
        max_z: cz + bound,
    };
    let max_debris = p.max_debris.max(0);
    let expected_carved = (core_columns as f64 * f64::from(crater_depth.max(1)) * 0.5).max(1.0);
    let debris_chance = (f64::from(max_debris) * 1.5 / expected_carved).min(1.0);
    Plan {
        columns,
        area,
        debris_chance,
        debris: max_debris as u32,
    }
}

/// A root being traced: where it is (relative to the impact), which way it points, how far it
/// goes and how wide it is.
struct Root {
    cx: i32,
    cz: i32,
    px: f64,
    pz: f64,
    angle: f64,
    len: f64,
    width: f64,
    depth: u32,
}

/// Lays a root's columns. A top-level root can fork one or two branches.
fn trace_root(cols: &mut Columns, rng: &mut Rng, mut r: Root) {
    let mut branches_left = if r.depth == 0 {
        rng.int_between(1, 3)
    } else {
        0
    };
    let mut s = 0.0;
    while s < r.len {
        r.angle += rng.between(-0.12, 0.12);
        r.px += r.angle.cos() * 0.7;
        r.pz += r.angle.sin() * 0.7;
        let progress = s / r.len;
        let width = (r.width * (1.0 - progress)).max(0.4);
        let tip = progress > 0.7;
        let mut w = -width;
        while w <= width {
            let x = r.cx + round(r.px - r.angle.sin() * w);
            let z = r.cz + round(r.pz + r.angle.cos() * w);
            let extra = if !tip && rng.chance(4) {
                Extra::Sculk
            } else {
                Extra::None
            };
            let dist = f64::from((x - r.cx) * (x - r.cx) + (z - r.cz) * (z - r.cz)).sqrt();
            cols.entry((x, z)).or_insert(Column {
                x,
                z,
                kind: Kind::Root,
                depth: 0,
                pit: false,
                extra,
                tip,
                dist,
            });
            w += 0.8;
        }
        if branches_left > 0 && progress > 0.2 && progress < 0.6 && rng.chance(4) {
            branches_left -= 1;
            let side = if rng.flip() { 1.0 } else { -1.0 };
            let branch = Root {
                cx: r.cx,
                cz: r.cz,
                px: r.px,
                pz: r.pz,
                angle: r.angle + side * rng.between(0.45, 0.8),
                len: (r.len - s) * rng.between(0.4, 0.7),
                width: width * 0.7,
                depth: r.depth + 1,
            };
            trace_root(cols, rng, branch);
        }
        s += 0.7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: Params = Params {
        core: 33,
        depth: 10,
        roots: 24,
        root_len: 50,
        max_debris: 80,
    };

    fn made(seed: u64) -> Plan {
        plan(&mut Rng::new(seed), 100, -200, &DEFAULT)
    }

    fn count(plan: &Plan, kind: Kind) -> usize {
        plan.columns.iter().filter(|c| c.kind == kind).count()
    }

    #[test]
    fn the_same_seed_makes_the_same_crater_and_another_makes_another() {
        assert_eq!(made(5), made(5));
        assert_ne!(made(5).columns, made(6).columns);
    }

    #[test]
    fn every_column_is_there_once_and_the_order_is_inside_out() {
        let p = made(1);
        let mut seen = std::collections::HashSet::new();
        for c in &p.columns {
            assert!(seen.insert((c.x, c.z)), "column {} {} twice", c.x, c.z);
        }
        assert!(p.columns.windows(2).all(|w| w[0].dist <= w[1].dist));
    }

    #[test]
    fn the_core_is_about_the_area_of_its_circle() {
        // The radius wobbles by at most a quarter, and the waves average out: a circle of 33 is
        // 3 421 columns, and the ragged edge takes a few off.
        let circle = PI * 33.0 * 33.0;
        for seed in 1..=8 {
            let cores = count(&made(seed), Kind::Core) as f64;
            assert!(
                (0.8 * circle..1.2 * circle).contains(&cores),
                "seed {seed}: {cores} core columns against a circle of {circle}"
            );
        }
    }

    #[test]
    fn the_bowl_is_deepest_in_the_middle_and_never_deeper_than_asked() {
        let p = made(2);
        let deepest = p
            .columns
            .iter()
            .filter(|c| c.kind == Kind::Core)
            .map(|c| c.depth)
            .max()
            .unwrap();
        // A column's depth is the depth asked for, less its distance from the middle, plus up to
        // 0.8 of noise, and a pit digs one or two more at carve time.
        assert!((10..=11).contains(&deepest), "{deepest}");
        let near = p
            .columns
            .iter()
            .filter(|c| c.kind == Kind::Core && c.dist < 5.0)
            .all(|c| c.depth >= 9);
        assert!(near);
        // The edge wobbles out to 41, so only past 36 is a column surely in the shallow last tenth.
        let rim_of_core = p
            .columns
            .iter()
            .filter(|c| c.kind == Kind::Core && c.dist > 36.0)
            .all(|c| c.depth <= 4);
        assert!(rim_of_core);
        assert!(p.columns.iter().all(|c| c.depth >= 0));
        assert!(
            p.columns
                .iter()
                .filter(|c| c.kind != Kind::Core)
                .all(|c| c.depth == 0)
        );
    }

    #[test]
    fn about_one_core_column_in_seven_is_a_pit_and_the_floors_extras_have_their_odds() {
        let p = made(3);
        let core: Vec<&Column> = p.columns.iter().filter(|c| c.kind == Kind::Core).collect();
        let n = core.len() as f64;
        let share = |f: fn(&Column) -> bool| core.iter().filter(|c| f(c)).count() as f64 / n;
        assert!((0.11..0.17).contains(&share(|c| c.pit)));
        assert!((0.06..0.12).contains(&share(|c| c.extra == Extra::Catalyst)));
        assert!((0.03..0.09).contains(&share(|c| c.extra == Extra::Sculk)));
        assert!((0.07..0.13).contains(&share(|c| c.extra == Extra::Vein)));
    }

    #[test]
    fn roots_reach_out_to_their_length_and_specks_lie_beyond_the_core() {
        let p = made(4);
        let farthest = p
            .columns
            .iter()
            .filter(|c| c.kind == Kind::Root)
            .map(|c| c.dist)
            .fold(0.0, f64::max);
        // Roots start 0.8 of a radius out and run up to 1.15 of the length from there.
        assert!(
            farthest > 40.0 && farthest < 33.0 * 0.8 + 50.0 * 1.15 + 3.0,
            "{farthest}"
        );
        assert!(count(&p, Kind::Root) > 500);
        assert!(
            p.columns
                .iter()
                .filter(|c| c.kind == Kind::Speck)
                .all(|c| c.dist >= 33.0 && c.dist <= 83.0)
        );
        assert!(count(&p, Kind::Speck) > 300);
        assert!(
            p.columns
                .iter()
                .filter(|c| c.kind == Kind::Root)
                .any(|c| c.tip)
        );
    }

    #[test]
    fn the_area_holds_every_column() {
        let p = made(7);
        assert!(p.columns.iter().all(|c| p.area.contains(c.x, c.z)));
        // 33 + 50 out, and 6 of margin, around the impact.
        assert_eq!(p.area.min_x, 100 - 89);
        assert_eq!(p.area.max_z, -200 + 89);
    }

    #[test]
    fn without_roots_there_are_none_and_a_flat_crater_has_no_depth() {
        let flat = Params {
            roots: 0,
            root_len: 0,
            depth: 0,
            ..DEFAULT
        };
        let p = plan(&mut Rng::new(9), 0, 0, &flat);
        assert_eq!(count(&p, Kind::Root), 0);
        assert_eq!(count(&p, Kind::Speck), 0);
        assert!(p.columns.iter().all(|c| c.depth == 0));
    }

    #[test]
    fn debris_is_chosen_so_about_the_asked_number_fly() {
        let p = made(11);
        // 1.5 times as many as asked over half the core's columns' depth, as a chance, capped at 1.
        let core = count(&p, Kind::Core) as f64;
        let expected = (80.0 * 1.5 / (core * 10.0 * 0.5)).min(1.0);
        assert!((p.debris_chance - expected).abs() < 1e-12);
        assert_eq!(p.debris, 80);
        let none = plan(
            &mut Rng::new(1),
            0,
            0,
            &Params {
                max_debris: 0,
                ..DEFAULT
            },
        );
        assert_eq!(none.debris_chance, 0.0);
        let small = plan(
            &mut Rng::new(1),
            0,
            0,
            &Params {
                core: 3,
                depth: 1,
                max_debris: 400,
                ..DEFAULT
            },
        );
        assert_eq!(small.debris_chance, 1.0);
    }
}
