# Rift Event Server: a map for agents

Nissoco's server-only fork of Horizon (Minecraft: Java Edition 26.3 rewritten in Rust, MIT), for the Rift SMP's
event server. The product is the dedicated server: the engine (`horizon-client --dedicated`, built as
`rift-engine`) run by the supervisor (`horizon-server`, built as `rift-server`). Vanilla is the specification
wherever it has the feature. Read [CONTRIBUTING.md](CONTRIBUTING.md) once; this page tells you where things are.
Feature → files → harness for the most-changed areas: [docs/architecture.md](docs/architecture.md).

## Rules that bite

- **Never commit Mojang's files**: game jars, assets, decompiled source, prepared resources (`.local/`).
  `tools/check_public_tree.py` rejects them; `git config core.hooksPath .githooks` runs it on commit.
- **Keep your runs away from real data.** Any engine or tool you start gets fresh `mktemp -d` folders in
  `HORIZON_HOME` (logs, crash reports, telemetry consent, recordings; not saves), `XDG_DATA_HOME` and
  `XDG_CONFIG_HOME`, plus `HORIZON_NO_AUDIO=1`. Saves, `options.txt` and caches go in the game folder, by
  default the working directory: start the engine from a scratch folder with `--game-dir` pointing at it, or
  they land in the repo root, hidden by `.gitignore`. Server folders for real tests go under `run/`, which Git
  ignores.
- **Names.** What a player or an operator reads says Rift; internal names stay Horizon's
  ([docs/rift-naming.md](docs/rift-naming.md)). The programs are `rift-server`, `rift-engine` and
  `rift-prepare` (`scripts/build.sh` copies them into `dist/`); their Cargo packages and binaries are still
  `horizon-server`, `horizon-client` and `horizon-prepare`, which the harnesses use.
- **Where Rift's own code goes.** The server's players use stock vanilla clients behind a Velocity proxy, so
  nothing may need a client mod.
  1. Engine code, for what needs the engine's internals or hot paths: the anticheat
     (`crates/horizon-anticheat`, `lan/anticheat.rs`) and the core commands.
  2. Native mods on the hook points ([docs/native-mods.md](docs/native-mods.md)), for plugin ports and custom
     weapons: vanilla items with data components, plus hooks. A mod must not register custom component types
     or registry content: the engine turns vanilla clients away when it does (`lan/conn.rs`). No mod is linked
     in yet; they will be Rust crates that a small executable hands to `horizon_client::entry::main_with`.
  3. Data packs and resource packs, for data and looks.
- **Commits.** Plain, professional messages: a short imperative subject and a body that says why. No agent or
  tool branding, and no `Co-Authored-By` trailers.
- **Layers** ([docs/layers.md](docs/layers.md)): engine uses engine; platform uses engine and platform; the Horizon
  layer (its extras, cosmetics, telemetry) uses platform and Horizon only. `tools/check_layers.py` checks it, and a
  new crate needs a layer in `tools/layers.json`.
- Everything the server runs is Rust. Python/Java only for dev tools and harness drivers in `tools/`.
- **No hardcoded English for players.** Text the server sends players uses vanilla's translation keys
  (`Text::translate`, `lang::get`); plain English is for the operator's console, logs and CLI.
- **Updates are off.** `rift-server update` works only from an installer layout (`releases/` and `current`),
  which `dist/` is not, and nothing checks for updates by itself.

## Crates

Game workspace (root `Cargo.toml`). The launcher and the other Horizon-only crates are not part of this fork.

| Crate | Owns |
|---|---|
| `horizon-client` | Gameplay library (`src/lib.rs`: `game.rs` + `game/`, `entry.rs`) and thin executable (`src/main.rs`). Also the dedicated server (`--dedicated`), built as `rift-engine`. Re-exports `horizon-client-lib` for existing mod paths. |
| `horizon-client-lib` | Reusable client systems, `horizon-ctl`, probes and their tests. Its manifest points to the existing sources under `crates/horizon-client/`; the library root is `src/client_lib.rs`. It never depends on gameplay. |
| `horizon-items` | Blocks and items below gameplay: `blocks.rs` (block facts), `items.rs`, `item_data.rs` (components), `enchant.rs`, `cooldowns.rs`, `bundle.rs`, `item_nbt.rs`. Re-exported as `horizon_client::items`... |
| `horizon-command` | Command parsing (Brigadier): the vanilla tree, argument types, selectors, suggestions, the tree's network form. Re-exported as `horizon_client::command` |
| `horizon-client-core` | The client lib's bottom layer: `text.rs`, `lang.rs`, `snbt.rs`, `nbt_path.rs`, `component.rs`, `scoreboard.rs`, `bossbar.rs`, `gamerules.rs`, `block_entity_nbt.rs`, registry/particle/sound tables. Re-exported under the old paths (`horizon_client::text`...) |
| `horizon-client-input` | Key bindings and key names, toggle keys, mouse options, controller settings and glyphs, the clipboard, window options (`keybinds.rs`, `gamepad.rs`, `mouse.rs`, `window.rs`...). Re-exported under the old paths |
| `horizon-client-render` | The client's world data, rendering and screens (chunks, meshing, light, streaming, `gfx`, HUD, menus, settings), modules that horizon-client re-exports under the same paths. Nothing in it reaches gameplay |
| `horizon-world` | Vanilla world data with no gameplay loop: NBT, regions, chunks, `datafix/`, `worldgen/`, `level/` (server-side block behaviour, ticks, redstone), entity physics, items, effects, food, damage |
| `horizon-core` | Simulation primitives: Java `Random`s, `jmath` (HotSpot-exact `Math`), positions, noise, density, paletted storage, `version.rs` (the pinned version) |
| `horizon-protocol` | Network protocol 777: codecs, framing, encryption, `packets/{handshake,status,login,configuration,play}.rs`, `entity_data.rs` |
| `horizon-assets` | Resource decoding: block-state and item models, baking, atlases |
| `horizon-prepare` | `rift-prepare`: builds `.local/horizon-client/26.3` from your own Minecraft install (runs the Java exporters) |
| `horizon-server` | `rift-server`: `server.toml`, CLI, supervisor of the headless game ([docs/hosting.md](docs/hosting.md)) |
| `horizon-server-files` | `server.properties`, eula, ops/whitelist/bans, usercache |
| `horizon-anticheat` | The dedicated server's anticheat, on by default: `[anticheat]` settings, violation levels, the checks' rules; the hub runs them (`lan/anticheat.rs`) ([docs/anticheat.md](docs/anticheat.md)) |
| `horizon-cosmetics` | Cosmetic items Horizon clients draw on players: compiled in with the client code, unused by the server |
| `horizon-addresses` | Every built-in address of an outside service vanilla doesn't contact (Horizon layer; [docs/layers.md](docs/layers.md#built-in-addresses)) |
| `horizon-friends` | Play with Friends: invite codes and an encrypted tunnel (the `friends` feature, off in the server build) |
| `horizon-mods` | Content mods (`.hmod`: manifest, content fence, no code) and the registry index ([docs/mods.md](docs/mods.md)). Code mods are native ([docs/native-mods.md](docs/native-mods.md)) |
| `horizon-optifine` | OptiFine-format pack files below every feature that reads them: `.properties`, the name/range/block matchers, paths and `optifine/` folders, and custom entity models (`cem/`: `.jem`/`.jpm` parsing and baking) |
| `horizon-polytone` | Polytone-format pack files (`assets/*/polytone/`), read clean-room from the public wiki: targets, biome and dimension modifiers and their environment attribute layers (#11486) |
| `horizon-shader-api` | Horizon's own WGSL shader packs ([docs/api/shaders.md](docs/api/shaders.md)): `horizon-shaders.toml`, the `.hshader` package; shared by the game, the launcher and the registry |
| `horizon-shaders` | Shader packs (the `shaders/` folder format), rebuilt clean-room from a written spec of the format; no wgpu |
| `horizon-theme` | Design tokens and fonts for the launcher, the game's own menus and the website |
| `horizon-telemetry`, `horizon-bench` | Opt-in data collection the engine keeps and nothing here sends, and the benchmark report format |
| `horizon-system` | The machine the game runs on (OS, CPU, memory, drive, power, GPU maker and memory), for the crash report, the renderer's GPU info and telemetry |
| `horizon-platform` | The platform's public API: what the Horizon layer may use of the engine ([docs/layers.md](docs/layers.md)); re-exports `horizon-system` today |
| `horizon-hooks` | The layer seam: the engine's hooks for crates above it ([docs/layers.md](docs/layers.md#how-a-horizon-crate-plugs-in)) |
| `horizon-dualsense`, `horizon-threads`, `horizon-release-sign`, `horizon-bots`, `xtask` | DualSense output, thread priorities, signed release checksums, load-test bots, repo tasks |
| `horizon-mod-api-gen`, `horizon-mod-deps` | Generates the native mod API (`Hooks`, `Host`, dispatch) from `native-hooks.json` at build time; the allow-list of registry crates a mod may depend on |
| `horizon-chat-filter`, `horizon-mcp` | The dedicated server's chat filter ([docs/chat-filter.md](docs/chat-filter.md)); the Model Context Protocol shared by `horizon-server` and `horizon-ctl` |

Other top-level folders: `scripts/` (`build.sh`), `tools/` (harnesses and dev scripts), `tests/` (Python unit tests: `python3 -m
unittest discover -s tests`), `mods/horizon/src/test/java/dev/horizonlab/parity/` (Java oracles), `docs/`, `packaging/` (a Dockerfile
and the Windows executable's resources), `third_party/` (patched dependencies the build needs), `native-hooks.json` (the native mod
hook catalog) and `research-lock.json` (the pinned Minecraft version and the hashes `rift-prepare` checks). `dist/` (the built
programs) and `run/` (server folders) are generated and ignored by Git.

## Write tests with signal

- Test observable behavior, not words in docs, comments or source. Keep checks for machine-consumed contracts.
- Use independent expected values or a vanilla oracle; do not derive the answer from the code under test.
- Test outcomes, not only calls to stubs. Use mocks to isolate behavior, not as the behavior itself.
- Skip tests of trivial constants, derives and getters, or behavior already covered by another test.
- Avoid pinning implementation details; a harmless refactor should not break a behavior test.
- Make slow tests faster without removing cases or weakening their coverage.
- Never delete parity harnesses or tests that pin vanilla behaviour: they are the spec.

## Build and test fast

```sh
scripts/build.sh                           # dist/rift-server, dist/rift-engine, dist/rift-prepare (--release for production)
cargo test --profile fast -p horizon-server -p horizon-server-files -p horizon-anticheat -p horizon-chat-filter -p horizon-command -p horizon-protocol
cargo test --profile fast -p horizon-client --lib lan::    # engine unit tests by filter: the whole crate is large
cargo fmt --all --check
HORIZON_WRITE_DOCS=1 cargo test --profile fast -p horizon-server reference   # regenerate docs/api/server-reference.md after changing help, config or API text
cargo run -p xtask -- mod-api              # fills in a new native mod hook ("Adding a hook" in docs/native-mods.md)
dist/rift-prepare --prepare --server-jar PATH   # the game files; needs JDK 25 and your own Minecraft 26.3 (README.md)
cargo build --profile fast -p horizon-client        # the full game, optimized and quick to link: lan_e2e runs it
python3 tools/parity.py --changed origin/main       # the harnesses your branch can affect
python3 tools/parity.py --changed-files paths.txt --list   # which harnesses cover these files, without running them
python3 tools/parity.py mob_ai sculk       # named harnesses
python3 tools/lan_e2e.py --check chest     # two real games over LAN (server-thread host)
```

- Harnesses and the engine need the vanilla data in `.local/` (`minecraft/`, `horizon-client/26.3`, `datagen/`,
  `tools/`): see [docs/parity-workflow.md](docs/parity-workflow.md).
- A harness = `tools/test_<name>_parity.py` + a Java oracle (`mods/horizon/src/test/java/dev/horizonlab/parity/Vanilla*Oracle.java`)
  + a Rust probe: `crates/<crate>/examples/<name>_{probe,fixture}.rs` (mostly horizon-world and horizon-client),
  or the game binary with a fixture env var (`game/dispense_fixture.rs`, `game/sculk_fixture.rs`,
  `game/command_harness.rs`, `game/net_harness.rs`). Copy a nearby harness for a new one.
- Harness scripts run the Cargo outputs in `target/` (`horizon-client`, `horizon-server`), not the renamed
  copies in `dist/`.
- Probes build into `.local/parity-target/`; harnesses find binaries via `lab.cargo_target()`, never `target/`.
- Harness selection uses Cargo dep-info and reachable-code narrowing (`tools/parity_reach.py`).
  `horizon-client-lib` builds the control tool and client probes without gameplay; their dep-info
  excludes `game.rs` and `game/`, so gameplay edits do not rebuild or select those probes.
  Source paths remain in `crates/horizon-client/`. Measure actual harness selection on refreshed
  records with `tools/parity.py --changed-files paths.txt --list`; historical counts are not a guarantee.
- Rust integration tests: `crates/horizon-client/tests/` (`lan_bot.rs`, `generation_saves.rs`, `streaming_meshes.rs`).
- The server build (`--no-default-features`) drops audio, controllers and `friends`, and links neither
  `libasound` nor `libudev` on Linux; the full game needs `libasound2-dev libudev-dev`.

## Vanilla reference

- Start parity work from the port ledger, not grep: `python3 tools/port_ledger.py issue N` (or `where
  Class.method`, `what file:line`), and `check --base origin/main` before the PR. How:
  [.claude/skills/vanilla-parity/SKILL.md](.claude/skills/vanilla-parity/SKILL.md), [docs/port-ledger.md](docs/port-ledger.md).
- Decompiled sources: `.local/minecraft/26.3/sources/{client,server}/` (`python3 tools/lab.py decompile`).
- Bytecode queries (callers through lambdas, every overload):
  `python3 tools/rea_vanilla.py method net.minecraft.world.level.block.DispenserBlock dispenseFrom`
  ([docs/parity-workflow.md](docs/parity-workflow.md#reading-vanilla)).
- A harness runs the oracle against the real 26.3 jars and the Rust probe on the same inputs, then compares
  their JSON. Oracle runs are cached by `tools/parity_shim.py`; `--fresh` reruns them.
- Mirror vanilla's `Math` calls with `horizon_core::jmath`, and vanilla's RNG with `horizon_core::random`:
  random draws must happen in vanilla's order.

## horizon-client: where things live

`L:` = lib (`crates/horizon-client/src/`, or `crates/horizon-client-render/src/` for the modules that crate holds, under
the same `horizon_client::<module>` paths), `G:` = gameplay (`crates/horizon-client/src/game/`), also in the lib in normal builds.
Gameplay keeps its `horizon_client::<module>` imports through a self-crate alias. The executable
contains only the allocator, mod API marker and a call to `entry::main`. For unit tests, the lib
excludes `entry` and `game`, and the executable test target owns `game` so its process-wide test
state stays separate from the library tests. This compiles gameplay twice for executable tests.

| Subsystem | Files |
|---|---|
| Game loop, tick, frame | `game.rs`: `fn run`, `fn tick`, `fn tick_world`, `fn render`; G: `window_handler.rs` (`impl ApplicationHandler for Game`: `window_event`, `about_to_wait`), `launch_options.rs` (the command line); L: `clock.rs`, `spans.rs`; `horizon-client-input/src/window.rs` |
| Rendering | L: `gfx.rs` (`Renderer::new`, `Renderer::render`), `gfx/` (cull, occlusion, translucency, upload, timing, lod, weather, stars, end_sky), `*.wgsl`, `particles.rs`, `block_entity_render.rs`, `item_render.rs`, `fog.rs`, `clouds.rs`, `camera.rs`, `first_person.rs`; G: `render_tick.rs`, `frame_geometry.rs` (cracks, sun and moon, first-person hand, rain and snow), `item_draw.rs`, `screen_backgrounds.rs` (panorama, menu blur, border wall) |
| Chunks, streaming, light | L: `streaming.rs` (worker pool: generate, light, mesh), `world.rs`, `chunk.rs`, `chunk_map.rs`, `light.rs`, `mesher.rs` + `mesher/`, `vanillagen.rs` (binds horizon-world's generator), `lod.rs` + `lod/`, `server_terrain.rs` (a server's kept chunks), `bluemap.rs` + `bluemap_detect.rs` (far terrain from its web map), `ticket_levels.rs`; G: `world_loading.rs`, `world_entry.rs`, `postprocess.rs`, `world_update.rs` (`fn update_world`, far terrain) |
| World generation | `crates/horizon-world/src/worldgen/` (`generator*.rs`, `feature_graph.rs`, `structure/`, `carver/`) |
| Block behaviour | `crates/horizon-world/src/level/` (`mod.rs`, `tick.rs`, `hooks.rs`, `redstone.rs`, `place.rs`, `interact.rs`); G: `block_ticks.rs` (`fn tick_blocks`); game.rs `fn process_level_events` |
| Mobs | L: `mobs.rs` (types), `mobs/` (`ai.rs`, `goals.rs`, `brain.rs`, `*_brain.rs`, `nav.rs`, `path.rs`, `species.rs`, `spawning.rs`, `render.rs`, `anim.rs`); G: `creatures.rs` (ticking, spawning, mob events), one file per mob family (`dragon.rs`, `wither.rs`, `villagers.rs`, `piglins.rs`, `golems.rs`...), `natural_spawning.rs` |
| Other entities | L: `entities.rs`, `projectiles.rs`, `arrows.rs`; G: `item_entities.rs` (dropped items, experience orbs), `boats.rs`, `minecarts.rs`, `mounts.rs`, `armor_stands.rs`, `paintings.rs`, `frames.rs`, `fireworks.rs`, `tnt.rs`, `explosion.rs` |
| Player actions | game.rs (`fn tick_mining`, `fn break_block`); G: `use_hand.rs` (`fn use_hand`), `item_actions.rs` (`fn use_item`), `melee.rs` (`fn attack`), `hand.rs`, `combat.rs`, `consume.rs`, `using_item.rs` (an item in use, tick by tick), `potions.rs`, `shield.rs`, `crossbow.rs`, `trident.rs`, `fishing.rs`; L: `motion.rs` + `motion/` (movement), `collisions.rs`; crate `horizon-items`: `items.rs`, `item_data.rs`, `enchant.rs`, `blocks.rs` |
| Inventory and containers | L: `menu.rs` (`AbstractContainerMenu.clicked`), `container_screen.rs`, `containers.rs`, `creative.rs`, `crafting.rs`, `recipe_book*.rs`, `stations.rs`, `merchant.rs`; G: `menu_clicks.rs` (`fn menu_ops`, `fn click_inventory`), `stations.rs`, `trading.rs`, `recipe_book.rs`, `machines.rs`, `inventory_input.rs` (keys and clicks over a container), `inventory_slots.rs` (slot layout and drawing) |
| Commands | parse: crate `horizon-command` (`args.rs`, `selector.rs`, `tree_*.rs`, `vanilla.rs`); run: G: `commands.rs` (dispatch) + `commands/*.rs` (`admin`, `execute`, `items`, `players`, `world_cmds`...); L: `scoreboard_cmd.rs`, `data_cmd.rs`; `horizon-client-core`: `nbt_path.rs`, `scoreboard.rs`; G: `functions.rs`, `command_blocks.rs` |
| UI screens | `Screen` enum in game.rs; L: `title.rs` + `title/`, `options_ui.rs`, `multiplayer.rs`, `pack_screen.rs`, `settings_shell.rs` (Horizon's settings screens on it: `extras_ui.rs`, `graphics_ui.rs`, `controller_ui.rs`, `shader_ui.rs`; a page list too long for its column collapses into one row), `screen_draw.rs`, `ui.rs` (widgets), `hud.rs`, `hud_layout.rs`, `hud_overlays.rs`, `hud_kit/` (Horizon overlays' `Room` and `Plate`, the collapsed page strip `tabs` the shell's page list uses, [docs/ui-kit.md](docs/ui-kit.md)), `pad_glyph.rs` (controller glyphs, by what the button does), `chat.rs`, `tab_list.rs`; G: `startup_menu.rs`, `loading_screen.rs` (the loading screen, the pre-game `App`), `resource_loader.rs` (what a start loads, `run_loading`), `screens.rs` (pause, death), `options.rs`, `lan_screen.rs`, `chat.rs`, `sign_edit.rs`, `books.rs`, `creative_screen.rs` |
| Multiplayer client | L: `net.rs` (network thread); G: `remote.rs` (`fn handle_packet`, `fn tick_remote`), `remote_*.rs` (mob data, objects, projectiles, vehicles, stations) |
| LAN host / server side | L: `lan/` (`conn.rs` handshake + login threads, `hub.rs` + `hub/` play state, `chunks.rs`, `entity.rs`, `tracking.rs`, `guest.rs`, `playerdata.rs`); G: `lan.rs` (`LanHost`), `lan_actions.rs` (`fn as_guest`, `fn tick_guests`), `lan_*.rs` per feature, `lan_entities.rs` (tracked entities) |
| Integrated server | G: `integrated.rs` (the server thread), `levels.rs` (one level per dimension, `fn tick_server`), `server_travel.rs` |
| Dedicated server | G: `dedicated.rs`, `dedicated/status.rs`; crates `horizon-server-files`, `horizon-server`, `horizon-anticheat` (L: `lan/anticheat.rs`) |
| Save and load | L: `save.rs` (level.dat, regions, playerdata), `entity_nbt.rs`, `options_file.rs`; `horizon-client-core/src/block_entity_nbt.rs`; G: `saving.rs`, `persist.rs` (entities), `world_session.rs` (opening and closing a world); `crates/horizon-world/src/{region.rs,nbt.rs,datafix/}` |
| Input | game.rs `fn window_event`; G: `play_keys.rs` (`fn key_pressed`), `play_input.rs` (`fn play_mouse`, toggled keys, the hotbar wheel); crate `horizon-client-input` (`keybinds.rs`, `keymapping.rs`, `key_names.rs`, `mouse.rs`, `input.rs`, `interaction_input.rs`, `gamepad.rs`, `clipboard.rs`); L: `osk.rs`; G: `pad.rs`, `pad_menu.rs`, `pad_craft.rs`, `pointer_capture.rs` (mouse look, pointer grab) |
| Audio | L: `audio.rs` (mixer), `sound_engine.rs`, `sound_events.rs`, `music.rs`, `ambience.rs`; G: `music.rs`, `subtitle_hud.rs` |
| Resource packs | L: `packs.rs` + `packs/`, `pack_screen.rs`, `atlas_sources.rs`, `sprites.rs`, `mipmap.rs`, `model_library.rs`, `font.rs`, `mobs/cem.rs` + `mobs/cem_anim.rs` (OptiFine/EMF entity models and their animations); `horizon-client-core/src/lang.rs`; crates `horizon-assets`, `horizon-optifine`; G: `packs.rs` |
| Agent control | L: `control.rs` (socket), `bin/horizon-ctl/`; G: `control.rs`, `agent.rs`, `events.rs`, `ui_control.rs` ([docs/agent-control.md](docs/agent-control.md)) |
| Mods | crate `horizon-mods`; G: `mods.rs`; L: `mod_content.rs`, `mod_list.rs` |
| Horizon extras, settings | L: `extras.rs`, `graphics.rs`, `settings.rs`, `minimap.rs`; G: `extras.rs`, `tweaks.rs`, `options.rs` |

## How `Game` is organised

- `struct Game` (game.rs) holds everything. Its methods are spread over many files: game.rs keeps
  the core (tick, render, input, inventory, world open/close) and each `game/<feature>.rs` adds an
  `impl Game<'_>` block for one feature, with its own state types. Fields stay private to the gameplay module and its descendants. Gameplay tests run in the gameplay library, separate from reusable-library tests.
- Find a feature's code: `grep -rn "fn tick_<thing>\|fn <verb>_<thing>" crates/horizon-client/src/game*`;
  the file is usually `game/<thing>s.rs`. Who calls it: grep the name in game.rs's `fn tick` / `fn tick_world`.
- Fields that say which mode runs: `remote` (this window is a client of a server: multiplayer, LAN guest, or
  single player on its server thread), `lan` (`LanHost`: this world is open to the network), `integrated`
  (the window's own server thread), `serving` (this `Game` *is* that server thread), `options.dedicated`.
  `fn tick` returns `tick_remote()` when `remote` is set; a server ticks its levels (`fn tick_world`).
- A server's players run through the shared gameplay code: `as_guest` swaps the guest's
  state (position, inventory, mode, mining, item use) into `Game` for one call (`game/lan_actions.rs`), and
  `levels.rs` swaps whole dimensions in the same way. Changes reach clients through the block log, level
  events and the entity tracker (`lan/tracking.rs`, `game/lan_entities.rs`).
- Shared read-only data: `Resources` (`res`: blocks, items, recipes, mob types, biomes) and `PackResources`
  (`pack`), both in game.rs, their methods in `game/resource_tables.rs`.

## Single player runs on a server thread

`game/integrated.rs` runs a second `Game` (`serving`) on a "Server thread" with one level per dimension;
the window joins it as a client through memory (`remote`), exactly as a LAN guest or a dedicated server's
player does (#1514). A window's `Game` never ticks a world itself: the old in-frame path is gone
(`--in-frame` and `--no-server-thread` only print a notice; `HORIZON_SERVER_THREAD` is ignored).
Open `[server-thread]` issues list the gaps left.

Single player, LAN guests and dedicated servers all run the server code, so a fix there serves all three.
Test the server thread when you touch player-facing gameplay:

```sh
cargo run --profile fast -p horizon-client -- --resources .local/horizon-client/26.3 --world T               # server thread
python3 tools/lan_e2e.py --check <name>                  # host on its server thread, guest is a server client
python3 tools/parity.py dedicated_server server_entities server_player_damage
```

## Docs worth knowing

[docs/rift-naming.md](docs/rift-naming.md) (Rift names and Horizon names) ·
[docs/hosting.md](docs/hosting.md) (the operator's guide) ·
[docs/native-mods.md](docs/native-mods.md) (the mod API and its server hooks) ·
[docs/anticheat.md](docs/anticheat.md) ·
[docs/parity-workflow.md](docs/parity-workflow.md) (runner, gate, shards, scopes) ·
[docs/server-levels.md](docs/server-levels.md) (levels per dimension) ·
[docs/rust-parity.md](docs/rust-parity.md) (what is ported) · [docs/known-issues.md](docs/known-issues.md) ·
[docs/README.md](docs/README.md) (index).
