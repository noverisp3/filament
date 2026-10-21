//! Putting somebody into the world.

use super::angle_byte;
use super::id;
use crate::protocol::VarInt;

/// The entity type id for a player, in the registry the client was sent.
///
/// Read out of the data generator's report, where the entry carries an explicit
/// protocol id and the 161 of them run from 0 to 160 with nothing missing. That
/// matters: the client numbers this registry by the order it arrived in the
/// login frames, so an id that is off by one is a client trying to spawn the
/// wrong thing.
pub const PLAYER_ENTITY_TYPE: i32 = 159;

/// The entity type id for an item, in the registry the client was sent.
///
/// Seventy-two, from the same report as the player's one hundred and fifty
/// nine. An item on the ground is an entity of this type, and the client
/// draws it as the little spinning cube of whatever it is holding.
pub const ITEM_ENTITY_TYPE: i32 = 72;

/// Writes an `add_entity`: putting somebody into the world.
///
/// The three angles are a byte each, and they are **quantised to 1/256 of a
/// turn**, so 90.0 degrees is 64 and not 90. A full circle is 256.
///
/// The movement delta is not three doubles and not a long. It is a quantised
/// vector, and its length depends on the value: smaller than about 3e-5 in every
/// component it is **one byte**, zero, and otherwise six bytes.
///
/// Writing a fixed eight bytes for it, which reads like a packed long and is
/// what the constant names suggest, makes every client's packet eight bytes
/// longer than expected and drops the connection on the second player.
///
/// A player being spawned has not moved yet, so the delta is zero and the one
/// byte is the whole of it.
#[allow(clippy::too_many_arguments)]
pub fn write_add_entity(
    out: &mut Vec<u8>,
    entity_id: i32,
    uuid: [u8; 16],
    entity_type: i32,
    pos: (f64, f64, f64),
    yaw: f32,
    pitch: f32,
    data: i32,
) {
    VarInt(id::ADD_ENTITY).write(out);
    VarInt(entity_id).write(out);
    out.extend_from_slice(&uuid);
    VarInt(entity_type).write(out);

    out.extend_from_slice(&pos.0.to_be_bytes());
    out.extend_from_slice(&pos.1.to_be_bytes());
    out.extend_from_slice(&pos.2.to_be_bytes());

    // The delta, which is zero, which is one byte.
    out.push(0);

    // Three angles, and pitch *before* yaw. The order is the packet.
    //
    // ```java
    // output.writeByte(this.xRot);
    // output.writeByte(this.yRot);
    // output.writeByte(this.yHeadRot);
    // ```
    //
    // There is no head *pitch*: three angles and not four, and a fourth byte is
    // one too many on every entity ever spawned. But the first two are pitch
    // then yaw, and writing them the other way round leaves a body that faces
    // wherever it happened to be looking.
    out.push(angle_byte(pitch));
    out.push(angle_byte(yaw));
    out.push(angle_byte(yaw));

    // The data value: zero for a player, one for living things generally, and
    // zero for an item, which is not a living thing and carries nothing else.
    VarInt(data).write(out);
}
