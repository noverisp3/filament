//! The middle of the world the client is looking at.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundSetChunkCacheCenterPacket`.
///
/// Two VarInts, each a chunk coordinate. The client will only ask for chunks
/// around this, so until it is sent there is nowhere for chunks to go.
pub fn write_chunk_cache_center(out: &mut Vec<u8>, chunk_x: i32, chunk_z: i32) {
    VarInt(id::SET_CHUNK_CACHE_CENTER).write(out);
    VarInt(chunk_x).write(out);
    VarInt(chunk_z).write(out);
}
