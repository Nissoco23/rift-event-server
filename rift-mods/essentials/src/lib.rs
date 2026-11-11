//! Operator shortcuts for Rift Event Server: `/gmc`, `/gms`, `/gmsp` and `/gma` change a player's
//! game mode (creative, survival, spectator, adventure) and `/flyspeed` their creative flying speed,
//! each in a command shorter than vanilla's `/gamemode`.
//!
//! A command checks what it can and answers at once, and only queues the change: the engine's
//! helpers for another player's game mode and flying speed work where no player's state is swapped
//! in, and a command runs inside the player's own turn. The tick makes the change (`state.rs`).
//!
//! The engine runs mods as Rust code linked into its executable, so this crate is a library and
//! `crates/rift-engine` is the engine's `main` with it handed over (docs/rift-mods.md).
#![forbid(unsafe_code)]

mod command;
mod state;
#[cfg(test)]
mod testing;

use horizon_client::mod_api::{
    Flow, Hooks, Host, PerformCommandEvent, PerformConsoleCommandEvent, RegisterCommandsEvent,
    ServerTickEvent, ServerTickPhase,
};
use state::{Actor, State};
use std::sync::{Mutex, MutexGuard};

/// The name the engine's executable gives the mod (`horizon_client::mod_api::name_mods`), which a
/// crash report and a log line name its hooks by.
pub const ID: &str = "essentials";

/// The mod as the engine's executable takes it: the `Hooks` to hand to
/// `horizon_client::entry::main_with`.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(Essentials::default())
}

/// The mod: the changes queued for the next tick.
#[derive(Default)]
pub struct Essentials {
    state: Mutex<State>,
}

impl Essentials {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn flow(ran: bool) -> Flow {
    if ran { Flow::Cancel } else { Flow::Continue }
}

impl Hooks for Essentials {
    /// At the end of a tick of a running world: the changes the commands queued.
    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        let ServerTickPhase::ServerTickEnd {
            world_running: true,
        } = event.phase
        else {
            return;
        };
        self.state().tick(host);
    }

    fn register_commands(&self, _host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        command::register(event.commands);
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        let actor = Actor::Player {
            uuid: event.player.uuid,
            name: event.player.name.clone(),
        };
        flow(command::run(&mut self.state(), host, &actor, event.command))
    }

    fn perform_console_command(
        &self,
        host: &mut dyn Host,
        event: &mut PerformConsoleCommandEvent<'_>,
    ) -> Flow {
        flow(command::run(
            &mut self.state(),
            host,
            &Actor::Console,
            event.command,
        ))
    }
}
