//! The mod's configuration: `config/voicechat.toml`.
//!
//! The file is written, with its comments, when it is missing. Numbers outside what means something
//! are brought into range, and a key the mod does not know is an error, so a typo is told rather than
//! left at its default.

use crate::proto::Codec;
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "voicechat.toml";

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Whether the voice chat server runs at all.
    pub enabled: bool,
    /// The UDP port the voice chat server listens on.
    pub port: u16,
    /// The address it binds to; empty for every address.
    pub bind_address: String,
    /// The address players send their voice to, when it is not the one they play on: `host`,
    /// `host:port` or just a port. Behind a proxy it has to be set.
    pub voice_host: String,
    /// How far a player is heard, in blocks.
    pub max_voice_distance: f64,
    /// How far a player who whispers is heard.
    pub whisper_distance: f64,
    pub codec: String,
    /// The most bytes a voice packet may hold, 512 to 1275.
    pub mtu_size: i32,
    /// Milliseconds between keep-alive packets; a connection with no answer in ten times it is
    /// dropped and the player asked to connect again.
    pub keep_alive: i32,
    /// Whether players may make and join groups.
    pub groups: bool,
    /// Whether a client may record what it hears (the client's own setting; the server only says).
    pub allow_recording: bool,
    /// Whether a spectator is heard by the players around them. Off, only other spectators hear a
    /// spectator by proximity.
    pub spectators_talk_to_players: bool,
    /// Whether the server answers the pings that test a voice connection.
    pub allow_pings: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 24454,
            bind_address: String::new(),
            voice_host: String::new(),
            max_voice_distance: 48.0,
            whisper_distance: 24.0,
            codec: "voip".to_owned(),
            mtu_size: 1275,
            keep_alive: 1000,
            groups: true,
            allow_recording: true,
            spectators_talk_to_players: false,
            allow_pings: true,
        }
    }
}

impl Config {
    /// The same with every number held to what means something.
    fn held(self) -> Self {
        Self {
            max_voice_distance: self.max_voice_distance.clamp(1.0, 1_000_000.0),
            whisper_distance: self.whisper_distance.clamp(1.0, 1_000_000.0),
            mtu_size: self.mtu_size.clamp(512, 1275),
            keep_alive: self.keep_alive.max(1000),
            ..self
        }
    }

    pub fn codec(&self) -> Codec {
        Codec::parse(&self.codec).unwrap_or(Codec::Voip)
    }
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Reads a config's text.
pub fn parse(text: &str) -> Result<Config, String> {
    let config = toml::from_str::<Config>(text).map_err(|e| e.message().to_owned())?;
    if Codec::parse(&config.codec).is_none() {
        return Err(format!(
            "codec must be voip, audio or restricted_lowdelay, not {}",
            config.codec
        ));
    }
    Ok(config.held())
}

/// Reads `dir/voicechat.toml`, writing the default one when there is none.
pub fn load(dir: &Path) -> Result<Config, String> {
    let file = path(dir);
    match std::fs::read_to_string(&file) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            std::fs::write(&file, DEFAULT_FILE).map_err(|e| format!("{}: {e}", file.display()))?;
            Ok(Config::default())
        }
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// What a new `voicechat.toml` holds.
pub const DEFAULT_FILE: &str = r#"# Voice chat, for players who have the Simple Voice Chat mod (version 2.6). Nothing is needed on
# the players' side but that mod; the server needs a UDP port open to them.

# Turn this off and no voice chat server runs.
enabled = true

# The UDP port the voice chat server listens on. Open it (UDP) on the machine and in its firewall.
port = 24454

# The address to bind to. Leave empty for every address.
bind-address = ""

# The address players send their voice to. Empty means the address they play on. Behind a Velocity
# proxy that runs the Simple Voice Chat plugin, leave it empty: the plugin tells the players where to
# send their voice and passes it on to this server's UDP port above, which only has to be reachable
# from the proxy. Behind a proxy without the plugin, set it to this server's public address, as `host`
# (the port above is used), `host:port`, or just a port (the address they play on, another port).
voice-host = ""

# How far a player is heard, in blocks, and how far one who whispers is.
max-voice-distance = 48.0
whisper-distance = 24.0

# How the players' clients encode what they say: voip, audio or restricted_lowdelay.
codec = "voip"

# The most bytes a voice packet holds, 512 to 1275.
mtu-size = 1275

# Milliseconds between the keep-alive packets that tell the server a player is still there. At least
# 1000. A connection with no answer in ten times it is dropped and the player asked to connect again.
keep-alive = 1000

# Whether players can make and join groups, who hear each other wherever they are.
groups = true

# Whether the clients may record what they hear. The server only says; the clients' setting decides.
allow-recording = true

# Whether a spectator is heard by the players around them. Off, only other spectators hear them (a
# group they are in still does).
spectators-talk-to-players = false

# Whether the server answers the pings that test a voice connection.
allow-pings = true
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_a_new_server_gets_is_the_default_config() {
        assert_eq!(parse(DEFAULT_FILE), Ok(Config::default()));
        assert_eq!(parse(""), Ok(Config::default()));
    }

    #[test]
    fn numbers_are_held_to_what_means_something_and_a_typo_is_told() {
        let c = parse("max-voice-distance = -4\nmtu-size = 99999\nkeep-alive = 5\nport = 25000")
            .unwrap();
        assert_eq!(c.max_voice_distance, 1.0);
        assert_eq!((c.mtu_size, c.keep_alive, c.port), (1275, 1000, 25000));
        assert!(parse("voice-hots = \"x\"").is_err());
        assert!(parse("codec = \"loud\"").unwrap_err().contains("codec"));
        assert_eq!(parse("codec = \"AUDIO\"").unwrap().codec(), Codec::Audio);
    }
}
