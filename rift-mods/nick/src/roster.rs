//! The players online as the mod last saw them.
//!
//! The server's players are out of reach in some hooks (a completion request, a player's packet),
//! so the mod keeps its own list: taken whole from `Host::server_players` in a tick and where a
//! command runs, and kept right by the join and leave hooks in between.

use horizon_client::mod_api::NativeServerPlayer;

#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub uuid: u128,
    /// The name they logged in with.
    pub name: String,
    /// Their operator level.
    pub permission: u8,
    pub position: [f64; 3],
    pub dimension: String,
    /// A mod's bot, with no client to give a nickname to.
    pub fake: bool,
}

impl From<&NativeServerPlayer> for Member {
    fn from(p: &NativeServerPlayer) -> Self {
        Self {
            uuid: p.uuid,
            name: p.name.clone(),
            permission: p.permission,
            position: p.position,
            dimension: p.dimension.clone(),
            fake: p.fake,
        }
    }
}

#[derive(Debug, Default)]
pub struct Roster {
    members: Vec<Member>,
}

impl Roster {
    /// Takes the server's players as they are.
    pub fn replace(&mut self, players: &[NativeServerPlayer]) {
        self.members = players.iter().map(Member::from).collect();
    }

    /// A player joined and the server's list was out of reach: they are known by name until the
    /// next whole list says more.
    pub fn joined(&mut self, uuid: u128, name: &str) {
        self.left(uuid);
        self.members.push(Member {
            uuid,
            name: name.to_owned(),
            permission: 0,
            position: [0.0; 3],
            dimension: "minecraft:overworld".to_owned(),
            fake: false,
        });
    }

    pub fn left(&mut self, uuid: u128) {
        self.members.retain(|m| m.uuid != uuid);
    }

    pub fn get(&self, uuid: u128) -> Option<&Member> {
        self.members.iter().find(|m| m.uuid == uuid)
    }

    /// The player logged in as `name` (any case).
    pub fn by_name(&self, name: &str) -> Option<&Member> {
        self.members
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Member> {
        self.members.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(uuid: u128, name: &str, permission: u8) -> NativeServerPlayer {
        NativeServerPlayer::new(uuid, name, permission)
    }

    #[test]
    fn a_whole_list_replaces_what_was_known_and_the_hooks_keep_it_right_between() {
        let mut roster = Roster::default();
        roster.replace(&[player(1, "Bob", 0), player(2, "Alice", 4)]);
        assert_eq!(roster.iter().count(), 2);
        assert_eq!(roster.get(2).unwrap().permission, 4);
        roster.joined(3, "Eve");
        assert_eq!(roster.by_name("EVE").unwrap().uuid, 3);
        roster.left(1);
        assert!(roster.get(1).is_none() && roster.by_name("bob").is_none());
        // A player who joins twice is there once.
        roster.joined(3, "Eve");
        assert_eq!(roster.iter().filter(|m| m.uuid == 3).count(), 1);
        roster.replace(&[player(2, "Alice", 0)]);
        assert_eq!(roster.iter().count(), 1);
        assert_eq!(roster.get(2).unwrap().permission, 0);
    }
}
