use super::*;
use std::collections::HashMap;

fn world() -> (BlockRegistry, TerrainBlocks) {
    let mut registry = BlockRegistry::new();
    let blocks = register_terrain(&mut registry);
    (registry, blocks)
}

#[test]
fn every_chunk_has_a_surface_and_a_floor() {
    let (_registry, blocks) = world();
    let chunk = generate_chunk(ChunkPos::new(0, 0), &blocks);

    for local_z in 0..16i32 {
        for local_x in 0..16i32 {
            let column = [
                chunk.block(BlockPos::new(local_x, MIN_BLOCK_Y, local_z)),
                chunk.block(BlockPos::new(local_x, 120, local_z)),
            ];
            assert_eq!(
                column[0],
                Some(blocks.stone),
                "no floor at {local_x},{local_z}"
            );
            assert_eq!(
                column[1],
                Some(blocks.air),
                "sky not clear at {local_x},{local_z}"
            );

            // Walking down from the sky: air until the surface, solid all the way
            // to the floor. Anything else means the column is not monotonic.
            let mut saw_solid = false;
            for y in (MIN_BLOCK_Y..=120).rev() {
                let block = chunk
                    .block(BlockPos::new(local_x, y, local_z))
                    .expect("in range");
                if block == blocks.air {
                    assert!(
                        !saw_solid,
                        "column at {local_x},{local_z} has air below y={y}"
                    );
                } else {
                    saw_solid = true;
                }
            }
            assert!(saw_solid, "column at {local_x},{local_z} is all air");
        }
    }
}

#[test]
fn terrain_is_deterministic() {
    let (_registry, blocks) = world();
    let a = generate_chunk(ChunkPos::new(3, -7), &blocks);
    let b = generate_chunk(ChunkPos::new(3, -7), &blocks);
    assert_eq!(a, b, "same chunk position must give the same terrain");
}

/// Chunk generation evaluates the height in world coordinates, so the same
/// column always gets the same surface no matter which chunk produced it.
#[test]
fn a_column_gets_the_same_surface_from_either_chunk() {
    let (_registry, blocks) = world();
    let left = generate_chunk(ChunkPos::new(0, 0), &blocks);
    let right = generate_chunk(ChunkPos::new(1, 0), &blocks);

    // World x = 15 belongs to the left chunk as local x = 15; world x = 31
    // belongs to the right chunk as local x = 15. Both must match the pure
    // function.
    for z in 0..16i32 {
        for (chunk, world_x) in [(&left, 15i32), (&right, 31i32)] {
            let mut found = None;
            for y in (MIN_BLOCK_Y..200).rev() {
                if chunk.block(BlockPos::new(world_x, y, z)) != Some(blocks.air) {
                    found = Some(y);
                    break;
                }
            }
            let expected = MIN_BLOCK_Y + height_at(world_x as f64, z as f64) as i32;
            assert_eq!(found, Some(expected), "world x={world_x} z={z}");
        }
    }
}

/// Terrain must be continuous across a chunk seam: the last column of one
/// chunk and the first of the next may differ by a step or two, never by a
/// wall.
#[test]
fn the_seam_between_chunks_is_continuous() {
    let (_registry, blocks) = world();
    let left = generate_chunk(ChunkPos::new(0, 0), &blocks);
    let right = generate_chunk(ChunkPos::new(1, 0), &blocks);

    let surface = |chunk: &Chunk, x: i32, z: i32| {
        (MIN_BLOCK_Y..200)
            .rev()
            .find(|&y| chunk.block(BlockPos::new(x, y, z)) != Some(blocks.air))
    };

    for z in 0..16i32 {
        let a = surface(&left, 15, z).expect("left edge column");
        let b = surface(&right, 16, z).expect("right edge column");
        assert!(
            a.abs_diff(b) <= 2,
            "wall at z={z}: left ends at y={a}, right starts at y={b}"
        );
    }
}

#[test]
fn a_small_world_is_mostly_stone_and_air() {
    let (_registry, blocks) = world();
    let mut counts: HashMap<BlockId, usize> = HashMap::new();
    for cx in 0..3 {
        for cz in 0..3 {
            let chunk = generate_chunk(ChunkPos::new(cx, cz), &blocks);
            for section in chunk.sections() {
                for block in section.blocks().iter() {
                    *counts.entry(block).or_default() += 1;
                }
            }
        }
    }
    let count = |id: BlockId| counts.get(&id).copied().unwrap_or(0);
    let solid = count(blocks.stone) + count(blocks.dirt) + count(blocks.grass);

    // The world is 384 blocks tall and the surface sits near y = 0, so most
    // of the volume is sky.
    assert!(
        count(blocks.air) > solid,
        "sky should dominate the column: air={} solid={solid}",
        count(blocks.air)
    );
    // Within the solid part, stone dominates: dirt and grass are only a
    // handful of layers.
    assert!(
        count(blocks.stone) > count(blocks.dirt),
        "stone={} dirt={}",
        count(blocks.stone),
        count(blocks.dirt)
    );
    assert!(
        count(blocks.dirt) > count(blocks.grass),
        "dirt={} grass={}",
        count(blocks.dirt),
        count(blocks.grass)
    );
}