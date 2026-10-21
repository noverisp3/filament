//! What the player is allowed to do.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundPlayerAbilitiesPacket`.
///
/// A bitfield byte and two floats: the flying speed and the walking speed. There
/// is no field for a field of view and no optional marker, which is what an
/// earlier version of this wrote and why the client reported two bytes extra.
///
/// Nothing is granted. The bitfield stays zero, so the player is not
/// invulnerable, is not flying, may not fly and is not in creative mode.
pub fn write_player_abilities(out: &mut Vec<u8>) {
    VarInt(id::PLAYER_ABILITIES).write(out);

    // invulnerable | flying | may fly | instabuild
    out.push(0);

    out.extend_from_slice(&flying_speed().to_be_bytes());
    out.extend_from_slice(&walking_speed().to_be_bytes());
}

/// The default flying speed, 0.05.
///
/// The literal rather than a value read from the player, because there is no
/// player yet: this is what a fresh login is sent.
fn flying_speed() -> f32 {
    0.05
}

/// The default walking speed, 0.1.
fn walking_speed() -> f32 {
    0.1
}
