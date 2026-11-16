//! The voice chat server in memory, and over a real UDP socket on the loopback.
//!
//! `World` drives the `Core` the way the UDP thread and the hooks do, with clients that hold a
//! secret and encrypt as a real one does, and reads what the server sends back by decrypting it with
//! the receiver's secret. `Rig` boxes the mod as the engine does over a fake server and runs its hooks
//! through the engine's own `dispatch_*` functions; with it a test can put two real sockets in front
//! of the real voice server.

use crate::buf::{Reader, Writer};
use crate::config::Config;
use crate::core::{Core, Event, Message, Outgoing, PlayerInfo};
use crate::crypt::{self, SECRET_LEN, Secret};
use crate::proto::{self, GroupKind, Voice};
use crate::{VoiceChat, proto::channel};
use horizon_client::mod_api::{
    CustomPayloadFromClientEvent, Flow, Hooks, Host, NativeAuthority, NativeLifecycleContext,
    NativeServerPlayer, NativeWorldEpoch, PerformCommandEvent, PerformConsoleCommandEvent,
    PlayerLoggedInEvent, PlayerLoggedOutEvent, RegisterCommandsEvent, ServerStoppingEvent,
    ServerTickEvent, ServerTickPhase, Text, dispatch_custom_payload_from_client,
    dispatch_perform_command, dispatch_perform_console_command, dispatch_player_logged_in,
    dispatch_player_logged_out, dispatch_register_commands, dispatch_server_stopping,
    dispatch_server_tick,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub const OVERWORLD: &str = "minecraft:overworld";
pub const NETHER: &str = "minecraft:the_nether";
pub const A: u128 = 0xa;
pub const B: u128 = 0xb;
pub const C: u128 = 0xc;
pub const D: u128 = 0xd;

/// What a client hears from the server, decrypted.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    Player {
        channel: u128,
        sender: u128,
        data: Vec<u8>,
        sequence: i64,
        distance: f32,
        whispering: bool,
    },
    Group {
        channel: u128,
        sender: u128,
        data: Vec<u8>,
        sequence: i64,
    },
    AuthenticateAck,
    KeepAlive,
    ConnectionCheckAck,
}

/// Reads the decrypted payload of a datagram the server sent.
pub fn hear(plain: &[u8]) -> Heard {
    let mut r = Reader::new(plain);
    match r.u8().unwrap() {
        0x2 => {
            let channel = r.uuid().unwrap();
            let sender = r.uuid().unwrap();
            let data = r.byte_array(1275).unwrap().to_vec();
            let sequence = r.i64().unwrap();
            let distance = r.f32().unwrap();
            let flags = r.u8().unwrap();
            assert!(r.is_empty(), "nothing after the flags");
            Heard::Player {
                channel,
                sender,
                data,
                sequence,
                distance,
                whispering: flags & 1 != 0,
            }
        }
        0x3 => {
            let channel = r.uuid().unwrap();
            let sender = r.uuid().unwrap();
            let data = r.byte_array(1275).unwrap().to_vec();
            let sequence = r.i64().unwrap();
            assert_eq!(r.u8().unwrap(), 0, "no category");
            assert!(r.is_empty());
            Heard::Group {
                channel,
                sender,
                data,
                sequence,
            }
        }
        0x6 => Heard::AuthenticateAck,
        0x8 => Heard::KeepAlive,
        0xa => Heard::ConnectionCheckAck,
        other => panic!("a packet the server does not send: {other}"),
    }
}

/// The plain payload of a datagram from the server, opened with `secret`.
pub fn open_datagram(secret: &Secret, bytes: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader::new(bytes);
    if r.u8()? != proto::MAGIC {
        return None;
    }
    crypt::open(secret, r.byte_array(4096)?)
}

pub fn addr(n: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 30000 + n))
}

/// A client: who it is, the secret it was given and where it sends from.
#[derive(Clone)]
pub struct Peer {
    pub player: u128,
    pub secret: Secret,
    pub addr: SocketAddr,
}

impl Peer {
    /// A datagram of `voice`, as this client sends it.
    pub fn send(&self, voice: &Voice) -> Vec<u8> {
        proto::datagram_to_server(self.player, &crypt::seal(&self.secret, &voice.write()))
    }

    pub fn authenticate(&self) -> Vec<u8> {
        self.send(&Voice::Authenticate {
            player: self.player,
            secret: self.secret,
        })
    }

    pub fn mic(&self, data: &[u8], sequence: i64, whispering: bool) -> Vec<u8> {
        self.send(&Voice::Mic {
            data: data.to_vec(),
            sequence,
            whispering,
        })
    }
}

/// The secret in a `voicechat:secret` message.
pub fn secret_of(messages: &[Message]) -> Option<Secret> {
    let message = messages.iter().find(|m| m.channel == channel::SECRET)?;
    message.data[..SECRET_LEN].try_into().ok()
}

/// A core with players, and clients that hold their secrets.
pub struct World {
    pub core: Core,
    pub now: Instant,
    peers: HashMap<u128, Peer>,
    next: u16,
}

pub fn info(name: &str, position: [f64; 3]) -> PlayerInfo {
    PlayerInfo {
        name: name.to_owned(),
        dimension: OVERWORLD.to_owned(),
        position,
        spectator: false,
    }
}

impl World {
    pub fn new() -> Self {
        Self::with(Config::default())
    }

    pub fn with(config: Config) -> Self {
        Self {
            core: Core::new(config),
            now: Instant::now(),
            peers: HashMap::new(),
            next: 0,
        }
    }

    /// A player is on the server, with the mod, asked for their secret.
    pub fn player(&mut self, uuid: u128, name: &str, position: [f64; 3]) {
        self.core.player_joined(uuid, info(name, position));
        let messages = self.core.request_secret(uuid, 20).unwrap();
        self.next += 1;
        self.peers.insert(
            uuid,
            Peer {
                player: uuid,
                secret: secret_of(&messages).expect("a secret"),
                addr: addr(self.next),
            },
        );
    }

    pub fn peer(&self, uuid: u128) -> Peer {
        self.peers[&uuid].clone()
    }

    /// What the server sends back to a datagram, decrypted, by who it is for.
    pub fn inbox(&self, out: Vec<Outgoing>) -> Vec<(u128, Heard)> {
        out.into_iter()
            .map(|o| {
                let peer = self
                    .peers
                    .values()
                    .find(|p| p.addr == o.to)
                    .expect("a datagram to a client we know");
                let plain =
                    open_datagram(&peer.secret, &o.bytes).expect("it opens with their secret");
                (peer.player, hear(&plain))
            })
            .collect()
    }

    pub fn send(&mut self, uuid: u128, datagram: Vec<u8>) -> Vec<(u128, Heard)> {
        let from = self.peers[&uuid].addr;
        let out = self.core.handle(self.now, from, &datagram);
        self.inbox(out)
    }

    /// A player authenticates and checks the connection, so they can talk and be talked to.
    pub fn connect(&mut self, uuid: u128) {
        let peer = self.peer(uuid);
        assert_eq!(
            self.send(uuid, peer.authenticate()),
            [(uuid, Heard::AuthenticateAck)]
        );
        assert_eq!(
            self.send(uuid, peer.send(&Voice::ConnectionCheck)),
            [(uuid, Heard::ConnectionCheckAck)]
        );
        self.core.voice_connected(uuid);
    }

    /// Players, each with the mod and connected, one after the other along x at `x`.
    pub fn talker(&mut self, uuid: u128, name: &str, position: [f64; 3]) {
        self.player(uuid, name, position);
        self.connect(uuid);
    }

    pub fn speak(&mut self, uuid: u128, data: &[u8]) -> Vec<(u128, Heard)> {
        let datagram = self.peer(uuid).mic(data, 7, false);
        self.send(uuid, datagram)
    }

    pub fn whisper(&mut self, uuid: u128, data: &[u8]) -> Vec<(u128, Heard)> {
        let datagram = self.peer(uuid).mic(data, 7, true);
        self.send(uuid, datagram)
    }

    pub fn set_position(&mut self, uuid: u128, position: [f64; 3]) {
        let mut info = self.core.player(uuid).unwrap().clone();
        info.position = position;
        self.core.refresh(vec![(uuid, info)]);
    }

    pub fn set_dimension(&mut self, uuid: u128, dimension: &str) {
        let mut info = self.core.player(uuid).unwrap().clone();
        info.dimension = dimension.to_owned();
        self.core.refresh(vec![(uuid, info)]);
    }

    pub fn set_spectator(&mut self, uuid: u128, spectator: bool) {
        let mut info = self.core.player(uuid).unwrap().clone();
        info.spectator = spectator;
        self.core.refresh(vec![(uuid, info)]);
    }
}

/// Who got a frame, in order.
pub fn who(heard: &[(u128, Heard)]) -> Vec<u128> {
    heard.iter().map(|(uuid, _)| *uuid).collect()
}

// ---- the server in memory -----------------------------------------------------------------

pub struct FakeHost {
    pub players: Vec<NativeServerPlayer>,
    /// Every plugin message sent: to whom, on what channel, with what bytes.
    pub sent: Vec<(u128, String, Vec<u8>)>,
    pub told: Vec<(u128, Text)>,
    pub feedback: Vec<String>,
    logs: RefCell<Vec<String>>,
    dir: PathBuf,
}

impl FakeHost {
    pub fn logged(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }

    /// What was sent to `uuid`, taken.
    pub fn take_sent(&mut self, uuid: u128) -> Vec<(String, Vec<u8>)> {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.sent)
            .into_iter()
            .partition(|(to, ..)| *to == uuid);
        self.sent = rest;
        mine.into_iter().map(|(_, c, d)| (c, d)).collect()
    }
}

impl Host for FakeHost {
    fn server_players(&self) -> Vec<NativeServerPlayer> {
        self.players.clone()
    }

    fn send_custom_payload_to_player(
        &mut self,
        player: u128,
        channel: &str,
        data: &[u8],
    ) -> Result<(), String> {
        self.sent.push((player, channel.to_owned(), data.to_vec()));
        Ok(())
    }

    fn tell(&mut self, uuid: u128, line: Text) -> bool {
        self.told.push((uuid, line));
        true
    }

    fn config_dir(&self) -> Result<PathBuf, String> {
        Ok(self.dir.join("config"))
    }

    fn log(&self, level: &str, line: &str) {
        self.logs.borrow_mut().push(format!("{level}: {line}"));
    }

    fn feedback(&mut self, line: Text) {
        self.feedback.push(line.plain());
    }
}

/// The mod, held by the test and by the engine's list at once.
pub struct Shared(pub Arc<VoiceChat>);

impl Hooks for Shared {
    fn register_commands(&self, host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        self.0.register_commands(host, event)
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        self.0.perform_command(host, event)
    }

    fn perform_console_command(
        &self,
        host: &mut dyn Host,
        event: &mut PerformConsoleCommandEvent<'_>,
    ) -> Flow {
        self.0.perform_console_command(host, event)
    }

    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        self.0.server_tick(host, event)
    }

    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        self.0.player_logged_in(host, event)
    }

    fn player_logged_out(&self, host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        self.0.player_logged_out(host, event)
    }

    fn custom_payload_from_client(
        &self,
        host: &mut dyn Host,
        event: &mut CustomPayloadFromClientEvent<'_>,
    ) {
        self.0.custom_payload_from_client(host, event)
    }

    fn server_stopping(&self, host: &mut dyn Host, event: &mut ServerStoppingEvent) {
        self.0.server_stopping(host, event)
    }
}

pub struct Rig {
    pub host: FakeHost,
    pub mods: Vec<Box<dyn Hooks>>,
    pub voice: Arc<VoiceChat>,
    tick: u64,
}

impl Rig {
    /// A server whose voice chat server listens on a port the system picks, on the loopback;
    /// `extra` is more of `voicechat.toml`.
    pub fn new(extra: &str) -> Self {
        Self::raw(&format!("port = 0\nbind-address = \"127.0.0.1\"\n{extra}"))
    }

    /// The same, with all of `voicechat.toml` given.
    pub fn raw(config: &str) -> Self {
        static RIGS: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "voicechat-rig-{}-{}",
            std::process::id(),
            RIGS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(dir.join("config").join("voicechat.toml"), config).unwrap();
        let voice = Arc::new(VoiceChat::default());
        Self {
            host: FakeHost {
                players: Vec::new(),
                sent: Vec::new(),
                told: Vec::new(),
                feedback: Vec::new(),
                logs: RefCell::new(Vec::new()),
                dir,
            },
            mods: vec![Box::new(Shared(Arc::clone(&voice)))],
            voice,
            tick: 0,
        }
    }

    pub fn join(&mut self, name: &str, uuid: u128, permission: u8) {
        let mut player = NativeServerPlayer::new(uuid, name, permission);
        player.position = [0.0, 64.0, 0.0];
        self.host.players.push(player);
        dispatch_player_logged_in(&mut self.host, &self.mods, uuid, name);
    }

    pub fn leave(&mut self, uuid: u128) {
        let name = self.player(uuid).name.clone();
        dispatch_player_logged_out(&mut self.host, &self.mods, uuid, &name);
        self.host.players.retain(|p| p.uuid != uuid);
    }

    pub fn player(&mut self, uuid: u128) -> &mut NativeServerPlayer {
        self.host
            .players
            .iter_mut()
            .find(|p| p.uuid == uuid)
            .unwrap()
    }

    pub fn tick(&mut self) {
        for phase in [
            ServerTickPhase::ServerTickStart {
                world_running: true,
            },
            ServerTickPhase::ServerTickEnd {
                world_running: true,
            },
        ] {
            let context = NativeLifecycleContext::new(
                NativeAuthority::DedicatedServer,
                NativeWorldEpoch::new(1),
                self.tick,
            );
            dispatch_server_tick(&mut self.host, &self.mods, &context, phase);
        }
        self.tick += 1;
    }

    /// A client sends a plugin message.
    pub fn plugin(&mut self, uuid: u128, channel: &str, data: &[u8]) {
        dispatch_custom_payload_from_client(&mut self.host, &self.mods, uuid, channel, data);
    }

    pub fn request_secret(&mut self, uuid: u128, version: i32) {
        self.plugin(uuid, channel::REQUEST_SECRET, &version.to_be_bytes());
    }

    pub fn stop(&mut self) {
        dispatch_server_stopping(&mut self.host, &self.mods, true);
    }

    pub fn register(&mut self) -> Vec<horizon_client::mod_api::NativeCommand> {
        let mut commands = Vec::new();
        dispatch_register_commands(&mut self.host, &self.mods, &mut commands);
        commands
    }

    pub fn console(&mut self, line: &str) -> bool {
        let mut ran = false;
        dispatch_perform_console_command(&mut self.host, &self.mods, line, &mut ran);
        ran
    }

    pub fn say(&mut self, uuid: u128, line: &str) -> bool {
        let player = self
            .host
            .players
            .iter()
            .find(|p| p.uuid == uuid)
            .cloned()
            .unwrap();
        let mut ran = false;
        dispatch_perform_command(&mut self.host, &self.mods, &player, line, &mut ran);
        ran
    }
}

/// A client that talks to the real voice server over a real socket.
pub struct Client {
    pub socket: UdpSocket,
    pub peer: Peer,
    server: SocketAddr,
}

impl Client {
    pub fn new(server: SocketAddr, player: u128, secret: Secret) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let addr = socket.local_addr().unwrap();
        Self {
            socket,
            peer: Peer {
                player,
                secret,
                addr,
            },
            server,
        }
    }

    pub fn send(&self, datagram: &[u8]) {
        self.socket.send_to(datagram, self.server).unwrap();
    }

    /// The next datagram from the server, decrypted and read.
    pub fn hear(&self) -> Option<Heard> {
        let mut buffer = [0u8; 4096];
        let (len, _) = self.socket.recv_from(&mut buffer).ok()?;
        open_datagram(&self.peer.secret, &buffer[..len]).map(|p| hear(&p))
    }

    /// The next sound, passing keep-alives.
    pub fn hear_sound(&self) -> Option<Heard> {
        loop {
            match self.hear()? {
                Heard::KeepAlive => continue,
                other => return Some(other),
            }
        }
    }
}

/// A plugin message's data, written like the clients write theirs.
pub fn create_group_bytes(name: &str, password: Option<&str>, kind: i16) -> Vec<u8> {
    let mut w = Writer::new();
    w.utf(name).bool(password.is_some());
    if let Some(p) = password {
        w.utf(p);
    }
    w.i16(kind);
    w.finish()
}

pub fn join_group_bytes(group: u128, password: Option<&str>) -> Vec<u8> {
    let mut w = Writer::new();
    w.uuid(group).bool(password.is_some());
    if let Some(p) = password {
        w.utf(p);
    }
    w.finish()
}

pub fn events(world: &mut World) -> Vec<Event> {
    world.core.take_events()
}

#[cfg(test)]
mod handshake {
    use super::*;

    fn state_bytes(
        uuid: u128,
        name: &str,
        disabled: bool,
        disconnected: bool,
        group: Option<u128>,
    ) -> Vec<u8> {
        let mut w = Writer::new();
        w.bool(disabled)
            .bool(disconnected)
            .uuid(uuid)
            .utf(name)
            .bool(group.is_some());
        if let Some(g) = group {
            w.uuid(g);
        }
        w.finish()
    }

    #[test]
    fn a_player_joins_with_voice_chat_on_and_no_connection_and_those_with_the_mod_are_told() {
        let mut core = Core::new(Config::default());
        // Nobody has the mod yet, so nobody is told.
        assert_eq!(core.player_joined(A, info("Alice", [0.0; 3])), []);
        core.request_secret(A, 20).unwrap();
        let told = core.player_joined(B, info("Bob", [0.0; 3]));
        assert_eq!(
            told,
            [Message {
                to: A,
                channel: channel::STATE,
                data: state_bytes(B, "Bob", false, true, None)
            }]
        );
    }

    #[test]
    fn a_client_with_the_right_version_is_sent_the_states_the_groups_and_its_secret_in_that_order()
    {
        let mut core = Core::new(Config {
            port: 24455,
            voice_host: "vc.example.org:25000".into(),
            max_voice_distance: 60.0,
            ..Config::default()
        });
        core.player_joined(A, info("Alice", [0.0; 3]));
        core.player_joined(B, info("Bob", [0.0; 3]));
        core.request_secret(A, 20).unwrap();
        core.create_group(A, "Friends".into(), Some("pw".into()), GroupKind::Open);
        let messages = core.request_secret(B, 20).unwrap();
        let channels: Vec<&str> = messages.iter().map(|m| m.channel).collect();
        assert_eq!(
            channels,
            [channel::STATES, channel::ADD_GROUP, channel::SECRET]
        );
        assert!(messages.iter().all(|m| m.to == B));
        // Both players' states, Alice in the group she made.
        let group = core.groups().next().unwrap().id;
        let mut expected = 2i32.to_be_bytes().to_vec();
        expected.extend(state_bytes(A, "Alice", false, true, Some(group)));
        expected.extend(state_bytes(B, "Bob", false, true, None));
        assert_eq!(messages[0].data, expected);
        // The secret says where to send voice and how far it carries.
        let data = &messages[2].data;
        let mut r = Reader::new(&data[SECRET_LEN..]);
        assert_eq!(r.i32(), Some(24455));
        assert_eq!(r.uuid(), Some(B));
        assert_eq!(r.u8(), Some(0));
        assert_eq!(r.i32(), Some(1275));
        assert_eq!(r.f64(), Some(60.0));
        assert_eq!(r.i32(), Some(1000));
        assert_eq!(r.bool(), Some(true));
        assert_eq!(r.utf(100).as_deref(), Some("vc.example.org:25000"));
        assert_eq!(r.bool(), Some(true));
        assert!(r.is_empty());
    }

    #[test]
    fn a_client_of_another_version_gets_no_secret_and_is_not_told_anything_else() {
        let mut core = Core::new(Config::default());
        core.player_joined(A, info("Alice", [0.0; 3]));
        assert_eq!(
            core.request_secret(A, 21),
            Err(crate::core::Refused::Version(21))
        );
        assert_eq!(
            core.request_secret(A, 19),
            Err(crate::core::Refused::Version(19))
        );
        assert!(!core.is_compatible(A) && !core.has_secret(A));
        // Nor is it sent the others' states afterwards.
        assert_eq!(core.update_state(A, true).len(), 0);
        core.player_joined(B, info("Bob", [0.0; 3]));
        assert_eq!(core.refresh(vec![(B, info("Robert", [0.0; 3]))]), []);
    }

    #[test]
    fn asking_again_gives_the_states_again_and_keeps_the_secret() {
        let mut core = Core::new(Config::default());
        core.player_joined(A, info("Alice", [0.0; 3]));
        let first = secret_of(&core.request_secret(A, 20).unwrap()).unwrap();
        let again = core.request_secret(A, 20).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].channel, channel::STATES);
        assert_eq!(secret_of(&again), None);
        assert!(core.has_secret(A));
        // The secret is the one it was.
        let mut world = World::with(Config::default());
        world.core = core;
        world.peers.insert(
            A,
            Peer {
                player: A,
                secret: first,
                addr: addr(1),
            },
        );
        world.connect(A);
    }

    #[test]
    fn a_secret_is_each_players_own() {
        let mut world = World::new();
        world.player(A, "Alice", [0.0; 3]);
        world.player(B, "Bob", [0.0; 3]);
        assert_ne!(world.peer(A).secret, world.peer(B).secret);
    }

    #[test]
    fn authenticating_is_acknowledged_to_where_it_came_from_and_only_with_the_secret() {
        let mut world = World::new();
        world.player(A, "Alice", [0.0; 3]);
        let peer = world.peer(A);
        assert_eq!(
            world.send(A, peer.authenticate()),
            [(A, Heard::AuthenticateAck)]
        );
        // Not the secret: nothing, and the same for another player's number.
        let wrong = Peer {
            secret: [1; SECRET_LEN],
            ..peer.clone()
        };
        let out = world
            .core
            .handle(world.now, peer.addr, &wrong.authenticate());
        assert!(out.is_empty());
        let mut lying = Vec::new();
        let mut w = Writer::new();
        w.u8(0x5).uuid(B).raw(&peer.secret);
        lying.extend(proto::datagram_to_server(
            A,
            &crypt::seal(&peer.secret, &w.finish()),
        ));
        assert!(world.core.handle(world.now, peer.addr, &lying).is_empty());
        // A datagram that is not ours, or from a player with no secret, is left alone.
        assert!(
            world
                .core
                .handle(world.now, peer.addr, &[1, 2, 3])
                .is_empty()
        );
        let stranger = Peer {
            player: 0x99,
            secret: [2; SECRET_LEN],
            addr: addr(9),
        };
        assert!(
            world
                .core
                .handle(world.now, stranger.addr, &stranger.authenticate())
                .is_empty()
        );
    }

    #[test]
    fn a_connection_must_be_checked_after_it_authenticates_and_the_game_hears_of_it_once() {
        let mut world = World::new();
        world.player(A, "Alice", [0.0; 3]);
        let peer = world.peer(A);
        // A check before authenticating is no connection.
        assert!(
            world
                .core
                .handle(world.now, peer.addr, &peer.send(&Voice::ConnectionCheck))
                .is_empty()
        );
        world.send(A, peer.authenticate());
        assert!(!world.core.is_connected(A));
        assert_eq!(
            world.send(A, peer.send(&Voice::ConnectionCheck)),
            [(A, Heard::ConnectionCheckAck)]
        );
        assert!(world.core.is_connected(A));
        assert_eq!(events(&mut world), [Event::Connected(A)]);
        // Checking again is answered and is not a new connection.
        assert_eq!(
            world.send(A, peer.send(&Voice::ConnectionCheck)),
            [(A, Heard::ConnectionCheckAck)]
        );
        assert_eq!(events(&mut world), []);
        // From another address it is nothing.
        assert!(
            world
                .core
                .handle(world.now, addr(77), &peer.send(&Voice::ConnectionCheck))
                .is_empty()
        );
    }

    #[test]
    fn a_client_that_comes_from_another_address_has_to_check_again() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        let peer = world.peer(A);
        let moved = addr(55);
        let out = world.core.handle(world.now, moved, &peer.authenticate());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, moved);
        assert!(!world.core.is_connected(A));
        let out = world
            .core
            .handle(world.now, moved, &peer.send(&Voice::ConnectionCheck));
        assert_eq!(out.len(), 1);
        assert!(world.core.is_connected(A));
    }

    #[test]
    fn the_ping_that_tests_a_connection_is_echoed_when_pings_are_allowed() {
        let ping = 0x58bc_9ae9_c7a8_45e4_a11c_efbb_6719_9425u128;
        let payload = [7u8; 24];
        let datagram = proto::datagram_to_server(ping, &payload);
        let mut core = Core::new(Config::default());
        let out = core.handle(Instant::now(), addr(1), &datagram);
        assert_eq!(
            out,
            [Outgoing {
                to: addr(1),
                bytes: payload.to_vec()
            }]
        );
        // Longer than a ping is not one; and pings can be turned off.
        assert!(
            core.handle(
                Instant::now(),
                addr(1),
                &proto::datagram_to_server(ping, &[7u8; 25])
            )
            .is_empty()
        );
        let mut core = Core::new(Config {
            allow_pings: false,
            ..Config::default()
        });
        assert!(core.handle(Instant::now(), addr(1), &datagram).is_empty());
    }
}

#[cfg(test)]
mod hearing {
    use super::*;

    fn player_sound(sender: u128, distance: f32, whispering: bool) -> Heard {
        Heard::Player {
            channel: sender,
            sender,
            data: vec![1, 2, 3],
            sequence: 7,
            distance,
            whispering,
        }
    }

    #[test]
    fn a_player_is_heard_by_the_players_near_them_and_not_by_themselves_or_far_ones() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0, 64.0, 0.0]);
        world.talker(B, "Bob", [10.0, 64.0, 0.0]);
        world.talker(C, "Cat", [48.5, 64.0, 0.0]);
        world.talker(D, "Dan", [49.5, 64.0, 0.0]);
        let heard = world.speak(A, &[1, 2, 3]);
        // Up to a block past the distance (49): the client fades the voice by the distance sent.
        assert_eq!(
            heard,
            [
                (B, player_sound(A, 48.0, false)),
                (C, player_sound(A, 48.0, false))
            ]
        );
    }

    #[test]
    fn the_sequence_number_and_the_frame_pass_through_unchanged() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [1.0, 0.0, 0.0]);
        let frame = vec![9u8; 200];
        let datagram = world.peer(A).mic(&frame, -5, false);
        let heard = world.send(A, datagram);
        assert_eq!(
            heard,
            [(
                B,
                Heard::Player {
                    channel: A,
                    sender: A,
                    data: frame,
                    sequence: -5,
                    distance: 48.0,
                    whispering: false
                }
            )]
        );
    }

    #[test]
    fn a_whisper_is_marked_and_carries_the_whisper_distance() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [20.0, 0.0, 0.0]);
        world.talker(C, "Cat", [40.0, 0.0, 0.0]);
        let heard = world.whisper(A, &[1, 2, 3]);
        // The same reach, a block past the full distance; what changes is the distance the
        // clients fade by.
        assert_eq!(
            heard,
            [
                (B, player_sound(A, 24.0, true)),
                (C, player_sound(A, 24.0, true))
            ]
        );
    }

    #[test]
    fn the_distances_come_from_the_config() {
        let mut world = World::with(Config {
            max_voice_distance: 10.0,
            whisper_distance: 5.0,
            ..Config::default()
        });
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [10.5, 0.0, 0.0]);
        world.talker(C, "Cat", [12.0, 0.0, 0.0]);
        assert_eq!(
            world.speak(A, &[1, 2, 3]),
            [(B, player_sound(A, 10.0, false))]
        );
        assert_eq!(
            world.whisper(A, &[1, 2, 3]),
            [(B, player_sound(A, 5.0, true))]
        );
    }

    #[test]
    fn distance_counts_in_three_dimensions_and_only_inside_a_dimension() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0, 64.0, 0.0]);
        world.talker(B, "Bob", [0.0, 64.0, 30.0]);
        world.talker(C, "Cat", [0.0, 120.0, 0.0]);
        world.talker(D, "Dan", [0.0, 64.0, 3.0]);
        world.set_dimension(D, NETHER);
        // Cat is 56 blocks above: out of reach. Dan is near but in the Nether.
        assert_eq!(who(&world.speak(A, &[1, 2, 3])), [B]);
    }

    #[test]
    fn a_player_who_moved_is_heard_where_they_are_now() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [200.0, 0.0, 0.0]);
        assert_eq!(world.speak(A, &[1, 2, 3]), []);
        world.set_position(B, [5.0, 0.0, 0.0]);
        assert_eq!(who(&world.speak(A, &[1, 2, 3])), [B]);
    }

    #[test]
    fn each_listener_gets_a_datagram_only_they_can_open() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [1.0, 0.0, 0.0]);
        world.talker(C, "Cat", [2.0, 0.0, 0.0]);
        let datagram = world.peer(A).mic(&[1, 2, 3], 7, false);
        let out = world.core.handle(world.now, world.peer(A).addr, &datagram);
        assert_eq!(out.len(), 2);
        let b = out.iter().find(|o| o.to == world.peer(B).addr).unwrap();
        assert!(open_datagram(&world.peer(B).secret, &b.bytes).is_some());
        assert!(open_datagram(&world.peer(C).secret, &b.bytes).is_none());
        // The sender's secret opens nothing of what the others get.
        assert!(open_datagram(&world.peer(A).secret, &b.bytes).is_none());
    }

    #[test]
    fn nobody_is_heard_who_is_not_connected_and_nobody_hears_who_is_not() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [1.0, 0.0, 0.0]);
        // Cat has a secret and is on the server but has no voice connection.
        world.player(C, "Cat", [2.0, 0.0, 0.0]);
        assert_eq!(who(&world.speak(A, &[1, 2, 3])), [B]);
        let datagram = world.peer(C).mic(&[1, 2, 3], 1, false);
        assert!(
            world
                .core
                .handle(world.now, world.peer(C).addr, &datagram)
                .is_empty()
        );
        // From another address, a connected player's frame is no one's.
        let datagram = world.peer(A).mic(&[1, 2, 3], 1, false);
        assert!(world.core.handle(world.now, addr(88), &datagram).is_empty());
    }

    #[test]
    fn a_player_with_voice_chat_off_hears_nothing_and_a_disconnected_one_neither() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [1.0, 0.0, 0.0]);
        world.talker(C, "Cat", [2.0, 0.0, 0.0]);
        world.core.update_state(B, true);
        assert_eq!(who(&world.speak(A, &[1, 2, 3])), [C]);
        // They still speak when they have it off? A client that turned it off does not send.
        world.core.update_state(B, false);
        world.core.voice_disconnected(C);
        assert_eq!(who(&world.speak(A, &[1, 2, 3])), [B]);
    }

    #[test]
    fn a_spectator_is_heard_only_by_spectators_unless_the_config_says_otherwise() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [1.0, 0.0, 0.0]);
        world.talker(C, "Cat", [2.0, 0.0, 0.0]);
        world.set_spectator(A, true);
        world.set_spectator(C, true);
        // The living do not hear the dead; the dead hear the dead, and the living.
        assert_eq!(who(&world.speak(A, &[1, 2, 3])), [C]);
        assert_eq!(who(&world.speak(B, &[1, 2, 3])), [A, C]);
        world.core.config.spectators_talk_to_players = true;
        assert_eq!(who(&world.speak(A, &[1, 2, 3])), [B, C]);
    }

    #[test]
    fn a_group_hears_its_members_wherever_they_are_and_others_do_not() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [5000.0, 0.0, 0.0]);
        world.talker(C, "Cat", [1.0, 0.0, 0.0]);
        world
            .core
            .create_group(A, "Friends".into(), None, GroupKind::Normal);
        let group = world.core.state_of(A).unwrap().group.unwrap();
        world.core.join_group(B, group, None);
        let heard = world.speak(A, &[1, 2, 3]);
        // Bob, far away, hears the group; Cat, next to Alice, hears nothing of a closed group.
        assert_eq!(
            heard,
            [(
                B,
                Heard::Group {
                    channel: A,
                    sender: A,
                    data: vec![1, 2, 3],
                    sequence: 7
                }
            )]
        );
    }

    #[test]
    fn an_open_group_is_also_heard_by_proximity_but_a_member_next_to_the_speaker_hears_it_once() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [5000.0, 0.0, 0.0]);
        world.talker(C, "Cat", [1.0, 0.0, 0.0]);
        world.talker(D, "Dan", [2.0, 0.0, 0.0]);
        world
            .core
            .create_group(A, "Friends".into(), None, GroupKind::Open);
        let group = world.core.state_of(A).unwrap().group.unwrap();
        world.core.join_group(B, group, None);
        world.core.join_group(C, group, None);
        let heard = world.speak(A, &[1, 2, 3]);
        let kinds: Vec<(u128, bool)> = heard
            .iter()
            .map(|(u, h)| (*u, matches!(h, Heard::Group { .. })))
            .collect();
        // Members B and C hear it as the group's; Dan, next to Alice, by proximity. Nobody twice.
        assert_eq!(kinds, [(B, true), (C, true), (D, false)]);
    }

    #[test]
    fn an_isolated_group_hears_only_itself() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [1.0, 0.0, 0.0]);
        world.talker(C, "Cat", [2.0, 0.0, 0.0]);
        world
            .core
            .create_group(B, "Quiet".into(), None, GroupKind::Isolated);
        let group = world.core.state_of(B).unwrap().group.unwrap();
        // Alice, outside, speaks next to Bob: Bob, in an isolated group, does not hear her.
        assert_eq!(who(&world.speak(A, &[1, 2, 3])), [C]);
        world.core.join_group(C, group, None);
        // Bob speaks: Cat hears the group, Alice nothing (it is not open).
        assert_eq!(who(&world.speak(B, &[1, 2, 3])), [C]);
        assert!(matches!(
            world.speak(B, &[1, 2, 3])[0].1,
            Heard::Group { .. }
        ));
    }

    #[test]
    fn a_group_member_who_turned_voice_chat_off_does_not_hear_the_group() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [9000.0, 0.0, 0.0]);
        world
            .core
            .create_group(A, "Friends".into(), None, GroupKind::Normal);
        let group = world.core.state_of(A).unwrap().group.unwrap();
        world.core.join_group(B, group, None);
        world.core.update_state(B, true);
        assert_eq!(world.speak(A, &[1, 2, 3]), []);
    }
}

#[cfg(test)]
mod keeping_alive {
    use super::*;

    #[test]
    fn every_connection_is_sent_a_keep_alive_and_answering_keeps_it() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [0.0; 3]);
        // Cat has authenticated and not been checked: no keep-alive yet.
        world.player(C, "Cat", [0.0; 3]);
        let peer = world.peer(C);
        world.send(C, peer.authenticate());
        let out = world.core.keep_alives(world.now);
        let heard = world.inbox(out);
        assert_eq!(heard, [(A, Heard::KeepAlive), (B, Heard::KeepAlive)]);
    }

    #[test]
    fn a_connection_that_stops_answering_is_dropped_after_ten_intervals_and_told_to_the_game() {
        let mut world = World::new();
        world.talker(A, "Alice", [0.0; 3]);
        world.talker(B, "Bob", [0.0; 3]);
        events(&mut world);
        let start = world.now;
        // Bob answers all along; Alice does not.
        let mut now = start;
        for second in 1..=9 {
            now = start + Duration::from_secs(second);
            let datagram = world.peer(B).send(&Voice::KeepAlive);
            world.core.handle(now, world.peer(B).addr, &datagram);
            assert!(!world.core.keep_alives(now).is_empty());
            assert_eq!(events(&mut world), []);
        }
        let _ = now;
        let later = start + Duration::from_secs(10);
        let datagram = world.peer(B).send(&Voice::KeepAlive);
        world.core.handle(later, world.peer(B).addr, &datagram);
        let out = world.core.keep_alives(later);
        assert_eq!(events(&mut world), [Event::TimedOut(A)]);
        // Alice has no connection and no secret now; Bob is still sent his keep-alive.
        assert!(!world.core.is_connected(A) && !world.core.has_secret(A));
        assert!(world.core.is_connected(B));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, world.peer(B).addr);
        // What she sends now is no one's.
        let datagram = world.peer(A).mic(&[1], 1, false);
        assert!(
            world
                .core
                .handle(later, world.peer(A).addr, &datagram)
                .is_empty()
        );
    }

    #[test]
    fn a_connection_that_was_never_checked_is_forgotten() {
        let mut world = World::new();
        world.player(A, "Alice", [0.0; 3]);
        let peer = world.peer(A);
        world.send(A, peer.authenticate());
        world.core.keep_alives(world.now + Duration::from_secs(61));
        // The check that comes too late is no connection.
        let out = world
            .core
            .handle(world.now, peer.addr, &peer.send(&Voice::ConnectionCheck));
        assert!(out.is_empty());
        assert!(!world.core.is_connected(A));
    }
}

#[cfg(test)]
mod controlling {
    use super::*;

    fn sent_to(messages: &[Message], uuid: u128) -> Vec<&'static str> {
        messages
            .iter()
            .filter(|m| m.to == uuid)
            .map(|m| m.channel)
            .collect()
    }

    fn world() -> World {
        let mut world = World::new();
        world.player(A, "Alice", [0.0; 3]);
        world.player(B, "Bob", [0.0; 3]);
        world.player(C, "Cat", [0.0; 3]);
        world
    }

    #[test]
    fn turning_voice_chat_off_is_told_to_everyone_with_the_mod() {
        let mut world = world();
        let out = world.core.update_state(A, true);
        assert_eq!(sent_to(&out, A), [channel::STATE]);
        assert_eq!(sent_to(&out, B), [channel::STATE]);
        assert_eq!(sent_to(&out, C), [channel::STATE]);
        assert_eq!(out[0].data[0], 1, "disabled");
        assert!(world.core.state_of(A).unwrap().disabled);
        // Connecting clears it for the others, too.
        let out = world.core.voice_connected(A);
        assert_eq!(out[0].data[1], 0, "no longer disconnected");
    }

    #[test]
    fn a_group_is_made_announced_and_joined_by_its_maker() {
        let mut world = world();
        let out = world
            .core
            .create_group(A, "Friends".into(), Some("pw".into()), GroupKind::Open);
        let group = world.core.state_of(A).unwrap().group.unwrap();
        // Everyone is told of the group, then of Alice's state, and Alice that she is in it.
        let order: Vec<(u128, &str)> = out.iter().map(|m| (m.to, m.channel)).collect();
        assert_eq!(
            order,
            [
                (A, channel::ADD_GROUP),
                (B, channel::ADD_GROUP),
                (C, channel::ADD_GROUP),
                (A, channel::STATE),
                (B, channel::STATE),
                (C, channel::STATE),
                (A, channel::JOINED_GROUP),
            ]
        );
        let joined = out.last().unwrap();
        let mut expected = vec![1];
        expected.extend(group.to_be_bytes());
        expected.push(0);
        assert_eq!(joined.data, expected);
        let g = world.core.group(group).unwrap();
        assert_eq!(
            (g.name.as_str(), g.password.as_deref(), g.kind),
            ("Friends", Some("pw"), GroupKind::Open)
        );
    }

    #[test]
    fn only_text_a_group_may_have_makes_one() {
        let mut world = world();
        for name in [
            "",
            " leading",
            "a\u{0}b",
            "x".repeat(25).as_str(),
            "\u{200b}zero",
        ] {
            assert_eq!(
                world
                    .core
                    .create_group(A, name.into(), None, GroupKind::Normal),
                [],
                "{name:?}"
            );
        }
        assert_eq!(
            world
                .core
                .create_group(A, "ok".into(), Some(" bad".into()), GroupKind::Normal),
            []
        );
        assert_eq!(world.core.groups().count(), 0);
        assert!(
            !world
                .core
                .create_group(A, "Fine name 1".into(), None, GroupKind::Normal)
                .is_empty()
        );
        assert!(crate::core::valid_group_text("x"));
        assert!(crate::core::valid_group_text(&"x".repeat(24)));
        assert!(crate::core::valid_group_text("a b"));
        assert!(!crate::core::valid_group_text("a\tb"));
    }

    #[test]
    fn joining_needs_the_password_and_a_group_that_exists() {
        let mut world = world();
        world
            .core
            .create_group(A, "Secret".into(), Some("pw".into()), GroupKind::Normal);
        let group = world.core.state_of(A).unwrap().group.unwrap();
        // The wrong or no password: told, and not in.
        let out = world.core.join_group(B, group, Some("no".into()));
        assert_eq!(out.len(), 1);
        assert_eq!(
            (out[0].to, out[0].channel, out[0].data.clone()),
            (B, channel::JOINED_GROUP, vec![0, 1])
        );
        assert_eq!(world.core.join_group(B, group, None)[0].data, [0, 1]);
        assert_eq!(world.core.state_of(B).unwrap().group, None);
        // A group that is not there: told with no group and no wrong password.
        let out = world.core.join_group(B, 0x1234, None);
        assert_eq!(out[0].data, [0, 0]);
        // The right password.
        let out = world.core.join_group(B, group, Some("pw".into()));
        assert_eq!(sent_to(&out, B).last(), Some(&channel::JOINED_GROUP));
        assert_eq!(world.core.state_of(B).unwrap().group, Some(group));
    }

    #[test]
    fn leaving_a_group_tells_the_player_and_removes_a_group_nobody_is_in() {
        let mut world = world();
        world
            .core
            .create_group(A, "Friends".into(), None, GroupKind::Normal);
        let group = world.core.state_of(A).unwrap().group.unwrap();
        world.core.join_group(B, group, None);
        // Alice leaves: Bob is still in it, so it stays.
        let out = world.core.leave_group(A);
        assert!(!out.iter().any(|m| m.channel == channel::REMOVE_GROUP));
        assert_eq!(out.last().unwrap().data, [0, 0]);
        assert_eq!(world.core.state_of(A).unwrap().group, None);
        // Bob leaves: nobody is, and everyone is told it is gone.
        let out = world.core.leave_group(B);
        let gone: Vec<u128> = out
            .iter()
            .filter(|m| m.channel == channel::REMOVE_GROUP)
            .map(|m| m.to)
            .collect();
        assert_eq!(gone, [A, B, C]);
        assert!(world.core.group(group).is_none());
    }

    #[test]
    fn a_player_who_leaves_the_server_leaves_their_group_and_is_forgotten() {
        let mut world = world();
        world
            .core
            .create_group(A, "Friends".into(), None, GroupKind::Normal);
        let group = world.core.state_of(A).unwrap().group.unwrap();
        world.talker(D, "Dan", [0.0; 3]);
        let out = world.core.player_left(A);
        // The others are told Alice is gone, and then that the group she was alone in is.
        let order: Vec<(u128, &str)> = out.iter().map(|m| (m.to, m.channel)).collect();
        assert_eq!(
            order,
            [
                (B, channel::REMOVE_STATE),
                (C, channel::REMOVE_STATE),
                (D, channel::REMOVE_STATE),
                (B, channel::REMOVE_GROUP),
                (C, channel::REMOVE_GROUP),
                (D, channel::REMOVE_GROUP),
            ]
        );
        assert!(world.core.group(group).is_none());
        assert!(world.core.state_of(A).is_none() && !world.core.has_secret(A));
        assert!(!world.core.is_connected(A) && !world.core.is_compatible(A));
        // Nothing of hers is heard or sent any more.
        assert_eq!(world.core.update_state(A, true), []);
    }

    #[test]
    fn groups_can_be_turned_off() {
        let mut world = World::with(Config {
            groups: false,
            ..Config::default()
        });
        world.player(A, "Alice", [0.0; 3]);
        assert_eq!(
            world
                .core
                .create_group(A, "Friends".into(), None, GroupKind::Normal),
            []
        );
        assert_eq!(world.core.join_group(A, 1, None), []);
    }

    #[test]
    fn a_player_who_changes_name_is_announced_again_and_one_who_moves_is_not() {
        let mut world = world();
        let out = world
            .core
            .refresh(vec![(A, info("Alice", [100.0, 0.0, 0.0]))]);
        assert_eq!(out, []);
        let out = world
            .core
            .refresh(vec![(A, info("Captain", [100.0, 0.0, 0.0]))]);
        assert_eq!(sent_to(&out, B), [channel::STATE]);
        let mut r = Reader::new(&out[0].data);
        assert_eq!(
            (r.bool(), r.bool(), r.uuid()),
            (Some(false), Some(true), Some(A))
        );
        assert_eq!(r.utf(256).as_deref(), Some("Captain"));
    }

    #[test]
    fn a_player_may_send_sixteen_packets_a_second() {
        let mut world = world();
        let now = Instant::now();
        assert!((0..16).all(|_| world.core.allow(A, now)));
        assert!(!world.core.allow(A, now));
        assert!(!world.core.allow(A, now + Duration::from_millis(900)));
        assert!(world.core.allow(A, now + Duration::from_secs(1)));
        // Another player has their own count.
        assert!(world.core.allow(B, now));
    }
}

#[cfg(test)]
mod in_the_game {
    use super::*;

    fn rig() -> Rig {
        let mut rig = Rig::new("");
        rig.join("Alice", A, 0);
        rig.join("Bob", B, 0);
        rig.tick();
        rig
    }

    #[test]
    fn the_voice_server_starts_with_the_first_tick_and_stops_with_the_server() {
        let mut rig = rig();
        let address = rig.voice.address().expect("the voice server runs");
        assert!(address.port() != 0);
        assert!(
            rig.host
                .logged()
                .iter()
                .any(|l| l.starts_with("INFO: Voice chat server started at 127.0.0.1"))
        );
        rig.stop();
        assert!(rig.voice.address().is_none());
        // The port is free again.
        assert!(UdpSocket::bind(address).is_ok());
    }

    #[test]
    fn a_client_that_asks_is_sent_the_states_and_its_secret_and_the_others_only_after() {
        let mut rig = rig();
        rig.host.take_sent(A);
        rig.request_secret(A, 20);
        let sent = rig.host.take_sent(A);
        let channels: Vec<&str> = sent.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(channels, [channel::STATES, channel::SECRET]);
        // Bob has not asked: he is told nothing of it.
        assert!(rig.host.take_sent(B).is_empty());
        // The port in the secret is the one the server listens on.
        let address = rig.voice.address().unwrap();
        let mut r = Reader::new(&sent[1].1[SECRET_LEN..]);
        assert_eq!(r.i32(), Some(i32::from(address.port())));
        assert_eq!(r.uuid(), Some(A));
    }

    #[test]
    fn a_client_of_another_version_is_told_in_chat_and_gets_nothing_else() {
        let mut rig = rig();
        rig.request_secret(A, 21);
        assert!(rig.host.take_sent(A).is_empty());
        assert_eq!(rig.host.told.len(), 1);
        assert_eq!(rig.host.told[0].0, A);
        assert_eq!(
            rig.host.told[0].1.plain(),
            "Your voice chat client version is not compatible with the server-side version.
Please install version 2.6.x of Simple Voice Chat."
        );
        assert!(
            rig.host
                .logged()
                .iter()
                .any(|l| l.contains("not compatible"))
        );
    }

    #[test]
    fn what_is_not_ours_or_is_broken_is_ignored() {
        let mut rig = rig();
        rig.plugin(A, "minecraft:brand", b"vanilla");
        rig.plugin(A, "voicechat:request_secret", &[1, 2]);
        rig.plugin(A, "voicechat:unknown", &[1, 2]);
        rig.plugin(0x99, "voicechat:request_secret", &20i32.to_be_bytes());
        assert!(rig.host.sent.is_empty() && rig.host.told.is_empty());
    }

    #[test]
    fn a_player_with_a_nickname_is_known_by_it_not_by_their_name() {
        let mut rig = rig();
        rig.player(A).nickname = Some("Captain".to_owned());
        rig.request_secret(B, 20);
        rig.host.take_sent(B);
        // The nickname is noticed at the tick and told to those with the mod.
        rig.tick();
        let sent = rig.host.take_sent(B);
        assert_eq!(sent.len(), 1);
        let mut r = Reader::new(&sent[0].1);
        r.bool();
        r.bool();
        assert_eq!(r.uuid(), Some(A));
        assert_eq!(r.utf(256).as_deref(), Some("Captain"));
        // Bob asking now is sent Captain, never Alice.
        rig.request_secret(A, 20);
        let first = rig.host.take_sent(A);
        assert!(!first[0].1.windows(5).any(|w| w == b"Alice"));
    }

    #[test]
    fn groups_are_made_and_joined_over_the_game_connection() {
        let mut rig = rig();
        rig.request_secret(A, 20);
        rig.request_secret(B, 20);
        rig.host.sent.clear();
        rig.plugin(
            A,
            channel::CREATE_GROUP,
            &create_group_bytes("Friends", None, 0),
        );
        let to_b = rig.host.take_sent(B);
        let channels: Vec<&str> = to_b.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(channels, [channel::ADD_GROUP, channel::STATE]);
        let group = {
            let mut r = Reader::new(&to_b[0].1);
            r.uuid().unwrap()
        };
        rig.host.sent.clear();
        rig.plugin(B, channel::SET_GROUP, &join_group_bytes(group, None));
        let to_b = rig.host.take_sent(B);
        assert_eq!(to_b.last().unwrap().0, channel::JOINED_GROUP);
        rig.plugin(B, channel::LEAVE_GROUP, &[]);
        rig.plugin(A, channel::LEAVE_GROUP, &[]);
        assert!(
            rig.host
                .sent
                .iter()
                .any(|(_, c, _)| c == channel::REMOVE_GROUP)
        );
    }

    #[test]
    fn a_player_who_floods_the_connection_is_ignored() {
        let mut rig = rig();
        for _ in 0..40 {
            rig.plugin(A, channel::UPDATE_STATE, &[0]);
        }
        // Only the first sixteen counted: none of them is from a client that asked, so it is
        // silence; the log says what was dropped.
        assert!(
            rig.host
                .logged()
                .iter()
                .any(|l| l.contains("too many in a second"))
        );
    }

    #[test]
    fn a_player_who_leaves_is_removed_for_the_others() {
        let mut rig = rig();
        rig.request_secret(A, 20);
        rig.request_secret(B, 20);
        rig.host.sent.clear();
        rig.leave(B);
        let to_a = rig.host.take_sent(A);
        assert_eq!(
            to_a,
            [(channel::REMOVE_STATE.to_owned(), B.to_be_bytes().to_vec())]
        );
    }

    #[test]
    fn the_voice_server_can_be_turned_off_in_the_config() {
        let mut rig = Rig::new("enabled = false\n");
        rig.join("Alice", A, 0);
        rig.tick();
        rig.request_secret(A, 20);
        assert!(rig.voice.address().is_none());
        assert!(rig.host.sent.is_empty());
        assert!(rig.host.logged().iter().any(|l| l.contains("turned off")));
    }

    #[test]
    fn a_port_that_is_taken_is_logged_and_nothing_runs() {
        let taken = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let mut rig = Rig::raw(&format!("port = {port}\nbind-address = \"127.0.0.1\"\n"));
        rig.join("Alice", A, 0);
        rig.tick();
        assert!(rig.voice.address().is_none());
        assert!(
            rig.host
                .logged()
                .iter()
                .any(|l| l.starts_with("ERROR: Voice chat: could not start"))
        );
        rig.request_secret(A, 20);
        assert!(rig.host.sent.is_empty());
    }

    #[test]
    fn the_status_command_says_who_has_the_mod_and_is_connected() {
        let mut rig = rig();
        assert!(rig.say(A, "voicechat status"));
        assert!(rig.host.feedback[0].starts_with("The voice chat server listens on UDP port "));
        assert_eq!(rig.host.feedback[2], "0 with the mod, 0 connected");
        rig.request_secret(A, 20);
        rig.host.feedback.clear();
        assert!(rig.console("voicechat status"));
        assert_eq!(rig.host.feedback[2], "1 with the mod, 0 connected");
        assert_eq!(rig.host.feedback[3], "Alice: not connected");
        let commands = rig.register();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].level, 2);
        assert_eq!(commands[0].permission.as_deref(), Some("voicechat.admin"));
    }

    /// Two players talk through the real voice server over real sockets, as the Simple Voice Chat
    /// mod does: ask for a secret, authenticate, check the connection, then speak.
    #[test]
    fn two_clients_talk_through_the_real_socket() {
        let mut rig = rig();
        let address = rig.voice.address().unwrap();
        rig.request_secret(A, 20);
        rig.request_secret(B, 20);
        let secret = |rig: &mut Rig, uuid| -> Secret {
            let sent = rig.host.take_sent(uuid);
            let (_, data) = sent.iter().find(|(c, _)| c == channel::SECRET).unwrap();
            data[..SECRET_LEN].try_into().unwrap()
        };
        let alice = Client::new(address, A, secret(&mut rig, A));
        let bob = Client::new(address, B, secret(&mut rig, B));
        for client in [&alice, &bob] {
            client.send(&client.peer.authenticate());
            assert_eq!(client.hear(), Some(Heard::AuthenticateAck));
            client.send(&client.peer.send(&Voice::ConnectionCheck));
            assert_eq!(client.hear(), Some(Heard::ConnectionCheckAck));
        }
        // The game hears the connections at its next tick, and tells everyone.
        rig.host.sent.clear();
        rig.tick();
        let to_a = rig.host.take_sent(A);
        assert_eq!(
            to_a.iter().filter(|(c, _)| c == channel::STATE).count(),
            2,
            "{to_a:?}"
        );
        // Bob stands next to Alice and hears her.
        rig.player(B).position = [3.0, 64.0, 0.0];
        rig.tick();
        alice.send(&alice.peer.mic(&[5, 6, 7], 42, false));
        assert_eq!(
            bob.hear_sound(),
            Some(Heard::Player {
                channel: A,
                sender: A,
                data: vec![5, 6, 7],
                sequence: 42,
                distance: 48.0,
                whispering: false
            })
        );
        // And Alice, who spoke, hears nothing of it; Bob far away is not heard.
        alice
            .socket
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        assert_eq!(alice.hear_sound(), None);
        rig.player(B).position = [500.0, 64.0, 0.0];
        rig.tick();
        alice.send(&alice.peer.mic(&[1], 43, false));
        bob.socket
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        assert_eq!(bob.hear_sound(), None);
    }
}
