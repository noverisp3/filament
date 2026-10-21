//! The beat that keeps the connection alive.

use super::id;
use crate::protocol::VarInt;

/// The stage that says "no crack here any more".
pub const NO_CRACK: i8 = -1;

/// Writes `ClientboundKeepAlivePacket`.
///
/// The id comes back verbatim, so it has to be distinguishable from one still
/// in flight when the answer arrives. The caller supplies it for that reason.
pub fn write_keep_alive(out: &mut Vec<u8>, id: i64) {
    VarInt(id::KEEP_ALIVE).write(out);
    out.extend_from_slice(&id.to_be_bytes());
}
