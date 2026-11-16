//! `/voicechat status`: whether the voice chat server runs, and who has the mod and is connected.
//! A message that shows a state is gray with the copper accent on what matters.

use crate::core::Core;
use horizon_client::mod_api::{NativeCommand, NativeServerPlayer, Text};
use rift_chat::{count, escape, mini};

/// The permission node a permissions mod can grant, which operators of the level below have
/// without one.
pub const ADMIN: &str = "voicechat.admin";
const OP_LEVEL: u8 = 2;

pub fn register(commands: &mut Vec<NativeCommand>) {
    commands.push(NativeCommand::op("voicechat status", OP_LEVEL).permission(ADMIN));
}

/// Whether `line` is `voicechat` or `voicechat status`: the command is one word, and anything after
/// it that is not `status` is the usage.
pub fn is_ours(line: &str) -> bool {
    line.trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("voicechat"))
}

/// The answer to the command.
pub fn status(core: &Core, players: &[NativeServerPlayer], active: bool) -> Vec<Text> {
    if !active {
        return vec![mini(
            "<gray>The voice chat server is <accent>off</accent>: see <accent>config/voicechat.toml</accent> and the log</gray>",
        )];
    }
    let mut lines = vec![
        mini(&format!(
            "<gray>The voice chat server listens on UDP port <accent>{}</accent></gray>",
            core.port
        )),
        mini(&format!(
            "<gray>Voice host: <accent>{}</accent></gray>",
            if core.config.voice_host.trim().is_empty() {
                "the address players play on".to_owned()
            } else {
                escape(core.config.voice_host.trim())
            }
        )),
    ];
    let with_mod: Vec<&NativeServerPlayer> = players
        .iter()
        .filter(|p| core.is_compatible(p.uuid))
        .collect();
    let connected = with_mod
        .iter()
        .filter(|p| core.is_connected(p.uuid))
        .count();
    lines.push(mini(&format!(
        "<gray><accent>{}</accent> with the mod, <accent>{connected}</accent> connected</gray>",
        with_mod.len()
    )));
    for player in with_mod {
        let state = if core.is_connected(player.uuid) {
            match core.state_of(player.uuid) {
                Some(s) if s.disabled => "connected, voice chat off",
                _ => "connected",
            }
        } else {
            "not connected"
        };
        lines.push(mini(&format!(
            "<gray>{}: <accent>{state}</accent></gray>",
            escape(&player.name)
        )));
    }
    let groups = core.groups().count();
    if groups > 0 {
        lines.push(mini(&format!(
            "<gray><accent>{}</accent></gray>",
            count(groups, "group")
        )));
    }
    lines
}
