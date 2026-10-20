//! An item lying in the world, waiting to be picked up.
//!
//! The game drops what a broken block was into the world as an entity, and the
//! player picks it up by walking over it. The item is not in anybody's hands
//! until then, which is the whole of the difference: a block that vanishes
//! when it is broken and appears in the pocket straight away skips the half of
//! the game where a thing on the ground can be lost, dropped, or fought over.

use super::pos::BlockPos;

/// How long an item lies about before it goes away, in ticks.
///
/// The game's number: five minutes. An item that nobody picks up is not there
/// for ever, and the world does not fill up with the rubble of a week.
pub const LIFETIME: u32 = 6000;

/// How far a player reaches for an item, in blocks.
///
/// A player picks up what they are standing on, and a little of what is around
/// them. The game's own reach is a box a little over a block wide.
pub const PICKUP_RADIUS: f64 = 1.5;

/// A name for an entity, sixteen bytes.
///
/// The client uses it to tell this entity from another one that has the same
/// number after a restart, so it has to be different every time. The clock and
/// a counter are enough for that: two items dropped in the same nanosecond
/// would still be told apart by the counter.
pub fn new_uuid(counter: u64) -> [u8; 16] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&nanos.to_be_bytes());
    bytes[8..].copy_from_slice(&counter.to_be_bytes());
    // Version four and the variant the game uses, so the bytes look like a
    // uuid rather than like a number.
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    bytes
}

/// An item on the ground.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemEntity {
    /// The entity's number, which is how the client knows which one this is.
    pub id: i32,
    /// The entity's name, sixteen bytes, which the client uses to tell this
    /// item from another one that has the same number after a restart.
    pub uuid: [u8; 16],
    /// Where it is.
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Which item it is, by the id the item registry uses.
    pub item: u32,
    /// How many.
    pub count: i32,
    /// How long it has been lying there, in ticks.
    pub age: u32,
    /// Whether it is still falling.
    pub falling: bool,
}

impl ItemEntity {
    /// A new item on the ground, in the middle of a block.
    pub fn new(id: i32, uuid: [u8; 16], pos: BlockPos, item: u32, count: i32) -> Self {
        Self {
            id,
            uuid,
            x: pos.x as f64 + 0.5,
            y: pos.y as f64,
            z: pos.z as f64 + 0.5,
            item,
            count,
            age: 0,
            falling: true,
        }
    }

    /// How far a point is from this item.
    pub fn distance_to(&self, x: f64, y: f64, z: f64) -> f64 {
        let dx = self.x - x;
        let dy = self.y - y;
        let dz = self.z - z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// Whether a player standing at a point is close enough to pick it up.
    pub fn within_reach(&self, x: f64, y: f64, z: f64) -> bool {
        self.distance_to(x, y, z) <= PICKUP_RADIUS
    }
}
