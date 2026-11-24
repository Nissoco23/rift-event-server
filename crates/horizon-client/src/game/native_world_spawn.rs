//! The world spawn for native server mods (docs/native-mods.md, "Server hooks for plugin
//! ports"): a mod that sends players to a place near it needs to know where it is.
use super::Game;

// Native mods use this; nothing in Horizon does.
impl Game<'_> {
    /// The world spawn (`/setworldspawn`): its block, or none while the level has not found one
    /// yet. API @API:world_spawn@.
    pub(super) fn native_world_spawn(&self) -> Option<[i32; 3]> {
        self.level.world_spawn
    }
}
