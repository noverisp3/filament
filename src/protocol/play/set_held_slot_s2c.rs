//! Which of the nine slots the player is holding.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundSetHeldSlotPacket`.
///
/// A VarInt, not the short an earlier version used. The capture is two bytes
/// for the whole packet, which leaves one for the slot.
pub fn write_set_held_slot(out: &mut Vec<u8>, slot: i32) {
    VarInt(id::SET_HELD_SLOT).write(out);
    VarInt(slot).write(out);
}
