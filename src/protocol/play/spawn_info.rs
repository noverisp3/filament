//! The spawn information a client is given about the world it is joining.
//!
//! The same shape in the login packet and in the respawn packet: the game
//! calls it `CommonPlayerSpawnInfo` and it holds the dimension, the seed and
//! the rules of the world rather than a position.

use crate::protocol::VarInt;

/// The spawn information a client is given about the world it is joining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnInfo<'a> {
    pub dimension: &'a str,
    pub seed: i64,
    pub game_type: u8,
    pub is_flat: bool,
    pub portal_cooldown: i32,
    pub sea_level: i32,
}

/// Writes the tail of `ClientboundLoginPacket`, which upstream calls
/// `CommonPlayerSpawnInfo`.
///
/// Despite the name this holds no position. The dimension is a holder, written
/// as a zero byte followed by the registry key; the zero is what marks the
/// holder as being sent by key rather than by inline contents, and writing the
/// key without it makes the client read the length as part of the payload.
pub fn write_spawn_info(out: &mut Vec<u8>, info: &SpawnInfo<'_>) {
    let SpawnInfo {
        dimension,
        seed,
        game_type,
        is_flat,
        portal_cooldown,
        sea_level,
    } = *info;
    out.push(0);
    VarInt(dimension.len() as i32).write(out);
    out.extend_from_slice(dimension.as_bytes());

    out.extend_from_slice(&seed.to_be_bytes());

    out.push(game_type);
    // An optional previous game type, present only when there is one.
    out.push(0);
    out.push(0); // isDebug
    out.push(is_flat as u8);
    // An optional last death location, absent.
    out.push(0);

    // VarInts, not the ints the record declares them as. The declaration is
    // what led to writing four bytes each: the client reads one, stops on the
    // first zero byte of the pair, and reports the rest as trailing bytes.
    VarInt(portal_cooldown).write(out);
    VarInt(sea_level).write(out);
}
