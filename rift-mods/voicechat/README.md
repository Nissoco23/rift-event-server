# VoiceChat

Voice chat for Rift Event Server, as a native Rust mod: a server for the [Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat)
protocol. Players who have the Simple Voice Chat mod on their client talk to the players near them, and in groups;
players without it play as before, so nothing is required of anyone's client.

It speaks **compatibility version 20, the 2.6 releases** (2.6.23 and the others for game 26.3). A client with another
version (the 2.7 pre-releases are 21) is told in chat to install 2.6 and gets no voice chat. It is not a port of Simple
Voice Chat and uses none of its code (see [Where it comes from](#where-it-comes-from)); it does not decode a voice, only
relays opus frames.

## Setting it up

1. Open the **UDP port 24454** (`port` in `config/voicechat.toml`) in the machine's firewall, and on a panel allocate it
   to the server (Pterodactyl: the egg's **Voice chat port** variable).
2. **Behind a proxy (Velocity)**, set `voice-host` to this server's own public address, as `host` or `host:port`
   (Pterodactyl: **Voice chat host**). Empty means the players' clients send their voice to the address they play on,
   which is the proxy's, and no voice chat server runs there.
3. Players install Simple Voice Chat 2.6 and join. The log says `Voice chat server started at ...` at the first tick;
   `/voicechat status` says who has the mod and is connected.

The config (`config/voicechat.toml`, written with its comments on the first start; a key it does not know is an error):

| Key | Default | |
| --- | --- | --- |
| `enabled` | `true` | Off, no voice chat server runs. |
| `port` | `24454` | The UDP port. |
| `bind-address` | empty | Every address. |
| `voice-host` | empty | Where the clients send their voice: `host`, `host:port` or a port. |
| `max-voice-distance` | `48` | Blocks a player is heard. |
| `whisper-distance` | `24` | Blocks a whisper is heard. |
| `codec` | `voip` | `voip`, `audio` or `restricted_lowdelay`: how the clients encode. |
| `mtu-size` | `1275` | The most bytes a voice packet holds (512 to 1275). |
| `keep-alive` | `1000` | Milliseconds between keep-alives; ten times it with no answer drops the connection. |
| `groups` | `true` | Whether players can make and join groups. |
| `allow-recording` | `true` | What the server tells the clients about recording. |
| `spectators-talk-to-players` | `false` | Off, a spectator is heard by other spectators only. |
| `allow-pings` | `true` | Whether the server answers the pings that test a connection. |

`/voicechat status` is for operators of level 2 (or `voicechat.admin`) and the console.

## How it behaves

- **Near players.** A player is heard by players in their dimension within `max-voice-distance` blocks (a block more is
  sent, and the clients fade the voice by the distance in the packet), by `whisper-distance` when they whisper. Spectators
  are heard by spectators only, unless `spectators-talk-to-players`: the dead do not talk to the living.
- **Groups.** A player makes a group (a name of 1 to 24 characters, an optional password, a type) and others join it.
  A **normal** group hears only itself; an **open** group also hears the players near it and is heard by them;
  an **isolated** group hears only its members. A group nobody is in is gone. Members hear each other wherever they are.
- **Nicknames.** The voice chat knows a player by their nickname when they have one (`nick` mod), so a group's member
  list does not give their name away, and tells the clients again when it changes.
- **Voice chat off.** A player who turns it off in their client hears nothing; one with no connection yet hears nothing.
- A game-connection packet rate limit of 16 a second a player; more are dropped.

## What the protocol is

Over the game connection, plugin messages on `voicechat:` channels: the client sends `request_secret` (with its
compatibility version), `update_state` (voice chat off or on), `create_group`, `set_group` (join) and `leave_group`; the
server answers with `secret` (the player's AES key, the UDP port, the host, the distance, the keep-alive, whether groups
are on), `states`, `state`, `remove_state`, `add_group`, `remove_group` and `joined_group`. Over UDP, each datagram is
`0xFF`, the player's UUID (from the client; none to it) and the payload sealed with AES-128-GCM under the player's secret
(a 12-byte nonce, then the ciphertext and its tag): the client authenticates with its secret, checks the connection, then
sends `mic` frames; the server sends `player_sound` and `group_sound` frames to those who hear it, `keep_alive`s, and
acknowledgements. Code: `proto.rs` (the packets), `crypt.rs`, `core.rs` (who knows what and who hears whom, as plain
functions of the state), `udp.rs` (the socket thread), `lib.rs` (the hooks).

## Where it comes from

Simple Voice Chat is "Copyright (c) Max Henkel - All Rights Reserved". This is an independent implementation of its
network protocol for this server, written in Rust from the layouts of the packets and the behaviour a client expects;
no code of it is copied or translated. The groundwork repository [horizon-voice-chat](https://github.com/Nirokx/horizon-voice-chat)
(a client for the same protocol) was used to check the layouts. It is not made, endorsed or supported by Max Henkel; do
not report this server's problems to Simple Voice Chat.

## Tests

`cargo test --profile fast -p voicechat` drives the server in memory (the handshake, who hears whom, groups, keep-alives, the
hooks over a fake server, the config) and over two real sockets on the loopback. Besides, the groundwork repository's client
code (its packet parsers, its encryption and its link, written apart from this server) was run against this one in a
scratch crate: it authenticated, checked the connection, parsed the secret, states and groups, and heard a whisper and a
group's voice. That scratch crate is not in the repository.

Not tested: a real Simple Voice Chat client (the game and its mod) against this server, and a server behind a real
Velocity proxy.
