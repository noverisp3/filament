//! The pong the server sends back, echoing the ping.

use super::ping_c2s::PingRequest;
use crate::protocol::VarInt;

/// Writes a pong response body given a packet id of 1.
pub fn pong_response_body(ping: &PingRequest) -> Vec<u8> {
    let mut body = Vec::new();
    VarInt(1).write(&mut body);
    body.extend_from_slice(&ping.id.to_be_bytes());
    body
}
