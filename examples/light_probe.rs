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

    // Every block in the chunk that gives off light, and how much: this is
    // where a lamp that the server does not know about shows up.
    println!("\nthe blocks that give off light in this chunk:");
    let mut found = std::collections::BTreeMap::new();
    let base_x = pos.chunk_pos().x * 16;
    let base_z = pos.chunk_pos().z * 16;    for dx in 0..16 {
        for dz in 0..16 {
            for y in -64..320 {
                let at = BlockPos::new(base_x + dx, y, base_z + dz);
                let Some(block) = world.block(at) else { continue };
                let light = world.registry().light_emission(block);
                if light > 0 {
                    *found
                        .entry((world.registry().name(block).to_string(), light))
                        .or_insert(0) += 1;
                    if *found
                        .entry((world.registry().name(block).to_string(), light))
                        .or_insert(0)
                        <= 3
                    {
                        println!(
                            "  {name} at {} {} {} emits {light}, and the light there is {}",
                            at.x,
                            at.y,
                            at.z,
                            world.block_light(at),
                            name = world.registry().name(block),
                        );
                    }
                }
            }
        }
    }
    for ((name, light), count) in found {
        println!("  {name}: {light}, in {count} places");
    }

    // Every kind of block in the chunk, so a block that is there and is not
    // counted shows up.
    println!("\nthe blocks in this chunk:");
    let mut kinds = std::collections::BTreeMap::new();
    let base_x = pos.chunk_pos().x * 16;
    let base_z = pos.chunk_pos().z * 16;
    for dx in 0..16 {
        for dz in 0..16 {
            for y in -64..320 {
                let at = BlockPos::new(base_x + dx, y, base_z + dz);
                let Some(block) = world.block(at) else { continue };
                *kinds.entry(world.registry().name(block).to_string()).or_insert(0) += 1;
            }
        }
    }
    for (name, count) in kinds {
        println!("  {name}: {count}");
    }

    // Whether the chunk still carries the body it was given when it was
    // built. The body is a copy of the chunk made before the world worked out
    // its light, so a body that is still there after the light was computed
    // is a body that describes a world without light in it.
    let chunk = world.chunk_mut(pos.chunk_pos()).expect("the chunk is there");
    println!(
        "\nthe chunk still carries the body it was built with: {}",
        chunk.body_is_current()
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
