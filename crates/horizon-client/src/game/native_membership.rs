//! The "joined the game" and "left the game" lines for native server mods (docs/native-mods.md,
//! "Server hooks for plugin ports"): a mod that announces a player's leaving itself (a hardcore
//! server whose players leave by dying) turns the real lines off.
use super::Game;

// Native mods use this; nothing in Horizon does.
impl Game<'_> {
    /// Turn the "joined the game" and "left the game" lines on or off for every player. Off, a
    /// player who joins or leaves is not announced to the others (the list entry and the entity
    /// still come and go, and the log still says it), until a mod turns them on again. They are on
    /// when the server starts, as in vanilla, and the setting lasts until the server stops. False
    /// off a server, and inside a guest's packet hook, where the hub is taken out (call it from a
    /// tick hook). API @API:membership_lines@.
    pub(super) fn native_set_membership_lines(&mut self, show: bool) -> bool {
        if !self.native_authoritative() {
            return false;
        }
        match self.lan.as_mut() {
            Some(host) => {
                host.hub.membership_lines = show;
                true
            }
            None => false,
        }
    }
}
