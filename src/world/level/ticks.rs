//! The world's clock: what is due, and what the rules do when it is.
//!
//! The order is the game's (`ServerLevel.tick`): work that is due is taken
//! under a budget, and what does not fit waits for the next tick.

use crate::world::block::BlockId;
use crate::world::pos::{BlockPos, Direction};

use super::BlockEdit;
use super::World;

impl World {
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
            if item.age >= crate::world::item_entity::LIFETIME {
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
    pub(super) fn random_update(&mut self, pos: BlockPos, edit: &mut BlockEdit) {
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
}
