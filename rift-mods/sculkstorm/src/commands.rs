//! `/sculkstorm give <player> | blast <x> <y> <z> [world] | undo | reload`, as the plugin has it.

use crate::SculkStorm;
use crate::cataclysm::Blast;
use crate::item;
use crate::tasks::Task;
use horizon_client::mod_api::{Host, NativeCommand, NativeServerPlayer, Tag, Text, parse_snbt};

/// Who may run the command: the permission node the plugin's `plugin.yml` has, or an operator of
/// level 2 where nothing grants it.
pub const ADMIN: &str = "sculkstorm.admin";
const OP_LEVEL: u8 = 2;

const AQUA: u32 = 0x55FFFF;
const RED: u32 = 0xFF5555;
const GRAY: u32 = 0xAAAAAA;

const USAGES: [&str; 4] = [
    "sculkstorm give <player:player_name>",
    "sculkstorm blast <x:number> <y:number> <z:number> [world:word]",
    "sculkstorm undo",
    "sculkstorm reload",
];

const USAGE: &str = "Usage: /sculkstorm give <player> | blast <x> <y> <z> [world] | undo | reload";

/// The command's usages, for the players' command trees.
pub fn register(commands: &mut Vec<NativeCommand>) {
    for usage in USAGES {
        commands.push(NativeCommand::op(usage, OP_LEVEL).permission(ADMIN));
    }
}

/// A line of the command, read.
#[derive(Clone, Debug, PartialEq)]
pub enum Command<'a> {
    Give(&'a str),
    Blast {
        at: [f64; 3],
        world: Option<&'a str>,
    },
    Undo,
    Reload,
    /// `blast` with coordinates that are not numbers.
    BadBlast,
    /// Our command, said wrongly.
    Usage,
}

/// Reads `line` (with or without its slash). None when it is not a `sculkstorm` command.
pub fn parse(line: &str) -> Option<Command<'_>> {
    let mut words = line.trim().trim_start_matches('/').split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("sculkstorm") {
        return None;
    }
    let args: Vec<&str> = words.collect();
    let Some(first) = args.first() else {
        return Some(Command::Usage);
    };
    Some(match first.to_ascii_lowercase().as_str() {
        "reload" => Command::Reload,
        "undo" => Command::Undo,
        "give" if args.len() >= 2 => Command::Give(args[1]),
        "blast" if args.len() >= 4 => {
            let number = |text: &str| text.parse::<f64>().ok().filter(|n| n.is_finite());
            match (number(args[1]), number(args[2]), number(args[3])) {
                (Some(x), Some(y), Some(z)) => Command::Blast {
                    at: [x, y, z],
                    world: args.get(4).copied(),
                },
                _ => Command::BadBlast,
            }
        }
        _ => Command::Usage,
    })
}

fn say(host: &mut dyn Host, text: impl Into<String>, color: u32) {
    host.feedback(Text::colored(text.into(), color));
}

/// Runs `line` as `player` (None for the console). Whether it was a `sculkstorm` command.
pub fn run(
    module: &SculkStorm,
    host: &mut dyn Host,
    player: Option<&NativeServerPlayer>,
    line: &str,
) -> bool {
    let Some(command) = parse(line) else {
        return false;
    };
    match command {
        Command::Give(name) => give(host, name),
        Command::Blast { at, world } => blast(module, host, player, at, world),
        Command::Undo => undo(module, host),
        Command::Reload => match module.reload_config(&*host) {
            Ok(()) => say(host, "Sculkstorm config reloaded.", AQUA),
            Err(e) => {
                host.log(
                    "warn",
                    &format!("Sculkstorm: could not read the config: {e}"),
                );
                say(
                    host,
                    format!("Could not read the config, keeping the old one: {e}"),
                    RED,
                );
            }
        },
        Command::BadBlast | Command::Usage => say(host, USAGE, RED),
    }
    true
}

/// The entity data of the scythe lying on the ground, for what did not fit in an inventory.
fn dropped_item() -> Option<horizon_client::mod_api::Compound> {
    let (stack, _) = parse_snbt(&item::snbt()).ok()?;
    match Tag::compound([("Item", stack), ("PickupDelay", Tag::Short(10))]) {
        Tag::Compound(compound) => Some(compound),
        _ => None,
    }
}

fn give(host: &mut dyn Host, name: &str) {
    let target = host
        .server_players()
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name));
    let Some(target) = target else {
        say(host, "Player not found.", RED);
        return;
    };
    let Some(stack) = item::create(&*host) else {
        say(
            host,
            "The scythe could not be made: this server has no netherite hoe.",
            RED,
        );
        return;
    };
    // What does not fit in their inventory lands at their feet.
    if host.give_stack(target.uuid, stack).is_some()
        && let Some(nbt) = dropped_item()
    {
        host.summon(&target.dimension, "minecraft:item", target.position, nbt);
    }
    say(
        host,
        format!("Gave {} to {}.", item::DISPLAY_NAME, target.name),
        AQUA,
    );
}

fn blast(
    module: &SculkStorm,
    host: &mut dyn Host,
    player: Option<&NativeServerPlayer>,
    at: [f64; 3],
    world: Option<&str>,
) {
    let dimension = match world {
        Some(name) => host.dimension_of_world(name).or_else(|| {
            // A dimension's own id is taken too.
            host.world_of_dimension(name).map(|_| name.to_owned())
        }),
        None => Some(
            player
                .map(|p| p.dimension.clone())
                .unwrap_or_else(|| "minecraft:overworld".to_owned()),
        ),
    };
    let Some(dimension) = dimension else {
        say(host, "Unknown world.", RED);
        return;
    };
    {
        let mut state = module.shared().state();
        state.cataclysm_started();
        state.tasks.push(Task::Blast(Blast::new(&dimension, at)));
    }
    say(
        host,
        format!(
            "Cataclysm set off at {} {} {}.",
            at[0].floor() as i64,
            at[1].floor() as i64,
            at[2].floor() as i64
        ),
        AQUA,
    );
}

fn undo(module: &SculkStorm, host: &mut dyn Host) {
    let tick = module.shared().tick();
    let craters = module.shared().state().craters.undo_all(tick);
    if craters == 0 {
        say(
            host,
            "Nothing to undo: no crater is waiting to be restored.",
            GRAY,
        );
    } else {
        let plural = if craters == 1 { "" } else { "s" };
        say(
            host,
            format!("Undoing {craters} crater{plural}, the land is being put back."),
            AQUA,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_is_not_ours_is_not_read() {
        assert_eq!(parse("gamemode creative"), None);
        assert_eq!(parse("sculk"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
    }

    #[test]
    fn the_four_subcommands_are_read_with_or_without_the_slash_in_any_case() {
        assert_eq!(parse("sculkstorm reload"), Some(Command::Reload));
        assert_eq!(parse("/SculkStorm UNDO"), Some(Command::Undo));
        assert_eq!(parse("sculkstorm give Alex"), Some(Command::Give("Alex")));
        assert_eq!(
            parse("sculkstorm blast 10 64.5 -20"),
            Some(Command::Blast {
                at: [10.0, 64.5, -20.0],
                world: None
            })
        );
        assert_eq!(
            parse("  sculkstorm   blast 1 2 3   world_nether "),
            Some(Command::Blast {
                at: [1.0, 2.0, 3.0],
                world: Some("world_nether")
            })
        );
    }

    #[test]
    fn a_subcommand_said_wrongly_gets_the_usage() {
        assert_eq!(parse("sculkstorm"), Some(Command::Usage));
        assert_eq!(parse("sculkstorm give"), Some(Command::Usage));
        assert_eq!(parse("sculkstorm blast 1 2"), Some(Command::Usage));
        assert_eq!(parse("sculkstorm dance"), Some(Command::Usage));
        assert_eq!(parse("sculkstorm blast a b c"), Some(Command::BadBlast));
        assert_eq!(parse("sculkstorm blast 1 NaN 3"), Some(Command::BadBlast));
        assert_eq!(parse("sculkstorm blast 1 inf 3"), Some(Command::BadBlast));
    }

    #[test]
    fn the_command_is_registered_for_operators_of_level_two_and_the_admin_permission() {
        let mut commands = Vec::new();
        register(&mut commands);
        assert_eq!(commands.len(), 4);
        for command in &commands {
            assert_eq!(command.level, 2);
            assert_eq!(command.permission.as_deref(), Some("sculkstorm.admin"));
            assert!(!command.replace_vanilla);
            assert!(command.usage.starts_with("sculkstorm "));
        }
        let usages: Vec<&str> = commands.iter().map(|c| c.usage.as_str()).collect();
        assert!(usages.contains(&"sculkstorm give <player:player_name>"));
        assert!(usages.contains(&"sculkstorm blast <x:number> <y:number> <z:number> [world:word]"));
    }

    #[test]
    fn a_dropped_scythe_is_an_item_entity_holding_the_scythe() {
        let nbt = Tag::Compound(dropped_item().expect("the item reads"));
        assert!(nbt.get("Item").is_some());
        assert_eq!(
            nbt.get("Item").and_then(|i| i.get("id")),
            Some(&Tag::String("minecraft:netherite_hoe".into()))
        );
        assert_eq!(nbt.get("PickupDelay"), Some(&Tag::Short(10)));
    }
}
