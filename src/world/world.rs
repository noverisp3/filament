//! The world: chunks, the block registry, and the queries that read them.
//!
//! This sits between storage and rendering. The renderer never owns world
//! data; it borrows a `&World` when it needs geometry, which keeps the rule
//! that logic in `world/` does not depend on wgpu, and the reverse.

use std::collections::{HashMap, HashSet};

use super::block::{BlockId, BlockRegistry};
use super::chunk::{Chunk, MIN_SECTION_Y, SECTIONS_PER_CHUNK};
use super::pos::{BlockPos, ChunkPos, Direction, SECTION_DIM, SectionPos};
use super::raycast::{RayHit, raycast};
use super::section::{Section, SectionNeighbors};
use super::terrain::{TerrainBlocks, generate_chunk, register_terrain};

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

impl BlockEdit {
    pub fn touches(&self, section: SectionPos) -> bool {
        self.sections.contains(&section)
    }

    /// Chunks containing `section`, whether or not they need rebuilding.
    pub fn owns(&self, section: SectionPos) -> bool {
        self.chunks.contains(&section.chunk_pos())
    }
}

pub struct World {
    registry: BlockRegistry,
    terrain: TerrainBlocks,
    chunks: HashMap<ChunkPos, Chunk>,
    /// What has been changed, and where, separately from the chunks themselves.
    ///
    /// A chunk is thrown away the moment the last player walks away from it and
    /// rebuilt from the noise when somebody comes back, so an edit that lives
    /// only in the chunk is gone by then. The report from the field was a hole
    /// that filled itself back in: both players watched a block go, and walking
    /// back to it found it solid again.
    ///
    /// The edits are kept out here, keyed by chunk, and replayed onto a chunk as
    /// it arrives. The journal outlives the chunk, so the chunk can be as
    /// disposable as it likes.
    ///
    /// It grows with what players have done and not with what they have walked
    /// past, which is the thing that matters: a server nobody edited has an
    /// empty one no matter how far anyone travelled.
    edits: HashMap<ChunkPos, Vec<(BlockPos, BlockId)>>,

    /// Chunks that must be written back: either generated here (not on disk
    /// yet) or edited since they were loaded. Loaded-but-untouched chunks are
    /// left alone, so a session that only walked around writes nothing.
    dirty: HashSet<ChunkPos>,

    /// Ticks since the world began, counting up and never back.
    ///
    /// The game's `getGameTime`, and the number almost everything scheduled
    /// refers to: "in twenty ticks" is a statement about this, so a rule that
    /// wants to run again in twenty ticks has to know what now is.
    game_time: u64,

    /// Where the sun is, which is a different number from the same clock.
    ///
    /// This one wraps, at twenty-four thousand ticks, and it is what the client
    /// is told so that the sky matches the world it is standing in. The two
    /// answer different questions -- *how long has this world existed* and *what
    /// time is it* -- and a server that keeps one number for both sends a sky
    /// that has been running since the world was made.
    day_time: u64,

    /// Work the world owes itself, ordered by when.
    ticks: crate::world::ticks::BlockTicks,

    /// What a tick changed on its own, waiting for a connection to send it.
    ///
    /// Separate from `edits`, which is the whole history and is never drained:
    /// this is only what has happened since someone last asked.
    tick_changes: Vec<(BlockPos, BlockId)>,

    /// Where the world came from and will go back to. None means memory only.
    store: Option<std::sync::Arc<super::storage::WorldStore>>,
}

impl BlockEdit {
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

impl Clone for World {
    /// Chunks and their light are plain data, so copying a world is copying two
    /// maps. Worth having for tests that need to run the same edit through a
    /// cheap path and an expensive one and compare the results.
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            terrain: self.terrain,
            chunks: self.chunks.clone(),
            edits: self.edits.clone(),
            dirty: self.dirty.clone(),
            game_time: self.game_time,
            day_time: self.day_time,
            ticks: self.ticks.clone(),
            tick_changes: Vec::new(),
            // A clone has no folder of its own; sharing one would mean two
            // worlds writing the same region files.
            store: None,
        }
    }
}

impl World {
    /// An empty world that generates chunks as they are asked for.
    ///
    /// `generate` below fills a square up front, which is what a test or a
    /// fixed map wants. A server does not know how big the map is: it learns the
    /// shape from where the player walks, so it needs a world that starts with
    /// nothing and grows.
    pub fn empty() -> Self {
        Self::with_store(None)
    }

    /// A world backed by a folder on disk.
    ///
    /// `None` is a world that lives only as long as the process does, which is
    /// what every test wants and what a server without a folder gets.
    pub fn with_store(store: Option<super::storage::WorldStore>) -> Self {
    let mut registry = BlockRegistry::new();
    let terrain = register_terrain(&mut registry);
    let mut world = Self {
        registry,
        terrain,
        chunks: HashMap::new(),
        edits: HashMap::new(),
        dirty: HashSet::new(),
        game_time: 0,
        day_time: 0,
        ticks: crate::world::ticks::BlockTicks::new(),
        tick_changes: Vec::new(),
        store: store.map(std::sync::Arc::new),
    };

    // The clock the world was saved with. A restart that wound `Time` back
    // would replay a day through every crop and furnace in it, so what is in
    // `level.dat` is what the ticks continue from.
    if let Some(level) = world.store.as_ref().and_then(|store| store.read_level()) {
        let data = level.compound("Data").unwrap_or(&level);
        if let Some(super::nbt::Tag::Long(t)) = data.get("Time") {
            world.game_time = *t as u64;
        }
        if let Some(super::nbt::Tag::Long(t)) = data.get("DayTime") {
            world.day_time = *t as u64;
        }
    }
    world
}

    /// Writes the chunks that changed back to their region files.
    ///
    /// A chunk loaded from disk and left alone is not written again: rewriting
    /// every loaded region on every save is what turned an idle session into
    /// tens of megabytes of disk traffic. Only chunks that were generated here
    /// or edited get saved; the edits journal keeps them consistent even when
    /// the chunk itself has been thrown away and rebuilt.
    pub fn flush(&mut self) -> std::io::Result<usize> {
        let game_time = self.game_time;
        let Some(store) = self.store.as_mut() else {
            return Ok(0);
        };
        let dirty: Vec<ChunkPos> = self.dirty.iter().copied().collect();
        let mut written = 0;
        for pos in dirty {
            let chunk = if let Some(chunk) = self.chunks.get(&pos) {
                chunk.clone()
            } else {
                let mut chunk = crate::world::terrain::generate_chunk(pos, &self.terrain);
                if let Some(edits) = self.edits.get(&pos) {
                    for (at, block) in edits {
                        chunk.set_block(*at, *block);
                    }
                }
                chunk
            };
            match store.save(pos, &chunk, &self.registry, game_time) {
                Ok(()) => {
                    written += 1;
                    self.dirty.remove(&pos);
                }
                Err(e) => eprintln!("cannot save chunk {},{}: {e}", pos.x, pos.z),
            }
        }
        Ok(written)
    }

    /// Writes `level.dat`, if there is a world folder to write it into.
    pub fn write_level(&self) -> std::io::Result<()> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        store.write_level(&super::storage::default_level(store.name(), self.game_time))
    }

    /// Ticks since the world began.
    pub fn game_time(&self) -> u64 {
        self.game_time
    }

    /// How many block types are registered, which grows as a world is loaded.
    ///
    /// Five at the start. A world written by the game has hundreds, and this
    /// is how a caller can see that it read one rather than generated it.
    pub fn registry_len(&self) -> usize {
        self.registry.len()
    }

    /// The registry name of a block, for printing.
    pub fn block_name(&self, id: BlockId) -> &str {
        self.registry.name(id)
    }

    /// Where the sun is, in ticks, wrapping at a full day.
    pub fn day_time(&self) -> u64 {
        self.day_time
    }

    /// Promises to come back to this position in `delay` ticks.
    pub fn schedule_tick(&mut self, pos: BlockPos, delay: u64) {
        self.ticks.schedule(self.game_time, delay, pos);
    }

    /// What the world promised itself and not yet did.
    pub fn scheduled_ticks(&self) -> usize {
        self.ticks.len()
    }

    /// What the last tick changed on its own, for the players who can see it.
    ///
    /// A tick has no connection to look out of, so what it changed has to be
    /// collected here and drained by the connections. It is drained rather than
    /// broadcast because the world thread does not know who is connected, and
    /// sending to nobody is worse than sending late.
    pub fn take_tick_changes(&mut self) -> Vec<(BlockPos, BlockId)> {
        std::mem::take(&mut self.tick_changes)
    }

    /// What the world changed by itself since anyone last looked.
    pub fn pending_tick_changes(&self) -> usize {
        self.tick_changes.len()
    }

    /// Tells the six blocks around a change that something near them moved.
    ///
    /// The game's `updateNeighborsAt`, and the reason it exists: a block cannot
    /// know what happened next to it, so the change goes round. It is scheduled
    /// rather than run because the point of a neighbour update is that it may
    /// change something else, and answering that inline turns one edit into a
    /// chain of them all inside the event that caused it.
    pub fn update_neighbors(&mut self, pos: BlockPos) {
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
    }

    /// Moves the world on by one tick.
    ///
    /// This is what a tick is *for*. Everything the game does on a delay -- a
    /// block that wants to check its neighbours again, water looking for
    /// somewhere to go, an item waiting to be picked up -- comes back through
    /// here, and nothing is answered anywhere else.
    ///
    /// The order is the game's (`ServerLevel.tick`): work that is due is taken
    /// under a budget, and what does not fit waits for the next tick rather than
    /// being dropped or paid for all at once here.
    pub fn tick(&mut self) -> BlockEdit {
    // The clock runs on without being wrapped, which is what the game does:
    // the client takes the day itself out of the count when it works out where
    // the sun is, so wrapping here would send a sky that jumps backwards at
    // every day's end. `DAY_LENGTH` is the day's length, not the clock's cap.
    self.game_time = self.game_time.wrapping_add(1);
    self.day_time = self.day_time.wrapping_add(1);

        let mut edit = BlockEdit {
            chunks: Vec::new(),
            sections: Vec::new(),
        };
        let due = self
            .ticks
            .take_due(self.game_time, crate::world::ticks::TICK_BUDGET);
        for pos in due {
            self.block_update(pos, &mut edit);
        }
        edit
    }

    /// One block's turn to notice something.
    ///
    /// The game's is:
    ///
    /// ```java
    /// private void tickBlock(final BlockPos pos, final Block type) {
    ///    BlockState state = this.getBlockState(pos);
    ///    if (state.is(type)) { state.tick(this, pos, this.random); }
    /// }
    /// ```
    ///
    /// -- re-read the block and only act if it is still the thing that was
    /// asked. A player can break a block between it being scheduled and its turn
    /// coming round, and a rule that acts on the stale name writes into a block
    /// that is now something else.
    fn block_update(&mut self, pos: BlockPos, edit: &mut BlockEdit) {
        let Some(block) = self.block(pos) else { return };

        // Grass comes back to dirt that has grass under it and air over it.
        //
        // That is the rule that makes a dug hillside green again instead of
        // leaving a stripe of bare dirt across it, and it is the first thing the
        // tick does here because it is the first thing that needed the tick to
        // exist at all. Everything else this will eventually run -- water,
        // crops, growing saplings -- is the same shape.
        if block == self.terrain.dirt {
            let above = self.block(pos.offset(Direction::Up));
            let below = self.block(pos.offset(Direction::Down));
            let open_above = above.map(|b| b == self.terrain.air).unwrap_or(true);
            let grass_under = below.map(|b| b == self.terrain.grass).unwrap_or(false);
            if open_above && grass_under
                && let Some(made) = self.set_block(pos, self.terrain.grass)
            {
                self.tick_changes.push((pos, self.terrain.grass));
                edit.merge(made);
            }
        }
    }

    /// A full day, in ticks. Twenty-four thousand, which is what a Minecraft day
    /// is: twenty minutes at twenty a second.
    pub const DAY_LENGTH: u64 = 24_000;

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
        // answer -- which is exactly why it has to be the second choice and not
        // the first: asked first, it answers every chunk in the world and the
        // files on disk are never opened at all.
        if let Some(chunk) = self
            .store
            .as_mut()
            .and_then(|store| store.load(pos, &mut self.registry))
        {
            self.store_chunk(pos, chunk);
            return true;
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
    /// insert that has to happen on the thread that owns the map. Splitting them
    /// is what lets a connection ask for a chunk without waiting for the
    /// world's thread to have finished building it.
    ///
    /// It is also the one place a chunk enters the world, so it is the one place
    /// that has to remember what was done to the last one. `ensure_chunk` used
    /// to insert straight into the map, which meant the generator's chunk came
    /// back without the edits and a hole filled itself in whenever the slow
    /// path was the one that rebuilt it.
    pub fn store_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        let mut chunk = chunk;
        // Replay whatever was done here before. Without this the chunk comes
        // back as the noise made it and a hole is a hole again.
        if let Some(edits) = self.edits.get(&pos) {
            for (at, block) in edits {
                chunk.set_block(*at, *block);
            }
        }
        self.chunks.insert(pos, chunk);
    }

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

    /// Drops a chunk and its edits.
    ///
    /// Called when the player walks out of range of it. The terrain comes back
    /// identically if they walk back, since the generator is deterministic, but
    /// anything the player built in it does not: that lived only in the stored
    /// copy, and this is the code that decides how long a build is kept.
    pub fn forget_chunk(&mut self, pos: ChunkPos) -> bool {
        self.chunks.remove(&pos).is_some()
    }

    /// Builds a world of generated terrain.
    ///
    /// Takes the chunk radius to generate along each axis.
    pub fn generate(radius: i32) -> Self {
        let mut registry = BlockRegistry::new();
        let terrain = register_terrain(&mut registry);

        let mut chunks = HashMap::new();
        for x in -radius..=radius {
            for z in -radius..=radius {
                let pos = ChunkPos::new(x, z);
                chunks.insert(pos, generate_chunk(pos, &terrain));
            }
        }

        Self {
            registry,
            terrain,
            chunks,
            store: None,
            edits: HashMap::new(),
            dirty: HashSet::new(),
            game_time: 0,
            day_time: 0,
            ticks: crate::world::ticks::BlockTicks::new(),
            tick_changes: Vec::new(),
        }
    }

    pub fn registry(&self) -> &BlockRegistry {
        &self.registry
    }

    /// The terrain generator's block ids, for mapping a stored block onto the
    /// game's state id on the way out.
    ///
    /// The two registries are unrelated: air is zero in both by coincidence and
    /// nothing else lines up. The mapping belongs to whoever sends packets, not
    /// to storage, which is why this hands the ids over rather than doing it.
    pub fn terrain_blocks(&self) -> &TerrainBlocks {
        &self.terrain
    }

    /// A handle to the backing folder, if there is one. Shared with the
    /// terrain workers so they can check whether a chunk already exists on
    /// disk before generating it.
    pub fn store_handle(&self) -> Option<std::sync::Arc<super::storage::WorldStore>> {
        self.store.clone()
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

    /// The six sections around `section_pos`.
    ///
    /// `None` means "not loaded", which the mesher treats as air.
    pub fn section_neighbors(&self, section_pos: SectionPos) -> SectionNeighbors<'_> {
        let step = |dx: i32, dy: i32, dz: i32| -> Option<&Section> {
            let target =
                SectionPos::new(section_pos.x + dx, section_pos.y + dy, section_pos.z + dz);
            if target.y < MIN_SECTION_Y || target.y >= MIN_SECTION_Y + SECTIONS_PER_CHUNK as i32 {
                return None;
            }
            self.chunk(ChunkPos::new(target.x, target.z))?
                .section(target)
        };

        SectionNeighbors {
            pos_x: step(1, 0, 0),
            neg_x: step(-1, 0, 0),
            pos_y: step(0, 1, 0),
            neg_y: step(0, -1, 0),
            pos_z: step(0, 0, 1),
            neg_z: step(0, 0, -1),
        }
    }

    /// The 3Ã—3Ã—3 block of sections around `section_pos`, with their light.
    ///
    /// Six face neighbours are not enough: shading a face corner reads the
    /// diagonal block, so a face on a section boundary reaches into edge and
    /// corner neighbours. Without those, every section boundary would show a
    /// band where the shading stops.
    pub fn section_context(
        &self,
        section_pos: SectionPos,
    ) -> crate::render::mesh::SectionContext<'_> {
        use crate::render::mesh::SectionContext;

        let mut sections = [None; 27];
        let mut light = [None; 27];

        for dy in -1..=1i32 {
            for dz in -1..=1i32 {
                for dx in -1..=1i32 {
                    let target =
                        SectionPos::new(section_pos.x + dx, section_pos.y + dy, section_pos.z + dz);
                    let slot =
                        (((dx + 1) as usize) * 3 + (dy + 1) as usize) * 3 + (dz + 1) as usize;

                    // Outside the world's height is genuinely nothing, and must
                    // read as absent rather than as an empty section: the top of
                    // the world is open sky, so a phantom section there would
                    // turn it into a ceiling.
                    if target.y < MIN_SECTION_Y
                        || target.y >= MIN_SECTION_Y + SECTIONS_PER_CHUNK as i32
                    {
                        continue;
                    }
                    let Some(chunk) = self.chunk(ChunkPos::new(target.x, target.z)) else {
                        continue;
                    };
                    sections[slot] = chunk.section(target);
                    light[slot] = chunk.section_light(target);
                }
            }
        }

        SectionContext::from_parts(sections, light)
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
        sections.extend(self.relight_around(pos));

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

    /// Recomputes sky light around `center`, for use after [`Self::set_block_quiet`].
    pub fn relight(&mut self, center: BlockPos) {
        self.relight_around(center);
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

#[cfg(test)]
mod tests;
mod tick_tests;

#[cfg(test)]
mod sky_light_tests;
