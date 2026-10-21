//! The game event that holds the loading screen.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundGameStateChangePacket` for the event that holds the
/// loading screen.
///
/// Reason 13 is "start waiting for level chunks". It is a VarInt and a float,
/// and the capture sends 13 with 0.0, which is what tells the client to wait
/// rather than to give up waiting.
pub fn write_game_event(out: &mut Vec<u8>, reason: i32, value: f32) {
    VarInt(id::GAME_EVENT).write(out);
    VarInt(reason).write(out);
    out.extend_from_slice(&value.to_be_bytes());
}
