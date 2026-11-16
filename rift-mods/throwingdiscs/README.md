# ThrowingDiscs

The throwing disc Mojang showed at TwitchCon San Diego on November 14, 2026, for Rift Event Server, as a
native Rust mod: a port of the Paper plugin of the same name. Right-click throws a disc along the view; it
pierces every mob and player in its path, ricochets off hard blocks and returns to the hand it came from.
Nothing needs installing on the players' side except the resource pack.

## Using it

1. Players need the plugin's resource pack (`ThrowingDiscs-ResourcePack.zip`): without it discs look like
   echo shards. The mod does not send it; players install it themselves.
2. Operators give discs with `/discs give`. With `crafting = true` (the default) players can also craft them.

| Feature | Behavior |
| --- | --- |
| Tiers | Copper, iron, golden, diamond and netherite. There is no wooden or stone disc. |
| Crafting | Four copper ingots, iron ingots, gold ingots or diamonds in a plus shape with the centre empty. |
| Netherite | Smithing table with a netherite upgrade template, a diamond disc and a netherite ingot. The disc keeps its enchantments, name and wear. Any other echo shard gives nothing. |
| Throwing | Right-click to throw. The disc flies straight where you look, with no gravity, and spins. No cooldown or charge time. Sneak to throw at a chest or a door, which a plain click would open. |
| Piercing | The disc passes through every mob and player in its path. |
| Return | When its range runs out, the disc curves back to you and hits everything again on the way. |
| Ricochet | The disc bounces off hard blocks. Each bounce makes it faster and adds range. Soft blocks such as wool, leaves, hay and slime send it straight back. |
| Off hand | A disc thrown from the off hand returns to the off hand. A disc thrown from the main hand returns to the same hotbar slot. |
| Dual wielding | With a disc in each hand, the first right-click throws the main-hand disc and the second the off-hand one. |
| Decorated pots | The disc breaks pots in its path and keeps flying; they drop their sherds. |
| Durability | Each throw uses 1 durability, and Unbreaking applies. Like a trident, a disc with 1 durability left cannot be thrown. Creative players wear nothing. Netherite discs survive fire and lava. |
| Enchanting | Sharpness I-V, Knockback I-II, Unbreaking I-III and Mending, from books in an anvil, a second disc of the same tier, or `/enchant`. Enchanting tables do not work on discs. |
| Repair | In an anvil, with the tier's material or with a second disc of the same tier. |
| Safety | The disc goes back to the inventory if you log out, change dimension, the chunk it is in unloads or the server stops. If you die it drops with your items, or comes back on respawn under `keep_inventory`. |

### Damage

A hit deals the tier's damage plus the Sharpness bonus: +1 at level I and +0.5 per level after that, so +3
at level V, on every hit out and back. With the defaults:

| Tier | Damage | Durability | Range | Speed |
| --- | --- | --- | --- | --- |
| Copper | 6 | 190 | 10 | 1.0 |
| Iron | 7 | 250 | 11 | 1.05 |
| Golden | 6 | 32 | 12 | 1.2 |
| Diamond | 8 | 1561 | 12 | 1.1 |
| Netherite | 9 | 2031 | 13 | 1.15 |

Mojang has published one number: the iron disc deals 7 damage. The others are estimates; all of them are in
the config. A plain disc pushes a mob about 1 block, Knockback II about 5.

## Commands

For operators of level 2, or whoever a permissions mod grants `throwingdiscs.admin`; the console too.

| Command | What it does |
| --- | --- |
| `/discs give <player> <copper\|iron\|golden\|diamond\|netherite\|all> [amount]` | Gives discs; what does not fit drops at the player's feet. |
| `/discs reload` | Reads `config/throwingdiscs.toml` again. Discs that exist keep their old durability and lore, and recipes change on `/reload`. |
| `/enchant <player> <enchantment> [level]` | Vanilla's command, also on a disc held in the main hand, for the four enchantments above; for operators of level 2. It takes a player's name, `@s`, `@p` or `@a`; with a target that holds no disc, or any other selector, vanilla runs it. The mod reads who holds a disc as of the last tick. Running it again sets the new level. |

## Configuration

`config/throwingdiscs.toml` is written with comments the first time. A number outside what the flight can
use is brought into range, and a file that does not read leaves the old settings in place.

| Key | Default | Meaning |
| --- | --- | --- |
| `crafting` | `true` | Whether the crafting and smithing recipes exist (read when the data loads: `/reload`). |
| `pvp` | `true` | Whether discs can hurt players. |
| `durability-per-throw` | `1` | Durability a throw uses. |
| `break-decorated-pots` | `true` | Whether pots in the way are broken. |
| `soft-blocks` | leaves, wool, hay, slime... | Block ids and `#tags` the disc turns back at instead of bouncing. |
| `[tiers.<tier>]` | see above | `damage`, `durability`, `enchantability`, `range`, `speed` of each tier. |
| `[flight]` | | Knockback, the return's speed and acceleration, the catch distance, the lifetime and the spin. |
| `[ricochet]` | | Bounces, the speed each gives and its limit, the range each adds. |

## How it differs from the plugin

- The disc is drawn with an `item_display` that is moved and turned once a tick, as in the plugin. A crash can
  leave one behind: the mod removes the strays it finds at the first tick of each level.
- A mob a disc has just hit has vanilla's hurt cooldown, so it cannot be hit again within half a second; the
  plugin skipped the cooldown. A disc that hits a mob on the way out and again within 10 ticks on the way back
  hurts it once.
- Breaking a pot asks no protection plugin, as the engine has none; the disc has no block-break event.
- A crafter can still use a disc as an echo shard for a recovery compass (only a crafting table and the
  inventory grid are stopped); the engine's crafter hook cannot cancel.
- `/enchant` is taken over only when every target holds a disc; the plugin also handled mixed targets itself.
- The thrower is assumed right-handed, so a main-hand disc leaves from the right side of the view.
- The resource pack is not sent by the mod.

## Tests

`cargo test --profile fast -p throwingdiscs` drives the mod's real hooks over a fake server (`src/testing.rs`):
the throw, the flight and its ricochets, hits and damage by tier and Sharpness, the return and the catch,
durability and Unbreaking, safety on logout, death and stop, the commands, `/enchant`, recipes, smithing, the
recovery compass and the anvil.
