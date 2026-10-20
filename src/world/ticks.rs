//! Work the world owes itself later, ordered by when.
//!
//! Everything the game does to a block on a delay goes through here: dirt
//! turning back to grass, water looking for somewhere to go, a crop deciding
//! it has grown. None of it is answered where the event happened: an event
//! that changed the world has only just started, and answering it inline is
//! how one change becomes forty.
//!
//! The shape is the game's (`LevelTicks.tick`): work is **collected** and then
//! **run**, each tick runs only as much as its **budget**, and a tick that has
//! gone is **cleaned up** even when it threw.
//!
//! This is a heap rather than the game's bucket-per-chunk: at the scale of a
//! world with one player in it, that is the same behaviour for less machinery.

use std::collections::BinaryHeap;

use crate::world::pos::BlockPos;

/// How much work one tick will do before leaving the rest for the next.
///
/// The game uses 65536 (`ServerLevel.java:394`), sized for a world with
/// thousands of players. A quarter of it is still far more than fits in fifty
/// milliseconds here, and a budget that can never be reached is not a budget.
pub const TICK_BUDGET: usize = 16_384;

/// A tick, the position it is for, and the order it was asked for in.
///
/// Ordered by tick descending, because [`BinaryHeap`] is a max-heap and the tick
/// we want next is the smallest. The counter breaks ties so that two things due
/// at the same moment run in the order they were scheduled.
///
/// The flag says whether this is one of the game's **random** ticks: the ones
/// the game rolls for the blocks that change on their own. Those are not
/// answered on the change that touched the block but now and then.
#[derive(Debug, PartialEq, Eq, Clone)]
struct Due {
    tick: u64,
    order: u64,
    pos: BlockPos,
    random: bool,
}

impl Ord for Due {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reversed: the heap's largest must be the soonest.
        other
            .tick
            .cmp(&self.tick)
            .then_with(|| other.order.cmp(&self.order))
    }
}

impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Everything the world has promised to come back to.
#[derive(Debug, Default, Clone)]
pub struct BlockTicks {
    due: BinaryHeap<Due>,
    order: u64,
}

impl BlockTicks {
    pub fn new() -> Self {
        BlockTicks::default()
    }

    /// How many are waiting, due or not.
    pub fn len(&self) -> usize {
        self.due.len()
    }

    pub fn is_empty(&self) -> bool {
        self.due.is_empty()
    }

    /// Promises to come back to this position in `delay` ticks.
    ///
    /// A repeated schedule for the same position is a second promise, not a
    /// replacement: the game allows several ticks on one block and runs them in
    /// the order they were asked for.
    pub fn schedule(&mut self, now: u64, delay: u64, pos: BlockPos) {
        self.push(now, delay, pos, false);
    }

    /// Promises to come back to this position in `delay` ticks, as one of the
    /// game's random ticks.
    ///
    /// The same promise, but flagged as the game's own randomness: the rules
    /// that grow grass and grow crops are answered by these and by nothing
    /// else, so they happen now and then rather than the moment a block
    /// changes.
    pub fn schedule_random(&mut self, now: u64, delay: u64, pos: BlockPos) {
        self.push(now, delay, pos, true);
    }

    fn push(&mut self, now: u64, delay: u64, pos: BlockPos, random: bool) {
        self.order += 1;
        self.due.push(Due {
            tick: now.saturating_add(delay),
            order: self.order,
            pos,
            random,
        });
    }

    /// The earliest tick anything is waiting for.
    pub fn earliest(&self) -> Option<u64> {
        self.due.peek().map(|d| d.tick)
    }

    /// Takes what is due, and only as much as the budget allows.
    ///
    /// What does not fit stays in the heap and is picked up next tick, which is
    /// the whole point of the budget: a spike of work is spread rather than
    /// dropped and rather than paid for all at once.
    pub fn take_due(&mut self, now: u64, budget: usize) -> Vec<(BlockPos, bool)> {
        let mut out = Vec::new();
        while out.len() < budget
            && let Some(next) = self.due.peek()
            && next.tick <= now
        {
            // `peek` has just said this is there, so it is there.
            if let Some(due) = self.due.pop() {
                out.push((due.pos, due.random));
            }
        }
        out
    }

    /// Forgets every promise for a chunk that has gone.
    ///
    /// Without this a chunk that is unloaded and reloaded keeps its backlog and
    /// runs the whole of it at once, on a world that has since been rebuilt.
    pub fn forget_chunk(&mut self, in_chunk: impl Fn(BlockPos) -> bool) -> usize {
        let before = self.due.len();
        let kept: BinaryHeap<Due> = self
            .due
            .iter()
            .filter(|d| !in_chunk(d.pos))
            .map(|d| Due {
                tick: d.tick,
                order: d.order,
                pos: d.pos,
                random: d.random,
            })
            .collect();
        self.due = kept;
        before - self.due.len()
    }
}

#[cfg(test)]
mod tests;
