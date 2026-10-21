//! The item went to the player.

use super::id;
use crate::protocol::VarInt;

/// The packet id of `take_item_entity`.
pub const TAKE_ITEM_ENTITY: i32 = id::TAKE_ITEM_ENTITY;

/// Writes a `take_item_entity`: the item went to the player.
///
/// ```text
/// VarInt   the item's entity id
/// VarInt   the player who took it
/// VarInt   how many
/// ```
///
/// The client plays the sound of a pickup and counts the stack, and the count
/// is how many the player got rather than how many there were.
pub fn write_take_item_entity(out: &mut Vec<u8>, item_id: i32, player_id: i32, amount: i32) {
    VarInt(TAKE_ITEM_ENTITY).write(out);
    VarInt(item_id).write(out);
    VarInt(player_id).write(out);
    VarInt(amount).write(out);
}
