//! View distance limits for Rift Event Server: a player is sent the chunks within one distance, an
//! operator within another, so an event with many players loads and sends what the server can carry
//! while its operators still look as far as they like.
//!
//! The engine already caps one player's view distance (`Host::set_player_view_distance`, 2 to 32
//! chunks, under the server's own and what the client asks for). This mod decides each player's cap
//! from their operator level, once a second and when they join, and sets it only when it changed. The
//! far ring (`max_view_distance`, docs/hosting.md) honours the same cap, so a player held to the
//! server's own distance gets no saved chunks past it.
//!
//! The engine runs mods as Rust code linked into its executable, so this crate is a library and
//! `crates/rift-engine` is the engine's `main` with it handed over (docs/rift-mods.md).
#![forbid(unsafe_code)]

mod command;
mod config;
#[cfg(test)]
mod testing;

use config::Config;
use horizon_client::mod_api::{
    Flow, Hooks, Host, NativeServerPlayer, PerformCommandEvent, PerformConsoleCommandEvent,
    PlayerLoggedInEvent, PlayerLoggedOutEvent, RegisterCommandsEvent, ServerTickEvent,
    ServerTickPhase,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The name the engine's executable gives the mod (`horizon_client::mod_api::name_mods`), which a
/// crash report and a log line name its hooks by.
pub const ID: &str = "viewdistance";

/// The mod as the engine's executable takes it: the `Hooks` to hand to
/// `horizon_client::entry::main_with`.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(ViewDistance::default())
}

/// A server tick is 50 ms, so this is once a second.
const TICKS_PER_SECOND: u64 = 20;

/// The mod: every player's limit follows their operator level.
#[derive(Default)]
pub struct ViewDistance {
    state: Mutex<State>,
    config_loaded: AtomicBool,
}

#[derive(Default)]
struct State {
    config: Config,
    /// The limit the engine was last told for each player, so it is told again only when it
    /// changes. A player's limit lasts until they leave, so a leaving player is forgotten.
    applied: HashMap<u128, i32>,
}

impl ViewDistance {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn config(&self) -> Config {
        self.state().config.clone()
    }

    /// The limit the engine was told for `player`, if it was.
    pub(crate) fn applied(&self, player: u128) -> Option<i32> {
        self.state().applied.get(&player).copied()
    }

    /// Reads `config/viewdistance.toml` (writing it when the server has none) and takes it up. An
    /// error leaves the settings as they were.
    pub(crate) fn reload_config(&self, host: &dyn Host) -> Result<(), String> {
        let loaded = config::load(&host.config_dir()?)?;
        self.state().config = loaded;
        Ok(())
    }

    /// Reads the config the first time a hook needs it.
    fn ensure_config(&self, host: &dyn Host) {
        if self.config_loaded.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Err(e) = self.reload_config(host) {
            host.log(
                "WARN",
                &format!("View distance: could not read the config, using the defaults: {e}"),
            );
        }
    }

    /// Gives each of `players` the limit their operator level calls for, telling the engine only
    /// of what changed. With `everyone`, `players` is every player on the server, so the ones that
    /// are not among them have left. A refused call (the engine cannot reach the player from the
    /// hook it is in) is tried again at the next one.
    pub(crate) fn reconcile(
        &self,
        host: &mut dyn Host,
        players: &[NativeServerPlayer],
        everyone: bool,
    ) {
        let (release, changes) = {
            let mut state = self.state();
            if everyone {
                state
                    .applied
                    .retain(|uuid, _| players.iter().any(|p| p.uuid == *uuid));
            }
            if state.config.enabled {
                // A fake player (a mod's bot) has no client to limit.
                let changes: Vec<(u128, i32)> = players
                    .iter()
                    .filter(|p| !p.fake)
                    .map(|p| (p.uuid, state.config.limit_for(p.permission)))
                    .filter(|(uuid, limit)| state.applied.get(uuid) != Some(limit))
                    .collect();
                (Vec::new(), changes)
            } else {
                // Turned off: every limit given is taken back, once.
                let release: Vec<u128> = state.applied.drain().map(|(uuid, _)| uuid).collect();
                (release, Vec::new())
            }
        };
        for uuid in release {
            host.set_player_view_distance(uuid, None);
        }
        for (uuid, limit) in changes {
            if host.set_player_view_distance(uuid, Some(limit)) {
                self.state().applied.insert(uuid, limit);
            }
        }
    }
}

impl Hooks for ViewDistance {
    /// Once a second, at the end of a tick of a running world: every player's limit.
    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        let ServerTickPhase::ServerTickEnd {
            world_running: true,
        } = event.phase
        else {
            return;
        };
        if !event.context.tick.get().is_multiple_of(TICKS_PER_SECOND) {
            return;
        }
        self.ensure_config(&*host);
        let players = host.server_players();
        self.reconcile(host, &players, true);
    }

    /// A joining player is limited at once, not up to a second later.
    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        self.ensure_config(&*host);
        let joined: Vec<NativeServerPlayer> = host
            .server_players()
            .into_iter()
            .filter(|p| p.uuid == event.player)
            .collect();
        self.reconcile(host, &joined, false);
    }

    fn player_logged_out(&self, _host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        self.state().applied.remove(&event.player);
    }

    fn register_commands(&self, _host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        command::register(event.commands);
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        self.ensure_config(&*host);
        if command::run(self, host, event.command) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn perform_console_command(
        &self,
        host: &mut dyn Host,
        event: &mut PerformConsoleCommandEvent<'_>,
    ) -> Flow {
        self.ensure_config(&*host);
        if command::run(self, host, event.command) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }
}
