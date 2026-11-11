//! The crater's work, ported from the plugin's `SculkSpreader`: carving one column after another
//! from a plan (`SpreadJob`), and putting every block back afterwards (`RestoreJob`).
//!
//! Every column is independent: all it reads and writes is its own x and z. So a column is worked
//! out from reads of the world plus what the column itself has already decided (its `overlay`),
//! the writes of a whole tick go to the game as one bulk edit, and nothing waits on a write.

use super::debris::Debris;
use super::journal;
use super::plan::{Area, Column, Extra, Kind, Plan};
use super::terrain::{self, NameCache, NameInfo};
use crate::rng::Rng;
use crate::state::Shared;
use crate::tasks::{Cx, Task};
use horizon_client::mod_api::{
    BlockStateId, Host, LevelRef, NativeBlockEdit, NativeBlockRun, NativeHeightmap, ParticleType,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

/// Search range around the impact's height when the heightmap is no use (caves).
const SCAN_UP: i32 = 12;
const SCAN_DOWN: i32 = 18;
/// A column whose ground is further than this from the impact's height is skipped.
const HILL_UP: i32 = 24;
const HILL_DOWN: i32 = 32;
/// A blast in mid air looks this far down for ground.
const MAX_DROP: i32 = 96;
const TREE_CLEAR_HEIGHT: i32 = 16;
/// How many columns' heights are read at once.
const HEIGHTS_AT_ONCE: usize = 64;
/// How many writes go to the game in one bulk edit.
const EDIT_RUNS: usize = 4096;
/// The flags a block the mod puts back is set with: send it to players, but let nothing react.
const SET_FLAGS: i32 = 2 | 16 | 32;

/// One write for each block the runs name, the last of them, in the order the blocks were first
/// written: a block dug out and then given a catalyst is written once, with the catalyst, so the
/// edit does not depend on the game applying two runs of one block in order.
pub fn collapse(runs: Vec<NativeBlockRun>) -> Vec<NativeBlockRun> {
    let mut at: HashMap<[i32; 3], usize> = HashMap::with_capacity(runs.len());
    let mut out: Vec<NativeBlockRun> = Vec::with_capacity(runs.len());
    for run in runs {
        match at.get(&[run.x, run.y, run.z]) {
            Some(&i) => out[i].block = run.block,
            None => {
                at.insert([run.x, run.y, run.z], out.len());
                out.push(run);
            }
        }
    }
    out
}

/// A block the crater puts down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Place {
    Air = 0,
    Sculk,
    Deepslate,
    CobbledDeepslate,
    Tuff,
    Stone,
    Catalyst,
    /// A sculk vein on the floor.
    Vein,
}

impl Place {
    pub const ALL: [Place; 8] = [
        Place::Air,
        Place::Sculk,
        Place::Deepslate,
        Place::CobbledDeepslate,
        Place::Tuff,
        Place::Stone,
        Place::Catalyst,
        Place::Vein,
    ];

    /// The block, without its properties.
    pub fn name(self) -> &'static str {
        match self {
            Place::Air => "minecraft:air",
            Place::Sculk => "minecraft:sculk",
            Place::Deepslate => "minecraft:deepslate",
            Place::CobbledDeepslate => "minecraft:cobbled_deepslate",
            Place::Tuff => "minecraft:tuff",
            Place::Stone => "minecraft:stone",
            Place::Catalyst => "minecraft:sculk_catalyst",
            Place::Vein => "minecraft:sculk_vein",
        }
    }

    /// The state to write, as `/setblock` writes it.
    pub fn state(self) -> &'static str {
        match self {
            Place::Vein => "minecraft:sculk_vein[down=true]",
            other => other.name(),
        }
    }

    /// A block that stands in the way of whatever is above it: what `solid` reads for a block the
    /// column has put down but the world has not got yet.
    fn solid(self) -> bool {
        !matches!(self, Place::Air | Place::Vein)
    }
}

/// The plugin's `sculkPalette`: what ground the crater turns to.
pub fn sculk_palette(rng: &mut Rng) -> Place {
    match rng.below(100) {
        0..=81 => Place::Sculk,
        82..=91 => Place::Deepslate,
        _ => Place::CobbledDeepslate,
    }
}

/// The plugin's `grayPalette`: the floor of a pit, and some of the rim.
pub fn gray_palette(rng: &mut Rng) -> Place {
    match rng.below(100) {
        0..=44 => Place::Deepslate,
        45..=69 => Place::CobbledDeepslate,
        70..=87 => Place::Tuff,
        _ => Place::Stone,
    }
}

/// Whether the block there now is one the crater may put its original back over: still the
/// crater's own, or gone (mined, or flooded). Anything else is something a player built over it
/// and is left alone.
pub fn can_restore_over(now: &str, placed: &str) -> bool {
    now == placed
        || matches!(
            now,
            "minecraft:air"
                | "minecraft:cave_air"
                | "minecraft:void_air"
                | "minecraft:water"
                | "minecraft:lava"
        )
}

/// A packed position, for sets of the blocks the crater placed.
pub fn pack(pos: [i32; 3]) -> u64 {
    ((pos[0] as u64 & 0x03FF_FFFF) << 38)
        | ((pos[2] as u64 & 0x03FF_FFFF) << 12)
        | (pos[1] as u64 & 0xFFF)
}

/// A block the crater changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub pos: [i32; 3],
    /// The block put there (an index into the list's strings).
    pub placed: u32,
    /// The state it replaced (also an index).
    pub original: u32,
}

/// Every change a crater made, in order, with the states and blocks they name kept once.
#[derive(Clone, Debug, Default)]
pub struct Changes {
    strings: Vec<String>,
    index: HashMap<String, u32>,
    list: Vec<Change>,
}

impl Changes {
    fn intern(&mut self, text: &str) -> u32 {
        if let Some(i) = self.index.get(text) {
            return *i;
        }
        let i = self.strings.len() as u32;
        self.strings.push(text.to_owned());
        self.index.insert(text.to_owned(), i);
        i
    }

    pub fn push(&mut self, pos: [i32; 3], placed: &str, original: &str) {
        let placed = self.intern(placed);
        let original = self.intern(original);
        self.list.push(Change {
            pos,
            placed,
            original,
        });
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn placed(&self, change: &Change) -> &str {
        &self.strings[change.placed as usize]
    }

    pub fn original(&self, change: &Change) -> &str {
        &self.strings[change.original as usize]
    }

    pub fn from_records(records: &[journal::Record]) -> Self {
        let mut changes = Self::default();
        for r in records {
            changes.push(r.pos, &r.placed, &r.original);
        }
        changes
    }
}

/// A block another thing changed in a crater's area that the crater puts back with the rest:
/// sculk spreading from a catalyst.
#[derive(Clone, Debug)]
pub struct Foreign {
    pub pos: [i32; 3],
    pub old: BlockStateId,
    /// What it was changed to; it is put back only if it still is that.
    pub new_name: String,
}

/// A block thrown out of the crater, to become a debris.
struct Seed {
    pos: [f64; 3],
    state: String,
    vel: [f64; 3],
}

/// What one column decided to do.
struct Edit {
    y: i32,
    placed: Place,
    original: String,
}

enum Outcome {
    Done,
    Unloaded,
    NoGround,
}

/// One column of a crater being worked out: reads from the world and from what the column has
/// already decided, and the decisions themselves.
struct Cc<'a> {
    host: &'a mut dyn Host,
    level: LevelRef,
    dimension: &'a str,
    names: &'a mut NameCache,
    rng: &'a mut Rng,
    x: i32,
    z: i32,
    min_y: i32,
    max_y: i32,
    break_all: bool,
    /// What this column has put at each height so far; the world is not written until the tick's
    /// edit goes out, so a read of one of these heights must not ask the world.
    overlay: Vec<(i32, Place)>,
    edits: Vec<Edit>,
    seeds: &'a mut Vec<Seed>,
    debris_left: &'a mut u32,
    debris_chance: f64,
    /// The impact's column, which debris flies away from.
    center: [i32; 2],
}

impl Cc<'_> {
    fn placed_at(&self, y: i32) -> Option<Place> {
        self.overlay
            .iter()
            .rev()
            .find(|(at, _)| *at == y)
            .map(|(_, place)| *place)
    }

    /// The block at `y` (without properties); None where the world has none (not loaded, or past
    /// its height).
    fn name(&self, y: i32) -> Option<String> {
        if let Some(place) = self.placed_at(y) {
            return Some(place.name().to_owned());
        }
        if y < self.min_y || y >= self.max_y {
            return None;
        }
        self.host.level_block(self.level, [self.x, y, self.z])
    }

    /// The whole state at `y`, as `/setblock` writes it.
    fn state(&self, y: i32) -> Option<String> {
        if let Some(place) = self.placed_at(y) {
            return Some(place.state().to_owned());
        }
        if y < self.min_y || y >= self.max_y {
            return None;
        }
        self.host.level_block_state(self.level, [self.x, y, self.z])
    }

    fn info(&mut self, name: &str) -> NameInfo {
        self.names.info(&*self.host, name)
    }

    /// The plugin's `isSolid`: something at `y` that stops what is above it.
    fn solid(&self, y: i32) -> bool {
        match self.placed_at(y) {
            Some(place) => place.solid(),
            None => !self.host.block_collision([self.x, y, self.z]).is_empty(),
        }
    }

    /// The plugin's `isOccluding`: a whole block that conducts redstone.
    fn occluding(&mut self, y: i32, info: &NameInfo) -> bool {
        match self.placed_at(y) {
            Some(place) => place.solid(),
            None => {
                !info.see_through
                    && terrain::full_cube(&self.host.block_collision([self.x, y, self.z]))
            }
        }
    }

    fn air(&mut self, y: i32) -> bool {
        match self.name(y) {
            Some(name) => self.info(&name).air,
            None => false,
        }
    }

    /// Whether the block at `y` is one the crater never touches: unbreakable, or holding data.
    fn protected(&mut self, y: i32, info: &NameInfo) -> bool {
        info.unbreakable
            || (self.placed_at(y).is_none()
                && self
                    .host
                    .block_entity_nbt(self.dimension, [self.x, y, self.z])
                    .is_some())
    }

    /// The plugin's `canCarve`.
    fn can_carve(&mut self, y: i32, info: &NameInfo) -> bool {
        if info.terrain {
            return true;
        }
        if !self.break_all || info.air || info.liquid {
            return false;
        }
        !self.protected(y, info)
    }

    /// The plugin's `canConvert`: full blocks only, or fences and torches would turn to sculk.
    fn can_convert(&mut self, y: i32, info: &NameInfo) -> bool {
        if info.terrain {
            return true;
        }
        if !self.break_all || !self.solid(y) || !self.occluding(y, info) {
            return false;
        }
        !self.protected(y, info)
    }

    fn is_ground(&mut self, y: i32, info: &NameInfo) -> bool {
        info.terrain || (self.break_all && self.solid(y) && !info.log && !info.leaves)
    }

    fn is_open_above(&mut self, y: i32) -> bool {
        match self.name(y) {
            Some(name) => {
                let info = self.info(&name);
                info.air || info.plant || info.log || info.leaves
            }
            None => false,
        }
    }

    /// Puts `place` at `y`, unless it is there already.
    fn set(&mut self, y: i32, place: Place) {
        let Some(current) = self.name(y) else {
            return;
        };
        if current == place.name() {
            return;
        }
        let Some(original) = self.state(y) else {
            return;
        };
        self.edits.push(Edit {
            y,
            placed: place,
            original,
        });
        self.overlay.push((y, place));
    }

    fn convert_if_terrain(&mut self, y: i32) {
        let Some(name) = self.name(y) else {
            return;
        };
        let info = self.info(&name);
        if self.can_convert(y, &info) {
            let place = sculk_palette(self.rng);
            self.set(y, place);
        }
    }

    /// A vein on the floor at `y`, if it is open and has something to stand on.
    fn set_vein(&mut self, y: i32) {
        if self.air(y) && self.solid(y - 1) {
            self.set(y, Place::Vein);
        }
    }

    /// Clears the plants (up to two high) standing at `y`.
    fn clear_plants(&mut self, y: i32) {
        for at in [y, y + 1] {
            let Some(name) = self.name(at) else {
                return;
            };
            let info = self.info(&name);
            if info.air || !info.plant {
                return;
            }
            self.set(at, Place::Air);
        }
    }

    /// The first solid block at or below `y` that is not leaves.
    fn ground_below(&mut self, y: i32) -> i32 {
        let bottom = self.min_y.max(y - MAX_DROP);
        let mut at = y.min(self.max_y - 1);
        while at >= bottom {
            if let Some(name) = self.name(at) {
                let info = self.info(&name);
                if self.solid(at) && !info.leaves {
                    return at;
                }
            }
            at -= 1;
        }
        y
    }

    /// The plugin's `findSurface`: the height the crater works down from in this column.
    fn find_surface(&mut self, base: Option<i32>, cy: i32) -> Option<i32> {
        // `base` is one above the highest block that counts on the heightmap.
        let h = base? - 1;
        if h >= cy - HILL_DOWN && h <= cy + HILL_UP {
            // Usually the heightmap is right: walk down past the plants.
            let bottom = (self.min_y + 1).max(h - 24);
            for y in (bottom..=h).rev() {
                let name = self.name(y)?;
                let info = self.info(&name);
                if info.liquid {
                    return None;
                }
                if self.is_ground(y, &info) {
                    return (self.break_all || self.is_open_above(y + 1)).then_some(y);
                }
            }
            return None;
        }
        // Caves and overhangs: the heightmap is far off, so look round the impact's height.
        let top = (self.max_y - 2).min(cy + SCAN_UP);
        let bottom = (self.min_y + 1).max(cy - SCAN_DOWN);
        for y in (bottom..=top).rev() {
            let Some(name) = self.name(y) else {
                continue;
            };
            let info = self.info(&name);
            if self.is_ground(y, &info) && self.is_open_above(y + 1) {
                return Some(y);
            }
        }
        None
    }

    fn process(&mut self, c: &Column, base: Option<i32>, cy: i32) -> Outcome {
        let Some(surface) = self.find_surface(base, cy) else {
            return Outcome::NoGround;
        };
        match c.kind {
            Kind::Core => self.carve(c, surface, cy),
            Kind::Rim => {
                self.clear_plants(surface + 1);
                if let Some(name) = self.name(surface) {
                    let info = self.info(&name);
                    if self.can_convert(surface, &info) {
                        let place = if self.rng.chance(75) {
                            sculk_palette(self.rng)
                        } else {
                            gray_palette(self.rng)
                        };
                        self.set(surface, place);
                    }
                }
                if c.extra == Extra::Vein {
                    self.set_vein(surface + 1);
                }
            }
            Kind::Root => {
                self.clear_plants(surface + 1);
                if c.tip && self.rng.chance(45) {
                    self.set_vein(surface + 1);
                } else {
                    self.convert_if_terrain(surface);
                    if self.rng.flip() {
                        self.convert_if_terrain(surface - 1);
                    }
                    if c.extra == Extra::Sculk && self.air(surface + 1) {
                        self.set(surface + 1, Place::Sculk);
                    }
                }
            }
            Kind::Speck => {
                self.clear_plants(surface + 1);
                if c.extra == Extra::Vein {
                    self.set_vein(surface + 1);
                } else {
                    self.convert_if_terrain(surface);
                }
            }
        }
        Outcome::Done
    }

    /// The plugin's `carveCrater`: clear the trees, dig the bowl, turn its floor to sculk and
    /// stand a catalyst, sculk or a vein on it.
    fn carve(&mut self, c: &Column, surface: i32, cy: i32) {
        let tree_top = (self.max_y - 1).min(surface + TREE_CLEAR_HEIGHT);
        for y in surface + 1..=tree_top {
            let Some(name) = self.name(y) else {
                break;
            };
            let info = self.info(&name);
            if info.air {
                continue;
            }
            if info.log || info.leaves || info.plant {
                self.set(y, Place::Air);
            } else {
                break;
            }
        }

        let depth = c.depth + if c.pit { self.rng.int_between(1, 3) } else { 0 };
        // With break-all, everything is flattened down to the impact's height first, or the bowl
        // would follow the shape of every roof and hill.
        let floor = (if self.break_all {
            surface.min(cy)
        } else {
            surface
        }) - depth;
        for y in ((floor + 1)..=surface).rev() {
            let Some(name) = self.name(y) else {
                continue;
            };
            let info = self.info(&name);
            if !self.can_carve(y, &info) {
                continue;
            }
            let was_solid = self.solid(y);
            let original = self.state(y).unwrap_or_default();
            self.set(y, Place::Air);
            if was_solid {
                self.maybe_debris(y, &original, c);
            }
        }

        if c.pit {
            // Pits are gray, not sculk.
            if let Some(name) = self.name(floor) {
                let info = self.info(&name);
                if self.can_convert(floor, &info) {
                    let place = gray_palette(self.rng);
                    self.set(floor, place);
                }
            }
            return;
        }
        self.convert_if_terrain(floor);
        self.convert_if_terrain(floor - 1);

        if !self.air(floor + 1) || !self.solid(floor) {
            return;
        }
        match c.extra {
            Extra::Catalyst => self.set(floor + 1, Place::Catalyst),
            Extra::Sculk => self.set(floor + 1, Place::Sculk),
            Extra::Vein => self.set_vein(floor + 1),
            Extra::None => {}
        }
    }

    /// Throws the block that was at `y` out of the crater, if any debris are left and its turn
    /// comes: low and fast, which looks better than high lobs.
    fn maybe_debris(&mut self, y: i32, original: &str, c: &Column) {
        if *self.debris_left == 0 || self.rng.unit() >= self.debris_chance {
            return;
        }
        *self.debris_left -= 1;
        let mut out = [
            f64::from(c.x - self.center[0]),
            f64::from(c.z - self.center[1]),
        ];
        if out[0] * out[0] + out[1] * out[1] < 0.01 {
            out = [self.rng.between(-1.0, 1.0), self.rng.between(-1.0, 1.0)];
        }
        let length = (out[0] * out[0] + out[1] * out[1]).sqrt().max(1e-9);
        let speed = self.rng.between(0.6, 1.3);
        let vel = [
            out[0] / length * speed,
            self.rng.between(0.45, 0.95),
            out[1] / length * speed,
        ];
        // 40% of the flying blocks are sculk, whatever they were.
        let state = if self.rng.chance(40) {
            Place::Sculk.state().to_owned()
        } else {
            original.to_owned()
        };
        self.seeds.push(Seed {
            pos: [
                f64::from(c.x) + 0.5,
                f64::from(y) + 0.6,
                f64::from(c.z) + 0.5,
            ],
            state,
            vel,
        });
    }
}

/// A crater being dug.
pub struct SpreadJob {
    pub id: u64,
    pub dimension: String,
    center: [i32; 3],
    plan: Plan,
    index: usize,
    debris_left: u32,
    break_all: bool,
    /// What it has changed so far; empty (and not kept) when the config turns restoring off.
    changes: Changes,
    changed: usize,
    skipped_unloaded: u32,
    skipped_no_ground: u32,
    journal: Option<PathBuf>,
    pending_journal: String,
    names: NameCache,
    rng: Rng,
    restore: bool,
    started: Instant,
    slowest: std::time::Duration,
    /// The heights read ahead, for the columns from `heights_from` on.
    heights: Vec<Option<i32>>,
    heights_from: usize,
}

impl SpreadJob {
    pub fn new(
        id: u64,
        dimension: &str,
        center: [i32; 3],
        plan: Plan,
        break_all: bool,
        restore: bool,
        journal: Option<PathBuf>,
        rng: Rng,
    ) -> Self {
        let debris_left = plan.debris;
        let mut pending_journal = String::new();
        if restore && journal.is_some() {
            pending_journal.push_str(&journal::header(dimension));
        }
        Self {
            id,
            dimension: dimension.to_owned(),
            center,
            plan,
            index: 0,
            debris_left,
            break_all,
            changes: Changes::default(),
            changed: 0,
            skipped_unloaded: 0,
            skipped_no_ground: 0,
            journal,
            pending_journal,
            names: NameCache::default(),
            rng,
            restore,
            started: Instant::now(),
            slowest: std::time::Duration::ZERO,
            heights: Vec::new(),
            heights_from: 0,
        }
    }

    pub fn area(&self) -> Area {
        self.plan.area
    }

    pub fn finished(&self) -> bool {
        self.index >= self.plan.columns.len()
    }

    pub fn journal(&self) -> Option<&PathBuf> {
        self.journal.as_ref()
    }

    pub fn changes(&self) -> &Changes {
        &self.changes
    }

    /// A line for the log: what this crater did.
    pub fn summary(&self) -> String {
        format!(
            "Cataclysm at {} {} {}: {} columns planned, {} blocks changed, {} columns in unloaded chunks, \
             {} with no ground found. Slowest tick {:.1} ms, {:.1} s in all.",
            self.center[0],
            self.center[1],
            self.center[2],
            self.plan.columns.len(),
            self.changed,
            self.skipped_unloaded,
            self.skipped_no_ground,
            self.slowest.as_secs_f64() * 1000.0,
            self.started.elapsed().as_secs_f64()
        )
    }

    /// Works on columns until the budget or the deadline runs out; returns the blocks it placed
    /// (not air), for the set of blocks the crater watches.
    pub fn advance(
        &mut self,
        cx: &mut Cx<'_>,
        budget: &mut i32,
        deadline: Instant,
    ) -> Vec<[i32; 3]> {
        let started = Instant::now();
        let mut placed = Vec::new();
        let Some(level) = cx.host.level(&self.dimension) else {
            // The level is gone: there is nothing more to dig.
            self.index = self.plan.columns.len();
            return placed;
        };
        let (min_y, height) = cx.host.level_height(level).unwrap_or((-64, 384));
        let max_y = min_y + height;
        let mut runs: Vec<NativeBlockRun> = Vec::new();
        let mut seeds: Vec<Seed> = Vec::new();
        let mut visuals = 0;
        let columns = self.plan.columns.len();

        while *budget > 0 && self.index < columns && Instant::now() < deadline {
            if self.index >= self.heights_from + self.heights.len() {
                self.read_heights(cx.host);
            }
            let column = self.plan.columns[self.index].clone();
            let base = self.heights[self.index - self.heights_from];
            self.index += 1;
            *budget -= 1;

            let chunk = [column.x >> 4, column.z >> 4];
            let outcome = if !cx.host.chunk_is_loaded(&self.dimension, chunk) {
                Outcome::Unloaded
            } else {
                let mut cc = Cc {
                    host: &mut *cx.host,
                    level,
                    dimension: &self.dimension,
                    names: &mut self.names,
                    rng: &mut self.rng,
                    x: column.x,
                    z: column.z,
                    min_y,
                    max_y,
                    break_all: self.break_all,
                    overlay: Vec::new(),
                    edits: Vec::new(),
                    seeds: &mut seeds,
                    debris_left: &mut self.debris_left,
                    debris_chance: self.plan.debris_chance,
                    center: [self.center[0], self.center[2]],
                };
                let outcome = cc.process(&column, base, self.center[1]);
                let edits = std::mem::take(&mut cc.edits);
                drop(cc);
                for edit in edits {
                    let pos = [column.x, edit.y, column.z];
                    runs.push(NativeBlockRun {
                        x: pos[0],
                        z: pos[2],
                        y: pos[1],
                        height: 1,
                        surface: None,
                        block: edit.placed as u32,
                        only: None,
                    });
                    self.changed += 1;
                    if self.restore {
                        // Where another crater that stands changed the block first, the land is
                        // what that one found: the restore of either puts that back.
                        let original = cx.shared.state().craters.original_for(
                            &self.dimension,
                            pos,
                            self.id,
                            &edit.original,
                        );
                        self.changes.push(pos, edit.placed.name(), &original);
                        if self.journal.is_some() {
                            self.pending_journal.push_str(&journal::line(
                                pos,
                                edit.placed.name(),
                                &original,
                            ));
                        }
                        if edit.placed != Place::Air {
                            placed.push(pos);
                        }
                    }
                }
                outcome
            };
            match outcome {
                Outcome::Done => {
                    if visuals < 6 && cx.rng.below(5) == 0 {
                        visuals += 1;
                        let at = [
                            f64::from(column.x) + 0.5,
                            f64::from(self.center[1]) + 1.0,
                            f64::from(column.z) + 0.5,
                        ];
                        cx.fx
                            .particle(cx.host, ParticleType::SculkChargePop, at, 2, 0.4, 0.02);
                    }
                }
                Outcome::Unloaded => self.skipped_unloaded += 1,
                Outcome::NoGround => self.skipped_no_ground += 1,
            }
            if runs.len() >= EDIT_RUNS {
                self.flush(cx.host, &mut runs);
            }
        }
        self.flush(cx.host, &mut runs);
        self.heights.clear();
        self.heights_from = 0;

        for seed in seeds {
            if let Some(debris) =
                Debris::launch(cx.host, &self.dimension, &seed.state, seed.pos, seed.vel)
            {
                cx.spawn.push(Task::Debris(debris));
            }
        }
        if cx.tick.is_multiple_of(4) && self.index > 0 {
            let frontier = &self.plan.columns[self.index - 1];
            let at = [
                f64::from(frontier.x),
                f64::from(self.center[1]),
                f64::from(frontier.z),
            ];
            crate::fx::sound(cx.host, at, "minecraft:block.sculk.spread", 1.5, 0.6);
        }
        self.flush_journal();
        self.slowest = self.slowest.max(started.elapsed());
        placed
    }

    /// Hands the journal lines not yet written to the thread that writes them.
    pub fn flush_journal(&mut self) {
        if let Some(file) = &self.journal {
            let text = std::mem::take(&mut self.pending_journal);
            super::JOURNAL.append(file, text);
        }
    }

    /// Reads the heights of the next columns, for the columns the tick will reach.
    fn read_heights(&mut self, host: &mut dyn Host) {
        let from = self.index;
        let to = (from + HEIGHTS_AT_ONCE).min(self.plan.columns.len());
        let columns: Vec<(i32, i32)> = self.plan.columns[from..to]
            .iter()
            .map(|c| (c.x, c.z))
            .collect();
        let heights = host
            .surface_heights(
                &self.dimension,
                &columns,
                NativeHeightmap::MotionBlockingNoLeaves,
            )
            .unwrap_or_else(|_| vec![None; columns.len()]);
        self.heights = heights;
        self.heights_from = from;
    }

    /// Sends the writes of the tick to the game as one edit.
    fn flush(&mut self, host: &mut dyn Host, runs: &mut Vec<NativeBlockRun>) {
        if runs.is_empty() {
            return;
        }
        let edit = NativeBlockEdit {
            palette: Place::ALL.iter().map(|p| p.state().to_owned()).collect(),
            filters: Vec::new(),
            runs: collapse(std::mem::take(runs)),
            neighbour_updates: false,
            block_entities: Vec::new(),
        };
        if let Err(e) = host.edit_blocks(&self.dimension, &edit) {
            host.log(
                "warn",
                &format!(
                    "Sculkstorm: a crater edit of {} blocks was refused: {e}",
                    edit.runs.len()
                ),
            );
        }
    }

    /// Turns the job into the restore of what it has done, due at `due_tick`.
    pub fn into_restore(self, due_tick: u64) -> RestoreJob {
        let area = self.plan.area;
        RestoreJob::new(
            self.id,
            self.dimension,
            self.changes,
            area,
            due_tick,
            self.journal,
        )
    }
}

/// Where in a chunk's changes the restore is: the indices still to do, the last to restore at the end.
struct ChunkQueue {
    chunk: (i32, i32),
    todo: Vec<usize>,
}

/// A crater being put back.
pub struct RestoreJob {
    pub id: u64,
    pub dimension: String,
    changes: Changes,
    area: Area,
    pub due_tick: u64,
    journal: Option<PathBuf>,
    queues: Option<Vec<ChunkQueue>>,
    /// Chunks the job asked the server to load because they were not.
    requested: HashSet<(i32, i32)>,
    restored: usize,
}

impl RestoreJob {
    pub fn new(
        id: u64,
        dimension: String,
        changes: Changes,
        area: Area,
        due_tick: u64,
        journal: Option<PathBuf>,
    ) -> Self {
        Self {
            id,
            dimension,
            changes,
            area,
            due_tick,
            journal,
            queues: None,
            requested: HashSet::new(),
            restored: 0,
        }
    }

    pub fn area(&self) -> Area {
        self.area
    }

    pub fn journal(&self) -> Option<&PathBuf> {
        self.journal.as_ref()
    }

    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// How many changes are still to be put back.
    pub fn remaining(&self) -> usize {
        match &self.queues {
            Some(queues) => queues.iter().map(|q| q.todo.len()).sum(),
            None => self.changes.len(),
        }
    }

    /// Every change's position and the state it replaced, in the order they were made.
    pub fn originals(&self) -> impl Iterator<Item = ([i32; 3], &str)> + '_ {
        self.changes
            .list
            .iter()
            .map(|c| (c.pos, self.changes.original(c)))
    }

    /// The positions of every block it placed that is not air (what the crater watches).
    pub fn placed_positions(&self) -> Vec<[i32; 3]> {
        self.changes
            .list
            .iter()
            .filter(|c| self.changes.placed(c) != Place::Air.name())
            .map(|c| c.pos)
            .collect()
    }

    fn queues(&mut self) -> &mut Vec<ChunkQueue> {
        let changes = &self.changes;
        self.queues.get_or_insert_with(|| {
            let mut by_chunk: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
            for (i, c) in changes.list.iter().enumerate() {
                by_chunk
                    .entry((c.pos[0] >> 4, c.pos[2] >> 4))
                    .or_default()
                    .push(i);
            }
            // Each chunk's last change is restored first, so a block changed twice ends as it was.
            by_chunk
                .into_iter()
                .map(|(chunk, todo)| ChunkQueue { chunk, todo })
                .collect()
        })
    }

    /// Restores up to `budget` blocks; whether everything is back. A chunk that is not loaded is
    /// asked for and waited on, and the blocks of the others go on meanwhile. `restored` is
    /// given the positions put back.
    pub fn restore(
        &mut self,
        host: &mut dyn Host,
        shared: &Shared,
        budget: &mut i32,
        restored: &mut Vec<[i32; 3]>,
    ) -> bool {
        self.revert_foreign(host, shared, restored);
        let Some(level) = host.level(&self.dimension) else {
            return false;
        };
        let dimension = self.dimension.clone();
        let mut queues = std::mem::take(self.queues());
        for queue in &mut queues {
            if *budget <= 0 {
                break;
            }
            if queue.todo.is_empty() {
                continue;
            }
            let (cx, cz) = queue.chunk;
            if !host.chunk_is_loaded(&dimension, [cx, cz]) {
                if self.requested.insert(queue.chunk) {
                    host.chunk_load_request(OWNER, &dimension, [cx, cz]);
                }
                continue;
            }
            let take = (*budget as usize).min(queue.todo.len());
            let batch: Vec<usize> = queue
                .todo
                .split_off(queue.todo.len() - take)
                .into_iter()
                .rev()
                .collect();
            *budget -= take as i32;
            self.restore_batch(host, level, &batch, restored);
            if queue.todo.is_empty() && self.requested.remove(&queue.chunk) {
                host.chunk_load_release(OWNER, &dimension, [cx, cz]);
            }
        }
        let done = queues.iter().all(|q| q.todo.is_empty());
        self.queues = Some(queues);
        done && self.foreign_left(shared) == 0
    }

    /// Puts back the blocks in `batch` (indices of changes, last first) that may still be put back.
    fn restore_batch(
        &mut self,
        host: &mut dyn Host,
        level: LevelRef,
        batch: &[usize],
        restored: &mut Vec<[i32; 3]>,
    ) {
        let mut palette: Vec<String> = Vec::new();
        let mut at: HashMap<u32, u32> = HashMap::new();
        let mut runs: Vec<NativeBlockRun> = Vec::new();
        // A block the crater changed twice (dug out, then a catalyst stood in the hole) is in the
        // batch twice, the later change first. The world is not written until the batch is done,
        // so the earlier change has to be judged against what the later one is about to leave
        // there, not against what is there now.
        let mut becoming: HashMap<[i32; 3], (String, usize)> = HashMap::new();
        for &i in batch {
            let c = self.changes.list[i];
            let now = match becoming.get(&c.pos) {
                Some((name, _)) => name.clone(),
                None => match host.level_block(level, c.pos) {
                    Some(name) => name,
                    None => continue,
                },
            };
            if !can_restore_over(&now, self.changes.placed(&c)) {
                continue;
            }
            let block = *at.entry(c.original).or_insert_with(|| {
                palette.push(self.changes.original(&c).to_owned());
                (palette.len() - 1) as u32
            });
            let name = super::debris::split_state(self.changes.original(&c))
                .0
                .to_owned();
            match becoming.get_mut(&c.pos) {
                // One write for a block, to what it was before the earliest change.
                Some((becomes, run)) => {
                    *becomes = name;
                    runs[*run].block = block;
                }
                None => {
                    becoming.insert(c.pos, (name, runs.len()));
                    runs.push(NativeBlockRun {
                        x: c.pos[0],
                        z: c.pos[2],
                        y: c.pos[1],
                        height: 1,
                        surface: None,
                        block,
                        only: None,
                    });
                }
            }
        }
        if runs.is_empty() {
            return;
        }
        let positions: Vec<[i32; 3]> = becoming.into_keys().collect();
        let edit = NativeBlockEdit {
            palette,
            filters: Vec::new(),
            runs,
            neighbour_updates: false,
            block_entities: Vec::new(),
        };
        match host.edit_blocks(&self.dimension, &edit) {
            Ok(_) => {
                self.restored += positions.len();
                restored.extend(positions);
            }
            Err(e) => {
                host.log(
                    "warn",
                    &format!(
                        "Sculkstorm: a crater restore of {} blocks was refused: {e}",
                        edit.runs.len()
                    ),
                );
            }
        }
    }

    /// Puts back what something else changed in the crater's area (sculk spreading).
    fn revert_foreign(
        &mut self,
        host: &mut dyn Host,
        shared: &Shared,
        restored: &mut Vec<[i32; 3]>,
    ) {
        let list = shared.state().craters.take_foreign(self.id);
        if list.is_empty() {
            return;
        }
        let Some(level) = host.level(&self.dimension) else {
            shared.state().craters.add_foreign(self.id, list);
            return;
        };
        let mut left = Vec::new();
        shared.set_reverting(true);
        for f in list.into_iter().rev() {
            if !host.chunk_is_loaded(&self.dimension, [f.pos[0] >> 4, f.pos[2] >> 4]) {
                left.push(f);
                continue;
            }
            // Only if it still is what it was changed to: anything else is someone's now.
            if host.level_block(level, f.pos).as_deref() == Some(f.new_name.as_str()) {
                host.set_block(&self.dimension, f.pos, f.old, SET_FLAGS);
                restored.push(f.pos);
            }
        }
        shared.set_reverting(false);
        if !left.is_empty() {
            shared.state().craters.add_foreign(self.id, left);
        }
    }

    fn foreign_left(&self, shared: &Shared) -> usize {
        shared.state().craters.foreign_count(self.id)
    }

    /// Lets go of the chunks the job asked the server to hold.
    pub fn release_chunks(&mut self, host: &mut dyn Host) {
        for (cx, cz) in std::mem::take(&mut self.requested) {
            host.chunk_load_release(OWNER, &self.dimension, [cx, cz]);
        }
    }
}

/// The owner the crater's chunk requests are made under.
const OWNER: &str = "sculkstorm:crater";

/// Teleports anyone the restore would leave inside a block to the top of the ground, as the
/// plugin's `unstickPlayers` does.
pub fn unstick_players(host: &mut dyn Host, dimension: &str, area: Area) {
    let Some(level) = host.level(dimension) else {
        return;
    };
    let mut names = NameCache::default();
    for p in host.server_players() {
        let (x, z) = (p.position[0].floor() as i32, p.position[2].floor() as i32);
        if p.dimension != dimension || !area.contains(x, z) {
            continue;
        }
        let feet = p.position[1].floor() as i32;
        let eye = (p.position[1] + f64::from(p.eye_height)).floor() as i32;
        let mut stuck = false;
        for y in [feet, eye] {
            let pos = [x, y, z];
            let Some(name) = host.level_block(level, pos) else {
                continue;
            };
            let info = names.info(&*host, &name);
            if !info.see_through && terrain::full_cube(&host.block_collision(pos)) {
                stuck = true;
            }
        }
        if !stuck {
            continue;
        }
        let top = host
            .surface_heights(dimension, &[(x, z)], NativeHeightmap::MotionBlocking)
            .ok()
            .and_then(|h| h.first().copied().flatten());
        if let Some(top) = top {
            host.teleport_player(
                p.uuid,
                dimension,
                [f64::from(x) + 0.5, f64::from(top), f64::from(z) + 0.5],
                p.yaw,
                p.pitch,
            );
        }
    }
}

/// The ground height under an impact at `at`, as the plugin's `groundBelow` finds it, read in the
/// level that is in place. None when the level has no such block to read.
pub fn ground_below(
    host: &mut dyn Host,
    dimension: &str,
    break_all: bool,
    at: [i32; 3],
) -> Option<i32> {
    let level = host.level(dimension)?;
    let (min_y, height) = host.level_height(level)?;
    let mut names = NameCache::default();
    let mut rng = Rng::new(0);
    let mut seeds = Vec::new();
    let mut left = 0;
    let mut cc = Cc {
        host,
        level,
        dimension,
        names: &mut names,
        rng: &mut rng,
        x: at[0],
        z: at[2],
        min_y,
        max_y: min_y + height,
        break_all,
        overlay: Vec::new(),
        edits: Vec::new(),
        seeds: &mut seeds,
        debris_left: &mut left,
        debris_chance: 0.0,
        center: [at[0], at[2]],
    };
    Some(cc.ground_below(at[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palettes_have_the_plugins_odds() {
        let mut rng = Rng::new(42);
        let mut sculk = [0u32; 3];
        let mut gray = [0u32; 4];
        let n = 100_000;
        for _ in 0..n {
            match sculk_palette(&mut rng) {
                Place::Sculk => sculk[0] += 1,
                Place::Deepslate => sculk[1] += 1,
                Place::CobbledDeepslate => sculk[2] += 1,
                other => panic!("{other:?}"),
            }
            match gray_palette(&mut rng) {
                Place::Deepslate => gray[0] += 1,
                Place::CobbledDeepslate => gray[1] += 1,
                Place::Tuff => gray[2] += 1,
                Place::Stone => gray[3] += 1,
                other => panic!("{other:?}"),
            }
        }
        let share = |count: u32| f64::from(count) / f64::from(n);
        // 82 / 10 / 8 and 45 / 25 / 18 / 12 in a hundred.
        for (got, want) in [(sculk[0], 0.82), (sculk[1], 0.10), (sculk[2], 0.08)] {
            assert!((share(got) - want).abs() < 0.01, "{got} against {want}");
        }
        for (got, want) in [
            (gray[0], 0.45),
            (gray[1], 0.25),
            (gray[2], 0.18),
            (gray[3], 0.12),
        ] {
            assert!((share(got) - want).abs() < 0.01, "{got} against {want}");
        }
    }

    fn run(x: i32, y: i32, block: u32) -> NativeBlockRun {
        NativeBlockRun {
            x,
            z: 0,
            y,
            height: 1,
            surface: None,
            block,
            only: None,
        }
    }

    #[test]
    fn a_block_written_twice_in_an_edit_is_written_once_with_the_last_write() {
        let runs = vec![
            run(1, 63, 0),
            run(2, 63, 0),
            run(1, 63, 6),
            run(1, 62, 1),
            run(2, 63, 7),
        ];
        let collapsed = collapse(runs);
        let blocks: Vec<(i32, i32, u32)> = collapsed.iter().map(|r| (r.x, r.y, r.block)).collect();
        assert_eq!(blocks, vec![(1, 63, 6), (2, 63, 7), (1, 62, 1)]);
        assert!(collapse(Vec::new()).is_empty());
    }

    #[test]
    fn what_a_crater_puts_down_is_named_as_the_game_names_it() {
        assert_eq!(Place::Vein.state(), "minecraft:sculk_vein[down=true]");
        assert_eq!(Place::Vein.name(), "minecraft:sculk_vein");
        assert_eq!(Place::Catalyst.state(), "minecraft:sculk_catalyst");
        assert_eq!(Place::Air.state(), "minecraft:air");
        // The edit's palette is every place in turn, so a run's block is the place's number.
        for (i, place) in Place::ALL.iter().enumerate() {
            assert_eq!(*place as usize, i);
        }
    }

    #[test]
    fn only_a_block_that_stops_things_is_solid_when_the_column_has_put_it_down() {
        assert!(!Place::Air.solid() && !Place::Vein.solid());
        for place in [
            Place::Sculk,
            Place::Deepslate,
            Place::CobbledDeepslate,
            Place::Tuff,
            Place::Stone,
            Place::Catalyst,
        ] {
            assert!(place.solid(), "{place:?}");
        }
    }

    #[test]
    fn a_block_is_put_back_over_the_crater_air_or_a_liquid_but_never_a_players_build() {
        assert!(can_restore_over("minecraft:sculk", "minecraft:sculk"));
        assert!(can_restore_over("minecraft:air", "minecraft:sculk"));
        assert!(can_restore_over("minecraft:cave_air", "minecraft:sculk"));
        assert!(can_restore_over("minecraft:water", "minecraft:air"));
        assert!(can_restore_over("minecraft:lava", "minecraft:sculk_vein"));
        assert!(!can_restore_over("minecraft:oak_planks", "minecraft:sculk"));
        assert!(!can_restore_over("minecraft:stone", "minecraft:air"));
    }

    #[test]
    fn positions_pack_without_clashing_across_the_worlds_range() {
        let spots = [
            [0, 0, 0],
            [1, 0, 0],
            [0, 1, 0],
            [0, 0, 1],
            [-1, 64, -1],
            [29_999_999, 319, 29_999_999],
            [-30_000_000, -64, -30_000_000],
            [100, -64, 100],
            [100, 319, 100],
        ];
        let packed: HashSet<u64> = spots.iter().map(|p| pack(*p)).collect();
        assert_eq!(packed.len(), spots.len());
    }

    #[test]
    fn a_list_of_changes_keeps_each_state_once_and_in_order() {
        let mut changes = Changes::default();
        changes.push([1, 2, 3], "minecraft:air", "minecraft:stone");
        changes.push([1, 1, 3], "minecraft:sculk", "minecraft:stone");
        changes.push([2, 2, 3], "minecraft:air", "minecraft:oak_log[axis=y]");
        assert_eq!(changes.len(), 3);
        assert_eq!(changes.strings.len(), 4, "air, stone, sculk and the log");
        let c = changes.list[2];
        assert_eq!(changes.placed(&c), "minecraft:air");
        assert_eq!(changes.original(&c), "minecraft:oak_log[axis=y]");
        assert_eq!(changes.list[0].pos, [1, 2, 3]);
    }

    #[test]
    fn a_journal_makes_the_same_changes_that_wrote_it() {
        let records = vec![
            journal::Record {
                pos: [5, 64, 5],
                placed: "minecraft:air".into(),
                original: "minecraft:grass_block".into(),
            },
            journal::Record {
                pos: [5, 63, 5],
                placed: "minecraft:sculk".into(),
                original: "minecraft:dirt".into(),
            },
        ];
        let changes = Changes::from_records(&records);
        let back: Vec<journal::Record> = changes
            .list
            .iter()
            .map(|c| journal::Record {
                pos: c.pos,
                placed: changes.placed(c).to_owned(),
                original: changes.original(c).to_owned(),
            })
            .collect();
        assert_eq!(back, records);
    }
}
