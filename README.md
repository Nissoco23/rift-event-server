# Rift Event Server

The Rift SMP's dedicated event server, built in Rust for large-scale scripted events and cinematic recordings.

Rift Event Server is a server-only fork of [Horizon](https://github.com/horizon-rust/horizon), the
Minecraft: Java Edition 26.3 rewrite in Rust. It keeps Horizon's dedicated-server engine and its supervisor,
and drops the client, the launcher and the other parts a server never runs. Players join with a stock vanilla
26.3 client; put a proxy such as Velocity (with ViaVersion, for older clients) in front of it.

> NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT. Minecraft is a
> trademark of Mojang Synergies AB; its code and assets belong to Mojang and Microsoft under the
> [Minecraft EULA](https://www.minecraft.net/en-us/eula). This repository holds none of them: the engine reads
> the game files from your own copy of Minecraft.

## The programs

- `rift-server`: the supervisor. It keeps the settings in `server.toml`, runs the engine, restarts it if it
  crashes, takes backups, and offers a command line, a web panel, a control API, RCON and an MCP server.
- `rift-engine`: the game engine, run headless by the supervisor.
- `rift-prepare`: builds the engine's game files from your own copy of Minecraft. It never downloads the game.

Their Cargo packages are still called `horizon-server`, `horizon-client` and `horizon-prepare`;
[docs/rift-naming.md](docs/rift-naming.md) says what is Rift's name and what stays Horizon's.

## Build

You need Rust 1.95 or newer ([rustup](https://rustup.rs)) and a C compiler (Xcode Command Line Tools on
macOS, `build-essential` on Linux).

```sh
scripts/build.sh              # fast profile, quick to link: for development and testing
scripts/build.sh --release    # release profile, slow to build: for production
```

Either one leaves `dist/rift-server`, `dist/rift-engine` and `dist/rift-prepare`. The first build takes a
while; later ones only rebuild what changed.

## Prepare the game files (once per machine)

The engine needs data derived from **your own** copy of Minecraft Java Edition 26.3. You need:

- a JDK 25 with `javac` (`java` on the `PATH`, `JAVA_HOME`, or `--java-home PATH`);
- Minecraft Java Edition 26.3, played once from the official launcher, so that its files are on the computer
  (`rift-prepare` reads the launcher's folder, or `--minecraft-dir PATH`);
- the 26.3 `server.jar` (the `downloads.server.url` in the launcher's `versions/26.3/26.3.json`); Prism
  Launcher and MultiMC users skip this, see below.

From the repository folder:

```sh
dist/rift-prepare --prepare --server-jar /path/to/server.jar
```

**Prism Launcher or MultiMC.** They keep no server jar, and `rift-prepare` does not need one: pass
`--prism-dir` the launcher's folder or one of its instances, and `--java-home` a JDK 25. Prism's client jar
stands in for the server side and nothing is downloaded. The exporters are compiled from source, so the Java
must be a JDK with `javac`, not only a runtime; on the machine this was tried on, Prism's own Java 25 (the
`java/java-runtime-epsilon` folder in its data folder) is one. On macOS:

```sh
dist/rift-prepare --prepare \
  --prism-dir "$HOME/Library/Application Support/PrismLauncher/instances/26.3" \
  --java-home "$HOME/Library/Application Support/PrismLauncher/java/java-runtime-epsilon"
```

The result goes to `.local/horizon-client/26.3`, which Git ignores. Never commit or share it. On a headless
machine, prepare on your own computer and copy the folder over with `rift-server import-resources DIR`.

## First run

```sh
mkdir -p run/main && cd run/main        # any folder; run/ is ignored by Git
../../dist/rift-server init             # asks about the EULA, the world, the port and who may join
../../dist/rift-server run              # in the foreground; Ctrl-C stops it cleanly
```

`init --eula --offline --yes --player YourName --op YourName` skips the questions, turns online mode off and
whitelists you, for a first look on your own computer. Then join `localhost:25565` with a vanilla 26.3
client.

The supervisor looks for `rift-engine` beside itself and for the prepared game files in
`.local/horizon-client/26.3` above it. `rift-server --help` lists every command, and the web panel is at
`127.0.0.1:8130` (`rift-server panel` prints a login link).

## Behind Velocity

The proxy signs players in and the server trusts what it forwards:

1. In `server.toml`, set `online_mode = false` and `bind = "127.0.0.1"` (or keep the port on a private
   network), so that only the proxy can reach it.
2. Put the proxy's forwarding secret (Velocity's `forwarding.secret`) in a file `forwarding.secret` in the
   server folder, and set `player-info-forwarding-mode = "modern"` in `velocity.toml`.
3. Restart the server. Its log says `Velocity player information forwarding is on`.

Details: [docs/hosting.md](docs/hosting.md#behind-a-velocity-proxy) and
[docs/velocity-forwarding.md](docs/velocity-forwarding.md).

## What is where

- `crates/`: the engine and its supporting crates. [docs/architecture.md](docs/architecture.md) maps features
  to files, and [AGENTS.md](AGENTS.md) has the crate table.
- `docs/`: [the index](docs/README.md). Start with [hosting](docs/hosting.md) and
  [the mod API](docs/native-mods.md).
- `tools/`, `tests/`, `mods/horizon/`: the harnesses that compare the engine with the real Java game. They
  need prepared game files and a JDK, and they are the specification of vanilla behaviour.
- `scripts/build.sh`: the build. `dist/` and `run/` are generated and ignored by Git.

## Licence and credits

Rift Event Server is [MIT](LICENSE) licensed: Copyright (c) 2026 Nissoco.

It is based on [Horizon](https://github.com/horizon-rust/horizon), Copyright (c) 2026 Ludovic Marie and the
Horizon contributors, MIT, forked from commit `1405044d09`. The MIT terms require Horizon's notice to stay,
and it does in [LICENSE](LICENSE). Dependencies and bundled third-party code keep their own licences
([THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)).
