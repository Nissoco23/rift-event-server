//! Voice chat for Rift Event Server: a server for the Simple Voice Chat protocol (the 2.6 releases,
//! compatibility version 20), as a native mod. Players who have the Simple Voice Chat mod on their
//! client talk to the players near them, and in groups; nothing else is needed on their side, and
//! players without it play as before.
//!
//! The mod speaks the two halves of the protocol. Over the game connection, plugin messages on
//! `voicechat:` channels: a client asks for a secret and is sent it with the players' states and the
//! groups, tells the server when it turns voice chat off, and makes, joins and leaves groups. Over
//! UDP, on a port of its own, the voice itself: the client authenticates with its secret, checks the
//! connection, then sends what the player says, which the server passes on to whoever should hear it,
//! each datagram encrypted with its receiver's secret (`core.rs`, `udp.rs`). The server never decodes
//! a voice; it only relays the opus frames.
//!
//! The engine runs mods as Rust code linked into its executable, so this crate is a library
//! (`VoiceChat`, a `Hooks`) and `crates/rift-engine` is the engine's `main` with it, and every other
//! mod in `rift-mods/`, handed over (docs/rift-mods.md).
#![forbid(unsafe_code)]

mod buf;
mod command;
mod config;
mod core;
mod crypt;
mod proto;
#[cfg(test)]
mod testing;
mod udp;

use crate::core::{Core, Event, Message, PlayerInfo, Refused};
use horizon_client::mod_api::{
    CustomPayloadFromClientEvent, Flow, Hooks, Host, NativeServerPlayer, PerformCommandEvent,
    PerformConsoleCommandEvent, PlayerLoggedInEvent, PlayerLoggedOutEvent, RegisterCommandsEvent,
    ServerStoppingEvent, ServerTickEvent, ServerTickPhase, Text,
};
use proto::Inbound;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use udp::Running;

/// The name the engine's executable gives the mod (`horizon_client::mod_api::name_mods`), which a
/// crash report and a log line name its hooks by.
pub const ID: &str = "voicechat";

/// The mod as the engine's executable takes it: the `Hooks` to hand to
/// `horizon_client::entry::main_with`.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(VoiceChat::default())
}

/// The mod: the voice chat server, when it runs.
pub struct VoiceChat {
    core: Arc<Mutex<Core>>,
    running: Mutex<Option<Running>>,
    /// The config was read (and the server started, if it was to be).
    started: AtomicBool,
    /// The voice chat server runs: its socket is bound.
    active: AtomicBool,
}

impl Default for VoiceChat {
    fn default() -> Self {
        Self {
            core: Arc::new(Mutex::new(Core::new(config::Config::default()))),
            running: Mutex::new(None),
            started: AtomicBool::new(false),
            active: AtomicBool::new(false),
        }
    }
}

fn info_of(player: &NativeServerPlayer) -> PlayerInfo {
    PlayerInfo {
        name: player
            .nickname
            .clone()
            .unwrap_or_else(|| player.name.clone()),
        dimension: player.dimension.clone(),
        position: player.position,
        spectator: player.game_mode == 3,
    }
}

impl VoiceChat {
    fn core(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// The address the voice server is bound to, once it is.
    #[cfg(test)]
    pub(crate) fn address(&self) -> Option<std::net::SocketAddr> {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|r| r.address)
    }

    /// Reads `config/voicechat.toml` and starts the voice server, the first time a hook needs it.
    fn ensure_started(&self, host: &dyn Host) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let loaded = host.config_dir().and_then(|dir| config::load(&dir));
        let config = match loaded {
            Ok(config) => config,
            Err(e) => {
                host.log(
                    "WARN",
                    &format!("Voice chat: could not read the config, using the defaults: {e}"),
                );
                config::Config::default()
            }
        };
        if !config.enabled {
            host.log(
                "INFO",
                "Voice chat is turned off (enabled = false in config/voicechat.toml)",
            );
            return;
        }
        *self.core() = Core::new(config);
        match Running::start(Arc::clone(&self.core)) {
            Ok(running) => {
                host.log(
                    "INFO",
                    &format!("Voice chat server started at {}", running.address),
                );
                let config = self.core().config.clone();
                if config.voice_host.trim().is_empty() {
                    host.log(
                        "INFO",
                        "Voice chat: voice-host is empty: players send their voice to the address they play on, which is right alone or behind a proxy that runs the Simple Voice Chat plugin; behind one that does not, set it in config/voicechat.toml",
                    );
                }
                *self.running.lock().unwrap_or_else(|e| e.into_inner()) = Some(running);
                self.active.store(true, Ordering::Release);
            }
            Err(e) => host.log(
                "ERROR",
                &format!(
                    "Voice chat: could not start the voice chat server on UDP port {}: {e}",
                    self.core().config.port
                ),
            ),
        }
    }

    /// Sends what the core asked to be sent.
    fn send(&self, host: &mut dyn Host, messages: Vec<Message>) {
        for message in messages {
            if let Err(e) =
                host.send_custom_payload_to_player(message.to, message.channel, &message.data)
            {
                host.log(
                    "DEBUG",
                    &format!(
                        "Voice chat: could not send {} to a player: {e}",
                        message.channel
                    ),
                );
            }
        }
    }

    /// The players as this tick sees them, and what the voice server has to tell the game.
    fn tick(&self, host: &mut dyn Host) {
        let players: Vec<NativeServerPlayer> = host
            .server_players()
            .into_iter()
            .filter(|p| !p.fake)
            .collect();
        let (mut out, events) = {
            let mut core = self.core();
            let mut out = Vec::new();
            // A player the mod has not heard join (it started after them) joins now.
            for player in &players {
                if core.player(player.uuid).is_none() {
                    out.extend(core.player_joined(player.uuid, info_of(player)));
                }
            }
            let seen = players.iter().map(|p| (p.uuid, info_of(p))).collect();
            out.extend(core.refresh(seen));
            (out, core.take_events())
        };
        for event in events {
            let mut core = self.core();
            match event {
                Event::Connected(uuid) => {
                    host.log(
                        "INFO",
                        &format!("Voice chat: {} connected", name_of(&players, uuid)),
                    );
                    out.extend(core.voice_connected(uuid));
                }
                Event::TimedOut(uuid) => {
                    host.log(
                        "INFO",
                        &format!("Voice chat: {} timed out", name_of(&players, uuid)),
                    );
                    out.extend(core.voice_disconnected(uuid));
                    out.extend(core.reconnect(uuid));
                }
            }
        }
        self.send(host, out);
    }

    /// What a client sent over the game connection.
    fn handle(&self, host: &mut dyn Host, player: u128, inbound: Inbound) {
        let mut core = self.core();
        let out = match inbound {
            Inbound::RequestSecret(version) => match core.request_secret(player, version) {
                Ok(messages) => {
                    if core.has_secret(player) {
                        host.log(
                            "INFO",
                            &format!("Voice chat: sent the secret to a player ({player:032x})"),
                        );
                    }
                    messages
                }
                Err(Refused::Version(version)) => {
                    drop(core);
                    host.log(
                        "WARN",
                        &format!("Voice chat: a client with voice chat version {version} is not compatible (this server speaks {})", proto::COMPATIBILITY),
                    );
                    host.tell(player, incompatible_message());
                    return;
                }
            },
            Inbound::UpdateState { disabled } => core.update_state(player, disabled),
            Inbound::JoinGroup { group, password } => core.join_group(player, group, password),
            Inbound::CreateGroup {
                name,
                password,
                kind,
            } => core.create_group(player, name, password, kind),
            Inbound::LeaveGroup => core.leave_group(player),
        };
        drop(core);
        self.send(host, out);
    }
}

fn name_of(players: &[NativeServerPlayer], uuid: u128) -> String {
    players
        .iter()
        .find(|p| p.uuid == uuid)
        .map_or_else(|| format!("{uuid:032x}"), |p| p.name.clone())
}

/// What a player whose voice chat mod is of another version is told: the voice chat mod's own
/// translation, with this text where a client has none.
fn incompatible_message() -> Text {
    Text::translate_with_fallback(
        "message.voicechat.incompatible_version",
        "Your voice chat client version is not compatible with the server-side version.\nPlease install version %s of %s.",
        &[Text::literal("2.6.x"), Text::literal("Simple Voice Chat")],
    )
}

impl Hooks for VoiceChat {
    fn register_commands(&self, host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        self.ensure_started(&*host);
        command::register(event.commands);
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        self.run_command(host, event.command)
    }

    fn perform_console_command(
        &self,
        host: &mut dyn Host,
        event: &mut PerformConsoleCommandEvent<'_>,
    ) -> Flow {
        self.run_command(host, event.command)
    }

    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        if let ServerTickPhase::ServerTickEnd {
            world_running: true,
        } = event.phase
        {
            self.ensure_started(&*host);
            if self.active() {
                self.tick(host);
            }
        }
    }

    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        self.ensure_started(&*host);
        if !self.active() {
            return;
        }
        let info = host
            .server_players()
            .into_iter()
            .find(|p| p.uuid == event.player)
            .filter(|p| !p.fake)
            .map(|p| info_of(&p));
        let Some(info) = info else {
            return;
        };
        let out = self.core().player_joined(event.player, info);
        self.send(host, out);
    }

    fn player_logged_out(&self, host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        if !self.active() {
            return;
        }
        let out = self.core().player_left(event.player);
        self.send(host, out);
    }

    fn custom_payload_from_client(
        &self,
        host: &mut dyn Host,
        event: &mut CustomPayloadFromClientEvent<'_>,
    ) {
        if !self.active() || !event.channel.starts_with("voicechat:") {
            return;
        }
        if !self.core().allow(event.player, Instant::now()) {
            host.log(
                "DEBUG",
                &format!(
                    "Voice chat: dropped a packet of {:032x}: too many in a second",
                    event.player
                ),
            );
            return;
        }
        match proto::read_inbound(event.channel, event.data) {
            Some(Some(inbound)) => self.handle(host, event.player, inbound),
            Some(None) => host.log(
                "DEBUG",
                &format!(
                    "Voice chat: a badly formed {} packet from {:032x}",
                    event.channel, event.player
                ),
            ),
            None => {}
        }
    }

    fn server_stopping(&self, _host: &mut dyn Host, _event: &mut ServerStoppingEvent) {
        self.active.store(false, Ordering::Release);
        if let Some(mut running) = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            running.stop();
        }
    }
}

impl VoiceChat {
    fn run_command(&self, host: &mut dyn Host, line: &str) -> Flow {
        if !command::is_ours(line) {
            return Flow::Continue;
        }
        self.ensure_started(&*host);
        let players = host.server_players();
        let lines = {
            let core = self.core();
            command::status(&core, &players, self.active())
        };
        for line in lines {
            host.feedback(line);
        }
        Flow::Cancel
    }
}
