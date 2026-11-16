//! What the voice chat server knows and does, apart from the sockets and the game: the players,
//! their states and groups, who has a secret and a connection, and the rules for who hears whom.
//!
//! Everything here is a plain function of the state and what came in, answering with what to send:
//! datagrams for [`Core::handle`] and [`Core::keep_alives`], and plugin messages for the game
//! connection for the rest. The UDP thread and the game's hooks share one `Core` behind a lock, so
//! neither holds it across a call out of the mod.

use crate::config::Config;
use crate::crypt::{self, Secret};
use crate::proto::{
    self, COMPATIBILITY, ClientGroup, Datagram, GroupKind, Out, PlayerState, SecretInfo, Sound,
    Voice,
};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The id of the ping that tests a connection to the voice server (before any secret exists).
const PING_V1: u128 = 0x58bc_9ae9_c7a8_45e4_a11c_efbb_6719_9425;
const PING_PAYLOAD: usize = 24;
/// Game-connection packets a player may send in a second.
const RATE_LIMIT: u32 = 16;
/// How long a connection that has authenticated but not checked may wait to be checked.
const UNCHECKED_FOR: Duration = Duration::from_secs(60);

/// Where a player is, as the last server tick saw them.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInfo {
    /// The name the others know them by: their nickname when they have one.
    pub name: String,
    pub dimension: String,
    pub position: [f64; 3],
    pub spectator: bool,
}

/// What the voice chat knows of a player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceState {
    pub disabled: bool,
    pub disconnected: bool,
    pub group: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: u128,
    pub name: String,
    pub password: Option<String>,
    pub kind: GroupKind,
}

struct Connection {
    addr: SocketAddr,
    last_keep_alive: Instant,
}

/// What the UDP side tells the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A player's voice connection checked out.
    Connected(u128),
    /// A player's voice connection stopped answering.
    TimedOut(u128),
}

/// A datagram to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub to: SocketAddr,
    pub bytes: Vec<u8>,
}

/// A plugin message for a player's game connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub to: u128,
    pub channel: &'static str,
    pub data: Vec<u8>,
}

/// Why a client got no secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Its voice chat mod is of another compatibility version.
    Version(i32),
}

pub struct Core {
    pub config: Config,
    /// The port the voice socket is bound to (the config's, or the one the system gave for 0).
    pub port: u16,
    players: HashMap<u128, PlayerInfo>,
    states: HashMap<u128, VoiceState>,
    groups: HashMap<u128, Group>,
    secrets: HashMap<u128, Secret>,
    /// Players whose client has the mod at a version this server speaks: the ones that are sent
    /// anything.
    compatible: HashSet<u128>,
    connections: HashMap<u128, Connection>,
    unchecked: HashMap<u128, (Connection, Instant)>,
    events: Vec<Event>,
    rates: HashMap<u128, (Instant, u32)>,
}

impl Core {
    pub fn new(config: Config) -> Self {
        Self {
            port: config.port,
            config,
            players: HashMap::new(),
            states: HashMap::new(),
            groups: HashMap::new(),
            secrets: HashMap::new(),
            compatible: HashSet::new(),
            connections: HashMap::new(),
            unchecked: HashMap::new(),
            events: Vec::new(),
            rates: HashMap::new(),
        }
    }

    // ---- reading it --------------------------------------------------------------------------

    pub fn state_of(&self, uuid: u128) -> Option<&VoiceState> {
        self.states.get(&uuid)
    }

    #[cfg(test)]
    pub fn group(&self, id: u128) -> Option<&Group> {
        self.groups.get(&id)
    }

    pub fn groups(&self) -> impl Iterator<Item = &Group> {
        self.groups.values()
    }

    pub fn is_compatible(&self, uuid: u128) -> bool {
        self.compatible.contains(&uuid)
    }

    pub fn is_connected(&self, uuid: u128) -> bool {
        self.connections.contains_key(&uuid)
    }

    pub fn has_secret(&self, uuid: u128) -> bool {
        self.secrets.contains_key(&uuid)
    }

    pub fn player(&self, uuid: u128) -> Option<&PlayerInfo> {
        self.players.get(&uuid)
    }

    /// The events the UDP side has queued, taken.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn announced(&self, uuid: u128) -> Option<PlayerState> {
        let info = self.players.get(&uuid)?;
        let state = self.states.get(&uuid)?;
        Some(PlayerState {
            uuid,
            name: info.name.clone(),
            disabled: state.disabled,
            disconnected: state.disconnected,
            group: state.group,
        })
    }

    fn client_group(group: &Group) -> ClientGroup {
        ClientGroup {
            id: group.id,
            name: group.name.clone(),
            has_password: group.password.is_some(),
            kind: group.kind,
        }
    }

    /// `(channel, data)` for every player whose client has the mod.
    fn to_all(&self, packet: (&'static str, Vec<u8>)) -> Vec<Message> {
        let mut to: Vec<u128> = self.compatible.iter().copied().collect();
        to.sort_unstable();
        to.into_iter()
            .map(|to| Message {
                to,
                channel: packet.0,
                data: packet.1.clone(),
            })
            .collect()
    }

    fn tell(&self, to: u128, packet: (&'static str, Vec<u8>)) -> Vec<Message> {
        if !self.compatible.contains(&to) {
            return Vec::new();
        }
        vec![Message {
            to,
            channel: packet.0,
            data: packet.1,
        }]
    }

    // ---- players -----------------------------------------------------------------------------

    /// A player is on the server: they start with voice chat on and no connection.
    pub fn player_joined(&mut self, uuid: u128, info: PlayerInfo) -> Vec<Message> {
        self.players.insert(uuid, info);
        self.states.insert(
            uuid,
            VoiceState {
                disabled: false,
                disconnected: true,
                group: None,
            },
        );
        self.announced(uuid)
            .map(|s| self.to_all(proto::state(&s)))
            .unwrap_or_default()
    }

    /// The players as the server's tick sees them: who is where, and the name they go by. A
    /// player whose name changed is announced again.
    pub fn refresh(&mut self, seen: Vec<(u128, PlayerInfo)>) -> Vec<Message> {
        let mut out = Vec::new();
        for (uuid, info) in seen {
            let renamed = self
                .players
                .get(&uuid)
                .is_some_and(|old| old.name != info.name);
            if let Some(slot) = self.players.get_mut(&uuid) {
                *slot = info;
            } else {
                continue;
            }
            if renamed && let Some(state) = self.announced(uuid) {
                out.extend(self.to_all(proto::state(&state)));
            }
        }
        out
    }

    /// A player left the server.
    pub fn player_left(&mut self, uuid: u128) -> Vec<Message> {
        self.players.remove(&uuid);
        self.states.remove(&uuid);
        self.secrets.remove(&uuid);
        self.compatible.remove(&uuid);
        self.connections.remove(&uuid);
        self.unchecked.remove(&uuid);
        self.rates.remove(&uuid);
        let mut out = self.to_all(proto::remove_state(uuid));
        out.extend(self.cleanup_groups());
        out
    }

    /// Removes the groups nobody is in.
    fn cleanup_groups(&mut self) -> Vec<Message> {
        let used: HashSet<u128> = self.states.values().filter_map(|s| s.group).collect();
        let mut gone: Vec<u128> = self
            .groups
            .keys()
            .filter(|id| !used.contains(id))
            .copied()
            .collect();
        gone.sort_unstable();
        let mut out = Vec::new();
        for id in gone {
            self.groups.remove(&id);
            out.extend(self.to_all(proto::remove_group(id)));
        }
        out
    }

    // ---- the game connection -----------------------------------------------------------------

    /// Whether a player may send another packet now: no more than 16 in a second.
    pub fn allow(&mut self, uuid: u128, now: Instant) -> bool {
        let (since, count) = self.rates.entry(uuid).or_insert((now, 0));
        if now.duration_since(*since) >= Duration::from_secs(1) {
            *since = now;
            *count = 0;
        }
        *count += 1;
        *count <= RATE_LIMIT
    }

    /// A client with the voice chat mod asks for its secret, at compatibility version `version`.
    /// A client of the right version is sent the players' states, the groups and the secret; one
    /// of another is refused.
    pub fn request_secret(&mut self, uuid: u128, version: i32) -> Result<Vec<Message>, Refused> {
        if version != COMPATIBILITY {
            return Err(Refused::Version(version));
        }
        if !self.players.contains_key(&uuid) {
            return Ok(Vec::new());
        }
        self.compatible.insert(uuid);
        let mut out = self.initial(uuid);
        // A client that asks again has its secret already, and is told the states again.
        if self.secrets.contains_key(&uuid) {
            return Ok(out);
        }
        let secret = crypt::new_secret();
        self.secrets.insert(uuid, secret);
        out.extend(self.tell(
            uuid,
            proto::secret(&SecretInfo {
                secret,
                port: i32::from(self.port),
                player: uuid,
                codec: self.config.codec(),
                mtu: self.config.mtu_size,
                distance: self.config.max_voice_distance,
                keep_alive: self.config.keep_alive,
                groups: self.config.groups,
                host: self.config.voice_host.clone(),
                allow_recording: self.config.allow_recording,
            }),
        ));
        Ok(out)
    }

    /// The states of every player and every group, for a client that just asked.
    fn initial(&self, to: u128) -> Vec<Message> {
        let mut states: Vec<PlayerState> = self
            .players
            .keys()
            .filter_map(|uuid| self.announced(*uuid))
            .collect();
        states.sort_by_key(|s| s.uuid);
        let mut out = self.tell(to, proto::states(&states));
        let mut groups: Vec<&Group> = self.groups.values().collect();
        groups.sort_by_key(|g| g.id);
        for group in groups {
            out.extend(self.tell(to, proto::add_group(&Self::client_group(group))));
        }
        out
    }

    /// A player's connection timed out: they are asked to connect again with a new secret.
    pub fn reconnect(&mut self, uuid: u128) -> Vec<Message> {
        if !self.compatible.contains(&uuid) || !self.players.contains_key(&uuid) {
            return Vec::new();
        }
        self.secrets.remove(&uuid);
        self.request_secret(uuid, COMPATIBILITY).unwrap_or_default()
    }

    /// A player turned voice chat off or on in their client.
    pub fn update_state(&mut self, uuid: u128, disabled: bool) -> Vec<Message> {
        let Some(state) = self.states.get_mut(&uuid) else {
            return Vec::new();
        };
        state.disabled = disabled;
        self.announced(uuid)
            .map(|s| self.to_all(proto::state(&s)))
            .unwrap_or_default()
    }

    /// A player's voice connection checked out.
    pub fn voice_connected(&mut self, uuid: u128) -> Vec<Message> {
        let Some(state) = self.states.get_mut(&uuid) else {
            return Vec::new();
        };
        state.disconnected = false;
        self.announced(uuid)
            .map(|s| self.to_all(proto::state(&s)))
            .unwrap_or_default()
    }

    /// A player's voice connection is gone.
    pub fn voice_disconnected(&mut self, uuid: u128) -> Vec<Message> {
        let Some(state) = self.states.get_mut(&uuid) else {
            return Vec::new();
        };
        state.disconnected = true;
        self.announced(uuid)
            .map(|s| self.to_all(proto::state(&s)))
            .unwrap_or_default()
    }

    fn set_group(&mut self, uuid: u128, group: Option<u128>) -> Vec<Message> {
        let Some(state) = self.states.get_mut(&uuid) else {
            return Vec::new();
        };
        state.group = group;
        self.announced(uuid)
            .map(|s| self.to_all(proto::state(&s)))
            .unwrap_or_default()
    }

    /// A player makes a group and is in it. Names and passwords that are not text a group may have
    /// are refused without an answer, as a client's own check would have.
    pub fn create_group(
        &mut self,
        uuid: u128,
        name: String,
        password: Option<String>,
        kind: GroupKind,
    ) -> Vec<Message> {
        if !self.config.groups || !self.states.contains_key(&uuid) {
            return Vec::new();
        }
        if !valid_group_text(&name) || password.as_deref().is_some_and(|p| !valid_group_text(p)) {
            return Vec::new();
        }
        let id = random_id();
        let group = Group {
            id,
            name,
            password,
            kind,
        };
        let mut out = self.to_all(proto::add_group(&Self::client_group(&group)));
        self.groups.insert(id, group);
        out.extend(self.set_group(uuid, Some(id)));
        out.extend(self.tell(uuid, proto::joined_group(Some(id), false)));
        out
    }

    /// A player joins a group, with the password if it has one.
    pub fn join_group(&mut self, uuid: u128, id: u128, password: Option<String>) -> Vec<Message> {
        if !self.config.groups || !self.states.contains_key(&uuid) {
            return Vec::new();
        }
        let Some(group) = self.groups.get(&id) else {
            return self.tell(uuid, proto::joined_group(None, false));
        };
        if group.password.is_some() && group.password != password {
            return self.tell(uuid, proto::joined_group(None, true));
        }
        let mut out = self.set_group(uuid, Some(id));
        out.extend(self.tell(uuid, proto::joined_group(Some(id), false)));
        // The group they left may be empty now.
        out.extend(self.cleanup_groups());
        out
    }

    /// A player leaves the group they are in.
    pub fn leave_group(&mut self, uuid: u128) -> Vec<Message> {
        if !self.states.contains_key(&uuid) {
            return Vec::new();
        }
        let mut out = self.set_group(uuid, None);
        out.extend(self.tell(uuid, proto::joined_group(None, false)));
        out.extend(self.cleanup_groups());
        out
    }

    // ---- the UDP side ------------------------------------------------------------------------

    fn seal(&self, to: u128, out: &Out) -> Option<Vec<u8>> {
        let secret = self.secrets.get(&to)?;
        Some(proto::datagram_to_client(&crypt::seal(
            secret,
            &out.write(),
        )))
    }

    fn packet(&self, to: u128, addr: SocketAddr, out: &Out) -> Option<Outgoing> {
        Some(Outgoing {
            to: addr,
            bytes: self.seal(to, out)?,
        })
    }

    /// What a datagram from `from` is answered with.
    pub fn handle(&mut self, now: Instant, from: SocketAddr, data: &[u8]) -> Vec<Outgoing> {
        let Some(datagram) = Datagram::read(data) else {
            return Vec::new();
        };
        let player = datagram.player;
        let Some(secret) = self.secrets.get(&player).copied() else {
            return self.ping(from, &datagram);
        };
        let Some(plain) = crypt::open(&secret, datagram.sealed) else {
            return Vec::new();
        };
        let Some(voice) = Voice::read(&plain) else {
            return Vec::new();
        };
        match voice {
            Voice::Authenticate {
                player: claimed,
                secret: given,
            } => {
                if claimed != player || !crypt::same(&given, &secret) {
                    return Vec::new();
                }
                // A client that comes from another address (it was restarted) is checked again.
                let known = self.connections.get(&player).map(|c| c.addr);
                if known != Some(from) {
                    self.connections.remove(&player);
                    self.unchecked.insert(
                        player,
                        (
                            Connection {
                                addr: from,
                                last_keep_alive: now,
                            },
                            now,
                        ),
                    );
                }
                self.packet(player, from, &Out::AuthenticateAck)
                    .into_iter()
                    .collect()
            }
            Voice::ConnectionCheck => {
                if let Some((connection, _)) = self.unchecked.get(&player)
                    && connection.addr == from
                {
                    self.unchecked.remove(&player);
                    self.connections.insert(
                        player,
                        Connection {
                            addr: from,
                            last_keep_alive: now,
                        },
                    );
                    self.events.push(Event::Connected(player));
                } else if self.connections.get(&player).is_none_or(|c| c.addr != from) {
                    return Vec::new();
                }
                self.packet(player, from, &Out::ConnectionCheckAck)
                    .into_iter()
                    .collect()
            }
            other => {
                let Some(connection) = self.connections.get_mut(&player) else {
                    return Vec::new();
                };
                if connection.addr != from {
                    return Vec::new();
                }
                match other {
                    Voice::KeepAlive => {
                        connection.last_keep_alive = now;
                        Vec::new()
                    }
                    Voice::Mic {
                        data,
                        sequence,
                        whispering,
                    } => self.route(player, data, sequence, whispering),
                    _ => Vec::new(),
                }
            }
        }
    }

    /// The reply to a ping that tests the connection, for a datagram of a player with no secret.
    fn ping(&self, from: SocketAddr, datagram: &Datagram<'_>) -> Vec<Outgoing> {
        if self.config.allow_pings
            && datagram.player == PING_V1
            && datagram.sealed.len() <= PING_PAYLOAD
        {
            return vec![Outgoing {
                to: from,
                bytes: datagram.sealed.to_vec(),
            }];
        }
        Vec::new()
    }

    /// Keep-alives for every connection, and the end of those that have not answered in ten times
    /// the interval. Called about as often as the interval.
    pub fn keep_alives(&mut self, now: Instant) -> Vec<Outgoing> {
        let patience = Duration::from_millis(self.config.keep_alive.max(1000) as u64 * 10);
        let mut timed_out: Vec<u128> = self
            .connections
            .iter()
            .filter(|(_, c)| now.duration_since(c.last_keep_alive) >= patience)
            .map(|(uuid, _)| *uuid)
            .collect();
        timed_out.sort_unstable();
        for uuid in &timed_out {
            self.connections.remove(uuid);
            self.secrets.remove(uuid);
            self.events.push(Event::TimedOut(*uuid));
        }
        self.unchecked
            .retain(|_, (_, since)| now.duration_since(*since) < UNCHECKED_FOR);
        let mut to: Vec<(u128, SocketAddr)> = self
            .connections
            .iter()
            .map(|(uuid, c)| (*uuid, c.addr))
            .collect();
        to.sort_unstable_by_key(|(uuid, _)| *uuid);
        to.into_iter()
            .filter_map(|(uuid, addr)| self.packet(uuid, addr, &Out::KeepAlive))
            .collect()
    }

    // ---- who hears whom ----------------------------------------------------------------------

    /// A frame a player said: to the others in their group, and to the players near enough.
    fn route(&self, sender: u128, data: Vec<u8>, sequence: i64, whispering: bool) -> Vec<Outgoing> {
        let (Some(state), Some(info)) = (self.states.get(&sender), self.players.get(&sender))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(group) = state.group {
            let sound = Out::Sound(Sound::Group {
                channel: sender,
                sender,
                data: data.clone(),
                sequence,
            });
            let mut members: Vec<u128> = self
                .states
                .iter()
                .filter(|(uuid, s)| **uuid != sender && s.group == Some(group))
                .map(|(uuid, _)| *uuid)
                .collect();
            members.sort_unstable();
            for member in members {
                out.extend(self.deliver(member, &sound));
            }
            // A group that is not open keeps what it says to itself.
            let open = self
                .groups
                .get(&group)
                .is_none_or(|g| g.kind == GroupKind::Open);
            if !open {
                return out;
            }
        }
        let distance = if whispering {
            self.config.whisper_distance
        } else {
            self.config.max_voice_distance
        } as f32;
        // Players a block past the distance are told too: the clients fade a voice out by the
        // distance in the packet.
        let range = (self.config.max_voice_distance + 1.0).max(f64::from(distance));
        let sound = Out::Sound(Sound::Player {
            channel: sender,
            sender,
            data,
            sequence,
            distance,
            whispering,
        });
        let mut hearers: Vec<u128> = self
            .players
            .iter()
            .filter(|(uuid, other)| {
                **uuid != sender
                    && other.dimension == info.dimension
                    && distance_squared(other.position, info.position) <= range * range
                    && (!info.spectator
                        || self.config.spectators_talk_to_players
                        || other.spectator)
            })
            .map(|(uuid, _)| *uuid)
            .collect();
        hearers.sort_unstable();
        for hearer in hearers {
            let Some(state) = self.states.get(&hearer) else {
                continue;
            };
            // Their own group already heard it, and an isolated group hears only itself.
            if state.group.is_some() && state.group == self.states[&sender].group {
                continue;
            }
            if state
                .group
                .and_then(|g| self.groups.get(&g))
                .is_some_and(|g| g.kind == GroupKind::Isolated)
            {
                continue;
            }
            out.extend(self.deliver(hearer, &sound));
        }
        out
    }

    /// `sound` for `to`, when they have a voice connection and are listening.
    fn deliver(&self, to: u128, sound: &Out) -> Option<Outgoing> {
        let state = self.states.get(&to)?;
        if state.disabled || state.disconnected {
            return None;
        }
        let connection = self.connections.get(&to)?;
        self.packet(to, connection.addr, sound)
    }
}

fn distance_squared(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn random_id() -> u128 {
    let bytes = crypt::new_secret();
    u128::from_be_bytes(bytes)
}

/// Whether `text` may be a group's name or password: one to 24 characters, the first neither
/// whitespace nor a control or format character, none of the rest a control or format character.
pub fn valid_group_text(text: &str) -> bool {
    let count = text.chars().count();
    if !(1..=24).contains(&count) {
        return false;
    }
    let invisible = |c: char| {
        c.is_control()
            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}')
    };
    let mut chars = text.chars();
    let first = chars.next().unwrap_or(' ');
    !first.is_whitespace() && !invisible(first) && chars.all(|c| !invisible(c))
}
