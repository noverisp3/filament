//! How far the world goes, in chunks.

use super::id;
use crate::protocol::VarInt;

/// Writes `set_chunk_cache_radius`, telling the client how far the world goes.
///
/// Without it the client falls back to a radius of its own that has nothing to
/// do with what this server sends, and then keeps chunks that were never
/// streamed and drops ones that were.
pub fn write_chunk_cache_radius(out: &mut Vec<u8>, radius: i32) {
    VarInt(id::SET_CHUNK_CACHE_RADIUS).write(out);
    VarInt(radius).write(out);
}
