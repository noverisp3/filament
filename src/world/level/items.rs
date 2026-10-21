//! The items lying on the ground, waiting to be picked up.
//!
//! A broken block becomes one of these instead of going straight into a
//! player's pocket: the game drops it, and the player walks over it to pick it
//! up.

use crate::world::item_entity::ItemEntity;
use crate::world::pos::BlockPos;

use super::World;

impl World {
    /// The items lying on the ground.
    pub fn items(&self) -> &[ItemEntity] {
        &self.items
    }

    /// Drops an item into the world, and says where it is.
    ///
    /// The item starts in the middle of the block and falls, the way the
    /// game's does: a thing on the ground is not in mid-air, it is on the
    /// ground, and it gets there by falling.
    pub fn drop_item(&mut self, id: i32, uuid: [u8; 16], pos: BlockPos, item: u32, count: i32) {
        let entity = ItemEntity::new(id, uuid, pos, item, count);
        self.items.push(entity.clone());
        self.item_changes.push(entity);
    }

    /// The items that are near a point, and can therefore be picked up.
    pub fn items_near(&self, x: f64, y: f64, z: f64) -> Vec<ItemEntity> {
        self.items
            .iter()
            .filter(|item| item.within_reach(x, y, z))
            .cloned()
            .collect()
    }

    /// The items within a radius of a point, which are the ones a player there
    /// can see.
    pub fn items_within(&self, x: f64, y: f64, z: f64, radius: f64) -> Vec<ItemEntity> {
        let square = radius * radius;
        self.items
            .iter()
            .filter(|item| {
                let dx = item.x - x;
                let dy = item.y - y;
                let dz = item.z - z;
                dx * dx + dy * dy + dz * dz <= square
            })
            .cloned()
            .collect()
    }

    /// Takes an item off the ground, and says it is gone.
    pub fn take_item(&mut self, id: i32) -> Option<ItemEntity> {
        let index = self.items.iter().position(|item| item.id == id)?;
        Some(self.items.remove(index))
    }

    /// The items that moved, waiting to be told about.
    pub fn take_item_changes(&mut self) -> Vec<ItemEntity> {
        std::mem::take(&mut self.item_changes)
    }
}
