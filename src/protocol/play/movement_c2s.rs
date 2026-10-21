//! The movement packets, which share a shape and a decoder.

use super::id;
use crate::protocol::{DecodeError, VarInt};

/// The movement the client should apply, as `PositionMoveRotation` carries it.
///
/// A struct rather than nine arguments, since the three vectors and the two
/// angles are read together at every call site and a swapped pair would be a
/// silent bug rather than a compile error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Movement {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Velocity the client keeps after applying the move.
    pub delta_x: f64,
    pub delta_y: f64,
    pub delta_z: f64,
    /// Degrees, where 0 is south and increases clockwise.
    pub yaw: f32,
    /// Degrees, where -90 is straight up and 90 is straight down.
    pub pitch: f32,
}

impl Default for Movement {
    fn default() -> Self {
        Self {
            x: 0.5,
            y: 64.0,
            z: 0.5,
            delta_x: 0.0,
            delta_y: 0.0,
            delta_z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
        }
    }
}

/// One movement packet from the client, decoded.
///
/// The four packets share a class upstream and a shape on the wire: the ones
/// that carry a position open with three doubles, the ones that carry a
/// rotation follow with two floats, and every one of them closes with a byte
/// of flags. Which fields a packet carries depends on its id, and the id is
/// what says which of them below are `Some`.
///
/// The flags byte packs two booleans: bit 0 is whether the player is on the
/// ground, bit 1 is whether it hit a wall on the way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientMovement {
    /// The position, when the packet carries one.
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    /// The rotation in degrees, when the packet carries one.
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    /// Bit 0 of the trailing flags byte.
    pub on_ground: bool,
    /// Bit 1 of the trailing flags byte.
    pub horizontal_collision: bool,
}

impl ClientMovement {
    /// Decodes one of the four movement packets.
    ///
    /// `frame` is the whole packet: the id VarInt first, then the body. The id
    /// decides which fields follow, and an id that is not one of the four is
    /// rejected rather than guessed at.
    pub fn read(frame: &[u8]) -> Result<Self, DecodeError> {
        let mut r: &[u8] = frame;
        let (id, _) = VarInt::read(&mut r)?;
        let id = id.0;

        let has_pos = matches!(id, id::MOVE_PLAYER_POS | id::MOVE_PLAYER_POS_ROT);
        let has_rot = matches!(id, id::MOVE_PLAYER_POS_ROT | id::MOVE_PLAYER_ROT);

        if !matches!(
            id,
            id::MOVE_PLAYER_POS
                | id::MOVE_PLAYER_POS_ROT
                | id::MOVE_PLAYER_ROT
                | id::MOVE_PLAYER_STATUS_ONLY
        ) {
            return Err(DecodeError::UnexpectedEof {
                wanted: "a movement packet id",
                got: id as usize,
            });
        }

        let mut movement = ClientMovement {
            x: None,
            y: None,
            z: None,
            yaw: None,
            pitch: None,
            on_ground: false,
            horizontal_collision: false,
        };

        if has_pos {
            movement.x = Some(read_f64(&mut r)?);
            movement.y = Some(read_f64(&mut r)?);
            movement.z = Some(read_f64(&mut r)?);
        }

        if has_rot {
            movement.yaw = Some(read_f32(&mut r)?);
            movement.pitch = Some(read_f32(&mut r)?);
        }

        let flags = read_u8(&mut r)?;
        movement.on_ground = flags & 1 != 0;
        movement.horizontal_collision = flags & 2 != 0;

        Ok(movement)
    }
}

/// A double straight off the wire.
fn read_f64(r: &mut &[u8]) -> Result<f64, DecodeError> {
    if r.len() < 8 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a double",
            got: r.len(),
        });
    }
    let bytes: [u8; 8] = r[..8].try_into().unwrap();
    *r = &r[8..];
    Ok(f64::from_be_bytes(bytes))
}

/// A float straight off the wire.
fn read_f32(r: &mut &[u8]) -> Result<f32, DecodeError> {
    if r.len() < 4 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a float",
            got: r.len(),
        });
    }
    let bytes: [u8; 4] = r[..4].try_into().unwrap();
    *r = &r[4..];
    Ok(f32::from_be_bytes(bytes))
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
