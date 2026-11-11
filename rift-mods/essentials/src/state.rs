//! The changes the commands queued, and making them at the tick.

use horizon_client::mod_api::{Host, Text};
use rift_chat::{escape, mini, plain};
use std::collections::VecDeque;

/// Who ran a command, for the reports that come after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    Console,
    Player { uuid: u128, name: String },
}

impl Actor {
    pub fn label(&self) -> &str {
        match self {
            Self::Console => "console",
            Self::Player { name, .. } => name,
        }
    }

    fn is(&self, uuid: u128) -> bool {
        matches!(self, Self::Player { uuid: me, .. } if *me == uuid)
    }
}

/// A game mode, with the number the engine names it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl Mode {
    pub fn id(self) -> i32 {
        match self {
            Self::Survival => 0,
            Self::Creative => 1,
            Self::Adventure => 2,
            Self::Spectator => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
            Self::Spectator => "spectator",
        }
    }
}

/// The creative flying speed of level 1 (vanilla's) and of level 10: past 0.45 a player sprint-flies
/// more than ten blocks a tick, which the server's moved-too-quickly check sends back.
const FLY_SPEED_LEVEL_1: f32 = 0.05;
const FLY_SPEED_LEVEL_10: f32 = 0.45;

/// The flying speed of a level, 1 to 10, in the engine's units.
pub fn fly_speed(level: u8) -> f32 {
    let steps = f32::from(level.clamp(1, 10) - 1);
    FLY_SPEED_LEVEL_1 + steps * (FLY_SPEED_LEVEL_10 - FLY_SPEED_LEVEL_1) / 9.0
}

/// A change for the next tick.
#[derive(Clone, Debug)]
pub enum Job {
    GameMode {
        uuid: u128,
        name: String,
        mode: Mode,
        actor: Actor,
    },
    FlySpeed {
        uuid: u128,
        name: String,
        level: u8,
        actor: Actor,
    },
}

pub struct State {
    queue: VecDeque<Job>,
    /// The state of the generator `@r` draws from.
    random: u64,
}

impl Default for State {
    fn default() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Self {
            queue: VecDeque::new(),
            random: nanos | 1,
        }
    }
}

/// `text` as markup in the accent colour, escaped.
pub fn accent(text: &str) -> String {
    format!("<accent>{}</accent>", escape(text))
}

/// A line of markup to whoever ran a command, now or later: a player is told in chat, the console
/// in the log.
fn report(host: &mut dyn Host, actor: &Actor, markup: &str) {
    match actor {
        Actor::Player { uuid, .. } => {
            if !host.tell(*uuid, mini(markup)) {
                host.log(
                    "INFO",
                    &format!("Essentials: (for {}) {}", actor.label(), plain(markup)),
                );
            }
        }
        Actor::Console => host.log("INFO", &plain(markup)),
    }
}

fn audit(host: &dyn Host, text: String) {
    host.log("INFO", &format!("Essentials: {text}"));
}

impl State {
    pub fn push(&mut self, job: Job) {
        self.queue.push_back(job);
    }

    /// A number below `below` (more than 0), for `@r`: xorshift64*, which is all a pick of one
    /// player needs.
    pub fn pick(&mut self, below: usize) -> usize {
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        let drawn = self.random.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33;
        (drawn % below as u64) as usize
    }

    /// The work of one server tick: every change the commands queued.
    pub fn tick(&mut self, host: &mut dyn Host) {
        while let Some(job) = self.queue.pop_front() {
            match job {
                Job::GameMode {
                    uuid,
                    name,
                    mode,
                    actor,
                } => game_mode(host, uuid, &name, mode, &actor),
                Job::FlySpeed {
                    uuid,
                    name,
                    level,
                    actor,
                } => flying(host, uuid, &name, level, &actor),
            }
        }
    }
}

fn game_mode(host: &mut dyn Host, uuid: u128, name: &str, mode: Mode, actor: &Actor) {
    if !host.set_game_mode(uuid, mode.id()) {
        // Someone may have made it so since the command, which is no failure.
        let now = host
            .server_players()
            .into_iter()
            .find(|p| p.uuid == uuid)
            .map(|p| p.game_mode);
        if now != Some(mode.id()) {
            report(
                host,
                actor,
                &format!(
                    "<red>{}'s game mode was not changed: they left, or a mod kept it</red>",
                    accent(name)
                ),
            );
        }
        return;
    }
    audit(
        host,
        format!(
            "{} set {name}'s game mode to {}",
            actor.label(),
            mode.name()
        ),
    );
    // As `/gamemode` tells a player whose mode another changed, in vanilla's own words.
    if !actor.is(uuid) {
        let mode_name = Text::translate(&format!("gameMode.{}", mode.name()), &[]);
        host.tell(
            uuid,
            Text::translate("gameMode.changed", std::slice::from_ref(&mode_name)),
        );
    }
}

fn flying(host: &mut dyn Host, uuid: u128, name: &str, level: u8, actor: &Actor) {
    if !host.set_flying_speed(uuid, Some(fly_speed(level))) {
        report(
            host,
            actor,
            &format!(
                "<red>{}'s flight speed was not changed: they left</red>",
                accent(name)
            ),
        );
        return;
    }
    audit(
        host,
        format!("{} set {name}'s flight speed to {level}", actor.label()),
    );
}
