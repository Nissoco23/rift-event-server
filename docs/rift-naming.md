# Rift names and Horizon names

Rift Event Server is built from the Horizon codebase. What a player or an operator meets is Rift's. What
sits inside the code is still Horizon's, so that Horizon's own work can keep being merged and so that nothing
internal breaks over a rename. This page lists both sides. When you add text a player or an operator reads,
say Rift.

## Rift (what players and operators see)

- **Product:** Rift Event Server.
- **Programs.** `scripts/build.sh` copies the Cargo outputs into `dist/` under these names:
  - `rift-server`: the supervisor (Cargo package `horizon-server`).
  - `rift-engine`: the game engine with the mods of `rift-mods/` linked in, built without default features
    (package `rift-engine`, which wraps `horizon-client`: [rift-mods.md](rift-mods.md)). The supervisor looks for
    `rift-engine` first, then `horizon-engine`, then `horizon-client`.
  - `rift-prepare`: prepares the game files from your own Minecraft copy (package `horizon-prepare`).
- **CLI and panel:** every `rift-server` and `rift-prepare` help text and message; the web panel's title and
  header; the MCP server's name and instructions; `docs/api/server-reference.md`.
- **Server brand:** the engine tells clients it is `rift` (the `minecraft:brand` message, which clients can
  display). The brand `horizon` that `lan/conn.rs` compares a *client* with is the Horizon client's and
  stays.
- **MOTD:** `rift-server init` writes `motd = "Rift Event Server"` into a new `server.toml`. Vanilla's default
  ("A Minecraft Server") still applies to a `server.toml` written some other way.
- **First log line:** `Rift Event Server <version> (Horizon engine)`.
- **Anticheat:** ban reason `Rift anticheat: <check>` and ban source `Rift anticheat`.
- **Console commands:** `rift-report` and `rift-trace`. The old `horizon-report` and `horizon-trace` still
  work.
- **Service names:** `rift-server-<folder>` (systemd unit, Windows task) and `dev.rift.server.<folder>`
  (launchd label).
- **Updates:** a build from source never checks for updates and `rift-server update` explains why: updating
  needs an installer layout (`releases/` and `current`), which a `dist/` folder is not. The repository the
  updater would use is this one.

## Horizon (kept on purpose)

- **Cargo package names and Rust identifiers:** `horizon-core`, `horizon_client::...` and the rest.
- **Wire protocol:** the `horizon:` channel names (`horizon:mod_list`, `horizon:quiet_commands`,
  `horizon:mod_net`, `horizon:lod`) and the client brand `horizon`.
- **`server.properties` keys:** `horizon-pregen`, `horizon-clients-only`, `horizon-lod-distance`,
  `horizon-max-view-distance`, `horizon-admin-feedback-to-ops` and `horizon-accept-older-clients`.
- **Environment variables:** `HORIZON_*` (`HORIZON_EULA`, `HORIZON_HOME`, `HORIZON_SERVER_JAR`,
  `HORIZON_NO_UPDATE_CHECK`...).
- **Files and folders:** `.horizon/` in a server folder (control endpoint, API token, lock),
  `horizon-resources.json`, the prepared game files in `.local/horizon-client/26.3`, `horizon-mods/`,
  `horizon-native-mods/` and `horizon-smp/`.
- **Web panel header:** `X-Horizon`, which the panel's pages send and its API checks.
- **Prometheus metrics:** `horizon_up`, `horizon_tps`, `horizon_mspt`, `horizon_players_online` and the rest.
  They are pinned by golden files in `crates/horizon-server/testdata/`.
- **Diagnostics:** crash reports (`---- Horizon Crash Report ----`, `Horizon Version`), the `Horizon:` prefix
  of warnings in the log, and `horizon` in a world's `ServerBrands` (`level.dat`). Crash reports are tied to
  an integration test and to the telemetry code.
- **The technical docs under `docs/`:** they describe the engine as Horizon's, and mention `horizon-server`,
  `horizon-prepare` and `horizon-engine`. Read those as `rift-server`, `rift-prepare` and `rift-engine`.
- **Windows icon:** `packaging/icons/horizon.ico`, which `packaging/windows/exe.rs` embeds into a Windows
  build.

## Renaming more later

Names in the second list can be renamed one at a time. For a key or a variable that operators set, accept
both spellings for a while. Renaming a Prometheus metric means updating its golden files, and renaming a
`horizon:` channel breaks Horizon clients, which this server does not need.
