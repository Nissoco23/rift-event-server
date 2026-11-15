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
  dandelions still breed wild rabbits. A tamed rabbit breeds with any of the three, unless it is hurt: then the
  food heals it by its nutrition value instead.
- **Following.** A tamed rabbit follows its owner and teleports to them when 12 or more blocks away, like a
  wolf (`FollowOwner`). It never runs from players.
- **Staying.** The owner right-clicks a tamed rabbit with an item that is not its food (an empty hand works) to
  make it stay put, and right-clicks again to make it follow. Nobody else can. A hit makes a sitting rabbit stand
  again. A newly tamed rabbit starts out following, not sitting.
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
