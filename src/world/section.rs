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

    /// Rebuilds a section from a stored palette and index array.
    ///
    /// The mirror of [`Paletted::from_parts`], for the same reason: the bits
    /// are not in the words, they have to come from the palette.
    pub fn from_parts(palette: Vec<BlockId>, bits: u8, words: Vec<u64>) -> Self {
        Self {
            blocks: Paletted::from_parts(palette, bits, words, SECTION_VOLUME),
        }
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
/// `None` means "not loaded", which the mesher treats as air: that way it can
/// show one extra face, but it can never punch a hole in the world.
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
mod tests;
