//! `/viewdistance status | reload`, for operators.

use crate::ViewDistance;
use horizon_client::mod_api::{Host, NativeCommand};

/// Who may run the command: the permission node a permissions mod can grant, or an operator of level
/// 2 where nothing does.
pub const ADMIN: &str = "viewdistance.admin";
const OP_LEVEL: u8 = 2;

const USAGES: [&str; 2] = ["viewdistance status", "viewdistance reload"];

/// What the command takes, for the error of a line it cannot read.
const USAGE: &str = "status | reload";

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

/// A line of markup (`rift_chat`) to whoever ran the command.
fn say(host: &mut dyn Host, markup: &str) {
    host.feedback(rift_chat::mini(markup));
}

/// Runs `line`. Whether it was a `viewdistance` command.
pub fn run(module: &ViewDistance, host: &mut dyn Host, line: &str) -> bool {
    let Some(command) = parse(line) else {
        return false;
    };
    match command {
        Command::Status => status(module, host),
        Command::Reload => reload(module, host),
        Command::Usage => {
            let words = line.split_whitespace().count();
            let message = format!("Usage: <accent>/viewdistance</accent> {USAGE}");
            let [header, context] = rift_chat::error(line, words, &message);
            host.feedback(header);
            host.feedback(context);
        }
    }
    true
}

/// One player's line of `status` as markup: whether they count as an operator, what their client
/// asks for, the radius the engine sends them in full (the far ring reaches past it for those whose
/// limit allows) and the limit this mod set. A `?` is what the server could not tell.
pub fn describe(
    name: &str,
    operator: bool,
    fake: bool,
    asks: Option<i32>,
    gets: Option<i32>,
    limit: Option<i32>,
) -> String {
    let number = |n: Option<i32>| n.map_or_else(|| "?".to_owned(), |n| n.to_string());
    let name = rift_chat::escape(name);
    if fake {
        return format!("<accent>{name}</accent><gray> (fake player): not limited</gray>");
    }
    let who = if operator { "operator" } else { "player" };
    let limit = limit.map_or_else(|| "none yet".to_owned(), |n| n.to_string());
    format!(
        "<accent>{name}</accent><gray> ({who}): asks {}, near {}, limit {limit}</gray>",
        number(asks),
        number(gets)
    )
}

fn status(module: &ViewDistance, host: &mut dyn Host) {
    let cfg = module.config();
    if cfg.enabled {
        say(
            host,
            &format!(
                "<white>View distance limits: players <accent>{}</accent>, operators <accent>{}</accent></white><gray> (operator level {} and up)</gray>",
                cfg.non_op_max, cfg.op_max, cfg.op_level
            ),
        );
    } else {
        say(
            host,
            "<gray>View distance limits are off (enabled = false in config/viewdistance.toml)</gray>",
        );
    }
    let players = host.server_players();
    if players.is_empty() {
        say(host, "<gray>No players are online</gray>");
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
        say(host, &line);
    }
}

fn reload(module: &ViewDistance, host: &mut dyn Host) {
    match module.reload_config(&*host) {
        Ok(()) => {
            // The new limits apply now, not at the next second.
            let players = host.server_players();
            module.reconcile(host, &players, true);
            say(
                host,
                "<white>Reloaded the <accent>view distance</accent> config!</white>",
            );
        }
        Err(e) => {
            host.log(
                "WARN",
                &format!("View distance: could not read the config: {e}"),
            );
            say(
                host,
                &format!(
                    "<red>Could not read the config, keeping the old one: {}</red>",
                    rift_chat::escape(&e)
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rift_chat::plain;

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
            plain(&describe("Bob", false, false, Some(24), Some(8), Some(8))),
            "Bob (player): asks 24, near 8, limit 8"
        );
        assert_eq!(
            plain(&describe(
                "Alice",
                true,
                false,
                Some(12),
                Some(12),
                Some(32)
            )),
            "Alice (operator): asks 12, near 12, limit 32"
        );
        assert_eq!(
            plain(&describe("Eve", false, false, None, None, None)),
            "Eve (player): asks ?, near ?, limit none yet"
        );
        assert_eq!(
            plain(&describe("Bot", true, true, None, None, None)),
            "Bot (fake player): not limited"
        );
    }
}
