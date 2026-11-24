//! The packets of Simple Voice Chat's compatibility version 20 (the 2.6 releases), as a server
//! sends and reads them.
//!
//! The game connection carries the control packets as plugin messages on `voicechat:` channels:
//! the client asks for a secret, the server answers with it and the player states and groups, and
//! the client says whether voice chat is off for it and what groups it joins. The voice itself goes
//! over UDP, each datagram encrypted with the player's secret.

use crate::buf::{Reader, Writer};
use crate::crypt::{SECRET_LEN, Secret};

/// The compatibility version this server speaks. A client that asks with another gets no secret.
pub const COMPATIBILITY: i32 = 20;

/// The longest group name, and password, a client may send.
pub const MAX_GROUP_NAME: usize = 24;
/// The longest an opus frame is.
pub const MAX_OPUS: usize = 1275;
/// The most a datagram's payload may hold.
pub const MAX_DATAGRAM_PAYLOAD: usize = 2048;
/// Every datagram begins with this byte.
pub const MAGIC: u8 = 0xff;

pub mod channel {
    pub const REQUEST_SECRET: &str = "voicechat:request_secret";
    pub const SECRET: &str = "voicechat:secret";
    pub const UPDATE_STATE: &str = "voicechat:update_state";
    pub const STATE: &str = "voicechat:state";
    pub const STATES: &str = "voicechat:states";
    pub const REMOVE_STATE: &str = "voicechat:remove_state";
    pub const ADD_GROUP: &str = "voicechat:add_group";
    pub const REMOVE_GROUP: &str = "voicechat:remove_group";
    pub const SET_GROUP: &str = "voicechat:set_group";
    pub const CREATE_GROUP: &str = "voicechat:create_group";
    pub const LEAVE_GROUP: &str = "voicechat:leave_group";
    pub const JOINED_GROUP: &str = "voicechat:joined_group";
}

/// How a group hears the players around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    /// Its members talk to each other and are not heard outside.
    Normal,
    /// Its members talk to each other and to the players near them.
    Open,
    /// Its members hear only each other.
    Isolated,
}

impl GroupKind {
    fn from_wire(v: i16) -> Self {
        match v {
            1 => Self::Open,
            2 => Self::Isolated,
            _ => Self::Normal,
        }
    }

    fn to_wire(self) -> i16 {
        match self {
            Self::Normal => 0,
            Self::Open => 1,
            Self::Isolated => 2,
        }
    }
}

/// How the client should encode what it records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Voip,
    Audio,
    RestrictedLowDelay,
}

impl Codec {
    pub fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "voip" => Some(Self::Voip),
            "audio" => Some(Self::Audio),
            "restricted_lowdelay" | "restricted-lowdelay" => Some(Self::RestrictedLowDelay),
            _ => None,
        }
    }

    fn to_wire(self) -> u8 {
        match self {
            Self::Voip => 0,
            Self::Audio => 1,
            Self::RestrictedLowDelay => 2,
        }
    }
}

/// What a client sends over the game connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inbound {
    /// The client has the mod, at this compatibility version, and wants a secret.
    RequestSecret(i32),
    /// Voice chat is off for the client (or on again).
    UpdateState {
        disabled: bool,
    },
    JoinGroup {
        group: u128,
        password: Option<String>,
    },
    CreateGroup {
        name: String,
        password: Option<String>,
        kind: GroupKind,
    },
    LeaveGroup,
}

/// What `data` on plugin channel `channel` says: none for a channel that is not a client's packet of
/// this protocol, `Some(None)` for one that is badly formed.
pub fn read_inbound(channel: &str, data: &[u8]) -> Option<Option<Inbound>> {
    let mut r = Reader::new(data);
    Some(match channel {
        channel::REQUEST_SECRET => r.i32().map(Inbound::RequestSecret),
        channel::UPDATE_STATE => r.bool().map(|disabled| Inbound::UpdateState { disabled }),
        channel::SET_GROUP => (|| {
            let group = r.uuid()?;
            let password = if r.bool()? { Some(r.utf(512)?) } else { None };
            Some(Inbound::JoinGroup { group, password })
        })(),
        channel::CREATE_GROUP => (|| {
            let name = r.utf(MAX_GROUP_NAME)?;
            let password = if r.bool()? {
                Some(r.utf(MAX_GROUP_NAME)?)
            } else {
                None
            };
            let kind = GroupKind::from_wire(r.i16()?);
            Some(Inbound::CreateGroup {
                name,
                password,
                kind,
            })
        })(),
        channel::LEAVE_GROUP => Some(Inbound::LeaveGroup),
        _ => return None,
    })
}

/// A player as the clients are told of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerState {
    pub uuid: u128,
    /// The name the others know them by: their nickname when they have one.
    pub name: String,
    /// Voice chat is off for them: they hear nothing.
    pub disabled: bool,
    /// They have no voice connection.
    pub disconnected: bool,
    pub group: Option<u128>,
}

impl PlayerState {
    pub fn write(&self, w: &mut Writer) {
        w.bool(self.disabled)
            .bool(self.disconnected)
            .uuid(self.uuid)
            .utf(&self.name)
            .bool(self.group.is_some());
        if let Some(group) = self.group {
            w.uuid(group);
        }
    }
}

pub fn state(state: &PlayerState) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    state.write(&mut w);
    (channel::STATE, w.finish())
}

pub fn states(states: &[PlayerState]) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    w.i32(states.len() as i32);
    for s in states {
        s.write(&mut w);
    }
    (channel::STATES, w.finish())
}

pub fn remove_state(uuid: u128) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    w.uuid(uuid);
    (channel::REMOVE_STATE, w.finish())
}

/// A group as the clients are told of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientGroup {
    pub id: u128,
    pub name: String,
    pub has_password: bool,
    pub kind: GroupKind,
}

pub fn add_group(group: &ClientGroup) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    // Not persistent, not hidden: groups here are made by players.
    w.uuid(group.id)
        .utf(&group.name)
        .bool(group.has_password)
        .bool(false)
        .bool(false)
        .i16(group.kind.to_wire());
    (channel::ADD_GROUP, w.finish())
}

pub fn remove_group(id: u128) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    w.uuid(id);
    (channel::REMOVE_GROUP, w.finish())
}

/// The answer to a client asking to join or create a group, or to leave: the group they are in now,
/// or none, and whether the password was wrong.
pub fn joined_group(group: Option<u128>, wrong_password: bool) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    w.bool(group.is_some());
    if let Some(group) = group {
        w.uuid(group);
    }
    w.bool(wrong_password);
    (channel::JOINED_GROUP, w.finish())
}

/// What a client needs to open its voice connection.
#[derive(Clone, Debug, PartialEq)]
pub struct SecretInfo {
    pub secret: Secret,
    pub port: i32,
    pub player: u128,
    pub codec: Codec,
    pub mtu: i32,
    pub distance: f64,
    pub keep_alive: i32,
    pub groups: bool,
    /// Where the client sends its voice, as `host`, `host:port` or `port`; empty for the address it
    /// plays on.
    pub host: String,
    pub allow_recording: bool,
}

pub fn secret(info: &SecretInfo) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    w.raw(&info.secret)
        .i32(info.port)
        .uuid(info.player)
        .u8(info.codec.to_wire())
        .i32(info.mtu)
        .f64(info.distance)
        .i32(info.keep_alive)
        .bool(info.groups)
        .utf(&info.host)
        .bool(info.allow_recording);
    (channel::SECRET, w.finish())
}

// ---- the voice datagrams ---------------------------------------------------------------------

const MIC: u8 = 0x1;
const PLAYER_SOUND: u8 = 0x2;
const GROUP_SOUND: u8 = 0x3;
const AUTHENTICATE: u8 = 0x5;
const AUTHENTICATE_ACK: u8 = 0x6;
const PING: u8 = 0x7;
const KEEP_ALIVE: u8 = 0x8;
const CONNECTION_CHECK: u8 = 0x9;
const CONNECTION_CHECK_ACK: u8 = 0xa;

const WHISPER_FLAG: u8 = 0b1;

/// What a client sends over UDP, once its datagram is decrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Voice {
    /// A frame of what the player says.
    Mic {
        data: Vec<u8>,
        sequence: i64,
        whispering: bool,
    },
    Authenticate {
        player: u128,
        secret: Secret,
    },
    KeepAlive,
    ConnectionCheck,
    /// A ping the server asked for; nothing here asks, so it is read and left.
    Ping,
}

impl Voice {
    /// What the decrypted `payload` of a datagram holds, none when it is not a packet a client
    /// sends.
    pub fn read(payload: &[u8]) -> Option<Self> {
        let mut r = Reader::new(payload);
        Some(match r.u8()? {
            MIC => Self::Mic {
                data: r.byte_array(MAX_OPUS)?.to_vec(),
                sequence: r.i64()?,
                whispering: r.bool()?,
            },
            AUTHENTICATE => Self::Authenticate {
                player: r.uuid()?,
                secret: {
                    let bytes: [u8; SECRET_LEN] = r.rest().get(..SECRET_LEN)?.try_into().ok()?;
                    bytes
                },
            },
            KEEP_ALIVE => Self::KeepAlive,
            CONNECTION_CHECK => Self::ConnectionCheck,
            PING => Self::Ping,
            _ => return None,
        })
    }

    /// A frame as a client would send it, for a test.
    #[cfg(test)]
    pub fn write(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::Mic {
                data,
                sequence,
                whispering,
            } => {
                w.u8(MIC).byte_array(data).i64(*sequence).bool(*whispering);
            }
            Self::Authenticate { player, secret } => {
                w.u8(AUTHENTICATE).uuid(*player).raw(secret);
            }
            Self::KeepAlive => {
                w.u8(KEEP_ALIVE);
            }
            Self::ConnectionCheck => {
                w.u8(CONNECTION_CHECK);
            }
            Self::Ping => {
                w.u8(PING).uuid(0).i64(0);
            }
        }
        w.finish()
    }
}

/// What the server sends over UDP.
#[derive(Clone, Debug, PartialEq)]
pub enum Sound {
    /// A player heard where they stand, from `distance` blocks off at most.
    Player {
        channel: u128,
        sender: u128,
        data: Vec<u8>,
        sequence: i64,
        distance: f32,
        whispering: bool,
    },
    /// A player heard by the group they are in.
    Group {
        channel: u128,
        sender: u128,
        data: Vec<u8>,
        sequence: i64,
    },
}

/// A packet the server sends, before it is encrypted.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Sound(Sound),
    AuthenticateAck,
    KeepAlive,
    ConnectionCheckAck,
}

impl Out {
    /// The plain payload of the datagram.
    pub fn write(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::Sound(Sound::Player {
                channel,
                sender,
                data,
                sequence,
                distance,
                whispering,
            }) => {
                w.u8(PLAYER_SOUND)
                    .uuid(*channel)
                    .uuid(*sender)
                    .byte_array(data)
                    .i64(*sequence)
                    .f32(*distance)
                    .u8(if *whispering { WHISPER_FLAG } else { 0 });
            }
            Self::Sound(Sound::Group {
                channel,
                sender,
                data,
                sequence,
            }) => {
                w.u8(GROUP_SOUND)
                    .uuid(*channel)
                    .uuid(*sender)
                    .byte_array(data)
                    .i64(*sequence)
                    .u8(0);
            }
            Self::AuthenticateAck => {
                w.u8(AUTHENTICATE_ACK);
            }
            Self::KeepAlive => {
                w.u8(KEEP_ALIVE);
            }
            Self::ConnectionCheckAck => {
                w.u8(CONNECTION_CHECK_ACK);
            }
        }
        w.finish()
    }
}

/// A datagram a client sends: its magic byte, the player it is for, and the sealed payload.
pub struct Datagram<'a> {
    pub player: u128,
    pub sealed: &'a [u8],
}

impl<'a> Datagram<'a> {
    pub fn read(bytes: &'a [u8]) -> Option<Self> {
        let mut r = Reader::new(bytes);
        if r.u8()? != MAGIC {
            return None;
        }
        let player = r.uuid()?;
        let sealed = r.byte_array(MAX_DATAGRAM_PAYLOAD)?;
        Some(Self { player, sealed })
    }
}

/// The datagram that carries `sealed` to a client.
pub fn datagram_to_client(sealed: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(MAGIC).byte_array(sealed);
    w.finish()
}

/// A datagram as a client would send it, for a test.
#[cfg(test)]
pub fn datagram_to_server(player: u128, sealed: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(MAGIC).uuid(player).byte_array(sealed);
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_the_bytes_the_client_reads() {
        let info = SecretInfo {
            secret: [9; SECRET_LEN],
            port: 24454,
            player: 0xABCD,
            codec: Codec::Voip,
            mtu: 1275,
            distance: 48.0,
            keep_alive: 1000,
            groups: true,
            host: "voice.example.net:5000".into(),
            allow_recording: false,
        };
        let (name, bytes) = secret(&info);
        assert_eq!(name, "voicechat:secret");
        let mut expected = vec![9u8; 16];
        expected.extend([0, 0, 0x5f, 0x86]);
        expected.extend(0xABCDu128.to_be_bytes());
        expected.push(0);
        expected.extend(1275i32.to_be_bytes());
        expected.extend(48f64.to_be_bytes());
        expected.extend(1000i32.to_be_bytes());
        expected.push(1);
        expected.push(22);
        expected.extend(b"voice.example.net:5000");
        expected.push(0);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn a_player_state_and_the_list_of_them_are_as_the_clients_read_them() {
        let state_of = |group| PlayerState {
            uuid: 5,
            name: "Alex".into(),
            disabled: true,
            disconnected: false,
            group,
        };
        let (name, bytes) = states(&[state_of(Some(6)), state_of(None)]);
        assert_eq!(name, "voicechat:states");
        let mut expected = 2i32.to_be_bytes().to_vec();
        expected.extend([1, 0]);
        expected.extend(5u128.to_be_bytes());
        expected.extend([4, b'A', b'l', b'e', b'x', 1]);
        expected.extend(6u128.to_be_bytes());
        expected.extend([1, 0]);
        expected.extend(5u128.to_be_bytes());
        expected.extend([4, b'A', b'l', b'e', b'x', 0]);
        assert_eq!(bytes, expected);
        assert_eq!(state(&state_of(None)).0, "voicechat:state");
        assert_eq!(remove_state(5).1, 5u128.to_be_bytes());
    }

    #[test]
    fn groups_and_the_answers_about_them_are_as_the_clients_read_them() {
        let group = ClientGroup {
            id: 0x11,
            name: "Friends".into(),
            has_password: true,
            kind: GroupKind::Open,
        };
        let (name, bytes) = add_group(&group);
        assert_eq!(name, "voicechat:add_group");
        let mut expected = 0x11u128.to_be_bytes().to_vec();
        expected.extend(b"\x07Friends");
        expected.extend([1, 0, 0, 0, 1]);
        assert_eq!(bytes, expected);
        assert_eq!(remove_group(0x11).1, 0x11u128.to_be_bytes());
        assert_eq!(joined_group(None, true).1, [0, 1]);
        let mut joined = vec![1];
        joined.extend(0x11u128.to_be_bytes());
        joined.push(0);
        assert_eq!(joined_group(Some(0x11), false).1, joined);
    }

    #[test]
    fn what_a_client_sends_over_the_game_connection_is_read() {
        assert_eq!(
            read_inbound("voicechat:request_secret", &20i32.to_be_bytes()),
            Some(Some(Inbound::RequestSecret(20)))
        );
        assert_eq!(
            read_inbound("voicechat:update_state", &[1]),
            Some(Some(Inbound::UpdateState { disabled: true }))
        );
        let mut join = 0x11u128.to_be_bytes().to_vec();
        join.extend([1, 2, b'p', b'w']);
        assert_eq!(
            read_inbound("voicechat:set_group", &join),
            Some(Some(Inbound::JoinGroup {
                group: 0x11,
                password: Some("pw".into())
            }))
        );
        assert_eq!(
            read_inbound("voicechat:create_group", b"\x07Friends\x00\x00\x01"),
            Some(Some(Inbound::CreateGroup {
                name: "Friends".into(),
                password: None,
                kind: GroupKind::Open
            }))
        );
        assert_eq!(
            read_inbound("voicechat:leave_group", &[]),
            Some(Some(Inbound::LeaveGroup))
        );
        // A channel that is not ours; a packet that is cut short or whose name is too long.
        assert_eq!(read_inbound("minecraft:brand", &[1]), None);
        assert_eq!(
            read_inbound("voicechat:request_secret", &[0, 0]),
            Some(None)
        );
        let mut long = vec![25];
        long.extend([b'a'; 25]);
        long.extend([0, 0, 0]);
        assert_eq!(read_inbound("voicechat:create_group", &long), Some(None));
    }

    #[test]
    fn what_a_client_sends_over_udp_is_read() {
        let mic = Voice::Mic {
            data: vec![1, 2, 3],
            sequence: 7,
            whispering: true,
        };
        assert_eq!(
            mic.write(),
            [vec![1u8, 3, 1, 2, 3], 7i64.to_be_bytes().to_vec(), vec![1]].concat()
        );
        for voice in [
            mic,
            Voice::Authenticate {
                player: 0xABCD,
                secret: [4; SECRET_LEN],
            },
            Voice::KeepAlive,
            Voice::ConnectionCheck,
            Voice::Ping,
        ] {
            assert_eq!(Voice::read(&voice.write()), Some(voice));
        }
        assert_eq!(Voice::read(&[0x42]), None);
        assert_eq!(Voice::read(&[]), None);
        // A frame longer than an opus frame can be is refused.
        let mut long = vec![1u8, 0xfc, 0x09];
        long.extend(vec![0u8; 1276]);
        assert_eq!(Voice::read(&long), None);
    }

    #[test]
    fn what_the_server_sends_over_udp_is_as_the_client_reads_it() {
        let sound = Out::Sound(Sound::Player {
            channel: 1,
            sender: 2,
            data: vec![1, 2, 3],
            sequence: 7,
            distance: 24.0,
            whispering: true,
        });
        let mut expected = vec![0x2];
        expected.extend(1u128.to_be_bytes());
        expected.extend(2u128.to_be_bytes());
        expected.extend([3, 1, 2, 3]);
        expected.extend(7i64.to_be_bytes());
        expected.extend(24f32.to_be_bytes());
        expected.push(1);
        assert_eq!(sound.write(), expected);
        let group = Out::Sound(Sound::Group {
            channel: 1,
            sender: 1,
            data: vec![9],
            sequence: 3,
        });
        let mut expected = vec![0x3];
        expected.extend(1u128.to_be_bytes());
        expected.extend(1u128.to_be_bytes());
        expected.extend([1, 9]);
        expected.extend(3i64.to_be_bytes());
        expected.push(0);
        assert_eq!(group.write(), expected);
        assert_eq!(Out::AuthenticateAck.write(), [0x6]);
        assert_eq!(Out::KeepAlive.write(), [0x8]);
        assert_eq!(Out::ConnectionCheckAck.write(), [0xa]);
    }

    #[test]
    fn a_datagram_has_a_magic_byte_the_player_and_the_sealed_payload() {
        let bytes = datagram_to_server(0xABCD, &[1, 2, 3]);
        let mut expected = vec![0xff];
        expected.extend(0xABCDu128.to_be_bytes());
        expected.extend([3, 1, 2, 3]);
        assert_eq!(bytes, expected);
        let datagram = Datagram::read(&bytes).unwrap();
        assert_eq!(
            (datagram.player, datagram.sealed),
            (0xABCD, &[1u8, 2, 3][..])
        );
        assert!(Datagram::read(&[0x00, 1, 2]).is_none());
        assert!(Datagram::read(&bytes[..10]).is_none());
        // To a client the player is left out: it knows who it is.
        assert_eq!(datagram_to_client(&[1, 2, 3]), [0xff, 3, 1, 2, 3]);
    }
}
