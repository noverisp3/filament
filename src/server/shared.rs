//! The world, shared by every connection.
//!
//! # Why this exists
//!
//! The world used to be created inside the play phase, which runs once per
//! connection. That made the server correct for exactly one player: a second
//! connection got a second world, generated separately, so two people on the
//! same server stood in different places, dug in different ground, and neither
//! saw the other's work. Not a missing feature so much as a bug that only stays
//! hidden while nobody else connects.
//!
//! So the world is built once and handed to every connection.
//!
//! # Why a wrapper rather than `Arc<Mutex<World>>` passed around
//!
//! Because every access has to take the lock, and a call site that forgets is a
//! data race that reproduces once in a thousand connections rather than once in
//! a thousand runs. This type has no way to reach the `World` without going
//! through one of these methods, so the lock is taken in one file.
//!
//! It also keeps open the door to the other design. A tick thread that owns the
//! world outright and takes commands from connections would be a different
//! answer to the same question, and every method here is a command that such a
//! queue would carry. Moving to it is then a change of what the body of each
//! method does, not a change to every call site.
//!
//! # What is not handled here
//!
//! The lock is held while a chunk is read out for sending, which serialises
//! players against each other for as long as that takes. For one chunk it is a
//! few milliseconds; for the two hundred and eighty-nine of an initial join it
//! is a second, during which nobody else generates anything.
//!
//! That is the cost of doing this first rather than doing the tick loop first,
//! and it is stated here rather than discovered later: the fix is a per-chunk
//! lock or a read-only snapshot handed to the connection, and both of those
//! want the tick loop to already exist.

use std::sync::{Arc, Mutex};

use crate::world::block::BlockId;
use crate::world::pos::{BlockPos, ChunkPos};
use crate::world::section;
use crate::world::terrain::TerrainBlocks;
use crate::world::world::World;

/// A world that several connections may be looking at at once.
///
/// Cheap to clone: it is a handle, not a copy of the world.
#[derive(Clone)]
pub struct SharedWorld(Arc<Mutex<World>>);

impl SharedWorld {
    /// An empty world that generates chunks as they are asked for.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(World::empty())))
    }

    /// How many chunks are held right now.
    ///
    /// For the log, and for a test that wants to see a chunk actually be let
    /// go rather than trusting that it was.
    pub fn chunk_count(&self) -> usize {
        self.lock().chunk_count()
    }

    /// The terrain generator's block ids, for mapping onto game state ids.
    ///
    /// A copy rather than a borrow, because the borrow would have to outlive the
    /// lock and the point of the wrapper is that callers cannot hold one.
    pub fn terrain_blocks(&self) -> TerrainBlocks {
        *self.lock().terrain_blocks()
    }

    /// Generates and stores a chunk if it is missing, then reads it back out.
    ///
    /// Returns one `Vec<i32>` of game state ids per section, which is what the
    /// chunk packet wants and what the world is not.
    ///
    /// The mapping to state ids happens under the lock because the alternative
    /// is handing out the world's own blocks and mapping them later, and then
    /// the mapping constant would have to be threaded through every caller.
    pub fn sections_for_wire(&self, pos: ChunkPos) -> Vec<Vec<i32>> {
        let mut world = self.lock();
        world.ensure_chunk(pos);

        let terrain = *world.terrain_blocks();
        let chunk = world.chunk(pos).expect("the chunk was just stored");

        chunk
            .sections()
            .iter()
            .map(|s| {
                section::iter_blocks(s)
                    .map(|block| to_state_id(block, &terrain))
                    .collect()
            })
            .collect()
    }

    /// The block at a position, as a game state id.
    ///
    /// `None` if no chunk has been generated there yet, which is not an error:
    /// a player can reach out over ground the server has never built.
    pub fn state_at(&self, pos: BlockPos) -> Option<i32> {
        let world = self.lock();
        let terrain = *world.terrain_blocks();
        world.block(pos).map(|block| to_state_id(block, &terrain))
    }

    /// Breaks a block, returning what changed.
    ///
    /// `None` if there was nothing there to break, which covers both air and a
    /// chunk the server has not generated.
    pub fn break_block(&self, pos: BlockPos) -> Option<()> {
        self.lock().break_block(pos).map(|_| ())
    }

    /// Drops a chunk the player has walked away from.
    pub fn forget(&self, pos: ChunkPos) -> bool {
        self.lock().forget_chunk(pos)
    }

    /// Takes the lock.
    ///
    /// Private on purpose. Every method above goes through it, so the set of
    /// places that can block a connection is exactly the set of places that
    /// touch the world.
    fn lock(&self) -> std::sync::MutexGuard<'_, World> {
        // A panic while holding the lock would poison it and take every other
        // connection down with it. The world is plain data behind these methods
        // and is not left half-edited by unwinding, so continuing is the right
        // answer here: one bad request should not end the server.
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for SharedWorld {
    fn default() -> Self {
        Self::new()
    }
}

/// The game state id for air.
///
/// Used when there is no block to read: a hole in the world, or a chunk the
/// server has not built. Also the test's way of asking "is this a hole".
pub const AIR: i32 = crate::protocol::chunk::AIR_STATE_ID;

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

    #[test]
    fn a_chunk_read_for_the_wire_comes_back_the_same_every_time() {
        let world = SharedWorld::new();
        let pos = ChunkPos::new(0, 0);

        let first = world.sections_for_wire(pos);
        let second = world.sections_for_wire(pos);

        assert_eq!(first, second, "the generator is deterministic");
        assert_eq!(world.chunk_count(), 1, "reading it twice made one chunk");
    }

    #[test]
    fn two_handles_see_the_same_world() {
        let world = SharedWorld::new();
        let other = world.clone();

        world.sections_for_wire(ChunkPos::new(3, -2));

        assert_eq!(
            other.chunk_count(),
            1,
            "a clone is a second handle, not a second world"
        );
        assert!(
            other.state_at(BlockPos::new(48, 64, -32)).is_some(),
            "the chunk the first handle built is visible through the second"
        );
    }

    #[test]
    fn a_break_is_visible_through_every_handle() {
        let world = SharedWorld::new();
        let other = world.clone();

        world.sections_for_wire(ChunkPos::new(0, 0));

        // The generator's own surface for this column, worked out the way
        // `generate_chunk` does it: the noise gives a height above the bottom of
        // the world, not an absolute y. Leaving out the -64 looks for ground
        // thirty blocks under the world and finds air.
        let world_x = 8.0;
        let world_z = 8.0;
        let surface = crate::world::chunk::MIN_BLOCK_Y
            + crate::world::terrain::height_at(world_x, world_z) as i32;
        let pos = BlockPos::new(8, surface, 8);

        let before = world.state_at(pos).expect("ground is there");
        assert_ne!(before, AIR, "the surface of this column is something");

        assert!(world.break_block(pos).is_some());
        assert_ne!(
            other.state_at(pos),
            Some(before),
            "what one handle broke, the other sees gone"
        );
    }

    #[test]
    fn forgetting_a_chunk_releases_it() {
        let world = SharedWorld::new();
        world.sections_for_wire(ChunkPos::new(5, 5));
        assert_eq!(world.chunk_count(), 1);

        assert!(world.forget(ChunkPos::new(5, 5)));
        assert_eq!(world.chunk_count(), 0, "the memory went back");

        // The terrain is deterministic, so walking back gives the same ground.
        // Anything the player did in it does not, and this is where that starts.
        assert_eq!(
            world.sections_for_wire(ChunkPos::new(5, 5)).len(),
            crate::protocol::chunk::SECTIONS,
            "walking back rebuilds it"
        );

        // Forgetting something that was never there is not a thing that happened.
        assert!(
            !world.forget(ChunkPos::new(900, 900)),
            "a chunk that was never built"
        );
    }
}
