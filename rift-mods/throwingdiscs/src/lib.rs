//! Throwing discs for Rift Event Server: a port of the Paper plugin of the same name, which adds the
//! throwing disc Mojang showed at TwitchCon San Diego. Right-click throws the disc along the view; it
//! pierces what it meets, ricochets off hard blocks and returns to the hand it came from.
//!
//! A disc is an echo shard made of vanilla components, so stock clients can use it (with the
//! resource pack for its model). The engine runs mods as Rust code linked into its executable, so
//! this crate is a library (`ThrowingDiscs`, a `Hooks`) and `crates/rift-engine` is the engine's
//! `main` with it, and every other mod in `rift-mods/`, handed over (docs/rift-mods.md).
#![forbid(unsafe_code)]

mod anvil;
mod commands;
mod config;
mod flight;
mod item;
mod recipes;
mod rng;
mod smith;
mod state;
#[cfg(test)]
mod testing;
mod vec3;

use config::Config;
use flight::{Around, Disc, Step};
use horizon_client::mod_api::{
    AnvilUpdateEvent, DataPackFilesEvent, EquipmentSlot, Flow, GameRuleValue, Hand, Hooks, Host,
    ItemStack, LevelTickPostEvent, LivingDeathEvent, NativeInventoryKind, NativeServerPlayer,
    PerformCommandEvent, PerformConsoleCommandEvent, PlayerCommandEvent, PlayerLoggedInEvent,
    PlayerLoggedOutEvent, PlayerRespawnedEvent, PrepareItemCraftV2Event, PrepareSmithingV2Event,
    RegisterCommandsEvent, ServerStoppingEvent, ServerTickEvent, ServerTickPhase,
    TabCompleteV2Event, UseItemEvent,
};
use rng::Rng;
use state::{Member, Shared, Task};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The name the engine's executable gives the mod (`horizon_client::mod_api::name_mods`), which a
/// crash report and a log line name its hooks by.
pub const ID: &str = "throwingdiscs";

/// The mod as the engine's executable takes it: the `Hooks` to hand to
/// `horizon_client::entry::main_with`.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(ThrowingDiscs::default())
}

/// The mod: the discs in flight and the config.
pub struct ThrowingDiscs {
    shared: Arc<Shared>,
    config_loaded: AtomicBool,
}

impl Default for ThrowingDiscs {
    fn default() -> Self {
        Self::with(Rng::from_clock())
    }
}

impl ThrowingDiscs {
    pub(crate) fn with(rng: Rng) -> Self {
        Self {
            shared: Arc::new(Shared::new(rng)),
            config_loaded: AtomicBool::new(false),
        }
    }

    /// The state, for a test that has boxed the mod and still wants to look inside it.
    #[cfg(test)]
    pub(crate) fn handle(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    /// Reads `config/throwingdiscs.toml` (writing it when the server has none) and takes it up. An
    /// error leaves the settings as they were.
    pub(crate) fn reload_config(&self, host: &dyn Host) -> Result<Arc<Config>, String> {
        let dir = host.config_dir()?;
        self.shared.set_config(config::load(&dir)?);
        Ok(self.shared.config())
    }

    /// Reads the config the first time a hook needs it.
    fn ensure_config(&self, host: &dyn Host) {
        if self.config_loaded.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Err(e) = self.reload_config(host) {
            host.log(
                "WARN",
                &format!("ThrowingDiscs: could not read the config, using the defaults: {e}"),
            );
        }
    }

    // ---- a tick of one level -------------------------------------------------------------

    /// What the queued work and the flights of `dimension` do this tick.
    fn run_level(&self, host: &mut dyn Host, dimension: &str) {
        self.ensure_config(&*host);
        let cfg = self.shared.config();
        let players = host.server_players();
        let by_entity: HashMap<u32, NativeServerPlayer> = players
            .iter()
            .filter_map(|p| {
                Some((
                    u32::try_from(p.entity_id).ok().filter(|id| *id > 0)?,
                    p.clone(),
                ))
            })
            .collect();

        // The players as this tick sees them, for the hooks that cannot reach the server's players.
        let roster: Vec<Member> = players
            .iter()
            .map(|p| Member {
                player: p.clone(),
                holds_disc: host.held_stack(p.uuid).is_some_and(|s| item::is_disc(&s)),
            })
            .collect();
        self.shared.state().roster = roster;

        // A crash can leave a flight's display behind: remove them once per level.
        if self.shared.state().cleaned.insert(dimension.to_owned()) {
            host.run_command(&format!(
                "execute in {dimension} run kill @e[type=minecraft:item_display,tag={}]",
                flight::FLIGHT_TAG
            ));
        }

        let tasks: Vec<Task> = self.shared.state().tasks.drain(..).collect();
        for task in tasks {
            let uuid = match &task {
                Task::Throw { uuid, .. } | Task::Enchant { uuid, .. } => *uuid,
            };
            let Some(player) = players.iter().find(|p| p.uuid == uuid) else {
                continue;
            };
            if player.dimension != dimension {
                self.shared.state().tasks.push_back(task);
                continue;
            }
            match task {
                Task::Throw { hand, .. } => self.throw(host, &cfg, player, hand),
                Task::Enchant {
                    enchantment, level, ..
                } => enchant_held(host, player, &enchantment, level),
            }
        }

        let mine: Vec<Disc> = {
            let mut state = self.shared.state();
            let (mine, rest) = std::mem::take(&mut state.flying)
                .into_iter()
                .partition(|d| d.dimension == dimension);
            state.flying = rest;
            mine
        };
        let mut keep = Vec::new();
        for mut disc in mine {
            let owner = players.iter().find(|p| p.uuid == disc.owner);
            let around = Around {
                cfg: &cfg,
                owner,
                players: &by_entity,
            };
            match disc.tick(host, &around) {
                Step::Flying => keep.push(disc),
                Step::Caught => disc.give_back(host, true),
                Step::GiveBack => disc.give_back(host, false),
                Step::DropHere => self.owner_gone(host, &mut disc, owner),
            }
        }
        self.shared.state().flying.extend(keep);
    }

    /// The disc's owner is gone or dead. A dead one under keepInventory has the disc back when they
    /// respawn; without it, it drops with the rest of their items.
    fn owner_gone(&self, host: &mut dyn Host, disc: &mut Disc, owner: Option<&NativeServerPlayer>) {
        let Some(owner) = owner else {
            return disc.drop_here(host);
        };
        disc.remove_display(host);
        if keeps_inventory(host, &disc.dimension) {
            self.shared
                .state()
                .returns
                .entry(owner.uuid)
                .or_default()
                .push(disc.item().clone());
        } else {
            flight::drop_item(host, &disc.dimension, owner.position, disc.item());
        }
    }

    /// Takes the disc out of `hand` and sends it flying.
    fn throw(&self, host: &mut dyn Host, cfg: &Config, player: &NativeServerPlayer, hand: Hand) {
        let (held, slot) = match hand {
            Hand::Main => (
                host.held_stack(player.uuid),
                usize::from(player.selected_slot),
            ),
            Hand::Off => (
                host.player_equipment(player.uuid, EquipmentSlot::OffHand),
                OFF_HAND_SLOT,
            ),
        };
        let Some(held) = held else {
            return;
        };
        let Some(tier) = item::tier_of(&held) else {
            return;
        };
        let mut thrown = held.clone();
        let cost = u32::try_from(cfg.durability_per_throw).unwrap_or(0);
        // 1 is creative, where nothing wears.
        if player.game_mode != 1
            && cost > 0
            && let Some(max) = item::max_damage(&held)
        {
            // Same rule as the trident: a disc with one durability point left can't be thrown.
            if held.damage + cost >= max {
                return;
            }
            let unbreaking = item::level(&held, "minecraft:unbreaking");
            let used = {
                let mut state = self.shared.state();
                (0..cost)
                    .filter(|_| state.rng.below(unbreaking + 1) == 0)
                    .count() as u32
            };
            thrown.damage += used;
        }
        if host
            .set_inventory_slot(
                player.uuid,
                NativeInventoryKind::Player,
                slot,
                Some(&held),
                None,
            )
            .is_err()
        {
            return;
        }
        let disc = Disc::launch(host, cfg, player, thrown, tier, hand);
        self.shared.state().flying.push(disc);
    }

    /// Hands every flying disc of `owner` back, or all of them when `owner` is None.
    fn recall(&self, host: &mut dyn Host, owner: Option<u128>, into_inventory: bool) {
        let mine: Vec<Disc> = {
            let mut state = self.shared.state();
            let (mine, rest) = std::mem::take(&mut state.flying)
                .into_iter()
                .partition(|d| owner.is_none_or(|o| d.owner == o));
            state.flying = rest;
            mine
        };
        let online = host.server_players();
        for mut disc in mine {
            if into_inventory && online.iter().any(|p| p.uuid == disc.owner) {
                disc.give_back(host, false);
            } else if into_inventory {
                // Gone already: their saved inventory takes it, else it drops where it is.
                disc.give_back_offline(host);
            } else {
                disc.drop_here(host);
            }
        }
    }
}

/// The off hand's slot in a player's inventory (`NativeInventoryKind::Player`).
const OFF_HAND_SLOT: usize = 40;

/// Whether the `keep_inventory` rule is on in `dimension`.
fn keeps_inventory(host: &dyn Host, dimension: &str) -> bool {
    host.level(dimension)
        .and_then(|level| level.game_rule(host, "keep_inventory"))
        .is_some_and(|v| matches!(v, GameRuleValue::Bool(true)))
}

/// Puts `enchantment` at `level` on the disc the player holds, as `/enchant` does (a level already
/// there is set to the new one).
fn enchant_held(host: &mut dyn Host, player: &NativeServerPlayer, enchantment: &str, level: u32) {
    let Some(held) = host.held_stack(player.uuid) else {
        return;
    };
    if !item::is_disc(&held) {
        return;
    }
    let mut changed = held.clone();
    let mut levels = changed.enchantments().to_vec();
    horizon_client::item_data::set_level(&mut levels, enchantment, level);
    changed.set_crafting_enchantments(levels);
    let slot = usize::from(player.selected_slot);
    if let Err(e) = host.set_inventory_slot(
        player.uuid,
        NativeInventoryKind::Player,
        slot,
        Some(&held),
        Some(changed),
    ) {
        host.log(
            "WARN",
            &format!(
                "ThrowingDiscs: could not enchant {}'s disc: {e}",
                player.name
            ),
        );
    }
}

impl Hooks for ThrowingDiscs {
    fn server_tick(&self, _host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        if let ServerTickPhase::ServerTickEnd {
            world_running: true,
        } = event.phase
        {
            self.shared.state().tick += 1;
        }
    }

    fn level_tick_post(&self, host: &mut dyn Host, event: &mut LevelTickPostEvent<'_>) {
        self.run_level(host, event.dimension);
    }

    /// Right-click on a disc: it is thrown at the next level tick. Only a disc is claimed; a click on
    /// a chest or a door never gets here, as the block takes it first (sneak to throw at one).
    fn use_item(&self, host: &mut dyn Host, event: &mut UseItemEvent<'_>) -> Flow {
        if !item::is_disc(event.stack) {
            return Flow::Continue;
        }
        self.ensure_config(&*host);
        let Some(player) = host.acting_player() else {
            return Flow::Continue;
        };
        // The hook is told the stack and not the hand: the main hand goes first, as it does in vanilla.
        let hand = if host
            .held_stack(player.uuid)
            .is_some_and(|main| &main == event.stack)
        {
            Hand::Main
        } else {
            Hand::Off
        };
        let cfg = self.shared.config();
        let cost = u32::try_from(cfg.durability_per_throw).unwrap_or(0);
        if player.game_mode != 1
            && cost > 0
            && let Some(max) = item::max_damage(event.stack)
            && event.stack.damage + cost >= max
        {
            return Flow::Continue;
        }
        let mut state = self.shared.state();
        // A player throws at most one disc a tick: with a disc in each hand, one click fires for both,
        // so the main-hand disc goes first and the off-hand disc needs a second click.
        let now = state.tick;
        if state.last_throw.get(&player.uuid) == Some(&now) {
            return Flow::Continue;
        }
        state.last_throw.insert(player.uuid, now);
        state.tasks.push_back(Task::Throw {
            uuid: player.uuid,
            hand,
        });
        Flow::Cancel
    }

    fn register_commands(&self, _host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        commands::register(event.commands);
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        self.ensure_config(&*host);
        self.discs_command(host, event.command)
    }

    /// Vanilla's `/enchant` on a held disc. This is the hook that hears every command: the registered
    /// ones only reach `perform_command`, and `/enchant` is vanilla's.
    fn player_command(&self, _host: &mut dyn Host, event: &mut PlayerCommandEvent<'_>) -> Flow {
        let outcome = {
            let state = self.shared.state();
            commands::enchant(event.player, event.command, &state.roster)
        };
        match outcome {
            commands::Enchant::Vanilla => Flow::Continue,
            commands::Enchant::Handled { replies, tasks } => {
                event.replies.extend(replies);
                self.shared.state().tasks.extend(tasks);
                Flow::Cancel
            }
        }
    }

    fn perform_console_command(
        &self,
        host: &mut dyn Host,
        event: &mut PerformConsoleCommandEvent<'_>,
    ) -> Flow {
        self.ensure_config(&*host);
        self.discs_command(host, event.command)
    }

    fn tab_complete_v2(&self, _host: &mut dyn Host, event: &mut TabCompleteV2Event<'_>) {
        if !event.label.eq_ignore_ascii_case("discs") {
            return;
        }
        let Some((typing, before)) = event.arguments.split_last() else {
            return;
        };
        let before: Vec<String> = before.iter().map(|w| w.to_ascii_lowercase()).collect();
        let before: Vec<&str> = before.iter().map(String::as_str).collect();
        let found = commands::completions(&before, typing, (event.range[0], event.range[1]));
        event.suggestions.extend(found);
    }

    /// The recipes, when the crafting switch is on.
    fn data_pack_files(&self, host: &dyn Host, event: &mut DataPackFilesEvent<'_>) {
        self.ensure_config(host);
        event.files.extend(recipes::files(&self.shared.config()));
    }

    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        self.ensure_config(&*host);
        if !self.shared.config().crafting {
            return;
        }
        for id in recipes::ids() {
            host.run_command(&format!("recipe give {} {id}", event.name));
        }
    }

    /// The discs they had thrown go into their saved inventory as they leave.
    fn player_logged_out(&self, host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        self.recall(host, Some(event.player), true);
        let mut state = self.shared.state();
        state.last_throw.remove(&event.player);
        state.returns.remove(&event.player);
    }

    /// A player who dies with discs in the air: they drop with the rest of their items, or stay with
    /// them under keepInventory.
    fn living_death(&self, host: &mut dyn Host, event: &mut LivingDeathEvent<'_>) -> Flow {
        if event.kind != "minecraft:player" {
            return Flow::Continue;
        }
        let Some(player) = host
            .server_players()
            .into_iter()
            .find(|p| u32::try_from(p.entity_id).is_ok_and(|id| id == event.entity))
        else {
            return Flow::Continue;
        };
        let mine: Vec<Disc> = {
            let mut state = self.shared.state();
            let (mine, rest) = std::mem::take(&mut state.flying)
                .into_iter()
                .partition(|d| d.owner == player.uuid);
            state.flying = rest;
            mine
        };
        for mut disc in mine {
            self.owner_gone(host, &mut disc, Some(&player));
        }
        Flow::Continue
    }

    /// The discs a keepInventory death took out of the air come back.
    fn player_respawned(&self, host: &mut dyn Host, event: &mut PlayerRespawnedEvent<'_>) {
        let discs: Vec<ItemStack> = self
            .shared
            .state()
            .returns
            .remove(&event.player)
            .unwrap_or_default();
        for disc in discs {
            if let Some(left) = host.give_stack(event.player, disc)
                && let Some(p) = host
                    .server_players()
                    .into_iter()
                    .find(|p| p.uuid == event.player)
            {
                flight::drop_item(host, &p.dimension, p.position, &left);
            }
        }
    }

    fn server_stopping(&self, host: &mut dyn Host, _event: &mut ServerStoppingEvent) {
        self.recall(host, None, true);
    }

    /// An anvil result for books and a second disc of the same tier.
    fn anvil_update(&self, _host: &mut dyn Host, event: &mut AnvilUpdateEvent<'_>) -> Flow {
        match anvil::combine(event.left, event.right, event.item_name) {
            anvil::Outcome::Vanilla => Flow::Continue,
            anvil::Outcome::Nothing => {
                *event.output = None;
                event.xp_cost = 0;
                event.material_cost = 0;
                Flow::Cancel
            }
            // A result is set, not cancelled: cancelling leaves the anvil with no result at all.
            anvil::Outcome::Result(stack, cost) => {
                *event.output = Some(*stack);
                event.xp_cost = cost;
                event.material_cost = 1;
                Flow::Continue
            }
        }
    }

    /// The smithing recipe's base slot takes any echo shard: only a real diamond disc may produce a
    /// netherite one, which keeps its enchantments, name and wear.
    fn prepare_smithing_v2(&self, _host: &mut dyn Host, event: &mut PrepareSmithingV2Event<'_>) {
        let Some(made) = event.result.as_ref() else {
            return;
        };
        if item::tier_of(made) != Some(item::Tier::Netherite) {
            return;
        }
        let fresh = made.clone();
        *event.result = smith::upgrade(event.template, event.base, event.addition, fresh);
    }

    /// A disc is an echo shard underneath, so it would otherwise work in the recovery compass recipe.
    fn prepare_item_craft_v2(&self, _host: &mut dyn Host, event: &mut PrepareItemCraftV2Event<'_>) {
        if event.grid.iter().flatten().any(item::is_disc) {
            *event.result = None;
        }
    }
}

impl ThrowingDiscs {
    /// `/discs`, for a player or the console. Whether it was one.
    fn discs_command(&self, host: &mut dyn Host, line: &str) -> Flow {
        let cfg = self.shared.config();
        match commands::run(host, &cfg, line) {
            commands::Done::NotOurs => Flow::Continue,
            commands::Done::Ran => Flow::Cancel,
            commands::Done::Reload => {
                let before = self.shared.config();
                match self.reload_config(&*host) {
                    Ok(now) => {
                        host.feedback(rift_chat::mini(
                            "<white>Reloaded the <accent>Throwing Discs</accent> config!</white>",
                        ));
                        if before.crafting != now.crafting || before.tiers != now.tiers {
                            host.feedback(rift_chat::mini(
                                "<gray>Recipes change on <accent>/reload</accent>; discs that exist keep their old durability and lore</gray>",
                            ));
                        }
                    }
                    Err(e) => {
                        host.log(
                            "WARN",
                            &format!("ThrowingDiscs: could not read the config: {e}"),
                        );
                        host.feedback(rift_chat::mini(&format!(
                            "<red>Could not read the config, keeping the old one: {}</red>",
                            rift_chat::escape(&e)
                        )));
                    }
                }
                Flow::Cancel
            }
        }
    }
}
