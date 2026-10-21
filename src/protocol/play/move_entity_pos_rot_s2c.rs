//! Somebody moved, as a delta rather than a place.

use super::angle_byte;
use super::id;
use crate::protocol::VarInt;

/// How finely a movement delta is measured: one part in 4096 of a block.
///
/// From the game's own codec, which rounds on the way out and divides on the
/// way back in. That is finer than a block, so a slow walk is a delta of a few
/// hundred rather than of zero.
pub const DELTA_SCALE: f64 = 4096.0;

/// The largest movement a delta can express, in blocks.
///
/// Three shorts at 1/4096 of a block a unit: 32767 of them is a little under
/// eight blocks. Anything past that does not fit in the packet, and it is the
/// number that decides when a movement has to be sent as a position instead.
pub const MAX_DELTA_BLOCKS: f64 = 32767.0 / DELTA_SCALE;

/// Writes a `move_entity_pos_rot`: somebody moved.
///
/// **A delta, not a position.** Each client is told how far the body moved since
/// that client last heard about it, which is why the server has to remember what
/// each observer was last told separately.
///
/// Returns false without writing anything when the movement is too large to be
/// a delta, which is the case for anything past about eight blocks. Clamping is
/// not an option: it moves the body a plausible-looking amount that is simply
/// wrong, and every later delta is measured from where the client thinks it is.
///
/// The shape:
///
/// ```text
/// VarInt   the entity id
/// VarInt   properties: bit 0 is on the ground, the rest is a step count
/// short    dx, dy, dz   when the step count is zero
/// byte     yaw
/// byte     pitch
/// ```
///
/// The step count is packed into the same VarInt as the ground flag, which is
/// why it is shifted left by one when written. A count of zero means the simple
/// form: three shorts and nothing else. A stepped movement instead carries,
/// per step, a tick count and three shorts, and this server never sends one.
pub fn write_move_entity_pos_rot(
    out: &mut Vec<u8>,
    entity_id: i32,
    delta: (f64, f64, f64),
    yaw: f32,
    pitch: f32,
    on_ground: bool,
) -> bool {
    if [delta.0, delta.1, delta.2]
        .iter()
        .any(|a| a.abs() > MAX_DELTA_BLOCKS)
    {
        return false;
    }

    VarInt(id::MOVE_ENTITY_POS_ROT).write(out);
    VarInt(entity_id).write(out);

    // No steps, and the ground flag in the low bit.
    VarInt(if on_ground { 1 } else { 0 }).write(out);

    for axis in [delta.0, delta.1, delta.2] {
        let scaled = (axis * DELTA_SCALE).round() as i16;
        out.extend_from_slice(&scaled.to_be_bytes());
    }

    out.push(angle_byte(yaw));
    out.push(angle_byte(pitch));
    true
}
