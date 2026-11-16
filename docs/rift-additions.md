# Rift additions to vanilla behaviour

Vanilla is the specification wherever it has the feature. This page lists the places where the engine
deliberately does something else, so a parity difference there is not a bug. Each addition is engine code, not
a mod, because the data it needs (an owner, a sitting flag) has to be saved with the mob.

## Tameable rabbits

A player can tame a rabbit the way they tame a wolf.

- **Taming.** Feeding a wild rabbit a carrot uses the carrot up and tames the rabbit one time in three
  (`nextInt(3) == 0`, the wolf's odds). A tamed rabbit shows hearts; a failed try shows smoke. A player in
  creative mode keeps the carrot. Taming fires the `minecraft:tame_animal` advancement trigger.
- **Breeding food.** A carrot no longer puts a wild rabbit in love mode, because it tames it. Golden carrots and
  dandelions still breed wild rabbits. A tamed rabbit is bred by golden carrots and dandelions only; a carrot
  never breeds it (or speeds up a tamed baby). Any of the three heals a hurt tamed rabbit by its nutrition
  value, a carrot included. A carrot on a whole tamed rabbit is not used up: it is an item like any other, so
  its owner's click makes it sit or stand, and another player's click does nothing.
- **Following.** A tamed rabbit follows its owner from 10 blocks away and teleports to them when 12 or more
  blocks away, like a wolf (`FollowOwner`), but hops about four times as fast as a plain follow (speed modifier
  2.2, the panic speed, which also shortens the pause between hops). It never runs from players.
- **Staying.** The owner right-clicks a tamed rabbit with anything that does not breed it (an empty hand, a
  carrot) to make it stay put, and right-clicks again to make it follow. Nobody else can. A hit makes a sitting
  rabbit stand again. A newly tamed rabbit starts out following, not sitting. A rabbit comes to a player
  holding its food, as any rabbit does (`Tempt`), until it is told to stay.
- **Offspring.** A baby of a tamed parent is tamed and has the same owner.
- **Saving.** `Owner` and `Sitting` are written to the rabbit's NBT only while it is tamed. A wild rabbit saves
  exactly as in vanilla, and `summon minecraft:rabbit` with `Owner` set makes a tamed one.

A stock client shows no collar and no sitting pose: a rabbit's entity-data slot 18 is its variant, so the tame
and sitting flags have no place to go in the packets a vanilla client reads. The hearts and the behaviour are
all a player sees.

Code: `mobs/tame.rs` (`add_pet_goals`), `mobs/rabbit.rs`, `game/creatures.rs` (mob interaction),
`entity_nbt.rs`; tests in `game/rabbit_pet_tests.rs` (they need `HORIZON_TEST_RESOURCES`, and run with
`--include-ignored`). There is no vanilla oracle for it, and the wild rabbit's behaviour does not change: the pet
goals do nothing without an owner.

## A player's own load and simulation distance

A native mod can cap one player's view distance (`Host::set_player_view_distance`; the `viewdistance` mod does it
by operator level). Vanilla keeps the server's view and simulation distances round every player whatever they
asked for. Here the cap is also how far the server keeps that player's chunks loaded and simulated, under the
server's view distance, which is the most any player can have.

- **Loaded and simulated.** A player capped at 8 holds 8 chunks loaded and 8 simulating (entities, random ticks,
  block ticks) in place of the server's two distances; an operator capped at 32 holds 32 and 32. Set the server's
  view distance to the largest cap (`server.view_distance=32`): only the operators reach it. A player with no cap
  has the server's view and simulation distances, as in vanilla.
- **Simulation follows the cap.** A capped player's simulation distance is their cap, whatever the server's
  `simulation_distance` says: it can be higher than the server's for an operator. `simulation_distance` still
  applies to players with no cap.
- **Cost.** Each operator at 32 holds a 65 by 65 chunk area loaded and simulating; players at 8 hold 17 by 17.

Code: `lan/hub.rs` (`capped_distance`, `loading_areas`), `game.rs` (`update_chunk_tickets`), `ticket_levels.rs`
(`Sources::limited`), `game/lan.rs` and `game/dedicated.rs` (the kept chunks); tests in `ticket_levels.rs` and
`lan/hub_tests.rs`. There is no vanilla oracle for it.
