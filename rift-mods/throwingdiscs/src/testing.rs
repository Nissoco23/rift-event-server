//! A server in memory, and the mod's real hooks driven over it.
//!
//! `FakeHost` is the part of the engine's `Host` the mod uses: players with inventories, entities with
//! boxes, a few solid blocks to throw a disc at, and a record of what the mod did. `Rig` boxes the mod as
//! the engine does and runs its hooks through the engine's own `dispatch_*` functions, so what the
//! tests exercise is what the server runs.

use crate::ThrowingDiscs;
use crate::config::Config;
use crate::item::{self, Tier};
use crate::state::Shared;
use crate::vec3::*;
use horizon_client::mod_api::{
    BlockStateId, Compound, EntityRef, EquipmentSlot, GameRuleValue, Hooks, Host, ItemStack,
    LevelRef, LivingRef, NativeBlockReadError, NativeHit, NativeInventoryKind,
    NativeLifecycleContext, NativeServerPlayer, NativeWorldEpoch, ParticleOptions, ServerTickPhase,
    Tag, Text, dispatch_anvil_update, dispatch_data_pack_files, dispatch_level_tick_post,
    dispatch_living_death, dispatch_perform_command, dispatch_perform_console_command,
    dispatch_player_logged_in, dispatch_player_logged_out, dispatch_player_respawned,
    dispatch_prepare_item_craft_v2, dispatch_prepare_smithing_v2, dispatch_server_stopping,
    dispatch_server_tick, dispatch_use_item, parse_snbt,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const OVERWORLD: &str = "minecraft:overworld";
pub const BOB: u128 = 0xb0b;
pub const ALICE: u128 = 0xa11ce;

/// A mob (or armor stand) in the world.
#[derive(Clone, Debug)]
pub struct Mob {
    pub id: u32,
    pub kind: String,
    pub pos: V3,
    /// Width and height of its box.
    pub size: [f64; 2],
    pub living: bool,
    pub health: f32,
    pub velocity: V3,
    /// While set, it takes no hurt: a hurt cooldown, as vanilla's.
    pub immune: bool,
}

pub struct FakeHost {
    pub players: Vec<NativeServerPlayer>,
    /// A player's 41 slots: the hotbar, the rest, the armour, the off hand last.
    pub inventories: HashMap<u128, Vec<Option<ItemStack>>>,
    pub mobs: Vec<Mob>,
    /// Solid unit cubes, by block position, with the block's id.
    pub blocks: HashMap<[i32; 3], String>,
    pub unloaded: HashSet<(i32, i32)>,
    pub keep_inventory: bool,
    pub acting: Option<u128>,
    pub permitted: bool,
    /// Every hurt, in order: target, amount, damage type, attacker.
    pub hurts: Vec<(u32, f32, String, Option<u32>)>,
    pub player_velocities: Vec<(u128, V3)>,
    /// What was summoned: its id, kind and position, and the compound it was given.
    pub summoned: Vec<(u32, String, V3, Compound)>,
    pub removed: Vec<u32>,
    /// Where the last `move_entity` put each entity: position and yaw.
    pub moved: HashMap<u32, (V3, f32)>,
    pub sounds: Vec<String>,
    pub particles: usize,
    pub commands: Vec<String>,
    pub block_properties: Vec<([i32; 3], String, String)>,
    pub feedback: Vec<String>,
    pub told: Vec<(u128, String)>,
    next_entity: u32,
    logs: RefCell<Vec<String>>,
    dir: PathBuf,
}

impl FakeHost {
    pub fn player(&self, uuid: u128) -> &NativeServerPlayer {
        self.players.iter().find(|p| p.uuid == uuid).unwrap()
    }

    pub fn player_mut(&mut self, uuid: u128) -> &mut NativeServerPlayer {
        self.players.iter_mut().find(|p| p.uuid == uuid).unwrap()
    }

    pub fn slot(&self, uuid: u128, slot: usize) -> Option<&ItemStack> {
        self.inventories.get(&uuid)?.get(slot)?.as_ref()
    }

    pub fn set_slot(&mut self, uuid: u128, slot: usize, stack: Option<ItemStack>) {
        self.inventories
            .entry(uuid)
            .or_insert_with(|| vec![None; 41])[slot] = stack;
    }

    /// Everything the player carries.
    pub fn carried(&self, uuid: u128) -> Vec<ItemStack> {
        self.inventories
            .get(&uuid)
            .map(|slots| slots.iter().flatten().cloned().collect())
            .unwrap_or_default()
    }

    pub fn add_mob(&mut self, kind: &str, pos: V3) -> u32 {
        self.next_entity += 1;
        let id = self.next_entity;
        self.mobs.push(Mob {
            id,
            kind: kind.to_owned(),
            pos,
            size: [0.6, 1.8],
            living: kind != "minecraft:item_frame",
            health: 20.0,
            velocity: [0.0; 3],
            immune: false,
        });
        id
    }

    pub fn mob(&self, id: u32) -> &Mob {
        self.mobs.iter().find(|m| m.id == id).unwrap()
    }

    /// The display the mod summoned for the flight, if one is still there.
    pub fn display(&self) -> Option<u32> {
        self.summoned
            .iter()
            .find(|(id, kind, ..)| kind == "minecraft:item_display" && !self.removed.contains(id))
            .map(|(id, ..)| *id)
    }

    pub fn item_entities(&self) -> Vec<&(u32, String, V3, Compound)> {
        self.summoned
            .iter()
            .filter(|(_, kind, ..)| kind == "minecraft:item")
            .collect()
    }

    fn box_of(&self, id: u32) -> Option<[V3; 2]> {
        let (pos, size) = if let Some(m) = self.mobs.iter().find(|m| m.id == id) {
            (m.pos, m.size)
        } else {
            let p = self
                .players
                .iter()
                .find(|p| u32::try_from(p.entity_id).is_ok_and(|e| e == id))?;
            (p.position, [0.6, 1.8])
        };
        let half = size[0] / 2.0;
        Some([
            [pos[0] - half, pos[1], pos[2] - half],
            [pos[0] + half, pos[1] + size[1], pos[2] + half],
        ])
    }

    fn position_of(&self, id: u32) -> Option<V3> {
        self.mobs
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.pos)
            .or_else(|| {
                self.players
                    .iter()
                    .find(|p| u32::try_from(p.entity_id).is_ok_and(|e| e == id))
                    .map(|p| p.position)
            })
    }
}

fn tag_members(tag: &str) -> &'static [&'static str] {
    match tag {
        "minecraft:leaves" => &["minecraft:oak_leaves"],
        "minecraft:wool" => &["minecraft:white_wool"],
        _ => &[],
    }
}

impl Host for FakeHost {
    fn server_players(&self) -> Vec<NativeServerPlayer> {
        self.players.clone()
    }

    fn acting_player(&self) -> Option<NativeServerPlayer> {
        self.acting
            .and_then(|u| self.players.iter().find(|p| p.uuid == u).cloned())
    }

    fn held_stack(&self, uuid: u128) -> Option<ItemStack> {
        let selected = usize::from(self.players.iter().find(|p| p.uuid == uuid)?.selected_slot);
        self.slot(uuid, selected).cloned()
    }

    fn player_equipment(&self, uuid: u128, slot: EquipmentSlot) -> Option<ItemStack> {
        match slot {
            EquipmentSlot::MainHand => self.held_stack(uuid),
            EquipmentSlot::OffHand => self.slot(uuid, 40).cloned(),
            _ => None,
        }
    }

    fn set_inventory_slot(
        &mut self,
        uuid: u128,
        kind: NativeInventoryKind,
        slot: usize,
        expected: Option<&ItemStack>,
        new: Option<ItemStack>,
    ) -> Result<Option<ItemStack>, String> {
        assert_eq!(kind, NativeInventoryKind::Player);
        if slot >= 41 {
            return Err("no such slot".into());
        }
        let slots = self
            .inventories
            .entry(uuid)
            .or_insert_with(|| vec![None; 41]);
        if slots[slot].as_ref() != expected {
            return Err("the slot changed".into());
        }
        Ok(std::mem::replace(&mut slots[slot], new))
    }

    fn give_stack(&mut self, uuid: u128, stack: ItemStack) -> Option<ItemStack> {
        if !self.players.iter().any(|p| p.uuid == uuid) {
            return Some(stack);
        }
        let slots = self
            .inventories
            .entry(uuid)
            .or_insert_with(|| vec![None; 41]);
        match slots[..36].iter_mut().find(|s| s.is_none()) {
            Some(free) => {
                *free = Some(stack);
                None
            }
            None => Some(stack),
        }
    }

    fn has_permission(&self, _player: u128, _node: &str, _default_level: u8) -> bool {
        self.permitted
    }

    fn summon(
        &mut self,
        _dimension: &str,
        kind: &str,
        pos: [f64; 3],
        nbt: Compound,
    ) -> Option<u32> {
        self.next_entity += 1;
        self.summoned
            .push((self.next_entity, kind.to_owned(), pos, nbt));
        self.moved.insert(self.next_entity, (pos, 0.0));
        Some(self.next_entity)
    }

    fn move_entity(&mut self, _d: &str, id: u32, pos: [f64; 3], yaw: f32, _pitch: f32) -> bool {
        self.moved.insert(id, (pos, yaw));
        self.summoned.iter().any(|(i, ..)| *i == id) && !self.removed.contains(&id)
    }

    fn remove_entity(&mut self, _dimension: &str, id: u32) -> bool {
        self.removed.push(id);
        true
    }

    fn entities_in(&mut self, _d: &str, min: [f64; 3], max: [f64; 3]) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .mobs
            .iter()
            .filter(|m| inside(m.pos, min, max))
            .map(|m| m.id)
            .collect();
        ids.extend(
            self.players
                .iter()
                .filter(|p| inside(p.position, min, max))
                .filter_map(|p| u32::try_from(p.entity_id).ok()),
        );
        ids
    }

    fn entity(&self, id: u32) -> Option<EntityRef> {
        self.position_of(id)
            .map(|_| EntityRef::new(NativeWorldEpoch::new(1), id))
    }

    fn living(&self, entity: EntityRef) -> Option<LivingRef> {
        let id = entity.id();
        let living = self
            .mobs
            .iter()
            .find(|m| m.id == id)
            .is_none_or(|m| m.living);
        living.then(|| LivingRef::new(entity))
    }

    fn entity_position(&self, entity: EntityRef) -> Option<[f64; 3]> {
        self.position_of(entity.id())
    }

    fn entity_bounding_box(&self, entity: EntityRef) -> Option<[[f64; 3]; 2]> {
        self.box_of(entity.id())
    }

    fn entity_velocity(&self, entity: EntityRef) -> Option<[f64; 3]> {
        self.mobs
            .iter()
            .find(|m| m.id == entity.id())
            .map(|m| m.velocity)
    }

    fn entity_set_velocity(&mut self, entity: EntityRef, velocity: [f64; 3]) -> bool {
        match self.mobs.iter_mut().find(|m| m.id == entity.id()) {
            Some(m) => {
                m.velocity = velocity;
                true
            }
            None => false,
        }
    }

    fn entity_kind(&self, entity: EntityRef) -> Option<String> {
        let id = entity.id();
        self.mobs
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.kind.clone())
            .or_else(|| {
                self.players
                    .iter()
                    .any(|p| u32::try_from(p.entity_id).is_ok_and(|e| e == id))
                    .then(|| "minecraft:player".to_owned())
            })
    }

    fn entity_nbt(&mut self, _entity: EntityRef) -> Option<Compound> {
        None
    }

    fn hurt_entity(
        &mut self,
        _dimension: &str,
        target: u32,
        amount: f32,
        damage_type: &str,
        attacker: Option<u32>,
    ) -> bool {
        if let Some(m) = self.mobs.iter_mut().find(|m| m.id == target) {
            if m.immune || m.health <= 0.0 {
                return false;
            }
            m.health -= amount;
            self.hurts
                .push((target, amount, damage_type.to_owned(), attacker));
            return true;
        }
        if let Some(p) = self
            .players
            .iter_mut()
            .find(|p| u32::try_from(p.entity_id).is_ok_and(|e| e == target))
        {
            // 1 is creative: nothing hurts them.
            if p.game_mode == 1 || p.health <= 0.0 {
                return false;
            }
            p.health -= amount;
            self.hurts
                .push((target, amount, damage_type.to_owned(), attacker));
            return true;
        }
        false
    }

    fn set_player_velocity(&mut self, uuid: u128, velocity: [f64; 3]) -> bool {
        self.player_velocities.push((uuid, velocity));
        true
    }

    fn attribute(&mut self, _uuid: u128, _attribute: &str) -> Option<(f64, f64)> {
        None
    }

    fn pick_block(
        &self,
        eye: [f64; 3],
        dir: [f64; 3],
        range: f64,
        _fluids: bool,
    ) -> Option<NativeHit> {
        let dir = normalize(dir);
        if dir == [0.0; 3] {
            return None;
        }
        let mut pos = eye.map(|v| v.floor() as i32);
        let step = dir.map(|d| if d > 0.0 { 1 } else { -1 });
        let mut t_max = [f64::INFINITY; 3];
        let delta = dir.map(|d| {
            if d == 0.0 {
                f64::INFINITY
            } else {
                (1.0 / d).abs()
            }
        });
        for i in 0..3 {
            if dir[i] != 0.0 {
                let boundary = if dir[i] > 0.0 {
                    f64::from(pos[i]) + 1.0
                } else {
                    f64::from(pos[i])
                };
                t_max[i] = (boundary - eye[i]) / dir[i];
            }
        }
        loop {
            let axis = (0..3)
                .min_by(|a, b| t_max[*a].total_cmp(&t_max[*b]))
                .unwrap();
            let t = t_max[axis];
            if t > range {
                return None;
            }
            pos[axis] += step[axis];
            t_max[axis] += delta[axis];
            let mut face = [0; 3];
            face[axis] = -step[axis];
            if let Some(block) = self.blocks.get(&pos) {
                let point = add(eye, scale(dir, t));
                return Some(NativeHit {
                    block_x: pos[0],
                    block_y: pos[1],
                    block_z: pos[2],
                    x: point[0],
                    y: point[1],
                    z: point[2],
                    block: block.clone(),
                    face,
                });
            }
        }
    }

    fn block_name_in_tag(&self, block: &str, tag: &str) -> Result<bool, NativeBlockReadError> {
        Ok(tag_members(tag).contains(&block))
    }

    fn set_block_property(&mut self, pos: [i32; 3], key: &str, value: &str) -> bool {
        self.block_properties
            .push((pos, key.to_owned(), value.to_owned()));
        true
    }

    fn run_command(&mut self, line: &str) -> bool {
        self.commands.push(line.to_owned());
        // A pot broken by `setblock ... destroy` is gone.
        if let Some(rest) = line.split("run setblock ").nth(1) {
            let n: Vec<i32> = rest
                .split_whitespace()
                .take(3)
                .filter_map(|w| w.parse().ok())
                .collect();
            if n.len() == 3 {
                self.blocks.remove(&[n[0], n[1], n[2]]);
            }
        }
        true
    }

    fn play_sound(
        &mut self,
        sound: &str,
        _at: [f64; 3],
        _source: usize,
        _volume: f32,
        _pitch: f32,
    ) -> bool {
        self.sounds.push(sound.to_owned());
        true
    }

    fn play_sound_to(
        &mut self,
        _uuid: u128,
        sound: &str,
        _category: &str,
        _at: Option<[f64; 3]>,
        _volume: f32,
        _pitch: f32,
    ) -> bool {
        self.sounds.push(format!("to:{sound}"));
        true
    }

    fn send_particles(
        &mut self,
        _options: &ParticleOptions,
        _state: Option<BlockStateId>,
        _at: [f64; 3],
        _spread: [f64; 3],
        _speed: f64,
        count: i32,
    ) -> bool {
        self.particles += count as usize;
        true
    }

    fn chunk_is_loaded(&self, _dimension: &str, chunk: [i32; 2]) -> bool {
        !self.unloaded.contains(&(chunk[0], chunk[1]))
    }

    fn level(&self, dimension: &str) -> Option<LevelRef> {
        (dimension == OVERWORLD).then(|| LevelRef::new(NativeWorldEpoch::new(1), OVERWORLD))
    }

    fn level_game_rule(&self, _level: LevelRef, rule: &str) -> Option<GameRuleValue> {
        (rule == "keep_inventory").then_some(GameRuleValue::Bool(self.keep_inventory))
    }

    fn item_from_snbt(&self, snbt: &str) -> Option<ItemStack> {
        let (Tag::Compound(compound), _) = parse_snbt(snbt).ok()? else {
            return None;
        };
        ItemStack::from_nbt(&compound)
    }

    fn feedback(&mut self, line: Text) {
        self.feedback.push(line.plain());
    }

    fn tell(&mut self, uuid: u128, line: Text) -> bool {
        self.told.push((uuid, line.plain()));
        true
    }

    fn config_dir(&self) -> Result<PathBuf, String> {
        Ok(self.dir.join("config"))
    }

    fn log(&self, level: &str, line: &str) {
        self.logs.borrow_mut().push(format!("{level}: {line}"));
    }
}

/// The mod boxed as the engine holds it, over a `FakeHost`.
pub struct Rig {
    pub host: FakeHost,
    pub mods: Vec<Box<dyn Hooks>>,
    pub shared: Arc<Shared>,
    tick: u64,
    dir: PathBuf,
}

impl Rig {
    pub fn new() -> Self {
        Self::with_config(None)
    }

    /// A rig whose `config/throwingdiscs.toml` holds `config`.
    pub fn with_config(config: Option<&str>) -> Self {
        static RIGS: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "throwingdiscs-rig-{}-{}",
            std::process::id(),
            RIGS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        if let Some(config) = config {
            std::fs::write(dir.join("config").join(crate::config::FILE_NAME), config).unwrap();
        }
        let module = ThrowingDiscs::with(crate::rng::Rng::new(7));
        let shared = module.handle();
        let mut rig = Self {
            host: FakeHost {
                players: Vec::new(),
                inventories: HashMap::new(),
                mobs: Vec::new(),
                blocks: HashMap::new(),
                unloaded: HashSet::new(),
                keep_inventory: false,
                acting: None,
                permitted: true,
                hurts: Vec::new(),
                player_velocities: Vec::new(),
                summoned: Vec::new(),
                removed: Vec::new(),
                moved: HashMap::new(),
                sounds: Vec::new(),
                particles: 0,
                commands: Vec::new(),
                block_properties: Vec::new(),
                feedback: Vec::new(),
                told: Vec::new(),
                next_entity: 500,
                logs: RefCell::new(Vec::new()),
                dir: dir.clone(),
            },
            mods: vec![Box::new(module)],
            shared,
            tick: 0,
            dir,
        };
        // Bob stands at the origin facing south (+z), with the first hotbar slot held; Alice is an
        // operator elsewhere.
        rig.join("Bob", BOB, 0, 1, [0.0, 64.0, 0.0]);
        rig.join("Alice", ALICE, 4, 2, [100.0, 64.0, 100.0]);
        rig
    }

    pub fn join(&mut self, name: &str, uuid: u128, permission: u8, entity_id: i32, at: V3) {
        let mut player = NativeServerPlayer::new(uuid, name, permission);
        player.entity_id = entity_id;
        player.position = at;
        player.eye_height = 1.62;
        self.host.players.push(player);
        self.host.inventories.insert(uuid, vec![None; 41]);
        dispatch_player_logged_in(&mut self.host, &self.mods, uuid, name);
    }

    pub fn leave(&mut self, uuid: u128) {
        let name = self.host.player(uuid).name.clone();
        self.host.players.retain(|p| p.uuid != uuid);
        dispatch_player_logged_out(&mut self.host, &self.mods, uuid, &name);
    }

    pub fn config(&self) -> Arc<Config> {
        self.shared.config()
    }

    /// A new disc of `tier`, as the mod makes it.
    pub fn disc(&self, tier: Tier) -> ItemStack {
        item::create(&self.host, tier, &self.config()).expect("the host reads the disc")
    }

    /// One server tick of a running world: its start, the overworld's level tick, its end.
    pub fn tick(&mut self) {
        let context = |tick| {
            NativeLifecycleContext::new(
                horizon_client::mod_api::NativeAuthority::DedicatedServer,
                NativeWorldEpoch::new(1),
                tick,
            )
        };
        dispatch_server_tick(
            &mut self.host,
            &self.mods,
            &context(self.tick),
            ServerTickPhase::ServerTickStart {
                world_running: true,
            },
        );
        dispatch_level_tick_post(&mut self.host, &self.mods, OVERWORLD);
        dispatch_server_tick(
            &mut self.host,
            &self.mods,
            &context(self.tick),
            ServerTickPhase::ServerTickEnd {
                world_running: true,
            },
        );
        self.tick += 1;
    }

    pub fn ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// The player right-clicks with the stack in their hands: whether the mod took the click.
    pub fn use_item(&mut self, uuid: u128, stack: &ItemStack) -> bool {
        self.host.acting = Some(uuid);
        let mut claimed = false;
        dispatch_use_item(&mut self.host, &self.mods, stack, &mut claimed);
        self.host.acting = None;
        claimed
    }

    /// Bob puts a disc in the hotbar slot he holds and clicks with it.
    pub fn throw_from_hand(&mut self, uuid: u128) -> bool {
        let held = self.host.held_stack(uuid).expect("a disc in the main hand");
        self.use_item(uuid, &held)
    }

    /// A player types a command: whether the mod took it.
    pub fn say(&mut self, uuid: u128, line: &str) -> bool {
        let player = self.host.player(uuid).clone();
        self.host.feedback.clear();
        let mut ran = false;
        dispatch_perform_command(&mut self.host, &self.mods, &player, line, &mut ran);
        ran
    }

    pub fn console(&mut self, line: &str) -> bool {
        self.host.feedback.clear();
        let mut ran = false;
        dispatch_perform_console_command(&mut self.host, &self.mods, line, &mut ran);
        ran
    }

    pub fn die(&mut self, uuid: u128) {
        let entity = self.host.player(uuid).entity_id as u32;
        self.host.player_mut(uuid).health = 0.0;
        let mut cancel = false;
        dispatch_living_death(
            &mut self.host,
            &self.mods,
            entity,
            "minecraft:player",
            "minecraft:mob_attack",
            0,
            &mut cancel,
        );
    }

    pub fn respawn(&mut self, uuid: u128) {
        let name = self.host.player(uuid).name.clone();
        self.host.player_mut(uuid).health = 20.0;
        dispatch_player_respawned(&mut self.host, &self.mods, uuid, &name);
    }

    pub fn stop_server(&mut self) {
        dispatch_server_stopping(&mut self.host, &self.mods, true);
    }

    pub fn data_pack_files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        dispatch_data_pack_files(&self.host, &self.mods, &mut files);
        files
    }

    /// What an anvil answers for `left` and `right`: the result and its level cost, or None for no
    /// result; `None` inside is vanilla's.
    pub fn anvil(
        &mut self,
        left: &ItemStack,
        right: Option<&ItemStack>,
        name: Option<&str>,
    ) -> Option<(Option<ItemStack>, i32)> {
        let mut output = None;
        let (mut xp, mut material, mut canceled) = (0, 0, false);
        dispatch_anvil_update(
            &mut self.host,
            &self.mods,
            BOB,
            "Bob",
            left,
            right,
            name,
            None,
            0,
            0,
            &mut output,
            &mut xp,
            &mut material,
            &mut canceled,
        );
        canceled.then_some((output, xp))
    }

    /// What the smithing table shows: the result vanilla made, as the mods leave it.
    pub fn smithing(
        &mut self,
        template: Option<&ItemStack>,
        base: Option<&ItemStack>,
        addition: Option<&ItemStack>,
        made: Option<ItemStack>,
    ) -> Option<ItemStack> {
        let mut result = made;
        dispatch_prepare_smithing_v2(
            &mut self.host,
            &self.mods,
            BOB,
            "Bob",
            [0, 64, 0],
            template,
            base,
            addition,
            &mut result,
        );
        result
    }

    pub fn crafting(
        &mut self,
        grid: &[Option<ItemStack>],
        made: Option<ItemStack>,
    ) -> Option<ItemStack> {
        let mut result = made;
        dispatch_prepare_item_craft_v2(
            &mut self.host,
            &self.mods,
            BOB,
            "Bob",
            Some([0, 64, 0]),
            grid,
            &mut result,
        );
        result
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod scenarios {
    use super::*;
    use std::sync::Arc as Shared;

    fn zombie_in_the_path(rig: &mut Rig, z: f64) -> u32 {
        // The disc leaves a little left of Bob's eye height, so a mob at x -0.25 is in its line.
        rig.host.add_mob("minecraft:zombie", [-0.25, 64.0, z])
    }

    /// Bob holds `disc` in the hotbar slot he has selected.
    fn hold(rig: &mut Rig, disc: ItemStack) {
        rig.host.set_slot(BOB, 0, Some(disc));
    }

    /// A rig with Bob holding a disc of `tier`, thrown and one tick on.
    fn thrown(tier: Tier) -> Rig {
        let mut rig = Rig::new();
        let disc = rig.disc(tier);
        hold(&mut rig, disc);
        assert!(rig.throw_from_hand(BOB));
        rig.tick();
        rig
    }

    fn display_at(rig: &Rig) -> V3 {
        rig.host.moved[&rig.host.display().expect("a disc in flight")].0
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1.0e-6
    }

    // ---- throwing --------------------------------------------------------------------------

    #[test]
    fn a_click_queues_the_throw_and_the_next_level_tick_sends_the_disc_along_the_view() {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        assert!(rig.throw_from_hand(BOB), "the mod takes the click");
        assert!(
            rig.host.slot(BOB, 0).is_some(),
            "nothing is done inside the click"
        );
        rig.tick();
        assert!(rig.host.slot(BOB, 0).is_none(), "the disc left the hand");
        // From the eye (y 65.62), 0.4 ahead, a quarter to the right and 0.15 down; then one tick of
        // 1.05 blocks along +z (Bob faces south).
        let [x, y, z] = display_at(&rig);
        assert!(
            near(x, -0.25) && near(y, 65.47) && near(z, 0.4 + 1.05),
            "{x} {y} {z}"
        );
        rig.ticks(3);
        assert!(
            near(display_at(&rig)[2], 0.4 + 4.0 * 1.05),
            "no gravity: a straight line"
        );
        // It spins as it flies.
        let yaw = rig.host.moved[&rig.host.display().unwrap()].1;
        assert!(near(f64::from(yaw), 160.0), "four ticks of 40 degrees");
        assert!(
            rig.host
                .sounds
                .contains(&"minecraft:item.trident.throw".to_owned())
        );
    }

    #[test]
    fn the_display_is_the_discs_model_lying_flat_and_is_tagged_so_strays_can_be_found() {
        let rig = thrown(Tier::Diamond);
        let (_, kind, _, nbt) = rig
            .host
            .summoned
            .iter()
            .find(|(_, k, ..)| k == "minecraft:item_display")
            .unwrap();
        assert_eq!(kind, "minecraft:item_display");
        let nbt = Tag::Compound(nbt.clone());
        let model = nbt
            .get("item")
            .and_then(|i| i.get("components"))
            .and_then(|c| c.get("minecraft:item_model"));
        assert!(
            matches!(model, Some(Tag::String(m)) if m.to_string_lossy() == "throwingdiscs:diamond_throwing_disc")
        );
        let Some(Tag::List(tags)) = nbt.get("Tags") else {
            panic!("tagged")
        };
        assert!(
            tags.iter().any(
                |t| matches!(t, Tag::String(s) if s.to_string_lossy() == "throwingdiscs_flight")
            )
        );
    }

    #[test]
    fn only_a_disc_is_taken_and_a_player_throws_one_disc_a_tick() {
        let mut rig = Rig::new();
        let stick = ItemStack::new("minecraft:stick", 1);
        assert!(!rig.use_item(BOB, &stick), "a stick is not the mod's");
        let echo = ItemStack::new("minecraft:echo_shard", 1);
        assert!(!rig.use_item(BOB, &echo), "nor a plain echo shard");
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc.clone());
        assert!(rig.use_item(BOB, &disc));
        assert!(
            !rig.use_item(BOB, &disc),
            "a second click in the same tick is left to vanilla"
        );
    }

    #[test]
    fn with_a_disc_in_each_hand_the_main_hand_goes_first_and_the_off_hand_on_the_next_click() {
        let mut rig = Rig::new();
        let (iron, gold) = (rig.disc(Tier::Iron), rig.disc(Tier::Golden));
        hold(&mut rig, iron.clone());
        rig.host.set_slot(BOB, 40, Some(gold.clone()));
        assert!(rig.use_item(BOB, &iron), "the main hand's click");
        assert!(
            !rig.use_item(BOB, &gold),
            "the off hand waits for the next tick"
        );
        rig.tick();
        assert!(rig.host.slot(BOB, 0).is_none() && rig.host.slot(BOB, 40).is_some());
        // The second click: the main hand is empty now, so the stack is the off hand's.
        assert!(rig.use_item(BOB, &gold));
        rig.tick();
        assert!(rig.host.slot(BOB, 40).is_none());
        assert_eq!(rig.shared.state().flying.len(), 2);
    }

    // ---- damage ----------------------------------------------------------------------------

    /// The damage of the first hit of a disc of `tier` with `sharpness` on a mob in its way.
    fn first_hit(tier: Tier, sharpness: u32) -> f32 {
        let mut rig = Rig::new();
        let mut disc = rig.disc(tier);
        if sharpness > 0 {
            disc.set_crafting_enchantments(vec![(Shared::from("minecraft:sharpness"), sharpness)]);
        }
        hold(&mut rig, disc);
        let mob = zombie_in_the_path(&mut rig, 4.0);
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(6);
        let hurt = rig
            .host
            .hurts
            .iter()
            .find(|h| h.0 == mob)
            .expect("it hit the zombie");
        assert_eq!(hurt.2, "minecraft:thrown");
        assert_eq!(hurt.3, Some(1), "Bob is the attacker");
        hurt.1
    }

    #[test]
    fn damage_scales_with_the_tier() {
        // Iron's 7 is Mojang's published number; the rest are the config's estimates.
        let damage: Vec<f32> = Tier::ALL.iter().map(|t| first_hit(*t, 0)).collect();
        assert_eq!(damage, [6.0, 7.0, 6.0, 8.0, 9.0]);
        assert!(damage[1] < damage[3] && damage[3] < damage[4]);
    }

    #[test]
    fn sharpness_adds_one_at_level_one_and_half_more_for_each_level_after() {
        let by_level: Vec<f32> = (0..=5).map(|l| first_hit(Tier::Iron, l)).collect();
        assert_eq!(by_level, [7.0, 8.0, 8.5, 9.0, 9.5, 10.0]);
        // Tier and sharpness add up: a netherite disc with Sharpness V.
        assert_eq!(first_hit(Tier::Netherite, 5), 12.0);
        assert_eq!(first_hit(Tier::Copper, 1), 7.0);
    }

    #[test]
    fn the_configs_damage_is_what_a_hit_does() {
        let config = "[tiers.iron]\ndamage = 12.5\n";
        let mut rig = Rig::with_config(Some(config));
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        let mob = zombie_in_the_path(&mut rig, 4.0);
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(6);
        assert_eq!(rig.host.hurts.iter().find(|h| h.0 == mob).unwrap().1, 12.5);
    }

    #[test]
    fn the_disc_pierces_every_mob_in_its_path_once_on_the_way_out() {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        let (near_mob, far_mob) = (
            zombie_in_the_path(&mut rig, 3.0),
            zombie_in_the_path(&mut rig, 7.0),
        );
        let beside = rig.host.add_mob("minecraft:zombie", [5.0, 64.0, 5.0]);
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(9);
        let hit: Vec<u32> = rig.host.hurts.iter().map(|h| h.0).collect();
        assert_eq!(hit, [near_mob, far_mob], "in the order they were reached");
        assert!(!hit.contains(&beside));
    }

    #[test]
    fn coming_back_it_hits_everything_again_and_is_caught_into_the_slot_it_left() {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        let mob = zombie_in_the_path(&mut rig, 5.0);
        assert!(rig.throw_from_hand(BOB));
        let mut returned = false;
        for _ in 0..80 {
            rig.tick();
            if rig.host.slot(BOB, 0).is_some() {
                returned = true;
                break;
            }
        }
        assert!(returned, "the disc comes back to the hand");
        let hits = rig.host.hurts.iter().filter(|h| h.0 == mob).count();
        assert_eq!(hits, 2, "out and back");
        assert!(
            rig.host.hurts.iter().all(|h| h.0 != 1),
            "never the one who threw it"
        );
        let back = rig.host.slot(BOB, 0).unwrap();
        assert_eq!(item::tier_of(back), Some(Tier::Iron));
        assert_eq!(back.damage, 1, "a throw costs a point");
        assert!(rig.host.display().is_none(), "the display is gone");
        assert!(
            rig.host
                .sounds
                .contains(&"to:minecraft:item.trident.return".to_owned())
        );
        assert!(rig.shared.state().flying.is_empty());
    }

    #[test]
    fn a_disc_thrown_from_a_hotbar_slot_goes_back_to_that_slot() {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        rig.host.player_mut(BOB).selected_slot = 4;
        rig.host.set_slot(BOB, 4, Some(disc));
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(80);
        assert!(rig.host.slot(BOB, 4).is_some());
        assert!(rig.host.slot(BOB, 0).is_none());
    }

    #[test]
    fn a_disc_thrown_from_the_off_hand_returns_to_it_and_the_slot_taken_meanwhile_gets_it_elsewhere()
     {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        rig.host.set_slot(BOB, 40, Some(disc.clone()));
        assert!(rig.use_item(BOB, &disc));
        rig.tick();
        rig.ticks(80);
        assert!(rig.host.slot(BOB, 40).is_some(), "back in the off hand");

        // The slot it left is taken when it comes back: the next free one has it.
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        assert!(rig.throw_from_hand(BOB));
        rig.tick();
        let stick = ItemStack::new("minecraft:stick", 1);
        rig.host.set_slot(BOB, 0, Some(stick.clone()));
        rig.ticks(80);
        assert_eq!(rig.host.slot(BOB, 0), Some(&stick));
        assert!(
            rig.host.carried(BOB).iter().any(item::is_disc),
            "it is in the inventory"
        );
    }

    #[test]
    fn knockback_pushes_along_the_flight_and_a_level_pushes_further() {
        let push = |level: u32| {
            let mut rig = Rig::new();
            let mut disc = rig.disc(Tier::Iron);
            if level > 0 {
                disc.set_crafting_enchantments(vec![(Shared::from("minecraft:knockback"), level)]);
            }
            hold(&mut rig, disc);
            let mob = zombie_in_the_path(&mut rig, 4.0);
            assert!(rig.throw_from_hand(BOB));
            rig.ticks(6);
            rig.host.mob(mob).velocity
        };
        let [x, y, z] = push(0);
        assert!(
            near(x, 0.0) && near(z, 0.25) && near(y, 0.2 + 0.25 * 0.3),
            "{x} {y} {z}"
        );
        let [_, y2, z2] = push(2);
        assert!(near(z2, 0.25 + 2.0 * 0.5) && near(y2, 0.4), "{y2} {z2}");
    }

    #[test]
    fn players_are_hit_unless_pvp_is_off_they_are_spectators_or_it_is_no_mob() {
        // Alice stands in the path.
        let hit = |config: Option<&str>, mode: i32| {
            let mut rig = Rig::with_config(config);
            let disc = rig.disc(Tier::Iron);
            hold(&mut rig, disc);
            rig.host.player_mut(ALICE).position = [-0.25, 64.0, 4.0];
            rig.host.player_mut(ALICE).game_mode = mode;
            assert!(rig.throw_from_hand(BOB));
            rig.ticks(6);
            rig.host.hurts.iter().any(|h| h.0 == 2)
        };
        assert!(hit(None, 0), "a player in survival is hit");
        assert!(!hit(Some("pvp = false\n"), 0), "pvp off");
        assert!(!hit(None, 3), "a spectator");

        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        let stand = rig
            .host
            .add_mob("minecraft:armor_stand", [-0.25, 64.0, 4.0]);
        let frame = rig.host.add_mob("minecraft:item_frame", [-0.25, 64.0, 5.0]);
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(8);
        assert!(rig.host.hurts.iter().all(|h| h.0 != stand && h.0 != frame));
    }

    // ---- blocks ----------------------------------------------------------------------------

    fn wall(rig: &mut Rig, z: i32, block: &str) {
        for x in -3..=2 {
            for y in 62..=68 {
                rig.host.blocks.insert([x, y, z], block.to_owned());
            }
        }
    }

    #[test]
    fn a_hard_block_sends_the_disc_back_faster_and_with_more_range() {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        wall(&mut rig, 6, "minecraft:stone");
        assert!(rig.throw_from_hand(BOB));
        // Five ticks reach z 5.65; the sixth meets the wall at 6.0 and reflects off it.
        rig.ticks(6);
        assert!(
            near(display_at(&rig)[2], 5.85),
            "pushed off the face by 0.15"
        );
        rig.tick();
        assert!(
            near(display_at(&rig)[2], 5.85 - 1.05 * 1.2),
            "1.2 times as fast, going back"
        );
        assert!(
            rig.host
                .sounds
                .contains(&"minecraft:item.trident.hit_ground".to_owned())
        );
        // The bounce gave it 6 more blocks of range, so it flies on past Bob before turning.
        let mut lowest = f64::MAX;
        for _ in 0..14 {
            rig.tick();
            if let Some(id) = rig.host.display() {
                lowest = lowest.min(rig.host.moved[&id].0[2]);
            }
        }
        assert!(lowest < -4.0, "it reached {lowest}");
    }

    #[test]
    fn with_no_bounces_left_a_hard_block_turns_the_disc_back_like_a_soft_one() {
        let mut rig = Rig::with_config(Some("[ricochet]\nmax-bounces = 0\n"));
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        wall(&mut rig, 6, "minecraft:stone");
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(8);
        // No reflection: the disc did not get faster and never went through the wall.
        let z = display_at(&rig)[2];
        assert!(z < 5.85, "it turned back, at {z}");
        rig.ticks(80);
        assert!(rig.host.slot(BOB, 0).is_some());
    }

    #[test]
    fn soft_blocks_by_tag_or_id_turn_the_disc_back_without_a_bounce() {
        for block in ["minecraft:oak_leaves", "minecraft:hay_block"] {
            let mut rig = Rig::new();
            let disc = rig.disc(Tier::Iron);
            hold(&mut rig, disc);
            wall(&mut rig, 6, block);
            assert!(rig.throw_from_hand(BOB));
            let mut farthest = 0.0_f64;
            let mut back = false;
            for _ in 0..80 {
                rig.tick();
                if let Some(id) = rig.host.display() {
                    farthest = farthest.max(rig.host.moved[&id].0[2]);
                }
                if rig.host.slot(BOB, 0).is_some() {
                    back = true;
                    break;
                }
            }
            assert!(back, "{block}: it came back");
            assert!(
                farthest <= 6.0,
                "{block}: it never passed the wall ({farthest})"
            );
        }
    }

    #[test]
    fn a_decorated_pot_is_broken_and_the_disc_flies_on() {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        rig.host
            .blocks
            .insert([-1, 65, 6], "minecraft:decorated_pot".to_owned());
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(8);
        assert_eq!(
            rig.host.block_properties,
            [([-1, 65, 6], "cracked".to_owned(), "true".to_owned())],
            "cracked first, so it drops its sherds"
        );
        assert!(rig.host.commands.contains(
            &"execute in minecraft:overworld run setblock -1 65 6 minecraft:air destroy".to_owned()
        ));
        assert!(
            rig.host
                .sounds
                .contains(&"minecraft:block.decorated_pot.shatter".to_owned())
        );
        assert!(display_at(&rig)[2] > 6.0, "it kept flying");
        assert!(rig.host.blocks.is_empty(), "the pot is gone");
    }

    #[test]
    fn with_pot_breaking_off_a_pot_is_a_hard_block() {
        let mut rig = Rig::with_config(Some("break-decorated-pots = false\n"));
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        rig.host
            .blocks
            .insert([-1, 65, 6], "minecraft:decorated_pot".to_owned());
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(8);
        assert!(rig.host.block_properties.is_empty());
        assert!(rig.host.blocks.contains_key(&[-1, 65, 6]));
    }

    // ---- durability ------------------------------------------------------------------------

    #[test]
    fn a_throw_costs_durability_and_a_disc_with_one_point_left_cannot_be_thrown() {
        let mut rig = Rig::new();
        let mut disc = rig.disc(Tier::Iron);
        disc.damage = 248;
        hold(&mut rig, disc.clone());
        assert!(rig.use_item(BOB, &disc), "two points left: one throw");
        rig.tick();
        rig.ticks(80);
        let worn = rig.host.slot(BOB, 0).unwrap().clone();
        assert_eq!(worn.damage, 249);
        assert!(
            !rig.use_item(BOB, &worn),
            "one point left: like a trident, no throw"
        );
    }

    #[test]
    fn creative_players_wear_nothing_and_the_config_sets_the_cost() {
        let mut rig = Rig::new();
        rig.host.player_mut(BOB).game_mode = 1;
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(80);
        assert_eq!(rig.host.slot(BOB, 0).unwrap().damage, 0);

        let mut rig = Rig::with_config(Some("durability-per-throw = 5\n"));
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(80);
        assert_eq!(rig.host.slot(BOB, 0).unwrap().damage, 5);
    }

    #[test]
    fn unbreaking_makes_a_throw_cost_nothing_some_of_the_time() {
        // Unbreaking III: each point is spent one time in four.
        let wear = |level: u32| {
            let mut rig = Rig::new();
            let mut disc = rig.disc(Tier::Diamond);
            if level > 0 {
                disc.set_crafting_enchantments(vec![(Shared::from("minecraft:unbreaking"), level)]);
            }
            hold(&mut rig, disc);
            for _ in 0..80 {
                assert!(rig.throw_from_hand(BOB));
                rig.tick();
                // The server stops and hands the disc back, for the next throw.
                rig.stop_server();
            }
            rig.host.slot(BOB, 0).unwrap().damage
        };
        assert_eq!(wear(0), 80, "without it, a point a throw");
        let with = wear(3);
        assert!((8..=34).contains(&with), "about a quarter of 80: {with}");
    }

    // ---- safety ----------------------------------------------------------------------------

    #[test]
    fn a_player_who_logs_out_has_the_disc_in_their_saved_inventory() {
        let mut rig = thrown(Tier::Iron);
        rig.leave(BOB);
        assert!(rig.shared.state().flying.is_empty());
        assert!(rig.host.display().is_none());
        assert!(rig.host.slot(BOB, 0).is_some(), "back in the slot it left");
    }

    #[test]
    fn a_death_drops_the_disc_with_the_items_or_returns_it_under_keep_inventory() {
        let mut rig = thrown(Tier::Iron);
        rig.die(BOB);
        assert!(rig.shared.state().flying.is_empty());
        assert!(rig.host.display().is_none());
        let dropped = rig.host.item_entities();
        assert_eq!(dropped.len(), 1, "an item at Bob's feet");
        assert_eq!(dropped[0].2, [0.0, 64.0, 0.0]);
        assert!(rig.host.carried(BOB).is_empty());

        let mut rig = thrown(Tier::Iron);
        rig.host.keep_inventory = true;
        rig.die(BOB);
        assert!(rig.host.item_entities().is_empty(), "nothing on the ground");
        assert!(rig.host.carried(BOB).is_empty(), "not while he is dead");
        rig.respawn(BOB);
        assert!(
            rig.host.carried(BOB).iter().any(item::is_disc),
            "back with him on respawn"
        );
    }

    #[test]
    fn a_death_while_the_disc_is_being_moved_is_handled_in_the_tick() {
        let mut rig = thrown(Tier::Iron);
        // The player died without the death hook running first: the next tick finds them dead.
        rig.host.player_mut(BOB).health = 0.0;
        rig.tick();
        assert!(rig.shared.state().flying.is_empty());
        assert_eq!(rig.host.item_entities().len(), 1);
    }

    #[test]
    fn stopping_the_server_gives_every_disc_back() {
        let mut rig = thrown(Tier::Iron);
        rig.stop_server();
        assert!(rig.shared.state().flying.is_empty());
        assert!(rig.host.display().is_none());
        assert!(rig.host.slot(BOB, 0).is_some());
    }

    #[test]
    fn leaving_the_level_an_unloaded_chunk_and_a_long_flight_end_with_the_disc_in_the_inventory() {
        // Another dimension.
        let mut rig = thrown(Tier::Iron);
        rig.host.player_mut(BOB).dimension = "minecraft:the_nether".to_owned();
        rig.tick();
        assert!(rig.host.slot(BOB, 0).is_some() && rig.host.display().is_none());

        // The chunk it is in unloads.
        let mut rig = thrown(Tier::Iron);
        rig.host.unloaded.insert((-1, 0));
        rig.tick();
        assert!(rig.host.slot(BOB, 0).is_some());

        // Still flying after the lifetime: a slow disc that cannot get far.
        let mut rig = Rig::with_config(Some(
            "[flight]\nmax-lifetime-ticks = 20\n[tiers.iron]\nspeed = 0.1\n",
        ));
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        assert!(rig.throw_from_hand(BOB));
        rig.ticks(21);
        assert!(rig.host.slot(BOB, 0).is_some(), "given back");
        assert!(
            !rig.host
                .sounds
                .contains(&"to:minecraft:item.trident.return".to_owned()),
            "not caught: sent back"
        );
    }

    #[test]
    fn strays_a_crash_left_are_removed_once_a_level() {
        let mut rig = Rig::new();
        rig.ticks(3);
        let kills: Vec<&String> = rig
            .host
            .commands
            .iter()
            .filter(|c| c.contains("kill @e"))
            .collect();
        assert_eq!(
            kills,
            [
                "execute in minecraft:overworld run kill @e[type=minecraft:item_display,tag=throwingdiscs_flight]"
            ]
        );
    }

    // ---- commands --------------------------------------------------------------------------

    #[test]
    fn discs_give_hands_out_the_tier_the_amount_or_all_of_them() {
        let mut rig = Rig::new();
        assert!(rig.console("discs give Bob iron"));
        assert_eq!(rig.host.feedback, ["Gave 1 throwing disc to Bob!"]);
        let discs = rig.host.carried(BOB);
        assert_eq!(item::tier_of(&discs[0]), Some(Tier::Iron));
        assert!(rig.console("/discs give bob gold 2"));
        assert_eq!(rig.host.feedback, ["Gave 2 throwing discs to Bob!"]);
        assert!(rig.console("discs give Bob all"));
        assert_eq!(rig.host.feedback, ["Gave 5 throwing discs to Bob!"]);
        assert_eq!(rig.host.carried(BOB).len(), 8);
    }

    #[test]
    fn discs_give_says_what_is_wrong_in_the_two_line_style() {
        let mut rig = Rig::new();
        let header = "Unknown or incomplete command. See below for error";
        rig.console("discs give Nobody iron");
        assert_eq!(
            rig.host.feedback,
            [header, "...ive Nobody<--[Player was not found]"].map(String::from)
        );
        rig.console("discs give Bob wooden");
        assert_eq!(rig.host.feedback[0], header);
        assert!(
            rig.host.feedback[1].contains("<--[Unknown tier"),
            "{:?}",
            rig.host.feedback
        );
        rig.console("discs give Bob iron many");
        assert!(rig.host.feedback[1].ends_with("<--[The amount must be a number]"));
        rig.console("discs give");
        assert!(rig.host.feedback[1].contains("<--[Usage: /discs give"));
        assert!(rig.host.carried(BOB).is_empty());
    }

    #[test]
    fn discs_that_do_not_fit_drop_at_the_players_feet() {
        let mut rig = Rig::new();
        for slot in 0..36 {
            rig.host
                .set_slot(BOB, slot, Some(ItemStack::new("minecraft:stick", 1)));
        }
        assert!(rig.console("discs give Bob iron"));
        assert_eq!(rig.host.item_entities().len(), 1);
    }

    #[test]
    fn reload_reads_the_file_and_a_broken_one_keeps_the_old_settings() {
        let mut rig = Rig::new();
        assert_eq!(rig.config().tiers.iron.damage, 7.0);
        std::fs::write(
            rig.dir.join("config").join("throwingdiscs.toml"),
            "[tiers.iron]\ndamage = 9.0\n",
        )
        .unwrap();
        assert!(rig.console("discs reload"));
        assert_eq!(rig.host.feedback[0], "Reloaded the Throwing Discs config!");
        assert_eq!(rig.config().tiers.iron.damage, 9.0);
        assert!(
            rig.host.feedback[1].contains("/reload"),
            "recipes follow a /reload"
        );
        std::fs::write(
            rig.dir.join("config").join("throwingdiscs.toml"),
            "[tiers.iron\n",
        )
        .unwrap();
        assert!(rig.console("discs reload"));
        assert!(rig.host.feedback[0].starts_with("Could not read the config"));
        assert_eq!(rig.config().tiers.iron.damage, 9.0);
    }

    #[test]
    fn the_first_hook_writes_the_config_a_new_server_has_none_of() {
        let rig = Rig::new();
        let _ = rig.data_pack_files();
        assert!(rig.dir.join("config").join("throwingdiscs.toml").exists());
    }

    // ---- /enchant --------------------------------------------------------------------------

    #[test]
    fn enchant_works_on_the_disc_in_hand_and_setting_a_level_again_replaces_it() {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        assert!(rig.say(BOB, "enchant Bob sharpness 3"));
        assert_eq!(
            rig.host.feedback,
            ["Applied enchantment Sharpness III to Bob's item!"]
        );
        rig.tick();
        assert_eq!(
            rig.host
                .slot(BOB, 0)
                .unwrap()
                .enchantment_level("minecraft:sharpness"),
            3
        );
        assert!(rig.say(BOB, "/minecraft:enchant @s minecraft:sharpness 1"));
        rig.tick();
        assert_eq!(
            rig.host
                .slot(BOB, 0)
                .unwrap()
                .enchantment_level("minecraft:sharpness"),
            1
        );
        assert!(rig.say(BOB, "enchant @p mending"));
        rig.tick();
        assert_eq!(
            rig.host
                .slot(BOB, 0)
                .unwrap()
                .enchantment_level("minecraft:mending"),
            1
        );
    }

    #[test]
    fn enchant_refuses_what_a_disc_cannot_have_and_leaves_other_lines_to_vanilla() {
        let mut rig = Rig::new();
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        assert!(rig.say(BOB, "enchant Bob fire_aspect 1"));
        assert_eq!(
            rig.host.feedback[0],
            "Unknown or incomplete command. See below for error"
        );
        assert!(rig.host.feedback[1].contains("Throwing discs can't have Fire aspect"));
        assert!(rig.say(BOB, "enchant Bob sharpness 6"));
        assert!(rig.host.feedback[1].contains("goes up to level 5"));
        assert!(rig.say(BOB, "enchant Bob sharpness 0"));
        assert!(rig.host.feedback[1].contains("at least 1"));
        rig.tick();
        assert!(!rig.host.slot(BOB, 0).unwrap().has_any_enchantments());

        // Not a disc in the hand, a selector this does not read, or no permission: vanilla's.
        hold(&mut rig, ItemStack::new("minecraft:iron_sword", 1));
        assert!(!rig.say(BOB, "enchant Bob sharpness 3"));
        let disc = rig.disc(Tier::Iron);
        hold(&mut rig, disc);
        assert!(!rig.say(BOB, "enchant @e[type=zombie] sharpness 3"));
        rig.host.permitted = false;
        assert!(!rig.say(BOB, "enchant Bob sharpness 3"));
    }

    // ---- crafting, smithing and the anvil ---------------------------------------------------

    #[test]
    fn the_recipes_are_added_with_the_crafting_switch_on_and_not_with_it_off() {
        let rig = Rig::new();
        assert_eq!(rig.data_pack_files().len(), 5);
        let rig = Rig::with_config(Some("crafting = false\n"));
        assert!(rig.data_pack_files().is_empty());
        assert!(
            rig.host
                .commands
                .iter()
                .all(|c| !c.starts_with("recipe give"))
        );
    }

    #[test]
    fn a_joining_player_is_shown_the_recipes_in_their_recipe_book() {
        let mut rig = Rig::new();
        rig.host.commands.clear();
        rig.join("Eve", 0xe7e, 0, 3, [0.0; 3]);
        let given: Vec<&String> = rig
            .host
            .commands
            .iter()
            .filter(|c| c.starts_with("recipe give Eve"))
            .collect();
        assert_eq!(given.len(), 5);
        assert!(
            given
                .iter()
                .any(|c| c.ends_with("throwingdiscs:netherite_throwing_disc_smithing"))
        );
    }

    #[test]
    fn the_smithing_table_upgrades_only_a_real_diamond_disc_and_keeps_its_enchantments() {
        let mut rig = Rig::new();
        let template = ItemStack::new("minecraft:netherite_upgrade_smithing_template", 1);
        let ingot = ItemStack::new("minecraft:netherite_ingot", 1);
        let mut diamond = rig.disc(Tier::Diamond);
        diamond.set_crafting_enchantments(vec![(Shared::from("minecraft:sharpness"), 4)]);
        let recipe_result = Some(rig.disc(Tier::Netherite));

        let made = rig
            .smithing(
                Some(&template),
                Some(&diamond),
                Some(&ingot),
                recipe_result.clone(),
            )
            .expect("an upgrade");
        assert_eq!(item::tier_of(&made), Some(Tier::Netherite));
        assert_eq!(made.enchantment_level("minecraft:sharpness"), 4);

        // The base slot took an echo shard that is no disc, or another tier: no result.
        let shard = ItemStack::new("minecraft:echo_shard", 1);
        assert_eq!(
            rig.smithing(
                Some(&template),
                Some(&shard),
                Some(&ingot),
                recipe_result.clone()
            ),
            None
        );
        let iron = rig.disc(Tier::Iron);
        assert_eq!(
            rig.smithing(Some(&template), Some(&iron), Some(&ingot), recipe_result),
            None
        );
        // A smithing recipe of vanilla's is left alone.
        let vanilla = Some(ItemStack::new("minecraft:netherite_sword", 1));
        assert_eq!(
            rig.smithing(
                Some(&template),
                Some(&ItemStack::new("minecraft:diamond_sword", 1)),
                Some(&ingot),
                vanilla.clone()
            ),
            vanilla
        );
    }

    #[test]
    fn a_disc_cannot_be_crafted_into_a_recovery_compass() {
        let mut rig = Rig::new();
        let compass = Some(ItemStack::new("minecraft:recovery_compass", 1));
        let shard = ItemStack::new("minecraft:echo_shard", 1);
        let mut grid = vec![Some(shard.clone()); 8];
        grid.push(Some(ItemStack::new("minecraft:compass", 1)));
        assert_eq!(
            rig.crafting(&grid, compass.clone()),
            compass,
            "plain shards still make one"
        );
        grid[3] = Some(rig.disc(Tier::Iron));
        assert_eq!(rig.crafting(&grid, compass), None);
    }

    #[test]
    fn the_anvil_takes_books_and_matching_discs_through_the_hook() {
        let mut rig = Rig::new();
        let left = rig.disc(Tier::Iron);
        let mut book = ItemStack::new("minecraft:enchanted_book", 1);
        book.set_crafting_enchantments(vec![(Shared::from("minecraft:knockback"), 2)]);
        let (result, cost) = rig
            .anvil(&left, Some(&book), None)
            .expect("the mod answers");
        assert_eq!(result.unwrap().enchantment_level("minecraft:knockback"), 2);
        assert_eq!(cost, 4);
        // A sword with a book is vanilla's; a disc as the material of a sword is nobody's.
        let sword = ItemStack::new("minecraft:iron_sword", 1);
        assert_eq!(rig.anvil(&sword, Some(&book), None), None);
        assert_eq!(rig.anvil(&sword, Some(&left), None), Some((None, 0)));
    }
}
