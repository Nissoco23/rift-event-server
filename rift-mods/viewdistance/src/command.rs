//! `/viewdistance status | reload`, for operators.

use crate::ViewDistance;
use horizon_client::mod_api::{Host, NativeCommand, Text};

/// Who may run the command: the permission node a permissions mod can grant, or an operator of level
/// 2 where nothing does.
pub const ADMIN: &str = "viewdistance.admin";
const OP_LEVEL: u8 = 2;

const AQUA: u32 = 0x55FFFF;
const RED: u32 = 0xFF5555;
const GRAY: u32 = 0xAAAAAA;

const USAGES: [&str; 2] = ["viewdistance status", "viewdistance reload"];

const USAGE: &str = "Usage: /viewdistance status | reload";

/// The command's usages, for the players' command trees.
pub fn register(commands: &mut Vec<NativeCommand>) {
    for usage in USAGES {
        commands.push(NativeCommand::op(usage, OP_LEVEL).permission(ADMIN));
    }
}

/// A line of the command, read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Reload,
    /// Our command, said wrongly.
    Usage,
}

/// Reads `line` (with or without its slash). None when it is not a `viewdistance` command.
pub fn parse(line: &str) -> Option<Command> {
    let mut words = line.trim().trim_start_matches('/').split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("viewdistance") {
        return None;
    }
    let rest: Vec<&str> = words.collect();
    Some(match rest.as_slice() {
        [word] if word.eq_ignore_ascii_case("status") => Command::Status,
        [word] if word.eq_ignore_ascii_case("reload") => Command::Reload,
        _ => Command::Usage,
    })
}

fn say(host: &mut dyn Host, text: impl Into<String>, color: u32) {
    host.feedback(Text::colored(text.into(), color));
}

/// Runs `line`. Whether it was a `viewdistance` command.
pub fn run(module: &ViewDistance, host: &mut dyn Host, line: &str) -> bool {
    let Some(command) = parse(line) else {
        return false;
    };
    match command {
        Command::Status => status(module, host),
        Command::Reload => reload(module, host),
        Command::Usage => say(host, USAGE, RED),
    }
    true
}

/// One player's line of `status`: whether they count as an operator, what their client asks for,
/// the radius the engine sends them in full (the far ring reaches past it for those whose limit allows) and the limit this mod set. A `?` is what the server could not tell.
pub fn describe(
    name: &str,
    operator: bool,
    fake: bool,
    asks: Option<i32>,
    gets: Option<i32>,
    limit: Option<i32>,
) -> String {
    let number = |n: Option<i32>| n.map_or_else(|| "?".to_owned(), |n| n.to_string());
    if fake {
        return format!("{name} (fake player): not limited");
    }
    let who = if operator { "operator" } else { "player" };
    let limit = limit.map_or_else(|| "none yet".to_owned(), |n| n.to_string());
    format!(
        "{name} ({who}): asks {}, near {}, limit {limit}",
        number(asks),
        number(gets)
    )
}

fn status(module: &ViewDistance, host: &mut dyn Host) {
    let cfg = module.config();
    if cfg.enabled {
        say(
            host,
            format!(
                "View distance limits: players {}, operators {} (operator level {} and up).",
                cfg.non_op_max, cfg.op_max, cfg.op_level
            ),
            AQUA,
        );
    } else {
        say(
            host,
            "View distance limits are off (enabled = false in config/viewdistance.toml).",
            GRAY,
        );
    }
    let players = host.server_players();
    if players.is_empty() {
        say(host, "No players are online.", GRAY);
    }
    for p in &players {
        let asks = host
            .player_client(p.uuid)
            .map(|client| i32::from(client.view_distance));
        let line = describe(
            &p.name,
            p.permission >= cfg.op_level,
            p.fake,
            asks,
            host.player_view_distance(p.uuid),
            module.applied(p.uuid),
        );
        say(host, line, GRAY);
    }
}

fn reload(module: &ViewDistance, host: &mut dyn Host) {
    match module.reload_config(&*host) {
        Ok(()) => {
            // The new limits apply now, not at the next second.
            let players = host.server_players();
            module.reconcile(host, &players, true);
            say(host, "View distance config reloaded.", AQUA);
        }
        Err(e) => {
            host.log(
                "WARN",
                &format!("View distance: could not read the config: {e}"),
            );
            say(
                host,
                format!("Could not read the config, keeping the old one: {e}"),
                RED,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_is_not_ours_is_not_read() {
        assert_eq!(parse("gamemode creative"), None);
        assert_eq!(parse("view"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
    }

    #[test]
    fn the_subcommands_are_read_with_or_without_the_slash_in_any_case() {
        assert_eq!(parse("viewdistance status"), Some(Command::Status));
        assert_eq!(parse("/ViewDistance  RELOAD "), Some(Command::Reload));
    }

    #[test]
    fn a_subcommand_said_wrongly_gets_the_usage() {
        for line in [
            "viewdistance",
            "viewdistance dance",
            "viewdistance status now",
            "viewdistance reload 3",
        ] {
            assert_eq!(parse(line), Some(Command::Usage), "{line}");
        }
    }

    #[test]
    fn the_command_is_registered_for_operators_of_level_two_and_its_permission() {
        let mut commands = Vec::new();
        register(&mut commands);
        let usages: Vec<&str> = commands.iter().map(|c| c.usage.as_str()).collect();
        assert_eq!(usages, ["viewdistance status", "viewdistance reload"]);
        for command in &commands {
            assert_eq!(command.level, 2);
            assert_eq!(command.permission.as_deref(), Some("viewdistance.admin"));
            assert!(!command.replace_vanilla);
        }
    }

    #[test]
    fn a_players_line_says_what_they_ask_get_and_are_limited_to() {
        assert_eq!(
            describe("Bob", false, false, Some(24), Some(8), Some(8)),
            "Bob (player): asks 24, near 8, limit 8"
        );
        assert_eq!(
            describe("Alice", true, false, Some(12), Some(12), Some(32)),
            "Alice (operator): asks 12, near 12, limit 32"
        );
        assert_eq!(
            describe("Eve", false, false, None, None, None),
            "Eve (player): asks ?, near ?, limit none yet"
        );
        assert_eq!(
            describe("Bot", true, true, None, None, None),
            "Bot (fake player): not limited"
        );
    }
}
