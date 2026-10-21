//! The entity's metadata, which is how a pose reaches other people.

use super::id;
use super::ENTITY_DATA_END;
use crate::protocol::VarInt;

/// Which piece of an entity's metadata the pose is.
///
/// Six, not five. It is counted by the order the fields are declared on the
/// entity, and the decompiled source lists seven where the shipped jar has
/// eight: a `DATA_CUSTOM_NAME` that is not in the source.
pub const ENTITY_DATA_POSE: u8 = 6;

/// Which serializer reads the pose, out of the table the game registers them in.
///
/// Twenty. The table is filled by a method that is called from nowhere in the
/// decompiled tree, so it is in neither the source nor the capture; counting
/// the field declarations in the source gives fifteen, which is wrong.
pub const ENTITY_DATA_SERIALIZER_POSE: i32 = 20;

/// Writes an entity's pose, which is how a crouch reaches other people.
///
/// ```text
/// VarInt   the entity id
/// byte     which piece of metadata this is
/// VarInt   which serializer reads what follows
/// VarInt   the pose
/// byte     255, and nothing after it
/// ```
///
/// One item per packet, and the end marker every time. Metadata is a list of
/// updates terminated by a value that cannot be an index, and sending the marker
/// is not optional: a packet that stops after the pose is a packet the client
/// is still waiting on.
pub fn write_entity_pose(out: &mut Vec<u8>, entity_id: i32, crouching: bool) {
    VarInt(id::SET_ENTITY_DATA).write(out);
    VarInt(entity_id).write(out);
    out.push(ENTITY_DATA_POSE);
    VarInt(ENTITY_DATA_SERIALIZER_POSE).write(out);
    VarInt(if crouching { POSE_CROUCHING } else { POSE_STANDING }).write(out);
    out.push(ENTITY_DATA_END);
}

/// Standing, and the first of the game's pose values.
pub const POSE_STANDING: i32 = 0;
/// Crouching, which is the sixth. A count from zero, so this is a number and
/// not a flag.
pub const POSE_CROUCHING: i32 = 5;
