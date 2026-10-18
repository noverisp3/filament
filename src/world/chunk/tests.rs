use super::*;
use crate::world::block::BlockRegistry;

fn ids() -> (BlockRegistry, BlockId, BlockId) {
    let mut registry = BlockRegistry::new();
    let air = registry.register("air", false, [0.0, 0.0, 0.0]);
    let stone = registry.register("stone", true, [0.50, 0.50, 0.52]);
    (registry, air, stone)
}

#[test]
fn empty_chunk_is_free() {
    let (_registry, _air, stone) = ids();
    let chunk = Chunk::uniform(ChunkPos::new(0, 0), stone);
    assert_eq!(chunk.nbytes(), 0);
    assert_eq!(chunk.sections().len(), SECTIONS_PER_CHUNK);
}

#[test]
fn world_writes_land_in_the_right_section() {
    let (_registry, air, stone) = ids();
    let mut chunk = Chunk::uniform(ChunkPos::new(0, 0), air);

    // Crosses every section boundary and both signs of Y.
    for y in [
        MIN_BLOCK_Y,
        -17,
        -16,
        -1,
        0,
        1,
        15,
        16,
        MIN_BLOCK_Y + WORLD_HEIGHT - 1,
    ] {
        assert!(chunk.set_block(BlockPos::new(5, y, 7), stone), "y={y}");
        assert_eq!(chunk.block(BlockPos::new(5, y, 7)), Some(stone), "y={y}");
        assert_eq!(
            chunk.block(BlockPos::new(6, y, 7)),
            Some(air),
            "y={y} neighbour"
        );
    }

    // One past the top, and one below the bottom.
    assert!(!chunk.set_block(BlockPos::new(0, MIN_BLOCK_Y + WORLD_HEIGHT, 0), stone));
    assert!(!chunk.set_block(BlockPos::new(0, MIN_BLOCK_Y - 1, 0), stone));
    assert_eq!(
        chunk.block(BlockPos::new(0, MIN_BLOCK_Y + WORLD_HEIGHT, 0)),
        None
    );

    // X and Z outside this chunk must be rejected, not silently redirected
    // into a neighbouring column.
    assert!(!chunk.set_block(BlockPos::new(16, 0, 0), stone));
    assert!(!chunk.set_block(BlockPos::new(-1, 0, 0), stone));
    assert_eq!(chunk.block(BlockPos::new(16, 0, 0)), None);
}

#[test]
fn only_the_written_section_grows() {
    let (_registry, air, stone) = ids();
    let mut chunk = Chunk::uniform(ChunkPos::new(0, 0), air);
    chunk.set_block(BlockPos::new(1, 0, 1), stone);

    let grown = chunk
        .sections()
        .iter()
        .filter(|section| !section.is_uniform())
        .count();
    assert_eq!(grown, 1, "one write must not touch 23 other sections");
}

#[test]
fn negative_chunk_coordinates_keep_local_offsets() {
    let (_registry, air, stone) = ids();
    let mut chunk = Chunk::uniform(ChunkPos::new(-1, -1), air);

    let corner = BlockPos::new(-16, 0, -16);
    assert!(chunk.set_block(corner, stone));
    assert_eq!(chunk.block(corner), Some(stone));

    // The very last block of the same chunk.
    let far_corner = BlockPos::new(-1, 0, -1);
    assert_eq!(chunk.pos(), ChunkPos::new(-1, -1));
    assert!(chunk.set_block(far_corner, stone));
    assert_eq!(chunk.block(far_corner), Some(stone));

    // And a block that belongs to the neighbouring chunk.
    assert_eq!(chunk.block(BlockPos::new(0, 0, -1)), None);
}

#[test]
fn every_block_position_round_trips_through_local_and_section() {
    let (_registry, air, stone) = ids();
    let mut chunk = Chunk::uniform(ChunkPos::new(-3, 7), air);

    for x in 0..64i32 {
        for z in 0..64i32 {
            let world = BlockPos::new(x, 40, z);
            let chunk_pos = world.chunk_pos();
            if chunk_pos != chunk.pos() {
                continue;
            }
            assert!(chunk.set_block(world, stone));
            assert_eq!(chunk.block(world), Some(stone), "{world:?}");

            let section_pos = world.section_pos();
            let local = world.local_pos();
            assert_eq!(section_pos.min_block_pos().x + local.x as i32, x);
            assert_eq!(section_pos.min_block_pos().z + local.z as i32, z);
        }
    }
}

#[test]
fn compact_shrinks_a_chunk_after_blocks_are_removed() {
    let (_registry, air, stone) = ids();
    let mut chunk = Chunk::uniform(ChunkPos::new(0, 0), air);

    for x in 0..16i32 {
        for z in 0..16i32 {
            chunk.set_block(BlockPos::new(x, 0, z), stone);
        }
    }
    assert!(chunk.nbytes() > 0);

    for x in 0..16i32 {
        for z in 0..16i32 {
            chunk.set_block(BlockPos::new(x, 0, z), air);
        }
    }
    chunk.compact();

    assert_eq!(
        chunk.nbytes(),
        0,
        "a chunk back to all air must cost nothing"
    );
}
#[test]
fn a_body_is_written_once_and_kept() {
    // The whole point of the cache: the world's own thread does not write a
    // chunk out again for every player that walks into it.
let (registry, _air, stone) = ids();
    let _ = &registry;
    let pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::uniform(pos, stone);

    let expected = chunk.nbytes().to_le_bytes().to_vec();
    let mut writes = 0;
    let body = chunk
        .body(|c| {
            writes += 1;
            c.nbytes().to_le_bytes().to_vec()
        })
        .to_vec();
    assert_eq!(writes, 1, "the first ask writes");
    assert_eq!(body, expected);

    let again = chunk.body(|_| { writes += 1; Vec::new() }).to_vec();
    assert_eq!(writes, 1, "and the second ask does not");
    assert_eq!(again, body, "it is the same body");
}

#[test]
fn editing_a_chunk_throws_the_body_away() {
    // The failure this guards is the ghost block coming back by a different
    // road: a chunk written out, then dug in, then served from the cache with
    // the dig not in it. The block would be whole on the wire and gone in the
    // world, which is the fault that took a bisect to find.
    let (registry, air, stone) = ids();
    let _ = &registry;
    let pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::uniform(pos, stone);
    let block = BlockPos::new(0, 0, 0);

    let before = chunk.body(|c| c.nbytes().to_le_bytes().to_vec()).to_vec();
    assert!(chunk.body_is_current(), "written");

    assert!(chunk.set_block(block, air), "the edit took");
    assert!(!chunk.body_is_current(), "and the body is no longer current");

    let after = chunk.body(|c| c.nbytes().to_le_bytes().to_vec()).to_vec();
    assert_ne!(before, after, "so the next ask writes it again");
    assert!(chunk.body_is_current());
}

