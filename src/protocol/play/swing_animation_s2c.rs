//! A hand going through a swing.

use super::id;
use crate::protocol::VarInt;

/// The hand a swing came from. Zero is the one most people use.
pub const MAIN_HAND: i32 = 0;
/// The off hand, which is what a client sends when shift is held while swinging.
pub const OFF_HAND: i32 = 1;

/// How a swing looks.
///
/// Read out of `SwingAnimationType`, which is a small enum with an explicit id
/// per value: none is zero, a sideways hit is one, and a straight stab is two.
pub const SWING_WHACK: i32 = 1;

/// How long a swing lasts, in ticks.
///
/// The game's own default, from `SwingAnimation.DEFAULT`. It is not a duration
/// the client ignores: the arm comes down on it, and a swing that says zero is
/// an arm that does not move.
pub const SWING_DURATION: i32 = 6;

/// Writes a `swing_animation`, so somebody else's swing is visible.
///
/// The player's own client already plays their swing when they click, from the
/// same packet that told us they clicked. This is the other half: what everyone
/// watching sees.
///
/// ```text
/// VarInt   the entity id
/// VarInt   which hand
/// VarInt   what the swing looks like
/// VarInt   how long it lasts, in ticks
/// ```
///
/// The last two are one field upstream, a record of a type and a duration, but
/// on the wire they are two VarInts.
pub fn write_swing_animation(
    out: &mut Vec<u8>,
    entity_id: i32,
    hand: i32,
    kind: i32,
    duration: i32,
) {
    VarInt(id::SWING_ANIMATION).write(out);
    VarInt(entity_id).write(out);
    VarInt(hand).write(out);
    VarInt(kind).write(out);
    VarInt(duration).write(out);
}
