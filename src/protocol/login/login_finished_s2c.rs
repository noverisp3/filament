//! The profile the server answers with.

use super::id;
use super::uuid::Uuid;
use crate::protocol::VarInt;

/// Writes `ClientboundLoginFinishedPacket`.
///
/// An empty property list is correct rather than a placeholder: the list is a
/// counted collection, and zero is a valid count. The client reads it and finds
/// no textures, which is what an offline player has.
pub fn write_login_finished(
    out: &mut Vec<u8>,
    profile_id: Uuid,
    name: &str,
    session_id: Uuid,
) {
    VarInt(id::LOGIN_FINISHED).write(out);

    profile_id.write(out);

    // The name is capped at 16 upstream.
    VarInt(name.len() as i32).write(out);
    out.extend_from_slice(name.as_bytes());

    // Property count: zero. A `readCount` cap of 16 applies, and zero is under it.
    VarInt(0).write(out);

    session_id.write(out);
}
