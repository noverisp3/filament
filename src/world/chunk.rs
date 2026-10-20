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
    /// The chunk already written out as the body of a `level_chunk_with_light`,
    /// or None when it has changed since.
    ///
    /// Writing it out costs about eleven milliseconds and reading the world
    /// costs two tenths, so a player waiting for their opening batch of two
    /// hundred and eighty-nine chunks was waiting on this and nothing else. It
    /// was done on the world's own thread because a chunk can be edited after
    /// it is generated and the body has to show the edit, which meant the copy
    /// could not simply be made once and left alone.
    ///
    /// It can: the body is invalidated whenever a block in the chunk changes,
    /// which is set_block and nothing else. So the pool builds it while the
    /// chunk is fresh and nothing on the world's thread has to wait for it, and
    /// a chunk that has been edited since is written out again when it is next
    /// asked for -- once, for the one chunk that was dug in, rather than for
    /// every chunk that was not.
    serialised: Option<Vec<u8>>,
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
            serialised: None,
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

    /// The light of one section by its place in the chunk, counting from the
    /// bottom.
    ///
    /// The way the wire wants it: the sections in order, so a chunk's light can
    /// be written without knowing any world position.
    pub fn light_at(&self, index: usize) -> Option<&SectionLight> {
        self.light.get(index)
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
            // The body on the wire carries the light as well as the blocks, so
            // a body written before the light moved describes a world that no
            // longer exists.
            self.serialised = None;
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

    /// Block light at a section-local cell, or 0 if the section is outside the
    /// loaded world.
    pub fn block_light_in(&self, section: SectionPos, local: LocalPos) -> u8 {
        self.section_light(section)
            .map_or(0, |light| light.block(local))
    }

    /// Writes block light into a section, ignoring positions outside the world.
    ///
    /// The body on the wire carries the light as well as the blocks, so a body
    /// written before the light moved describes a world that no longer exists.
    pub(crate) fn block_light_mut(&mut self, section: SectionPos, local: LocalPos, value: u8) {
        if let Some(light) = self.section_light_mut(section) {
            light.set_block(local, value);
            self.serialised = None;
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
                // The body on the wire is a copy of what was here before, and
                // set_block is the only thing that changes what is here.
                self.serialised = None;
                true
            }
            None => false,
        }
    }

    /// The body of a `level_chunk_with_light` for this chunk, written once and
    /// kept until a block in it changes.
    ///
    /// `write` is what turns a chunk into bytes; it is passed in rather than
    /// reached for, so that the world module does not have to know how a chunk
    /// is written and the two stay separate.
    pub fn body<F>(&mut self, write: F) -> &[u8]
    where
        F: FnOnce(&Self) -> Vec<u8>,
    {
        if self.serialised.is_none() {
            self.serialised = Some(write(self));
        }
        self.serialised.as_deref().expect("just written")
    }

    /// Whether the body on the wire is still current.
    ///
    /// Only a test wants this. It is the property the cache exists to keep: a
    /// chunk that has not been edited keeps its body, and a chunk that has been
    /// edited does not.
    pub fn body_is_current(&self) -> bool {
        self.serialised.is_some()
    }

    /// Sets the body ahead of the first time it is asked for, so the writing
    /// happens where the chunk is built rather than where it is sent.
    ///
    /// A no-op on a chunk that already has one, and on one that has been edited
    /// since, because a caller that sets a body on an edited chunk would be
    /// caching something that does not describe it.
    pub fn set_body<F>(&mut self, write: F)
    where
        F: FnOnce(&Self) -> Vec<u8>,
    {
        if self.serialised.is_none() {
            self.serialised = Some(write(self));
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
mod tests;
