//! A world in memory, and the mod's real hooks driven over it.
//!
//! `FakeWorld` is the part of the engine's `Host` that the crater, the abilities and the command
//! use, over flat terrain (stone to y 62, grass at 63, air above), with a record of what was done
//! to it. `Rig` boxes the mod as the engine does and runs its hooks through the engine's own
//! `dispatch_*` functions, so what the tests exercise is what the server runs. The scenarios here
//! are the ones that matter most for the crater: it digs, it is put back exactly, it survives a
//! crash and a stop, and it never takes what is not its to take.

use crate::SculkStorm;
use crate::crater::{self, journal};
use crate::state::Shared;
use horizon_client::mod_api::{
    BlockPos, BlockStateId, Compound, Hooks, Host, LevelDimension, LevelHost, LevelRef,
    NativeBlockEdit, NativeBlockReadError, NativeHeightmap, NativeServerPlayer, NativeWorldEpoch,
    ParticleOptions, Text, dispatch_block_changed, dispatch_destroy_block,
    dispatch_entity_join_level, dispatch_level_tick_post, dispatch_level_tick_pre,
    dispatch_living_fall, dispatch_perform_console_command, dispatch_server_stopping,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const OVERWORLD: &str = "minecraft:overworld";
const AIR: &str = "minecraft:air";
const GRASS: &str = "minecraft:grass_block[snowy=false]";
const STONE: &str = "minecraft:stone";

/// The terrain before anything touches it.
fn natural(y: i32) -> &'static str {
    match y {
        ..=62 => STONE,
        63 => GRASS,
        _ => AIR,
    }
}

fn name_of(state: &str) -> &str {
    state.split('[').next().unwrap_or(state)
}

fn in_tag(tag: &str, block: &str) -> bool {
    let members: &[&str] = match tag {
        "minecraft:dirt" => &["minecraft:grass_block", "minecraft:dirt"],
        "minecraft:base_stone_overworld" => {
            &["minecraft:stone", "minecraft:deepslate", "minecraft:tuff"]
        }
        "minecraft:logs" => &["minecraft:oak_log"],
        "minecraft:leaves" => &["minecraft:oak_leaves"],
        "minecraft:flowers" => &["minecraft:poppy"],
        "minecraft:replaceable" => &[
            "minecraft:air",
            "minecraft:short_grass",
            "minecraft:snow",
            "minecraft:water",
            "minecraft:lava",
        ],
        _ => &[],
    };
    members.contains(&block)
}

/// What the tests can look at after the mod has been at the world.
pub struct FakeWorld {
    /// Blocks written since the start (the terrain stays `natural`).
    pub blocks: HashMap<[i32; 3], String>,
    /// Chunks that are not loaded; every other chunk is.
    pub unloaded: HashSet<(i32, i32)>,
    pub requested: Vec<(i32, i32)>,
    pub released: Vec<(i32, i32)>,
    pub summoned: Vec<u32>,
    pub removed: Vec<u32>,
    pub lightning: usize,
    pub sounds: usize,
    pub particles: usize,
    pub feedback: Vec<String>,
    /// What each block state id is, for `set_block`.
    pub states: HashMap<u32, String>,
    pub set_blocks: Vec<([i32; 3], String)>,
    /// What the world calls the overworld ("world" in Paper).
    pub world_name: Option<String>,
    dir: PathBuf,
    logs: RefCell<Vec<String>>,
    next_entity: u32,
}

impl FakeWorld {
    fn new(dir: PathBuf) -> Self {
        Self {
            blocks: HashMap::new(),
            unloaded: HashSet::new(),
            requested: Vec::new(),
            released: Vec::new(),
            summoned: Vec::new(),
            removed: Vec::new(),
            lightning: 0,
            sounds: 0,
            particles: 0,
            feedback: Vec::new(),
            states: HashMap::new(),
            set_blocks: Vec::new(),
            world_name: None,
            dir,
            logs: RefCell::new(Vec::new()),
            next_entity: 100,
        }
    }

    fn loaded(&self, pos: [i32; 3]) -> bool {
        !self.unloaded.contains(&(pos[0] >> 4, pos[2] >> 4))
    }

    pub fn state(&self, pos: [i32; 3]) -> String {
        self.blocks
            .get(&pos)
            .cloned()
            .unwrap_or_else(|| natural(pos[1]).to_owned())
    }

    /// Every block that is not as the terrain was.
    pub fn differences(&self) -> Vec<[i32; 3]> {
        let mut found: Vec<[i32; 3]> = self
            .blocks
            .iter()
            .filter(|(pos, state)| state.as_str() != natural(pos[1]))
            .map(|(pos, _)| *pos)
            .collect();
        found.sort_unstable();
        found
    }

    pub fn count(&self, name: &str) -> usize {
        self.blocks.values().filter(|s| name_of(s) == name).count()
    }

    pub fn logged(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }
}

impl Host for FakeWorld {
    fn level(&self, dimension: &str) -> Option<LevelRef> {
        (dimension == OVERWORLD).then(|| LevelRef::new(NativeWorldEpoch::new(1), OVERWORLD))
    }

    fn level_height(&self, _level: LevelRef) -> Option<(i32, i32)> {
        Some((-64, 384))
    }

    fn level_block(&self, _level: LevelRef, pos: [i32; 3]) -> Option<String> {
        self.loaded(pos)
            .then(|| name_of(&self.state(pos)).to_owned())
    }

    fn level_block_state(&self, _level: LevelRef, pos: [i32; 3]) -> Option<String> {
        self.loaded(pos).then(|| self.state(pos))
    }

    fn block_collision(&self, pos: [i32; 3]) -> Vec<[f64; 6]> {
        match name_of(&self.state(pos)) {
            "minecraft:air"
            | "minecraft:short_grass"
            | "minecraft:sculk_vein"
            | "minecraft:water"
            | "minecraft:lava"
            | "minecraft:snow" => Vec::new(),
            _ => vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]],
        }
    }

    fn block_name_in_tag(&self, block: &str, tag: &str) -> Result<bool, NativeBlockReadError> {
        Ok(in_tag(tag, block))
    }

    fn block_entity_nbt(&mut self, _dimension: &str, pos: [i32; 3]) -> Option<String> {
        (name_of(&self.state(pos)) == "minecraft:chest").then(|| "{}".to_owned())
    }

    fn chunk_is_loaded(&self, _dimension: &str, chunk: [i32; 2]) -> bool {
        !self.unloaded.contains(&(chunk[0], chunk[1]))
    }

    fn surface_heights(
        &self,
        _dimension: &str,
        columns: &[(i32, i32)],
        _heightmap: NativeHeightmap,
    ) -> Result<Vec<Option<i32>>, String> {
        Ok(columns
            .iter()
            .map(|&(x, z)| {
                if self.unloaded.contains(&(x >> 4, z >> 4)) {
                    return None;
                }
                // One above the highest block that is not air or a plant.
                (-64..200).rev().find_map(|y| {
                    let name = name_of(&self.state([x, y, z])).to_owned();
                    (name != AIR && name != "minecraft:short_grass").then_some(y + 1)
                })
            })
            .collect())
    }

    fn edit_blocks(&mut self, _dimension: &str, edit: &NativeBlockEdit) -> Result<u64, String> {
        let mut changed = 0;
        for run in &edit.runs {
            assert!(
                run.surface.is_none() && run.only.is_none(),
                "the mod makes plain runs"
            );
            let state = edit
                .palette
                .get(run.block as usize)
                .ok_or("a run names a palette entry past the end")?
                .clone();
            if !state.starts_with("minecraft:") {
                return Err(format!("{state:?} is not a block"));
            }
            for dy in 0..run.height as i32 {
                let pos = [run.x, run.y + dy, run.z];
                if !self.loaded(pos) {
                    // As the game does: runs in chunks that are not loaded are left out.
                    continue;
                }
                if self.state(pos) != state {
                    changed += 1;
                }
                self.blocks.insert(pos, state.clone());
            }
        }
        Ok(changed)
    }

    fn set_block(
        &mut self,
        _dimension: &str,
        pos: [i32; 3],
        state: BlockStateId,
        _flags: i32,
    ) -> bool {
        let name = self
            .states
            .get(&state.0)
            .cloned()
            .unwrap_or_else(|| AIR.to_owned());
        self.set_blocks.push((pos, name.clone()));
        self.blocks.insert(pos, name);
        true
    }

    fn server_players(&self) -> Vec<NativeServerPlayer> {
        Vec::new()
    }

    fn entities_in(&mut self, _dimension: &str, _min: [f64; 3], _max: [f64; 3]) -> Vec<u32> {
        Vec::new()
    }

    fn level_entities(&mut self, _dimension: &str) -> Vec<(u32, String, [f64; 3])> {
        Vec::new()
    }

    fn lightning_bolt(&mut self, _dimension: &str, _pos: [f64; 3], visual_only: bool) -> bool {
        assert!(visual_only, "the storm's lightning is only for show");
        self.lightning += 1;
        true
    }

    fn world_dir(&self) -> Option<PathBuf> {
        Some(self.dir.join("world"))
    }

    fn config_dir(&self) -> Result<PathBuf, String> {
        Ok(self.dir.join("config"))
    }

    fn log(&self, level: &str, line: &str) {
        self.logs.borrow_mut().push(format!("{level}: {line}"));
    }

    fn play_sound(&mut self, _s: &str, _at: [f64; 3], _src: usize, _v: f32, _p: f32) -> bool {
        self.sounds += 1;
        true
    }

    fn send_particles(
        &mut self,
        _options: &ParticleOptions,
        _state: Option<BlockStateId>,
        _at: [f64; 3],
        _spread: [f64; 3],
        _speed: f64,
        _count: i32,
    ) -> bool {
        self.particles += 1;
        true
    }

    fn summon(
        &mut self,
        _dimension: &str,
        kind: &str,
        _pos: [f64; 3],
        _nbt: Compound,
    ) -> Option<u32> {
        assert_eq!(
            kind, "minecraft:block_display",
            "only debris are summoned here"
        );
        self.next_entity += 1;
        self.summoned.push(self.next_entity);
        Some(self.next_entity)
    }

    fn move_entity(&mut self, _d: &str, id: u32, _pos: [f64; 3], _yaw: f32, _pitch: f32) -> bool {
        self.summoned.contains(&id) && !self.removed.contains(&id)
    }

    fn remove_entity(&mut self, _dimension: &str, id: u32) -> bool {
        self.removed.push(id);
        true
    }

    fn teleport_player(
        &mut self,
        _u: u128,
        _d: &str,
        _p: [f64; 3],
        _yaw: f32,
        _pitch: f32,
    ) -> bool {
        true
    }

    fn chunk_load_request(&mut self, _owner: &str, _dimension: &str, chunk: [i32; 2]) -> bool {
        self.requested.push((chunk[0], chunk[1]));
        true
    }

    fn chunk_load_release(&mut self, _owner: &str, _dimension: &str, chunk: [i32; 2]) -> bool {
        self.released.push((chunk[0], chunk[1]));
        true
    }

    fn dimension(&self) -> &'static str {
        OVERWORLD
    }

    fn world_of_dimension(&self, dimension: &str) -> Option<String> {
        (dimension == OVERWORLD).then(|| self.world_name.clone().unwrap_or_else(|| "world".into()))
    }

    fn dimension_of_world(&self, world: &str) -> Option<String> {
        (world == "world").then(|| OVERWORLD.to_owned())
    }

    fn feedback(&mut self, line: Text) {
        self.feedback.push(line.plain());
    }
}

/// The level hooks hear block changes through: a read-only view that knows nothing.
struct Level;

impl LevelHost for Level {
    fn block_state(&self, _pos: [i32; 3]) -> BlockStateId {
        BlockStateId(0)
    }
    fn is_client_side(&self) -> bool {
        false
    }
    fn state_with_property(&self, _: BlockStateId, _: &str, _: &str) -> Option<BlockStateId> {
        None
    }
}

/// A small crater that is dug in a tick and put back a second later.
pub const SMALL: &str = "[cataclysm]\nimpact-radius = 10.0\nlethal-radius = 4.0\nlightning-strikes = 5\n\
    lightning-duration-ticks = 10\n[terrain]\ncore-radius = 8\ncrater-depth = 4\ntendrils = 4\n\
    tendril-length = 10\nmax-debris = 6\ncolumns-per-tick = 2048\nmax-ms-per-tick = 50.0\n\
    restore-delay-seconds = 1\nrestore-blocks-per-tick = 20000\n";

/// Like `SMALL`, with a restore that never comes by itself (a test undoes it when it wants).
pub const SMALL_KEPT: &str = "[cataclysm]\nimpact-radius = 10.0\nlethal-radius = 4.0\nlightning-strikes = 5\n\
    lightning-duration-ticks = 10\n[terrain]\ncore-radius = 8\ncrater-depth = 4\ntendrils = 4\n\
    tendril-length = 10\nmax-debris = 6\ncolumns-per-tick = 2048\nmax-ms-per-tick = 50.0\n\
    restore-delay-seconds = 86400\nrestore-blocks-per-tick = 20000\n";

/// The mod boxed as the engine holds it, over a `FakeWorld`.
pub struct Rig {
    pub world: FakeWorld,
    pub mods: Vec<Box<dyn Hooks>>,
    pub shared: Arc<Shared>,
    dir: PathBuf,
}

impl Rig {
    /// A rig whose `config/sculkstorm.toml` holds `config`.
    pub fn new(config: &str) -> Self {
        static RIGS: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sculkstorm-rig-{}-{}",
            std::process::id(),
            RIGS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(dir.join("config").join("sculkstorm.toml"), config).unwrap();
        let module = SculkStorm::default();
        let shared = module.handle();
        let mut world = FakeWorld::new(dir.clone());
        world.states.insert(1, GRASS.to_owned());
        world.states.insert(2, "minecraft:sculk".to_owned());
        Self {
            world,
            mods: vec![Box::new(module)],
            shared,
            dir,
        }
    }

    /// Runs one server tick's worth of the overworld's hooks.
    pub fn tick(&mut self) {
        self.shared.advance();
        dispatch_level_tick_pre(&mut self.world, &self.mods, OVERWORLD);
        dispatch_level_tick_post(&mut self.world, &self.mods, OVERWORLD);
    }

    pub fn ticks(&mut self, n: usize) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// Ticks until `done`, which has to happen within `limit` ticks.
    pub fn tick_until(&mut self, limit: usize, what: &str, done: impl Fn(&Rig) -> bool) {
        for _ in 0..limit {
            if done(self) {
                return;
            }
            self.tick();
        }
        assert!(done(self), "{what} did not happen within {limit} ticks");
    }

    /// Types a line at the console. Whether the mod took it.
    pub fn console(&mut self, line: &str) -> bool {
        let mut ran = false;
        dispatch_perform_console_command(&mut self.world, &self.mods, line, &mut ran);
        ran
    }

    /// (being dug, waiting for the restore, being put back).
    pub fn craters(&self) -> (usize, usize, usize) {
        self.shared.state().craters.counts()
    }

    /// Fires a blast and ticks until the crater is dug and waiting.
    pub fn blast_and_dig(&mut self) {
        assert!(self.console("sculkstorm blast 0 64 0"));
        self.tick_until(300, "the crater being dug", |r| {
            let (digging, waiting, putting_back) = r.craters();
            digging == 0 && waiting + putting_back > 0
        });
    }

    /// The journals on disk (after what is waiting to be written has been).
    pub fn journals(&self) -> Vec<PathBuf> {
        crater::JOURNAL.fence();
        let Ok(entries) = std::fs::read_dir(journal::dir(&self.dir.join("world"))) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        found.sort();
        found
    }

    /// Writes a new config and has the mod read it.
    pub fn set_config(&mut self, config: &str) {
        std::fs::write(self.dir.join("config").join("sculkstorm.toml"), config).unwrap();
        assert!(self.console("sculkstorm reload"));
    }

    /// Fires a second blast over the first crater (which stands) and ticks until both are dug and
    /// waiting.
    pub fn blast_over_and_dig(&mut self) {
        assert!(self.console("sculkstorm blast 5 64 3"));
        self.tick_until(300, "the second crater being dug", |r| {
            let (digging, waiting, putting_back) = r.craters();
            digging == 0 && waiting + putting_back == 2
        });
    }

    /// Starts the server again over the same world: a new mod with nothing in memory, which then
    /// does what it does when a world opens.
    pub fn restart(&mut self) {
        let module = SculkStorm::default();
        self.shared = module.handle();
        self.mods = vec![Box::new(module)];
        // A world opening reads the config and the journals (`WorldOpenReady`'s work).
        crater::recover(&mut self.world, &self.shared, 0);
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blast_digs_a_crater_and_everything_is_put_back_exactly() {
        let mut rig = Rig::new(SMALL);
        assert!(rig.shared.state().cataclysms() == 0);
        rig.blast_and_dig();
        assert_eq!(
            rig.shared.state().cataclysms(),
            1,
            "one is going until its field is gone"
        );

        // The bowl: the middle is dug out, the floors are sculk, a few catalysts stand on them.
        assert_eq!(
            rig.world.state([0, 63, 0]),
            AIR,
            "the surface in the middle is gone"
        );
        let changed = rig.world.differences();
        assert!(changed.len() > 300, "{} blocks changed", changed.len());
        assert!(rig.world.count("minecraft:sculk") > 100);
        assert!(rig.world.count("minecraft:sculk_catalyst") > 0);
        assert!(rig.world.count("minecraft:air") > 100);
        // Nothing outside the crater's area (8 + 10 out, and 6 of margin) is touched.
        assert!(
            changed.iter().all(|p| p[0].abs() <= 24 && p[2].abs() <= 24),
            "{changed:?}"
        );
        // Nothing is dug below the bowl's floor: the deepest is 4 (and a pit's 1 or 2 more).
        assert!(
            changed.iter().all(|p| p[1] >= 63 - 7),
            "{:?}",
            changed.iter().map(|p| p[1]).min()
        );
        assert!(rig.world.lightning >= 3, "the opening bolts");

        // Each block is journalled with what it was, in the order it was changed.
        let journals = rig.journals();
        assert_eq!(journals.len(), 1);
        let (dimension, records) =
            journal::parse(&std::fs::read_to_string(&journals[0]).unwrap()).unwrap();
        assert_eq!(dimension, OVERWORLD);
        assert!(records.len() >= changed.len());
        let journalled: HashSet<[i32; 3]> = records.iter().map(|r| r.pos).collect();
        assert!(changed.iter().all(|p| journalled.contains(p)));
        assert!(
            records
                .iter()
                .filter(|r| r.pos == [0, 63, 0])
                .any(|r| r.original == GRASS && r.placed == AIR)
        );

        // Debris flew, and is all gone again once it has landed or run out its time.
        assert!(!rig.world.summoned.is_empty() && rig.world.summoned.len() <= 6);
        rig.tick_until(400, "the storm ending", |r| {
            r.shared.state().tasks.is_empty()
        });
        assert!(
            rig.world
                .summoned
                .iter()
                .all(|id| rig.world.removed.contains(id))
        );
        assert_eq!(
            rig.shared.state().cataclysms(),
            0,
            "the field's end frees the slot"
        );

        // After the delay the land is put back, block for block, and the journal goes.
        rig.tick_until(400, "the restore", |r| r.craters() == (0, 0, 0));
        assert_eq!(rig.world.differences(), Vec::<[i32; 3]>::new());
        assert!(rig.journals().is_empty());
        assert!(!rig.shared.watching());
    }

    #[test]
    fn undo_puts_a_waiting_crater_back_at_once_and_says_so() {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        assert_eq!(rig.craters(), (0, 1, 0), "it waits out a day");
        assert!(rig.console("sculkstorm undo"));
        assert!(
            rig.world
                .feedback
                .last()
                .unwrap()
                .starts_with("Undoing 1 crater, ")
        );
        rig.tick_until(50, "the restore", |r| r.craters() == (0, 0, 0));
        assert!(rig.world.differences().is_empty());
        assert!(rig.console("sculkstorm undo"));
        assert!(
            rig.world
                .feedback
                .last()
                .unwrap()
                .starts_with("Nothing to undo")
        );
    }

    #[test]
    fn a_block_built_in_the_crater_while_it_stands_is_left_alone_by_the_restore() {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        // Someone builds in the hole, and someone mines a floor block (it is air, which is fine).
        rig.world
            .blocks
            .insert([0, 63, 0], "minecraft:oak_planks".to_owned());
        let a_floor = *rig
            .world
            .blocks
            .iter()
            .find(|(p, s)| p[1] < 63 && name_of(s) == "minecraft:sculk")
            .map(|(p, _)| p)
            .unwrap();
        rig.world.blocks.insert(a_floor, AIR.to_owned());
        rig.console("sculkstorm undo");
        rig.tick_until(50, "the restore", |r| r.craters() == (0, 0, 0));
        assert_eq!(
            rig.world.differences(),
            vec![[0, 63, 0]],
            "only the build is left"
        );
        assert_eq!(rig.world.state([0, 63, 0]), "minecraft:oak_planks");
    }

    /// Two craters that overlap, the second dug while the first stands: the second's idea of the
    /// land under the overlap must be the land, not the first crater's work. The older is the one
    /// that comes due first (its delay is the short one) or the last.
    fn overlapping_craters(older_comes_back_first: bool) -> Rig {
        let (first, second) = if older_comes_back_first {
            (SMALL, SMALL_KEPT)
        } else {
            (SMALL_KEPT, SMALL)
        };
        let mut rig = Rig::new(first);
        rig.blast_and_dig();
        let one = rig.world.differences();
        rig.set_config(second);
        rig.blast_over_and_dig();
        let both = rig.world.differences();
        // The second really did work over the first, so the test means something.
        assert!(
            both.len() > one.len() + 100,
            "{} then {}",
            one.len(),
            both.len()
        );
        let overlap = one.iter().filter(|p| p[0] >= -3 && p[2] >= -5).count();
        assert!(overlap > 100, "only {overlap} blocks shared");
        rig
    }

    #[test]
    fn craters_that_overlap_are_put_back_exactly_when_the_older_comes_back_first() {
        let mut rig = overlapping_craters(true);
        // The older one's timer runs out first; the newer waits a day.
        rig.tick_until(100, "the first restore", |r| r.craters() == (0, 1, 0));
        assert!(rig.console("sculkstorm undo"));
        rig.tick_until(100, "the second restore", |r| r.craters() == (0, 0, 0));
        assert_eq!(rig.world.differences(), Vec::<[i32; 3]>::new());
        assert!(rig.journals().is_empty());
    }

    #[test]
    fn craters_that_overlap_are_put_back_exactly_when_the_newer_comes_back_first() {
        let mut rig = overlapping_craters(false);
        rig.tick_until(100, "the first restore", |r| r.craters() == (0, 1, 0));
        assert!(rig.console("sculkstorm undo"));
        rig.tick_until(100, "the second restore", |r| r.craters() == (0, 0, 0));
        assert_eq!(rig.world.differences(), Vec::<[i32; 3]>::new());
        assert!(rig.journals().is_empty());
    }

    #[test]
    fn overlapping_craters_that_a_crash_left_are_put_back_exactly_from_their_journals() {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        rig.blast_over_and_dig();
        assert_eq!(rig.journals().len(), 2);
        rig.restart();
        assert_eq!(rig.craters(), (0, 0, 2));
        rig.tick_until(100, "the restore", |r| r.craters() == (0, 0, 0));
        assert_eq!(rig.world.differences(), Vec::<[i32; 3]>::new());
        assert!(rig.journals().is_empty());
    }

    #[test]
    fn a_crater_in_chunks_that_are_not_loaded_waits_for_them_and_asks_for_them() {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        let dug = rig.world.differences();
        rig.world.unloaded.extend([(0, 0), (-1, -1)]);
        rig.console("sculkstorm undo");
        rig.ticks(5);
        assert_eq!(rig.craters(), (0, 0, 1), "it cannot finish yet");
        assert!(rig.world.requested.contains(&(0, 0)) && rig.world.requested.contains(&(-1, -1)));
        // The loaded chunks are back already; the unloaded ones are as they were left.
        let left = rig.world.differences();
        assert!(!left.is_empty() && left.len() < dug.len());
        assert!(
            left.iter()
                .all(|p| rig.world.unloaded.contains(&(p[0] >> 4, p[2] >> 4)))
        );
        // They load, and the rest is put back and the requests are let go.
        rig.world.unloaded.clear();
        rig.tick_until(50, "the restore", |r| r.craters() == (0, 0, 0));
        assert!(rig.world.differences().is_empty());
        for chunk in [(0, 0), (-1, -1)] {
            assert!(rig.world.released.contains(&chunk), "{chunk:?} is let go");
        }
    }

    #[test]
    fn a_server_that_crashed_with_a_crater_standing_puts_it_back_when_it_starts() {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        let dug = rig.world.differences();
        assert!(!dug.is_empty());
        assert_eq!(rig.journals().len(), 1);
        // The crash: the mod's memory is gone, the world and the journal on disk are not.
        rig.restart();
        assert_eq!(rig.craters(), (0, 0, 1), "the journal makes a restore");
        assert!(
            rig.world
                .logged()
                .iter()
                .any(|l| l.contains("left over from a server stop"))
        );
        rig.tick_until(50, "the restore", |r| r.craters() == (0, 0, 0));
        assert!(rig.world.differences().is_empty());
        assert!(
            rig.journals().is_empty(),
            "the journal goes once the land is back"
        );
    }

    #[test]
    fn a_journal_cut_short_by_a_crash_still_restores_what_it_has() {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        let file = rig.journals().remove(0);
        // The crash cut the file in the middle of its last line.
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, &text[..text.len() - 9]).unwrap();
        rig.restart();
        rig.tick_until(50, "the restore", |r| r.craters() == (0, 0, 0));
        // All but the last block (the line that was cut) are back.
        assert!(rig.world.differences().len() <= 1);
    }

    #[test]
    fn a_server_that_stops_puts_every_crater_back_first_whether_dug_or_still_digging() {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        assert!(!rig.world.differences().is_empty());
        dispatch_server_stopping(&mut rig.world, &rig.mods, true);
        assert!(rig.world.differences().is_empty());
        assert!(rig.journals().is_empty());
        assert!(!rig.shared.watching());

        // One that is only half dug when the server stops.
        let slow = SMALL_KEPT.replace("columns-per-tick = 2048", "columns-per-tick = 40");
        let mut rig = Rig::new(&slow);
        assert!(rig.console("sculkstorm blast 0 64 0"));
        rig.ticks(4);
        assert_eq!(rig.craters().0, 1, "still being dug");
        assert!(!rig.world.differences().is_empty());
        dispatch_server_stopping(&mut rig.world, &rig.mods, true);
        assert!(rig.world.differences().is_empty());
        assert!(rig.journals().is_empty());
        assert_eq!(rig.craters(), (0, 0, 0));
    }

    #[test]
    fn what_cannot_be_broken_is_not_and_a_build_on_the_ground_is_spared_unless_builds_may_break() {
        let place = |rig: &mut Rig| {
            rig.world
                .blocks
                .insert([1, 63, 0], "minecraft:chest[facing=north]".to_owned());
            rig.world
                .blocks
                .insert([0, 63, 1], "minecraft:bedrock".to_owned());
            rig.world
                .blocks
                .insert([2, 63, 0], "minecraft:oak_planks".to_owned());
        };
        // Builds may break: the planks go, the chest and the bedrock stay.
        let mut rig = Rig::new(SMALL_KEPT);
        place(&mut rig);
        rig.blast_and_dig();
        assert_eq!(rig.world.state([1, 63, 0]), "minecraft:chest[facing=north]");
        assert_eq!(rig.world.state([0, 63, 1]), "minecraft:bedrock");
        assert_ne!(
            rig.world.state([2, 63, 0]),
            "minecraft:oak_planks",
            "the planks are carved"
        );
        rig.console("sculkstorm undo");
        rig.tick_until(50, "the restore", |r| r.craters() == (0, 0, 0));
        assert_eq!(
            rig.world.state([2, 63, 0]),
            "minecraft:oak_planks",
            "and put back"
        );
        assert_eq!(
            rig.world.differences(),
            vec![[0, 63, 1], [1, 63, 0], [2, 63, 0]]
        );

        // Builds may not: a column with a build on top is skipped whole.
        let natural_only = SMALL_KEPT.replace("[terrain]", "[terrain]\nbreak-all-blocks = false");
        let mut rig = Rig::new(&natural_only);
        place(&mut rig);
        rig.blast_and_dig();
        assert_eq!(rig.world.state([2, 63, 0]), "minecraft:oak_planks");
        assert_eq!(
            rig.world.state([2, 62, 0]),
            STONE,
            "nothing under the build is touched"
        );
        assert_eq!(
            rig.world.state([0, 63, 0]),
            AIR,
            "the open ground beside it is"
        );
    }

    #[test]
    fn sculk_that_spreads_in_the_crater_is_put_back_with_it_and_the_rest_of_the_world_is_not_watched()
     {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        // A catalyst blooms at the edge of the area, where the crater itself touched nothing.
        let spot = [23, 63, 0];
        assert_eq!(rig.world.state(spot), GRASS);
        rig.world.blocks.insert(spot, "minecraft:sculk".to_owned());
        let far = [500, 63, 500];
        rig.world.blocks.insert(far, "minecraft:sculk".to_owned());
        let level = Level;
        let hear = |at: [i32; 3]| {
            dispatch_block_changed(
                &level,
                &rig.mods,
                LevelDimension::Overworld,
                BlockPos::new(at[0], at[1], at[2]),
                BlockStateId(1),
                BlockStateId(2),
            );
        };
        hear(spot);
        hear(far);
        rig.tick();
        rig.console("sculkstorm undo");
        rig.tick_until(50, "the restore", |r| r.craters() == (0, 0, 0));
        assert_eq!(rig.world.state(spot), GRASS, "put back with the crater");
        assert_eq!(rig.world.set_blocks.len(), 1);
        assert_eq!(
            rig.world.state(far),
            "minecraft:sculk",
            "nobody watches the far world"
        );
    }

    #[test]
    fn a_crater_block_a_player_breaks_gives_no_experience_but_other_orbs_are_kept() {
        let mut rig = Rig::new(SMALL_KEPT);
        rig.blast_and_dig();
        let catalyst = *rig
            .world
            .blocks
            .iter()
            .find(|(_, s)| name_of(s) == "minecraft:sculk_catalyst")
            .map(|(p, _)| p)
            .expect("a catalyst stands in the crater");
        let mut canceled = false;
        dispatch_destroy_block(
            &mut rig.world,
            &rig.mods,
            catalyst,
            BlockStateId(0),
            &mut canceled,
        );
        assert!(!canceled, "the break itself goes ahead");
        let at = catalyst.map(|c| f64::from(c) + 0.5);
        // Whether the mod keeps something that appeared at `at` out of the level.
        fn kept_out(rig: &mut Rig, kind: &str, at: [f64; 3]) -> bool {
            let mut cancel = false;
            dispatch_entity_join_level(
                &mut rig.world,
                &rig.mods,
                OVERWORLD,
                900,
                kind,
                at,
                false,
                &mut cancel,
            );
            cancel
        }
        assert!(
            kept_out(&mut rig, "minecraft:experience_orb", at),
            "the catalyst's experience is taken"
        );
        assert!(
            !kept_out(&mut rig, "minecraft:item", at),
            "anything else that appears there is not"
        );
        let beside = [at[0] + 5.0, at[1], at[2]];
        assert!(
            !kept_out(&mut rig, "minecraft:experience_orb", beside),
            "an orb elsewhere is not"
        );
        // A block that was never the crater's is broken as ever.
        let mut cancel = false;
        dispatch_destroy_block(
            &mut rig.world,
            &rig.mods,
            [200, 62, 200],
            BlockStateId(0),
            &mut cancel,
        );
        assert!(!kept_out(
            &mut rig,
            "minecraft:experience_orb",
            [200.5, 62.5, 200.5]
        ));
    }

    #[test]
    fn the_ascension_fall_is_forgiven_once_and_only_a_real_fall_counts() {
        let rig = Rig::new(SMALL);
        let uuid = 0xa11ce;
        rig.shared.state().protect_from_fall(
            uuid,
            7,
            std::time::Instant::now() + std::time::Duration::from_secs(12),
        );
        let fall = |entity: u32, kind: &str, distance: f64| {
            let (mut distance, mut multiplier, mut cancel) = (distance, 1.0f32, false);
            dispatch_living_fall(
                &rig.mods,
                entity,
                kind,
                &mut distance,
                &mut multiplier,
                &mut cancel,
            );
            (cancel, multiplier)
        };
        assert_eq!(
            fall(7, "minecraft:player", 2.5),
            (false, 1.0),
            "a short hop spends nothing"
        );
        assert_eq!(
            fall(8, "minecraft:player", 20.0),
            (false, 1.0),
            "someone else's fall"
        );
        assert_eq!(
            fall(7, "minecraft:zombie", 20.0),
            (false, 1.0),
            "not a player"
        );
        assert_eq!(
            fall(7, "minecraft:player", 20.0),
            (true, 0.0),
            "the fall that was forgiven"
        );
        assert_eq!(fall(7, "minecraft:player", 20.0), (false, 1.0), "only once");
    }

    #[test]
    fn a_world_where_the_scythe_is_off_gets_the_storm_but_not_the_crater() {
        for off in [
            "disabled-worlds = [\"world\"]\n",
            "disabled-worlds = [\"minecraft:overworld\"]\n",
            "",
        ] {
            let config = if off.is_empty() {
                SMALL.replace("[terrain]", "[terrain]\nenabled = false")
            } else {
                format!("{off}{SMALL}")
            };
            let mut rig = Rig::new(&config);
            assert!(rig.console("sculkstorm blast 0 64 0"));
            rig.ticks(40);
            assert!(
                rig.world.lightning >= 3,
                "the lightning still strikes ({off:?})"
            );
            assert!(rig.world.blocks.is_empty(), "no block is touched ({off:?})");
            assert_eq!(rig.craters(), (0, 0, 0));
        }
    }

    #[test]
    fn the_command_reloads_the_config_keeps_the_old_one_when_the_new_does_not_read_and_says_usage()
    {
        let mut rig = Rig::new("pvp = true\n");
        assert!(
            !rig.console("gamemode creative"),
            "someone else's command is not ours"
        );
        assert!(rig.console("sculkstorm reload"));
        assert_eq!(
            rig.world.feedback.last().unwrap(),
            "Reloaded the Sculkstorm config!"
        );
        assert!(rig.shared.config().pvp);
        let file = rig.dir.join("config").join("sculkstorm.toml");
        std::fs::write(&file, "pvp = [").unwrap();
        assert!(rig.console("sculkstorm reload"));
        assert!(
            rig.world
                .feedback
                .last()
                .unwrap()
                .starts_with("Could not read the config")
        );
        assert!(rig.shared.config().pvp, "the old settings stay");
        std::fs::write(&file, "pvp = false\n").unwrap();
        assert!(rig.console("sculkstorm reload"));
        assert!(!rig.shared.config().pvp);
        assert!(rig.console("sculkstorm"));
        assert_eq!(
            rig.world.feedback[rig.world.feedback.len() - 2..],
            [
                "Unknown or incomplete command. See below for error",
                "sculkstorm<--[Usage: /sculkstorm give <player> | blast <x> <y> <z> [world] | undo | reload]"
            ]
        );
    }

    #[test]
    fn a_player_who_is_not_on_and_a_position_that_is_not_numbers_are_told_where_the_line_went_wrong()
     {
        let mut rig = Rig::new(SMALL);
        assert!(rig.console("sculkstorm give Ja"));
        assert_eq!(
            rig.world.feedback,
            [
                "Unknown or incomplete command. See below for error",
                "...rm give Ja<--[Player was not found]"
            ]
        );
        rig.world.feedback.clear();
        assert!(rig.console("sculkstorm blast a b c"));
        assert_eq!(
            rig.world.feedback,
            [
                "Unknown or incomplete command. See below for error",
                "...last a b c<--[The position must be three numbers: /sculkstorm blast <x> <y> <z> [world]]"
            ]
        );
        assert!(rig.shared.state().tasks.is_empty());
    }

    #[test]
    fn a_blast_in_a_world_that_does_not_exist_does_nothing_and_says_so() {
        let mut rig = Rig::new(SMALL);
        assert!(rig.console("sculkstorm blast 0 64 0 nowhere"));
        assert_eq!(
            rig.world.feedback,
            [
                "Unknown or incomplete command. See below for error",
                "...0 nowhere<--[Unknown world]"
            ]
        );
        assert_eq!(rig.shared.state().cataclysms(), 0);
        assert!(rig.shared.state().tasks.is_empty());
        // A world named by its name or its dimension is found.
        assert!(rig.console("sculkstorm blast 0 64 0 world"));
        assert!(
            rig.world
                .feedback
                .last()
                .unwrap()
                .starts_with("Set off a cataclysm at 0 64 0")
        );
        assert!(rig.console("sculkstorm blast 0 64 0 minecraft:overworld"));
        assert_eq!(rig.shared.state().cataclysms(), 2);
    }
}
