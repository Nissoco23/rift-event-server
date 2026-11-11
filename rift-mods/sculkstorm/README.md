# SculkStorm

The Sculkstorm Scythe for Rift Event Server, as a native Rust mod. It is a port of the Sculkstorm Paper
plugin by the same author: a netherite hoe made of vanilla components, so players use it with stock
Minecraft 26.3 clients and nothing to install.

- **Right-click: Sculk Cataclysm.** A sonic boom flies where you look. Where it lands everything
  close dies, everything farther is hurt and thrown, lightning plays over the spot, and a storm
  crater of sculk, deepslate and catalysts opens in the ground. After a while the land is put back.
- **Sneak + right-click: Soulstorm Ascension.** Two seconds of gathering soul fire, then a shockwave
  that hurts and pushes back what stands near you, and a launch into the sky. The landing does not
  hurt you.

## Building and running

A native mod is Rust code, so it is compiled into the engine rather than loaded by it. Nothing here
is built apart: `scripts/build.sh`, run from the root of the repository, builds `dist/rift-engine`
with this mod linked in (`crates/rift-engine` hands every mod in `rift-mods/` to the engine,
[docs/rift-mods.md](../../docs/rift-mods.md)), next to `dist/rift-server`. Run the server as usual
([docs/hosting.md](../../docs/hosting.md)). The first start writes `config/sculkstorm.toml` in the
server folder.

## Using it

An operator gives themselves or a player a scythe:

```text
/sculkstorm give <player>
```

Hold it in the main hand. Right-click for Sculk Cataclysm, sneak and right-click for Soulstorm
Ascension. Each ability has its own cooldown, shown on the action bar when it is tried too early.

### Commands and permissions

| Command | What it does |
| --- | --- |
| `/sculkstorm give <player>` | Gives the player a scythe. |
| `/sculkstorm blast <x> <y> <z> [world]` | Sets off a Cataclysm there with no caster, to try it out. |
| `/sculkstorm undo` | Puts every crater back now. |
| `/sculkstorm reload` | Reads `config/sculkstorm.toml` again. |

The commands are for operators (permission level 2), or whoever a permissions mod grants
`sculkstorm.admin`. Using the scythe needs `sculkstorm.use`, which everyone has unless a permissions
mod says otherwise. The commands are also run from the server console.

## Configuration

`config/sculkstorm.toml` is written with comments the first time and read again by
`/sculkstorm reload`. A value outside its range is brought into it, so a typo cannot swallow the
server. The defaults:

Top level

| Key | Default | Meaning |
| --- | --- | --- |
| `disabled-worlds` | `[]` | Worlds where the scythe does not work, by world name or dimension (`"minecraft:the_end"`). |
| `pvp` | `true` | Whether the abilities can hit players. |
| `debug` | `false` | Logs the block counts and timings of each crater. |
| `particle-multiplier` | `1.0` | `0.5` is half the particles (0 to 16). |

`[ascension]`

| Key | Default | Meaning |
| --- | --- | --- |
| `cooldown-seconds` | `15` | 0 to 86400. |
| `charge-ticks` | `40` | The wind-up, during which the caster is slowed (10 to 1200). |
| `shockwave-radius` | `8.0` | 0.5 to 64. |
| `damage` | `8.0` | Damage to what the shockwave hits. |
| `knockback` | `1.6` | 0 to 4. |
| `launch-power` | `2.2` | The upward velocity the caster is launched with (0 to 3.9). |
| `fall-protection-seconds` | `12` | How long the first landing that would hurt is forgiven (0 to 3600). |

`[cataclysm]`

| Key | Default | Meaning |
| --- | --- | --- |
| `cooldown-seconds` | `30` | 0 to 86400. |
| `projectile-speed` | `1.6` | Blocks a tick (0.3 to 8). |
| `max-range` | `80.0` | How far the boom flies before it goes off (1 to 1024). |
| `impact-radius` | `33.0` | The radius entities are hurt in (1 to 128). |
| `damage` | `14.0` | The damage between `lethal-radius` and `impact-radius`, falling to half at the edge. |
| `lethal-radius` | `13.0` | Everything inside dies, armour or not (a totem still works). 0 turns it off. |
| `darkness-seconds` | `8` | 0 to 600. |
| `lightning-strikes` | `45` | Only the effect: no fire and no damage (0 to 500). |
| `lightning-duration-ticks` | `100` | 1 to 1200. |
| `max-active` | `3` | How many cataclysms can go at once on the whole server (1 to 32). |

`[terrain]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Whether a cataclysm digs a crater at all. |
| `core-radius` | `33` | The dug bowl, about 66 blocks across (3 to 120). |
| `crater-depth` | `10` | 0 to 64. At 0 the bowl is flat. |
| `tendrils` | `24` | The number of sculk roots (0 to 96). |
| `tendril-length` | `50` | 0 to 160. |
| `break-all-blocks` | `true` | `true`: builds break too (never bedrock, portals, spawners, or anything with data such as chests and signs). `false`: only natural blocks such as dirt, stone and sand. |
| `columns-per-tick` | `80` | The crater is built over a few seconds so it does not spike a tick (1 to 2048). |
| `max-ms-per-tick` | `4.0` | A time budget as well; lower is smoother and slower (0.5 to 50). |
| `max-debris` | `80` | Flying blocks for each impact (0 to 400). |
| `restore` | `true` | Put the land back afterwards. Crater blocks drop nothing while they wait. |
| `restore-delay-seconds` | `90` | 1 to 86400. |
| `restore-blocks-per-tick` | `400` | 1 to 20000. |

`[resource-pack]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `false` | Send the pack when a player joins. |
| `url` | `""` | Where the zip can be downloaded. |
| `sha1` | `""` | The zip's SHA-1, from `scripts/pack.sh`. |
| `required` | `false` | Whether a player who declines is turned away. |

## Resource pack

The scythe's model and texture are in `resourcepack/`. The item works without them; players only need
the pack to see the model.

```sh
rift-mods/sculkstorm/scripts/pack.sh
```

run from the root of the repository, writes `dist/sculkstorm-resourcepack.zip` and prints its SHA-1.
The zip is the same bytes for the same files, so the SHA-1 changes only when the pack does. Host the
zip where players can download it (HTTPS), then either have the mod send it, in
`config/sculkstorm.toml`:

```toml
[resource-pack]
enabled = true
url = "https://example.com/sculkstorm-resourcepack.zip"
sha1 = "the SHA-1 scripts/pack.sh printed"
required = false
```

or use the server's own setting, in `server.toml`:

```toml
[properties]
resource-pack = "https://example.com/sculkstorm-resourcepack.zip"
resource-pack-sha1 = "the SHA-1 scripts/pack.sh printed"
```

The mod sends its pack under a fixed id, so a client replaces it instead of stacking a second copy.

## What stands and what comes back

- The crater is dug a little each tick, inside the time and column budget, and every change is written
  to a journal under `<world>/sculkstorm/pending-restores/` as it is made. A server that stops
  cleanly puts every crater back first; one that crashes puts them back the next time the world
  opens.
- Only blocks that are still the crater's, or have gone (mined, flooded, burnt), are put back. A block
  a player built over a crater block is left alone.
- Crater blocks a player breaks drop nothing and give no experience, or the catalysts would be farmed
  before the land returns.
- Sculk that spreads from the crater's catalysts inside its area is put back with the crater.
- Craters may overlap. Each records the land under whatever another crater has put there, so they come
  back in either order.
- The writes make no block updates, as the plugin's physics-off writes made none: a gravel block over
  a dug-out block stays until something updates it.

## How it differs from the Paper plugin

- The configuration is TOML (`config/sculkstorm.toml`) rather than YAML. The keys are the plugin's.
  Values under the plugin's floors are raised as the plugin raises them, and a few are also held
  under a ceiling.
- A native mod cannot stop or remove real falling blocks, so the flying debris are `block_display`
  entities that the mod moves itself (tagged `sculkstorm_debris`, and swept when a world opens).
- A mod cannot cancel sculk spreading, so what spreads inside a crater's area is recorded and put
  back with it.
- The charge's Slowness shows its icon.
- `crater-depth = 0` gives a flat crater; the plugin still dug a block here and there.
- The crater is planned on the server thread (it is cheap) and dug under the budget; there is no
  asynchronous planning.
- A restore asks the server for the chunks it needs when they are not loaded, and lets them go
  afterwards.
- Scythes made by the plugin work with the mod, and the other way round: both mark the item with the
  same `sculkstorm:weapon` value.

## Development

```sh
cargo test --profile fast -p sculkstorm
cargo clippy --profile fast -p sculkstorm -- -D warnings
cargo fmt --all --check
```

The tests drive the engine's own hook dispatch over a fake world, so they run the mod's real hooks:
digging and putting back a crater block for block, undo, crash recovery from the journals, overlapping
craters, the protections, commands and configuration.

Layout:

- `src/lib.rs`: the mod, `SculkStorm`, the engine's `Hooks`, and `hooks()` and `ID` for the launcher.
  `ascension.rs`, `cataclysm.rs` and `crater/` hold the abilities and the crater (`plan.rs`: the
  layout; `job.rs`: digging and restoring; `journal.rs`: the crash journal).
- `resourcepack/`: the scythe's model and texture. `scripts/pack.sh`: zips it.

## Licence and credits

MIT, like the rest of the repository ([LICENSE](../../LICENSE)). By Nissoco. It is the Sculkstorm
plugin for Paper ported to Rift Event Server's native mod API.
