//! The game clock, and where the sun is.

use super::id;
use crate::protocol::{VarInt, VarLong};

/// `world_clock` registry id of `minecraft:overworld`.
///
/// Ordinals come from registration order, and `WorldClocks.bootstrap` registers
/// the overworld before the end.
const WORLD_CLOCK_OVERWORLD: i32 = 0;

/// Writes `ClientboundSetTimePacket`: the game clock, and where the sun is.
///
/// Two parts, and they are not the same number. `game_time` is how long the
/// world has been running and only ever goes up. The map is the world's own
/// clocks, keyed by registry id, and the overworld's total is the sun's
/// position -- the part that wraps at twenty-four thousand.
///
/// Sent once when a player arrives and then only when the time is changed,
/// because the client runs this clock itself from whatever it is told.
pub fn write_set_time(out: &mut Vec<u8>, game_time: i64, day_time: i64) {
    VarInt(id::SET_TIME).write(out);
    out.extend_from_slice(&game_time.to_be_bytes());

    // A map is a count and then that many entries.
    VarInt(1).write(out);
    VarInt(WORLD_CLOCK_OVERWORLD).write(out);
    VarLong::zigzag(day_time).write(out);
    // Partial tick is the fraction into the current tick. Zero here because the
    // server has no sub-tick to report, and inventing one would make the sun
    // jitter by up to a tick against everything else that happens on a tick.
    out.extend_from_slice(&0.0f32.to_be_bytes());
    // Rate one: real time is real time. Zero is what a paused world sends, and
    // this one is not paused.
    out.extend_from_slice(&1.0f32.to_be_bytes());
}

/// Writes `ClientboundSetTimePacket` with no clock updates, only the age.
///
/// The regular beat the server's game sends every twenty ticks, and the shape
/// matters more than it looks. An update carrying the clock overwrites what
/// the client has been running its own count against; the game's own periodic
/// sync carries an empty map, `Map.of()`, so the client keeps its smoothly
/// advanced clock and only the age it extrapolates by is corrected.
pub fn write_set_time_bare(out: &mut Vec<u8>, game_time: i64) {
    VarInt(id::SET_TIME).write(out);
    out.extend_from_slice(&game_time.to_be_bytes());

    // Zero entries: no clock update is carried, which is the point.
    VarInt(0).write(out);
}
