use super::*;
use crate::world::block::BlockRegistry;

fn ids() -> (BlockRegistry, BlockId, BlockId, BlockId) {
    let mut registry = BlockRegistry::new();
    let air = registry.register("air", false, [0.0, 0.0, 0.0]);
    let stone = registry.register("stone", true, [0.50, 0.50, 0.52]);
    let dirt = registry.register("dirt", true, [0.45, 0.32, 0.20]);
    (registry, air, stone, dirt)
}

#[test]
fn uniform_section_costs_nothing_dense() {
    let (_registry, _air, stone, _dirt) = ids();
    let section = Section::uniform(stone);
    assert!(section.is_uniform());
    assert_eq!(section.nbytes(), 0);
    assert_eq!(section.distinct_blocks(), 1);
    assert_eq!(section.len(), 4096);
    for y in 0..16u8 {
        for z in 0..16u8 {
            for x in 0..16u8 {
                assert_eq!(section.get(LocalPos { x, y, z }), stone);
            }
        }
    }
}

#[test]
fn writes_are_readable_at_every_position() {
    let (_registry, air, stone, _dirt) = ids();
    let mut section = Section::uniform(air);

    for y in 0..16u8 {
        for z in 0..16u8 {
            for x in 0..16u8 {
                assert_eq!(section.get(LocalPos { x, y, z }), air);
            }
        }
    }

    section.set(LocalPos { x: 0, y: 0, z: 0 }, stone);
    section.set(
        LocalPos {
            x: 15,
            y: 15,
            z: 15,
        },
        stone,
    );
    section.set(LocalPos { x: 8, y: 4, z: 12 }, stone);

    assert_eq!(section.get(LocalPos { x: 0, y: 0, z: 0 }), stone);
    assert_eq!(
        section.get(LocalPos {
            x: 15,
            y: 15,
            z: 15
        }),
        stone
    );
    assert_eq!(section.get(LocalPos { x: 8, y: 4, z: 12 }), stone);
    assert_eq!(section.get(LocalPos { x: 1, y: 0, z: 0 }), air);
}

#[test]
fn dense_cost_tracks_block_diversity() {
    let (_registry, air, stone, _dirt) = ids();
    let mut section = Section::uniform(air);

    // Two block types: one bit per entry, 4096 entries -> 512 bytes.
    section.set(LocalPos { x: 1, y: 1, z: 1 }, stone);
    assert_eq!(section.bits_per_block(), 1);
    assert_eq!(section.nbytes(), 512);

    section.compact();
    assert_eq!(section.distinct_blocks(), 2);
    assert_eq!(section.nbytes(), 512);
}

#[test]
fn a_typical_terrain_section_stays_small() {
    // The case that decides whether palette storage is worth it at all:
    // a slab of stone over air, plus a few dirt patches.
    let (_registry, air, stone, dirt) = ids();
    let mut section = Section::uniform(air);

    for y in 0..5u8 {
        for z in 0..16u8 {
            for x in 0..16u8 {
                section.set(LocalPos { x, y, z }, stone);
            }
        }
    }
    for index in 0..40usize {
        let pos = LocalPos::from_index(index);
        section.set(pos, dirt);
    }

    assert_eq!(section.distinct_blocks(), 3);
    assert_eq!(section.bits_per_block(), 2, "three types fit in 2 bits");
    assert_eq!(section.nbytes(), 1024, "quarter of a naive u16 section");
}

#[test]
fn iteration_visits_every_block_once_in_index_order() {
    let (_registry, air, stone, _dirt) = ids();
    let mut section = Section::uniform(air);
    section.set(LocalPos { x: 3, y: 2, z: 1 }, stone);

    let collected: Vec<BlockId> = iter_blocks(&section).collect();
    assert_eq!(collected.len(), SECTION_VOLUME);
    assert_eq!(collected[LocalPos { x: 3, y: 2, z: 1 }.index()], stone);
    assert_eq!(collected[0], air);
    assert_eq!(collected[SECTION_VOLUME - 1], air);
}