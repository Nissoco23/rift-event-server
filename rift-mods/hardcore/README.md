# Hardcore

The hardcore rules of Rift Event Server, as a native Rust mod: death protection that holds an operator's health
above a floor, what happens to a player who dies (kicked, put in spectator mode, or nothing) and what the
others are told about it. Nothing needs installing on the players' side. It is made of this crate and four
things the engine gives mods for it: `native_living_damage_pre_v2`, `native_player_death_message`,
`native_set_membership_lines` and the hub's `names_within` ([docs/native-mods.md](../../docs/native-mods.md)).

## Using it

All of it is for operators of level 2 (the console too), or whoever a permissions mod grants `hardcore.admin`.
A player below that level gets vanilla's "unknown command".

| Command | What it does |
| --- | --- |
| `/deathprotect [player]` | Switches death protection for one player, or for you. |
| `/deathprotect on\|off [targets]` | Sets it for every player named; `@a`, `@p`, `@r` and `@s` work. |
| `/deathprotect level <1-20> [targets]` | The lowest health a protected player can be left at, in health points (half a heart each). 1 is the default. |
| `/ondeath [spectate\|kick\|none]` | What happens to a player who dies. `kick` by default. With no word, says what it is. |
| `/ondeath admin [spectate\|kick\|none]` | The same for operators (level 2 and up). `spectate` by default. |
| `/deathmsg [none\|death\|leave]` | What the others are told of a death. `leave` by default. |
| `/deathmsgradius [blocks]` | How far a death message travels. 150 by default. `0` is the player who died alone, `-1` everyone. |

### Death protection

- It is off for everyone until it is turned on, and it holds a player's health at or above their level: a hit
  that would take them lower takes only what leaves them at it, so one that does more damage than they have
  health leaves them at their level. At the level already (or below it, when it was set higher than their
  health), a hit takes nothing; nobody is healed up to it.
- It does not engage while either hand holds a death-protection item (a totem of undying): the hit that would
  kill them pops it as in vanilla. A hit that would not kill them takes the health it takes, and no totem.
- `/kill` and the void still kill: otherwise a protected player would fall for ever.
- Armour, enchantments, effects and absorption take their part first; the floor is on what reaches the health.
- `/deathprotect` with no word switches it for one player: the one who typed it, or the one named. On and off
  say what they set, so "on" on a player who has it stays on and says so ("Bob death protection is already
  enabled"); for several players the line counts those it changed and, in gray, those that already had it. Level does not turn protection on; it says so when
  protection is off. Targets that are players are named as others know them: by their nickname when they
  have one. Either name finds them, the nickname first when one player's is another's name.
- The chat box completes targets with nicknames (the name when there is none) and the selectors the command takes
  there, `@a` only where several players are allowed.
- Nobody is told that an operator changed their protection.

Answers:

```
Your death protection has been enabled
Minimum health: 0.5 hearts
Bob death protection has been disabled
Bob death protection is already disabled
Death protection has been disabled for 29 players
```

The minimum health line is gray.

A health point is half a heart, so level 1 is `0.5 hearts`, 2 is `1 heart` and 3 is `1.5 hearts`: only exactly one
heart is singular.

### What happens on death

- `kick` disconnects the player with their death message once the death is done (the drops, the death message).
  It is a kick, not a ban: they can reconnect and respawn. Behind a Velocity proxy they go to the fallback server
  if there is one.
- `spectate` respawns them as a spectator where they died, once they press respawn.
- `none` is vanilla's death screen and respawn.
- Both settings are for this run of the server: a restart sets them back to `kick` and `spectate`.
- A mod's bots (fake players) are left alone.

### What the others are told

`/deathmsg` says what a death shows, to the players within `/deathmsgradius` blocks in the same dimension (the
dying player too; the server log always has the death message):

- `leave` (the default): "Bob left the game", in vanilla's yellow and with the player's nickname, in place of the
  death message. While it is set, the real "joined the game" and "left the game" lines are not sent, so a player
  who dies and is kicked is not announced twice and nobody learns who joins or leaves otherwise.
- `death`: the death message, as vanilla shows it, and the real lines as vanilla sends them.
- `none`: nothing, and the real lines as vanilla sends them.

Nothing is told when the `showDeathMessages` game rule is off, whatever `/deathmsg` says. The dying player's own
death screen keeps the death message.

## What is saved

`config/hardcore.toml`, written by the mod each time a command changes it: the `deathmsg` and
`deathmsgradius` settings, and under `[protection]` each player's switch and level by UUID (a player who is back
to the default is not kept). `/ondeath` is not saved. A file that cannot be read is moved to
`hardcore.toml.broken` and the log says so; the defaults hold, and the next change writes a fresh file.

## Tests

`cargo test --profile fast -p hardcore` drives the mod's real hooks over a fake server (`src/testing.rs`): every
command and message, who a name or a selector finds, the floor on a hit with and without a totem, what a death
does to players and to operators, what is told and how far, the join and leave lines, the saved file across a
restart, a file that cannot be read, and the completions. The engine's half is tested in
`lan/hub_tests.rs` (the join and leave lines, who is within a distance) and `health.rs` (what a player's hit
hook is told).
