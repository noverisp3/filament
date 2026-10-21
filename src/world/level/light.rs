//! The light of the world: what the client is told, and how much work it took.
//!
//! The light of a section is read out of the chunk it belongs to, so a chunk
//! that was thrown away has none to give.

use crate::world::pos::{BlockPos, SectionPos};

use super::World;

impl World {
    /// The sections whose light moved since anyone last looked.
    pub fn take_light_changes(&mut self) -> Vec<SectionPos> {
        std::mem::take(&mut self.light_changes)
    }

    /// The light of one section, packed the way the game packs it.
    ///
    /// `None` when the section is not here: a chunk that was thrown away has
    /// no light to give, and the client is told nothing rather than told
    /// something that is not there.
    pub fn section_sky_bytes(&self, section: SectionPos) -> Option<Vec<u8>> {
        let chunk = self.chunk(section.chunk_pos())?;
        let light = chunk.section_light(section)?;
        Some(light.sky_bytes())
    }

    /// The light the blocks themselves give off in one section, packed the way
    /// the game packs it.
    pub fn section_block_bytes(&self, section: SectionPos) -> Option<Vec<u8>> {
        let chunk = self.chunk(section.chunk_pos())?;
        let light = chunk.section_light(section)?;
        Some(light.block_bytes())
    }

    /// How many cells the light has been worked out for, since the world began.
    pub fn light_work(&self) -> usize {
        self.light_work
    }

    /// Recomputes sky light around `center`, for use after [`Self::set_block_quiet`].
    pub fn relight(&mut self, center: BlockPos) {
        self.relight_around(center);
    }
}
