//! Repository tasks, run with `cargo run -p xtask -- TASK`.
//!
//! `mod-api`: fills in the placeholder bodies of new native mod hooks (docs/native-mods.md,
//! "Adding a hook").
//!
//! `tokens`: writes `crates/horizon-theme/tokens.css`, the design tokens of `crates/horizon-theme` as CSS
//! custom properties (a horizon-theme test fails when the file is out of date).

mod mod_api;

use std::fs;
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: cargo run -p xtask -- mod-api | tokens";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [task] if task == "mod-api" => mod_api::run(),
        [task] if task == "tokens" => tokens(),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn tokens() -> Result<(), String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/horizon-theme/tokens.css");
    fs::write(&path, horizon_theme::css()).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote crates/horizon-theme/tokens.css");
    Ok(())
}
