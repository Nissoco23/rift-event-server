//! The Sculkstorm Scythe, a native mod for Rift Event Server: a port of the Paper plugin of the
//! same name. A netherite hoe made of vanilla components, so stock clients can use it:
//! right-click for Sculk Cataclysm (a sonic boom that ends in a storm crater, put back after a
//! while), sneak + right-click for Soulstorm Ascension (a shockwave and a launch).
//!
//! The engine runs mods as Rust code linked into its executable, so this crate is a library
//! (`SculkStorm`, a `Hooks`) and `crates/rift-engine` is the engine's `main` with it, and every
//! other mod in `rift-mods/`, handed over (docs/rift-mods.md).
#![forbid(unsafe_code)]

mod ascension;
mod cataclysm;
mod commands;
mod config;
mod crater;
mod fx;
mod item;
mod rng;
mod state;
mod tasks;
#[cfg(test)]
mod testing;

use ascension::Charge;
use cataclysm::Flight;
use config::Config;
use fx::Fx;
use horizon_client::mod_api::{
    BlockChangedEvent, DestroyBlockEvent, EntityJoinLevelEvent, Flow, Hand, Hooks, Host,
    LevelDimension, LevelHost, LevelTickPostEvent, LevelTickPreEvent, LivingFallEvent,
    LootDropsEvent, NativeServerPlayer, PerformCommandEvent, PerformConsoleCommandEvent,
    PlayerLoggedInEvent, PlayerLoggedOutEvent, RegisterCommandsEvent, RightClickBlockV2Event,
    ServerStoppingEvent, ServerTickEvent, ServerTickPhase, UseItemEvent, WorldLifecycleEvent,
    WorldPhase,
};
use rng::Rng;
use state::{Ability, Shared};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tasks::{Cx, Task};

/// The permission that lets a player use the scythe, which everyone has unless a permissions mod
/// says otherwise.
const USE: &str = "sculkstorm.use";

/// The id the resource pack is sent under, so a client replaces it rather than stacking a second.
const PACK_ID: u128 = 0x5c1c_5702_1ce5_4e0a_9b3e_2f7a_6d4c_1a01;

/// The mod: what the engine's executable hands to `horizon_client::entry::main_with`.
pub struct SculkStorm {
    shared: Arc<Shared>,
    config_loaded: AtomicBool,
}

impl Default for SculkStorm {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared::new()),
            config_loaded: AtomicBool::new(false),
        }
    }
}

/// The name the engine's executable gives the mod (`horizon_client::mod_api::name_mods`), which a
/// crash report and a log line name its hooks by.
pub const ID: &str = "sculkstorm";

/// The mod as the engine's executable takes it: the `Hooks` to hand to
/// `horizon_client::entry::main_with`.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(SculkStorm::default())
}

fn seconds(count: i64) -> Duration {
    Duration::from_secs(u64::try_from(count).unwrap_or(0))
}

/// The id of a dimension, as the level names are written.
fn dimension_id(dimension: LevelDimension) -> &'static str {
    match dimension {
        LevelDimension::Overworld => "minecraft:overworld",
        LevelDimension::Nether => "minecraft:the_nether",
        LevelDimension::End => "minecraft:the_end",
    }
}

/// A line of markup (`rift_chat`) over the player's hotbar.
fn notify(host: &mut dyn Host, player: u128, markup: &str) {
    host.action_bar(player, rift_chat::mini(markup));
}

fn recharging(host: &mut dyn Host, player: u128, ability: &str, left: Duration) {
    notify(
        host,
        player,
        &format!(
            "<gray>{ability} recharging: <accent>{:.1}s</accent></gray>",
            left.as_secs_f64()
        ),
    );
}

impl SculkStorm {
    pub(crate) fn shared(&self) -> &Shared {
        &self.shared
    }

    /// The state, for a test that has boxed the mod and still wants to look inside it.
    #[cfg(test)]
    pub(crate) fn handle(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    /// Reads `config/sculkstorm.toml` (writing it when the server has none) and takes it up. An
    /// error leaves the settings as they were.
    pub(crate) fn reload_config(&self, host: &dyn Host) -> Result<(), String> {
        let dir = host.config_dir()?;
        self.shared.set_config(config::load(&dir)?);
        Ok(())
    }

    /// Reads the config the first time a hook needs it.
    fn ensure_config(&self, host: &dyn Host) {
        if self.config_loaded.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Err(e) = self.reload_config(host) {
            host.log(
                "warn",
                &format!("Sculkstorm: could not read the config, using the defaults: {e}"),
            );
        }
    }

    /// A right click, on a block or in the air, by a player whose main hand holds the scythe. The
    /// click is the scythe's whatever follows (it is a hoe underneath, and must not till dirt).
    fn click(&self, host: &mut dyn Host, p: &NativeServerPlayer) -> Flow {
        self.ensure_config(&*host);
        let cfg = self.shared.config();
        if !host.has_permission(p.uuid, USE, 0) {
            return Flow::Cancel;
        }
        // A click on a block reaches the game as two (the block's, then the item's): count it once.
        if self
            .shared
            .state()
            .repeated_click(p.uuid, self.shared.tick())
        {
            return Flow::Cancel;
        }
        let world = host.world_of_dimension(&p.dimension);
        if cfg.is_disabled(&p.dimension, world.as_deref()) {
            notify(
                host,
                p.uuid,
                "<red>The storm cannot reach this world.</red>",
            );
            return Flow::Cancel;
        }
        if p.sneaking {
            self.ascend(host, &cfg, p);
        } else {
            self.cataclysm(host, &cfg, p);
        }
        Flow::Cancel
    }

    /// Sneak + right-click: Soulstorm Ascension.
    fn ascend(&self, host: &mut dyn Host, cfg: &Config, p: &NativeServerPlayer) {
        let now = Instant::now();
        {
            let mut state = self.shared.state();
            if state.is_charging(p.uuid) {
                return;
            }
            if let Some(left) = state.cooldown_left(p.uuid, Ability::Ascension, now) {
                drop(state);
                recharging(host, p.uuid, "Soulstorm Ascension", left);
                return;
            }
            state.start_cooldown(
                p.uuid,
                Ability::Ascension,
                now + seconds(cfg.ascension.cooldown_seconds),
            );
            state.set_charging(p.uuid, true);
        }
        let charge = Charge::start(host, cfg, p);
        self.shared.state().tasks.push(Task::Charge(charge));
    }

    /// Right-click: Sculk Cataclysm.
    fn cataclysm(&self, host: &mut dyn Host, cfg: &Config, p: &NativeServerPlayer) {
        let now = Instant::now();
        {
            let mut state = self.shared.state();
            if let Some(left) = state.cooldown_left(p.uuid, Ability::Cataclysm, now) {
                drop(state);
                recharging(host, p.uuid, "Sculk Cataclysm", left);
                return;
            }
            let most = usize::try_from(cfg.cataclysm.max_active).unwrap_or(1);
            if !state.can_start_cataclysm(most) {
                drop(state);
                notify(
                    host,
                    p.uuid,
                    "<gray>The deep dark is already stirring... try again in a moment.</gray>",
                );
                return;
            }
            state.start_cooldown(
                p.uuid,
                Ability::Cataclysm,
                now + seconds(cfg.cataclysm.cooldown_seconds),
            );
            // Counted from the cast, or spamming would get past the limit while a boom flies.
            state.cataclysm_started();
        }
        let flight = Flight::cast(host, cfg, p);
        self.shared.state().tasks.push(Task::Flight(flight));
    }

    /// The acting player, if their main hand holds a scythe.
    fn scythe_in_hand(&self, host: &dyn Host) -> Option<NativeServerPlayer> {
        let player = host.acting_player()?;
        let held = host.held_stack(player.uuid)?;
        item::is_scythe(&held).then_some(player)
    }

    /// Runs what is going on in `dimension`: the tasks that belong to it, then the crater work.
    fn run_level(&self, host: &mut dyn Host, dimension: &str) {
        let cfg = self.shared.config();
        let tick = self.shared.tick();
        let first = self.shared.state().swept.insert(dimension.to_owned());
        if first {
            // Debris a stop left in the world.
            crater::sweep_debris(host, dimension);
        }
        let mine: Vec<Task> = {
            let mut state = self.shared.state();
            let (mine, rest): (Vec<Task>, Vec<Task>) = std::mem::take(&mut state.tasks)
                .into_iter()
                .partition(|t| t.dimension() == dimension);
            state.tasks = rest;
            mine
        };
        let mut rng = Rng::from_entropy();
        let mut spawned = Vec::new();
        let mut keep = Vec::with_capacity(mine.len());
        for mut task in mine {
            let mut cx = Cx {
                host: &mut *host,
                cfg: &cfg,
                fx: Fx::new(cfg.particle_multiplier),
                tick,
                rng: &mut rng,
                shared: &self.shared,
                spawn: &mut spawned,
            };
            if task.step(&mut cx) {
                keep.push(task);
            }
        }
        {
            let mut cx = Cx {
                host: &mut *host,
                cfg: &cfg,
                fx: Fx::new(cfg.particle_multiplier),
                tick,
                rng: &mut rng,
                shared: &self.shared,
                spawn: &mut spawned,
            };
            crater::tick(&mut cx, dimension);
        }
        let mut state = self.shared.state();
        state.tasks.extend(keep);
        state.tasks.extend(spawned);
    }
}

impl Hooks for SculkStorm {
    fn server_tick(&self, _host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        // Only ticks that run count: a paused or frozen world does not move the mod's clock.
        if let ServerTickPhase::ServerTickStart {
            world_running: true,
        } = event.phase
        {
            self.shared.advance();
        }
    }

    fn level_tick_pre(&self, host: &mut dyn Host, event: &mut LevelTickPreEvent<'_>) {
        self.ensure_config(&*host);
        self.run_level(host, event.dimension);
    }

    fn level_tick_post(&self, host: &mut dyn Host, event: &mut LevelTickPostEvent<'_>) {
        if self.shared.watching() {
            crater::resolve_candidates(host, &self.shared, event.dimension);
        }
    }

    fn world_lifecycle(&self, host: &mut dyn Host, event: &mut WorldLifecycleEvent<'_>) {
        if event.phase == WorldPhase::WorldOpenReady {
            self.ensure_config(&*host);
            crater::recover(host, &self.shared, self.shared.tick());
        }
    }

    fn server_stopping(&self, host: &mut dyn Host, _event: &mut ServerStoppingEvent) {
        crater::restore_all_now(host, &self.shared);
    }

    fn use_item(&self, host: &mut dyn Host, event: &mut UseItemEvent<'_>) -> Flow {
        if !item::is_scythe(event.stack) {
            return Flow::Continue;
        }
        // Only the main hand: the plugin leaves the off hand's click alone.
        match self.scythe_in_hand(&*host) {
            Some(player) => self.click(host, &player),
            None => Flow::Continue,
        }
    }

    fn right_click_block_v2(
        &self,
        host: &mut dyn Host,
        event: &mut RightClickBlockV2Event,
    ) -> Flow {
        if event.hand != Hand::Main {
            return Flow::Continue;
        }
        match self.scythe_in_hand(&*host) {
            Some(player) => self.click(host, &player),
            None => Flow::Continue,
        }
    }

    fn living_fall(&self, event: &mut LivingFallEvent<'_>) -> Flow {
        // A fall of 3 blocks or less hurts no one, so it does not spend the protection.
        if event.kind != "minecraft:player" || event.distance <= 3.0 || event.multiplier <= 0.0 {
            return Flow::Continue;
        }
        let protected = self
            .shared
            .state()
            .consume_fall_protection(event.entity, Instant::now());
        if protected {
            event.multiplier = 0.0;
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn register_commands(&self, _host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        commands::register(event.commands);
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        self.ensure_config(&*host);
        if commands::run(self, host, Some(event.player), event.command) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn perform_console_command(
        &self,
        host: &mut dyn Host,
        event: &mut PerformConsoleCommandEvent<'_>,
    ) -> Flow {
        self.ensure_config(&*host);
        if commands::run(self, host, None, event.command) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        self.ensure_config(&*host);
        let cfg = self.shared.config();
        let pack = &cfg.resource_pack;
        if !pack.enabled || pack.url.trim().is_empty() {
            return;
        }
        host.send_resource_pack(
            event.player,
            PACK_ID,
            pack.url.trim(),
            cfg.pack_sha1().unwrap_or(""),
            None,
            pack.required,
        );
    }

    fn player_logged_out(&self, _host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        self.shared.state().forget(event.player, Instant::now());
    }

    /// A crater block a player breaks drops nothing: otherwise the catalysts would be farmed
    /// before the crater is put back.
    fn destroy_block(&self, host: &mut dyn Host, event: &mut DestroyBlockEvent) -> Flow {
        if self.shared.watching() {
            let dimension = host.dimension();
            self.shared
                .state()
                .craters
                .broke(dimension, event.pos, self.shared.tick());
        }
        Flow::Continue
    }

    fn loot_drops(&self, _host: &mut dyn Host, event: &mut LootDropsEvent<'_>) {
        if !self.shared.watching() || event.context.used {
            return;
        }
        if let Some((pos, _)) = event.context.block
            && self
                .shared
                .state()
                .craters
                .take_no_drops(event.context.dimension, pos)
        {
            event.drops.clear();
        }
    }

    /// The experience of a crater block that was just broken is not given either.
    fn entity_join_level(
        &self,
        _host: &mut dyn Host,
        event: &mut EntityJoinLevelEvent<'_>,
    ) -> Flow {
        if event.kind != "minecraft:experience_orb" || !self.shared.watching() {
            return Flow::Continue;
        }
        let pos = event.pos.map(|c| c.floor() as i32);
        let from_crater =
            self.shared
                .state()
                .craters
                .is_crater_xp(event.dimension, pos, self.shared.tick());
        if from_crater {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    /// Every block change in the world is heard here, so this is the cheapest check there is until
    /// a crater stands.
    fn block_changed(&self, _level: &dyn LevelHost, event: &mut BlockChangedEvent) {
        if !self.shared.watching() || self.shared.reverting() {
            return;
        }
        self.shared.state().craters.candidate(
            dimension_id(event.dimension),
            [event.pos.x, event.pos.y, event.pos.z],
            event.old,
        );
    }
}
