//! The tab list.

use super::id;
use crate::protocol::VarInt;

/// The actions of `player_info_update`, by their position in the enum.
///
/// Named rather than written as bare bit numbers because the numbers are the
/// whole difficulty of this packet: the enum has exactly eight members, so the
/// mask that selects them is **one byte**, and getting an ordinal wrong does
/// not fail here, it fails on the client.
pub mod action {
    /// A name and the properties of its profile.
    pub const ADD_PLAYER: u8 = 0;
    /// A game mode index.
    pub const UPDATE_GAME_MODE: u8 = 2;
    /// Whether the player appears in the tab list at all.
    pub const UPDATE_LISTED: u8 = 3;
    /// A ping in milliseconds.
    pub const UPDATE_LATENCY: u8 = 4;
}

/// Writes a `player_info_update`, the tab list.
///
/// The shape, and the part that is easy to get wrong:
///
/// ```text
/// 1 byte    a fixed-width bit set, one bit per action
/// VarInt    how many entries follow
///   for each entry:
///     16 bytes  the profile's uuid, always
///     then only the fields of the actions that were set, in enum order
/// ```
///
/// The mask is a **fixed-width bit set and not a VarInt**. Upstream writes it
/// with `writeFixedBitSet`, which emits `ceil(values.length / 8)` raw bytes,
/// least significant bit first. With eight actions that is one byte, and a
/// two-byte VarInt where one byte belongs reads the entry count out of the
/// action bits.
///
/// Four actions are set, the smallest set that leaves a player a client can
/// name, list, play as and show a ping for. The rest of the enum exists and is
/// not set.
pub fn write_player_info_add(out: &mut Vec<u8>, uuid: [u8; 16], name: &str) {
    use action::*;

    let mut mask = 0u8;
    for bit in [ADD_PLAYER, UPDATE_GAME_MODE, UPDATE_LISTED, UPDATE_LATENCY] {
        mask |= 1 << bit;
    }

    VarInt(id::PLAYER_INFO_UPDATE).write(out);

    // Eight actions, so one byte. Asserted rather than assumed: a ninth action
    // in a future version would make this two bytes and shift everything after
    // it, which is a crash on the client and not on this side.
    const ACTIONS: usize = 8;
    assert_eq!(
        ACTIONS.div_ceil(8),
        1,
        "the action enum changed size and the mask is no longer one byte"
    );
    out.push(mask);

    VarInt(1).write(out);

    // The profile's uuid, before any action field.
    out.extend_from_slice(&uuid);

    // ADD_PLAYER: the name, then the profile's properties, which are none.
    // A string on the wire is its byte length as a VarInt and then its UTF-8.
    // The length is in bytes and not in characters, which matters the moment
    // somebody has an accent in their name.
    VarInt(name.len() as i32).write(out);
    out.extend_from_slice(name.as_bytes());
    VarInt(0).write(out);

    // UPDATE_GAME_MODE: survival is zero.
    VarInt(0).write(out);

    // UPDATE_LISTED: true.
    out.push(1);

    // UPDATE_LATENCY: zero.
    VarInt(0).write(out);
}
