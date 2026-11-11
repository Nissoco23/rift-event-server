//! Things that go on tick by tick: the plugin's `BukkitRunnable`s.
//!
//! Each belongs to a dimension, and runs in that level's tick (`level_tick_pre`), where the game
//! lets a mod read and change that level's blocks. A task that has to start another (an impact
//! starts a field, a lightning storm and a crater) pushes it to `Cx::spawn`.

use crate::ascension::{Charge, Ring};
use crate::cataclysm::{Blast, Field, Flight, Lightning};
use crate::config::Config;
use crate::crater::debris::Debris;
use crate::fx::Fx;
use crate::rng::Rng;
use crate::state::Shared;
use horizon_client::mod_api::Host;

/// What a task is given for one tick.
pub struct Cx<'a> {
    pub host: &'a mut dyn Host,
    pub cfg: &'a Config,
    pub fx: Fx,
    /// The server tick being run.
    pub tick: u64,
    pub rng: &'a mut Rng,
    pub shared: &'a Shared,
    /// Tasks to start; they first run in the next tick of their dimension.
    pub spawn: &'a mut Vec<Task>,
}

pub enum Task {
    Charge(Charge),
    Ring(Ring),
    Flight(Flight),
    Blast(Blast),
    Lightning(Lightning),
    Field(Field),
    Debris(Debris),
}

impl Task {
    /// The dimension whose tick runs it.
    pub fn dimension(&self) -> &str {
        match self {
            Task::Charge(t) => &t.dimension,
            Task::Ring(t) => &t.dimension,
            Task::Flight(t) => &t.dimension,
            Task::Blast(t) => &t.dimension,
            Task::Lightning(t) => &t.dimension,
            Task::Field(t) => &t.dimension,
            Task::Debris(t) => &t.dimension,
        }
    }

    /// Runs one tick; false once the task is over.
    pub fn step(&mut self, cx: &mut Cx<'_>) -> bool {
        match self {
            Task::Charge(t) => t.step(cx),
            Task::Ring(t) => t.step(cx),
            Task::Flight(t) => t.step(cx),
            Task::Blast(t) => t.step(cx),
            Task::Lightning(t) => t.step(cx),
            Task::Field(t) => t.step(cx),
            Task::Debris(t) => t.step(cx),
        }
    }
}
