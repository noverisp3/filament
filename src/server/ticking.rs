//! The world, and the thread that owns it.
//!
//! # Why one thread
//!
//! The world used to sit behind a mutex that every connection locked to read a
//! chunk. That is correct and it is also a queue: the two hundred and eighty-nine
//! chunks of an initial join are generated and serialised while the lock is
//! held, which is about a second during which no other player can generate
//! anything. Two players joining means the second waits for the first.
//!
//! Moving the world onto a thread of its own fixes that in the direction that
//! matters. A connection no longer touches the world at all: it asks for a chunk
//! and waits for the bytes. The work is still serialised, because one world with
//! one writer is a much easier thing to be correct about than one world with
//! several, but it is serialised on a thread that is doing nothing else, and the
//! connections waiting on it are not holding anything up.
//!
//! This is the shape the game's own server uses, and for the same reason.
//!
//! # What the loop does per tick
//!
//! Nothing yet, and it would be dishonest to imply otherwise. It ticks, and the
//! things it will do each tick are not written: the world clock, which is not
//! even sent yet, and mining progress. The point of landing this first is that
//! every one of those needs a tick, and a tick needs somewhere to live that is
//! not a connection thread that is blocked on a socket.
//!
//! # Where this stops being enough
//!
//! One world on one thread caps chunk generation at one core. That is the right
//! first answer and it will not be the last: splitting the world across threads
//! wants the world partitioned, and that is a much larger change than a channel.

use crate::world::chunk::Chunk;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::world::block::BlockId;
use crate::world::pos::{BlockPos, ChunkPos};
use crate::world::section;
use crate::world::terrain::TerrainBlocks;
use crate::world::world::World;

/// Twenty a second, which is what a client is told to expect and what its
/// interpolation assumes.
pub const TICKS_PER_SECOND: u32 = 20;

/// How long one tick is.
pub const TICK: Duration = Duration::from_millis(1000 / TICKS_PER_SECOND as u64);

/// The most ticks that will be run in one go after falling behind.
///
/// Without a bound, a process suspended for a minute comes back and runs sixty
/// seconds of ticks before it does anything else, which is worse than being
/// behind. Dropping ticks is the right answer here because nothing written yet
/// depends on catching up.
const MAX_CATCH_UP: u32 = 5;

/// What a connection can ask the world to do.
///
/// Every one of these is answered by the thread that owns the world, and none of
/// them can be answered by a caller holding a reference to it, because no caller
/// has one. That is the property the whole arrangement is for.
pub enum Command {
    /// One chunk, as the body of a `level_chunk_with_light` packet.
    Chunk { pos: ChunkPos, reply: Sender<Vec<u8>> },
    /// One chunk, but only if it is already here.
    ///
    /// Distinct from `Chunk` because the difference is the whole point: `Chunk`
    /// builds it if it is missing, which takes as long as it takes, and asking
    /// whether it is here must not be the thing that finds out.
    StoredChunk {
        pos: ChunkPos,
        reply: Sender<Option<Vec<u8>>>,
    },
    /// Break a block, and report the state now standing there.
    BreakBlock { pos: BlockPos, reply: Sender<Option<i32>> },
    /// Put a block back, the other way round from breaking one.
    ///
    /// Through [`World::set_block`] rather than `set_block_quiet`, so that it
    /// is remembered in the journal: a block placed and then walked away from
    /// would otherwise be gone when the chunk came back, which is the same fault
    /// in the other direction and was the one two commits ago.
    SetBlock {
        pos: BlockPos,
        block: BlockId,
        reply: Sender<bool>,
    },
    /// The state at a position, if the chunk holding it has been built.
    BlockAt { pos: BlockPos, reply: Sender<Option<i32>> },
    /// The world's own block id at a position.
    ///
    /// Not the game's state id. Hardness is keyed by what the world calls a
    /// block, and handing out a state id would mean walking back through
    /// thirty-five thousand of them to find out which block was mined.
    WorldBlock {
        pos: BlockPos,
        reply: Sender<Option<BlockId>>,
    },
    /// Drop a chunk the player has walked away from.
    Forget { pos: ChunkPos },
    /// The terrain palette, which is five block ids and never changes.
    TerrainBlocks { reply: Sender<TerrainBlocks> },
    /// How many chunks the world is holding.
    ChunkCount { reply: Sender<usize> },
    /// Every remembered block edit, by the chunk it was made in.
    Edits {
        reply: Sender<Vec<(ChunkPos, usize)>>,
    },
}

/// What a connection holds: a way to ask, not a world.
///
/// Cheap to clone, and the clone is another way to ask rather than another world.
/// The old shared world was an `Arc<Mutex<World>>` and cloning it handed out a
/// second way to reach the world; here the only thing reachable is the channel.
#[derive(Clone)]
pub struct WorldHandle {
    to_world: Sender<Command>,
    /// Where chunks to build are handed over. Closed when every handle is
    /// dropped, which is how the workers learn to stop.
    work: Sender<ChunkPos>,
    /// The queue of chunks to build, and the world's record of what it has.
    pool: Pool,
}

/// A chunk a worker finished, on its way into the world.
type Pending = (ChunkPos, Chunk);

/// Positions queued or being built, so a second request for one does not pay
/// for it twice and a worker finishing one lets the next request through.
impl WorldHandle {
    /// Sends a command and waits for its answer.
    ///
    /// `None` means the thread that owns the world has gone, which happens when
    /// the server is shutting down. It is not a reason to panic: a player
    /// disconnecting during shutdown is ordinary, and a panic per connection
    /// would bury the one line that says why.
    fn ask<T>(&self, build: impl FnOnce(Sender<T>) -> Command) -> Option<T> {
        let (reply, answer) = channel();
        self.to_world
            .send(build(reply))
            .map_err(|_| ())
            .ok()?;
        answer.recv().ok()
    }
    /// Asks for a chunk without waiting for it to be built.
    ///
    /// Returns the body if the world already has that chunk, which is the usual
    /// case once a worker has got to it. Otherwise the work goes to the pool and
    /// `None` comes back now; asking again later returns it.
    ///
    /// The point of not waiting is the player this is called for. Generating a
    /// chunk is the expensive half of answering, and doing it on the world's own
    /// thread meant a player walking into new terrain stopped answering their
    /// socket while it happened — so they sent no movement, and everyone
    /// watching saw them stand still for as long as the ground took to build.
    /// The client's own hitch is the client's; that one was ours.
    pub fn want_chunk(&self, pos: ChunkPos) -> Option<Vec<u8>> {
        // The cheap question first: is it here? A hash lookup against the
        // world's own record, with no thread woken and no message sent.
        //
        // This is the difference between asking the world's thread a few hundred
        // times per pass — once per chunk in the ring a player is walking into,
        // every pass, for as long as the pool is working — and asking it only
        // about chunks that have actually turned up. That was roughly a third of
        // the world's thread, spent answering no.
        if !self.pool.is_known(pos) {
            if self.pool.claim(pos) {
                // Nothing else has this one. If the pool has gone, undo the claim
                // so a later attempt is not blocked by a dead channel.
                if self.work.send(pos).is_err() {
                    self.pool.claim_released(pos);
                }
            }
            return None;
        }

        self.ask(|reply| Command::StoredChunk { pos, reply })
            .flatten()
    }
    ///
    /// One chunk, ready to frame and send.
    /// and the world does not know about compression or framing.
    pub fn chunk(&self, pos: ChunkPos) -> Option<Vec<u8>> {
        self.ask(|reply| Command::Chunk { pos, reply })
    }

    /// Breaks a block and reports what is now there.
    ///
    /// `None` if there was nothing to break, which covers air and a chunk the
    /// world has never built.
    pub fn break_block(&self, pos: BlockPos) -> Option<Option<i32>> {
        self.ask(|reply| Command::BreakBlock { pos, reply })
    }

    /// Puts a block down, and reports whether it went anywhere.
    ///
    /// `false` means the world declined: no chunk holding that position has been
    /// built, or the block was already what was being put there. Neither is an
    /// error and neither should cost the player the block they were holding.
    pub fn set_block(&self, pos: BlockPos, block: BlockId) -> Option<bool> {
        self.ask(|reply| Command::SetBlock { pos, block, reply })
    }

    /// The state at a position, if the chunk holding it exists.
    pub fn block_at(&self, pos: BlockPos) -> Option<Option<i32>> {
        self.ask(|reply| Command::BlockAt { pos, reply })
    }

    /// The world's own block id at a position, for anything keyed by block rather
    /// than by what the game calls it.
    pub fn world_block(&self, pos: BlockPos) -> Option<Option<BlockId>> {
        self.ask(|reply| Command::WorldBlock { pos, reply })
    }

    /// The terrain generator's block ids, for anything that needs to recognise a
    /// block: hardness, mostly.
    ///
    /// A copy, because handing out a borrow would mean holding the lock across
    /// the caller, and the lock is on the other side of a channel.
    pub fn terrain_blocks(&self) -> Option<TerrainBlocks> {
        self.ask(|reply| Command::TerrainBlocks { reply })
    }

    /// Drops a chunk the player has walked away from.
    pub fn forget(&self, pos: ChunkPos) -> bool {
        self.to_world
            .send(Command::Forget { pos })
            .map(|_| true)
            .unwrap_or(false)
    }

    /// How many chunks the world holds, for the log and for tests.
    pub fn chunk_count(&self) -> Option<usize> {
        self.ask(|reply| Command::ChunkCount { reply })
    }

    /// How many block edits are remembered, and which chunks hold them.
    ///
    /// This is the whole of what a server needs to keep to have a world that
    /// outlives its chunks. It is asked for rather than held here because the
    /// edits live on the thread that owns the world.
    pub fn edits(&self) -> Option<Vec<(ChunkPos, usize)>> {
        self.ask(|reply| Command::Edits { reply })
    }

    /// The number of remembered edits, over every chunk.
    pub fn edit_count(&self) -> Option<usize> {
        self.edits().map(|list| list.iter().map(|(_, n)| n).sum())
    }
}

/// How many terrain workers to run on a machine with `cores` of it.
///
/// Kept apart from the thread spawning so it can be checked rather than
/// guessed. It was four, hard, and the fault only shows on a machine that does
/// not have four cores: half the CPU sits idle, and the opening batch of chunks
/// takes twice as long as it should. On eight cores that was five to eight
/// seconds of a player waiting to arrive.
///
/// The bounds matter more than the subtraction. At least two, so a small machine
/// still builds chunks in parallel with its world thread, but never more than
/// the machine has cores, since a thread that has nothing to run on costs a
/// context switch and buys nothing. At most eight, so a large one does not
/// starve the connection threads, which answer keep alives and movement and are
/// the ones that notice when the pool falls behind.
fn worker_count(cores: usize) -> usize {
    cores.saturating_sub(2).clamp(2, 8).min(cores)
}

/// Starts the thread that owns the world, and returns the handle connections use.
///
/// The thread stops when every handle is dropped, since the channel closes with
/// them. That is what lets a test build a world, use it and have it go away
/// without reaching into the runtime.
pub fn spawn() -> WorldHandle {
    let (to_world, from_connections) = channel();
    let world = World::empty();
    let terrain = *world.terrain_blocks();

    let (work_tx, work_rx) = channel();
    let work_rx = Arc::new(Mutex::new(work_rx));
    let pending: Arc<Mutex<Vec<Pending>>> = Arc::new(Mutex::new(Vec::new()));
    let pool = Pool::default();

    std::thread::Builder::new()
        .name("world".to_string())
        .spawn({
            let pending = Arc::clone(&pending);
            let pool = pool.clone();
            move || run(world, from_connections, pending, pool, |_| {})
        })
        .expect("a thread can be spawned");

    // Workers.
    //
    // The terrain generator is a pure function of a chunk's position and the
    // block palette, so any number of these can run at once with nothing to
    // share and nothing to lock.
    //
    // How many used to be four, written down because the machine it was written
    // on had four real cores. That is a guess with a shelf life, and it expired:
    // on eight cores it used half the machine, which showed up as a player
    // waiting five to eight seconds for their opening batch of two hundred and
    // eighty-nine chunks, and a second player waiting longer still behind them
    // in the queue.
    //
    // Asked for instead, and never all of them: the world thread above needs
    // one, the connection threads want to keep answering keep alives and
    // movement, and the mesh and lighting are done by whoever picked the chunk
    // up. A pool that takes every core leaves nothing for the threads whose job
    // is to notice the pool is behind.
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let workers = worker_count(cores);
    for worker in 0..workers {
        let rx = Arc::clone(&work_rx);
        let pending = Arc::clone(&pending);
        std::thread::Builder::new()
            .name(format!("terrain-{worker}"))
            .spawn(move || {
                loop {
                    // The lock is held while waiting for the next job, not
                    // while building it, so the workers do run in parallel.
                    let pos = {
                        let rx = rx.lock().unwrap_or_else(|e| e.into_inner());
                        // Closed when every handle is gone: the server has
                        // shut down and there is nothing left to build for.
                        let Ok(pos) = rx.recv() else { return };
                        pos
                    };

                    let mut chunk = crate::world::terrain::generate_chunk(pos, &terrain);
                    // Written out here, where the chunk is already in this
                    // worker's hands, rather than on the world's own thread when
                    // it is asked for.
                    //
                    // Writing a chunk out is about forty times the cost of
                    // making one -- eleven milliseconds against a quarter of a
                    // millisecond -- and it used to be done on the one thread
                    // that everything else also runs on. Adding workers did
                    // nothing for a player's wait, because the wait was never in
                    // the part they were parallelising. Two hundred and
                    // eighty-nine chunks at eleven milliseconds is the three
                    // seconds a player spent looking at terrain arrive.
                    //
                    // The body is dropped again by set_block, so a chunk that is
                    // edited after this is written out afresh when it is next
                    // asked for. That is one chunk, once, rather than every
                    // chunk for every player.
                    chunk.set_body(|c| encode_chunk_body(c, &terrain, pos));
                    pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((pos, chunk));
                }
            })
            .expect("a thread can be spawned");
    }

    WorldHandle {
        to_world,
        work: work_tx,
        pool,
    }
}

/// The two sets the world and the generation pool share.
///
/// Both are read and written from two places — the world's own thread and the
/// connections asking it questions — so both sit behind a short lock, and both
/// are touched for a hash lookup and nothing longer.
#[derive(Clone, Default)]
pub struct Pool {
    /// Queued or being built, so a second request for one does not pay twice.
    queued: Arc<Mutex<std::collections::HashSet<ChunkPos>>>,
    /// Built and in the world.
    ///
    /// This exists to answer "is it here yet?" without asking the world's
    /// thread, because that question is asked a great deal. A connection walking
    /// into new ground asks about a whole ring of chunks on every pass of its
    /// loop, and asking the thread each time woke it up hundreds of times to
    /// say no. The thread being woken is the expensive part, not the lookup.
    ///
    /// It is updated on the world's thread at the moment a chunk goes in or
    /// comes out and nowhere else, so it cannot come to disagree with the world
    /// about what exists — a set that drifted would either hide a chunk that is
    /// there or send one that is not.
    known: Arc<Mutex<std::collections::HashSet<ChunkPos>>>,
}

impl Pool {
    /// The world now has this chunk.
    fn stored(&self, pos: ChunkPos) {
        self.known.lock().unwrap_or_else(|e| e.into_inner()).insert(pos);
    }

    /// The world no longer has it, so a request for it may be claimed afresh.
    fn dropped(&self, pos: ChunkPos) {
        self.known.lock().unwrap_or_else(|e| e.into_inner()).remove(&pos);
        self.queued.lock().unwrap_or_else(|e| e.into_inner()).remove(&pos);
    }

    /// Claims a chunk for building. False if somebody already has.
    fn claim(&self, pos: ChunkPos) -> bool {
        self.queued.lock().unwrap_or_else(|e| e.into_inner()).insert(pos)
    }

    /// Gives a claim back, for when there is nobody left to do the work.
    fn claim_released(&self, pos: ChunkPos) {
        self.queued.lock().unwrap_or_else(|e| e.into_inner()).remove(&pos);
    }

    fn is_known(&self, pos: ChunkPos) -> bool {
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&pos)
    }
}

/// The parts of the generation pool, for a world driven without one.
///
/// The sender's receiver is dropped on the floor, so nothing is ever queued: a
/// test that asks for a missing chunk gets nothing back and has to ask again,
/// which is the honest shape of the question. Tests that want a chunk built ask
/// through [`WorldHandle::chunk`], which still builds it on the world's own
/// thread.
pub fn no_pool() -> (Pool, Sender<ChunkPos>) {
    let (work, dropped) = channel();
    // Keep the receiver alive so the sender does not report a broken pipe: a
    // test world has no workers, and a closed channel would look like a server
    // shutting down.
    std::mem::forget(dropped);
    (Pool::default(), work)
}

/// The loop. Split out so a test can drive it without a thread.
pub fn run(
    mut world: World,
    commands: Receiver<Command>,
    pending: Arc<Mutex<Vec<Pending>>>,
    pool: Pool,
    mut tick: impl FnMut(&mut World),
) -> u32 {
    let mut ticks = 0;
    let mut next_tick = Instant::now() + TICK;

    loop {
        let now = Instant::now();
        let wait = next_tick.saturating_duration_since(now);

        // Take in anything the terrain pool has finished.
        //
        // Drained here rather than through a command so that a worker needs
        // nothing from this thread's channel, and therefore nothing that keeps
        // this thread alive: the pool stops when the last handle goes, and this
        // stops when the same thing happens.
        {
            let mut ready = pending.lock().unwrap_or_else(|e| e.into_inner());
            for (pos, chunk) in ready.drain(..) {
                world.store_chunk(pos, chunk);
                // Recorded here, on the thread that owns the world, at the moment
                // the chunk goes in. Nobody else may write to this set, so it
                // cannot come to disagree with the world about what exists.
                pool.stored(pos);
            }
        }

        match commands.recv_timeout(wait) {
            Ok(command) => apply(&mut world, command, &pool),
            Err(RecvTimeoutError::Timeout) => {}
            // Every handle is gone, so nobody is left to ask anything.
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Run however many ticks are due, bounded so that coming back from a
        // long suspend does not turn into a burst of catch-up work.
        let now = Instant::now();
        let mut due = 0;
        while now >= next_tick && due < MAX_CATCH_UP {
            tick(&mut world);
            ticks += 1;
            due += 1;
            next_tick += TICK;
        }

        // Fell too far behind to catch up: give up on the backlog rather than
        // spend the next minute paying for it.
        if due == MAX_CATCH_UP && next_tick < now {
            next_tick = now + TICK;
        }
    }

    ticks
}

/// Answers one command.
fn apply(world: &mut World, command: Command, pool: &Pool) {
    match command {
        Command::Chunk { pos, reply } => {
            let body = chunk_body(world, pos);
            pool.stored(pos);
            // A receiver that has gone is a connection that gave up waiting,
            // which is ordinary. The work was done either way.
            let _ = reply.send(body);
        }
        Command::StoredChunk { pos, reply } => {
            // Only if it is here. Generating is somebody else's job now, and so
            // is writing it out: the body was built with the chunk and is only
            // rebuilt here if the chunk has been edited since.
            let terrain = *world.terrain_blocks();
            let body = world.chunk_mut(pos).map(|c| chunk_body_of(c, &terrain, pos).to_vec());
            let _ = reply.send(body);
        }

        Command::BreakBlock { pos, reply } => {
            let broke = world.break_block(pos).is_some();
            let state = broke.then(|| state_at(world, pos)).flatten();
            let _ = reply.send(state);
        }

        Command::SetBlock { pos, block, reply } => {
            let _ = reply.send(world.set_block(pos, block).is_some());
        }

        Command::BlockAt { pos, reply } => {
            let _ = reply.send(state_at(world, pos));
        }

        Command::WorldBlock { pos, reply } => {
            let _ = reply.send(world.block(pos));
        }

        Command::TerrainBlocks { reply } => {
            let _ = reply.send(*world.terrain_blocks());
        }

        Command::Forget { pos } => {
            world.forget_chunk(pos);
            pool.dropped(pos);
        }

        Command::ChunkCount { reply } => {
            let _ = reply.send(world.chunk_count());
        }
        Command::Edits { reply } => {
            let _ = reply.send(world.edits().collect());
        }
    }
}

/// Builds a chunk and writes it as a packet body.
///
/// This is the expensive call, and it is here rather than in the connection
/// thread on purpose. It is also where the world's block ids become the game's
/// state ids, because the mapping needs the terrain palette and holding that
/// past the end of the command would mean copying it per chunk.
fn chunk_body(world: &mut World, pos: ChunkPos) -> Vec<u8> {
    world.ensure_chunk(pos);

    let terrain = *world.terrain_blocks();
    let chunk = world.chunk_mut(pos).expect("the chunk was just stored");
    chunk_body_of(chunk, &terrain, pos).to_vec()
}

/// The body of a chunk packet, written out once and kept until it is edited.
fn chunk_body_of<'a>(chunk: &'a mut Chunk, terrain: &TerrainBlocks, pos: ChunkPos) -> &'a [u8] {
    chunk.body(|c| encode_chunk_body(c, terrain, pos))
}

/// Writes a chunk out as a packet body.
///
/// Pure: a chunk and the block palette go in, bytes come out, and it needs
/// nothing else. That is what lets a worker thread outside the world do it,
/// which matters because this half is not cheap either — sixteen thousand
/// blocks and their light, every one of them looked up in a table of thirty-five
/// thousand states.
///
/// A chunk's bytes are therefore not the same thing as the chunk. Once a block
/// has been broken the stored chunk differs from the one these bytes were made
/// from, so anything wanting current bytes asks the world again rather than
/// keeping these.
fn encode_chunk_body(chunk: &Chunk, terrain: &TerrainBlocks, pos: ChunkPos) -> Vec<u8> {
    let sections: Vec<Vec<i32>> = chunk
        .sections()
        .iter()
        .map(|s| {
            section::iter_blocks(s)
                .map(|block| to_state_id(block, terrain))
                .collect()
        })
        .collect();

    let mut body = Vec::new();
    crate::protocol::chunk::write_chunk_with_states(&mut body, pos.x, pos.z, &sections);
    body
}

/// The state at a position, or `None` if the chunk holding it is not built.
fn state_at(world: &mut World, pos: BlockPos) -> Option<i32> {
    let terrain = *world.terrain_blocks();
    world.block(pos).map(|block| to_state_id(block, &terrain))
}

/// The game's name for one of the world's blocks.
///
/// The two registries are unrelated: the world's ids are fixed by registration
/// order and the game's are indices into a generated table of thirty-five
/// thousand states. Air is zero in both by coincidence, and nothing else lines
/// up, so every crossing between them goes through the name.
pub fn to_state_name(block: BlockId, terrain: &TerrainBlocks) -> &'static str {
    if block == terrain.air {
        "minecraft:air"
    } else if block == terrain.grass {
        "minecraft:grass_block"
    } else if block == terrain.dirt {
        "minecraft:dirt"
    } else if block == terrain.stone {
        "minecraft:stone"
    } else if block == terrain.glowstone {
        "minecraft:glowstone"
    } else {
        panic!("the terrain generator produced a block this cannot name")
    }
}

/// Maps one of the world's block ids onto the game's state id.
pub fn to_state_id(block: BlockId, terrain: &TerrainBlocks) -> i32 {
    state_id(to_state_name(block, terrain))
}

/// How long a block takes to break, from the generated table.
///
/// `None` when the table has nothing for it, which is the honest answer: a
/// made-up number paces the mining crack against a time the client does not
/// share, and the crack loses its last few stages.
pub fn hardness_of(block: BlockId, terrain: &TerrainBlocks) -> Option<f32> {
    crate::generated::hardness_of(to_state_name(block, terrain))
}

/// The game's name for a state id, for the log.
///
/// The reverse of [`state_id`], and needed because a state id on its own is not
/// readable by eye: a log line saying a block became state 17 says nothing
/// about whether 17 is air, and the whole question when a block stays solid is
/// whether the id that went out was the one meant for air.
pub fn state_id_name(state: i32) -> &'static str {
    // The generated table is a flat list of blocks, each owning a range of
    // state ids, so the name is the block whose range covers this id. Unknown
    // ids are reported rather than hidden: they would be a bug, and a log that
    // quietly said "air" for one would hide it.
    if state < 0 {
        return "<negative>";
    }
    for block in crate::generated::BLOCKS {
        if block.states.iter().any(|s| s.id as i32 == state) {
            return block.name;
        }
    }
    "<unknown>"
}

/// Reads a block's state id out of the generated table.
///
/// Looked up rather than written out, because these are indices into thirty-five
/// thousand block states and a hand-written number is a number that can be wrong
/// without anything noticing.
pub fn state_id(name: &str) -> i32 {
    crate::generated::BLOCKS
        .iter()
        .find(|b| b.name == name)
        .map(|b| b.default_state as i32)
        .unwrap_or_else(|| panic!("{name} is in the generated block table"))
}

/// What a player gets for a block, if it is something that can be held.
///
/// The name from [`state_id_name`] is looked up as it stands. It already
/// carries its namespace, and adding `minecraft:` to it again asks for
/// `minecraft:minecraft:grass_block` -- not an item, and not an error either,
/// just a lookup that quietly finds nothing while the player digs a block and
/// is handed silence. This has happened, and [`tests`] pins it.
pub fn held_item(state: i32) -> Option<u32> {
    let name = state_id_name(state);
    // `minecraft:air` is a real item and the registry gives it an id, but a
    // player never digs up air: breaking a block leaves air where the block
    // was, and that is what the block became rather than what was picked up.
    // Without this the block that stays after the dig would be handed over as
    // though it had come out of the ground.
    if name == "minecraft:air" {
        return None;
    }
    crate::generated::item_id(name)
}

#[cfg(test)]
mod tests;
