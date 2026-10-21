//! The blocks of the world: reading one, writing one, and what a write costs.
//!
//! A write is not just a write: it changes what the neighbours can see, it
//! moves the light, and it has to be remembered outside the chunk so that it
//! survives the chunk being thrown away.

use crate::world::block::BlockId;
use crate::world::pos::{BlockPos, ChunkPos, Direction, SECTION_DIM};
use crate::world::raycast::{RayHit, raycast};

use super::BlockEdit;
use super::World;

impl World {
    /// The block at a world position, or `None` outside the loaded world.
    pub fn block(&self, pos: BlockPos) -> Option<BlockId> {
        self.chunk(pos.chunk_pos())?.block(pos)
    }

    /// Whether a block stops a ray and hides the faces behind it.
    ///
    /// An unloaded position counts as transparent. That direction is the safe
    /// one: picking may select something in an unloaded chunk, but a hole can
    /// never be opened by editing nothing.
    pub fn is_solid(&self, pos: BlockPos) -> bool {
        match self.block(pos) {
            Some(block) => self.registry.is_opaque(block),
            None => false,
        }
    }

    /// Writes a block and reports what has to be rebuilt.
    ///
    /// Returns `None` when the position is outside the loaded world or the
    /// write is a no-op, so an unchanged block never triggers a re-mesh.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) -> Option<BlockEdit> {
        let chunk_pos = pos.chunk_pos();
        if !self.chunks.contains_key(&chunk_pos) {
            return None;
        }
        if self.block(pos) == Some(block) {
            return None;
        }

        // `Chunk::set_block` refuses positions outside the world's Y range, and
        // ignoring that would report an edit that never happened.
        if !self.chunk_mut(chunk_pos)?.set_block(pos, block) {
            return None;
        }

        // Whether this chunk holds a lamp may have changed with it.
        if self.registry().light_emission(block) > 0 {
            self.lamp_chunks.insert(chunk_pos);
        } else {
            // The lamp may have been taken away, and the chunk may hold
            // another one, so the question is asked of the chunk again.
            self.note_lamp(chunk_pos);
        }

        let section = pos.section_pos();
        let local = pos.local_pos();
        let mut sections = vec![section];

        // A block on a section boundary changes what the neighbour can see.
        if local.x == 0 {
            sections.push(section.offset(-1, 0, 0));
        }
        if local.x == (SECTION_DIM - 1) as u8 {
            sections.push(section.offset(1, 0, 0));
        }
        if local.y == 0 {
            sections.push(section.offset(0, -1, 0));
        }
        if local.y == (SECTION_DIM - 1) as u8 {
            sections.push(section.offset(0, 1, 0));
        }
        if local.z == 0 {
            sections.push(section.offset(0, 0, -1));
        }
        if local.z == (SECTION_DIM - 1) as u8 {
            sections.push(section.offset(0, 0, 1));
        }

        // Keep only sections that exist: a neighbour outside the world cannot
        // be rebuilt and has nothing to hide.
        sections.retain(|candidate| {
            self.chunk(ChunkPos::new(candidate.x, candidate.z))
                .is_some_and(|chunk| chunk.section(*candidate).is_some())
        });

        // Shading changed wherever the light moved, which reaches further than
        // the block edit itself.
        let relit = self.relight_around(pos);
        sections.extend(relit.iter().copied());
        // And the sections whose light moved are what the client has to be
        // told about, or the hole stays lit on its screen.
        self.light_changes.extend(relit);

        sections.sort_by_key(|section| (section.x, section.y, section.z));
        sections.dedup();

        // The chunks follow from the sections, so a section on a chunk border
        // cannot be rebuilt without its neighbour's chunk also being rebuilt.
        let mut chunks: Vec<ChunkPos> = sections
            .iter()
            .map(|section| ChunkPos::new(section.x, section.z))
            .collect();
        chunks.sort_by_key(|chunk| (chunk.x, chunk.z));
        chunks.dedup();

        // Remembered outside the chunk, which is the whole point: the chunk is
        // thrown away when the last player walks away from it and this is not.
        // Recorded after the write has been accepted, so a refused edit is not
        // remembered as though it had happened.
        let journal = self.edits.entry(chunk_pos).or_default();
        match journal.iter_mut().find(|(at, _)| *at == pos) {
            Some(entry) => entry.1 = block,
          None => journal.push((pos, block)),
      }
        self.dirty.insert(chunk_pos);

      // The six blocks around this one get told on the next tick, not now.
      //
      // A neighbour update is how a block learns something changed next to it,
      // and answering it here would answer it before the change is even
      // finished: a block placed against a fence needs the fence to notice, and
      // the fence may yet be replaced by the next of the six.
      self.schedule_tick(pos, 1);
      for face in [
          Direction::Down,
          Direction::Up,
          Direction::North,
          Direction::South,
          Direction::West,
          Direction::East,
      ] {
          self.schedule_tick(pos.offset(face), 1);
      }

      // And the game's own random ticks, for the block and the six around
      // it: the rules that happen now and then, and not the moment something
      // changed.
      //
      // The delay is a long one and a different one each time, because the
      // game does not roll these on a schedule either: it rolls them by
      // chance, and a hillside that heals itself the instant a block goes
      // down is not a hillside healing itself. A minute and a bit is the
      // game's own rate, near enough.
      for at in [
          pos,
          pos.offset(Direction::Down),
          pos.offset(Direction::Up),
          pos.offset(Direction::North),
          pos.offset(Direction::South),
          pos.offset(Direction::West),
          pos.offset(Direction::East),
      ] {
          let delay = 1200 + self.dice(at, 1) % 2400;
          self.schedule_random_tick(at, delay);
      }

      Some(BlockEdit { chunks, sections })
    }

    /// Writes a block without relighting or reporting what to rebuild.
    ///
    /// For bulk edits. Building a structure one block at a time would relight the
    /// same region once per block, dozens of times over, for the same final
    /// answer. Call [`Self::relight_around`] once when the work is done.
    ///
    /// Using this for a single edit would leave stale shading, which is why the
    /// ordinary [`Self::set_block`] is the one gameplay uses.
    pub fn set_block_quiet(&mut self, pos: BlockPos, block: BlockId) -> bool {
        let Some(chunk) = self.chunk_mut(pos.chunk_pos()) else {
            return false;
        };
        chunk.set_block(pos, block)
    }

    /// Breaks a block, turning it to air.
    pub fn break_block(&mut self, pos: BlockPos) -> Option<BlockEdit> {
        let air = self.registry.id("air")?;
        self.set_block(pos, air)
    }

    /// The first solid block along a ray.
    pub fn raycast(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        max_distance: f64,
    ) -> Option<RayHit> {
        raycast(origin, direction, max_distance, &|pos| self.is_solid(pos))
    }

    /// The position a block would occupy if placed against `hit`.
    ///
    /// `None` when the cell is already taken, or lies outside the loaded
    /// world. Both cases need explicit handling: air is a *block* like any
    /// other, so asking whether the cell holds anything would refuse every
    /// placement, and an unloaded position reports no block at all, which looks
    /// identical to an empty one.
    pub fn placement_for(&self, hit: &RayHit) -> Option<BlockPos> {
        let target = BlockPos::new(
            hit.block.x + hit.normal[0],
            hit.block.y + hit.normal[1],
            hit.block.z + hit.normal[2],
        );
        self.chunk(target.chunk_pos())?;
        if self.is_solid(target) {
            return None;
        }
        Some(target)
    }
}
