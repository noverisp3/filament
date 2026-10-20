use super::*;

fn test_world() -> World {
    World::generate(1)
}

fn stone(world: &World) -> BlockId {
    world.registry().id("stone").expect("registered")
}

fn air(world: &World) -> BlockId {
    world.registry().id("air").expect("registered")
}

#[test]
fn a_tick_moves_the_clock_on_by_one() {
    let mut world = test_world();
    assert_eq!(world.game_time(), 0, "a new world has not ticked");
    assert_eq!(world.day_time(), 0);

    world.tick();
    assert_eq!(world.game_time(), 1);
    assert_eq!(world.day_time(), 1);

    for _ in 0..9 {
        world.tick();
    }
    assert_eq!(world.game_time(), 10, "ten ticks, ten");
}

#[test]
fn the_clock_keeps_counting_with_the_age() {
    // The game's own clock is a plain count: where the sun is comes out of it
    // client-side, a day at a time. A server that wrapped the count at day's
    // end sent a sky that stepped backwards once every twenty minutes, which
    // is the sort of thing nobody can talk you out of seeing.
    let mut world = test_world();
    for _ in 0..World::DAY_LENGTH {
        world.tick();
    }
    assert_eq!(
        world.day_time(),
        World::DAY_LENGTH,
        "a day on leaves the clock a day on, not back at dawn"
    );
    assert_eq!(
        world.game_time(),
        World::DAY_LENGTH,
        "and the age follows it"
    );

    for _ in 0..World::DAY_LENGTH * 2 {
        world.tick();
    }
    assert_eq!(world.day_time(), World::DAY_LENGTH * 3, "and it keeps going");
    assert_eq!(world.game_time(), World::DAY_LENGTH * 3);
}

#[test]
fn a_day_is_twenty_minutes() {
    // Twenty-four thousand ticks at twenty a second is the game, and it is
    // worth pinning because a day that is the wrong length is a sky that runs at
    // the wrong speed and nothing says so.
    assert_eq!(World::DAY_LENGTH, 24_000);
    assert_eq!(World::DAY_LENGTH / 20, 1_200, "which is twenty minutes");
}

#[test]
fn setting_a_block_schedules_its_neighbours_rather_than_telling_them_now() {
    // A block cannot know what happened next to it, so the change goes round.
    // Answering it inline would answer it before the change is finished.
    let mut world = test_world();
    let pos = BlockPos::new(8, 64, 8);
    let before = world.scheduled_ticks();
    world.set_block(pos, stone(&world));
    assert!(
        world.scheduled_ticks() > before,
        "putting a block down schedules work, {} -> {}",
        before,
        world.scheduled_ticks()
    );
}

#[test]
fn a_tick_answers_deferred_work_and_not_inline_work() {
    // Scheduled means scheduled. A rule that ran inline would run several times
    // inside one edit -- once per neighbour that noticed -- and the tick would
    // be a formality. The clock moves either way; what it does about it is what
    // is being measured.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let pos = BlockPos::new(8, 120, 8);

    // A grass block with nothing under it: the world owes it an answer, and
    // the answer is not given here.
    world.set_block(pos.offset(Direction::Down), terrain.air);
    world.set_block(pos, terrain.grass);
    assert_eq!(
        world.block(pos),
        Some(terrain.grass),
        "not yet: the answer is owed"
    );

    world.tick();
    assert_eq!(
        world.block(pos),
        Some(terrain.dirt),
        "and the tick is what gives it"
    );
}

#[test]
fn grass_with_nothing_under_it_turns_to_dirt() {
    // The block below is what a grass block stands on, and when that is gone
    // the grass has nothing to be grass about. The game answers this at once,
    // and so does this: a hole dug under a hillside shows dirt.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let pos = BlockPos::new(8, 120, 8);

    world.set_block(pos.offset(Direction::Down), terrain.air);
    world.set_block(pos, terrain.grass);

    world.tick();

    assert_eq!(
        world.block(pos),
        Some(terrain.dirt),
        "a grass block with no dirt under it is dirt"
    );
}

#[test]
fn grass_that_stands_on_dirt_stays_grass() {
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let pos = BlockPos::new(8, 120, 8);

    world.set_block(pos.offset(Direction::Down), terrain.dirt);
    world.set_block(pos, terrain.grass);

    for _ in 0..3 {
        world.tick();
    }

    assert_eq!(
        world.block(pos),
        Some(terrain.grass),
        "grass on dirt is grass"
    );
}

#[test]
fn dirt_put_down_on_grass_stays_dirt() {
    // The rule that used to be here was written the other way round: dirt with
    // grass under it became grass, at once, and every block a player put down
    // next to a hillside turned into a lawn.
    //
    // The game's rule is the other way round and much slower: grass spreads
    // onto dirt near it, and only when the game rolls for it.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let pos = BlockPos::new(8, 120, 8);

    world.set_block(pos.offset(Direction::Down), terrain.grass);
    world.set_block(pos, terrain.dirt);

    for _ in 0..5 {
        world.tick();
    }

    assert_eq!(
        world.block(pos),
        Some(terrain.dirt),
        "dirt put down on grass is still dirt"
    );
}

#[test]
fn grass_spreads_onto_dirt_beside_it_when_the_game_rolls_for_it() {
    // The game's own rule: a grass block spreads onto a dirt block near it,
    // when the game rolls for it and when the place is bright enough.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let grass = BlockPos::new(8, 120, 8);
    let beside = grass.offset(Direction::East);

    world.set_block(grass.offset(Direction::Down), terrain.dirt);
    world.set_block(grass, terrain.grass);
    world.set_block(beside, terrain.dirt);
    world.set_block(beside.offset(Direction::Up), terrain.air);

    // Before the roll, the dirt is dirt.
    for _ in 0..3 {
        world.tick();
    }
    assert_eq!(world.block(beside), Some(terrain.dirt), "not yet");

    // And the roll is what makes it grass.
    assert!(
        world.sky_light(beside.offset(Direction::Up)) >= 9,
        "the place is bright enough: {}",
        world.sky_light(beside.offset(Direction::Up))
    );
    let mut edit = crate::world::world::BlockEdit {
        chunks: Vec::new(),
        sections: Vec::new(),
    };
    world.random_update(beside, &mut edit);

    assert_eq!(
        world.block(beside),
        Some(terrain.grass),
        "dirt beside grass becomes grass when the game rolls for it"
    );
}

#[test]
fn a_tick_says_what_it_changed_so_somebody_can_be_told() {
    // A tick has no connection to look out of. If it changes something and does
    // not say so, the world the players are looking at quietly stops being the
    // world they are standing in, with nothing to point at.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let pos = BlockPos::new(8, 120, 8);

    world.set_block(pos.offset(Direction::Down), terrain.air);
    world.set_block(pos, terrain.grass);

    assert!(
        world.pending_tick_changes() == 0,
        "nothing has ticked yet, so nothing has changed on its own"
    );

    for _ in 0..3 {
        world.tick();
    }

    let changes = world.take_tick_changes();
    assert!(
        changes.iter().any(|(at, _)| *at == pos),
        "the block that turned is named, and it is the one that turned"
    );

    assert!(
        world.take_tick_changes().is_empty(),
        "and taking them drains them, so nobody is told twice"
    );
}

#[test]
fn the_world_does_not_remember_that_it_has_nothing_left_to_say() {
    // The complement of the above, and the one that catches a drain that copies
    // instead of taking: a change nobody has collected grows without bound.
    let mut world = test_world();
    for _ in 0..20 {
        world.tick();
    }
    assert_eq!(world.pending_tick_changes(), 0, "twenty idle ticks, nothing");
}
