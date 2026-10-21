//! A chunk the client should let go of.

use super::id;
use crate::protocol::VarInt;

/// Writes a `forget_level_chunk`, telling the client to drop a chunk.
///
/// Without this the client's chunk cache only ever grows, and the server's
/// memory goes with it.
///
/// The coordinate is one long, not two VarInts: this is a `ChunkPos`, which
/// packs into a fixed eight bytes with x in the low half and z in the high
/// half. Writing VarInts here makes a packet that decodes as far as the id.
pub fn write_forget_chunk(out: &mut Vec<u8>, chunk_x: i32, chunk_z: i32) {
    VarInt(id::FORGET_LEVEL_CHUNK).write(out);
    let packed = (chunk_x as i64 & 0xFFFF_FFFF) | ((chunk_z as i64 & 0xFFFF_FFFF) << 32);
    out.extend_from_slice(&packed.to_be_bytes());
}
