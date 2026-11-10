# Rift Event Server documentation

Rift Event Server is a server-only fork of Horizon. Most of these pages describe the Horizon engine it is built
on, and still use Horizon's names: read `horizon-server` as `rift-server`, `horizon-prepare` as `rift-prepare`
and `horizon-engine` as `rift-engine` ([rift-naming.md](rift-naming.md) lists them all). To build and run it,
start with [the README](../README.md).

## Run a server

- [Host a server](hosting.md): the game files, first run, `server.toml`, the commands, the web panel,
  backups, services and Docker; what works today and what doesn't yet.
- [A server's levels](server-levels.md): how the server ticks, saves and travels between dimensions.
- [Velocity forwarding](velocity-forwarding.md): the player-information exchange behind a Velocity proxy and
  how it was checked.
- [The anticheat](anticheat.md): the dedicated server's own checks, violation levels, setback, alerts, kicks,
  the log, and how to write a check.
- [The chat filter](chat-filter.md): hate slurs in chat, message commands, signs, anvil names and book
  titles: the list, mutes, temporary bans, `[chat_filter]`.
- [PvP](pvp.md): fights by 1.8.9's rules with `[combat] legacy = true`.
- [Pregeneration](pregeneration.md): `/pregen`, generating, lighting and saving an area ahead of the players.
- [Resource packs](resource-packs.md): everything a 26.3 resource pack can change, what the engine honours,
  and the server resource pack settings.
- [Security](security.md): how the code, the checks and the signed releases are protected.

## Mods and the mod API

- [Mods](mods.md): content mods (data) and native mods (Rust).
- [Native mods](native-mods.md): the hook points and helpers, the server hooks (commands, players, packets,
  NPCs, bots...), trait mods and how to add a hook.
- [Native hook points modelled on NeoForge](native-hooks-neoforge.md): the proposal for new hooks.
- [Survival multiplayer](smp.md): an example of a server mod built on the server hooks (economy, market,
  trades, homes, claims).
- [Modpacks](modpacks.md): the data-driven content formats content mods use.
- [Live mod reloading](design/mod-hotload.md): loading and enabling mods without a restart.
- [Agent control](agent-control.md): the control socket and the Model Context Protocol server.

## Design and architecture

- [Architecture](architecture.md): which file holds which feature, and which harness checks it.
- [Layers](layers.md): engine, platform and Horizon, the dependency rule, where a new feature goes.
- [Forking](forking.md): the `horizon-extras` feature and building without Horizon's extras.
- [Hosting a server: design](server.md) and [the next server](server-next.md): the design, the measurements
  against vanilla and Pumpkin, and the plan. [Server against Pumpkin](performance/server-vs-pumpkin.md).
- [World generation](world-generation.md), [world height](world-height.md), [world data](world-data.md).
- [Resource preparation](launcher-preparation.md): the Java exporters and `rift-prepare`.
- [Known issues](known-issues.md), [profiling](profiling.md) and [build times](build-times.md).
- [Shader-pack clean room](cleanroom/README.md): how the shader-pack support was written, and the provenance
  log of every source read.

## Parity with vanilla

- [Rust rewrite status](rust-parity.md): where the state of the port lives and the rules every port keeps.
- [Parity workflow](parity-workflow.md): `tools/parity.py`, the gate, other machines, reading vanilla.
- [Port ledger](port-ledger.md): every vanilla method next to the Rust functions that port it.
- [Version ports](version-ports.md): how a new Minecraft release is diffed against the pinned one and ported.
- Area notes: [block behaviours](parity/blocks.md), [section meshing](parity/meshing.md),
  [redstone](parity/redstone-diff.md), [the same seed in vanilla and the engine](parity/world-compare.md).

## APIs

- [APIs](api/README.md): every surface other programs build on, each with its guide, generated reference and
  stability. [Server API](api/server.md) and [its reference](api/server-reference.md) cover `rift-server`.
