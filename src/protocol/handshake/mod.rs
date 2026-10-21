//! The handshake phase: the one packet a client sends before anything else.
//!
//! The client says what it wants (to be listed in the browser, or to join) and
//! which version it speaks, and the server's answer is in the phase that
//! follows.
//!
//! The port is a `writeShort`, which Netty writes big-endian, while VarInts
//! are little-endian seven-bit groups. Getting that backwards does not
//! corrupt anything obvious, it just makes the server appear on the wrong port.

pub mod intention_c2s;

pub use intention_c2s::{ClientIntent, ClientIntention, HANDSHAKE_INTENTION_ID, MAX_HOST_LENGTH};

use crate::protocol::VarInt;

#[cfg(test)]
mod tests;
