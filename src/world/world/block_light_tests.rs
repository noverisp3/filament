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
