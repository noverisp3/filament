//! The end of a batch of chunks.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundChunkBatchFinishedPacket`.
///
/// The field is how many chunks are in the batch that just finished, as a
/// VarInt. It was a float here for a while, on the assumption that it carried a
/// tick rate, and the client reported the packet as three bytes longer than it
/// expected.
pub fn write_chunk_batch_finished(out: &mut Vec<u8>, batch_size: i32) {
    VarInt(id::CHUNK_BATCH_FINISHED).write(out);
    VarInt(batch_size).write(out);
}
