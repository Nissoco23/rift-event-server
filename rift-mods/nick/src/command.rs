//! `/nick`: reading a line, finding the players it names, and answering it.
//!
//! A command checks everything it can and answers at once; what changes a player (the nickname,
//! the name tag, the skin) is queued for the next tick (`state.rs`) and what the services say about
//! a skin is told to whoever ran the command when it comes.

use crate::names;
use crate::state::{Actor, ClearWhy, Holder, Job, OnJoin, SkinState, State, accent, uuid_string};
use horizon_client::mod_api::{Host, NativeCommand, Suggestion};
use rift_chat::{count, escape, mini};

/// Who may run the command: the permission node a permissions mod can grant, or an operator of the
/// level the config names.
pub const ADMIN: &str = "nick.admin";

const USAGES: [&str; 16] = [
    "nick <name:word>",
    "nick <target:player> <name:word>",
    "nick off [targets:players]",
    "nick off admin",
    "nick random [targets:players]",
    "nick random admin",
    "nick info <nickname:word?>",
    "nick list",
    "nick reload",
    "nick pool add <name:word>",
    "nick pool remove <name:word?>",
    "nick pool check [name:word?]",
    "nickonjoin",
    "nickonjoin off",
    "nickonjoin on",
    "nickonjoin all",
];

const USAGE: &str = "Usage: /nick <name> | <player> <name> | off [players|admin] | random [players|admin] | info <nickname> | list | pool add|remove|check [name] | reload";

/// The command's usages, for the players' command trees.
pub fn register(commands: &mut Vec<NativeCommand>, level: u8) {
    for usage in USAGES {
        commands.push(NativeCommand::op(usage, level).permission(ADMIN));
    }
}

/// Who a command acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Targets {
    /// The one who ran it.
    Own,
    /// Every operator online.
    Admin,
    /// A name, a nickname or a selector, as it was typed.
    Spec(String),
}

/// A line of the command, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A nickname for `target` (the one who ran it without one).
    Set {
        target: Option<String>,
        nickname: String,
    },
    Off(Targets),
    Random(Targets),
    Info(String),
    List,
    Reload,
    PoolAdd(String),
    PoolRemove(String),
    PoolCheck(Option<String>),
    /// `/nickonjoin`: the word typed after it, none to ask the setting.
    OnJoin(Option<String>),
    /// Our command, said wrongly.
    Usage,
}

fn targets(word: &str) -> Targets {
    if word.eq_ignore_ascii_case("admin") {
        Targets::Admin
    } else {
        Targets::Spec(word.to_owned())
    }
}

/// Reads `line` (with or without its slash). None when it is not a `nick` command.
pub fn parse(line: &str) -> Option<Command> {
    let mut words = line.trim().trim_start_matches('/').split_whitespace();
    let head = words.next()?;
    let args: Vec<&str> = words.collect();
    if head.eq_ignore_ascii_case("nickonjoin") {
        return Some(Command::OnJoin((!args.is_empty()).then(|| args.join(" "))));
    }
    if !head.eq_ignore_ascii_case("nick") {
        return None;
    }
    let first = args.first().map(|w| w.to_ascii_lowercase());
    Some(match (first.as_deref(), args.as_slice()) {
        (None, _) => Command::Usage,
        (Some("off"), [_]) => Command::Off(Targets::Own),
        (Some("off"), [_, who]) => Command::Off(targets(who)),
        (Some("random"), [_]) => Command::Random(Targets::Own),
        (Some("random"), [_, who]) => Command::Random(targets(who)),
        (Some("info"), [_, name]) => Command::Info((*name).to_owned()),
        (Some("list"), [_]) => Command::List,
        (Some("reload"), [_]) => Command::Reload,
        (Some("pool"), [_, action, rest @ ..]) => {
            match (action.to_ascii_lowercase().as_str(), rest) {
                ("add", [name]) => Command::PoolAdd((*name).to_owned()),
                ("remove", [name]) => Command::PoolRemove((*name).to_owned()),
                ("check", []) => Command::PoolCheck(None),
                ("check", [name]) => Command::PoolCheck(Some((*name).to_owned())),
                _ => Command::Usage,
            }
        }
        // A word of the command with arguments it does not take.
        (Some("off" | "random" | "info" | "list" | "reload" | "pool"), _) => Command::Usage,
        (_, [nickname]) => Command::Set {
            target: None,
            nickname: (*nickname).to_owned(),
        },
        (_, [target, nickname]) => Command::Set {
            target: Some((*target).to_owned()),
            nickname: (*nickname).to_owned(),
        },
        _ => Command::Usage,
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

/// How many words `line` has.
fn words(line: &str) -> usize {
    line.split_whitespace().count()
}

/// Where the targets of `nick off|random <targets>` are in the line: the third word.
fn targets_at(line: &str, targets: &Targets) -> usize {
    match targets {
        Targets::Own => words(line),
        _ => 3,
    }
}

/// Runs `line` as `actor`. Whether it was a `nick` command.
pub fn run(state: &mut State, host: &mut dyn Host, actor: &Actor, line: &str) -> bool {
    let Some(command) = parse(line) else {
        return false;
    };
    // The players as they are now: the command runs where the server's list is in reach.
    state.roster.replace(&host.server_players());
    if !state.config.enabled && command != Command::Reload {
        refuse(
            host,
            line,
            words(line),
            "Nicknames are turned off (enabled = false in config/nick.toml): <accent><click:run_command:'/nick reload'>/nick reload</click></accent> reads it again",
        );
        return true;
    }
    match command {
        Command::Set { target, nickname } => {
            set(state, host, actor, line, target.as_deref(), &nickname);
        }
        Command::Off(who) => off(state, host, actor, line, &who),
        Command::Random(who) => random(state, host, actor, line, &who),
        Command::Info(name) => info(state, host, line, &name),
        Command::List => list(state, host),
        Command::Reload => reload(state, host),
        Command::PoolAdd(name) => pool_add(state, host, actor, line, &name),
        Command::PoolRemove(name) => pool_remove(state, host, line, &name),
        Command::PoolCheck(name) => pool_check(state, host, actor, line, name.as_deref()),
        Command::OnJoin(mode) => on_join(state, host, actor, line, mode.as_deref()),
        Command::Usage => refuse(host, line, words(line), &escape(USAGE)),
    }
    true
}

// ---- who -----------------------------------------------------------------------------------

/// The players a word names: a name or a nickname (a leading `~` is the name chat shows), `@s`,
/// `@p`, `@r` or `@a`. The two selectors for many players, `@a` and `admin`, skip operators or mean
/// only them; `@s` and a name are the player whoever they are. Options in brackets are not read.
/// The errors are markup. `verb` is the command (`off` or `random`) that `@a` points an operator's
/// way to `admin` with.
fn select(
    state: &mut State,
    actor: &Actor,
    spec: &str,
    many: bool,
    verb: &str,
) -> Result<Vec<u128>, String> {
    if spec.contains('[') {
        return Err(
            "Selector options are not supported here: use @a, @p, @r, @s or a name".to_owned(),
        );
    }
    let ordinary = |state: &State| -> Vec<u128> {
        state
            .roster
            .iter()
            .filter(|m| !m.fake && !state.config.is_op(m.permission))
            .map(|m| m.uuid)
            .collect()
    };
    let no_player = "The console is no player: name one".to_owned();
    match spec.to_ascii_lowercase().as_str() {
        "@s" => match actor {
            Actor::Player { uuid, .. } => Ok(vec![*uuid]),
            Actor::Console | Actor::Join => Err(no_player),
        },
        "@p" => {
            let Actor::Player { uuid: me, .. } = actor else {
                return Err(no_player);
            };
            let here = state
                .roster
                .get(*me)
                .cloned()
                .ok_or("You are not on the server")?;
            ordinary(state)
                .into_iter()
                .filter(|u| u != me)
                .filter_map(|u| state.roster.get(u))
                .filter(|m| m.dimension == here.dimension)
                .min_by(|a, b| {
                    distance(a.position, here.position)
                        .total_cmp(&distance(b.position, here.position))
                })
                .map(|m| vec![m.uuid])
                .ok_or_else(|| "No player was found near you".to_owned())
        }
        "@r" => {
            let candidates = ordinary(state);
            state
                .pick(&candidates)
                .map(|u| vec![u])
                .ok_or_else(|| "No player was found".to_owned())
        }
        "@a" if many => {
            let all = ordinary(state);
            if all.is_empty() {
                Err(format!(
                    "No player was found: operators are skipped, use <accent><click:run_command:'/nick {verb} admin'>/nick {verb} admin</click></accent> to target them"
                ))
            } else {
                Ok(all)
            }
        }
        "@a" => Err("One player only here: name one, or use @s, @p or @r".to_owned()),
        s if s.starts_with('@') => Err("Only @a, @p, @r and @s are supported here".to_owned()),
        _ => {
            let name = spec.strip_prefix('~').unwrap_or(spec);
            by_name(state, name)
                .map(|u| vec![u])
                .ok_or_else(|| "Player was not found".to_owned())
        }
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// The player logged in as `name`, or going by it as a nickname.
fn by_name(state: &State, name: &str) -> Option<u128> {
    state.roster.by_name(name).map(|m| m.uuid).or_else(|| {
        state
            .active
            .iter()
            .find(|(uuid, a)| {
                a.nickname.eq_ignore_ascii_case(name) && state.roster.get(**uuid).is_some()
            })
            .map(|(uuid, _)| *uuid)
    })
}

/// The players an `off` or a `random` acts on; the error is markup.
fn who(
    state: &mut State,
    actor: &Actor,
    targets: &Targets,
    verb: &str,
) -> Result<Vec<u128>, String> {
    match targets {
        Targets::Own => match actor {
            Actor::Player { uuid, .. } => Ok(vec![*uuid]),
            Actor::Console | Actor::Join => {
                Err("The console has no nickname: name a player".to_owned())
            }
        },
        Targets::Admin => {
            let ops: Vec<u128> = state
                .roster
                .iter()
                .filter(|m| !m.fake && state.config.is_op(m.permission))
                .map(|m| m.uuid)
                .collect();
            if ops.is_empty() {
                Err("No operator is online".to_owned())
            } else {
                Ok(ops)
            }
        }
        Targets::Spec(spec) => select(state, actor, spec, true, verb),
    }
}

fn real_name(state: &State, uuid: u128) -> String {
    state
        .roster
        .get(uuid)
        .map_or_else(|| uuid_string(uuid), |m| m.name.clone())
}

fn is_self(actor: &Actor, uuid: u128) -> bool {
    matches!(actor, Actor::Player { uuid: me, .. } if *me == uuid)
}

// ---- the commands --------------------------------------------------------------------------

fn set(
    state: &mut State,
    host: &mut dyn Host,
    actor: &Actor,
    line: &str,
    target: Option<&str>,
    nickname: &str,
) {
    let found = match target {
        None => who(state, actor, &Targets::Own, ""),
        Some(spec) => select(state, actor, spec, false, ""),
    };
    let uuid = match found {
        Ok(found) => found[0],
        Err(e) => {
            let at = if target.is_some() { 2 } else { words(line) };
            return refuse(host, line, at, &e);
        }
    };
    // The nickname is the last word of the line.
    let at = words(line);
    let real = real_name(state, uuid);
    let (min, max) = (state.config.min_length, state.config.max_length);
    if let Err(e) = names::check(nickname, min, max) {
        return refuse(host, line, at, &e.says(nickname));
    }
    // A player's own name is no nickname: it is the same as turning theirs off.
    if real.eq_ignore_ascii_case(nickname) {
        return off_players(state, host, actor, line, at, &[uuid]);
    }
    if host.text_allowed(nickname) == Some(false) {
        return refuse(
            host,
            line,
            at,
            &format!(
                "{} is not allowed by the server's chat filter",
                accent(nickname)
            ),
        );
    }
    if let Some(holder) = state.holder(&nickname.to_ascii_lowercase(), uuid) {
        let why = match holder {
            Holder::Player(name) => format!("{} is a player on the server", accent(&name)),
            Holder::Nickname { real } => format!("{} has it as a nickname", accent(&real)),
            Holder::Pending => "a random nickname is about to use it".to_owned(),
        };
        return refuse(
            host,
            line,
            at,
            &format!("{} is taken: {why}", accent(nickname)),
        );
    }
    state.push(Job::Set {
        uuid,
        nickname: nickname.to_owned(),
        actor: actor.clone(),
    });
    let said = if is_self(actor, uuid) {
        format!("<white>Your nickname is now {}!</white>", accent(nickname))
    } else {
        format!(
            "<white>Nicknamed {} as {}!</white>",
            accent(&real),
            accent(nickname)
        )
    };
    say(host, &said);
}

fn off(state: &mut State, host: &mut dyn Host, actor: &Actor, line: &str, targets: &Targets) {
    let at = targets_at(line, targets);
    match who(state, actor, targets, "off") {
        Ok(uuids) => off_players(state, host, actor, line, at, &uuids),
        Err(e) => refuse(host, line, at, &e),
    }
}

/// Takes the nicknames off `uuids`. One player without a nickname is an error; in a crowd the ones
/// without are left alone, and only a crowd without any is.
fn off_players(
    state: &mut State,
    host: &mut dyn Host,
    actor: &Actor,
    line: &str,
    at: usize,
    uuids: &[u128],
) {
    let nicknamed: Vec<u128> = uuids
        .iter()
        .copied()
        .filter(|uuid| state.active.contains_key(uuid))
        .collect();
    if nicknamed.is_empty() {
        let said = match uuids {
            [uuid] if is_self(actor, *uuid) => "You do not have a nickname set".to_owned(),
            [uuid] => format!(
                "{} does not have a nickname set",
                accent(&real_name(state, *uuid))
            ),
            _ => "None of them has a nickname set".to_owned(),
        };
        return refuse(host, line, at, &said);
    }
    for uuid in &nicknamed {
        state.push(Job::Clear {
            uuid: *uuid,
            actor: actor.clone(),
            why: ClearWhy::Asked,
        });
    }
    say(
        host,
        &format!(
            "<white>Removed nicknames from {}!</white>",
            accent(&count(nicknamed.len(), "player"))
        ),
    );
}

fn random(state: &mut State, host: &mut dyn Host, actor: &Actor, line: &str, targets: &Targets) {
    let at = targets_at(line, targets);
    let uuids = match who(state, actor, targets, "random") {
        Ok(found) => found,
        Err(e) => return refuse(host, line, at, &e),
    };
    // Several at once are answered once, when all have theirs.
    let batch = (uuids.len() > 1).then(|| state.new_batch(actor, uuids.len()));
    for uuid in &uuids {
        state.push(Job::Random {
            uuid: *uuid,
            actor: actor.clone(),
            batch,
        });
    }
    let for_whom = match uuids.as_slice() {
        [uuid] if is_self(actor, *uuid) => "you".to_owned(),
        [uuid] => escape(&real_name(state, *uuid)),
        many => count(many.len(), "player"),
    };
    say(
        host,
        &format!("<gray>Choosing random nickname for {for_whom}...</gray>"),
    );
}

/// `/nickonjoin`: who gets a random nickname as they join, said or changed.
fn on_join(state: &mut State, host: &mut dyn Host, actor: &Actor, line: &str, mode: Option<&str>) {
    let Some(word) = mode else {
        let now = state.on_join;
        return say(
            host,
            &format!(
                "<gray>Random nicknames on join: {} ({})</gray>",
                accent(now.word()),
                now.means()
            ),
        );
    };
    let Some(mode) = OnJoin::parse(word) else {
        return refuse(
            host,
            line,
            words(line),
            "Unknown setting: use off, on or all",
        );
    };
    state.on_join = mode;
    host.log(
        "INFO",
        &format!(
            "Nick: {} set random nicknames on join to {} ({})",
            actor.label(),
            mode.word(),
            mode.means()
        ),
    );
    say(
        host,
        &format!(
            "<white>Random nicknames on join set to {} ({})!</white>",
            accent(mode.word()),
            mode.means()
        ),
    );
}

/// Where a nickname's skin stands.
fn skin_line(active: &crate::state::Active) -> String {
    if active.looking {
        "being looked up".to_owned()
    } else {
        match &active.skin {
            SkinState::From(account) => format!("the skin of {account}"),
            SkinState::Kept => "their own".to_owned(),
        }
    }
}

fn info(state: &mut State, host: &mut dyn Host, line: &str, name: &str) {
    let name = name.strip_prefix('~').unwrap_or(name);
    let found = state.active.iter().find(|(_, a)| {
        a.nickname.eq_ignore_ascii_case(name) || a.real_name.eq_ignore_ascii_case(name)
    });
    match found {
        Some((uuid, a)) => {
            let who = format!(
                "<white>{} is {} <gray>({})</gray></white>",
                accent(&a.nickname),
                accent(&a.real_name),
                uuid_string(*uuid)
            );
            let how = format!(
                "<gray>Nicknamed by {}, skin: {}</gray>",
                escape(&a.by),
                escape(&skin_line(a))
            );
            say(host, &who);
            say(host, &how);
        }
        None => match state.roster.by_name(name) {
            Some(m) => say(
                host,
                &format!("<gray>{} is not nicknamed</gray>", accent(&m.name)),
            ),
            None => refuse(
                host,
                line,
                words(line),
                &format!("No player is nicknamed or called {}", accent(name)),
            ),
        },
    }
}

fn list(state: &mut State, host: &mut dyn Host) {
    if state.active.is_empty() {
        return say(host, "<gray>No one is nicknamed</gray>");
    }
    say(
        host,
        &format!(
            "<white>Nicknames in force: {}</white>",
            accent(&state.active.len().to_string())
        ),
    );
    let mut rows: Vec<(String, String)> = state
        .active
        .iter()
        .map(|(uuid, a)| {
            (
                a.nickname.to_ascii_lowercase(),
                format!(
                    "<click:run_command:'/nick info {nick}'>{shown}</click> <gray>= {real} ({uuid}), by {by}, skin: {skin}</gray>",
                    nick = a.nickname,
                    shown = accent(&a.nickname),
                    real = escape(&a.real_name),
                    uuid = uuid_string(*uuid),
                    by = escape(&a.by),
                    skin = escape(&skin_line(a))
                ),
            )
        })
        .collect();
    rows.sort();
    for (_, row) in rows {
        say(host, &row);
    }
}

fn reload(state: &mut State, host: &mut dyn Host) {
    match state.reload(host) {
        Ok(notes) => {
            say(
                host,
                &format!(
                    "<white>Reloaded the {} config: {} in the pool, {} kept!</white>",
                    accent("nick"),
                    accent(&count(state.pool.len(), "name")),
                    accent(&count(state.skins.pinned_count(), "skin"))
                ),
            );
            for note in notes {
                host.log("WARN", &format!("Nick: {note}"));
                say(host, &format!("<gray>{}</gray>", escape(&note)));
            }
        }
        Err(e) => {
            host.log("WARN", &format!("Nick: could not read the files: {e}"));
            say(
                host,
                &format!(
                    "<red>Could not read the files, keeping the old ones: {}</red>",
                    escape(&e.to_string())
                ),
            );
        }
    }
}

const NOT_AN_ACCOUNT: &str = "is not an account name: 1 to 16 letters, digits or underscores";

fn pool_add(state: &mut State, host: &mut dyn Host, actor: &Actor, line: &str, name: &str) {
    if !names::is_account_name(name) {
        return refuse(
            host,
            line,
            words(line),
            &format!("{} {NOT_AN_ACCOUNT}", accent(name)),
        );
    }
    if let Some(known) = state.pool.find(name) {
        return say(
            host,
            &format!("<gray>{} is in the pool already</gray>", accent(known)),
        );
    }
    state.push(Job::PoolAdd {
        name: name.to_owned(),
        actor: actor.clone(),
    });
    say(
        host,
        &format!(
            "<gray>Checking that {} is an account...</gray>",
            accent(name)
        ),
    );
}

fn pool_remove(state: &mut State, host: &mut dyn Host, line: &str, name: &str) {
    let spelled = state.pool.find(name).map(str::to_owned);
    let dir = state.dir.clone();
    match state.pool.remove(&dir, name) {
        Ok(true) => {
            state.skins.unpin(name);
            say(
                host,
                &format!(
                    "<white>Removed {} from the pool ({} left)!</white>",
                    accent(&spelled.unwrap_or_else(|| name.to_owned())),
                    accent(&count(state.pool.len(), "name"))
                ),
            );
        }
        Ok(false) => refuse(
            host,
            line,
            words(line),
            &format!("{} is not in the pool", accent(name)),
        ),
        Err(e) => say(
            host,
            &format!("<red>Could not change the pool: {}</red>", escape(&e)),
        ),
    }
}

fn pool_check(
    state: &mut State,
    host: &mut dyn Host,
    actor: &Actor,
    line: &str,
    name: Option<&str>,
) {
    let Some(name) = name else {
        let pinned = state
            .pool
            .names()
            .iter()
            .filter(|n| state.skins.pinned(n).is_some())
            .count();
        let total = state.pool.len();
        let mut said = format!(
            "<white>The pool has {}: {} with their skin kept, {} not looked up yet</white>",
            accent(&count(total, "name")),
            accent(&pinned.to_string()),
            accent(&(total - pinned).to_string())
        );
        if state.pool.bad() > 0 {
            said.push_str(&format!(
                "\n<gray>{} found to be no account this session</gray>",
                accent(&count(state.pool.bad(), "name"))
            ));
        }
        for part in said.split('\n') {
            say(host, part);
        }
        return;
    };
    if !names::is_account_name(name) {
        return refuse(
            host,
            line,
            words(line),
            &format!("{} {NOT_AN_ACCOUNT}", accent(name)),
        );
    }
    if let Some(skin) = state.skins.pinned(name) {
        let said = format!(
            "<gray>{} is in the pool; its skin is kept (the account {})</gray>",
            accent(state.pool.find(name).unwrap_or(name)),
            accent(&skin.account)
        );
        return say(host, &said);
    }
    state.push(Job::PoolCheck {
        name: name.to_owned(),
        actor: actor.clone(),
    });
    say(host, &format!("<gray>Checking {}...</gray>", accent(name)));
}

/// What `tab_complete_v2` offers for the word being typed after `/nick`, given the words before it:
/// the nicknames in force for `info` and the pool's names for `pool remove` and `pool check`. The
/// players' own names come from the client, which completes them from the tab list: the nicknames
/// of those who have one.
pub fn completions(
    state: &State,
    before: &[&str],
    typing: &str,
    range: (usize, usize),
) -> Vec<Suggestion> {
    let words: Vec<String> = before.iter().map(|w| w.to_ascii_lowercase()).collect();
    let candidates: Vec<String> = match words
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["info"] => state.active.values().map(|a| a.nickname.clone()).collect(),
        ["pool", "remove" | "check"] => state.pool.names().to_vec(),
        _ => return Vec::new(),
    };
    let typed = typing.to_ascii_lowercase();
    let mut out: Vec<String> = candidates
        .into_iter()
        .filter(|c| c.to_ascii_lowercase().starts_with(&typed))
        .collect();
    out.sort_by_key(|c| c.to_ascii_lowercase());
    out.into_iter()
        .map(|text| Suggestion {
            range,
            text,
            tooltip: None,
            int_value: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_is_not_ours_is_not_read() {
        assert_eq!(parse("gamemode creative"), None);
        assert_eq!(parse("nickname"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
    }

    #[test]
    fn a_nickname_is_for_the_one_who_typed_it_or_for_the_player_named() {
        assert_eq!(
            parse("nick Captain"),
            Some(Command::Set {
                target: None,
                nickname: "Captain".into()
            })
        );
        assert_eq!(
            parse("/NICK  Bob   Captain "),
            Some(Command::Set {
                target: Some("Bob".into()),
                nickname: "Captain".into()
            })
        );
        // A word of the command is a word of the command, in any case, in either place.
        assert_eq!(parse("nick OFF"), Some(Command::Off(Targets::Own)));
        assert_eq!(
            parse("nick Bob off"),
            Some(Command::Set {
                target: Some("Bob".into()),
                nickname: "off".into()
            })
        );
    }

    #[test]
    fn off_and_random_take_nobody_a_name_a_selector_or_admin() {
        assert_eq!(parse("nick off"), Some(Command::Off(Targets::Own)));
        assert_eq!(
            parse("nick off Bob"),
            Some(Command::Off(Targets::Spec("Bob".into())))
        );
        assert_eq!(
            parse("nick off @a"),
            Some(Command::Off(Targets::Spec("@a".into())))
        );
        assert_eq!(parse("nick off ADMIN"), Some(Command::Off(Targets::Admin)));
        assert_eq!(parse("nick random"), Some(Command::Random(Targets::Own)));
        assert_eq!(
            parse("nick random @r"),
            Some(Command::Random(Targets::Spec("@r".into())))
        );
        assert_eq!(
            parse("nick random admin"),
            Some(Command::Random(Targets::Admin))
        );
    }

    #[test]
    fn info_list_reload_and_the_pool_are_read() {
        assert_eq!(
            parse("nick info Captain"),
            Some(Command::Info("Captain".into()))
        );
        assert_eq!(parse("nick list"), Some(Command::List));
        assert_eq!(parse("nick reload"), Some(Command::Reload));
        assert_eq!(
            parse("nick pool add Alex"),
            Some(Command::PoolAdd("Alex".into()))
        );
        assert_eq!(
            parse("nick pool REMOVE Alex"),
            Some(Command::PoolRemove("Alex".into()))
        );
        assert_eq!(parse("nick pool check"), Some(Command::PoolCheck(None)));
        assert_eq!(
            parse("nick pool check Alex"),
            Some(Command::PoolCheck(Some("Alex".into())))
        );
    }

    #[test]
    fn nickonjoin_is_read_with_the_word_typed_after_it() {
        assert_eq!(parse("nickonjoin"), Some(Command::OnJoin(None)));
        assert_eq!(
            parse("/NICKONJOIN  all"),
            Some(Command::OnJoin(Some("all".into())))
        );
        // A word is the setting's to judge, so a wrong one is told what is possible.
        assert_eq!(
            parse("nickonjoin on now"),
            Some(Command::OnJoin(Some("on now".into())))
        );
    }

    #[test]
    fn a_line_said_wrongly_gets_the_usage() {
        for line in [
            "nick",
            "nick off a b",
            "nick info",
            "nick info a b",
            "nick list now",
            "nick reload now",
            "nick pool",
            "nick pool add",
            "nick pool add a b",
            "nick pool dance",
            "nick pool check a b",
            "nick a b c",
        ] {
            assert_eq!(parse(line), Some(Command::Usage), "{line}");
        }
    }

    #[test]
    fn the_command_is_registered_for_operators_of_the_level_and_the_admin_permission() {
        let mut commands = Vec::new();
        register(&mut commands, 3);
        assert_eq!(commands.len(), 16);
        for command in &commands {
            assert_eq!(command.level, 3);
            assert_eq!(command.permission.as_deref(), Some("nick.admin"));
            assert!(!command.replace_vanilla);
            assert!(command.usage.starts_with("nick"));
        }
        let usages: Vec<&str> = commands.iter().map(|c| c.usage.as_str()).collect();
        // Players are picked by the client's own selector argument.
        assert!(usages.contains(&"nick <target:player> <name:word>"));
        assert!(usages.contains(&"nick off [targets:players]"));
        assert!(usages.contains(&"nick random [targets:players]"));
        assert!(usages.contains(&"nick off admin") && usages.contains(&"nick random admin"));
    }
}
