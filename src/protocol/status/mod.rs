//! The status phase: what the multiplayer browser asks, and what it is told.
//!
//! A client asks for the server's description, the server answers with JSON,
//! and the client then pings to measure the round trip.
//!
//! The layouts come from `ClientboundStatusResponsePacket` in the decompiled
//! 26.3 server.

pub mod ping_c2s;
pub mod pong_s2c;
pub mod status_request_c2s;
pub mod status_response_s2c;

pub use ping_c2s::{ping_request_body, PingRequest};
pub use pong_s2c::pong_response_body;
pub use status_request_c2s::status_request_id;
pub use status_response_s2c::{status, status_response_body, ServerStatus};
