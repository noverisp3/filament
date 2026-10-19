use super::*;
use crate::world::chunk::MIN_BLOCK_Y;
use crate::world::pos::LocalPos;

fn test_world() -> World {
    World::generate(1)
}

fn stone(world: &World) -> BlockId {
    world.registry().id("stone").expect("registered")
}

fn air(world: &World) -> BlockId {
    world.registry().id("air").expect("registered")
}

/// Builds a world with a single known block, for deterministic tests.
fn world_with_one(radius: i32, chunk: ChunkPos, local: LocalPos) -> (World, BlockId) {
    let mut world = World::generate(radius);
    let stone = stone(&world);
    world
        .set_block(
            BlockPos::new(
                chunk.x * SECTION_DIM + local.x as i32,
                local.y as i32,
                chunk.z * SECTION_DIM + local.z as i32,
            ),
            stone,
        )
        .expect("inside the world");
    (world, stone)
}

/// The same world with a three-block pocket around `pos`, so generated
/// terrain cannot decide whether a cell is free.
fn pocket_world(pos: BlockPos) -> (World, BlockId, BlockId) {
    let mut world = World::generate(1);
    let stone = stone(&world);
    let air = air(&world);

    // Clear a column so the tests are not fighting the terrain generator.
    for y in pos.y - 2..=pos.y + 2 {
        for dz in -1..=1 {
            for dx in -1..=1 {
                world.set_block_quiet(BlockPos::new(pos.x + dx, y, pos.z + dz), air);
            }
        }
    }
    world.set_block(pos, stone).expect("inside the world");
    (world, stone, air)
}

#[test]
fn generated_world_has_the_requested_chunks() {
    let world = test_world();
    assert_eq!(world.chunk_count(), 9);
    assert!(world.chunk(ChunkPos::new(1, 1)).is_some());
    assert!(world.chunk(ChunkPos::new(2, 0)).is_none());
}

#[test]
fn chunk_positions_are_deterministic() {
    let a = World::generate(2).chunk_positions();
    let b = World::generate(2).chunk_positions();
    assert_eq!(a, b);
    assert!(a.windows(2).all(|w| w[0] <= w[1]), "not sorted");
}

#[test]
fn a_break_reports_the_section_it_landed_in() {
    let (mut world, block) =
        world_with_one(0, ChunkPos::new(0, 0), LocalPos { x: 4, y: 5, z: 6 });
    let pos = BlockPos::new(4, 5, 6);
    assert_eq!(world.block(pos), Some(block));

    let edit = world.break_block(pos).expect("should edit");
    assert_eq!(edit.chunks, vec![ChunkPos::new(0, 0)]);
    // The edited section is always included. Light reaches further than the
    // block itself, so the section below can be in the list too; what must
    // hold is that nothing outside this chunk is touched.
    assert!(
        edit.touches(SectionPos::new(0, 0, 0)),
        "the edited section should be in {:?}",
        edit.sections
    );
    assert!(
        edit.sections
            .iter()
            .all(|section| section.chunk_pos() == ChunkPos::new(0, 0)),
        "an interior break must stay inside one chunk, got {:?}",
        edit.sections
    );
    assert_eq!(world.block(pos), Some(air(&world)));
}

#[test]
fn a_break_on_a_chunk_border_pulls_in_the_neighbouring_chunk() {
    let (mut world, _block) =
        world_with_one(1, ChunkPos::new(1, 0), LocalPos { x: 0, y: 5, z: 6 });
    // x=0 of chunk 1 is world x=16, hard against the border with chunk 0.
    let pos = BlockPos::new(16, 5, 6);

    let edit = world.break_block(pos).expect("should edit");

    assert!(
        edit.owns(SectionPos::new(0, 0, 0)),
        "the neighbour's section is the one whose face appears, so chunk (0,0) \
         must be rebuilt too, got {:?}",
        edit.chunks
    );
    assert!(edit.owns(SectionPos::new(1, 0, 0)), "and the edited chunk");
}

#[test]
fn a_block_on_a_section_boundary_dirties_its_neighbour() {
    let (mut world, _block) =
        world_with_one(1, ChunkPos::new(0, 0), LocalPos { x: 0, y: 5, z: 6 });
    // Placing again at x=0 also touches the section to the -X, which is
    // the neighbouring chunk's section.
    let edit = world
        .set_block(BlockPos::new(0, 5, 6), air(&world))
        .expect("should edit");
    assert!(
        edit.sections.len() >= 2,
        "a boundary block must dirty the neighbour: {:?}",
        edit.sections
    );
    assert!(edit.touches(SectionPos::new(0, 0, 0)));
    assert!(
        edit.touches(SectionPos::new(-1, 0, 0)),
        "the -X neighbour section should be dirty: {:?}",
        edit.sections
    );
}

#[test]
fn an_edit_outside_the_world_is_refused() {
    let mut world = test_world();
    assert_eq!(world.break_block(BlockPos::new(1000, 0, 0)), None);
    assert_eq!(
        world.set_block(BlockPos::new(0, MIN_BLOCK_Y - 1, 0), stone(&world)),
        None
    );
}

#[test]
fn writing_the_same_block_reports_nothing() {
    let (mut world, block) =
        world_with_one(0, ChunkPos::new(0, 0), LocalPos { x: 4, y: 5, z: 6 });
    assert_eq!(
        world.set_block(BlockPos::new(4, 5, 6), block),
        None,
        "a no-op must not trigger a re-mesh"
    );
}

#[test]
fn neighbours_are_reported_across_chunks_and_vertically() {
    let world = test_world();
    let centre = SectionPos::new(0, 0, 0);
    let neighbors = world.section_neighbors(centre);

    assert!(neighbors.neg_x.is_some(), "chunk -1 should be loaded");
    assert!(neighbors.pos_x.is_some());
    assert!(neighbors.pos_y.is_some());
    assert!(neighbors.neg_y.is_some(), "the world is 24 sections tall");

    // The bottom of the world has nothing below it, and neither does the
    // chunk beyond the loaded square.
    let bottom = SectionPos::new(0, MIN_SECTION_Y, 0);
    assert!(
        world.section_neighbors(bottom).neg_y.is_none(),
        "nothing exists below the world's floor"
    );
    let far = SectionPos::new(2, 0, 0);
    assert!(
        world.section_neighbors(far).pos_x.is_none(),
        "chunk 2 is outside the loaded square"
    );
}

#[test]
fn a_ray_through_generated_terrain_hits_something() {
    let world = test_world();
    // Straight down from high above the surface.
    let hit = world
        .raycast([8.5, 200.0, 8.5], [0.0, -1.0, 0.0], 400.0)
        .expect("terrain should be hit");
    assert!(hit.block.y < 200);
    assert_eq!(hit.normal, [0, 1, 0], "entered through the top");
    assert!(world.is_solid(hit.block));
}

#[test]
fn placement_lands_against_the_hit_face_and_refuses_occupied_cells() {
    let target_block = BlockPos::new(4, 8, 6);
    let (mut world, stone, _air) = pocket_world(target_block);

    let hit = RayHit {
        block: target_block,
        normal: [0, 1, 0],
    };

    let cell = world.placement_for(&hit).expect("the cell above is empty");
    assert_eq!(cell, BlockPos::new(4, 9, 6));

    // Against a face whose neighbour is solid, there is nowhere to put it.
    world
        .set_block(BlockPos::new(4, 7, 6), stone)
        .expect("inside the world");
    let blocked = RayHit {
        block: target_block,
        normal: [0, -1, 0],
    };
    assert_eq!(world.placement_for(&blocked), None);

    // Unloaded space refuses placement rather than extending the world.
    let outside = RayHit {
        block: BlockPos::new(100_000, 0, 0),
        normal: [1, 0, 0],
    };
    assert_eq!(
        world.placement_for(&outside),
        None,
        "placement must not reach outside the loaded world"
    );
}

#[test]
fn unloaded_space_is_not_solid() {
    let world = test_world();
    assert!(!world.is_solid(BlockPos::new(100_000, 0, 0)));
    assert_eq!(world.block(BlockPos::new(100_000, 0, 0)), None);
}
///
/// The whole reason `WorldStore` exists: a block dug out, the world saved and
/// reopened, and the hole still there. The generator would fill solid ground
/// back in if the chunk came from it rather than from disk, and that mistake is
/// precisely what this test has to catch.
#[test]
fn an_edit_survives_a_save_and_a_reopen() {
    let dir = std::env::temp_dir().join(format!(
        "filament-persist-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).expect("a scratch world folder");

    let mut world =
        World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    let air = world.registry().id("air").expect("registered");

    // Solid ground somewhere in the first chunk, so the test does not care
    // where the generator put its surface. The chunk is loaded first, since
    // reading does not bring one in by itself.
    world.ensure_chunk(ChunkPos::new(0, 0));
    let target = (-40..320)
        .find_map(|y| {
            let pos = BlockPos::new(7, y, 7);
            match world.block(pos) {
                Some(id) if id != air => Some(pos),
                _ => None,
            }
        })
        .expect("terrain keeps a solid cell below y=64");
    assert!(world.set_block_quiet(target, air), "the dig itself must land");
    world.flush().expect("the folder is writable");
    let stop = dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region");
    let wrote = std::fs::read_dir(&stop).map(|rd| rd.count()).unwrap_or(0);
    assert!(wrote > 0, "flush must leave region files on disk");
    drop(world);

    let mut world = World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    world.ensure_chunk(ChunkPos::new(0, 0));
    let after = world
        .block(target)
        .expect("the chunk came from somewhere, and here it is");
    assert_eq!(
        world.block_name(after),
        "air",
        "the dug cell at {target:?} must stay a hole across the reopen"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_level_file_is_edited_rather_than_replaced() {
    // A file the game wrote holds the seed, the game rules, the borders and
    // the spawn. Writing a default over it loses every one of them, and the
    // day of the year with it.
    let dir = std::env::temp_dir().join(format!(
        "filament-level-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).expect("a scratch world folder");

    // A level file with something of its own in it.
    let level = crate::world::storage::default_level("keepme", 100, 200, (11, 22, 33));
    let mut level = level;    level.set("seed", crate::world::nbt::Tag::Long(987654321));
    level.set(
        "GameRules",
        crate::world::nbt::Tag::Compound(
            crate::world::nbt::Compound::new().put("doDaylightCycle", crate::world::nbt::Tag::Byte(0)),
        ),
    );
    {
        let store = crate::world::storage::WorldStore::open(&dir).unwrap();
        store.write_level(&level).expect("the folder is writable");
    }

    // Open it, and save it again.
    let world =
        World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    assert_eq!(
        world.spawn_point(),
        (11, 22, 33),
        "the spawn comes from the file"
    );
    world.write_level().expect("the folder is writable");
    drop(world);

    // And what is on the disk is the same file with the clock moved on.
    let store = crate::world::storage::WorldStore::open(&dir).unwrap();
    let after = store.read_level().expect("the file is there");
    assert_eq!(
        after.get("seed"),
        Some(&crate::world::nbt::Tag::Long(987654321)),
        "the seed must survive a save"
    );
    assert!(
        after.contains("GameRules"),
        "and so must the game rules"
    );
    assert_eq!(
        after.get("DayTime"),
        Some(&crate::world::nbt::Tag::Long(200)),
        "the day of the year must be written down, not lost"
    );
    assert_eq!(
        after.get("Time"),
        Some(&crate::world::nbt::Tag::Long(100)),
        "and so must the age"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn many_changed_chunks_in_one_region_are_all_saved() {
    // A region file holds a thousand chunks and is written whole, so saving
    // them one at a time writes the same megabytes again and again. All of
    // them have to end up on disk, and in the same file.
    let dir = std::env::temp_dir().join(format!(
        "filament-region-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).expect("a scratch world folder");

    let mut world =
        World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    let air = world.registry().id("air").expect("registered");

    // Twelve chunks, all in the region around the origin.
    let mut dug = Vec::new();
    for n in 0..12 {
        let pos = ChunkPos::new(n, 0);
        world.ensure_chunk(pos);
        let target = (-40..320)
            .find_map(|y| {
                let at = BlockPos::new(7, y, 7);
                match world.block(at) {
                    Some(id) if id != air => Some(at),
                    _ => None,
                }
            })
            .expect("terrain keeps a solid cell below y=64");
        world.set_block(target, air).expect("the dig lands");
        dug.push(target);
    }
    let written = world.flush().expect("the folder is writable");
    assert_eq!(written, 12, "every changed chunk is written");
    drop(world);

    // And they are all there.
    let mut world =
        World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    for (n, target) in dug.iter().enumerate() {
        world.ensure_chunk(ChunkPos::new(n as i32, 0));
        assert_eq!(
            world.block_name(world.block(*target).expect("a block")),
            "air",
            "the hole in chunk {n} must still be a hole"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_day_counter_comes_from_the_clock_file() {
    // The game moved the day counter out of the level file and into a clock
    // file of its own, so a world written by the game has no DayTime in it at
    // all. Reading the level file alone would put the sun at dawn every time
    // the server started.
    let dir = std::env::temp_dir().join(format!(
        "filament-clocks-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).expect("a scratch world folder");

    // A level file with an age and no day counter, and a clock file with one.
    let level = crate::world::storage::default_level("clocky", 500, 0, (0, 64, 0));
    let mut level = level;
    level.remove("DayTime");
    {
        let store = crate::world::storage::WorldStore::open(&dir).unwrap();
        store.write_level(&level).expect("the folder is writable");
        store.write_clocks(626_627).expect("the folder is writable");
    }

    let world = World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    assert_eq!(
        world.day_time(),
        626_627,
        "the day counter comes from the clock file"
    );
    assert_eq!(world.game_time(), 500, "and the age from the level file");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_chunk_that_was_walked_away_from_is_read_back_and_not_made_again() {
    // The case that loses a world. A chunk is loaded, edited and then thrown
    // out because the player walked away. At the save it is no longer in
    // memory, and building it from the noise would write the generator's
    // ground over the ground the game saved.
    let dir = std::env::temp_dir().join(format!(
        "filament-unloaded-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).expect("a scratch world folder");

    let pos = ChunkPos::new(0, 0);
    let mut world =
        World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    let air = world.registry().id("air").expect("registered");

    // Dig a hole and save it, so the file on disk is not what the generator
    // would make.
    world.ensure_chunk(pos);
    let target = (-40..320)
        .find_map(|y| {
            let at = BlockPos::new(7, y, 7);
            match world.block(at) {
                Some(id) if id != air => Some(at),
                _ => None,
            }
        })
        .expect("terrain keeps a solid cell below y=64");
    world.set_block(target, air).expect("the dig itself must land");
    world.flush().expect("the folder is writable");
    drop(world);

    // Open it again, edit it once more, and walk away from it.
    let mut world =
        World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    world.ensure_chunk(pos);
    let second = (-40..320)
        .find_map(|y| {
            let at = BlockPos::new(9, y, 7);
            match world.block(at) {
                Some(id) if id != air => Some(at),
                _ => None,
            }
        })
        .expect("terrain keeps a solid cell below y=64");
    world.set_block(second, air).expect("the second dig must land");
    world.forget_chunk(pos);
    assert!(
        world.chunk(pos).is_none(),
        "the chunk is out of memory now, which is the whole point"
    );

    // The save.
    world.flush().expect("the folder is writable");
    drop(world);

    // And what is on the disk is the ground that was there, hole and all.
    let mut world =
        World::with_store(Some(crate::world::storage::WorldStore::open(&dir).unwrap()));
    world.ensure_chunk(pos);
    assert_eq!(
        world.block_name(world.block(target).expect("a block")),
        "air",
        "the first hole must still be a hole"
    );
    assert_eq!(
        world.block_name(world.block(second).expect("a block")),
        "air",
        "and the second hole must have been written too"
    );

    std::fs::remove_dir_all(&dir).ok();
}
