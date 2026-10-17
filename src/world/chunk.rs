//! A 16×N×16 column of [`Section`]s, the unit the world map is keyed by.

use super::block::BlockId;
use super::light::SectionLight;
use super::pos::{BlockPos, ChunkPos, LocalPos, SECTION_DIM, SectionPos};
use super::section::Section;

/// Lowest world Y. Matches Minecraft's overworld range of −64..320.
pub const MIN_BLOCK_Y: i32 = -64;

/// Height of the buildable world in blocks.
pub const WORLD_HEIGHT: i32 = 384;

/// Sections per chunk column: 384 / 16.
pub const SECTIONS_PER_CHUNK: usize = (WORLD_HEIGHT / SECTION_DIM) as usize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pos: ChunkPos,
    /// Indexed by `section_y - MIN_SECTION_Y`.
    sections: Vec<Section>,
    /// One light record per section, indexed exactly like `sections` so the two
    /// stay in step without a lookup.
    light: Vec<SectionLight>,
}

pub const MIN_SECTION_Y: i32 = MIN_BLOCK_Y / SECTION_DIM;

impl Chunk {
    /// A chunk filled with `fill`, uniformly in every section — so an empty
    /// chunk costs 24 * (palette of one), and nothing per section beyond that.
    pub fn uniform(pos: ChunkPos, fill: BlockId) -> Self {
        let sections = (0..SECTIONS_PER_CHUNK)
            .map(|_| Section::uniform(fill))
            .collect();
        // Every section starts fully sunlit. A chunk on its own cannot know what
        // stands above it, and assuming daylight is the safe direction; the
        // world recomputes it once the surrounding chunks exist.
        let light = (0..SECTIONS_PER_CHUNK)
            .map(|_| SectionLight::open_sky())
            .collect();
        Self {
            pos,
            sections,
            light,
        }
    }

    pub fn pos(&self) -> ChunkPos {
        self.pos
    }

    /// Total dense index-array bytes across all sections.
    pub fn nbytes(&self) -> usize {
        self.sections.iter().map(Section::nbytes).sum()
    }

    /// Dense bytes spent on light, across every section. Blocks are not counted
    /// here, so the two together give the chunk's total.
    pub fn light_nbytes(&self) -> usize {
        self.light.iter().map(SectionLight::nbytes).sum()
    }

    /// Light for one section, or `None` for a Y outside the world.
    pub fn section_light(&self, pos: SectionPos) -> Option<&SectionLight> {
        debug_assert_eq!(pos.chunk_pos(), self.pos, "section from another chunk");
        Self::section_slot(pos.y).map(|slot| &self.light[slot])
    }

    pub fn section_light_mut(&mut self, pos: SectionPos) -> Option<&mut SectionLight> {
        debug_assert_eq!(pos.chunk_pos(), self.pos, "section from another chunk");
        Self::section_slot(pos.y).map(|slot| &mut self.light[slot])
    }

    /// Sky light at a world position, or 0 outside the loaded world.
    pub fn sky_light(&self, pos: BlockPos) -> u8 {
        if pos.chunk_pos() != self.pos {
            return 0;
        }
        match self.section_light(pos.section_pos()) {
            Some(light) => light.sky(pos.local_pos()),
            None => 0,
        }
    }

    /// Sky light at a section-local cell, or 0 if the section is outside the
    /// world's height.
    pub fn sky_light_in(&self, section: SectionPos, local: LocalPos) -> u8 {
        self.section_light(section)
            .map_or(0, |light| light.sky(local))
    }

    /// Writes sky light into a section, ignoring positions outside the world.
    ///
    /// Taken one cell at a time because the light driver writes back a whole
    /// horizontal slice at a time, and handing out a `&mut SectionLight` per
    /// cell would re-check the section bounds 4096 times per section.
    pub(crate) fn sky_light_mut(&mut self, section: SectionPos, local: LocalPos, value: u8) {
        if let Some(light) = self.section_light_mut(section) {
            light.set_sky(local, value);
        }
    }

    /// Block light at a world position, or 0 outside the loaded world.
    pub fn block_light(&self, pos: BlockPos) -> u8 {
        if pos.chunk_pos() != self.pos {
            return 0;
        }
        match self.section_light(pos.section_pos()) {
            Some(light) => light.block(pos.local_pos()),
            None => 0,
        }
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn sections_mut(&mut self) -> &mut [Section] {
        &mut self.sections
    }

    fn section_slot(section_y: i32) -> Option<usize> {
        let slot = (section_y - MIN_SECTION_Y) as usize;
        (slot < SECTIONS_PER_CHUNK).then_some(slot)
    }

    pub fn section(&self, pos: SectionPos) -> Option<&Section> {
        debug_assert_eq!(pos.chunk_pos(), self.pos, "section from another chunk");
        Self::section_slot(pos.y).map(|slot| &self.sections[slot])
    }

    pub fn section_mut(&mut self, pos: SectionPos) -> Option<&mut Section> {
        debug_assert_eq!(pos.chunk_pos(), self.pos, "section from another chunk");
        Self::section_slot(pos.y).map(|slot| &mut self.sections[slot])
    }

    /// Reads a block in world space.
    ///
    /// Returns `None` if `pos` belongs to another chunk or falls outside the
    /// world's Y range. Checking the chunk is not redundant with the Y check:
    /// without it, `Chunk(0,0).block(20, 0, 0)` would silently read from the
    /// neighbouring column instead of failing.
    pub fn block(&self, pos: BlockPos) -> Option<BlockId> {
        if pos.chunk_pos() != self.pos {
            return None;
        }
        let section = self.section(pos.section_pos())?;
        Some(section.get(pos.local_pos()))
    }

    /// Writes a block in world space. Returns `false` for positions that belong
    /// to another chunk or fall outside the world's Y range.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) -> bool {
        if pos.chunk_pos() != self.pos {
            return false;
        }
        match self.section_mut(pos.section_pos()) {
            Some(section) => {
                section.set(pos.local_pos(), block);
                true
            }
            None => false,
        }
    }

    /// Compacts every section, dropping block ids no longer present.
    pub fn compact(&mut self) {
        for section in &mut self.sections {
            section.compact();
        }
    }
}

#[cfg(test)]
mod tests {
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
}
