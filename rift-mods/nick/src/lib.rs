//! Operator nicknames for Rift Event Server: an operator gives a player (or themselves) another
//! name and another skin, in chat, death and leave lines, the tab list and over their head, so a
//! scripted event can hide who is who. A random nickname is drawn from a pool of ordinary accounts,
//! never the same name twice in a round. Nothing about a nickname is saved: it ends when the player
//! leaves.
//!
//! The engine does the naming (`Host::set_nickname`, `set_name_tag`) and the skin
//! (`set_profile_property`, `refresh_player`); this mod decides who gets what, looks the skins up
//! on Mojang's services in the background, and keeps the pool. A command only queues its work: the
//! tick does it, where those helpers are in reach (`state.rs`).
//!
//! The engine runs mods as Rust code linked into its executable, so this crate is a library and
//! `crates/rift-engine` is the engine's `main` with it handed over (docs/rift-mods.md).
#![forbid(unsafe_code)]

mod command;
mod config;
mod names;
mod pool;
mod rng;
mod roster;
mod skins;
mod state;
#[cfg(test)]
mod testing;

use horizon_client::mod_api::{
    Flow, Hooks, Host, PerformCommandEvent, PerformConsoleCommandEvent, PlayerLoggedInEvent,
    PlayerLoggedOutEvent, RegisterCommandsEvent, ServerTickEvent, ServerTickPhase,
    TabCompleteV2Event,
};
use rng::Rng;
use skins::Lookup;
use state::{Actor, State};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// The name the engine's executable gives the mod (`horizon_client::mod_api::name_mods`), which a
/// crash report and a log line name its hooks by.
pub const ID: &str = "nick";

/// The mod as the engine's executable takes it: the `Hooks` to hand to
/// `horizon_client::entry::main_with`.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(Nick::default())
}

/// The mod: the nicknames in force and the work for the next tick.
pub struct Nick {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    loaded: AtomicBool,
}

impl Default for Nick {
    fn default() -> Self {
        Self::with(Arc::new(skins::Mojang), true, Rng::from_clock())
    }
}

impl Nick {
    /// The mod over a source of skins, with lookups on worker threads or not, and a source of
    /// random numbers.
    pub(crate) fn with(lookup: Arc<dyn Lookup>, background: bool, rng: Rng) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::new(lookup, background, rng)),
                loaded: AtomicBool::new(false),
            }),
        }
    }

    /// The mod's state, for a test that has boxed the mod and still wants to look inside it.
    #[cfg(test)]
    pub(crate) fn handle(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the config, the pool and the pinned skins the first time a hook needs them.
    fn ensure_config(&self, host: &mut dyn Host) {
        if self.shared.loaded.swap(true, Ordering::AcqRel) {
            return;
        }
        match self.state().reload(host) {
            Ok(notes) => {
                for note in notes {
                    host.log("WARN", &format!("Nick: {note}"));
                }
            }
            Err(e) => host.log(
                "WARN",
                &format!("Nick: could not read the files, using the defaults: {e}"),
            ),
        }
    }
}

fn flow(ran: bool) -> Flow {
    if ran { Flow::Cancel } else { Flow::Continue }
}

impl Hooks for Nick {
    /// At the end of a tick of a running world: the queued work and the answers that came.
    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        let ServerTickPhase::ServerTickEnd {
            world_running: true,
        } = event.phase
        else {
            return;
        };
        self.ensure_config(host);
        self.state().tick(host);
    }

    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        self.ensure_config(host);
        self.state().joined(host, event.player, event.name);
    }

    /// After their leave line, which the engine wrote with their nickname and then took it off.
    fn player_logged_out(&self, _host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        self.state().left(event.player);
    }

    fn register_commands(&self, host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        self.ensure_config(host);
        let level = self.state().config.command_level;
        command::register(event.commands, level);
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        self.ensure_config(host);
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
        self.ensure_config(host);
        flow(command::run(
            &mut self.state(),
            host,
            &Actor::Console,
            event.command,
        ))
    }

    /// The nicknames in force for `/nick info`, and the pool's names for `/nick pool remove`: the
    /// server's players are out of reach here, so the mod answers from its own.
    fn tab_complete_v2(&self, _host: &mut dyn Host, event: &mut TabCompleteV2Event<'_>) {
        if !event.label.eq_ignore_ascii_case("nick") {
            return;
        }
        let Some((typing, before)) = event.arguments.split_last() else {
            return;
        };
        let before: Vec<&str> = before.iter().map(String::as_str).collect();
        let found = command::completions(
            &self.state(),
            &before,
            typing,
            (event.range[0], event.range[1]),
        );
        event.suggestions.extend(found);
    }
}
