//! `/discs give|reload`, and vanilla's `/enchant` on a held disc.
//!
//! Operator messages are in Rift's style (`rift_chat`): what happened in white with what it is about
//! in the accent colour, and a two-line error that shows the command up to the word at fault.

use crate::config::Config;
use crate::item::{self, Tier};
use crate::state::Task;
use horizon_client::mod_api::{Host, NativeCommand, NativeServerPlayer, Suggestion};
use rift_chat::{count, escape, mini};

/// The permission node a permissions mod can grant, which operators of the level below have
/// without one.
pub const ADMIN: &str = "throwingdiscs.admin";
const OP_LEVEL: u8 = 2;

const USAGES: [&str; 2] = [
    "discs give <player:player> <tier:word> [amount:integer]",
    "discs reload",
];

const USAGE: &str = "give <player> <copper|iron|golden|diamond|netherite|all> [amount] | reload";

/// The command's usages, for the players' command trees.
pub fn register(commands: &mut Vec<NativeCommand>) {
    for usage in USAGES {
        commands.push(NativeCommand::op(usage, OP_LEVEL).permission(ADMIN));
    }
}

/// A line of the command, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Give {
        player: String,
        tier: String,
        amount: Option<String>,
    },
    Reload,
    Usage,
}

/// Reads `line` (with or without its slash). None when it is not a `discs` command.
pub fn parse(line: &str) -> Option<Command> {
    let mut words = line.trim().trim_start_matches('/').split_whitespace();
    let head = words.next()?;
    if !head.eq_ignore_ascii_case("discs") {
        return None;
    }
    let args: Vec<&str> = words.collect();
    Some(
        match args.first().map(|w| w.to_ascii_lowercase()).as_deref() {
            Some("reload") if args.len() == 1 => Command::Reload,
            Some("give") if (3..=4).contains(&args.len()) => Command::Give {
                player: args[1].to_owned(),
                tier: args[2].to_owned(),
                amount: args.get(3).map(|a| (*a).to_owned()),
            },
            _ => Command::Usage,
        },
    )
}

fn say(host: &mut dyn Host, markup: &str) {
    host.feedback(mini(markup));
}

/// A command that cannot be run: the header, then the typed command up to the `words`th word, which
/// is the one at fault, and why.
fn refuse(host: &mut dyn Host, line: &str, words: usize, markup: &str) {
    let [header, context] = rift_chat::error(line, words, markup);
    host.feedback(header);
    host.feedback(context);
}

fn words(line: &str) -> usize {
    line.split_whitespace().count()
}

/// Runs `line` for an operator or the console. What it asks of the mod besides answering: whether to
/// reload the config.
pub enum Done {
    NotOurs,
    Ran,
    Reload,
}

pub fn run(host: &mut dyn Host, config: &Config, line: &str) -> Done {
    let Some(command) = parse(line) else {
        return Done::NotOurs;
    };
    match command {
        Command::Give {
            player,
            tier,
            amount,
        } => give(host, config, line, &player, &tier, amount.as_deref()),
        Command::Reload => return Done::Reload,
        Command::Usage => refuse(
            host,
            line,
            words(line),
            &format!("Usage: <accent>/discs</accent> {}", escape(USAGE)),
        ),
    }
    Done::Ran
}

fn give(
    host: &mut dyn Host,
    config: &Config,
    line: &str,
    player: &str,
    tier: &str,
    amount: Option<&str>,
) {
    let Some(target) = host
        .server_players()
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(player))
    else {
        return refuse(host, line, 3, "Player was not found");
    };
    let amount = match amount {
        None => 1,
        Some(text) => match text.parse::<i64>() {
            Ok(n) => n.clamp(1, 64) as usize,
            Err(_) => return refuse(host, line, words(line), "The amount must be a number"),
        },
    };
    let tiers: Vec<Tier> = if tier.eq_ignore_ascii_case("all") {
        Tier::ALL.to_vec()
    } else {
        match Tier::by_id(tier) {
            Some(t) => vec![t],
            None => {
                return refuse(
                    host,
                    line,
                    4,
                    "Unknown tier: use <accent>copper</accent>, <accent>iron</accent>, <accent>golden</accent>, <accent>diamond</accent>, <accent>netherite</accent> or <accent>all</accent>",
                );
            }
        }
    };
    let mut given = 0;
    for tier in tiers {
        for _ in 0..amount {
            let Some(disc) = item::create(&*host, tier, config) else {
                return say(
                    host,
                    "<red>The disc could not be made: this server has no echo shard.</red>",
                );
            };
            // What does not fit in their inventory lands at their feet.
            if let Some(left) = host.give_stack(target.uuid, disc) {
                crate::flight::drop_item(host, &target.dimension, target.position, &left);
            }
            given += 1;
        }
    }
    say(
        host,
        &format!(
            "<white>Gave <accent>{}</accent> to <accent>{}</accent>!</white>",
            escape(&count(given, "throwing disc")),
            escape(&target.name)
        ),
    );
}

/// The tiers and `all` for the third word of `give`.
pub fn completions(before: &[&str], typing: &str, range: (usize, usize)) -> Vec<Suggestion> {
    let candidates: Vec<String> = match before {
        ["give", _] => Tier::ALL
            .iter()
            .map(|t| t.id().to_owned())
            .chain(["all".to_owned()])
            .collect(),
        _ => return Vec::new(),
    };
    let typed = typing.to_ascii_lowercase();
    candidates
        .into_iter()
        .filter(|c| c.starts_with(&typed))
        .map(|text| Suggestion {
            range,
            text,
            tooltip: None,
            int_value: None,
        })
        .collect()
}

// ---- /enchant ------------------------------------------------------------------------------

/// How an `/enchant` line was dealt with.
pub enum Enchant {
    /// Vanilla runs it: not about discs, or something this does not read.
    Vanilla,
    /// Answered here; the tasks do the enchanting at the next tick.
    Handled(Vec<Task>),
}

/// Makes vanilla's `/enchant` work on discs, which vanilla refuses because echo shards are in no
/// enchantment's item list. It takes the line when every target is a player named, `@s`, `@p` or
/// `@a` holding a disc in the main hand; any other line runs in vanilla.
pub fn enchant(
    host: &mut dyn Host,
    player: &NativeServerPlayer,
    line: &str,
    players: &[NativeServerPlayer],
) -> Enchant {
    let words: Vec<&str> = line
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .collect();
    if !(3..=4).contains(&words.len())
        || !(words[0].eq_ignore_ascii_case("enchant")
            || words[0].eq_ignore_ascii_case("minecraft:enchant"))
        || line.contains('[')
        || !host.has_permission(player.uuid, "minecraft.command.enchant", 2)
    {
        return Enchant::Vanilla;
    }
    let Some(targets) = resolve(player, words[1], players) else {
        return Enchant::Vanilla;
    };
    let holds_disc =
        |host: &dyn Host, uuid| host.held_stack(uuid).is_some_and(|s| item::is_disc(&s));
    if targets.is_empty() || !targets.iter().all(|p| holds_disc(&*host, p.uuid)) {
        return Enchant::Vanilla;
    }

    let id = if words[2].contains(':') {
        words[2].to_ascii_lowercase()
    } else {
        format!("minecraft:{}", words[2].to_ascii_lowercase())
    };
    let refuse_here = |host: &mut dyn Host, at: usize, why: &str| {
        refuse(host, line, at, why);
        Enchant::Handled(Vec::new())
    };
    let Some(max) = item::allowed_level(&id) else {
        return refuse_here(
            host,
            3,
            &format!(
                "Throwing discs can't have <accent>{}</accent>",
                escape(&plain_name(&id))
            ),
        );
    };
    let level = match words.get(3) {
        None => 1,
        Some(text) => match text.parse::<u32>() {
            Ok(n) if n >= 1 => n,
            _ => return refuse_here(host, 4, "The level must be a whole number of at least 1"),
        },
    };
    if level > max {
        return refuse_here(
            host,
            words.len(),
            &format!(
                "<accent>{}</accent> on a throwing disc goes up to level <accent>{max}</accent>",
                escape(&plain_name(&id))
            ),
        );
    }

    let tasks = targets
        .iter()
        .map(|p| Task::Enchant {
            uuid: p.uuid,
            enchantment: id.clone(),
            level,
        })
        .collect();
    let target = match targets.as_slice() {
        [one] => format!("{}'s item", escape(&one.name)),
        many => format!("{} items", many.len()),
    };
    say(
        host,
        &format!(
            "<white>Applied enchantment <accent>{} {}</accent> to <accent>{target}</accent>!</white>",
            escape(&plain_name(&id)),
            roman(level)
        ),
    );
    Enchant::Handled(tasks)
}

/// The players a word names: a name, `@s`, `@p` (the nearest) or `@a`. None for anything else.
fn resolve<'a>(
    me: &'a NativeServerPlayer,
    word: &str,
    players: &'a [NativeServerPlayer],
) -> Option<Vec<&'a NativeServerPlayer>> {
    match word.to_ascii_lowercase().as_str() {
        "@s" => Some(vec![me]),
        "@a" => Some(players.iter().collect()),
        "@p" => players
            .iter()
            .filter(|p| p.dimension == me.dimension)
            .min_by(|a, b| {
                let d = |p: &NativeServerPlayer| {
                    (0..3)
                        .map(|i| (p.position[i] - me.position[i]).powi(2))
                        .sum::<f64>()
                };
                d(a).total_cmp(&d(b))
            })
            .map(|p| vec![p]),
        w if w.starts_with('@') => None,
        _ => players
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(word))
            .map(|p| vec![p]),
    }
}

/// `minecraft:sharpness` as `Sharpness`.
fn plain_name(id: &str) -> String {
    let name = id.rsplit(':').next().unwrap_or(id).replace('_', " ");
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn roman(level: u32) -> String {
    ["", "I", "II", "III", "IV", "V"]
        .get(level as usize)
        .map_or_else(|| level.to_string(), |s| (*s).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_is_not_ours_is_not_read() {
        assert_eq!(parse("gamemode creative"), None);
        assert_eq!(parse("disc"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn give_and_reload_are_read_in_any_case_with_an_optional_amount() {
        assert_eq!(
            parse("/DISCS give Bob iron"),
            Some(Command::Give {
                player: "Bob".into(),
                tier: "iron".into(),
                amount: None
            })
        );
        assert_eq!(
            parse("discs  give Bob all 3"),
            Some(Command::Give {
                player: "Bob".into(),
                tier: "all".into(),
                amount: Some("3".into())
            })
        );
        assert_eq!(parse("discs RELOAD"), Some(Command::Reload));
    }

    #[test]
    fn a_line_said_wrongly_gets_the_usage() {
        for line in [
            "discs",
            "discs give",
            "discs give Bob",
            "discs give Bob iron 1 2",
            "discs reload now",
            "discs dance",
        ] {
            assert_eq!(parse(line), Some(Command::Usage), "{line}");
        }
    }

    #[test]
    fn the_command_is_registered_for_operators_and_the_admin_permission() {
        let mut commands = Vec::new();
        register(&mut commands);
        assert_eq!(commands.len(), 2);
        for command in &commands {
            assert_eq!(command.level, 2);
            assert_eq!(command.permission.as_deref(), Some("throwingdiscs.admin"));
            assert!(command.usage.starts_with("discs "));
        }
    }

    #[test]
    fn the_tiers_and_all_complete_after_a_player() {
        let range = (0, 0);
        let texts = |before: &[&str], typing: &str| -> Vec<String> {
            completions(before, typing, range)
                .into_iter()
                .map(|s| s.text)
                .collect()
        };
        assert_eq!(
            texts(&["give", "Bob"], ""),
            ["copper", "iron", "golden", "diamond", "netherite", "all"]
        );
        assert_eq!(texts(&["give", "Bob"], "d"), ["diamond"]);
        assert!(texts(&["give"], "").is_empty());
    }

    #[test]
    fn enchantments_are_named_and_levels_written_in_roman_numerals() {
        assert_eq!(plain_name("minecraft:sharpness"), "Sharpness");
        assert_eq!(plain_name("minecraft:fire_aspect"), "Fire aspect");
        assert_eq!(roman(5), "V");
        assert_eq!(roman(7), "7");
    }
}
