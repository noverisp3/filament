//! Prints the light around a lamp, across a chunk border.
//!
//!     cargo run --example border_probe
//!
//! The point is to see the shape of the light the server works out, so that a
//! one-block gap at a chunk border can be pinned on the server rather than on
//! what the client draws.

use filament::world::pos::BlockPos;
use filament::world::world::World;

fn main() {
    let mut world = World::generate(2);
    let glowstone = world
        .registry()
        .id("glowstone")
        .expect("the world knows a glowstone");

    // The lamp two blocks from the edge of the chunk at x = 0, so the light
    // has to cross into the chunk at x = 1.
    let pos = BlockPos::new(14, 100, 8);
    world.set_block(pos, glowstone);

    println!("lamp at {pos:?}");
    println!("block light along x at y=100, z=8:");
    for x in 0..32 {
        let at = BlockPos::new(x, 100, 8);
        let chunk = if x < 16 { 0 } else { 1 };
        print!("x={x:2} (chunk {chunk}) light={:2}", world.block_light(at));
        if x % 4 == 3 {
            println!();
        } else {
            print!("   ");
        }
    }
    println!();

    println!("block light along z at y=100, x=8:");
    for z in 0..32 {
        let at = BlockPos::new(8, 100, z);
        let chunk = if z < 16 { 0 } else { 1 };
        print!("z={z:2} (chunk {chunk}) light={:2}", world.block_light(at));
        if z % 4 == 3 {
            println!();
        } else {
            print!("   ");
        }
    }
    println!();
}
