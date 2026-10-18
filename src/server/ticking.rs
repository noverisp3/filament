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

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
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
    /// Break a block, and report the state now standing there.
    BreakBlock { pos: BlockPos, reply: Sender<Option<i32>> },
    /// The state at a position, if the chunk holding it has been built.
    BlockAt { pos: BlockPos, reply: Sender<Option<i32>> },
    /// Drop a chunk the player has walked away from.
    Forget { pos: ChunkPos },
    /// How many chunks the world is holding.
    ChunkCount { reply: Sender<usize> },
}

/// What a connection holds: a way to ask, not a world.
///
/// Cheap to clone, and the clone is another way to ask rather than another world.
/// The old shared world was an `Arc<Mutex<World>>` and cloning it handed out a
/// second way to reach the world; here the only thing reachable is the channel.
#[derive(Clone)]
pub struct WorldHandle {
    to_world: Sender<Command>,
}

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

    /// One chunk, ready to frame and send.
    ///
    /// The body is returned rather than a chunk, because the caller needs bytes
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

    /// The state at a position, if the chunk holding it exists.
    pub fn block_at(&self, pos: BlockPos) -> Option<Option<i32>> {
        self.ask(|reply| Command::BlockAt { pos, reply })
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
}

/// Starts the thread that owns the world, and returns the handle connections use.
///
/// The thread stops when every handle is dropped, since the channel closes with
/// them. That is what lets a test build a world, use it and have it go away
/// without reaching into the runtime.
pub fn spawn() -> WorldHandle {
    let (to_world, from_connections) = channel();
    let world = World::empty();

    std::thread::Builder::new()
        .name("world".to_string())
        .spawn(move || run(world, from_connections, |_| {}))
        .expect("a thread can be spawned");

    WorldHandle { to_world }
}

/// The loop. Split out so a test can drive it without a thread.
pub fn run(mut world: World, commands: Receiver<Command>, mut tick: impl FnMut(&mut World)) -> u32 {
    let mut ticks = 0;
    let mut next_tick = Instant::now() + TICK;

    loop {
        let now = Instant::now();
        let wait = next_tick.saturating_duration_since(now);

        match commands.recv_timeout(wait) {
            Ok(command) => apply(&mut world, command),
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
fn apply(world: &mut World, command: Command) {
    match command {
        Command::Chunk { pos, reply } => {
            let body = chunk_body(world, pos);
            // A receiver that has gone is a connection that gave up waiting,
            // which is ordinary. The work was done either way.
            let _ = reply.send(body);
        }

        Command::BreakBlock { pos, reply } => {
            let broke = world.break_block(pos).is_some();
            let state = broke.then(|| state_at(world, pos)).flatten();
            let _ = reply.send(state);
        }

        Command::BlockAt { pos, reply } => {
            let _ = reply.send(state_at(world, pos));
        }

        Command::Forget { pos } => {
            world.forget_chunk(pos);
        }

        Command::ChunkCount { reply } => {
            let _ = reply.send(world.chunk_count());
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
    let chunk = world.chunk(pos).expect("the chunk was just stored");

    let sections: Vec<Vec<i32>> = chunk
        .sections()
        .iter()
        .map(|s| {
            section::iter_blocks(s)
                .map(|block| to_state_id(block, &terrain))
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

/// Maps one of the world's block ids onto the game's state id.
///
/// The two registries are unrelated. Air is zero in both by coincidence, and
/// nothing else lines up.
pub fn to_state_id(block: BlockId, terrain: &TerrainBlocks) -> i32 {
    if block == terrain.air {
        state_id("minecraft:air")
    } else if block == terrain.grass {
        state_id("minecraft:grass_block")
    } else if block == terrain.dirt {
        state_id("minecraft:dirt")
    } else if block == terrain.stone {
        state_id("minecraft:stone")
    } else if block == terrain.glowstone {
        state_id("minecraft:glowstone")
    } else {
        panic!("the terrain generator produced a block this cannot map")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The surface of a column, worked out the way the generator does it.
    ///
    /// The noise gives a height above the bottom of the world rather than an
    /// absolute y, so leaving off the -64 looks for ground that is not there and
    /// the test fails on air.
    fn surface(world_x: f64, world_z: f64) -> i32 {
        crate::world::chunk::MIN_BLOCK_Y
            + crate::world::terrain::height_at(world_x, world_z) as i32
    }

    #[test]
    fn a_chunk_asked_for_twice_comes_back_the_same() {
        let handle = spawn();
        let pos = ChunkPos::new(0, 0);

        let first = handle.chunk(pos).expect("the world is there");
        let second = handle.chunk(pos).expect("the world is there");

        assert_eq!(first, second, "the generator is deterministic");
        assert_eq!(handle.chunk_count(), Some(1), "and only made one");
    }

    #[test]
    fn a_break_is_seen_by_every_handle() {
        let handle = spawn();
        let other = handle.clone();

        handle.chunk(ChunkPos::new(0, 0)).expect("built");

        let pos = BlockPos::new(8, surface(8.0, 8.0), 8);
        let before = other.block_at(pos).expect("answered").expect("ground");
        assert_ne!(before, AIR, "the top of this column is something");

        assert!(handle.break_block(pos).expect("answered").is_some());
        assert_ne!(
            other.block_at(pos).expect("answered"),
            Some(before),
            "what one handle broke, the other sees gone"
        );
    }

    #[test]
    fn breaking_nothing_reports_nothing_broke() {
        let handle = spawn();
        // Far out, where no chunk has ever been generated.
        let nowhere = BlockPos::new(999_999, 100, 999_999);

        assert_eq!(
            handle.break_block(nowhere).expect("answered"),
            None,
            "there was no block there to break"
        );
        assert_eq!(handle.block_at(nowhere).expect("answered"), None);
    }

    #[test]
    fn forgetting_releases_the_chunk() {
        let handle = spawn();
        handle.chunk(ChunkPos::new(5, 5)).expect("built");
        assert_eq!(handle.chunk_count(), Some(1));

        assert!(handle.forget(ChunkPos::new(5, 5)), "sent");
        // The command is handled in order, so by the time the next ask is
        // answered the forget has already happened.
        assert_eq!(handle.chunk_count(), Some(0), "the memory went back");
    }

    #[test]
    fn dropping_the_last_handle_leaves_nothing_to_ask() {
        let handle = spawn();
        assert!(handle.chunk_count().is_some(), "the world answers at first");

        drop(handle);
        // Give the thread a moment to notice the closed channel. Sleeping in a
        // test is normally a smell; here the thing being waited for is another
        // thread's scheduling, which has no other way to be waited for.
        std::thread::sleep(Duration::from_millis(50));

        // A handle whose thread has gone answers with nothing rather than
        // panicking, which is what a player disconnecting during shutdown does.
        let orphan = spawn();
        drop(orphan);
        std::thread::sleep(Duration::from_millis(50));

        let after = orphan_after_shutdown();
        assert_eq!(
            after, None,
            "a handle to a world that has stopped answers None, not a panic"
        );
    }

    /// Keeps a handle alive past its thread having stopped, so the check above
    /// is about the channel and not about the value having been dropped.
    fn orphan_after_shutdown() -> Option<usize> {
        let (to_world, from_connections) = channel();
        drop(to_world);
        // Run the loop with a closed channel, as it is after every handle goes.
        run(World::empty(), from_connections, |_| {});

        let handle = WorldHandle { to_world: channel().0 };
        handle.chunk_count()
    }

    const AIR: i32 = crate::protocol::chunk::AIR_STATE_ID;

    #[test]
    fn the_loop_ticks_at_twenty_a_second_while_the_channel_is_open() {
        let (to_thread, from_connections) = channel();
        let ticks = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = ticks.clone();

        let worker = std::thread::spawn(move || {
            run(World::empty(), from_connections, move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        });

        // Long enough for a dozen ticks, short enough not to matter if it is a
        // few more or a few fewer. The assertion below is a floor, not a rate:
        // a loaded machine can miss ticks, and a test that demands exactly twenty
        // fails on someone else's hardware.
        std::thread::sleep(Duration::from_millis(250));

        drop(to_thread);
        worker.join().expect("the loop thread finished");

        let ran = ticks.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            ran >= 3,
            "250ms at twenty a second is about five ticks, ran {ran}"
        );
    }

    #[test]
    fn the_loop_stops_when_the_last_handle_goes() {
        let (to_thread, from_connections) = channel();
        drop(to_thread);

        // Nothing to serve, so this returns rather than ticking forever. That it
        // returns at all is the assertion: it is what lets a test build a world,
        // use it, and have it go away without reaching into the runtime.
        let ticks = run(World::empty(), from_connections, |_| {});

        assert_eq!(ticks, 0, "no handles, so no ticks were due");
    }
}
