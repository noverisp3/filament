//! The news that a player died.

use super::id;
use crate::protocol::VarInt;

/// Writes the news that a player died.
///
/// The client does not work out on its own that a health of zero means
/// death: it is told, and this is the telling. The message is the line the
/// chat shows, and the entity id is the player it happened to.
pub fn write_player_combat_kill(out: &mut Vec<u8>, player_id: i32, message: &str) {
    VarInt(id::PLAYER_COMBAT_KILL).write(out);
    VarInt(player_id).write(out);
    // A component, which in a packet is one NBT tag: a plain text one is a
    // string tag.
    crate::world::nbt::write_string_tag(out, message);
}
