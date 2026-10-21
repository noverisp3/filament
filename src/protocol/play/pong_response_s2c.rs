//! The answer to the client's ping.

use super::id;
use crate::protocol::VarInt;

/// Writes the pong, which carries the same eight bytes back.
pub fn write_pong_response(out: &mut Vec<u8>, time: i64) {
    VarInt(id::PONG_RESPONSE).write(out);
    // The same eight bytes the client sent, unchanged: the payload is a fixed
    // long on both sides, and the client works the round trip out of the
    // difference between the two.
    out.extend_from_slice(&time.to_be_bytes());
}
