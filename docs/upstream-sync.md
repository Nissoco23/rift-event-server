# Following Horizon

Rift Event Server is a fork of [Horizon](https://github.com/horizon-rust/horizon). This page says where the
fork branched, what was taken from Horizon since, what was left and why, and how to look again.

## Where it stands

- **Branched at** `1405044d09` (Horizon's `main`, 2026-11-10). `main` here is that commit; `dev` is the fork's work.
- **Last reviewed against** Horizon's `main` at `b4db365488` (2026-11-24): 23,546 commits ahead of the branch point
  (8,954 merges, 14,592 other commits), in two weeks.
- **Licence.** Horizon moved from MIT to the GNU LGPL-3.0-or-later on 2026-11-15 (`fb1135c602`, #17111). Of the
  23,546 commits, 3,893 came before that and 19,653 after. This fork is MIT, so **only commits from before the
  move were taken**; the ones after it are listed below, for you to decide on.

## Why Horizon is not merged in

A trial merge of Horizon's `main` into `dev` conflicts in 444 files. Most (383) are the launcher, the registry and
other things this fork removed. The rest is the real problem: Horizon reorganised the engine since the branch
point (the game's modules moved into folders, the native mod API left `horizon-client` for its own crates, new
crates appeared), so 18 files this fork changed (`mod_api.rs`, `native_hooks.rs`, `lan_actions.rs`, `health.rs`,
`hub.rs` and others, which hold the hooks the fork's mods use) no longer exist where they were. Taking the
whole of it means porting the fork's engine changes and its mods onto a new engine, with the licence change on
top: not something to do days before an event. Each fix below was taken one at a time instead.

## What was taken

89 commits, each with `git cherry-pick -x` (the original author stays, and the message ends with the Horizon commit
it came from). Horizon's AI-assistant `Co-Authored-By` lines were dropped from the messages, as this
project's rule says; a human co-author's was kept. Each applied on its own to the fork's code, and the fork's tests
(the server crates, the engine's library tests and every mod) pass with all of them. One of the 89 commits
(a control line cannot carry a second request; a backup is found by its whole label) holds two Horizon fixes
applied by hand.

Left out of what applied cleanly: Horizon's work for older clients (this server takes 26.3 clients), the
client's own screens and the single player, performance refactors (they touch hot paths and the parity
harnesses cannot be run here), and changes that need a Horizon commit this fork does not have.

### Supervisor, files and the management/RCON surface (27)

- `7d84ad3777` Server files: the management secret comes from OS randomness, not the seed's stream (yahya-nusseibeh)
- `63f703ccb8` Server: a --system unit runs as the account that ran sudo, never root (yahya-nusseibeh)
- `adb97c68d7` Server: list edits report a failed save, server.properties is written atomically, plain params (yahya-nusseibeh)
- `5323f98c42` horizon-server: a backup always turns saving back on (yahya-nusseibeh)
- `56a25648ac` horizon-server: a space in a config key can't get past the API's deny-list (yahya-nusseibeh)
- `e0d880754e` Server: per-address RCON places, write timeouts for event streams, digest token compare (yahya-nusseibeh)
- `d08c04020c` RCON: cut a command's output at 4096 UTF-16 units, as vanilla does (Akirabane)
- `289aba5aa5` Cap management and API connections, and give a request one deadline (yahya-nusseibeh)
- `978f37d092` Server: backup and settings edge cases (yahya-nusseibeh)
- `7da7209026` Server: players can't forge the game's log lines or flood the log (yahya-nusseibeh)
- `06d1d236bb` Server: write server.toml atomically under one lock, and keep the world if a restore fails (yahya-nusseibeh)
- `c8370400c6` Supervisor: apply queued restores and updates before every start (yahya-nusseibeh)
- `5e177afe1d` Prune: a spawn radius past i32 keeps everything instead of nothing (yahya-nusseibeh)
- `59972a243d` Panel: wrong login codes no longer lock out the right one (yahya-nusseibeh)
- `d3c30ae2b0` Server: a hand backup cannot take the automatic backups' name (Akirabane)
- `f181472b63` Query: cap the challenges held between prunes (yahya-nusseibeh)
- `eb44631ea7` Server files: read a ban or cache date's zone as SimpleDateFormat does (Akirabane)
- `35e7459102` Management protocol: a permission level is clamped as vanilla's codec reads it (Akirabane)
- `f117ec604a` Server: init refuses a port or world name that server.toml would refuse (Akirabane)
- `5b4d583209` Management protocol: a PlayerDto id of 32 undashed digits is refused, as vanilla's codec does (Akirabane)
- `65676a7dbd` WebSocket: control frames must come whole and short (yahya-nusseibeh)
- `0a9319ac38` Server files: a list or cache file with a byte that is not UTF-8 is read, not taken as missing (Akirabane)
- `eed146379c` Server files: parse_duration refuses a duration that overflows once added to the clock (Akirabane)
- `ae27f478a0` Server files: a UTF-8 byte-order mark at the start of a JSON file is skipped (Akirabane)
- `e7c0b1fd2e` Server panel: a test for the login code's ten minutes and the one-minute failure window (Akirabane)
- `afb158828e` Server: server.toml refuses backups.keep = 0 as `config set` does (#15982) (Akirabane)
- `55634ac84a` Port two server fixes by hand: a control line cannot carry a second request, a backup is found by its whole label (Nissoco)

### Protocol and chat filter (5)

- `654cefca28` Login custom queries: refuse a payload over 1048576 bytes, as vanilla does (Akirabane)
- `b689e1dfa2` Protocol text: a disconnect reason can't expand past 1 MiB (yahya-nusseibeh)
- `98b40e8e5e` protocol: grow the inflate buffer instead of reserving the declared size (yahya-nusseibeh)
- `28584b59bf` A stack's own tool and attribute modifiers reach the client and the hit (Mayonesu0)
- `0db7b7f317` fix(chat-filter): retain indices for large custom lists (angel2rider)

### Anticheat (12)

- `8b7a7d5cfe` Anticheat: write the flag log on a thread of its own (Kacpherek)
- `f31921a9a3` Anticheat: refuse a negative or non-finite timer tolerance_ms or max_lag_ms (Akirabane)
- `541ef4e5fa` Anticheat timer: test the balance at tick rates other than 20 (Akirabane)
- `7528ca15ff` Anticheat prediction: a history gap rebases the replay instead of dropping it (Joansitoh)
- `cb44c743b4` Anticheat: test that each check's table turns off and sets only that check (Akirabane)
- `ce8dba8108` Anticheat: test that every check's thresholds and decay are refused below 0, NaN or infinite (Akirabane)
- `ae667228c5` Anticheat: test that a player past a lowered ban_after is banned (Akirabane)
- `4d9e7b7774` Anticheat timer: test that a stall banks at most max_lag_ms (Akirabane)
- `ec69ba9928` horizon-anticheat: two doc comments say what the defaults are now (Akirabane)
- `cb768472db` Anticheat prediction: test the two caps the docs give, a tick's weight and an excuse's length (#16078) (Akirabane)
- `98a0730d41` Anticheat prediction: test that a push over MOST_MET keeps counting (#16165) (Akirabane)
- `e346e57da6` Anticheat: violation levels can decay by seconds of the server's clock (#16006) (Akirabane)

### World, saves and chunks (9)

- `452d4be9dc` A server player's saved Fire is narrowed to a short too (Verm)
- `23d4652362` Fix gaps and stalled retries in saved far-chunk sending (rdbtCVS)
- `f48c714adc` fix: a chunk the region file cannot deliver is absent, not an error (MSedgeMC)
- `3e6842009d` A chunk saved before 1.17 loads the entities it keeps (#14424) (voduff)
- `dbef6e3e14` fix: bound asynchronous chunk save backlog (Solenopsisbot)
- `f7ff9d2410` Load structure templates faster and preserve numeric NBT fields (Riccardo Guerriero)
- `74db568335` fix: a block entity saved in another chunk moves into the chunk that holds it (MSedgeMC)
- `301fc5fc05` test: a wrongly chunked block entity is read into its chunk (MSedgeMC)
- `a20e9e8575` Restore saved ocean monument positions and layouts (Riccardo Guerriero)

### Gameplay and server behaviour (vanilla parity) (36)

- `511921359c` Fix div_ceil(0) when HORIZON_FAR_TILES=0 (Web-mc1)
- `6dc2d97de4` fix: a shulker box lid updates its neighbours as it moves (MSedgeMC)
- `deb5860d9e` test: a shulker lid reaches all six neighbours (MSedgeMC)
- `3f63aa1f5c` Run a waxed wall hanging sign's click command instead of chaining a sign (Riccardo Guerriero)
- `6539061751` Sneaking keeps the held food off campfires and candles answer pick-block (Riccardo Guerriero)
- `d702f7cf60` A mace smash sends the attacker's motion after the hit (RLC248)
- `af65ddae12` LAN: only the pending keep-alive answer counts (Akirabane)
- `18ceb494e9` Server watchdog: put the server thread's stack in the crash report (Horizon agent)
- `0ce71c6934` Zombies burn you only with a bare hand, on the difficulty's chance (Riccardo Guerriero)
- `ee3e2699ca` An end gateway's ExactTeleport is read as a byte (#14975) (Verm)
- `a0b9b67f4a` Guard add_padding against bits==0 and bits>=64 (Web-mc1)
- `2b01dbcd5e` signs keep one editor at a time and only open on plain text (Riccardo Guerriero)
- `9d025d7e3c` Start the void 64 blocks below each level's own floor (Riccardo Guerriero)
- `c4146a25af` Count the broken statistic for every equipment break (DUzzL)
- `d288db6b6e` Award the crafted statistic for crafting-table and inventory-grid takes (DUzzL)
- `53b7a6768d` Record the predicted drop as sent so a refilling pickup syncs to a guest (DUzzL)
- `9a267cf4bf` Disconnect a player who attacks an item, an orb, themselves or an arrow (Akirabane)
- `3ef0ebf52f` Fix chest boat loot unpacking for hopper access and destruction (Appa)
- `91e62194e8` Jukeboxes keep a finished song stopped after a reload and vibrate when a disc goes in or out (Riccardo Guerriero)
- `c26187a6f0` A use packet with a negative block sequence is dropped before the hub, as in vanilla (Akirabane)
- `40ca777682` LAN and dedicated login: refuse a query answer or cookie response nobody asked for (Akirabane)
- `7cd92e310d` Dedicated server: a /tick sprint does not count toward pause-when-empty (Akirabane)
- `91c0bf1f42` Dedicated console and RCON: commands run as the server's source, as vanilla (Akirabane)
- `1751960d36` Dedicated console: /ban and /ban-ip of someone banned already change nothing (Akirabane)
- `ee8b7b891a` Dedicated server: no idle kick while the End poem is showing (Akirabane)
- `c0def62a4e` Stop an item use when the hand holds another item (Kacpherek)
- `0abe0b86c2` Credit the killer and leave a wither rose when a player dies (Riccardo Guerriero)
- `a7ac8319ba` Keep a mount's inventory open to vanilla's reach instead of 4.5 blocks (Riccardo Guerriero)
- `f23cf4a6a3` Hunt only enemies with a conduit, and show its target to every window (Riccardo Guerriero)
- `2498530cad` Dedicated server: max-chained-neighbor-updates is applied to each level, as vanilla applies it (#15568) (Akirabane)
- `7de27bcaf4` LAN and dedicated: a vehicle move with an infinite coordinate is clamped, not a disconnect, as in vanilla (#15992) (Akirabane)
- `267c676710` Hub: a system message is not sent to a guest whose chat is hidden (#16076) (Akirabane)
- `ea339479a6` Management protocol: view and simulation distance changes also move the server's own tickets (#15621) (Akirabane)
- `cf1b61e9ca` Elytra hops keep their speed: a landed glide's end reaches the client a tick later, as vanilla's (#15643) (Bl4ckspell)
- `cdd614bff7` LAN and dedicated: the idle timer counts the packets vanilla counts, so AFK chat acks and still boats no longer defeat player-idle-timeout (#15676) (Akirabane)
- `4b8f61ed0e` Count cave air as air for open-water fishing and the flying kick (Riccardo Guerriero)

## What was found and left

All of these are from after the licence change, so they are Horizon's LGPL-3.0-or-later code. A commit marked
**applies** applies on its own with `git cherry-pick -x <id>` (fetch Horizon first, below); the others need
their code ported by hand, as the files they change were reorganised.

| Commit | What it fixes | Here |
|---|---|---|
| `74a3f2c91` | Ban, op and whitelist files and the user cache are saved and loaded under a lock, and a save cut off midway leaves the old file | port |
| `5a5aca6b2` | A panic in a player's turn no longer saves that turn's half-done state | port |
| `a21982704` | A region file write keeps its synced chunks through a power cut | port |
| `f775803f2`, `34bc2ff7d` | A player's stats and advancements files are written whole | port |
| `b475dfb95` | The player file is kept when an item nests past the NBT depth limit | port |
| `09c040b80` | A safe force-stop control for hosting panels (#20018) | port |
| `5b13954d9` | A new `server.toml` is private to its owner | port |
| `aecc743f8` | Players hit by a wind charge are not sent back for moving wrongly (anticheat) | port |
| `33d1b8d2a` | The anticheat's ground check while a player falls | port |
| `bb8670d76`, `15e57501d`, `38f36c1cf`, `f48b3e472` | Protocol limits: packed signatures, the command tree's size, translated text, a login key's exponent | `f48b3e472` applies; the others port |
| `d2dd969a4`, `784083dd9` | The chat filter scans after an allowed prefix and skips invisible filler letters; its log keeps a hit on one line | port |
| `a80270c2c` | A name lookup Mojang rate-limits is tried again (whitelisting by name) | applies |
| `1b1d9f94d`, `7045af061`, `386a2c94e` | Backups skip symbolic links; restore needs an explicit name; `init` reads `1`, `yes` and `on` as true | apply |
| `c53eb2be7`, `430986aec` | A world border size set at once reaches the guests; a chunk stored in its own `.mcc` is kept | apply |
| `e26e9ae3a`, `2a50d99c0`, `739d58184`, `ef4c21829` | The web panel's session, the proxy's keep-alives and connection limits, RCON per IPv6 block (only if those are turned on) | port |

## Looking again

```sh
git remote add upstream https://github.com/horizon-rust/horizon.git     # once
git fetch upstream main --no-tags
git rev-list --count main..upstream/main                              # how far ahead
git log --oneline --no-merges main..upstream/main -- crates/horizon-server crates/horizon-server-files \
  crates/horizon-anticheat crates/horizon-protocol crates/horizon-chat-filter   # the server's own crates
git merge-base --is-ancestor <commit> fb1135c602 && echo MIT                    # before the licence change
git cherry-pick -x <commit>                                                     # one fix at a time
```

A fix to a file that Horizon reorganised after the branch point will not apply: read its diff, port it by hand, and
say so in the commit message.
