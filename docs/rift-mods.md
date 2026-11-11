# Rift's own mods

Rift Event Server's own code mods (plugin ports, custom weapons) are Rust crates in `rift-mods/`, one
folder each, linked into the engine program the server runs. There is no plugins folder to drop a file
into: a native mod is compiled into the engine, as [Trait mods](native-mods.md#trait-mods-draft)
describe. The launcher those pages mention is not part of this fork, so `crates/rift-engine` does its
job.

## How it fits together

- `rift-mods/<name>/`: the mod, a library crate (package `<name>`) that depends on `horizon-client`
  for `horizon_client::mod_api` and exports `pub const ID: &str` and `pub fn hooks() -> Box<dyn Hooks>`.
  Its README, its tests and its resource pack, if it has one, live in its folder.
- `crates/rift-engine`: the engine's `main` with the mods handed to it. `name_mods` names them and
  `horizon_client::entry::main_with` takes their hooks, in the order they are listed. It is a package
  of its own because Cargo forbids a package depending on a crate that depends on it back, and the
  mods depend on `horizon-client`.
- `scripts/build.sh` builds `rift-engine` and copies it to `dist/rift-engine`, where `rift-server`
  looks for the engine. `horizon-client` stays the plain engine: the harnesses run it, so a mod never
  changes what they pin. The Docker image (`packaging/server/Dockerfile`) builds `rift-engine` too.
- The supervisor, the server folder, the world and `server.toml` are the same with or without mods:
  the engine program is the only difference. A mod keeps its own files in the server folder
  (`config/<mod>.toml`, a folder in the world). A mod must not register custom component types or
  registry content, or the engine turns vanilla clients away (`lan/conn.rs`).

## Adding a mod

1. Make `rift-mods/<name>/` with a `Cargo.toml` like `rift-mods/sculkstorm/Cargo.toml` (version,
   edition, licence and lints from the workspace; `horizon-client = { path =
   "../../crates/horizon-client", default-features = false }`) and a `src/lib.rs` that implements
   `Hooks` and exports `ID` and `hooks()`.
2. Add it to `members` in the root `Cargo.toml`, and give it a layer in `tools/layers.json`
   (`platform`: a mod uses the engine and sits above it). `python3 tools/check_layers.py` checks it.
3. Add `<name> = { path = "../../rift-mods/<name>" }` to `crates/rift-engine/Cargo.toml` and its
   `(ID, hooks())` to the list in `crates/rift-engine/src/main.rs`. The engine runs the mods' hooks in
   that order.
4. Test it as [Testing a trait mod](native-mods.md#testing-a-trait-mod) says, then `scripts/build.sh`
   puts it in `dist/rift-engine`.

## The mods

- [Sculkstorm](../rift-mods/sculkstorm/README.md): the Sculkstorm Scythe, with Sculk Cataclysm and
  Soulstorm Ascension.
- [ViewDistance](../rift-mods/viewdistance/README.md): a view distance limit for players and another for
  operators.
