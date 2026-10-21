//! The edit journal: what has been changed, and where.
//!
//! The journal outlives the chunk on purpose: a chunk that was thrown away
//! when the last player walked away comes back from the folder without the
//! changes, and this is what puts them back.

use crate::world::block::BlockId;
use crate::world::chunk::Chunk;
use crate::world::pos::{BlockPos, ChunkPos, SectionPos};

use super::World;

/// What changed after an edit, and therefore what has to be rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEdit {
    /// Every chunk holding a section that has to be rebuilt.
    ///
    /// More than one is normal: breaking a block on a chunk border reveals the
    /// neighbouring block's face, and that face belongs to the *other* chunk.
    /// Re-meshing only the edited chunk left a hole in the world there.
    pub chunks: Vec<ChunkPos>,
    /// Sections whose visible surface or shading may differ. Always includes the
    /// section holding the block, a neighbour when the block sits on a boundary,
    /// and every section whose light changed.
    pub sections: Vec<SectionPos>,
}

/// Puts a chunk's remembered edits back onto it.
///
/// The journal outlives the chunk on purpose: a chunk that was thrown away
/// when the last player walked away comes back from the folder without the
/// changes, and this is what puts them back.
pub(super) fn replay_edits(chunk: &mut Chunk, edits: Option<&Vec<(BlockPos, BlockId)>>) {
    let Some(edits) = edits else {
        return;
    };
    for (at, block) in edits {
        chunk.set_block(*at, *block);
    }
}

impl BlockEdit {
    pub fn touches(&self, section: SectionPos) -> bool {
        self.sections.contains(&section)
    }

    /// Chunks containing `section`, whether or not they need rebuilding.
    pub fn owns(&self, section: SectionPos) -> bool {
        self.chunks.contains(&section.chunk_pos())
    }

    /// Folds another edit in, without reporting the same chunk twice.
    ///
    /// A tick that changes several neighbouring blocks produces several edits,
    /// and each one names the chunk it is in. Sending a chunk twice is not wrong
    /// -- the client takes the second -- but it is bytes for nothing, and the
    /// list of chunks is what a caller walks to decide what to re-send.
    pub fn merge(&mut self, other: BlockEdit) {
        for pos in other.chunks {
            if !self.chunks.contains(&pos) {
                self.chunks.push(pos);
            }
        }
        for pos in other.sections {
            if !self.sections.contains(&pos) {
                self.sections.push(pos);
            }
        }
    }
}

impl World {
    /// What has been changed in each chunk, and how much.
    ///
    /// This is the whole of what a server needs to keep to have a world that
    /// survives its players walking away from it. It is exposed because it is
    /// the part that would be written to disk, and because a test needs to see
    /// that an edit was remembered rather than only that it was applied.
    pub fn edits(&self) -> impl Iterator<Item = (ChunkPos, usize)> + '_ {
        self.edits
            .iter()
            .map(|(pos, edits)| (*pos, edits.len()))
            .filter(|(_, n)| *n > 0)
    }

    /// How many edits are remembered, over every chunk.
    pub fn edit_count(&self) -> usize {
        self.edits.values().map(|e| e.len()).sum()
    }
}
