//! A 16³ cube of blocks, the unit of chunk storage and mesh rebuilding.

use super::block::BlockId;
use super::palette::Paletted;
use super::pos::{LocalPos, SECTION_VOLUME};

/// One vertical slice of a chunk's blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    blocks: Paletted<BlockId>,
}

impl Section {
    /// A section filled entirely with `fill`, costing zero dense bytes.
    pub fn uniform(fill: BlockId) -> Self {
        Self {
            blocks: Paletted::uniform(fill, SECTION_VOLUME),
        }
    }

    #[inline]
    pub fn get(&self, pos: LocalPos) -> BlockId {
        self.blocks.get(pos.index())
    }

    #[inline]
    pub fn set(&mut self, pos: LocalPos, block: BlockId) {
        self.blocks.set(pos.index(), block);
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Dense index-array bytes. Excludes the palette itself.
    pub fn nbytes(&self) -> usize {
        self.blocks.nbytes()
    }

    /// Distinct block ids currently present.
    pub fn distinct_blocks(&self) -> usize {
        self.blocks.palette_len()
    }

    /// Entry width in bits; `0` while the section is uniform.
    pub fn bits_per_block(&self) -> u8 {
        self.blocks.bits()
    }

    /// True when every block in the section is the same id.
    pub fn is_uniform(&self) -> bool {
        self.blocks.bits() == 0
    }

    pub fn blocks(&self) -> &Paletted<BlockId> {
        &self.blocks
    }

    pub(crate) fn blocks_mut(&mut self) -> &mut Paletted<BlockId> {
        &mut self.blocks
    }

    /// Drops block ids no longer present and tightens entry width.
    pub fn compact(&mut self) {
        self.blocks.compact();
    }
}

/// A section plus, optionally, its six neighbours.
///
/// `None` means "not loaded", which the mesher treats as air. That is the safe
/// direction: it can show one extra face, but it can never punch a hole in the
/// world.
///
/// This lives here rather than in the renderer because it describes the storage
/// layout, not anything about drawing. `render::mesh` consumes it; it does not
/// own it.
#[derive(Clone, Copy, Debug, Default)]
pub struct SectionNeighbors<'a> {
    pub pos_x: Option<&'a Section>,
    pub neg_x: Option<&'a Section>,
    pub pos_y: Option<&'a Section>,
    pub neg_y: Option<&'a Section>,
    pub pos_z: Option<&'a Section>,
    pub neg_z: Option<&'a Section>,
}

/// Iterates a section in linear index order (y-major).
pub fn iter_blocks(section: &Section) -> impl Iterator<Item = BlockId> + '_ {
    section.blocks.iter()
}

#[cfg(test)]
mod tests {
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
}
