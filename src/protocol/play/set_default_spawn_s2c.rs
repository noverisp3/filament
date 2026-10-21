//! Where the player comes back to.

use super::id;
use super::pack_block_pos;
use crate::protocol::VarInt;

/// Writes `ClientboundSetDefaultSpawnPositionPacket`.
///
/// A dimension, a block position, then a yaw and a pitch. The position is not
/// three doubles: it is a block position, which is packed into a single eight
/// byte value, and writing coordinates as doubles here produces a packet nine
/// bytes too long.
pub fn write_set_default_spawn(out: &mut Vec<u8>, dimension: &str, x: i32, y: i32, z: i32) {
    VarInt(id::SET_DEFAULT_SPAWN).write(out);
    VarInt(dimension.len() as i32).write(out);
    out.extend_from_slice(dimension.as_bytes());
    out.extend_from_slice(&pack_block_pos(x, y, z).to_be_bytes());
    out.extend_from_slice(&0.0f32.to_be_bytes());
    out.extend_from_slice(&0.0f32.to_be_bytes());
}
