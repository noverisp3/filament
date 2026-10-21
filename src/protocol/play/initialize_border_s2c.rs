//! The edge of the world.

use super::id;
use crate::protocol::{VarInt, VarLong};

/// The centre a border with no restriction is parked at.
///
/// 59_999_968 is the value a stock server sends. Any centre works when the size
/// is zero, but matching the capture keeps the packet byte-identical.
pub const MAX_BORDER: f64 = 59_999_968.0;

/// Writes `ClientboundInitializeBorderPacket`.
///
/// Two sizes, two centres and then four numbers that are not doubles. The
/// record alone suggests seven doubles; the capture says four, followed by a
/// VarLong and three VarInts, and it walks to exactly forty bytes.
///
/// A zero size means the whole world is playable, which is right for a world
/// that is not finished.
pub fn write_initialize_border(out: &mut Vec<u8>) {
    VarInt(id::INITIALIZE_BORDER).write(out);

    let doubles = [
        0.0,         // old size
        0.0,         // new size
        MAX_BORDER,  // centre x
        MAX_BORDER,  // centre z
    ];
    for value in doubles {
        out.extend_from_slice(&value.to_be_bytes());
    }

    VarLong(0).write(out); // lerp time, in ticks
    VarInt(29_999_984).write(out); // absolute maximum size
    VarInt(5).write(out); // warning distance, in blocks
    VarInt(300).write(out); // warning delay, in ticks
}
