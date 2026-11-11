# Essentials

Short operator commands for Rift Event Server, as a native Rust mod: four shortcuts for the game mode,
vanilla's `/gamemode` mapped onto them, and one command for the flying speed. Nothing needs installing on the players' side.

## Using it

All of it is for operators of level 2 (the console too), the level vanilla's `/gamemode` needs, or
whoever a permissions mod grants `essentials.gamemode` (the four shortcuts) and `essentials.flyspeed`.
A player below that level gets vanilla's "unknown command".

| Command | What it does |
| --- | --- |
| `/gmc [targets]` | Creative mode for the player, or for you. |
| `/gms [targets]` | Survival mode. |
| `/gmsp [targets]` | Spectator mode. |
| `/gma [targets]` | Adventure mode. |
| `/gamemode <mode> [targets]` | Vanilla's command, answered by the mod as the shortcut for the mode: `/gamemode creative` is `/gmc`, `survival` is `/gms`, `spectator` `/gmsp`, `adventure` `/gma`. |
| `/flyspeed <1-10>` | Your creative flying speed: 1 is vanilla's, 10 the most (0.45, which a player can sprint-fly without the server's moved-too-quickly check sending them back). Above 10 is 10, a decimal goes to the nearest whole speed, and below 1 is an error. |

- `targets` is a player's name (the one they logged in with, or their nickname) or a selector: `@s` is
  you, `@a` everyone online, `@r` one player at random and `@p` the nearest to you in your level (you,
  when no one is nearer, as in vanilla). `--non-ops` is every player who is no operator (level 0). A name that is not on the server is "Player was not found". Options
  in brackets (`@a[distance=..5]`) are refused. Without targets the command is for whoever ran it, so the
  console has to name a player or use `@a` or `@r`.
- One player changed is said by name; more than one as "Set 4 players gamemode to creative!". Players
  already in the mode are left out of the count.
- A player already in the mode is told so and nothing changes. A player whose mode another operator
  changed is told with vanilla's own line ("Your game mode has been updated to ...").
- A flying speed is kept by the engine: a respawn or a game mode change keeps it, and it is saved with
  the player. It only matters while the player can fly (creative and spectator).
- A command answers at once and the tick makes the change, because the engine reaches another player
  only outside their own turn. A change that fails then (the player left, or a mod kept the game mode)
  is told to whoever ran the command.

Every change is logged: `Essentials: <operator> set <player>'s game mode to <mode>`.

## Vanilla's `/gamemode`

The mod's `/gamemode` replaces vanilla's in the players' command trees, and every line of that name
goes to the mod, so a line it cannot read (an unknown mode, a selector with options such as
`@a[distance=..5]`) is refused with the usage instead of reaching vanilla's. Commands that do not come
from a player or the console, such as command blocks, functions and `/execute ... run gamemode`, still
run vanilla's.

## Tests

`cargo test --profile fast -p essentials` drives the mod's real hooks over a fake server
(`src/testing.rs`): every command and message, the rounding of the flying speed, finding a player by
name and nickname, the console, a change that fails at the tick, and the registration for operators.
