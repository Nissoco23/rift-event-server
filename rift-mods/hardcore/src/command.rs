//! `/deathprotect`, `/ondeath`, `/deathmsg` and `/deathmsgradius`: reading a line, finding the
//! players it names, and answering it.
//!
//! Operator messages are in Rift's style (`rift_chat`): what happened in white with what it is about
//! in the accent colour, and a two-line error that shows the command up to the word at fault. A
//! command is a function of the state and the line: it changes the state and says what to tell the
//! one who ran it and what the mod must do about it (save, change the join and leave lines).

use crate::state::{CycleInventory, CycleMode, DeathMsg, MAX_DISTANCE, OnDeath, State};
use horizon_client::mod_api::{NativeCommand, NativeServerPlayer, Suggestion, Text};
use rift_chat::{count, escape, mini};

/// The permission node a permissions mod can grant, which operators of the level below have
/// without one.
pub const ADMIN: &str = "hardcore.admin";
pub const OP_LEVEL: u8 = 2;

const USAGES: &[&str] = &[
    "deathprotect",
    "deathprotect <target:text?>",
    "deathprotect on [targets:text?]",
    "deathprotect off [targets:text?]",
    "deathprotect level <level:integer> [targets:text?]",
    "ondeath",
    "ondeath spectate",
    "ondeath kick",
    "ondeath none",
    "ondeath admin",
    "ondeath admin spectate",
    "ondeath admin kick",
    "ondeath admin none",
    "deathmsg",
    "deathmsg none",
    "deathmsg death",
    "deathmsg leave",
    "deathmsgradius",
    "deathmsgradius <radius:integer>",
    "cycledeath",
    "cycledeath on",
    "cycledeath off",
    "cycledeath mode deathpoint",
    "cycledeath mode spawnpoint",
    "cycledeath setspawn",
    "cycledeath setspawn <pos:block_pos>",
    "cycledeath inventory drop",
    "cycledeath inventory keep",
    "cycledeath inventory both",
    "cycledeath distance <blocks:integer>",
    "cycledeath exempt <player:text?>",
];

const DEATHPROTECT_USAGE: &str =
    "Usage: /deathprotect [player] | on|off [targets] | level <1-20> [targets]";
const ONDEATH_USAGE: &str = "Usage: /ondeath [admin] <spectate|kick|none>";
const DEATHMSG_USAGE: &str = "Usage: /deathmsg <none|death|leave>";
const RADIUS_USAGE: &str = "Usage: /deathmsgradius <blocks, or -1 for everyone>";
const CYCLE_USAGE: &str = "Usage: /cycledeath [on|off] | mode <deathpoint|spawnpoint> | setspawn [x y z] | inventory <drop|keep|both> | distance <blocks> | exempt <player>";

/// The command's usages, for the players' command trees.
pub fn register(commands: &mut Vec<NativeCommand>) {
    for usage in USAGES {
        commands.push(NativeCommand::op(usage, OP_LEVEL).permission(ADMIN));
    }
}

/// Who ran a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    Player { uuid: u128, name: String },
    Console,
}

/// A line of a command, read. Words are as typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `/deathprotect [player]`: switch one player's protection.
    Toggle(Option<String>),
    /// `/deathprotect on|off [targets]`.
    Set { on: bool, who: Option<String> },
    /// `/deathprotect level <level> [targets]`.
    Level { level: String, who: Option<String> },
    /// `/ondeath [admin] [mode]`.
    OnDeath { admin: bool, mode: Option<String> },
    /// `/deathmsg [mode]`.
    DeathMsg(Option<String>),
    /// `/deathmsgradius [radius]`.
    Radius(Option<String>),
    /// `/cycledeath ...`.
    Cycle(CycleCommand),
    /// One of ours, said wrongly: its usage.
    Usage(&'static str),
}

/// A line of `/cycledeath`, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CycleCommand {
    Status,
    Switch(bool),
    Mode(String),
    /// The coordinates as typed: none, or three.
    SetSpawn(Vec<String>),
    Inventory(String),
    Distance(String),
    Exempt(String),
}

/// Reads `line` (with or without its slash). None when it is not one of ours.
pub fn parse(line: &str) -> Option<Command> {
    let mut words = line.trim().trim_start_matches('/').split_whitespace();
    let head = words.next()?.to_ascii_lowercase();
    let args: Vec<&str> = words.collect();
    let lower: Vec<String> = args.iter().map(|w| w.to_ascii_lowercase()).collect();
    let l: Vec<&str> = lower.iter().map(String::as_str).collect();
    let text = |i: usize| args[i].to_owned();
    Some(match head.as_str() {
        "deathprotect" => match l.as_slice() {
            [] => Command::Toggle(None),
            ["on" | "off", rest @ ..] if rest.len() <= 1 => Command::Set {
                on: l[0] == "on",
                who: rest.first().map(|_| text(1)),
            },
            ["level", _] => Command::Level {
                level: text(1),
                who: None,
            },
            ["level", _, _] => Command::Level {
                level: text(1),
                who: Some(text(2)),
            },
            ["level"] => Command::Usage(DEATHPROTECT_USAGE),
            [_] => Command::Toggle(Some(text(0))),
            _ => Command::Usage(DEATHPROTECT_USAGE),
        },
        "ondeath" => match l.as_slice() {
            [] => Command::OnDeath {
                admin: false,
                mode: None,
            },
            ["admin"] => Command::OnDeath {
                admin: true,
                mode: None,
            },
            ["admin", _] => Command::OnDeath {
                admin: true,
                mode: Some(text(1)),
            },
            [_] => Command::OnDeath {
                admin: false,
                mode: Some(text(0)),
            },
            _ => Command::Usage(ONDEATH_USAGE),
        },
        "deathmsg" => match l.as_slice() {
            [] => Command::DeathMsg(None),
            [_] => Command::DeathMsg(Some(text(0))),
            _ => Command::Usage(DEATHMSG_USAGE),
        },
        "deathmsgradius" => match l.as_slice() {
            [] => Command::Radius(None),
            [_] => Command::Radius(Some(text(0))),
            _ => Command::Usage(RADIUS_USAGE),
        },
        "cycledeath" => match l.as_slice() {
            [] => Command::Cycle(CycleCommand::Status),
            ["on"] => Command::Cycle(CycleCommand::Switch(true)),
            ["off"] => Command::Cycle(CycleCommand::Switch(false)),
            ["mode", _] => Command::Cycle(CycleCommand::Mode(text(1))),
            ["setspawn"] => Command::Cycle(CycleCommand::SetSpawn(Vec::new())),
            ["setspawn", _, _, _] => {
                Command::Cycle(CycleCommand::SetSpawn(vec![text(1), text(2), text(3)]))
            }
            ["inventory", _] => Command::Cycle(CycleCommand::Inventory(text(1))),
            ["distance", _] => Command::Cycle(CycleCommand::Distance(text(1))),
            ["exempt", _] => Command::Cycle(CycleCommand::Exempt(text(1))),
            _ => Command::Usage(CYCLE_USAGE),
        },
        _ => return None,
    })
}

/// What a command asks of the mod besides answering.
#[derive(Default)]
pub struct Outcome {
    pub replies: Vec<Text>,
    /// The file must be written again.
    pub save: bool,
    /// The join and leave lines must be set to what `deathmsg` says.
    pub lines: bool,
}

impl Outcome {
    fn say(&mut self, markup: &str) {
        self.replies.push(mini(markup));
    }

    /// A command that cannot be run: the header, then the typed command up to the `words`th word,
    /// which is the one at fault, and why.
    fn refuse(&mut self, line: &str, words: usize, markup: &str) {
        self.replies.extend(rift_chat::error(line, words, markup));
    }
}

fn words(line: &str) -> usize {
    line.split_whitespace().count()
}

/// Half a heart is a health point: `0.5 hearts`, `1 heart`, `1.5 hearts`, `2 hearts`.
pub fn hearts(level: u8) -> String {
    let whole = level / 2;
    if level % 2 == 1 {
        format!("{whole}.5 hearts")
    } else if whole == 1 {
        "1 heart".to_owned()
    } else {
        format!("{whole} hearts")
    }
}

/// The name others know the player by.
pub fn shown(player: &NativeServerPlayer) -> &str {
    player.nickname.as_deref().unwrap_or(&player.name)
}

// ---- who -----------------------------------------------------------------------------------

/// The players a word names: a nickname, or else a name (any case), or `@s`, `@p` (the nearest to
/// the one who ran it, themselves included, as in vanilla), `@r` or `@a`. `@a` only where `many`.
/// Options in brackets are not read. The errors are plain text.
fn find(
    state: &mut State,
    actor: &Actor,
    spec: &str,
    many: bool,
) -> Result<Vec<NativeServerPlayer>, String> {
    if spec.contains('[') {
        return Err(
            "Selector options are not supported here: use @a, @p, @r, @s or a name".to_owned(),
        );
    }
    let no_player = "The console is no player: name one".to_owned();
    let ordinary = |state: &State| -> Vec<NativeServerPlayer> {
        state.roster.iter().filter(|p| !p.fake).cloned().collect()
    };
    match spec.to_ascii_lowercase().as_str() {
        "@s" => match actor {
            Actor::Player { uuid, .. } => state
                .member(*uuid)
                .cloned()
                .map(|p| vec![p])
                .ok_or_else(|| "You are not on the server".to_owned()),
            Actor::Console => Err(no_player),
        },
        "@p" => {
            let Actor::Player { uuid, .. } = actor else {
                return Err(no_player);
            };
            let here = state
                .member(*uuid)
                .cloned()
                .ok_or_else(|| "You are not on the server".to_owned())?;
            ordinary(state)
                .into_iter()
                .filter(|p| p.dimension == here.dimension)
                .min_by(|a, b| {
                    distance(a.position, here.position)
                        .total_cmp(&distance(b.position, here.position))
                })
                .map(|p| vec![p])
                .ok_or_else(|| "No player was found near you".to_owned())
        }
        "@r" => {
            let pool = ordinary(state);
            if pool.is_empty() {
                return Err("No player was found".to_owned());
            }
            let pick = state.rng.below(pool.len() as u32) as usize;
            Ok(vec![pool[pick].clone()])
        }
        "@a" if many => {
            let all = ordinary(state);
            if all.is_empty() {
                Err("No player was found".to_owned())
            } else {
                Ok(all)
            }
        }
        "@a" => Err("One player only here: name one, or use @s, @p or @r".to_owned()),
        s if s.starts_with('@') => Err("Only @a, @p, @r and @s are supported here".to_owned()),
        _ => by_name(state, spec)
            .map(|p| vec![p])
            .ok_or_else(|| "Player was not found".to_owned()),
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// The player going by `name` as a nickname, or else logged in as it, in any case. What a player
/// sees is the nickname, so it comes first when one player's nickname is another's name.
fn by_name(state: &State, name: &str) -> Option<NativeServerPlayer> {
    state
        .roster
        .iter()
        .find(|p| {
            p.nickname
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .or_else(|| {
            state
                .roster
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))
        })
        .cloned()
}

/// The one who ran a command, as the roster has them.
fn myself(state: &State, actor: &Actor) -> Result<NativeServerPlayer, String> {
    match actor {
        Actor::Player { uuid, .. } => state
            .member(*uuid)
            .cloned()
            .ok_or_else(|| "You are not on the server".to_owned()),
        Actor::Console => Err("The console has no death protection: name a player".to_owned()),
    }
}

fn is_self(actor: &Actor, player: &NativeServerPlayer) -> bool {
    matches!(actor, Actor::Player { uuid, .. } if *uuid == player.uuid)
}

// ---- running -------------------------------------------------------------------------------

/// Runs `line` as `actor`, over the roster in `state`. None when it is not one of ours.
pub fn run(state: &mut State, actor: &Actor, line: &str) -> Option<Outcome> {
    let command = parse(line)?;
    let mut out = Outcome::default();
    match command {
        Command::Toggle(who) => toggle(state, actor, line, who.as_deref(), &mut out),
        Command::Set { on, who } => set(state, actor, line, on, who.as_deref(), &mut out),
        Command::Level { level, who } => {
            level_command(state, actor, line, &level, who.as_deref(), &mut out);
        }
        Command::OnDeath { admin, mode } => on_death(state, line, admin, mode.as_deref(), &mut out),
        Command::DeathMsg(mode) => death_msg(state, line, mode.as_deref(), &mut out),
        Command::Radius(radius) => radius_command(state, line, radius.as_deref(), &mut out),
        Command::Cycle(cycle) => cycle_command(state, actor, line, cycle, &mut out),
        Command::Usage(usage) => out.refuse(line, words(line), &escape(usage)),
    }
    Some(out)
}

fn toggle(state: &mut State, actor: &Actor, line: &str, who: Option<&str>, out: &mut Outcome) {
    let found = match who {
        Some(spec) => find(state, actor, spec, false).map(|mut v| v.remove(0)),
        None => myself(state, actor),
    };
    let target = match found {
        Ok(target) => target,
        Err(e) => {
            let at = if who.is_some() { 2 } else { words(line) };
            return out.refuse(line, at, &e);
        }
    };
    let on = !state.protection_of(target.uuid).enabled;
    state.protection.entry(target.uuid).or_default().enabled = on;
    out.save = true;
    announce(state, actor, &[target], &[true], on, out);
}

fn set(
    state: &mut State,
    actor: &Actor,
    line: &str,
    on: bool,
    who: Option<&str>,
    out: &mut Outcome,
) {
    let found = match who {
        Some(spec) => find(state, actor, spec, true),
        None => myself(state, actor).map(|p| vec![p]),
    };
    let targets = match found {
        Ok(targets) => targets,
        Err(e) => {
            let at = if who.is_some() { 3 } else { words(line) };
            return out.refuse(line, at, &e);
        }
    };
    let changed: Vec<bool> = targets
        .iter()
        .map(|t| state.protection_of(t.uuid).enabled != on)
        .collect();
    for target in &targets {
        state.protection.entry(target.uuid).or_default().enabled = on;
    }
    out.save = changed.contains(&true);
    announce(state, actor, &targets, &changed, on, out);
}

/// Tells who ran a command that `targets` have protection `on` or off now. `changed` says for each
/// whether it was a change: one who already had it is told so, and counted apart.
fn announce(
    state: &State,
    actor: &Actor,
    targets: &[NativeServerPlayer],
    changed: &[bool],
    on: bool,
    out: &mut Outcome,
) {
    let word = if on { "enabled" } else { "disabled" };
    match targets {
        [one] => {
            let name = escape(shown(one));
            let mine = is_self(actor, one);
            let whose = if mine {
                "Your".to_owned()
            } else {
                format!("<accent>{name}</accent>")
            };
            if !changed[0] {
                return out.say(&format!(
                    "<gray>{} death protection is already {word}</gray>",
                    if mine { "Your" } else { name.as_str() }
                ));
            }
            out.say(&format!(
                "<white>{whose} death protection has been <accent>{word}</accent></white>"
            ));
            if on {
                out.say(&format!(
                    "<gray>Minimum health: <accent>{}</accent></gray>",
                    hearts(state.protection_of(one.uuid).level)
                ));
            }
        }
        many => {
            let did = changed.iter().filter(|c| **c).count();
            let already = many.len() - did;
            if did == 0 {
                return out.say(&format!(
                    "<gray>Death protection is already {word} for {}</gray>",
                    count(already, "player")
                ));
            }
            out.say(&format!(
                "<white>Death protection has been <accent>{word}</accent> for <accent>{}</accent></white>",
                count(did, "player")
            ));
            if already > 0 {
                out.say(&format!("<gray>{already} already had it {word}</gray>"));
            }
        }
    }
}

fn level_command(
    state: &mut State,
    actor: &Actor,
    line: &str,
    level: &str,
    who: Option<&str>,
    out: &mut Outcome,
) {
    let level = match level.parse::<i64>() {
        Ok(n) if (1..=20).contains(&n) => n as u8,
        Ok(_) => {
            return out.refuse(
                line,
                3,
                "The level must be from 1 to 20 (half a heart each)",
            );
        }
        Err(_) => return out.refuse(line, 3, "The level must be a whole number from 1 to 20"),
    };
    let found = match who {
        Some(spec) => find(state, actor, spec, true),
        None => myself(state, actor).map(|p| vec![p]),
    };
    let targets = match found {
        Ok(targets) => targets,
        Err(e) => {
            let at = if who.is_some() { 4 } else { words(line) };
            return out.refuse(line, at, &e);
        }
    };
    for target in &targets {
        state.protection.entry(target.uuid).or_default().level = level;
    }
    out.save = true;
    let off = targets
        .iter()
        .filter(|t| !state.protection_of(t.uuid).enabled)
        .count();
    let hearts = hearts(level);
    match targets.as_slice() {
        [one] => {
            if is_self(actor, one) {
                out.say(&format!(
                    "<white>Your minimum health has been set to <accent>{hearts}</accent></white>"
                ));
                if off > 0 {
                    out.say("<gray>Death protection is off for you: <accent><click:run_command:'/deathprotect on'>/deathprotect on</click></accent> turns it on</gray>");
                }
            } else {
                let name = escape(shown(one));
                out.say(&format!(
                    "<white><accent>{name}</accent> minimum health has been set to <accent>{hearts}</accent></white>"
                ));
                if off > 0 {
                    out.say(&format!("<gray>Death protection is off for {name}</gray>"));
                }
            }
        }
        many => {
            out.say(&format!(
                "<white>Minimum health has been set to <accent>{hearts}</accent> for <accent>{}</accent></white>",
                count(many.len(), "player")
            ));
            if off > 0 {
                out.say(&format!(
                    "<gray>Death protection is off for {off} of them</gray>"
                ));
            }
        }
    }
}

/// What a death does to a player, in a sentence's end.
fn fate(mode: OnDeath) -> &'static str {
    match mode {
        OnDeath::Spectate => "put in spectator mode",
        OnDeath::Kick => "kicked",
        OnDeath::None => "left alone",
    }
}

fn on_death(state: &mut State, line: &str, admin: bool, mode: Option<&str>, out: &mut Outcome) {
    let who = if admin { "Operators" } else { "Players" };
    // Death cycling decides what a death does, so this option is not.
    if state.cycle.enabled {
        return match mode {
            None => {
                out.say(&format!(
                    "<gray>{who} who die are <accent>death cycled</accent>!</gray>"
                ));
                out.say("<gray>This option is currently being overridden by <accent>/cycledeath</accent>. Disable it to change this option</gray>");
            }
            Some(_) => out.refuse(
                line,
                words(line),
                "Death cycling is on and overrides this option: <accent><click:run_command:'/cycledeath off'>/cycledeath off</click></accent> turns it off",
            ),
        };
    }
    let Some(word) = mode else {
        let now = if admin {
            state.on_death_admin
        } else {
            state.on_death
        };
        return out.say(&format!(
            "<gray>{who} who die are <accent>{}</accent></gray>",
            fate(now)
        ));
    };
    let Some(mode) = OnDeath::parse(word) else {
        return out.refuse(line, words(line), &escape(ONDEATH_USAGE));
    };
    if admin {
        state.on_death_admin = mode;
    } else {
        state.on_death = mode;
    }
    out.say(&format!(
        "<white>{who} who die will now be <accent>{}</accent></white>",
        fate(mode)
    ));
}

fn death_msg(state: &mut State, line: &str, mode: Option<&str>, out: &mut Outcome) {
    let changed = mode.is_some();
    let mode = match mode {
        None => state.deathmsg,
        Some(word) => match DeathMsg::parse(word) {
            Some(mode) => {
                state.deathmsg = mode;
                out.save = true;
                out.lines = true;
                mode
            }
            None => return out.refuse(line, words(line), &escape(DEATHMSG_USAGE)),
        },
    };
    let what = match mode {
        DeathMsg::None => "A death tells nobody",
        DeathMsg::Death => "A death shows its death message",
        DeathMsg::Leave => {
            "A death shows a leave message, and the real join and leave messages are hidden"
        }
    };
    if changed {
        out.say(&format!(
            "<white>Death messages set to <accent>{}</accent></white>",
            mode.word()
        ));
    } else {
        out.say(&format!(
            "<gray>Death messages are set to <accent>{}</accent></gray>",
            mode.word()
        ));
    }
    out.say(&format!("<gray>{what}</gray>"));
}

fn radius_command(state: &mut State, line: &str, radius: Option<&str>, out: &mut Outcome) {
    let changed = radius.is_some();
    if let Some(text) = radius {
        match text.parse::<i64>() {
            Ok(n) if n >= -1 => {
                state.radius = i32::try_from(n).unwrap_or(i32::MAX);
                out.save = true;
            }
            _ => {
                return out.refuse(
                    line,
                    2,
                    "The radius must be a whole number of blocks, or -1 for everyone",
                );
            }
        }
    }
    let reach = match state.radius {
        r if r < 0 => "everyone".to_owned(),
        0 => "only the player who died".to_owned(),
        r => count(r as usize, "block"),
    };
    if changed {
        out.say(&format!(
            "<white>Death messages now reach <accent>{reach}</accent></white>"
        ));
    } else {
        out.say(&format!(
            "<gray>Death messages reach <accent>{reach}</accent></gray>"
        ));
    }
}

// ---- /cycledeath ---------------------------------------------------------------------------

/// A number of blocks, as a sentence has it.
fn blocks(n: i32) -> String {
    count(n.max(0) as usize, "block")
}

fn cycle_command(
    state: &mut State,
    actor: &Actor,
    line: &str,
    command: CycleCommand,
    out: &mut Outcome,
) {
    match command {
        CycleCommand::Status => cycle_status(state, out),
        CycleCommand::Switch(on) => {
            let word = if on { "enabled" } else { "disabled" };
            if state.cycle.enabled == on {
                return out.say(&format!("<gray>Death cycling is already {word}</gray>"));
            }
            state.cycle.enabled = on;
            state.revision += 1;
            out.save = true;
            out.say(&format!(
                "<white>Death cycling has been <accent>{word}</accent></white>"
            ));
        }
        CycleCommand::Mode(word) => {
            let Some(mode) = CycleMode::parse(&word) else {
                return out.refuse(line, 3, "Usage: /cycledeath mode <deathpoint|spawnpoint>");
            };
            if state.cycle.mode == mode {
                return out.say(&format!(
                    "<gray>Death cycled players already respawn <accent>{}</accent></gray>",
                    mode_phrase(mode)
                ));
            }
            state.cycle.mode = mode;
            state.revision += 1;
            out.save = true;
            out.say(&format!(
                "<white>Death cycled players now respawn <accent>{}</accent></white>",
                mode_phrase(mode)
            ));
        }
        CycleCommand::SetSpawn(tokens) => cycle_setspawn(state, actor, line, &tokens, out),
        CycleCommand::Inventory(word) => {
            let Some(inventory) = CycleInventory::parse(&word) else {
                return out.refuse(line, 3, "Usage: /cycledeath inventory <drop|keep|both>");
            };
            if state.cycle.inventory == inventory {
                return out.say(&format!(
                    "<gray>Death cycled players already <accent>{}</accent></gray>",
                    inventory_phrase(inventory)
                ));
            }
            state.cycle.inventory = inventory;
            out.save = true;
            out.say(&format!(
                "<white>Death cycled players now <accent>{}</accent></white>",
                inventory_phrase(inventory)
            ));
        }
        CycleCommand::Distance(text) => {
            let distance = match text.parse::<i64>() {
                Ok(n) if n < 0 => {
                    return out.refuse(line, 3, "The distance cannot be below 0");
                }
                Ok(n) if n > i64::from(MAX_DISTANCE) => {
                    return out.refuse(
                        line,
                        3,
                        &format!("The distance cannot be more than {}", blocks(MAX_DISTANCE)),
                    );
                }
                Ok(n) => n as i32,
                Err(_) => {
                    return out.refuse(line, 3, "The distance must be a whole number of blocks");
                }
            };
            if state.cycle.distance == distance {
                return out.say(&format!(
                    "<gray>The death cycle distance is already <accent>{}</accent></gray>",
                    blocks(distance)
                ));
            }
            state.cycle.distance = distance;
            state.revision += 1;
            out.save = true;
            out.say(&format!(
                "<white>Death cycle distance set to <accent>{}</accent></white>",
                blocks(distance)
            ));
        }
        CycleCommand::Exempt(spec) => {
            let target = match find(state, actor, &spec, false) {
                Ok(mut found) => found.remove(0),
                Err(e) => return out.refuse(line, 3, &e),
            };
            let name = escape(shown(&target));
            if state.exempt.remove(&target.uuid) {
                out.say(&format!(
                    "<white><accent>{name}</accent> is no longer exempt from death cycling</white>"
                ));
            } else {
                state.exempt.insert(target.uuid);
                out.say(&format!(
                    "<white><accent>{name}</accent> is now <accent>exempt</accent> from death cycling</white>"
                ));
            }
        }
    }
}

fn mode_phrase(mode: CycleMode) -> &'static str {
    match mode {
        CycleMode::DeathPoint => "around where they died",
        CycleMode::SpawnPoint => "around the spawn point",
    }
}

fn inventory_phrase(inventory: CycleInventory) -> &'static str {
    match inventory {
        CycleInventory::Drop => "drop their inventory",
        CycleInventory::Keep => "keep their inventory and drop nothing",
        CycleInventory::Both => "keep their inventory and drop a copy of it",
    }
}

/// `/cycledeath`: whether it is on and the settings that matter.
fn cycle_status(state: &State, out: &mut Outcome) {
    let cycle = &state.cycle;
    let on = if cycle.enabled { "on" } else { "off" };
    out.say(&format!(
        "<gray>Death cycling is <accent>{on}</accent></gray>"
    ));
    match cycle.mode {
        CycleMode::DeathPoint => out.say(&format!(
            "<gray>Mode: <accent>deathpoint</accent>, <accent>{}</accent> from where they died</gray>",
            blocks(cycle.distance)
        )),
        CycleMode::SpawnPoint => {
            let spawn = cycle.spawn.map_or_else(
                || "the world spawn".to_owned(),
                |[x, y, z]| format!("{x}, {y}, {z}"),
            );
            out.say(&format!(
                "<gray>Mode: <accent>spawnpoint</accent>, within <accent>{}</accent> of <accent>{spawn}</accent></gray>",
                blocks(cycle.distance)
            ));
        }
    }
    out.say(&format!(
        "<gray>Inventory: <accent>{}</accent>, and up to 100 points of experience always drop</gray>",
        cycle.inventory.word()
    ));
    let mut names: Vec<String> = state
        .exempt
        .iter()
        .map(|uuid| {
            state
                .member(*uuid)
                .map_or_else(|| crate::store::uuid_string(*uuid), |p| shown(p).to_owned())
        })
        .collect();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    let exempt = if names.is_empty() {
        "nobody".to_owned()
    } else {
        names
            .iter()
            .map(|n| escape(n))
            .collect::<Vec<_>>()
            .join(", ")
    };
    out.say(&format!("<gray>Exempt: <accent>{exempt}</accent></gray>"));
}

/// `/cycledeath setspawn [x y z]`: the point `spawnpoint` mode is around. With no coordinates, where
/// the one who ran it stands; `~` is their coordinate, as in vanilla.
fn cycle_setspawn(
    state: &mut State,
    actor: &Actor,
    line: &str,
    tokens: &[String],
    out: &mut Outcome,
) {
    let here = match actor {
        Actor::Player { uuid, .. } => state.member(*uuid).map(|p| p.position),
        Actor::Console => None,
    };
    let mut at = [0.0f64; 3];
    if tokens.is_empty() {
        let Some(here) = here else {
            return out.refuse(
                line,
                words(line),
                "The console has no position: give <accent>x y z</accent>",
            );
        };
        at = here;
    } else {
        for (k, token) in tokens.iter().enumerate() {
            let value = if let Some(offset) = token.strip_prefix('~') {
                let Some(here) = here else {
                    return out.refuse(line, 3 + k, "The console has no position for ~");
                };
                let offset = if offset.is_empty() {
                    Some(0.0)
                } else {
                    offset.parse::<f64>().ok()
                };
                offset.map(|o| here[k] + o)
            } else {
                token.parse::<f64>().ok()
            };
            match value {
                Some(v) if v.is_finite() && v.abs() <= 30_000_000.0 => at[k] = v,
                Some(_) => return out.refuse(line, 3 + k, "That is outside the world"),
                None => {
                    return out.refuse(
                        line,
                        3 + k,
                        "The coordinate must be a number, or ~ for yours",
                    );
                }
            }
        }
    }
    let block = [
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    ];
    state.cycle.spawn = Some(block);
    state.revision += 1;
    out.save = true;
    out.say(&format!(
        "<white>Death cycle spawn point set to <accent>{}, {}, {}</accent></white>",
        block[0], block[1], block[2]
    ));
}

// ---- completing ----------------------------------------------------------------------------

/// What a player completes while typing a target of `/deathprotect` or `/cycledeath exempt`: the nicknames of the players,
/// or their names where they have none, and the selectors the command takes there. The server's
/// players are out of reach in the hook, so the roster is what the last tick saw. Nothing for any
/// other command: its words are literals the client completes.
pub fn completions(
    state: &State,
    label: &str,
    arguments: &[String],
    range: (usize, usize),
) -> Vec<Suggestion> {
    let Some((typing, before)) = arguments.split_last() else {
        return Vec::new();
    };
    let before: Vec<String> = before.iter().map(|w| w.to_ascii_lowercase()).collect();
    let before: Vec<&str> = before.iter().map(String::as_str).collect();
    // Whether the word being typed is a player, and whether several may be named there.
    let many = match (label.to_ascii_lowercase().as_str(), before.as_slice()) {
        ("deathprotect", []) | ("cycledeath", ["exempt"]) => false,
        ("deathprotect", ["on" | "off"] | ["level", _]) => true,
        _ => return Vec::new(),
    };
    let typed = typing.to_ascii_lowercase();
    let matching = |c: &String| c.to_ascii_lowercase().starts_with(&typed);
    let mut candidates: Vec<String> = state
        .roster
        .iter()
        .map(|p| shown(p).to_owned())
        .filter(matching)
        .collect();
    candidates.sort_by_key(|c| c.to_ascii_lowercase());
    let selectors: &[&str] = if many {
        &["@a", "@p", "@r", "@s"]
    } else {
        &["@p", "@r", "@s"]
    };
    candidates.extend(selectors.iter().map(|s| (*s).to_owned()).filter(matching));
    candidates
        .into_iter()
        .map(|text| Suggestion {
            range,
            text,
            tooltip: None,
            int_value: None,
        })
        .collect()
}
