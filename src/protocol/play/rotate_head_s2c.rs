//! Which way somebody is looking.

use super::angle_byte;
use super::id;
use crate::protocol::VarInt;

/// Writes a `rotate_head`, so a client knows which way somebody is looking.
pub fn write_rotate_head(out: &mut Vec<u8>, entity_id: i32, yaw: f32) {
    VarInt(id::ROTATE_HEAD).write(out);
    VarInt(entity_id).write(out);
    out.push(angle_byte(yaw));
}
