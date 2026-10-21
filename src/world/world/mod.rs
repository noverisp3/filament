//! The world: chunks, the block registry, and the queries that read them.
//!
//! This sits between storage and rendering. The renderer never owns world
//! data; it borrows a `&World` when it needs geometry, which keeps the rule
//! that logic in `world/` does not depend on wgpu, and the reverse.

use std::collections::{HashMap, HashSet};

use super::block::{BlockId, BlockRegistry};
use super::chunk::{Chunk, MIN_SECTION_Y, SECTIONS_PER_CHUNK};
use super::pos::{BlockPos, ChunkPos, Direction, SectionPos};
use super::section::{Section, SectionNeighbors};
use super::terrain::{TerrainBlocks, generate_chunk, register_terrain};

mod blocks;
mod chunks;
mod edits;
mod items;
mod light;

pub use edits::BlockEdit;
use edits::replay_edits;

pub struct World {
    registry: BlockRegistry,
    terrain: TerrainBlocks,
    chunks: HashMap<ChunkPos, Chunk>,
    /// What has been changed, and where, separately from the chunks themselves.
    ///
    /// A chunk is thrown away the moment the last player walks away from it, so
    /// an edit that lives only in the chunk is gone by then: the report from the
    /// field was a hole that filled itself back in.
    ///
    /// The edits are kept out here, keyed by chunk, and replayed onto a chunk as
    /// it arrives. The journal outlives the chunk, so the chunk can be as
    /// disposable as it likes.
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
    /// is told so that the sky matches the world it is standing in.
    day_time: u64,

    /// Work the world owes itself, ordered by when.
    ticks: crate::world::ticks::BlockTicks,

    /// What a tick changed on its own, waiting for a connection to send it.
    ///
    /// Separate from `edits`, which is the whole history and is never drained:
    /// this is only what has happened since someone last asked.
    tick_changes: Vec<(BlockPos, BlockId)>,

    /// Sections whose light moved, waiting to be told about.
    ///
    /// The client keeps the light it was given with the chunk, so a hole dug
    /// in a lit room is still lit on the client's screen until somebody tells
    /// it otherwise. This is what tells it.
    pub(crate) light_changes: Vec<SectionPos>,

    /// Whether the light of the world is out of date.
    ///
    /// A chunk that arrives brings its own columns but not the light that
    /// comes in sideways, so the world as a whole is out of date until it is
    /// worked out again. This is what says so, so that the work is done once
    /// rather than once per player.
    pub(crate) light_stale: bool,

    /// The cells the light has been worked out for, since the world began.
    pub(crate) light_work: usize,

    /// The chunks that hold a block which gives off light.
    ///
    /// Whether there is a lamp near a chunk used to mean reading every block of
    /// the chunk and of the eight around it: nine hundred thousand blocks,
    /// asked nine times over for every chunk that arrived. The answer is kept
    /// instead.
    lamp_chunks: HashSet<ChunkPos>,

    /// The items lying on the ground, waiting to be picked up.
    ///
    /// A broken block becomes one of these instead of going straight into a
    /// player's pocket: the game drops it, and the player walks over it to
    /// pick it up. An item that is still falling moves every tick, and one
    /// that has landed stays where it is until somebody picks it up or it
    /// goes away of its own accord.
    items: Vec<super::item_entity::ItemEntity>,

    /// The items that moved, waiting to be told about.
    ///
    /// A client is told where an item is when it appears and when it is
    /// picked up, and in between it does not need to be told: an item that
    /// has landed does not move.
    pub(crate) item_changes: Vec<super::item_entity::ItemEntity>,

    /// Where the world came from and will go back to. None means memory only.
    store: Option<std::sync::Arc<super::storage::WorldStore>>,

    /// Where a player arrives, in blocks, as the world file says.
    ///
    /// The file's own answer, not a guess: a world that was saved has a spawn
    /// the game chose, and arriving somewhere else is arriving in the wrong
    /// place.
    spawn: (i32, i32, i32),

    /// Whether the spawn came from the world file.
    ///
    /// A world that was made here rather than read has no file to ask, and
    /// the ground it generated is the only answer there is: a spawn in the air
    /// is a player who falls the whole way down and dies on arrival.
    spawn_from_file: bool,
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
            light_changes: Vec::new(),
            light_stale: false,
            light_work: 0,
            lamp_chunks: self.lamp_chunks.clone(),
            items: Vec::new(),
            item_changes: Vec::new(),
            // A clone has no folder of its own; sharing one would mean two
            // worlds writing the same region files.
            store: None,
            spawn: self.spawn,
            spawn_from_file: self.spawn_from_file,
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
    light_changes: Vec::new(),
    light_stale: false,
    lamp_chunks: HashSet::new(),
    light_work: 0,
    items: Vec::new(),
    item_changes: Vec::new(),
    store: store.map(std::sync::Arc::new),
    spawn: (0, 64, 0),
    spawn_from_file: false,
};

    // The clock the world was saved with. A restart that wound `Time` back
    // would replay a day through every crop and furnace in it, so what is in
    // `level.dat` is what the ticks continue from.
    //
    // The spawn comes from the same file, and so does the day of the year
    // the world was left at: both are things the game wrote down and both
    // are lost if this server saves the file without them.
    if let Some((_name, level)) = world.store.as_ref().and_then(|store| store.read_level()) {
        let data = level.compound("Data").unwrap_or(&level);
        if let Some(super::nbt::Tag::Long(t)) = data.get("Time") {
            world.game_time = *t as u64;
        }
        if let Some(super::nbt::Tag::Long(t)) = data.get("DayTime") {
            world.day_time = *t as u64;
        } else if let Some(clocks) = world.store.as_ref().and_then(|store| store.read_clocks())
            && let Some(data) = clocks.compound("data")
            && let Some(overworld) = data.compound("minecraft:overworld")
            && let Some(super::nbt::Tag::Long(t)) = overworld.get("total_ticks")
        {
            // The game moved the day counter out of the level file and into a
            // clock file of its own. That is where the sun's position lives
            // now, and a day counter of zero would put the sun at dawn every
            // time the server started.
            world.day_time = *t as u64;
        }
        if let Some(super::nbt::Tag::Compound(spawn)) = data.get("spawn")
            && let Some(super::nbt::Tag::IntArray(pos)) = spawn.get("pos")
            && pos.len() >= 3
        {
            world.spawn = (pos[0], pos[1], pos[2]);
            world.spawn_from_file = true;
        }
    }

    // A world with no file to ask has the ground it generated, and a player
    // stands on that. A spawn in the air is a player who falls the whole way
    // down and dies on arrival, which is what a fixed height was.
    if !world.spawn_from_file {
        let (x, y, z) = crate::world::terrain::spawn_point();
        world.spawn = (x as i32, y as i32, z as i32);
    }
    world
}

    /// Writes the chunks that changed back to their region files.
    ///
    /// A chunk loaded from disk and left alone is not written again: rewriting
    /// every loaded region on every save is what turned an idle session into
    /// tens of megabytes of disk traffic.
    pub fn flush(&mut self) -> std::io::Result<usize> {
        let game_time = self.game_time;
        let Some(store) = self.store.as_mut() else {
            return Ok(0);
        };
        let dirty: Vec<ChunkPos> = self.dirty.iter().copied().collect();
        let mut ready: Vec<(ChunkPos, Chunk)> = Vec::with_capacity(dirty.len());
        for pos in dirty {
            // The chunk as it stands. If it is not -- a player walked away and
            // it was thrown out -- then it has to come back from the folder,
            // because that is where the ground the game saved is. Building it
            // from the noise instead would write the generator's ground over
            // the ground that was there.
            let chunk = if let Some(chunk) = self.chunks.get(&pos) {
                chunk.clone()
            } else if let Some(mut chunk) = store.load(pos, &mut self.registry) {
                replay_edits(&mut chunk, self.edits.get(&pos));
                chunk
            } else {
                // Nothing on disk either: this is a chunk the world made, and
                // the generator is the only thing that can make it again.
                let mut chunk = crate::world::terrain::generate_chunk(pos, &self.terrain);
                replay_edits(&mut chunk, self.edits.get(&pos));
                chunk
            };
            ready.push((pos, chunk));
        }
        if ready.is_empty() {
            return Ok(0);
        }

        // All of them at once, so that a region is written once rather than
        // once per chunk in it.
        let borrowed: Vec<(ChunkPos, &Chunk)> = ready
            .iter()
            .map(|(pos, chunk)| (*pos, chunk))
            .collect();
        match store.save_many(&borrowed, &self.registry, game_time) {
            Ok(written) => {
                for (pos, _) in &ready {
                    self.dirty.remove(pos);
                }
                Ok(written)
            }
            Err(e) => {
                eprintln!("cannot save the world: {e}");
                Ok(0)
            }
        }
    }

    /// Writes `level.dat`, if there is a world folder to write it into.
    ///
    /// The file as it was, with only the clock changed. A file the game wrote
    /// holds the seed, the spawn, the game rules and the world borders, and
    /// replacing it with a default loses every one of them -- the day of the
    /// year resets to zero and the player arrives somewhere else, both
    /// without a word of complaint.
    pub fn write_level(&self) -> std::io::Result<()> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let (level_name, mut level) = match store.read_level() {
            Some((name, level)) => (name, level),
            None => (
                "Data".to_string(),
                super::storage::default_level(
                    store.name(),
                    self.game_time,
                    self.day_time,
                    self.spawn,
                ),
            ),
        };
        // The file the game writes has its data under a name; a file this
        // build wrote has it at the top. Either way the clock goes in the
        // place the fields are.
        let had_day_time = {
            let data = match level.compound_mut("Data") {
                Some(data) => data,
                None => &mut level,
            };
            data.set("Time", super::nbt::Tag::Long(self.game_time as i64));
            // The day counter only goes in where it already was: the game
            // moved it to a clock file of its own, and a file that has it in
            // two places is a file the game has to fix.
            let had = data.contains("DayTime");
            if had {
                data.set("DayTime", super::nbt::Tag::Long(self.day_time as i64));
            }
            had
        };
        store.write_level(&level_name, &level)?;
        if !had_day_time {
            // The day counter belongs in the clock file, and is put there so
            // the game finds it where it looks.
            store.write_clocks(self.day_time)?;
        }
        Ok(())
    }

    /// Where a player arrives, in blocks.
    pub fn spawn_point(&self) -> (i32, i32, i32) {
        self.spawn
    }

    /// The spawn, put on the ground if it is not on it already.
    ///
    /// A spawn with nothing under it is a player who falls the whole way down
    /// and dies on arrival. The game does not write such a spawn, but this
    /// server did once, when the spawn was a fixed height, and a world file
    /// that says so is still out there with the world's name on it.
    ///
    /// So the ground is looked for: a spawn with a block within a step or two
    /// under it is left alone, because that is a player standing on something,
    /// and one with nothing under it is moved down to the topmost solid block of
    /// its column.
    pub fn settle_spawn(&mut self) -> (i32, i32, i32) {
        let (x, y, z) = self.spawn;
        self.ensure_chunk(ChunkPos::new(x.div_euclid(16), z.div_euclid(16)));

        for step in 1..=3 {
            if self.is_solid(super::pos::BlockPos::new(x, y - step, z)) {
                return self.spawn;
            }
        }

        // Nothing under it: the topmost solid block of the column is where a
        // player stands.
        for check in (super::chunk::MIN_BLOCK_Y..=y).rev() {
            if self.is_solid(super::pos::BlockPos::new(x, check, z)) {
                self.spawn = (x, check + 1, z);
                break;
            }
        }
        self.spawn
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

    /// Sets the time of day.
    ///
    /// The sun's place is the time of day and not the age: the age is how long
    /// the world has existed and it does not stop, while the time of day is
    /// where in the cycle the world is, and a command that sets it moves the
    /// sun and nothing else.
    pub fn set_day_time(&mut self, ticks: u64) {
        self.day_time = ticks;
    }

    /// Promises to come back to this position in `delay` ticks.
    pub fn schedule_tick(&mut self, pos: BlockPos, delay: u64) {
        self.ticks.schedule(self.game_time, delay, pos);
    }

    /// Promises to come back to this position in `delay` ticks, as one of the
    /// game's random ticks.
    pub fn schedule_random_tick(&mut self, pos: BlockPos, delay: u64) {
        self.ticks.schedule_random(self.game_time, delay, pos);
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
    /// The game's `updateNeighborsAt`: a block cannot know what happened next to
    /// it, so the change goes round. It is scheduled rather than run because a
    /// neighbour update may change something else, and answering that inline
    /// turns one edit into a chain of them all.
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
    /// Everything the game does on a delay -- a block that wants to check its
    /// neighbours again, water looking for somewhere to go, an item waiting to
    /// be picked up -- comes back through here.
    ///
    /// The order is the game's (`ServerLevel.tick`): work that is due is taken
    /// under a budget, and what does not fit waits for the next tick.
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
        for (pos, random) in due {
            if random {
                self.random_update(pos, &mut edit);
            } else {
                self.block_update(pos, &mut edit);
            }
        }
        self.tick_items();
        edit
    }

    /// The items on the ground fall, and the ones nobody wants go away.
    ///
    /// An item falls until it meets something solid and then it lies there.
    /// The game's item falls a little slower than a body does and stops
    /// sooner, and the difference does not matter to anybody watching.
    fn tick_items(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let mut gone: Vec<i32> = Vec::new();
        // The items are taken out while they are moved, because asking the
        // world whether a block is solid needs the world, and the world is
        // busy holding the items.
        let mut items = std::mem::take(&mut self.items);
        for item in &mut items {
            item.age = item.age.wrapping_add(1);
            if !item.falling {
                continue;
            }
            // Down it goes, until the block underneath says stop.
            let below = BlockPos::new(
                item.x.floor() as i32,
                (item.y - 0.05).floor() as i32,
                item.z.floor() as i32,
            );
            if self.is_solid(below) {
                item.falling = false;
                item.y = below.y as f64 + 1.0;
            } else {
                item.y -= 0.1;
            }
        }
        // And an item that has been lying about for five minutes is gone,
        // which is the game's own rule: the world does not fill up with the
        // rubble of a week.
        for item in &items {
            if item.age >= super::item_entity::LIFETIME {
                gone.push(item.id);
            }
        }
        if !gone.is_empty() {
            items.retain(|item| !gone.contains(&item.id));
        }
        self.items = items;
    }

    /// One block's turn to notice something.
    ///
    /// The game re-reads the block and only acts if it is still the thing that
    /// was asked. A player can break a block between it being scheduled and its
    /// turn coming round, and a rule that acts on the stale name writes into a
    /// block that is now something else.
    fn block_update(&mut self, pos: BlockPos, _edit: &mut BlockEdit) {
        // Nothing is answered here. There used to be a rule that turned grass
        // into dirt when the block under it was not dirt: the game has no such
        // rule, and it answered at once, so every block a player put down next
        // to a hillside was rewritten before the client had drawn it. The rules
        // the game does have about grass are both random ticks.
        let _ = pos;
    }

    /// The rules the game leaves to chance, and the only ones the world has.
    ///
    /// The two things it does about grass: a grass block that cannot stay
    /// alive becomes dirt, and dirt near a grass block becomes grass.
    ///
    /// The game rolls both on a random tick and it needs light: a dirt block in
    /// the dark stays dirt. They are meant to be rare, so that a hillside heals
    /// itself over minutes and not the moment a player puts a block down.
    fn random_update(&mut self, pos: BlockPos, edit: &mut BlockEdit) {
        let Some(block) = self.block(pos) else { return };

        // Grass that cannot stay alive.
        if self.is_grass(block) {
            let above = self.block(pos.offset(Direction::Up));
            let shut_in = above.map(|b| self.registry.is_opaque(b)).unwrap_or(false);
            if shut_in {
                let dirt = self.dirt_block();
                if let Some(made) = self.set_block(pos, dirt) {
                    self.tick_changes.push((pos, dirt));
                    edit.merge(made);
                }
            }
            return;
        }

        if !self.is_dirt(block) {
            return;
        }

        // Light. The game will not grow grass in the dark.
        //
        // The light is read in the air above, because that is where the grass
        // will be: the light inside a block of dirt is nothing at all, being
        // inside a block.
        let above = pos.offset(Direction::Up);
        if self.sky_light(above).max(self.block_light(above)) < 9 {
            return;
        }

        // And room above it: grass needs the air.
        let above = self.block(above);
        if !above.map(|b| self.is_air(b)).unwrap_or(true) {
            return;
        }

        // A grass block within reach: one across, and three up or down.
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -3..=3 {
                    let at = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                    let Some(other) = self.block(at) else {
                        continue;
                    };
                    if !self.is_grass(other) {
                        continue;
                    }
                    let grass = self.grass_block();
                    if let Some(made) = self.set_block(pos, grass) {
                        self.tick_changes.push((pos, grass));
                        edit.merge(made);
                    }
                    return;
                }
            }
        }
    }

    /// Whether a block is grass, by the name the game gives it.
    ///
    /// By name and not by id, because a world read from a file has its own
    /// grass block and its own dirt, registered under the game's names, and
    /// they are not the blocks the generator made.
    pub fn is_grass(&self, id: BlockId) -> bool {
        id == self.terrain.grass || self.registry.name(id) == "minecraft:grass_block"
    }

    /// Whether a block is dirt, by the name the game gives it.
    pub fn is_dirt(&self, id: BlockId) -> bool {
        id == self.terrain.dirt || self.registry.name(id) == "minecraft:dirt"
    }

    /// Whether a block is air, by the name the game gives it.
    pub fn is_air(&self, id: BlockId) -> bool {
        id == self.terrain.air || self.registry.name(id) == "minecraft:air"
    }

    /// The dirt this world uses.
    ///
    /// The world's own dirt if it has one -- a world read from a file has the
    /// game's dirt -- and the generator's otherwise.
    pub fn dirt_block(&self) -> BlockId {
        self.registry
            .id("minecraft:dirt")
            .unwrap_or(self.terrain.dirt)
    }

    /// The grass this world uses, in the same way as [`Self::dirt_block`].
    pub fn grass_block(&self) -> BlockId {
        self.registry
            .id("minecraft:grass_block")
            .unwrap_or(self.terrain.grass)
    }

    /// A number that changes, for the rules the game leaves to chance.
    ///
    /// The game rolls dice for the rules that happen now and then, and this is
    /// that dice: the same block at the same moment always gets the same
    /// answer, and a different moment gets a different one.
    fn dice(&self, pos: BlockPos, salt: u64) -> u64 {
        let mut hash = self.game_time ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        hash ^= u64::from(pos.x as u32) << 32
            | u64::from(pos.y as u32) << 16
            | u64::from(pos.z as u32);
        // The mix is splitmix64's: it spreads a small change in the input
        // across the whole output, so two neighbouring blocks do not get the
        // same answer.
        hash = hash.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = hash;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// A full day, in ticks. Twenty-four thousand, which is what a Minecraft day
    /// is: twenty minutes at twenty a second.
    pub const DAY_LENGTH: u64 = 24_000;

    /// Builds a world of generated terrain.
    ///
    /// Takes the chunk radius to generate along each axis.
    pub fn generate(radius: i32) -> Self {
        let mut registry = BlockRegistry::new();
        let terrain = register_terrain(&mut registry);

        let mut world = Self {
            registry,
            terrain,
            chunks: HashMap::new(),
            store: None,
            edits: HashMap::new(),
            dirty: HashSet::new(),
            game_time: 0,
            day_time: 0,
            ticks: crate::world::ticks::BlockTicks::new(),
            tick_changes: Vec::new(),
            light_changes: Vec::new(),
            light_stale: false,
            light_work: 0,
            lamp_chunks: HashSet::new(),
            items: Vec::new(),
            item_changes: Vec::new(),
            spawn: (0, 64, 0),
            spawn_from_file: false,
        };

        // Through `store_chunk`, so that every chunk has its light worked out
        // the way a chunk that arrives from a file would.
        for x in -radius..=radius {
            for z in -radius..=radius {
                let pos = ChunkPos::new(x, z);
                let chunk = generate_chunk(pos, &world.terrain);
                world.store_chunk(pos, chunk);
            }
        }

        // And the spawn is on the ground this world made, not in the air
        // above it.
        let (x, y, z) = crate::world::terrain::spawn_point();
        world.spawn = (x as i32, y as i32, z as i32);

        world
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
}

/// A generated world, for the tests of this module.
///
/// One copy, because three of the test files wanted one.
#[cfg(test)]
pub(crate) fn test_world() -> World {
    World::generate(1)
}

/// The stone block, for the tests of this module.
#[cfg(test)]
pub(crate) fn stone(world: &World) -> BlockId {
    world.registry().id("stone").expect("registered")
}

/// The air block, for the tests of this module.
#[cfg(test)]
pub(crate) fn air(world: &World) -> BlockId {
    world.registry().id("air").expect("registered")
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tick_tests;

#[cfg(test)]
mod sky_light_tests;

#[cfg(test)]
mod block_light_tests;

#[cfg(test)]
mod item_tests;
