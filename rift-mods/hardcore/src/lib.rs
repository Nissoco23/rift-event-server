//! Hardcore rules for Rift Event Server, whose lore is a hardcore server: death protection that
//! holds an operator's health above a floor, what happens to a player who dies (kicked, spectating
//! or nothing), and what the others are told of it (a death message, a fake "left the game" line,
//! or nothing) within a distance.
//!
//! The engine runs mods as Rust code linked into its executable, so this crate is a library
//! (`Hardcore`, a `Hooks`) and `crates/rift-engine` is the engine's `main` with it, and every other
//! mod in `rift-mods/`, handed over (docs/rift-mods.md). The engine gives the mod what it cannot do
//! alone: the health and the totem of a hit (`living_damage_pre_v2`), the line a death tells
//! (`player_death_message`) and a switch for the real join and leave lines
//! (`Host::set_membership_lines`).
#![forbid(unsafe_code)]

mod command;
mod rng;
mod state;
mod store;
#[cfg(test)]
mod testing;

use command::Actor;
use horizon_client::mod_api::{
    Flow, Hooks, Host, LevelTickPostEvent, LivingDamagePreV2Event, PerformCommandEvent,
    PerformConsoleCommandEvent, PlayerDeathMessageEvent, PlayerDiedV2Event, PlayerLoggedOutEvent,
    PlayerRespawnPositionEvent, PlayerRespawnedEvent, RegisterCommandsEvent, TabCompleteV2Event,
    Text,
};
use rng::Rng;
use state::{DeathMsg, OnDeath, Spot, State};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

/// The name the engine's executable gives the mod (`horizon_client::mod_api::name_mods`), which a
/// crash report and a log line name its hooks by.
pub const ID: &str = "hardcore";

/// The mod as the engine's executable takes it: the `Hooks` to hand to
/// `horizon_client::entry::main_with`.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(Hardcore::default())
}

/// What a hit of these types may kill through death protection: `/kill`, and the void, which
/// would otherwise leave a protected player falling for ever.
const NOT_PROTECTED: [&str; 2] = ["minecraft:generic_kill", "minecraft:out_of_world"];

/// The mod: the rules, and each player's protection.
pub struct Hardcore {
    state: Mutex<State>,
    loaded: AtomicBool,
}

impl Default for Hardcore {
    fn default() -> Self {
        Self::with(Rng::from_clock())
    }
}

impl Hardcore {
    pub(crate) fn with(rng: Rng) -> Self {
        Self {
            state: Mutex::new(State::new(rng)),
            loaded: AtomicBool::new(false),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads `config/hardcore.toml` the first time a hook needs it.
    fn ensure_loaded(&self, host: &dyn Host) {
        if self.loaded.swap(true, Ordering::AcqRel) {
            return;
        }
        let loaded = host.config_dir().and_then(|dir| store::load(&dir));
        match loaded {
            Ok(saved) => self.state().restore(saved),
            Err(e) => host.log(
                "WARN",
                &format!("Hardcore: could not read the saved rules, using the defaults: {e}"),
            ),
        }
    }

    /// Writes the rules and each player's protection.
    fn save(&self, host: &dyn Host) {
        let saved = self.state().saved();
        if let Err(e) = host.config_dir().and_then(|dir| store::save(&dir, &saved)) {
            host.log("WARN", &format!("Hardcore: could not save the rules: {e}"));
        }
    }

    /// Tells the engine whether to send the real join and leave lines: not while a death shows
    /// a leave line in their place. Tried again at each tick until the engine takes it.
    fn apply_lines(&self, host: &mut dyn Host) {
        let show = {
            let state = self.state();
            let show = state.deathmsg != DeathMsg::Leave;
            if state.lines_applied == Some(show) {
                return;
            }
            show
        };
        if host.set_membership_lines(show) {
            self.state().lines_applied = Some(show);
        }
    }

    /// Runs `line` for `actor` if it is one of ours.
    fn handle(&self, host: &mut dyn Host, actor: &Actor, line: &str) -> Flow {
        if command::parse(line).is_none() {
            return Flow::Continue;
        }
        self.ensure_loaded(&*host);
        let players = host.server_players();
        let outcome = {
            let mut state = self.state();
            state.roster = players;
            command::run(&mut state, actor, line)
        };
        let Some(outcome) = outcome else {
            return Flow::Continue;
        };
        for reply in outcome.replies {
            host.feedback(reply);
        }
        if outcome.save {
            self.save(&*host);
        }
        if outcome.lines {
            self.apply_lines(host);
        }
        Flow::Cancel
    }
}

/// What a hit of `amount` may take off a player with `health`, held above `level` health points.
fn protected(amount: f32, health: f32, level: u8) -> f32 {
    let room = (health - f32::from(level)).max(0.0);
    if amount > room { room } else { amount }
}

impl Hooks for Hardcore {
    fn register_commands(&self, host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        self.ensure_loaded(&*host);
        command::register(event.commands);
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        let actor = Actor::Player {
            uuid: event.player.uuid,
            name: event.player.name.clone(),
        };
        self.handle(host, &actor, event.command)
    }

    fn perform_console_command(
        &self,
        host: &mut dyn Host,
        event: &mut PerformConsoleCommandEvent<'_>,
    ) -> Flow {
        self.handle(host, &Actor::Console, event.command)
    }

    fn tab_complete_v2(&self, _host: &mut dyn Host, event: &mut TabCompleteV2Event<'_>) {
        let found = command::completions(
            &self.state(),
            event.label,
            event.arguments,
            (event.range[0], event.range[1]),
        );
        if found.is_empty() {
            return;
        }
        // The engine offers the real names; these show the nicknames in their place.
        event.suggestions.clear();
        event.suggestions.extend(found);
    }

    /// Each tick of each level: who is on the server, for the hooks that cannot see them, and the
    /// join and leave lines the engine has been told.
    fn level_tick_post(&self, host: &mut dyn Host, _event: &mut LevelTickPostEvent<'_>) {
        self.ensure_loaded(&*host);
        let players = host.server_players();
        self.state().roster = players;
        self.apply_lines(host);
    }

    /// A hit on a player with protection that is on takes no more off their health than leaves
    /// them at their level. Nothing is held when a totem would save them (the hit that would kill
    /// them pops it, as in vanilla), and `/kill` and the void always kill.
    fn living_damage_pre_v2(&self, event: &mut LivingDamagePreV2Event<'_>) {
        if event.kind != "minecraft:player"
            || event.totem_ready
            || NOT_PROTECTED.contains(&event.damage_type)
        {
            return;
        }
        let Some(uuid) = event.victim_uuid else {
            return;
        };
        let level = match self.state().protection.get(&uuid) {
            Some(p) if p.enabled => p.level,
            _ => return,
        };
        event.amount = protected(event.amount, event.health, level);
    }

    /// A player dies: they are kicked with their death message, or marked to spectate once they
    /// respawn, or left alone, as `/ondeath` says for players or for operators.
    fn player_died_v2(&self, host: &mut dyn Host, event: &mut PlayerDiedV2Event<'_>) {
        let (mode, spot) = {
            let state = self.state();
            let member = state.member(event.player);
            if member.is_some_and(|p| p.fake) {
                return;
            }
            let admin = member.is_some_and(|p| p.permission >= command::OP_LEVEL);
            let mode = if admin {
                state.on_death_admin
            } else {
                state.on_death
            };
            let spot = member.map(|p| Spot {
                dimension: p.dimension.clone(),
                position: p.position,
            });
            (mode, spot)
        };
        match mode {
            OnDeath::Kick => {
                host.kick(event.player, event.message);
            }
            OnDeath::Spectate => {
                // Not yet seen by a tick: they spectate where they respawn.
                let spot = spot.unwrap_or(Spot {
                    dimension: String::new(),
                    position: [0.0; 3],
                });
                self.state().spectate.insert(event.player, spot);
            }
            OnDeath::None => {}
        }
    }

    /// What a death tells the others: the death message, a "left the game" line in its place, or
    /// nothing, within the distance `/deathmsgradius` says.
    fn player_death_message(&self, _host: &mut dyn Host, event: &mut PlayerDeathMessageEvent<'_>) {
        let state = self.state();
        match state.deathmsg {
            DeathMsg::None => *event.line = None,
            DeathMsg::Death => {}
            DeathMsg::Leave => {
                let name = state
                    .member(event.player)
                    .map_or_else(|| event.name.to_owned(), |p| command::shown(p).to_owned());
                *event.line = Some(
                    Text::translate("multiplayer.player.left", &[Text::literal(name)])
                        .with_color(horizon_client::text::YELLOW),
                );
            }
        }
        *event.radius = (state.radius >= 0).then(|| f64::from(state.radius));
    }

    /// A player who is to spectate respawns where they died.
    fn player_respawn_position(
        &self,
        _host: &mut dyn Host,
        event: &mut PlayerRespawnPositionEvent<'_>,
    ) {
        let state = self.state();
        let Some(spot) = state.spectate.get(&event.player) else {
            return;
        };
        if spot.dimension.is_empty() {
            return;
        }
        *event.dimension = spot.dimension.clone();
        *event.pos = spot.position;
    }

    fn player_respawned(&self, host: &mut dyn Host, event: &mut PlayerRespawnedEvent<'_>) {
        let spectate = self.state().spectate.remove(&event.player).is_some();
        if spectate {
            host.set_game_mode(event.player, 3);
        }
    }

    fn player_logged_out(&self, _host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        self.state().spectate.remove(&event.player);
    }
}
