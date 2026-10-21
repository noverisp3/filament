//! A thing that happened at a place: a sound, or a burst of particles.

use super::id;
use super::pack_block_pos;
use crate::protocol::VarInt;

/// The two event numbers this uses:
///
/// - `2001` is a block being destroyed. Its data is the **state id of the block
///   that was there**, because the client looks the block up from that number to
///   decide what the particles should look like.
/// - `2014` is the same without the sound.
pub const BLOCK_DESTROYED: i32 = 2001;
pub const BLOCK_DESTROYED_QUIET: i32 = 2014;

/// Writes a `level_event`.
///
/// The type and the data are plain four byte ints, not VarInts. Nearly every
/// other number in the play packets is a VarInt, which is exactly why these two
/// are worth saying out loud: a VarInt here reads the first three bytes as a
/// length and the client reports the rest as trailing garbage.
pub fn write_level_event(
    out: &mut Vec<u8>,
    event: i32,
    pos: (i32, i32, i32),
    data: i32,
    global: bool,
) {
    VarInt(id::LEVEL_EVENT).write(out);
    // The event and the data are plain four-byte ints here, not VarInts.
    //
    // Every other number in the play packets is a VarInt, which is what makes
    // this one worth a note. It is read with `readInt` and written with
    // `writeInt`, and a test that once "proved" the opposite was wrong.
    out.extend_from_slice(&event.to_be_bytes());
    out.extend_from_slice(&pack_block_pos(pos.0, pos.1, pos.2).to_be_bytes());
    out.extend_from_slice(&data.to_be_bytes());
    out.push(global as u8);
}
