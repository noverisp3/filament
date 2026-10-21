//! Settling a block change the client has already guessed.

use super::id;
use crate::protocol::VarInt;

/// Writes a `block_changed_ack`, which is how a block change stops being a guess.
///
/// The client applies a block change straight away -- it draws the block gone
/// before the player has finished swinging. What it does not do straight away
/// is throw away the shape the block used to have: until this arrives the block
/// is invisible and still solid, and a player stands on air and walks into a
/// wall that is not drawn.
///
/// One VarInt, a sequence number counting the changes this connection has been
/// told are final. The client reads it as a counter, so what matters is that it
/// goes up.
pub fn write_block_changed_ack(out: &mut Vec<u8>, sequence: i32) {
    VarInt(id::BLOCK_CHANGED_ACK).write(out);
    VarInt(sequence).write(out);
}
