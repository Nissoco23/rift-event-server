//! Death cycling: a player who dies comes straight back somewhere else, under a new name, so a scene
//! with few players looks as if it had more.
//!
//! A respawn spot is found while the player lives, because finding one takes a few ticks (its chunk
//! must load and its surface be read) and a death should not wait for it. Each tick of the overworld
//! the mod gives every player who may be cycled a standby spot: a random point at the set distance,
//! its chunk asked for (and held loaded) and read for the surface, where a player can stand with no
//! block above and nothing to burn or drown in. A spot is found again when a setting changed, or when a
//! player has moved far from where it was found (`deathpoint` mode). A death takes the standby spot, or
//! waits for one, and the player is respawned there at once, before any click, and given a random
//! nickname (`/nick random`) once they are back.

use crate::Hardcore;
use crate::state::{CycleMode, State};
use horizon_client::mod_api::{Host, NativeChunkLoad, NativeHeightmap, NativeServerPlayer};

pub const OVERWORLD: &str = "minecraft:overworld";
/// How far a player may move before a `deathpoint` spot found for where they were is found again.
pub const MOVED: f64 = 16.0;
/// Spots tried in a row before the mod rests, and the ticks it rests for.
const TRIES: u32 = 10;
const REST: u64 = 100;
/// Ticks before a player who was asked to respawn and has not is asked again.
const ASK_AGAIN: u64 = 100;

/// Who holds a player's chunk: each player's own, so one letting go does not free another's.
pub fn owner(uuid: u128) -> String {
    format!("hardcore:{uuid:032x}")
}

/// A spot waiting for its player.
#[derive(Clone, Debug, PartialEq)]
pub struct Ready {
    /// Where they stand: on the block that tops the column.
    pub pos: [f64; 3],
    pub chunk: [i32; 2],
    /// Where the player was, for `deathpoint` mode, when this was found.
    pub origin: [f64; 2],
    pub revision: u64,
}

/// A spot whose chunk is loading.
#[derive(Clone, Debug, PartialEq)]
pub struct Loading {
    pub chunk: [i32; 2],
    pub x: i32,
    pub z: i32,
    pub origin: [f64; 2],
    pub revision: u64,
    /// The chunk has been asked for.
    pub asked: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Standby {
    pub ready: Option<Ready>,
    pub loading: Option<Loading>,
    /// Spots that failed in a row, and when to try again.
    pub failures: u32,
    pub retry_at: u64,
}

/// A player who died and is being respawned at their spot.
#[derive(Clone, Debug, PartialEq)]
pub struct Going {
    pub pos: [f64; 3],
    pub chunk: [i32; 2],
    /// The tick the engine was last asked to respawn them.
    pub asked: Option<u64>,
}

/// A random point for a player at `origin`: at the set distance from them in a random direction
/// (`deathpoint`), or within the distance of the spawn point (`spawnpoint`: `world_spawn` where none
/// was set). A block, as its column.
pub fn pick(state: &mut State, origin: [f64; 2], world_spawn: Option<[i32; 3]>) -> (i32, i32) {
    let angle = state.rng.unit() * std::f64::consts::TAU;
    let distance = f64::from(state.cycle.distance);
    let (centre, reach) = match state.cycle.mode {
        CycleMode::DeathPoint => (origin, distance),
        CycleMode::SpawnPoint => {
            let spawn = state.cycle.spawn.or(world_spawn).unwrap_or([0, 64, 0]);
            (
                [f64::from(spawn[0]) + 0.5, f64::from(spawn[2]) + 0.5],
                distance * state.rng.unit().sqrt(),
            )
        }
    };
    (
        (centre[0] + reach * angle.cos()).floor() as i32,
        (centre[1] + reach * angle.sin()).floor() as i32,
    )
}

fn is_air(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// A block a player should not be put on.
fn is_hazard(name: &str) -> bool {
    [
        "water",
        "lava",
        "cactus",
        "magma",
        "fire",
        "powder_snow",
        "bubble",
    ]
    .iter()
    .any(|bad| name.contains(bad))
}

/// Whether a player can stand on the block that tops column (`x`, `z`), and the height of their
/// feet. Reads the overworld, which must be the level being ticked.
fn standing(host: &dyn Host, x: i32, z: i32) -> Option<f64> {
    let read = host
        .read_columns(
            OVERWORLD,
            &[(x, z)],
            NativeHeightmap::MotionBlocking,
            (2, 2),
        )
        .ok()?;
    let base = read.bases.first().copied().flatten()?;
    // The blocks run from two under the height up: the one under it is the top block.
    let top = read
        .blocks
        .get(1)
        .and_then(|i| read.palette.get(usize::from(*i)))?;
    (!is_air(top) && !is_hazard(top)).then(|| f64::from(base))
}

/// What the host was asked about one player's spot.
enum Job {
    /// Look at a spot whose chunk loads, and free `replaced`, the spot it takes the place of.
    Check {
        uuid: u128,
        loading: Loading,
        replaced: Option<[i32; 2]>,
    },
    /// Let go of a chunk nobody wants.
    Release(u128, [i32; 2]),
}

enum Found {
    Waiting,
    Failed,
    Ready(Ready),
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl Hardcore {
    /// One tick of the overworld: spots found, held and used. `players` are the server's players.
    pub(crate) fn cycle_tick(&self, host: &mut dyn Host, players: &[NativeServerPlayer]) {
        let world_spawn = host.world_spawn();
        let mut jobs: Vec<Job> = Vec::new();
        {
            let mut state = self.state();
            state.tick += 1;
            plan(&mut state, players, world_spawn, &mut jobs);
        }

        // The host's answers, with the lock released.
        let mut found: Vec<(u128, Found)> = Vec::new();
        for job in jobs {
            match job {
                Job::Release(uuid, chunk) => {
                    host.chunk_load_release(&owner(uuid), OVERWORLD, chunk);
                }
                Job::Check {
                    uuid,
                    loading,
                    replaced,
                } => {
                    let who = owner(uuid);
                    if !loading.asked {
                        host.chunk_load_request(&who, OVERWORLD, loading.chunk);
                    }
                    let answer = match host.chunk_load_state(&who, OVERWORLD, loading.chunk) {
                        NativeChunkLoad::Pending => Found::Waiting,
                        NativeChunkLoad::Failed => Found::Failed,
                        NativeChunkLoad::Loaded => match standing(&*host, loading.x, loading.z) {
                            Some(y) => {
                                if let Some(old) = replaced {
                                    host.chunk_load_release(&who, OVERWORLD, old);
                                }
                                Found::Ready(Ready {
                                    pos: [
                                        f64::from(loading.x) + 0.5,
                                        y,
                                        f64::from(loading.z) + 0.5,
                                    ],
                                    chunk: loading.chunk,
                                    origin: loading.origin,
                                    revision: loading.revision,
                                })
                            }
                            None => {
                                host.chunk_load_release(&who, OVERWORLD, loading.chunk);
                                Found::Failed
                            }
                        },
                    };
                    found.push((uuid, answer));
                }
            }
        }

        let to_respawn = {
            let mut state = self.state();
            apply(&mut state, found);
            respawns(&mut state)
        };
        for uuid in to_respawn {
            host.respawn_player(uuid);
        }
        self.rename_respawned(host, players);
    }

    /// Gives each player who respawned a random nickname, as `/nick random` does, whatever
    /// `/nickonjoin` says.
    fn rename_respawned(&self, host: &mut dyn Host, players: &[NativeServerPlayer]) {
        let renames: Vec<(u128, String)> = std::mem::take(&mut self.state().rename);
        for (uuid, name) in renames {
            if players.iter().any(|p| p.uuid == uuid) {
                host.run_mod_command(&format!("nick random {name}"));
            }
        }
    }
}

/// What this tick asks of the host, and the state it leaves: a spot to find for each player who may
/// be cycled, those that loaded to look at, and the chunks of players who may not be to let go.
fn plan(
    state: &mut State,
    players: &[NativeServerPlayer],
    world_spawn: Option<[i32; 3]>,
    jobs: &mut Vec<Job>,
) {
    let (tick, revision) = (state.tick, state.revision);
    // Who needs a spot: the living who may be cycled, and the dead who wait for one.
    let mut wanted: Vec<(u128, [f64; 2])> = Vec::new();
    if state.cycle.enabled {
        for p in players {
            let waiting = state.waiting.contains(&p.uuid);
            if p.fake || state.exempt.contains(&p.uuid) || (p.health <= 0.0 && !waiting) {
                continue;
            }
            wanted.push((p.uuid, [p.position[0], p.position[2]]));
        }
    }
    // The rest let go of what they held.
    let gone: Vec<u128> = state
        .standby
        .keys()
        .filter(|u| !wanted.iter().any(|(w, _)| w == *u))
        .copied()
        .collect();
    for uuid in gone {
        if let Some(standby) = state.standby.remove(&uuid) {
            for chunk in standby
                .ready
                .iter()
                .map(|r| r.chunk)
                .chain(standby.loading.iter().map(|l| l.chunk))
            {
                jobs.push(Job::Release(uuid, chunk));
            }
        }
    }
    if !state.cycle.enabled {
        state.waiting.clear();
    }

    let mode = state.cycle.mode;
    for (uuid, _) in &wanted {
        let standby = state.standby.entry(*uuid).or_default();
        if standby.failures >= TRIES && standby.retry_at <= tick {
            standby.failures = 0;
        }
    }
    // Choosing a point needs the state's random source, so the ones to start are chosen first.
    let starts: Vec<(u128, [f64; 2])> = wanted
        .iter()
        .filter(|(uuid, origin)| {
            let standby = &state.standby[uuid];
            let stale = standby.ready.as_ref().is_some_and(|r| {
                r.revision != revision
                    || (mode == CycleMode::DeathPoint && distance(r.origin, *origin) > MOVED)
            });
            standby.loading.is_none()
                && (standby.ready.is_none() || stale)
                && standby.retry_at <= tick
        })
        .copied()
        .collect();
    for (uuid, origin) in starts {
        let (x, z) = pick(state, origin, world_spawn);
        if let Some(standby) = state.standby.get_mut(&uuid) {
            standby.loading = Some(Loading {
                chunk: [x >> 4, z >> 4],
                x,
                z,
                origin,
                revision,
                asked: false,
            });
        }
    }
    for (uuid, standby) in &mut state.standby {
        if let Some(loading) = &mut standby.loading {
            jobs.push(Job::Check {
                uuid: *uuid,
                loading: loading.clone(),
                replaced: standby.ready.as_ref().map(|r| r.chunk),
            });
            loading.asked = true;
        }
    }
}

/// Takes what the host answered: a spot found stays for its player, one that failed is tried again.
fn apply(state: &mut State, found: Vec<(u128, Found)>) {
    let tick = state.tick;
    for (uuid, answer) in found {
        let Some(standby) = state.standby.get_mut(&uuid) else {
            continue;
        };
        match answer {
            Found::Waiting => {}
            Found::Failed => {
                standby.loading = None;
                standby.failures += 1;
                if standby.failures >= TRIES {
                    standby.retry_at = tick + REST;
                }
            }
            Found::Ready(ready) => {
                standby.loading = None;
                standby.failures = 0;
                standby.ready = Some(ready);
            }
        }
    }
}

/// The players waiting for a spot who have one now are on their way; the players on their way who
/// have not been asked to respawn (or not lately) are the ones to ask.
fn respawns(state: &mut State) -> Vec<u128> {
    let tick = state.tick;
    let (revision, mode) = (state.revision, state.cycle.mode);
    // A spot is used when it is current: found for these settings and, in `deathpoint` mode, for
    // where the player died (or near it).
    let ready: Vec<u128> = state
        .waiting
        .iter()
        .filter(|u| {
            let died = state.member(**u).map(|p| [p.position[0], p.position[2]]);
            state
                .standby
                .get(u)
                .and_then(|s| s.ready.as_ref())
                .is_some_and(|r| {
                    r.revision == revision
                        && (mode != CycleMode::DeathPoint
                            || died.is_none_or(|died| distance(r.origin, died) <= MOVED))
                })
        })
        .copied()
        .collect();
    for uuid in ready {
        state.waiting.remove(&uuid);
        if let Some(ready) = state.standby.get_mut(&uuid).and_then(|s| s.ready.take()) {
            state.going.insert(
                uuid,
                Going {
                    pos: ready.pos,
                    chunk: ready.chunk,
                    asked: None,
                },
            );
        }
    }
    let mut ask = Vec::new();
    for (uuid, going) in &mut state.going {
        if going.asked.is_none_or(|at| at + ASK_AGAIN <= tick) {
            going.asked = Some(tick);
            ask.push(*uuid);
        }
    }
    ask
}
