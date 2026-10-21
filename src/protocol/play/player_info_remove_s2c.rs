//! Taking players out of the tab list.

use super::id;
use crate::protocol::VarInt;

/// Writes a `player_info_remove`, taking players out of the tab list.
///
/// Sent alongside `remove_entities` and for the same reason: an entry left in
/// the tab list for a player who is gone is a client that keeps a name for
/// somebody who has left.
///
/// By uuid, and not by entity id: the list is keyed by profile, and the entity
/// id is only ever something this side remembers.
pub fn write_player_info_remove(out: &mut Vec<u8>, uuids: &[[u8; 16]]) {
    VarInt(id::PLAYER_INFO_REMOVE).write(out);
    VarInt(uuids.len() as i32).write(out);
    for uuid in uuids {
        out.extend_from_slice(uuid);
    }
}
