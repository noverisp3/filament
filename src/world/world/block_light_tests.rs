//! The light the blocks themselves give off.
//!
//! The sun is one layer of light and the lamps are another, and they are kept
//! apart: a lamp in a sunlit room is not brighter than the room, and a lamp in
//! a cave is the only light there is.

use super::*;
use crate::world::chunk::MIN_BLOCK_Y;
use crate::world::light::MAX_LIGHT;
use crate::world::terrain::height_at;

/// Surface height of the generated terrain, without needing the world.
fn surface_y(x: i32, z: i32) -> i32 {
    MIN_BLOCK_Y + height_at(x as f64, z as f64) as i32
}

/// Puts a block that glows into the world, and gives back where it went.
fn place_glowstone(world: &mut World, x: i32, y: i32, z: i32) -> BlockPos {
    let glowstone = world
        .registry()
        .id("glowstone")
        .expect("the world knows a glowstone");
    let pos = BlockPos::new(x, y, z);
    world
        .set_block(pos, glowstone)
        .expect("the block went in");
    pos
}

#[test]
fn a_block_that_glows_lights_the_air_around_it() {
    let mut world = World::generate(1);
    let surface = surface_y(8, 8);

    // The lamp itself is as bright as the lamp is.
    place_glowstone(&mut world, 8, surface + 1, 8);

    assert_eq!(
        world.block_light(BlockPos::new(8, surface + 1, 8)),
        MAX_LIGHT,
        "the block that glows is lit as brightly as it glows"
    );
    // And the air next to it is one level less, and the air after that one
    // level less again: light loses one level for every block it crosses.
    assert_eq!(
        world.block_light(BlockPos::new(9, surface + 1, 8)),
        MAX_LIGHT - 1,
        "the cell next to the lamp is one level less"
    );
    assert_eq!(
        world.block_light(BlockPos::new(10, surface + 1, 8)),
        MAX_LIGHT - 2,
        "and the cell after that is one level less again"
    );
    assert_eq!(
        world.block_light(BlockPos::new(11, surface + 1, 8)),
        MAX_LIGHT - 3,
        "and so on, until the light runs out"
    );
    // And it keeps going for the whole fifteen blocks the game allows, which
    // is the difference between a lamp that lights a room and a lamp that
    // lights a chair.
    assert_eq!(
        world.block_light(BlockPos::new(8 + 14, surface + 1, 8)),
        MAX_LIGHT - 14,
        "the light reaches fifteen blocks"
    );
    assert_eq!(
        world.block_light(BlockPos::new(8 + 15, surface + 1, 8)),
        0,
        "and no further"
    );
}

#[test]
fn the_light_of_a_lamp_does_not_go_through_the_ground() {
    let mut world = World::generate(1);
    let surface = surface_y(8, 8);

    place_glowstone(&mut world, 8, surface + 1, 8);

    // The ground is solid, and light does not pass through solid ground.
    assert_eq!(
        world.block_light(BlockPos::new(8, surface - 1, 8)),
        0,
        "the ground under the lamp is dark"
    );
}

#[test]
fn the_light_the_client_is_told_after_a_placement_is_the_whole_pool() {
    // The client is told about the light that moved, and what it is told has to
    // be the whole pool. A smaller pool is a client that draws a lamp lighting
    // a chair.
    let mut world = World::generate(1);
    let glowstone = world
        .registry()
        .id("glowstone")
        .expect("the world knows a glowstone");
    let pos = BlockPos::new(8, 120, 8);

    world.set_block(pos, glowstone);

    // What the server would send: the sections whose light moved.
    let changes = world.take_light_changes();
    assert!(
        !changes.is_empty(),
        "placing a lamp moves light, and the client has to be told"
    );

    // And the light in the section the lamp is in is the whole pool.
    assert!(
        changes
            .iter()
            .any(|s| s.y == pos.section_pos().y && s.chunk_pos() == pos.chunk_pos()),
        "the lamp's own section is among the changes"
    );
    for distance in 1..=14u8 {
        let at = BlockPos::new(8 + distance as i32, 120, 8);
        // The light is in the section of the block it is in, and a lamp eight
        // blocks from the edge of its chunk lights the next one.
        let chunk = world.chunk(at.chunk_pos()).expect("the chunk is there");
        let light = chunk
            .section_light(at.section_pos())
            .expect("the section is there");
        assert_eq!(
            light.block(at.local_pos()),
            MAX_LIGHT - distance,
            "the light at {distance} blocks is what the client will draw"
        );
    }
}

#[test]
fn a_lamp_lights_across_a_chunk_boundary() {
    // The bug this catches: a chunk that hangs over the edge of the region the
    // light is worked out in was refused whole, so the cells it covers inside
    // the region were never read, and the light stopped dead at the chunk
    // boundary.
    //
    // A lamp near the edge of a chunk therefore lit a pool that ended in a
    // straight line, and the same lamp three blocks further on lit a bigger
    // one, and the map looked like a fault in the wiring.
    let mut world = World::generate(2);
    let glowstone = world
        .registry()
        .id("glowstone")
        .expect("the world knows a glowstone");

    // Two blocks from the edge of the chunk at x = 0, so the light has to
    // cross into the next one.
    let pos = BlockPos::new(14, 100, 8);
    world.set_block(pos, glowstone);

    assert_eq!(
        world.block_light(BlockPos::new(15, 100, 8)),
        14,
        "the cell across the chunk boundary is lit"
    );
    assert_eq!(
        world.block_light(BlockPos::new(20, 100, 8)),
        9,
        "and the light carries on past it"
    );
    assert_eq!(
        world.block_light(BlockPos::new(14, 100, 9)),
        14,
        "and across the other way"
    );
}

#[test]
fn a_lamp_lights_across_a_chunk_boundary_in_z() {
    // The same in the other direction: the boundary is a line, not a point.
    let mut world = World::generate(2);
    let glowstone = world
        .registry()
        .id("glowstone")
        .expect("the world knows a glowstone");

    let pos = BlockPos::new(8, 100, 14);
    world.set_block(pos, glowstone);

    assert_eq!(
        world.block_light(BlockPos::new(8, 100, 15)),
        14,
        "the cell across the chunk boundary is lit"
    );
    assert_eq!(
        world.block_light(BlockPos::new(8, 100, 20)),
        9,
        "and the light carries on past it"
    );
}

#[test]
fn a_lamp_that_is_taken_away_takes_its_light_with_it() {
    let mut world = World::generate(1);
    let surface = surface_y(8, 8);

    place_glowstone(&mut world, 8, surface + 1, 8);
    assert_eq!(
        world.block_light(BlockPos::new(9, surface + 1, 8)),
        MAX_LIGHT - 1
    );

    // And the light goes out again when the lamp is broken.
    world.break_block(BlockPos::new(8, surface + 1, 8));

    assert_eq!(
        world.block_light(BlockPos::new(8, surface + 1, 8)),
        0,
        "a broken lamp gives off no light"
    );
    assert_eq!(
        world.block_light(BlockPos::new(9, surface + 1, 8)),
        0,
        "and the cell that was lit by it is dark again"
    );
}

#[test]
fn the_lamp_reaches_downwards_too() {
    let mut world = World::generate(1);
    let surface = surface_y(8, 8);

    // A hole dug under the lamp, with the lamp at the bottom of it.
    let air = world.registry().id("air").expect("the world knows air");
    for y in [surface - 1, surface - 2, surface - 3] {
        world
            .set_block(BlockPos::new(8, y, 8), air)
            .expect("the hole was dug");
    }
    place_glowstone(&mut world, 8, surface - 3, 8);

    assert_eq!(
        world.block_light(BlockPos::new(8, surface - 2, 8)),
        MAX_LIGHT - 1,
        "the cell above the lamp is lit"
    );
    assert_eq!(
        world.block_light(BlockPos::new(8, surface - 1, 8)),
        MAX_LIGHT - 2,
        "and the one above that is one level less"
    );
}
