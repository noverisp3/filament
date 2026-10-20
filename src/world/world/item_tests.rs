//! The items on the ground, and what happens to them.
//!
//! A broken block becomes an item that lies on the ground, and a player picks
//! it up by walking over it. These are the rules of that: the item falls until
//! it lands, it stays where it landed, and it goes away of its own accord if
//! nobody wants it.

use super::*;
use crate::world::chunk::MIN_BLOCK_Y;
use crate::world::item_entity::LIFETIME;
use crate::world::terrain::surface_y;

fn air(world: &World) -> BlockId {
    world.registry().id("air").expect("registered")
}

fn stone(world: &World) -> BlockId {
    world.registry().id("stone").expect("registered")
}

/// A world with a flat floor at `floor`, and nothing else in the way.
///
/// The floor is the solid block at `floor` itself, so a thing that lands on it
/// comes to rest at `floor + 1`.
fn world_with_a_floor(floor: i32) -> World {
    let mut world = World::generate(1);
    let air = air(&world);
    let stone = stone(&world);
    for y in MIN_BLOCK_Y..=floor {
        world.set_block_quiet(BlockPos::new(0, y, 0), stone);
    }
    for y in floor + 1..=floor + 4 {
        world.set_block_quiet(BlockPos::new(0, y, 0), air);
    }
    world
}

#[test]
fn a_dropped_item_falls_until_it_lands_on_something_solid() {
    let mut world = world_with_a_floor(64);

    world.drop_item(1, [7u8; 16], BlockPos::new(0, 68, 0), 55, 1);

    // It starts in the air and it falls.
    assert!(world.items()[0].falling, "an item in the air is still falling");
    let start = world.items()[0].y;

    for _ in 0..200 {
        world.tick();
    }

    let item = &world.items()[0];
    assert!(!item.falling, "an item that has landed is not falling");
    assert!(item.y < start, "and it fell while it was falling");
    assert_eq!(
        item.y, 65.0,
        "it comes to rest on top of the floor, which is at 64"
    );
}

#[test]
fn an_item_waits_on_the_ground_for_somebody_to_pick_it_up() {
    let mut world = world_with_a_floor(64);
    world.drop_item(1, [7u8; 16], BlockPos::new(0, 65, 0), 55, 1);

    // Standing on it is what picks it up, and standing a long way off is not.
    let near = world.items_near(0.5, 65.0, 0.5);
    assert_eq!(near.len(), 1, "an item under the player's feet is picked up");

    let far = world.items_near(0.5, 65.0, 40.0);
    assert!(
        far.is_empty(),
        "an item forty blocks away is left where it is"
    );
}

#[test]
fn taking_an_item_off_the_ground_takes_it_out_of_the_world() {
    let mut world = world_with_a_floor(64);
    world.drop_item(1, [7u8; 16], BlockPos::new(0, 65, 0), 55, 1);
    world.drop_item(2, [8u8; 16], BlockPos::new(0, 65, 0), 56, 1);

    let taken = world.take_item(1).expect("the item is there to be taken");
    assert_eq!(taken.item, 55);
    assert_eq!(
        world.items().len(),
        1,
        "and the world has one item less in it"
    );
    assert!(
        world.take_item(1).is_none(),
        "and it cannot be taken twice"
    );
}

#[test]
fn an_item_nobody_wants_goes_away_after_five_minutes() {
    let mut world = world_with_a_floor(64);
    world.drop_item(1, [7u8; 16], BlockPos::new(0, 65, 0), 55, 1);

    for _ in 0..LIFETIME - 1 {
        world.tick();
    }
    assert_eq!(world.items().len(), 1, "it is still there after five minutes");

    world.tick();
    assert!(
        world.items().is_empty(),
        "and it is gone a tick later, which is the game's own five minutes"
    );
}

#[test]
fn the_items_a_player_can_see_are_the_ones_near_them() {
    let mut world = world_with_a_floor(64);
    world.drop_item(1, [7u8; 16], BlockPos::new(0, 65, 0), 55, 1);
    world.drop_item(2, [8u8; 16], BlockPos::new(30, 65, 0), 56, 1);

    let near = world.items_within(0.0, 65.0, 0.0, 16.0);
    assert_eq!(near.len(), 1, "the item thirty blocks away is not in sight");

    let both = world.items_within(0.0, 65.0, 0.0, 64.0);
    assert_eq!(both.len(), 2, "and it is when the reach is wide enough");
}

#[test]
fn the_height_of_the_terrain_does_not_decide_where_an_item_lands() {
    // A flat world, so the test is about the item and not about the hills.
    let mut world = world_with_a_floor(64);
    let surface = surface_y(0, 0);
    assert!(
        surface < 64,
        "the generator put the surface below the floor this test built"
    );

    world.drop_item(1, [7u8; 16], BlockPos::new(0, 68, 0), 55, 1);
    for _ in 0..200 {
        world.tick();
    }
    assert_eq!(
        world.items()[0].y, 65.0,
        "the item lands on the floor the test built, not on the terrain"
    );
}
