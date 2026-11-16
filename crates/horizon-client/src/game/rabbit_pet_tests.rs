//! Rift's tamed rabbits (vanilla's are not tamable): a carrot wins a wild one over a third of the
//! time, its owner makes it sit or stand, its food heals it, and it follows its owner, as a wolf's
//! `Wolf.mobInteract` and `FollowOwnerGoal` do for wolves.
use super::*;
use horizon_client::chunk::Chunk;
use horizon_client::mobs::{Mob, Species};
use horizon_core::position::ChunkPos;
use std::sync::Arc;

fn game() -> Game<'static> {
    let root = std::path::PathBuf::from(
        std::env::var_os("HORIZON_TEST_RESOURCES")
            .expect("set HORIZON_TEST_RESOURCES to prepared resources"),
    );
    let settings = Settings::default();
    let loaded = load_resources(
        root.clone(),
        None,
        &root,
        &settings,
        std::time::Instant::now(),
        false,
        true,
        &Progress::new(),
        &mut |_, _| unreachable!(),
    )
    .unwrap();
    let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
    options.resources = root.clone();
    options.execution_role = execution_role::ExecutionRole::IntegratedServer;
    let res: &'static _ = Box::leak(Box::new(loaded.resources));
    let mut game = Game::new(
        res,
        loaded.pack,
        loaded.gui.unwrap(),
        options,
        settings,
        Audio::silent(&root),
        None,
    )
    .unwrap();
    game.build_registries();
    game.set_allow_commands(true);
    // The centre chunk and the two rings round it, lit and loaded: a chunk ticks its entities only
    // once every chunk within two of it is loaded (`ChunkMap.prepareEntityTickingChunk`).
    for x in -2..=2 {
        for z in -2..=2 {
            let mut chunk = Chunk::empty(x, z);
            chunk.lit = true;
            game.world.chunks.insert((x, z), Arc::new(chunk));
            game.block_ticks
                .add_empty_chunk(ChunkPos::new(x, z))
                .unwrap();
        }
    }
    assert!(game.run_command("difficulty normal"));
    assert!(game.run_command("forceload add 0 0"));
    assert!(game.run_command("fill 0 63 0 15 63 15 stone"));
    assert!(game.run_command("gamerule spawn_mobs false"));
    game.player = Some(PlayerMotion::new(game.motion, Vec3::new(0.5, 64.0, 0.5)).unwrap());
    game.screen = Screen::Playing;
    game.selected = 0;
    game.set_mode_id(0);
    game.world.random = horizon_core::random::LegacyRandom::new(7);
    game.update_chunk_tickets();
    assert!(game.entity_ticking_area()(4, 4));
    game
}

/// A wild adult rabbit at `at`, with a random of its own, as a spawn gives it.
fn rabbit(game: &mut Game<'_>, id: u32, at: [f64; 3]) -> u32 {
    let kind = game
        .res
        .mob_types()
        .get("minecraft:rabbit")
        .unwrap()
        .clone();
    let (own, _) = horizon_client::mobs::living_random();
    let mut m = Mob::with_random(id, kind, Vec3::new(at[0], at[1], at[2]), 0.0, own);
    m.on_ground = true;
    game.mobs.push(m);
    id
}

fn mob<'a>(game: &'a Game<'_>, id: u32) -> &'a Mob {
    game.mobs.iter().find(|m| m.id == id).unwrap()
}

fn mob_mut<'a>(game: &'a mut Game<'_>, id: u32) -> &'a mut Mob {
    game.mobs.iter_mut().find(|m| m.id == id).unwrap()
}

/// A tamed rabbit of the acting player.
fn pet(game: &mut Game<'_>, id: u32, at: [f64; 3]) -> u32 {
    rabbit(game, id, at);
    let owner = game.acting_player_uuid();
    let m = mob_mut(game, id);
    m.extra.tame = true;
    m.extra.owner = Some(owner);
    m.persistent = true;
    id
}

/// The player right-clicks the rabbit with one `item`: how many are left.
fn use_on(game: &mut Game<'_>, id: u32, item: &str) -> u32 {
    game.inventory[0] = Some(ItemStack::new(item, 1));
    let target = Some(InteractTarget {
        id: id as i32,
        location: [0.0, 0.5, 0.0],
    });
    game.interact_mob(target).unwrap();
    game.inventory[0].as_ref().map_or(0, |s| s.count)
}

fn distance_to_player(game: &Game<'_>, id: u32) -> f64 {
    let p = game.player.as_ref().unwrap().position;
    let m = mob(game, id).position;
    ((m.x - p.x).powi(2) + (m.z - p.z).powi(2)).sqrt()
}

/// One server tick of the mobs.
fn tick(game: &mut Game<'_>) {
    game.world.game_time += 1;
    game.update_chunk_tickets();
    game.tick_creature_entities().unwrap();
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_carrot_tames_a_wild_rabbit_one_try_in_three_and_is_used_up() {
    let mut game = game();
    let me = game.acting_player_uuid();
    let tries = 90;
    let mut tamed = 0;
    for i in 0..tries {
        let id = rabbit(&mut game, 100 + i, [0.5, 64.0, 2.0]);
        assert_eq!(
            use_on(&mut game, id, "minecraft:carrot"),
            0,
            "the carrot is eaten"
        );
        let m = mob(&game, id);
        if m.extra.tame {
            tamed += 1;
            assert_eq!(m.extra.owner, Some(me));
            assert!(m.persistent, "a pet does not despawn");
            assert!(!m.extra.sitting, "it follows from the start");
        } else {
            assert!(m.extra.owner.is_none());
        }
    }
    // Expected 30 of 90, 4.5 standard deviations either way is 9 to 51.
    assert!((9..=51).contains(&tamed), "{tamed} of {tries} tamed");
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn in_creative_the_carrot_stays_in_the_hand() {
    let mut game = game();
    game.set_mode_id(1);
    let id = rabbit(&mut game, 100, [0.5, 64.0, 2.0]);
    assert_eq!(use_on(&mut game, id, "minecraft:carrot"), 1);
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn the_other_rabbit_foods_breed_a_wild_rabbit_and_tame_nothing() {
    let mut game = game();
    for (i, food) in ["minecraft:golden_carrot", "minecraft:dandelion"]
        .into_iter()
        .enumerate()
    {
        let id = rabbit(&mut game, 100 + i as u32, [0.5, 64.0, 2.0]);
        assert_eq!(use_on(&mut game, id, food), 0, "{food} is eaten");
        let m = mob(&game, id);
        assert!(m.extra.in_love > 0 && !m.extra.tame, "{food}");
    }
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn only_the_owner_makes_a_tamed_rabbit_sit_or_stand() {
    let mut game = game();
    let id = pet(&mut game, 100, [0.5, 64.0, 2.0]);
    assert_eq!(
        use_on(&mut game, id, "minecraft:stick"),
        1,
        "nothing is used up"
    );
    assert!(mob(&game, id).extra.sitting);
    use_on(&mut game, id, "minecraft:stick");
    assert!(!mob(&game, id).extra.sitting);
    // Another player's rabbit stays as it is.
    mob_mut(&mut game, id).extra.owner = Some([1, 2, 3, 4]);
    use_on(&mut game, id, "minecraft:stick");
    assert!(!mob(&game, id).extra.sitting);
    // A wild one does not sit either.
    let wild = rabbit(&mut game, 101, [0.5, 64.0, 2.0]);
    use_on(&mut game, wild, "minecraft:stick");
    assert!(!mob(&game, wild).extra.sitting && !mob(&game, wild).extra.tame);
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_carrot_heals_a_hurt_tamed_rabbit_but_only_the_other_foods_breed_it() {
    let mut game = game();
    let id = pet(&mut game, 100, [0.5, 64.0, 2.0]);
    let max = mob(&game, id).attrs.max_health;
    mob_mut(&mut game, id).health = 1.0;
    assert_eq!(use_on(&mut game, id, "minecraft:carrot"), 0);
    assert_eq!(
        mob(&game, id).health,
        max,
        "a carrot's 3 points of food heal a rabbit of 3"
    );
    assert!(!mob(&game, id).extra.sitting);
    // Whole, a carrot breeds nothing and is kept: it makes the rabbit sit, as any item does.
    assert_eq!(use_on(&mut game, id, "minecraft:carrot"), 1);
    assert_eq!(mob(&game, id).extra.in_love, 0);
    assert!(mob(&game, id).extra.sitting);
    // Golden carrots and dandelions breed it, and are used up.
    for (food, other) in [
        ("minecraft:golden_carrot", 101),
        ("minecraft:dandelion", 102),
    ] {
        let id = pet(&mut game, other, [0.5, 64.0, 2.0]);
        assert_eq!(use_on(&mut game, id, food), 0, "{food} is eaten");
        assert!(mob(&game, id).extra.in_love > 0, "{food}");
        assert!(!mob(&game, id).extra.sitting, "{food}");
    }
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn another_players_carrot_does_nothing_to_a_tamed_rabbit() {
    let mut game = game();
    let id = pet(&mut game, 100, [0.5, 64.0, 2.0]);
    mob_mut(&mut game, id).extra.owner = Some([1, 2, 3, 4]);
    assert_eq!(use_on(&mut game, id, "minecraft:carrot"), 1);
    let m = mob(&game, id);
    assert!(m.extra.in_love == 0 && !m.extra.sitting);
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_rabbit_told_to_sit_with_a_carrot_in_hand_stays_put_where_it_was_following() {
    let mut game = game();
    // Within the follow distance, the rabbit comes to a player holding its food (`Tempt`).
    game.player.as_mut().unwrap().position = Vec3::new(8.5, 64.0, 4.5);
    let id = pet(&mut game, 100, [1.5, 64.0, 1.5]);
    for _ in 0..40 {
        tick(&mut game);
    }
    assert!(!mob(&game, id).extra.sitting);
    assert_eq!(use_on(&mut game, id, "minecraft:carrot"), 1);
    assert!(
        mob(&game, id).extra.sitting,
        "the carrot is no breeding food"
    );
    // The carrot is still in the player's hand. Let it land and settle, then it must not move.
    for _ in 0..30 {
        tick(&mut game);
    }
    let at = mob(&game, id).position;
    for _ in 0..150 {
        tick(&mut game);
    }
    let now = mob(&game, id).position;
    assert!(
        ((now.x - at.x).powi(2) + (now.z - at.z).powi(2)).sqrt() < 0.3,
        "it moved from {at:?} to {now:?}"
    );
    assert!(mob(&game, id).extra.sitting);
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_tamed_rabbit_far_from_its_owner_comes_to_them_and_a_sitting_one_does_not() {
    let mut game = game();
    game.player.as_mut().unwrap().position = Vec3::new(12.5, 64.0, 12.5);
    let walker = pet(&mut game, 100, [1.5, 64.0, 1.5]);
    let sitter = pet(&mut game, 101, [1.5, 64.0, 2.5]);
    mob_mut(&mut game, sitter).extra.sitting = true;
    let wild = rabbit(&mut game, 102, [2.5, 64.0, 1.5]);
    let before = distance_to_player(&game, wild);
    let sitting_before = distance_to_player(&game, sitter);
    for _ in 0..10 {
        tick(&mut game);
    }
    // Past twelve blocks it goes to its owner at once, as a wolf does.
    assert!(
        distance_to_player(&game, walker) < 6.0,
        "{}",
        distance_to_player(&game, walker)
    );
    assert!(
        (distance_to_player(&game, sitter) - sitting_before).abs() < 0.5,
        "a sitting rabbit stays where it is"
    );
    assert!(
        (distance_to_player(&game, wild) - before).abs() < 4.0,
        "a wild rabbit is not drawn to anyone"
    );
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_tamed_rabbit_walks_to_its_owner_from_ten_blocks_and_stops_near() {
    let mut game = game();
    game.player.as_mut().unwrap().position = Vec3::new(9.5, 64.0, 6.5);
    let id = pet(&mut game, 100, [0.5, 64.0, 0.5]);
    let start = distance_to_player(&game, id);
    assert!(start > 10.0 && start < 12.0, "{start}");
    for _ in 0..200 {
        tick(&mut game);
    }
    let end = distance_to_player(&game, id);
    assert!(end < 5.0, "from {start} to {end}");
}

/// A rabbit that follows at a plain speed modifier of 1.0 takes about 180 ticks to cover 11 blocks
/// to its owner (measured); a pet hops about four times as fast, so it takes about 45.
#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_tamed_rabbit_hops_about_four_times_as_fast_as_a_plain_follow() {
    let mut game = game();
    game.player.as_mut().unwrap().position = Vec3::new(10.5, 64.0, 6.5);
    let id = pet(&mut game, 100, [0.5, 64.0, 0.5]);
    let start = distance_to_player(&game, id);
    assert!(start > 11.0 && start < 12.0, "{start}");
    let mut arrived = None;
    for t in 0..180 {
        tick(&mut game);
        if arrived.is_none() && distance_to_player(&game, id) < 3.0 {
            arrived = Some(t);
        }
    }
    let arrived = arrived.expect("it came");
    assert!(arrived < 70, "arrived after {arrived} ticks");
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_tamed_rabbit_does_not_run_from_players_where_a_wild_one_does() {
    let mut game = game();
    game.player.as_mut().unwrap().position = Vec3::new(6.5, 64.0, 6.5);
    let wild = rabbit(&mut game, 100, [8.5, 64.0, 6.5]);
    let tame = pet(&mut game, 101, [8.5, 64.0, 8.5]);
    mob_mut(&mut game, tame).extra.sitting = true;
    let mut fled = false;
    for _ in 0..40 {
        tick(&mut game);
        fled |= mob(&game, wild)
            .ai
            .goals
            .is_running("RabbitAvoidEntityGoal");
    }
    assert!(fled, "a wild rabbit runs from the player");
    assert!(
        !mob(&game, tame)
            .ai
            .goals
            .is_running("RabbitAvoidEntityGoal")
    );
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_tamed_rabbits_baby_is_tamed_by_the_same_owner() {
    let mut game = game();
    let mother = pet(&mut game, 100, [0.5, 64.0, 2.0]);
    let father = pet(&mut game, 101, [0.5, 64.0, 2.5]);
    for id in [mother, father] {
        mob_mut(&mut game, id).extra.in_love = 600;
    }
    let owner = game.acting_player_uuid();
    for _ in 0..200 {
        tick(&mut game);
        if game.mobs.iter().any(|m| m.is_baby()) {
            break;
        }
    }
    let baby = game
        .mobs
        .iter()
        .find(|m| m.is_baby() && m.species() == Species::Rabbit)
        .expect("a baby rabbit");
    assert!(baby.extra.tame && baby.persistent);
    assert_eq!(baby.extra.owner, Some(owner));
}

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn a_summoned_rabbit_with_an_owner_is_a_pet_and_a_wild_one_is_not() {
    let mut game = game();
    assert!(game.run_command(
        "summon rabbit 4.5 64 4.5 {Owner:[I;7,8,9,10],Sitting:1b,PersistenceRequired:1b}"
    ));
    assert!(game.run_command("summon rabbit 6.5 64 4.5 {PersistenceRequired:1b}"));
    let pets: Vec<_> = game
        .mobs
        .iter()
        .filter(|m| m.species() == Species::Rabbit && m.extra.tame)
        .collect();
    assert_eq!(pets.len(), 1);
    assert_eq!(pets[0].extra.owner, Some([7, 8, 9, 10]));
    assert!(pets[0].extra.sitting);
    assert_eq!(
        game.mobs
            .iter()
            .filter(|m| m.species() == Species::Rabbit && !m.extra.tame)
            .count(),
        1
    );
}
