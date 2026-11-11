# Essentials

Short operator commands for Rift Event Server, as a native Rust mod: four shortcuts for the game mode
and one for the flying speed. Nothing needs installing on the players' side.

## Using it

All of it is for operators of level 2 (the console too), the level vanilla's `/gamemode` needs, or
whoever a permissions mod grants `essentials.gamemode` (the four shortcuts) and `essentials.flyspeed`.
A player below that level gets vanilla's "unknown command".

| Command | What it does |
| --- | --- |
| `/gmc [player]` | Creative mode for the player, or for you. |
| `/gms [player]` | Survival mode. |
| `/gmsp [player]` | Spectator mode. |
| `/gma [player]` | Adventure mode. |
| `/flyspeed <1-10>` | Your creative flying speed: 1 is vanilla's, 10 the most (0.45, which a player can sprint-fly without the server's moved-too-quickly check sending them back). Above 10 is 10, a decimal goes to the nearest whole speed, and below 1 is an error. |

- `player` is the name the player logged in with, or their nickname, and has to be on the server:
  "Player was not found" otherwise. Without one the command is for whoever ran it, so the console has to
  name a player.
- A player already in the mode is told so and nothing changes. A player whose mode another operator
  changed is told with vanilla's own line ("Your game mode has been updated to ...").
- A flying speed is kept by the engine: a respawn or a game mode change keeps it, and it is saved with
  the player. It only matters while the player can fly (creative and spectator).
- A command answers at once and the tick makes the change, because the engine reaches another player
  only outside their own turn. A change that fails then (the player left, or a mod kept the game mode)
  is told to whoever ran the command.

Every change is logged: `Essentials: <operator> set <player>'s game mode to <mode>`.

## Tests

`cargo test --profile fast -p essentials` drives the mod's real hooks over a fake server
(`src/testing.rs`): every command and message, the rounding of the flying speed, finding a player by
name and nickname, the console, a change that fails at the tick, and the registration for operators.
