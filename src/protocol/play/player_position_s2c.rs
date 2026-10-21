//! Where the client thinks the player is.

use super::id;
use super::movement_c2s::Movement;
use crate::protocol::VarInt;

/// Writes `ClientboundPlayerPositionPacket`.
///
/// There is no flag byte: the packet is a teleport id, a
/// `PositionMoveRotation`, and a set of which fields are relative to where the
/// client thinks it already is. `PositionMoveRotation` is two vectors and two
/// angles, and the second vector has to be sent even at a teleport, because the
/// client keeps it as the player's velocity.
pub fn write_player_position(out: &mut Vec<u8>, teleport_id: i32, move_to: &Movement) {
    let Movement {
        x,
        y,
        z,
        delta_x,
        delta_y,
        delta_z,
        yaw,
        pitch,
    } = *move_to;

    VarInt(id::PLAYER_POSITION).write(out);
    VarInt(teleport_id).write(out);

    out.extend_from_slice(&x.to_be_bytes());
    out.extend_from_slice(&y.to_be_bytes());
    out.extend_from_slice(&z.to_be_bytes());

    out.extend_from_slice(&delta_x.to_be_bytes());
    out.extend_from_slice(&delta_y.to_be_bytes());
    out.extend_from_slice(&delta_z.to_be_bytes());

    out.extend_from_slice(&yaw.to_be_bytes());
    out.extend_from_slice(&pitch.to_be_bytes());

    // The relative set, empty. The client applies the position as absolute,
    // which is what a teleport should be. The encoding is four zero bytes in
    // the capture and none of the ten Relative constants is set.
    out.extend_from_slice(&[0, 0, 0, 0]);
}
