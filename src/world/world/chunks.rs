//! The chunks of the world: where they are, and how they arrive.
//!
//! Storing a chunk is the one place a chunk enters the world, so it is the one
//! place that has to remember what was done to the last one, and the one place
//! that has to work out the light around it.

use crate::world::chunk::Chunk;
use crate::world::pos::ChunkPos;
use crate::world::terrain::generate_chunk;

use super::World;

impl World {
    /// The chunk at `pos`, generating and storing it if it is not there yet.
    ///
    /// The generator is deterministic, so this is not "generate on miss" in the
    /// cache-invalidating sense: a chunk generated now and the same chunk
    /// generated an hour ago are the same chunk, which is what lets a player walk
    /// away and come back to the ground they left. What changes that is a block
    /// the player placed, and that lives in the stored copy.
    ///
    /// Returns whether it was already there, so a caller can tell a fresh chunk
    /// from a stored one.
    pub fn ensure_chunk(&mut self, pos: ChunkPos) -> bool {
        if self.chunks.contains_key(&pos) {
            return true;
        }
        // Disk first. The generator is deterministic, so it is always able to
        // answer, which is exactly why it has to be the second choice: asked
        // first, it answers every chunk in the world and the files on disk are
        // never opened at all.
        if let Some(chunk) = self
            .store
            .as_mut()
            .and_then(|store| store.load(pos, &mut self.registry))
        {
            self.store_chunk(pos, chunk);
            return true;
        }
        // A chunk that is on the disk and cannot be read is not a chunk that
        // is not there. Generating one in its place would save the generator's
        // world over the game's, and the one thing this server must never do
        // is write over a world it did not read.
        if self
            .store
            .as_ref()
            .is_some_and(|store| store.has(pos))
        {
            eprintln!(
                "the chunk at {pos:?} is on the disk and cannot be read; \
                 leaving it alone rather than putting something else there"
            );
            return false;
        }
        let chunk = generate_chunk(pos, &self.terrain);
        self.store_chunk(pos, chunk);
        self.dirty.insert(pos);
        false
    }

    /// Stores a chunk that was generated somewhere else, or right here.
    ///
    /// The point of the method is that generating and storing are separate
    /// jobs. Generating is a pure function of the position and the palette, so
    /// it can happen on any thread and several at once; storing is a hash map
    /// insert that has to happen on the thread that owns the map.
    ///
    /// It is also the one place a chunk enters the world, so it is the one place
    /// that has to remember what was done to the last one.
    pub fn store_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        let mut chunk = chunk;
        // Replay whatever was done here before. Without this the chunk comes
        // back as the noise made it and a hole is a hole again.
        let edits = self.edits.get(&pos).cloned();
        if let Some(edits) = &edits {
            for (at, block) in edits {
                chunk.set_block(*at, *block);
            }
        }
        self.chunks.insert(pos, chunk);

        // Whether this chunk holds a lamp, so that the question of whether
        // there is one nearby is a lookup and not a walk over its blocks.
        self.note_lamp(pos);

        // The light of a chunk is worked out when it arrives, not read from
        // the file: the game does not keep it either, because the light of a
        // chunk depends on the chunks around it.
        self.light_chunk_columns(pos);
        // And the light that comes in from the sides, which the columns cannot
        // know: the neighbouring chunk's lamp lights this one, and this chunk's
        // lamp lights that one.
        //
        // Only when there is a lamp to spread. Working the region out again
        // costs a moment, and a world with no lamp in it gains nothing.
        if self.lamp_near(pos) {
            self.spread_chunk(pos);
        }
        // And the chunks around it, because the light that crosses the border
        // has to be worked out on both sides of it: the chunk next door may not
        // have arrived yet, and the chunk that arrives later works the border
        // out again.
        //
        // The region is centred on the border rather than on the neighbour,
        // because the cells whose light can have changed are the ones within
        // reach of it. A region around the whole neighbour was twice the work
        // for the same answer.
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                // Only the four that share an edge: the nearest cell of a chunk
                // that touches this one at a corner is sixteen blocks away,
                // which is further than a lamp's light travels.
                if dx != 0 && dz != 0 {
                    continue;
                }
                let neighbour = ChunkPos::new(pos.x + dx, pos.z + dz);
                if !self.chunks.contains_key(&neighbour) {
                    continue;
                }
                if self.lamp_near(neighbour) {
                    self.spread_border(pos, neighbour);
                }
            }
        }
        // And the light of the world as a whole is now out of date, because
        // this chunk's light has not been spread into its neighbours yet.
        self.light_stale = true;

        // And the light around those edits has to be computed again. The
        // chunk came from the noise, which does not know a hole was dug in it,
        // so without this a hole that was dark when it was made is lit again
        // when the chunk comes back.
        if let Some(edits) = edits {
            for (at, _) in edits {
                self.relight_around(at);
            }
        }
    }

    /// Whether a lamp is in this chunk or in one of the eight around it.
    ///
    /// Light travels fifteen blocks, which is one chunk, so a lamp in a chunk
    /// next door reaches this one. If there is no lamp anywhere near, there is
    /// nothing to spread and no reason to work the region out again.
    fn lamp_near(&self, pos: ChunkPos) -> bool {
        for dx in -1..=1 {
            for dz in -1..=1 {
                if self.lamp_chunks.contains(&ChunkPos::new(pos.x + dx, pos.z + dz)) {
                    return true;
                }
            }
        }
        false
    }

    /// Whether a chunk holds a block that gives off light.
    ///
    /// One pass over the chunk's blocks, which is a hundred thousand of them,
    /// and the answer is kept so that it is not paid again for every chunk that
    /// arrives and every chunk that is asked about.
    fn chunk_has_lamp(&self, pos: ChunkPos) -> bool {
        let registry = self.registry();
        let Some(chunk) = self.chunk(pos) else {
            return false;
        };
        for section in chunk.sections() {
            for block in crate::world::section::iter_blocks(section) {
                if registry.light_emission(block) > 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Remembers whether a chunk holds a lamp, now that it has changed.
    pub(super) fn note_lamp(&mut self, pos: ChunkPos) {
        if self.chunk_has_lamp(pos) {
            self.lamp_chunks.insert(pos);
        } else {
            self.lamp_chunks.remove(&pos);
        }
    }

    /// Drops a chunk and its edits.
    ///
    /// Called when the player walks out of range of it. The terrain comes back
    /// identically if they walk back, since the generator is deterministic, but
    /// anything the player built in it does not: that lived only in the stored
    /// copy, and this is the code that decides how long a build is kept.
    pub fn forget_chunk(&mut self, pos: ChunkPos) -> bool {
        self.lamp_chunks.remove(&pos);
        self.chunks.remove(&pos).is_some()
    }

    /// Chunk positions in a stable order, so rebuilds are deterministic.
    pub fn chunk_positions(&self) -> Vec<ChunkPos> {
        let mut positions: Vec<ChunkPos> = self.chunks.keys().copied().collect();
        positions.sort_by_key(|pos| (pos.x, pos.z));
        positions
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}
