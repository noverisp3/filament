//! Puts a lamp in a world folder, so the light of it can be watched.
//!
//!     cargo run --example place_lamp -- <world folder> <x> <z>
//!
//! The lamp goes on the surface at that column and the chunk is written back,
//! so the next server that opens the world finds it there and works out the
//! light around it. That is the light the client is given, and it is the only
//! way to watch a lamp that was already there: a client that places one
//! herself gets a light update instead, which is a different packet.
//!
//! The column matters. A lamp one block into a chunk starts the region the
//! light is worked out in inside the chunk before it, and a chunk that hangs
//! over the edge of that region covers only part of itself inside it -- which
//! is where the light used to be cut in two at the border.

use filament::world::pos::BlockPos;
use filament::world::storage::WorldStore;
use filament::world::World;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let folder = args.next().ok_or("a world folder")?;
    let x: i32 = args.next().ok_or("an x")?.parse()?;
    let z: i32 = args.next().ok_or("a z")?.parse()?;

    let store = WorldStore::open(folder)?;
    let mut world = World::with_store(Some(store));
    let glowstone = world
        .registry()
        .id("glowstone")
        .ok_or("the world knows a glowstone")?;

    let chunk_pos = BlockPos::new(x, 0, z).chunk_pos();
    world.ensure_chunk(chunk_pos);

    // The surface: the first solid block down from the sky.
    let mut ground = None;
    for y in (0..320).rev() {
        if world.is_solid(BlockPos::new(x, y, z)) {
            ground = Some(y);
            break;
        }
    }
    let ground = ground.ok_or("no ground in that column")?;
    let pos = BlockPos::new(x, ground + 1, z);

    world
        .set_block(pos, glowstone)
        .ok_or("the world would not take the lamp")?;
    let written = world.flush()?;

    println!("lamp at {pos:?}, chunk {chunk_pos:?}, {written} chunks written");
    Ok(())
}
