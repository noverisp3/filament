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
    let pos = BlockPos::new(8, 64, 8);

    world.set_block(pos.offset(Direction::Down), terrain.grass);
    world.set_block(pos, terrain.dirt);
    world.set_block(pos.offset(Direction::Up), terrain.air);

    let owed = world.scheduled_ticks();
    assert!(
        owed > 0,
        "three edits left the world owing {} neighbour updates it has not told yet",
        owed
    );

    world.tick();
    assert!(
        world.scheduled_ticks() < owed,
        "and one tick started answering them: {} owed, was {}",
        world.scheduled_ticks(),
        owed
    );
}

#[test]
fn grass_comes_back_to_dirt_with_grass_under_it_and_air_over_it() {
    // The rule that makes a dug hillside green again instead of leaving a bare
    // stripe across it. It is the first thing the tick does here because it is
    // the first thing that needed the tick to exist.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let pos = BlockPos::new(8, 64, 8);

    world.set_block(pos.offset(Direction::Down), terrain.grass);
    world.set_block(pos, terrain.dirt);
    world.set_block(pos.offset(Direction::Up), terrain.air);

    for _ in 0..3 {
        world.tick();
    }

    assert_eq!(
        world.block(pos),
        Some(terrain.grass),
        "dirt under open air with grass below becomes grass"
    );
}

#[test]
fn dirt_that_does_not_earn_it_stays_dirt() {
    // The rule has three parts and dropping any one of them grows grass
    // everywhere, which is a green planet and a very visible bug.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let pos = BlockPos::new(8, 64, 8);

    // No grass underneath.
    world.set_block(pos.offset(Direction::Down), terrain.dirt);
    world.set_block(pos, terrain.dirt);
    world.set_block(pos.offset(Direction::Up), terrain.air);
    for _ in 0..3 {
        world.tick();
    }
    assert_eq!(world.block(pos), Some(terrain.dirt), "nothing under it");

    // Grass underneath, but something solid on top.
    world.set_block(pos.offset(Direction::Down), terrain.grass);
    world.set_block(pos.offset(Direction::Up), terrain.stone);
    for _ in 0..3 {
        world.tick();
    }
    assert_eq!(world.block(pos), Some(terrain.dirt), "and no air over it");
}

#[test]
fn grass_does_not_spread_sideways_or_downwards() {
    // Grass spreading is only ever onto dirt that has grass *below* it. A rule
    // that looked at any neighbour would turn the whole hillside green.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let grass = BlockPos::new(8, 64, 8);

    world.set_block(grass, terrain.grass);

    // Dirt beside it and dirt under it: neither has grass beneath.
    world.set_block(grass.offset(Direction::East), terrain.dirt);
    world.set_block(grass.offset(Direction::Down), terrain.dirt);
    for _ in 0..3 {
        world.tick();
    }

    assert_eq!(
        world.block(grass.offset(Direction::East)),
        Some(terrain.dirt),
        "dirt beside grass is not grass"
    );
    assert_eq!(
        world.block(grass.offset(Direction::Down)),
        Some(terrain.dirt),
        "nor is dirt under it"
    );
}

#[test]
fn a_tick_says_what_it_changed_so_somebody_can_be_told() {
    // A tick has no connection to look out of. If it changes something and does
    // not say so, the world the players are looking at quietly stops being the
    // world they are standing in, with nothing to point at.
    let mut world = test_world();
    let terrain = *world.terrain_blocks();
    let pos = BlockPos::new(8, 64, 8);

    world.set_block(pos.offset(Direction::Down), terrain.grass);
    world.set_block(pos, terrain.dirt);
    world.set_block(pos.offset(Direction::Up), terrain.air);

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
