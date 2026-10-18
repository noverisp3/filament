//! A small deterministic terrain generator, used to feed the renderer.
//!
//! This exists so phase 3 has something with a real surface to cull and mesh.
//! It is not meant to survive: there is no seed, no biome system, and the noise
//! is a two-line value noise. Phase 4 and later replace it.

use crate::world::block::{BlockId, BlockRegistry};
use crate::world::chunk::{Chunk, MIN_BLOCK_Y};
use crate::world::pos::{BlockPos, ChunkPos};

/// Block ids the terrain is built from.
#[derive(Clone, Copy, Debug)]
pub struct TerrainBlocks {
    pub air: BlockId,
    pub grass: BlockId,
    pub dirt: BlockId,
    pub stone: BlockId,
    /// Not part of the generated terrain: the player places it. Registered with
    /// the terrain palette because block ids are fixed by registration order, so
    /// adding it later would renumber everything already stored.
    pub glowstone: BlockId,
}

/// Registers the terrain palette. Ids are fixed by registration order, so the
/// order here is part of the format.
pub fn register_terrain(registry: &mut BlockRegistry) -> TerrainBlocks {
    TerrainBlocks {
        air: registry.register("air", false, [0.0, 0.0, 0.0]),
        grass: registry.register("grass", true, [0.36, 0.62, 0.28]),
        dirt: registry.register("dirt", true, [0.45, 0.32, 0.20]),
        stone: registry.register("stone", true, [0.50, 0.50, 0.52]),
        // A full opaque cube, so it lights the world without asking the mesher
        // to understand a new block shape. Bright on all 15 levels.
        glowstone: registry.register_emitting("glowstone", true, [0.96, 0.85, 0.55], 15),
    }
}

/// Builds one chunk of terrain.
///
/// Height comes from smooth value noise over two octaves, so the surface has
/// broad hills with some bumpiness. Everything below the surface is solid and
/// everything above is air, which is what makes greedy meshing worth doing:
/// large flat regions collapse into single quads.
pub fn generate_chunk(pos: ChunkPos, blocks: &TerrainBlocks) -> Chunk {
    let mut chunk = Chunk::uniform(pos, blocks.air);

    let origin_x = pos.x as f64 * 16.0;
    let origin_z = pos.z as f64 * 16.0;

    for local_z in 0..16i32 {
        for local_x in 0..16i32 {
            let world_x = origin_x + local_x as f64;
            let world_z = origin_z + local_z as f64;

            let surface = MIN_BLOCK_Y + height_at(world_x, world_z) as i32;
            let column = BlockPos::new(pos.x * 16 + local_x, 0, pos.z * 16 + local_z);

            for y in MIN_BLOCK_Y..=surface {
                chunk.set_block(
                    BlockPos::new(column.x, y, column.z),
                    block_for_depth(blocks, y, surface),
                );
            }
        }
    }

    chunk
}

/// Surface height in blocks above [`MIN_BLOCK_Y`], roughly 48..88.
///
/// Two octaves: broad hills plus finer bumpiness. Note that the noise has to be
/// scaled before quantising to an integer block height -- taking `(noise * k) as
/// u32` on a value that only spans 0..1 collapses almost every column onto the
/// same height, which flattens the world into a single plate.
pub fn height_at(world_x: f64, world_z: f64) -> u32 {
    let broad = value_noise(world_x / 70.0, world_z / 70.0);
    let detail = value_noise(world_x / 17.0 + 31.7, world_z / 17.0 - 17.3);
    let combined = broad * 0.68 + detail * 0.32;
    (combined * 40.0) as u32 + 48
}

/// The topmost solid block in a column, which is the block a player stands on.
///
/// Not a height above sea level and not a position: the *top solid* y. A
/// player's feet go one above this, because this block occupies the space from
/// this y up to the next one.
pub fn surface_y(world_x: f64, world_z: f64) -> i32 {
    MIN_BLOCK_Y + height_at(world_x, world_z) as i32
}

/// Where a player stands on arrival, in world coordinates.
///
/// Feet, which is one above the top solid block, and in the middle of a block
/// rather than on the seam between four. This is the same number the client is
/// told and the same number the other players are told this player is at; if
/// those three disagreed the player would arrive in one place and be seen in
/// another, which is a bug that looks like a rendering fault.
///
/// It used to be a constant at y=64, which is forty to eighty blocks above the
/// ground this terrain actually produces. Players fell on arrival, and the fall
/// was the second bug rather than the first: see the delta range in
/// [`crate::server::broadcast`].
pub fn spawn_point() -> (f64, f64, f64) {
    const X: f64 = 0.5;
    const Z: f64 = 0.5;
    (X, surface_y(X, Z) as f64 + 1.0, Z)
}

/// Grass on top, a few blocks of dirt, stone below that.
fn block_for_depth(blocks: &TerrainBlocks, y: i32, surface: i32) -> BlockId {
    match surface - y {
        0 => blocks.grass,
        1..=3 => blocks.dirt,
        _ => blocks.stone,
    }
}

/// Smooth value noise in 0..1 from a hashed integer lattice.
fn value_noise(x: f64, z: f64) -> f64 {
    let x0 = x.floor();
    let z0 = z.floor();
    let fx = smoothstep(x - x0);
    let fz = smoothstep(z - z0);

    let (xi, zi) = (x0 as i64, z0 as i64);

    let c00 = corner(xi, zi);
    let c10 = corner(xi + 1, zi);
    let c01 = corner(xi, zi + 1);
    let c11 = corner(xi + 1, zi + 1);

    let top = lerp(c00, c10, fx);
    let bottom = lerp(c01, c11, fx);
    lerp(top, bottom, fz)
}

fn corner(x: i64, z: i64) -> f64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
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
}

/// Height variation is what gives the mesher something to do and the player
/// something to look at. A flat plate passes every other test, so this needs
/// asserting explicitly.
#[test]
fn terrain_actually_has_relief() {
    let mut heights = std::collections::BTreeSet::new();
    for step in 0..40 {
        let h = height_at(step as f64 * 9.0, (step * 7 % 23) as f64 * 9.0);
        heights.insert(h);
    }
    assert!(
        heights.len() >= 8,
        "only {} distinct heights across 40 samples: {heights:?}",
        heights.len()
    );
    let lowest = *heights.iter().next().expect("non-empty");
    let highest = *heights.iter().next_back().expect("non-empty");
    assert!(
        highest - lowest >= 12,
        "relief of only {} blocks is too flat: {heights:?}",
        highest - lowest
    );

    // Adjacent columns must differ by at most a step or two, or the surface
    // becomes a staircase of cliffs. Sample every single block so a steep hill
    // is not mistaken for a discontinuity.
    let mut previous = height_at(0.0, 0.0);
    for x in 1..400i32 {
        let h = height_at(x as f64, 0.0);
        assert!(
            h.abs_diff(previous) <= 2,
            "height jumped from {previous} to {h} between x={} and x={x}",
            x - 1
        );
        previous = h;
    }
}
