//! The news that something hurt an entity.

use super::id;
use crate::protocol::VarInt;

/// The damage types of the game, by the id the client was told.
///
/// The order the client is sent the damage types in is the order they go in
/// here: `minecraft:fall` is the tenth and `minecraft:lava` the twenty-fourth.
pub mod damage {
    /// A fall from too high up.
    pub const FALL: i32 = 10;
    /// Touching a cactus.
    pub const CACTUS: i32 = 2;
    /// Being in lava.
    pub const LAVA: i32 = 24;
    /// Being on fire.
    pub const IN_FIRE: i32 = 21;
    /// Being under water with no air left.
    pub const DROWN: i32 = 6;
    /// Being inside a block with the head in it.
    pub const IN_WALL: i32 = 22;
    /// Damage with no other name.
    pub const GENERIC: i32 = 18;
}

/// Writes the news that something hurt an entity.
///
/// ```text
/// VarInt   the entity id
/// VarInt   which damage it was
/// VarInt   what caused it, plus one, and zero for nothing
/// VarInt   what hit it directly, plus one, and zero for nothing
/// byte     whether there is a place it came from
/// double   x, y, z   when there is
/// ```
///
/// The two ids are counted from one, because zero is the way of saying there
/// was nothing there.
pub fn write_damage_event(
    out: &mut Vec<u8>,
    entity_id: i32,
    damage_type: i32,
    cause_id: i32,
    direct_id: i32,
    from: Option<(f64, f64, f64)>,
) {
    VarInt(id::DAMAGE_EVENT).write(out);
    VarInt(entity_id).write(out);
    VarInt(damage_type).write(out);
    // Plus one, so that zero means there was nothing there.
    VarInt(cause_id + 1).write(out);
    VarInt(direct_id + 1).write(out);
    match from {
        Some((x, y, z)) => {
            out.push(1);
            out.extend_from_slice(&x.to_be_bytes());
            out.extend_from_slice(&y.to_be_bytes());
            out.extend_from_slice(&z.to_be_bytes());
        }
        None => out.push(0),
    }
}
