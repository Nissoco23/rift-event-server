# ViewDistance

Per-player view distance limits for Rift Event Server, as a native Rust mod. Players are sent the
chunks within one distance, operators within another, so an event with many players loads and sends
what the server can carry while its operators still look as far as they like. Nothing needs
installing on the players' side: it is the server's own `SetChunkCacheRadius` and chunk sending.

## What it does

Once a second, and when a player joins, the mod gives each player a limit from their operator level:

- **Players** (below the operator level) get `non-op-max`, 8 chunks by default.
- **Operators** (the `level` in `ops.json`, 1 and up by default) get `op-max`, 32 by default.

A player's distance is the smallest of the server's own `view_distance`, what their client asks for
and the limit, so the server's `view_distance` is the most anyone gets. The limit is also how far the
server keeps that player's chunks **loaded and simulated**: a player at 8 holds 8 chunks loaded and
8 simulating (mobs, crops, redstone), an operator at 32 holds 32 and 32, whatever the server's
`simulation_distance` says (that one is for players with no limit). The engine is told only when a
player's limit changes, so `/op` and `/deop` take effect within a second and nothing is done the
rest of the time. A fake player (a mod's bot) is left alone.

### Seeing further than the server loads

A limit lowers what the server loads, simulates and sends for that player, so a regular player at 8
costs the server 8 chunks round them however far operators go. Set the server's `view_distance` to the
largest limit, `op-max`, and let the limits do the rest. A setup for an event server:

```sh
rift-server config set server.view_distance=32
```

Players load, simulate and see 8 chunks (`non-op-max`), operators 32 (`op-max`), all of it loaded and
ticking, so the cost is that of each operator's 65 by 65 chunk area. With `max_view_distance` above
`view_distance`, each player is also sent chunks the world has already saved, past the loaded area,
without loading or ticking them ([docs/hosting.md](../../docs/hosting.md)); the limit decides who gets
that ring.

## Commands

For operators of level 2 (or whoever a permissions mod grants `viewdistance.admin`); the console too.

| Command | What it does |
| --- | --- |
| `/viewdistance status` | The limits, and for every player what their client asks for, the radius they are sent in full (`near`; the far ring reaches past it for operators) and the limit set. |
| `/viewdistance reload` | Reads `config/viewdistance.toml` again and applies it at once. |

## Configuration

`config/viewdistance.toml` is written with comments the first time. A number outside its range is
brought into it, a key the mod does not know is an error, and a file that does not read leaves the old
settings in place.

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Off, every limit is taken back and every player has the server's own distance. |
| `non-op-max` | `8` | The limit for players below the operator level (2 to 32). |
| `op-max` | `32` | The limit for operators (2 to 32). |
| `op-level` | `1` | The lowest operator level that counts as an operator (1 to 4). |

## Tests

`cargo test --profile fast -p viewdistance` drives the mod's real hooks over a fake server
(`src/testing.rs`): a limit set once and not again, `/op` and `/deop` within a second, a player the
engine could not reach tried again, a reload, switching it off and on, and the command.
