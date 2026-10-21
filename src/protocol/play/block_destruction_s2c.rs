//! How far through breaking a block the client should draw the crack.

use super::id;
use super::pack_block_pos;
use crate::protocol::VarInt;

/// Writes a `block_destruction`: how far through breaking a block is.
///
/// Stages are zero to nine, and minus one means there is no longer a crack on
/// that block. The minus one is written as a byte, so it is 255 on the wire and
/// comes back through an unsigned read as 255 too; it is signed on the way out.
///
/// The id is the entity that is doing the breaking. The client does not act on
/// it, but the game uses it to leave the breaker out of its own broadcast.
pub fn write_block_destruction(out: &mut Vec<u8>, breaker: i32, pos: (i32, i32, i32), stage: i8) {
    VarInt(id::BLOCK_DESTRUCTION).write(out);
    VarInt(breaker).write(out);
    out.extend_from_slice(&pack_block_pos(pos.0, pos.1, pos.2).to_be_bytes());
    out.push(stage as u8);
}
