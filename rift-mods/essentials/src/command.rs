//! `/gmc`, `/gms`, `/gmsp`, `/gma <targets>`, vanilla's `/gamemode <mode> <targets>` in their place,
//! and `/flyspeed <1-10>`, for operators.

use crate::state::{Actor, Job, Mode, State, accent};
use horizon_client::mod_api::{Host, NativeCommand, NativeServerPlayer};
use rift_chat::{count, escape, mini};

/// The permission nodes a permissions mod can grant, and the operator level that counts where none
/// does: 2, a game master, as vanilla's `/gamemode` needs.
pub const GAME_MODE: &str = "essentials.gamemode";
pub const FLY_SPEED: &str = "essentials.flyspeed";
const OP_LEVEL: u8 = 2;

const FLY_SPEED_USAGE: &str = "flyspeed <speed:number>";

/// The shortcuts and vanilla's `/gamemode` mode words, by game mode.
const SHORTCUTS: [(&str, &str); 4] = [
    ("gms", "survival"),
    ("gmc", "creative"),
    ("gma", "adventure"),
    ("gmsp", "spectator"),
];

/// What a game mode command takes besides players and selectors: every player who is no operator.
const NON_OPS: &str = "--non-ops";

/// The command's usages, for the players' command trees. `/gamemode` replaces vanilla's, which the
/// mod then answers in full (`NativeCommand::replace_vanilla`).
pub fn register(commands: &mut Vec<NativeCommand>) {
    let mut add = |usage: String, replace: bool| {
        commands.push(
            NativeCommand::op(&usage, OP_LEVEL)
                .permission(GAME_MODE)
                .replace_vanilla(replace),
        );
    };
    for (shortcut, mode) in SHORTCUTS {
        for (head, replace) in [
            (shortcut.to_owned(), false),
            (format!("gamemode {mode}"), true),
        ] {
            add(format!("{head} [targets:players]"), replace);
            add(format!("{head} {NON_OPS}"), replace);
        }
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

const MODE_USAGE: &str = "Usage: /<gmc|gms|gmsp|gma> [targets]";
const GAMEMODE_USAGE: &str = "Usage: /gamemode <survival|creative|adventure|spectator> [targets]";
const FLY_USAGE: &str = "Usage: /flyspeed <1-10>";

/// Reads `line` (with or without its slash). None when it is not one of the commands.
pub fn parse(line: &str) -> Option<Command> {
    let mut words = line.trim().trim_start_matches('/').split_whitespace();
    let head = words.next()?.to_ascii_lowercase();
    let args: Vec<&str> = words.collect();
    let mut args = args;
    let mode = match head.as_str() {
        // Vanilla's, by the mode's name: the same as the shortcut for it.
        "gamemode" => {
            let Some(mode) = args
                .first()
                .and_then(|word| {
                    SHORTCUTS
                        .iter()
                        .find(|(_, name)| name.eq_ignore_ascii_case(word))
                })
                .and_then(|(shortcut, _)| shortcut_mode(shortcut))
            else {
                return Some(Command::Usage(GAMEMODE_USAGE));
            };
            args.remove(0);
            if args.len() > 1 {
                return Some(Command::Usage(GAMEMODE_USAGE));
            }
            Some(mode)
        }
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

fn shortcut_mode(shortcut: &str) -> Option<Mode> {
    match shortcut {
        "gms" => Some(Mode::Survival),
        "gmc" => Some(Mode::Creative),
        "gma" => Some(Mode::Adventure),
        "gmsp" => Some(Mode::Spectator),
        _ => None,
    }
}

/// Where the targets are in a game mode line: after `/gamemode <mode>`, or after a shortcut.
fn target_at(line: &str) -> usize {
    let first = line
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next();
    if first.is_some_and(|word| word.eq_ignore_ascii_case("gamemode")) {
        3
    } else {
        2
    }
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

/// The player the command is for when it takes no one else: whoever ran it. The error is markup,
/// with where in the line it belongs.
fn runner(
    host: &dyn Host,
    actor: &Actor,
    line: &str,
) -> Result<NativeServerPlayer, (usize, String)> {
    match actor {
        Actor::Player { uuid, .. } => host
            .server_players()
            .into_iter()
            .find(|p| p.uuid == *uuid)
            .ok_or((words(line), "Player was not found".to_owned())),
        Actor::Console => Err((words(line), NO_PLAYER.to_owned())),
    }
}

const NO_PLAYER: &str = "The console is no player: name one";

/// The players a word names: the name they logged in with or their nickname, `@s` (whoever ran the
/// command), `@a` (everyone), `@p` (the nearest to whoever ran it, themselves when no one is
/// nearer, as vanilla has it), `@r` (one at random) or `--non-ops` (everyone who is no operator).
/// Options in brackets are not read. The error is markup.
fn select(
    state: &mut State,
    host: &dyn Host,
    actor: &Actor,
    spec: &str,
) -> Result<Vec<NativeServerPlayer>, String> {
    if spec.contains('[') {
        return Err(
            "Selector options are not supported here: use @a, @p, @r, @s, --non-ops or a name"
                .to_owned(),
        );
    }
    let players = host.server_players();
    let own = |players: Vec<NativeServerPlayer>| match actor {
        Actor::Player { uuid, .. } => players
            .into_iter()
            .find(|p| p.uuid == *uuid)
            .ok_or_else(|| "Player was not found".to_owned()),
        Actor::Console => Err(NO_PLAYER.to_owned()),
    };
    match spec.to_ascii_lowercase().as_str() {
        "@s" => own(players).map(|p| vec![p]),
        NON_OPS => {
            let found: Vec<NativeServerPlayer> =
                players.into_iter().filter(|p| p.permission == 0).collect();
            if found.is_empty() {
                Err("No player was found: everyone online is an operator".to_owned())
            } else {
                Ok(found)
            }
        }
        "@a" | "@r" if players.is_empty() => Err("No player was found".to_owned()),
        "@a" => Ok(players),
        "@r" => {
            let at = state.pick(players.len());
            Ok(players.into_iter().nth(at).into_iter().collect())
        }
        "@p" => {
            let me = own(players.clone())?;
            players
                .into_iter()
                .filter(|p| p.dimension == me.dimension)
                .min_by(|a, b| {
                    distance(a.position, me.position).total_cmp(&distance(b.position, me.position))
                })
                .map(|p| vec![p])
                .ok_or_else(|| "No player was found".to_owned())
        }
        word if word.starts_with('@') => {
            Err("Only @a, @p, @r and @s are supported here".to_owned())
        }
        word => players
            .into_iter()
            .find(|p| {
                p.name.eq_ignore_ascii_case(word)
                    || p.nickname
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(word))
            })
            .map(|p| vec![p])
            .ok_or_else(|| "Player was not found".to_owned()),
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn game_mode(
    state: &mut State,
    host: &mut dyn Host,
    actor: &Actor,
    line: &str,
    mode: Mode,
    target: Option<&str>,
) {
    let found = match target {
        Some(spec) => select(state, host, actor, spec).map_err(|why| (target_at(line), why)),
        None => runner(host, actor, line).map(|p| vec![p]),
    };
    let found = match found {
        Ok(found) => found,
        Err((at, why)) => return refuse(host, line, at, &why),
    };
    // Those already in the mode are left as they are.
    let changing: Vec<NativeServerPlayer> = found
        .iter()
        .filter(|p| p.game_mode != mode.id())
        .cloned()
        .collect();
    let mode_word = accent(mode.name());
    let own =
        |p: &NativeServerPlayer| matches!(actor, Actor::Player { uuid, .. } if *uuid == p.uuid);
    let said = match changing.as_slice() {
        [] => match found.as_slice() {
            [p] if own(p) => format!("<gray>You are already in {mode_word} mode</gray>"),
            [p] => format!(
                "<gray>{} is already in {mode_word} mode</gray>",
                accent(&p.name)
            ),
            many => format!(
                "<gray>All {} are already in {mode_word} mode</gray>",
                accent(&count(many.len(), "player"))
            ),
        },
        [p] if own(p) => format!("<white>Set your game mode to {mode_word}!</white>"),
        [p] => format!(
            "<white>Set {}'s game mode to {mode_word}!</white>",
            accent(&p.name)
        ),
        many => format!(
            "<white>Set {} gamemode to {mode_word}!</white>",
            accent(&count(many.len(), "player"))
        ),
    };
    for p in changing {
        state.push(Job::GameMode {
            uuid: p.uuid,
            name: p.name,
            mode,
            actor: actor.clone(),
        });
    }
    say(host, &said);
}

fn fly_speed(state: &mut State, host: &mut dyn Host, actor: &Actor, line: &str, typed: &str) {
    let player = match runner(host, actor, line) {
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
            "<white>Set your flight speed to {}!</white>",
            accent(&level.to_string())
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_is_not_ours_is_not_read() {
        for line in ["gamerule keepInventory true", "gmcc", "fly", "", "  "] {
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
    fn vanillas_gamemode_is_read_as_the_shortcut_for_its_mode() {
        for (line, mode) in [
            ("gamemode creative", Mode::Creative),
            ("/gamemode survival", Mode::Survival),
            ("GAMEMODE Adventure", Mode::Adventure),
            ("gamemode spectator", Mode::Spectator),
        ] {
            assert_eq!(
                parse(line),
                Some(Command::Mode { mode, target: None }),
                "{line}"
            );
        }
        assert_eq!(
            parse("gamemode creative Bob"),
            Some(Command::Mode {
                mode: Mode::Creative,
                target: Some("Bob".into())
            })
        );
        assert_eq!(
            parse("gamemode creative --non-ops"),
            Some(Command::Mode {
                mode: Mode::Creative,
                target: Some("--non-ops".into())
            })
        );
        // Every line of the name is the mod's, since vanilla's is replaced: a wrong one is told.
        for line in [
            "gamemode",
            "gamemode 1",
            "gamemode flying",
            "gamemode creative a b",
        ] {
            assert_eq!(parse(line), Some(Command::Usage(GAMEMODE_USAGE)), "{line}");
        }
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
        // A shortcut and vanilla's `/gamemode <mode>` for each of four modes, each with targets and
        // with `--non-ops`, and `/flyspeed`.
        assert_eq!(commands.len(), 4 * 2 * 2 + 1);
        for command in &commands {
            assert_eq!(command.level, 2);
            // Only `/gamemode` is vanilla's name, and the mod answers all of it.
            assert_eq!(
                command.replace_vanilla,
                command.usage.starts_with("gamemode "),
                "{}",
                command.usage
            );
        }
        let node = |usage: &str| {
            commands
                .iter()
                .find(|c| c.usage == usage)
                .and_then(|c| c.permission.clone())
        };
        assert_eq!(node("gmsp [targets:players]").as_deref(), Some(GAME_MODE));
        assert_eq!(node("flyspeed <speed:number>").as_deref(), Some(FLY_SPEED));
    }
}
