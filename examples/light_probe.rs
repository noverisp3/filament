//! Prints the block light around a position in a world folder.
//!
//!     cargo run --example light_probe -- <world-folder> <x> <y> <z>
//!
//! Used to find out what the light engine does in a world that came from a
//! file, which is not the same as what it does in one it made itself.

use std::path::Path;

use filament::world::pos::{BlockPos, ChunkPos};
use filament::world::storage::WorldStore;
use filament::world::world::World;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args.get(1).cloned().unwrap_or_default();
    let x: i32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
    let y: i32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
    let z: i32 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);

    let store = WorldStore::open(Path::new(&root)).expect("the world folder");
    let mut world = World::with_store(Some(store));
    let pos = BlockPos::new(x, y, z);
    for cx in pos.chunk_pos().x - 2..=pos.chunk_pos().x + 2 {
        for cz in pos.chunk_pos().z - 2..=pos.chunk_pos().z + 2 {
            world.ensure_chunk(ChunkPos::new(cx, cz));
        }
    }
    println!(
        "chunks around {:?} loaded, {} blocks registered",
        pos.chunk_pos(),
        world.registry_len()
    );

    let glow = world
        .registry()
        .id("minecraft:glowstone")
        .or_else(|| world.registry().id("glowstone"))
        .expect("the world knows a glowstone");
    println!(
        "glowstone is block {:?}, emission {}",
        glow,
        world.registry().light_emission(glow)
    );

    world.set_block(pos, glow);
    println!("placed at {x} {y} {z}");

    println!("\nthe slice of light around the lamp (rows are z, columns are x):");
    for dz in -6..=6i32 {
        let row: Vec<String> = (-6..=6i32)
            .map(|dx| {
                format!("{:>2}", world.block_light(BlockPos::new(x + dx, y, z + dz)))
            })
            .collect();
        println!("  {}", row.join(" "));
    }

    let east = |world: &World| {
        (0..8)
            .map(|d| world.block_light(BlockPos::new(x + d, y, z)))
            .collect::<Vec<_>>()
    };
    println!("\nafter the edit, to the east: {:?}", east(&world));

    world.compute_light();
    println!("after a full pass, to the east: {:?}", east(&world));
    println!("and down: {:?}", (0..8).map(|d| world.block_light(BlockPos::new(x, y - d, z))).collect::<Vec<_>>());

    println!("\nlight to the east:");
    for d in 0..16 {
        let at = BlockPos::new(x + d, y, z);
        println!("  {d:>2}: {}", world.block_light(at));
    }
    println!("\nlight downwards:");
    for d in 0..16 {
        let at = BlockPos::new(x, y - d, z);
        println!("  {d:>2}: {}", world.block_light(at));
    }
    println!("\nthe block below the lamp: {:?}", world.block(pos.offset(filament::world::pos::Direction::Down)));
    println!("the block above the lamp: {:?}", world.block(pos.offset(filament::world::pos::Direction::Up)));
}
