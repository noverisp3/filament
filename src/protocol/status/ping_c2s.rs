//! The ping the client sends to measure the round trip.

use crate::protocol::{DecodeError, VarInt};

/// A ping, which the client sends after the status response to measure latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingRequest {
    /// Echoed back verbatim; the client uses it to time the round trip.
    pub id: i64,
}

/// Reads a ping request body given a packet id of 1.
///
/// The payload is a fixed eight-byte long, not a VarLong: the game's own
/// packet reads a long on both sides.
pub fn ping_request_body(body: &[u8]) -> Result<PingRequest, DecodeError> {
    let mut r = body;
    // Leading packet id, then the payload the client wants echoed back.
    let (_, _) = VarInt::read(&mut r)?;
    if r.len() < 8 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "an eight-byte ping payload",
            got: r.len(),
        });
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&r[..8]);
    Ok(PingRequest {
        id: i64::from_be_bytes(bytes),
    })
}
