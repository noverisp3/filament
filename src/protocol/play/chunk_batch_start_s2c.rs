//! The start of a batch of chunks.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundChunkBatchStartPacket`, empty upstream.
pub fn write_chunk_batch_start(out: &mut Vec<u8>) {
    VarInt(id::CHUNK_BATCH_START).write(out);
}
