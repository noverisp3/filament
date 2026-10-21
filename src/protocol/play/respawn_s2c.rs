//! Putting a dead player back in the world.

use super::id;
use super::spawn_info::{write_spawn_info, SpawnInfo};
use crate::protocol::VarInt;

/// Writes the packet that puts a dead player back in the world.
///
/// The tail is the same spawn information the login packet carries, and the
/// byte after it says what the client should keep: the game keeps the
/// attributes and the entity data, which is all of it.
pub fn write_respawn(out: &mut Vec<u8>, info: &SpawnInfo<'_>, keep_all_data: bool) {
    VarInt(id::RESPAWN).write(out);
    write_spawn_info(out, info);
    out.push(if keep_all_data { 3 } else { 0 });
}
