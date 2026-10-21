//! Taking them out of the world.

use super::id;
use crate::protocol::VarInt;

/// Writes a `remove_entities`, taking them out of the world.
pub fn write_remove_entities(out: &mut Vec<u8>, entity_ids: &[i32]) {
    VarInt(id::REMOVE_ENTITIES).write(out);
    VarInt(entity_ids.len() as i32).write(out);
    for id in entity_ids {
        VarInt(*id).write(out);
    }
}
