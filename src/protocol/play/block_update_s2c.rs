//! One block changed.

use super::id;
use super::pack_block_pos;
use crate::protocol::VarInt;

/// Writes a `block_update` for one block.
///
/// The client learns a block changed from this packet alone: a position and the
/// new state at it. Nothing else tells it to rebuild the mesh at that spot, so
/// without it a break the server applied stays invisible.
pub fn write_block_update(out: &mut Vec<u8>, x: i32, y: i32, z: i32, state: i32) {
    VarInt(id::BLOCK_UPDATE).write(out);
    // Eight bytes, always. A block position is a fixed long and not a VarLong,
    // so the value is written out in full whatever its size: a VarLong would be
    // one byte here and the client would read the rest of the packet as the
    // position.
    out.extend_from_slice(&pack_block_pos(x, y, z).to_be_bytes());
    VarInt(state).write(out);
}
