# Nick

Operator nicknames for Rift Event Server, as a native Rust mod. An operator gives a player (or
themselves) another name and another skin, so a scripted event can hide who is who: the name shows in
chat, death, advancement and leave lines, the tab list and over the player's head, and the skin is
that of a real account. Nothing needs installing on the players' side. A nickname is never saved: it
ends when the player leaves.

## Using it

All of it is for operators of level 2 (the console too), or whoever a permissions mod grants
`nick.admin`.

| Command | What it does |
| --- | --- |
| `/nick <name>` | Nicknames you. |
| `/nick <player> <name>` | Nicknames that player. Says so if they are not on the server. |
| `/nick off [players]` | Takes the nickname off you, or off the players named. Says so if there is none. |
| `/nick random [players]` | Gives you, or the players named, a random nickname from the pool. |
| `/nick off admin`, `/nick random admin` | The same for every operator online. |
| `/nick info <nickname>` | Who the nickname really is: real name, UUID, who set it, whose skin. A real name works too. |
| `/nick list` | Every nickname in force, with the real name and UUID. |
| `/nick pool add <name>` | Puts an account in the pool, after checking it exists and keeping its skin. |
| `/nick pool remove <name>` | Takes a name out of the pool. |
| `/nick pool check [name]` | Alone: how many names the pool has and how many have their skin kept. With a name: whether it is in the pool and its skin. |
| `/nick reload` | Reads `nick.toml`, `nick-pool.txt` and `nick-skins.json` again. |

`players` is a name (the real one, or the nickname), `@s`, `@p`, `@r` or `@a`. `@a`, `@p` and `@r`
skip operators; `admin` is the operators; `@s` and a name reach anyone, operators included. A selector
with options in brackets is refused. The console has no `@s` or `@p`.

### Nicknames

- ASCII letters, digits and underscores, 1 to 16 characters (Mojang's own names have 3 or more; 1 and
  2 are allowed here, `min-length` changes it). The words `off random info pool list reload` are the
  command's and cannot be nicknames.
- It has to pass the server's chat filter, and cannot be the real name of a player online or a
  nickname in force. A name equal to the player's own is the same as `off`.
- If the name is an account (and so is written as that account writes it), the player gets its skin,
  looked up on Mojang's services in the background, so the nickname itself is never held up. If there
  is no such account, or the services do not answer, the player keeps their skin and the operator is
  told.
- `off` gives back the name and the skin the player came with.
- When a player joins with the name of a nickname in force, that nickname is taken off and the
  operators are told.

Every change is logged: `Nick: <operator> set <real name> (<uuid>) to <nickname>`.

### What a nickname hides, and what it does not

Chat, `/me`, `/msg`, the leave and join lines, death and advancement lines, `/list` and the tab list
show the nickname, and hovering it shows nothing of the player. The real name stays in the server's
log, commands, selectors and events, and the engine still tells a modded client or a packet logger who
is who (entity UUIDs travel in packets). The Discord bridge, the web panel's player list and the
server-list sample use real names. A skin or name tag change makes other clients re-add the player for
an instant (a blink), and the player's own client respawns in place.

## The pool

`/nick random` draws from `config/nick-pool.txt`, one account name a line, written the first time from
the 285 names the mod ships with: ordinary accounts that were looked up, their skins viewed and
checked, and compared against the chat filter's word list. A name is not drawn twice until every other
has been, names in use are skipped, and a name the services say is no account is left alone until a
reload. The skin of a pool name is kept in `config/nick-skins.json` the first time it is found, so
random nicknames work with the services down, and a skin an account's owner changes later does not
reach the server unasked. Skins a player might find unsuitable can appear on any account at any time,
which is why the list is yours to edit: `/nick pool remove`.

## Configuration

`config/nick.toml` is written with comments the first time. A number outside its range is brought
into it, a key the mod does not know is an error, and files that do not read leave the old settings in
place.

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Off, `/nick` says so and every nickname in force is taken off. |
| `command-level` | `2` | The operator level `/nick` needs (1 to 4). |
| `op-level` | `1` | The lowest level that counts as an operator for `@a`, `@p`, `@r` and `admin`. |
| `min-length`, `max-length` | `1`, `16` | How long a nickname may be. |
| `skin-cache-minutes` | `10` | How long a skin the services gave is remembered. |

## Tests

`cargo test --profile fast -p nick` drives the mod's real hooks over a fake server and a table of
accounts (`src/testing.rs`): every command and every message, the selectors, setting and clearing,
random draws, the pool files, the skin lookups (found, no account, services down, answer after the
nickname changed), leaving and joining, and a reload.
