//! The Rift engine with its mods linked in.
//!
//! A native mod is Rust code, so it is compiled into the engine's executable rather than loaded by
//! it. This is the engine's own `main` with the mods of `rift-mods/` handed to it. It is a package
//! of its own because `horizon-client` cannot depend on its mods: they depend on it, and Cargo
//! forbids the cycle. `scripts/build.sh` builds it and copies it to `dist/rift-engine`, where
//! `rift-server` looks for the engine. To add a mod, see docs/rift-mods.md.
#![forbid(unsafe_code)]

use horizon_client::mod_api::{Hooks, Mods, name_mods};
use std::sync::Arc;

#[global_allocator]
static ALLOCATOR: horizon_threads::Allocator = horizon_threads::ALLOCATOR;

fn main() {
    // Every mod with its name, in the order the engine runs their hooks: the first to cancel
    // stops the ones after it.
    let mods: Vec<(&'static str, Box<dyn Hooks>)> = vec![
        (sculkstorm::ID, sculkstorm::hooks()),
        (viewdistance::ID, viewdistance::hooks()),
        (nick::ID, nick::hooks()),
    ];
    // The names a crash report and a log line give a mod's hooks by. The engine keeps them for the
    // whole run, and this is the only call, so the list is leaked rather than copied.
    let names: &'static [&'static str] = Box::leak(mods.iter().map(|(id, _)| *id).collect());
    name_mods(names);
    let hooks: Mods = Arc::from(mods.into_iter().map(|(_, h)| h).collect::<Vec<_>>());
    horizon_client::entry::main_with(hooks);
}
