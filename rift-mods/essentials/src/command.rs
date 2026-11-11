//! `/gmc`, `/gms`, `/gmsp`, `/gma <player>` and `/flyspeed <1-10>`, for operators.

use crate::state::{Actor, Job, Mode, State, accent};
use horizon_client::mod_api::{Host, NativeCommand, NativeServerPlayer};
use rift_chat::{escape, mini};

/// The permission nodes a permissions mod can grant, and the operator level that counts where none
/// does: 2, a game master, as vanilla's `/gamemode` needs.
pub const GAME_MODE: &str = "essentials.gamemode";
pub const FLY_SPEED: &str = "essentials.flyspeed";
const OP_LEVEL: u8 = 2;

const GAME_MODE_USAGES: [&str; 4] = [
    "gmc [player:player_name]",
    "gms [player:player_name]",
    "gmsp [player:player_name]",
    "gma [player:player_name]",
];
const FLY_SPEED_USAGE: &str = "flyspeed <speed:number>";

/// The command's usages, for the players' command trees.
pub fn register(commands: &mut Vec<NativeCommand>) {
    for usage in GAME_MODE_USAGES {
        commands.push(NativeCommand::op(usage, OP_LEVEL).permission(GAME_MODE));
    }
    commands.push(NativeCommand::op(FLY_SPEED_USAGE, OP_LEVEL).permission(FLY_SPEED));
}

/// A line of the command, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A game mode for the player named, or for whoever ran it.
    Mode { mode: Mode, target: Option<String> },
    /// A flying speed, as it was typed, for whoever ran it.
    FlySpeed(String),
    /// One of ours, said wrongly: the line's usage.
    Usage(&'static str),
}

const MODE_USAGE: &str = "Usage: /<gmc|gms|gmsp|gma> [player]";
const FLY_USAGE: &str = "Usage: /flyspeed <1-10>";

/// Reads `line` (with or without its slash). None when it is not one of the commands.
pub fn parse(line: &str) -> Option<Command> {
    let mut words = line.trim().trim_start_matches('/').split_whitespace();
    let head = words.next()?.to_ascii_lowercase();
    let args: Vec<&str> = words.collect();
    let mode = match head.as_str() {
        "gms" => Some(Mode::Survival),
        "gmc" => Some(Mode::Creative),
        "gma" => Some(Mode::Adventure),
        "gmsp" => Some(Mode::Spectator),
        "flyspeed" => {
            return Some(match args.as_slice() {
                [speed] => Command::FlySpeed((*speed).to_owned()),
                _ => Command::Usage(FLY_USAGE),
            });
        }
        _ => None,
    }?;
    Some(match args.as_slice() {
        [] => Command::Mode { mode, target: None },
        [target] => Command::Mode {
            mode,
            target: Some((*target).to_owned()),
        },
        _ => Command::Usage(MODE_USAGE),
    })
}

/// A line of markup (`rift_chat`) to whoever ran the command.
fn say(host: &mut dyn Host, markup: &str) {
    host.feedback(mini(markup));
}

/// A command that cannot be run: the header, then the typed command up to the `at`th word, which
/// is the one at fault, and why.
fn refuse(host: &mut dyn Host, line: &str, at: usize, markup: &str) {
    let [header, context] = rift_chat::error(line, at, markup);
    host.feedback(header);
    host.feedback(context);
}

fn words(line: &str) -> usize {
    line.split_whitespace().count()
}

/// Runs `line` as `actor`. Whether it was one of the commands.
pub fn run(state: &mut State, host: &mut dyn Host, actor: &Actor, line: &str) -> bool {
    let Some(command) = parse(line) else {
        return false;
    };
    match command {
        Command::Mode { mode, target } => {
            game_mode(state, host, actor, line, mode, target.as_deref());
        }
        Command::FlySpeed(speed) => fly_speed(state, host, actor, line, &speed),
        Command::Usage(usage) => refuse(host, line, words(line), &escape(usage)),
    }
    true
}

/// The player the command is for: the one named (by the name they logged in with, or their
/// nickname), else whoever ran it. The error is markup, with where in the line it belongs.
fn player_for(
    host: &dyn Host,
    actor: &Actor,
    line: &str,
    named: Option<&str>,
) -> Result<NativeServerPlayer, (usize, String)> {
    let players = host.server_players();
    let found = match (named, actor) {
        (Some(word), _) => players.into_iter().find(|p| {
            p.name.eq_ignore_ascii_case(word)
                || p.nickname
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(word))
        }),
        (None, Actor::Player { uuid, .. }) => players.into_iter().find(|p| p.uuid == *uuid),
        (None, Actor::Console) => {
            return Err((words(line), "The console is no player: name one".to_owned()));
        }
    };
    found.ok_or((
        named.map_or(words(line), |_| 2),
        "Player was not found".to_owned(),
    ))
}

fn game_mode(
    state: &mut State,
    host: &mut dyn Host,
    actor: &Actor,
    line: &str,
    mode: Mode,
    target: Option<&str>,
) {
    let player = match player_for(host, actor, line, target) {
        Ok(player) => player,
        Err((at, why)) => return refuse(host, line, at, &why),
    };
    let own = matches!(actor, Actor::Player { uuid, .. } if *uuid == player.uuid);
    let (who, whose) = if own {
        ("You are".to_owned(), "your".to_owned())
    } else {
        (
            format!("{} is", accent(&player.name)),
            format!("{}'s", accent(&player.name)),
        )
    };
    if player.game_mode == mode.id() {
        return say(
            host,
            &format!("<gray>{who} already in {} mode</gray>", accent(mode.name())),
        );
    }
    state.push(Job::GameMode {
        uuid: player.uuid,
        name: player.name,
        mode,
        actor: actor.clone(),
    });
    say(
        host,
        &format!(
            "<white>Set {whose} game mode to {}!</white>",
            accent(mode.name())
        ),
    );
}

fn fly_speed(state: &mut State, host: &mut dyn Host, actor: &Actor, line: &str, typed: &str) {
    let player = match player_for(host, actor, line, None) {
        Ok(player) => player,
        Err((at, why)) => return refuse(host, line, at, &why),
    };
    let at = words(line);
    let Some(value) = typed.parse::<f64>().ok().filter(|v| v.is_finite()) else {
        return refuse(host, line, at, "Flight speed must be a number from 1 to 10");
    };
    if value < 1.0 {
        return refuse(host, line, at, "Flight speed must be at least 1");
    }
    // Above 10 is 10; in between, the nearest whole level.
    let level = value.round().min(10.0) as u8;
    state.push(Job::FlySpeed {
        uuid: player.uuid,
        name: player.name,
        level,
        actor: actor.clone(),
    });
    say(
        host,
        &format!(
            "<white>Set your flight speed to {} of 10!</white>",
            accent(&level.to_string())
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_is_not_ours_is_not_read() {
        for line in ["gamemode creative", "gmcc", "fly", "", "  "] {
            assert_eq!(parse(line), None, "{line:?}");
        }
    }

    #[test]
    fn each_shortcut_names_its_mode_for_the_player_typed_or_for_no_one() {
        for (word, mode) in [
            ("gmc", Mode::Creative),
            ("gms", Mode::Survival),
            ("gmsp", Mode::Spectator),
            ("gma", Mode::Adventure),
        ] {
            assert_eq!(
                parse(&format!("/{} ", word.to_uppercase())),
                Some(Command::Mode { mode, target: None })
            );
            assert_eq!(
                parse(&format!("{word}  Bob")),
                Some(Command::Mode {
                    mode,
                    target: Some("Bob".into())
                })
            );
        }
        assert_eq!(parse("gmc a b"), Some(Command::Usage(MODE_USAGE)));
    }

    #[test]
    fn flyspeed_takes_one_word() {
        assert_eq!(parse("flyspeed 4"), Some(Command::FlySpeed("4".into())));
        assert_eq!(parse("flyspeed"), Some(Command::Usage(FLY_USAGE)));
        assert_eq!(parse("flyspeed 4 5"), Some(Command::Usage(FLY_USAGE)));
    }

    #[test]
    fn every_usage_is_for_operators_and_has_a_permission_node() {
        let mut commands = Vec::new();
        register(&mut commands);
        assert_eq!(commands.len(), 5);
        for command in &commands {
            assert_eq!(command.level, 2);
            assert!(!command.replace_vanilla);
        }
        let node = |usage: &str| {
            commands
                .iter()
                .find(|c| c.usage == usage)
                .and_then(|c| c.permission.clone())
        };
        assert_eq!(
            node("gmsp [player:player_name]").as_deref(),
            Some(GAME_MODE)
        );
        assert_eq!(node("flyspeed <speed:number>").as_deref(), Some(FLY_SPEED));
    }
}
