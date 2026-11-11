//! Craters: planned, dug, protected while they stand, and put back.
//!
//! Dug a little each tick under a budget (`columns-per-tick`, `max-ms-per-tick`) so a crater of
//! thousands of columns does not spike the server, and put back the same way after
//! `restore-delay-seconds`. Every change is journalled to disk as it is made, so a server that
//! stops uncleanly still restores the land the next time it starts.

pub mod debris;
mod job;
pub mod journal;
pub mod plan;
pub mod terrain;

use crate::rng::Rng;
use crate::state::Shared;
use crate::tasks::{Cx, Task};
use horizon_client::mod_api::{BlockStateId, Host};
use job::{Changes, Foreign, RestoreJob, SpreadJob, pack};
use plan::{Area, Params};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The thread that writes the journals, for every crater.
pub static JOURNAL: journal::Writer = journal::Writer::new();

/// A crater's area while it stands.
struct Watch {
    id: u64,
    dimension: String,
    area: Area,
}

/// A block that a standing crater changed: the crater whose work is on top of it now, and what
/// the land really was before any crater touched it.
struct Live {
    owner: u64,
    original: Arc<str>,
}

/// A block changed in a watched area, to look at when the tick ends.
pub struct Candidate {
    pub id: u64,
    pub pos: [i32; 3],
    pub old: BlockStateId,
}

/// Every crater there is, and what is kept about them.
#[derive(Default)]
pub struct Craters {
    spread: VecDeque<SpreadJob>,
    /// Done digging, waiting out the restore delay.
    waiting: Vec<RestoreJob>,
    restoring: VecDeque<RestoreJob>,
    watched: Vec<Watch>,
    /// The blocks the craters placed that are still there, by dimension.
    placed: HashMap<String, HashSet<u64>>,
    candidates: HashMap<String, Vec<Candidate>>,
    foreign: HashMap<u64, Vec<Foreign>>,
    /// Every block a standing crater has changed and not yet put back, by dimension: so that a
    /// crater dug over another one knows what the land is under the other's work.
    live: HashMap<String, HashMap<u64, Live>>,
    /// Blocks a player broke that were a crater's: they drop nothing and give no experience.
    no_drops: HashSet<(String, [i32; 3])>,
    xp: Vec<(String, [i32; 3], u64)>,
    next_id: u64,
}

/// The most block changes kept to look at in one tick.
const CANDIDATES_MAX: usize = 20_000;

/// The sculk blocks the plugin keeps from spreading in a crater.
pub fn is_sculk_family(name: &str) -> bool {
    matches!(
        name,
        "minecraft:sculk"
            | "minecraft:sculk_vein"
            | "minecraft:sculk_sensor"
            | "minecraft:sculk_shrieker"
    )
}

/// The smallest area that holds every journalled block.
fn bounds(records: &[journal::Record]) -> Area {
    let mut area = Area {
        min_x: i32::MAX,
        min_z: i32::MAX,
        max_x: i32::MIN,
        max_z: i32::MIN,
    };
    for r in records {
        area.min_x = area.min_x.min(r.pos[0]);
        area.max_x = area.max_x.max(r.pos[0]);
        area.min_z = area.min_z.min(r.pos[2]);
        area.max_z = area.max_z.max(r.pos[2]);
    }
    area
}

impl Craters {
    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn watch(&mut self, id: u64, dimension: &str, area: Area) {
        self.watched.push(Watch {
            id,
            dimension: dimension.to_owned(),
            area,
        });
    }

    fn unwatch(&mut self, id: u64) {
        self.watched.retain(|w| w.id != id);
    }

    /// The crater whose area holds the column `x, z` of `dimension`.
    pub fn watched_by(&self, dimension: &str, x: i32, z: i32) -> Option<u64> {
        self.watched
            .iter()
            .find(|w| w.dimension == dimension && w.area.contains(x, z))
            .map(|w| w.id)
    }

    /// Whether any crater's area is being watched.
    pub fn is_active(&self) -> bool {
        !self.watched.is_empty()
    }

    /// How many craters are being dug, waiting out their delay, and being put back.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        (self.spread.len(), self.waiting.len(), self.restoring.len())
    }

    pub fn add_placed(&mut self, dimension: &str, positions: &[[i32; 3]]) {
        if positions.is_empty() {
            return;
        }
        let set = self.placed.entry(dimension.to_owned()).or_default();
        set.extend(positions.iter().map(|p| pack(*p)));
    }

    pub fn forget_placed(&mut self, dimension: &str, positions: &[[i32; 3]]) {
        if let Some(set) = self.placed.get_mut(dimension) {
            for p in positions {
                set.remove(&pack(*p));
            }
        }
    }

    pub fn is_placed(&self, dimension: &str, pos: [i32; 3]) -> bool {
        self.placed
            .get(dimension)
            .is_some_and(|set| set.contains(&pack(pos)))
    }

    /// A player broke the block at `pos`: whether it was a crater's, which it then no longer is.
    /// Such a block drops nothing and gives no experience, or people would farm catalysts out of
    /// the crater before it is put back.
    pub fn broke(&mut self, dimension: &str, pos: [i32; 3], tick: u64) -> bool {
        let was = self
            .placed
            .get_mut(dimension)
            .is_some_and(|set| set.remove(&pack(pos)));
        if was {
            self.no_drops.insert((dimension.to_owned(), pos));
            self.xp.push((dimension.to_owned(), pos, tick));
        }
        was
    }

    /// Whether the drops of the block at `pos` are to be taken away (once).
    pub fn take_no_drops(&mut self, dimension: &str, pos: [i32; 3]) -> bool {
        self.no_drops.remove(&(dimension.to_owned(), pos))
    }

    /// Whether an experience orb that appeared at `pos` in `tick` came from a crater block that
    /// was broken just now. A block can drop several, so a mark stays for the tick.
    pub fn is_crater_xp(&mut self, dimension: &str, pos: [i32; 3], tick: u64) -> bool {
        self.xp.retain(|(_, _, at)| tick <= at + 1);
        self.xp.iter().any(|(d, p, _)| d == dimension && *p == pos)
    }

    /// What crater `owner` is to record as the original of the block at `pos`, which its own
    /// reading found to be `seen`.
    ///
    /// Where another crater that still stands changed the block first, what is there now is that
    /// crater's work, and the land is what that crater found. Every crater that touches the block
    /// has to put the land back, whichever of them comes back first: a restore only writes over a
    /// block that is still its own (or gone), so one that recorded the other's work as the
    /// original would put the other's work back when it came last.
    pub fn original_for(
        &mut self,
        dimension: &str,
        pos: [i32; 3],
        owner: u64,
        seen: &str,
    ) -> Arc<str> {
        let live = self.live.entry(dimension.to_owned()).or_default();
        match live.get_mut(&pack(pos)) {
            // The same crater changing it again: what it saw is what its first change left, and
            // its restore undoes the two in order.
            Some(held) if held.owner == owner => Arc::from(seen),
            // Another's work stands there, and now this crater's is on top of it.
            Some(held) => {
                held.owner = owner;
                Arc::clone(&held.original)
            }
            None => {
                let original: Arc<str> = Arc::from(seen);
                live.insert(
                    pack(pos),
                    Live {
                        owner,
                        original: Arc::clone(&original),
                    },
                );
                original
            }
        }
    }

    /// Blocks a restore has put back hold no crater's work any more.
    pub fn forget_live(&mut self, dimension: &str, positions: &[[i32; 3]]) {
        if let Some(live) = self.live.get_mut(dimension) {
            for p in positions {
                live.remove(&pack(*p));
            }
        }
    }

    /// A crater is done with (put back, or given up): the blocks it still holds the work on are
    /// let go, whether it put them back or found a player's build over them.
    fn release_live(&mut self, owner: u64) {
        for live in self.live.values_mut() {
            live.retain(|_, held| held.owner != owner);
        }
    }

    pub fn add_foreign(&mut self, id: u64, mut list: Vec<Foreign>) {
        self.foreign.entry(id).or_default().append(&mut list);
    }

    pub fn take_foreign(&mut self, id: u64) -> Vec<Foreign> {
        self.foreign.remove(&id).unwrap_or_default()
    }

    pub fn foreign_count(&self, id: u64) -> usize {
        self.foreign.get(&id).map_or(0, Vec::len)
    }

    /// A block changed in `dimension` at `pos` from `old`: kept, if a crater watches the column,
    /// to look at once the tick is over.
    pub fn candidate(&mut self, dimension: &str, pos: [i32; 3], old: BlockStateId) {
        let Some(id) = self.watched_by(dimension, pos[0], pos[2]) else {
            return;
        };
        let list = self.candidates.entry(dimension.to_owned()).or_default();
        if list.len() < CANDIDATES_MAX {
            list.push(Candidate { id, pos, old });
        }
    }

    pub fn take_candidates(&mut self, dimension: &str) -> Vec<Candidate> {
        self.candidates.remove(dimension).unwrap_or_default()
    }

    pub fn queue_spread(&mut self, job: SpreadJob, restore: bool) {
        if restore {
            self.watch(job.id, &job.dimension, job.area());
        }
        self.spread.push_back(job);
    }

    fn take_spread(&mut self, dimension: &str) -> Option<SpreadJob> {
        let at = self.spread.iter().position(|j| j.dimension == dimension)?;
        self.spread.remove(at)
    }

    fn put_back_spread(&mut self, job: SpreadJob) {
        self.spread.push_front(job);
    }

    /// A crater is dug: it waits for its restore, or, with nothing to put back, is forgotten.
    fn finish_spread(&mut self, job: SpreadJob, due_tick: u64, restore: bool) {
        if !restore || job.changes().is_empty() {
            self.unwatch(job.id);
            self.release_live(job.id);
            if let Some(file) = job.journal() {
                JOURNAL.delete(file);
            }
            return;
        }
        self.waiting.push(job.into_restore(due_tick));
    }

    /// Moves what has waited out its delay to the restore queue.
    fn promote_due(&mut self, tick: u64) {
        let mut i = 0;
        while i < self.waiting.len() {
            if self.waiting[i].due_tick <= tick {
                let job = self.waiting.swap_remove(i);
                self.restoring.push_back(job);
            } else {
                i += 1;
            }
        }
    }

    fn take_restore(&mut self, dimension: &str) -> Option<RestoreJob> {
        let at = self
            .restoring
            .iter()
            .position(|j| j.dimension == dimension)?;
        self.restoring.remove(at)
    }

    fn put_back_restore(&mut self, job: RestoreJob) {
        self.restoring.push_front(job);
    }

    /// A restored crater is forgotten, and its journal goes.
    fn finish_restore(&mut self, job: &RestoreJob) {
        self.unwatch(job.id);
        self.release_live(job.id);
        self.forget_placed(&job.dimension, &job.placed_positions());
        self.foreign.remove(&job.id);
        if let Some(file) = job.journal() {
            JOURNAL.delete(file);
        }
    }

    /// A restore that came from a journal: watched, and its blocks are the crater's again.
    fn queue_restore(&mut self, job: RestoreJob) {
        self.watch(job.id, &job.dimension, job.area());
        let placed = job.placed_positions();
        self.add_placed(&job.dimension, &placed);
        // Its blocks are standing crater work again: a crater dug over them before they are back
        // has to know the land. The first change of a block recorded the land.
        let live = self.live.entry(job.dimension.clone()).or_default();
        for (pos, original) in job.originals() {
            live.entry(pack(pos)).or_insert_with(|| Live {
                owner: job.id,
                original: Arc::from(original),
            });
        }
        self.restoring.push_back(job);
    }

    /// `/sculkstorm undo`: every crater, whatever it is doing, is put back now (at the normal
    /// speed). How many there were.
    pub fn undo_all(&mut self, tick: u64) -> usize {
        let mut count = self.restoring.len();
        for mut job in std::mem::take(&mut self.spread) {
            job.flush_journal();
            self.restoring.push_back(job.into_restore(tick));
            count += 1;
        }
        count += self.waiting.len();
        for mut job in std::mem::take(&mut self.waiting) {
            job.due_tick = tick;
            self.restoring.push_back(job);
        }
        count
    }

    /// Every crater as a restore: what the server does when it stops.
    fn drain_all(&mut self) -> Vec<RestoreJob> {
        for mut job in std::mem::take(&mut self.spread) {
            job.flush_journal();
            self.restoring.push_back(job.into_restore(0));
        }
        let mut jobs: Vec<RestoreJob> = self.restoring.drain(..).collect();
        jobs.append(&mut self.waiting);
        jobs
    }

    fn forget_all(&mut self) {
        self.watched.clear();
        self.placed.clear();
        self.live.clear();
        self.candidates.clear();
        self.no_drops.clear();
        self.xp.clear();
    }
}

/// The impact of a cataclysm at `at`: plans the crater and queues it, to be dug in the next ticks.
pub fn start(cx: &mut Cx<'_>, dimension: &str, at: [f64; 3]) {
    let t = cx.cfg.terrain.clone();
    let (x, y, z) = (
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    );
    let Some(cy) = job::ground_below(cx.host, dimension, t.break_all_blocks, [x, y, z]) else {
        return;
    };
    let params = Params {
        core: t.core_radius,
        depth: t.crater_depth,
        roots: t.tendrils,
        root_len: t.tendril_length,
        max_debris: t.max_debris,
    };
    let plan = plan::plan(cx.rng, x, z, &params);
    let id = cx.shared.state().craters.next_id();
    let journal = t
        .restore
        .then(|| cx.host.world_dir())
        .flatten()
        .map(|world| journal::dir(&world).join(journal_name(id)));
    let job = SpreadJob::new(
        id,
        dimension,
        [x, cy, z],
        plan,
        t.break_all_blocks,
        t.restore,
        journal,
        Rng::from_entropy(),
    );
    cx.shared.state().craters.queue_spread(job, t.restore);
    if t.restore {
        cx.shared.set_watching(true);
    }
}

/// A journal's file name: the time (so a later run never reuses one) and the crater's number.
fn journal_name(id: u64) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{millis:013}-{id}.log")
}

/// One tick of the crater work in `dimension`: spreading, then restoring.
pub fn tick(cx: &mut Cx<'_>, dimension: &str) {
    cx.shared.state().craters.promote_due(cx.tick);
    spread_tick(cx, dimension);
    restore_tick(cx, dimension);
    let active = cx.shared.state().craters.is_active();
    cx.shared.set_watching(active);
}

fn spread_tick(cx: &mut Cx<'_>, dimension: &str) {
    let t = cx.cfg.terrain.clone();
    let mut budget = t.columns_per_tick;
    let deadline = Instant::now() + Duration::from_secs_f64(t.max_ms_per_tick / 1000.0);
    while budget > 0 && Instant::now() < deadline {
        let job = cx.shared.state().craters.take_spread(dimension);
        let Some(mut job) = job else {
            break;
        };
        let placed = job.advance(cx, &mut budget, deadline);
        cx.shared.state().craters.add_placed(dimension, &placed);
        if job.finished() {
            if cx.cfg.debug {
                cx.host
                    .log("info", &format!("Sculkstorm: {}", job.summary()));
            }
            let due = cx.tick + u64::try_from(t.restore_delay_seconds.max(1)).unwrap_or(1) * 20;
            cx.shared.state().craters.finish_spread(job, due, t.restore);
        } else {
            cx.shared.state().craters.put_back_spread(job);
            break;
        }
    }
}

fn restore_tick(cx: &mut Cx<'_>, dimension: &str) {
    let t = cx.cfg.terrain.clone();
    let mut budget = t.restore_blocks_per_tick;
    let deadline = Instant::now()
        + Duration::from_secs_f64(t.max_ms_per_tick / 1000.0)
        + Duration::from_millis(2);
    while budget > 0 && Instant::now() < deadline {
        let job = cx.shared.state().craters.take_restore(dimension);
        let Some(mut job) = job else {
            break;
        };
        let mut restored = Vec::new();
        let done = job.restore(cx.host, cx.shared, &mut budget, &mut restored);
        {
            let mut state = cx.shared.state();
            state.craters.forget_placed(dimension, &restored);
            state.craters.forget_live(dimension, &restored);
        }
        job::unstick_players(cx.host, dimension, job.area());
        if done {
            if cx.cfg.debug {
                cx.host.log(
                    "info",
                    &format!("Sculkstorm: crater restored ({} blocks).", job.len()),
                );
            }
            job.release_chunks(cx.host);
            cx.shared.state().craters.finish_restore(&job);
        } else {
            cx.shared.state().craters.put_back_restore(job);
            break;
        }
    }
}

/// The sculk that spread in a crater's area (a catalyst blooms when something dies near it) is
/// kept to put back with the crater, since a mod cannot stop it. `dimension`'s level is in place.
pub fn resolve_candidates(host: &mut dyn Host, shared: &Shared, dimension: &str) {
    let candidates = shared.state().craters.take_candidates(dimension);
    if candidates.is_empty() {
        return;
    }
    let Some(level) = host.level(dimension) else {
        return;
    };
    let mut found: HashMap<u64, Vec<Foreign>> = HashMap::new();
    for c in candidates {
        let Some(name) = host.level_block(level, c.pos) else {
            continue;
        };
        if !is_sculk_family(&name) || shared.state().craters.is_placed(dimension, c.pos) {
            continue;
        }
        found.entry(c.id).or_default().push(Foreign {
            pos: c.pos,
            old: c.old,
            new_name: name,
        });
    }
    let mut state = shared.state();
    for (id, list) in found {
        state.craters.add_foreign(id, list);
    }
}

/// Puts back what an unclean stop left: every journal in the world's folder.
pub fn recover(host: &mut dyn Host, shared: &Shared, tick: u64) {
    let Some(world) = host.world_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(journal::dir(&world)) else {
        return;
    };
    let mut jobs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "log") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                host.log(
                    "warn",
                    &format!(
                        "Sculkstorm: could not read the crater journal {}: {e}",
                        path.display()
                    ),
                );
                continue;
            }
        };
        match journal::parse(&text) {
            Some((dimension, records)) if !records.is_empty() => {
                let id = shared.state().craters.next_id();
                jobs.push(RestoreJob::new(
                    id,
                    dimension,
                    Changes::from_records(&records),
                    bounds(&records),
                    tick,
                    Some(path),
                ));
            }
            // Nothing in it to put back.
            _ => JOURNAL.delete(&path),
        }
    }
    if jobs.is_empty() {
        return;
    }
    let blocks: usize = jobs.iter().map(RestoreJob::len).sum();
    host.log(
        "info",
        &format!(
            "Sculkstorm: restoring {} crater{} ({blocks} blocks) left over from a server stop that was not clean.",
            jobs.len(),
            if jobs.len() == 1 { "" } else { "s" },
        ),
    );
    let mut state = shared.state();
    for job in jobs {
        state.craters.queue_restore(job);
    }
    shared.set_watching(true);
}

/// The server is stopping: puts every crater back in one go, and takes the debris out. What cannot
/// be (a chunk that is not loaded) stays in its journal for the next start.
pub fn restore_all_now(host: &mut dyn Host, shared: &Shared) {
    let jobs = shared.state().craters.drain_all();
    let flying: Vec<Task> = std::mem::take(&mut shared.state().tasks);
    for task in flying {
        if let Task::Debris(d) = task {
            d.remove(host);
        }
    }
    // The journals of craters still being dug are on disk before they are read or deleted.
    JOURNAL.fence();
    for mut job in jobs {
        let mut restored = Vec::new();
        // Until it is all back, or a round gets nowhere (chunks that cannot be loaded now).
        let done = loop {
            let before = job.remaining();
            let mut budget = i32::MAX;
            let done = job.restore(host, shared, &mut budget, &mut restored);
            if done || job.remaining() == before {
                break done;
            }
        };
        job.release_chunks(host);
        if done && let Some(file) = job.journal() {
            JOURNAL.delete(file);
        }
    }
    JOURNAL.fence();
    shared.state().craters.forget_all();
    shared.set_watching(false);
}

/// Takes the debris an earlier run left in `dimension` out (the server stopped while they flew).
pub fn sweep_debris(host: &mut dyn Host, dimension: &str) {
    for (id, kind, _) in host.level_entities(dimension) {
        if kind != "minecraft:block_display" {
            continue;
        }
        let Some(entity) = host.entity(id) else {
            continue;
        };
        if host
            .entity_tags(entity)
            .is_some_and(|tags| tags.iter().any(|t| t == debris::TAG))
        {
            host.remove_entity(dimension, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Area = Area {
        min_x: 0,
        min_z: 0,
        max_x: 100,
        max_z: 100,
    };
    const OVERWORLD: &str = "minecraft:overworld";

    fn watching() -> Craters {
        let mut c = Craters::default();
        c.watch(7, OVERWORLD, AREA);
        c
    }

    #[test]
    fn a_column_is_watched_by_the_crater_whose_area_holds_it_in_that_dimension() {
        let c = watching();
        assert_eq!(c.watched_by(OVERWORLD, 50, 50), Some(7));
        assert_eq!(
            c.watched_by(OVERWORLD, 0, 100),
            Some(7),
            "the edges are inside"
        );
        assert_eq!(c.watched_by(OVERWORLD, 101, 50), None);
        assert_eq!(c.watched_by("minecraft:the_nether", 50, 50), None);
        assert!(c.is_active());
        let mut c = c;
        c.unwatch(7);
        assert!(!c.is_active());
    }

    #[test]
    fn a_block_a_player_breaks_that_was_the_craters_drops_nothing_once() {
        let mut c = Craters::default();
        c.add_placed(OVERWORLD, &[[10, 64, 10], [11, 64, 10]]);
        assert!(c.is_placed(OVERWORLD, [10, 64, 10]));
        assert!(c.broke(OVERWORLD, [10, 64, 10], 100));
        assert!(
            !c.is_placed(OVERWORLD, [10, 64, 10]),
            "it is no longer the crater's"
        );
        assert!(c.take_no_drops(OVERWORLD, [10, 64, 10]));
        assert!(!c.take_no_drops(OVERWORLD, [10, 64, 10]), "once");
        // A block that was never the crater's drops what it always does.
        assert!(!c.broke(OVERWORLD, [99, 64, 99], 100));
        assert!(!c.take_no_drops(OVERWORLD, [99, 64, 99]));
        // Another dimension's block at the same place is not the crater's.
        assert!(!c.broke("minecraft:the_nether", [11, 64, 10], 100));
        assert!(c.is_placed(OVERWORLD, [11, 64, 10]));
    }

    #[test]
    fn a_crater_dug_over_another_is_told_the_land_under_the_others_work() {
        let land = |c: &mut Craters, dimension, pos, owner, seen| {
            c.original_for(dimension, pos, owner, seen).to_string()
        };
        let mut c = Craters::default();
        // The first finds stone, and puts something there.
        assert_eq!(land(&mut c, OVERWORLD, [1, 64, 1], 1, "stone"), "stone");
        // The second finds the first's work and is told the land, and so is a third over that.
        assert_eq!(land(&mut c, OVERWORLD, [1, 64, 1], 2, "sculk"), "stone");
        assert_eq!(land(&mut c, OVERWORLD, [1, 64, 1], 3, "deepslate"), "stone");
        // A crater changing a block twice keeps what its own first change left.
        assert_eq!(land(&mut c, OVERWORLD, [2, 64, 2], 3, "dirt"), "dirt");
        assert_eq!(land(&mut c, OVERWORLD, [2, 64, 2], 3, "air"), "air");
        // Once the land is back at a block, whoever digs there next finds the land itself.
        c.forget_live(OVERWORLD, &[[1, 64, 1]]);
        assert_eq!(
            land(&mut c, OVERWORLD, [1, 64, 1], 4, "cobblestone"),
            "cobblestone"
        );
        // Another dimension's block at the same place is no crater's.
        assert_eq!(
            land(&mut c, "minecraft:the_nether", [2, 64, 2], 5, "netherrack"),
            "netherrack"
        );
        // A crater that is done lets go of what it still held.
        c.release_live(3);
        assert_eq!(land(&mut c, OVERWORLD, [2, 64, 2], 6, "gravel"), "gravel");
    }

    #[test]
    fn experience_from_a_broken_crater_block_is_told_by_place_and_tick() {
        let mut c = Craters::default();
        c.add_placed(OVERWORLD, &[[10, 64, 10]]);
        c.broke(OVERWORLD, [10, 64, 10], 100);
        assert!(c.is_crater_xp(OVERWORLD, [10, 64, 10], 100));
        assert!(
            c.is_crater_xp(OVERWORLD, [10, 64, 10], 100),
            "a block can drop several orbs"
        );
        assert!(c.is_crater_xp(OVERWORLD, [10, 64, 10], 101));
        assert!(
            !c.is_crater_xp(OVERWORLD, [10, 64, 10], 102),
            "the mark has gone"
        );
        assert!(
            !c.is_crater_xp(OVERWORLD, [10, 65, 10], 100),
            "next door is not the same block"
        );
    }

    #[test]
    fn only_changes_inside_a_watched_area_are_kept_to_look_at() {
        let mut c = watching();
        c.candidate(OVERWORLD, [50, 64, 50], BlockStateId(3));
        c.candidate(OVERWORLD, [500, 64, 500], BlockStateId(3));
        c.candidate("minecraft:the_nether", [50, 64, 50], BlockStateId(3));
        let kept = c.take_candidates(OVERWORLD);
        assert_eq!(kept.len(), 1);
        assert_eq!((kept[0].id, kept[0].pos), (7, [50, 64, 50]));
        assert!(c.take_candidates(OVERWORLD).is_empty(), "taken once");
    }

    #[test]
    fn the_sculk_the_plugin_keeps_from_spreading_is_the_sculk_family_but_not_the_catalyst() {
        for name in [
            "minecraft:sculk",
            "minecraft:sculk_vein",
            "minecraft:sculk_sensor",
            "minecraft:sculk_shrieker",
        ] {
            assert!(is_sculk_family(name), "{name}");
        }
        assert!(!is_sculk_family("minecraft:sculk_catalyst"));
        assert!(!is_sculk_family("minecraft:stone"));
    }

    #[test]
    fn foreign_changes_are_kept_for_their_crater_and_taken_with_it() {
        let mut c = Craters::default();
        let f = |x| Foreign {
            pos: [x, 64, 0],
            old: BlockStateId(1),
            new_name: "minecraft:sculk".into(),
        };
        c.add_foreign(7, vec![f(1), f(2)]);
        c.add_foreign(7, vec![f(3)]);
        c.add_foreign(8, vec![f(9)]);
        assert_eq!(c.foreign_count(7), 3);
        let taken = c.take_foreign(7);
        assert_eq!(
            taken.iter().map(|f| f.pos[0]).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(c.foreign_count(7), 0);
        assert_eq!(c.foreign_count(8), 1);
    }

    fn job(id: u64, dimension: &str, due: u64) -> RestoreJob {
        let mut changes = Changes::default();
        changes.push([1, 64, 1], "minecraft:air", "minecraft:stone");
        RestoreJob::new(id, dimension.to_owned(), changes, AREA, due, None)
    }

    #[test]
    fn a_crater_waits_out_its_delay_and_then_is_put_back_in_its_own_dimension() {
        let mut c = Craters::default();
        c.waiting.push(job(1, OVERWORLD, 100));
        c.waiting.push(job(2, "minecraft:the_nether", 200));
        c.promote_due(99);
        assert!(c.take_restore(OVERWORLD).is_none(), "not yet");
        c.promote_due(100);
        assert!(
            c.take_restore("minecraft:the_nether").is_none(),
            "the nether's is later"
        );
        assert_eq!(c.take_restore(OVERWORLD).map(|j| j.id), Some(1));
        c.promote_due(250);
        assert_eq!(
            c.take_restore("minecraft:the_nether").map(|j| j.id),
            Some(2)
        );
    }

    #[test]
    fn undo_puts_every_waiting_crater_back_now_and_counts_them() {
        let mut c = Craters::default();
        c.waiting.push(job(1, OVERWORLD, 10_000));
        c.waiting.push(job(2, OVERWORLD, 20_000));
        c.restoring.push_back(job(3, OVERWORLD, 0));
        assert_eq!(c.undo_all(50), 3);
        assert!(c.waiting.is_empty());
        assert_eq!(c.restoring.len(), 3);
        assert!(c.restoring.iter().all(|j| j.due_tick <= 50));
        assert_eq!(Craters::default().undo_all(0), 0, "nothing to undo");
    }

    #[test]
    fn a_finished_restore_is_forgotten_with_its_blocks_and_its_area() {
        let mut c = Craters::default();
        let restore = job(7, OVERWORLD, 0);
        c.watch(7, OVERWORLD, AREA);
        c.add_foreign(7, vec![]);
        c.finish_restore(&restore);
        assert!(!c.is_active());
        assert_eq!(c.foreign_count(7), 0);
    }

    #[test]
    fn a_journals_bounds_are_its_blocks_extremes() {
        let record = |x, z| journal::Record {
            pos: [x, 64, z],
            placed: "minecraft:air".into(),
            original: "minecraft:stone".into(),
        };
        let area = bounds(&[record(5, -3), record(-8, 20), record(2, 2)]);
        assert_eq!(
            area,
            Area {
                min_x: -8,
                min_z: -3,
                max_x: 5,
                max_z: 20
            }
        );
    }

    #[test]
    fn journals_get_names_that_do_not_repeat() {
        let a = journal_name(1);
        let b = journal_name(2);
        assert!(a.ends_with("-1.log") && b.ends_with("-2.log"));
        assert_ne!(a, b);
        assert_eq!(a.split('-').next().unwrap().len(), 13);
    }
}
