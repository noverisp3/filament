//! How hard the world is.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundChangeDifficultyPacket`.
///
/// A difficulty index and then whether the difficulty is locked, both single
/// bytes. Peaceful is 0, so the world opens on easy by default unless told
/// otherwise.
pub fn write_difficulty(out: &mut Vec<u8>, difficulty: u8) {
    VarInt(id::CHANGE_DIFFICULTY).write(out);
    out.push(difficulty);
    out.push(0);
}
