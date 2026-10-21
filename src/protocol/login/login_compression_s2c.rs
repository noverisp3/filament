//! The threshold from which the server compresses.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundLoginCompressionPacket`.
pub fn write_login_compression(out: &mut Vec<u8>, threshold: i32) {
    VarInt(id::LOGIN_COMPRESSION).write(out);
    VarInt(threshold).write(out);
}
