//! The red flash when a player is hurt.

use super::id;
use crate::protocol::VarInt;

/// Writes the hurt animation.
pub fn write_hurt_animation(out: &mut Vec<u8>, entity_id: i32, yaw: f32) {
    VarInt(id::HURT_ANIMATION).write(out);
    VarInt(entity_id).write(out);
    // Which way the hit came from, in degrees. Zero is straight ahead, which
    // is what a fall from above looks like.
    out.extend_from_slice(&yaw.to_be_bytes());
}
