//! What the client is asking the server to do to a block.

use super::id;
use super::serverbound_body;
use crate::protocol::{DecodeError, VarInt};

/// What the client is asking the server to do to a block.
///
/// Only the three actions that matter for breaking are named; the rest of the
/// enum (dropping items, swapping hands, opening a chest) is not acted on yet,
/// so it reads as a raw number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerAction {
    StartDestroy,
    AbortDestroy,
    StopDestroy,
    Other,
}

/// One `player_action` from the client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerActionPacket {
    pub action: PlayerAction,
    /// The block being acted on, still packed as the wire sent it.
    pub pos: i64,
    /// Which face was clicked, 0 to 5. Unused for now.
    pub direction: u8,
    /// The client's own prediction number for this action.
    ///
    /// The last field on the wire, and the one that was being dropped: a
    /// player that breaks a block has already applied the change locally and is
    /// waiting to be told it was right. It tags its guess with a number it owns,
    /// and the server has to hand that number back for the guess to be settled
    /// and the shape the block used to have to be thrown away.
    pub sequence: i32,
}

impl PlayerActionPacket {
    /// Decodes a `player_action` frame.
    pub fn read(frame: &[u8]) -> Result<Self, DecodeError> {
        let mut r = serverbound_body(frame, id::SERVERBOUND_PLAYER_ACTION)?;

        // The action is an enum, which travels as a VarInt.
        let (raw, _) = VarInt::read(&mut r)?;
        let action = match raw.0 {
            0 => PlayerAction::StartDestroy,
            2 => PlayerAction::AbortDestroy,
            3 => PlayerAction::StopDestroy,
            _ => PlayerAction::Other,
        };

        let pos = read_i64(&mut r)?;
        let direction = read_u8(&mut r)?;

        // The client's prediction number, last on the wire. It was being
        // dropped, and dropping it is what left a broken block drawn but solid.
        let (sequence, _) = VarInt::read(&mut r)?;

        Ok(Self {
            action,
            pos,
            direction,
            sequence: sequence.0,
        })
    }
}

/// A signed 64-bit integer straight off the wire, which is how a block position
/// travels: one long holding the x, y and z packed together.
fn read_i64(r: &mut &[u8]) -> Result<i64, DecodeError> {
    if r.len() < 8 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a long",
            got: r.len(),
        });
    }
    let bytes: [u8; 8] = r[..8].try_into().unwrap();
    *r = &r[8..];
    Ok(i64::from_be_bytes(bytes))
}

/// One byte straight off the wire.
fn read_u8(r: &mut &[u8]) -> Result<u8, DecodeError> {
    let byte = *r.first().ok_or(DecodeError::UnexpectedEof {
        wanted: "a byte",
        got: r.len(),
    })?;
    *r = &r[1..];
    Ok(byte)
}
