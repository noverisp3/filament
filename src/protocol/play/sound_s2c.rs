//! A sound to play, and where it comes from.

use super::id;
use crate::protocol::VarInt;

/// The sound of a player being hurt, and of one dying.
///
/// The ids are the registry ids the client knows them by, from the data
/// generator's report. The wire carries one more than these, because a holder
/// that names a registry entry is written as its id plus one.
pub mod sound {
    /// A player being hurt.
    pub const PLAYER_HURT: i32 = 1316;
    /// A player dying.
    pub const PLAYER_DEATH: i32 = 1315;

    /// Which slider a sound plays on. The order is the enum's own.
    pub const SOURCE_PLAYER: i32 = 6;
}

/// A sound to play, and where it comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sound {
    /// The registry id of the sound.
    pub id: i32,
    /// Which slider it plays on: `sound::SOURCE_PLAYER` and the rest.
    pub source: i32,
    /// Where it comes from, in blocks.
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// How loud, and how high pitched.
    pub volume: f32,
    pub pitch: f32,
    /// A seed the client uses to vary the sound, so two of the same do not
    /// sound identical.
    pub seed: i64,
}

/// Writes a sound, so a client can hear it.
///
/// The sound is a registry id, the source is which slider it plays on, and
/// then the place it comes from, how loud, how high pitched, and a seed the
/// client uses to vary the sound so two of the same do not sound identical.
pub fn write_sound(out: &mut Vec<u8>, sound: &Sound) {
    VarInt(id::SOUND).write(out);
    // The sound is a **holder**, and a holder that names a registry entry is
    // written as its id **plus one**: the zero is reserved for a holder that
    // carries the value itself, in which case the name follows as a string.
    //
    // So the id on the wire is one higher than the id in the registry, and a
    // server that writes the registry id itself makes the client play the
    // sound *before* the one it meant.
    VarInt(sound.id + 1).write(out);
    VarInt(sound.source).write(out);
    // The place is in eighths of a block: the game's own constructor takes
    // a block position and stores it multiplied by eight, and the client
    // divides it back. A sound sent in whole blocks is heard eight times
    // further away than it is.
    out.extend_from_slice(&(sound.x * 8).to_be_bytes());
    out.extend_from_slice(&(sound.y * 8).to_be_bytes());
    out.extend_from_slice(&(sound.z * 8).to_be_bytes());
    out.extend_from_slice(&sound.volume.to_be_bytes());
    out.extend_from_slice(&sound.pitch.to_be_bytes());
    out.extend_from_slice(&sound.seed.to_be_bytes());
}
