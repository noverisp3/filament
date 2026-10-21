//! The request the client sends for the server's description.

use crate::protocol::{DecodeError, VarInt};

/// Reads a status request body. It is empty, so only the id is present.
pub fn status_request_id(body: &[u8]) -> Result<VarInt, DecodeError> {
    let mut r = body;
    VarInt::read(&mut r).map(|(v, _)| v)
}
