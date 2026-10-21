//! How far into the level the player is.

use super::id;
use crate::protocol::VarInt;

/// Writes the experience: how far into the level, the level, and the total.
pub fn write_set_experience(out: &mut Vec<u8>, progress: f32, level: i32, total: i32) {
    VarInt(id::SET_EXPERIENCE).write(out);
    out.extend_from_slice(&progress.to_be_bytes());
    VarInt(level).write(out);
    VarInt(total).write(out);
}
