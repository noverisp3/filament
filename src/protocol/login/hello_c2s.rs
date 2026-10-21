//! The name the player wants, and an id they claim.

use super::id;
use super::uuid::Uuid;
use crate::protocol::{DecodeError, VarInt};

/// `ServerboundHelloPacket`: the name the player wants, and an id they claim.
///
/// The id is ignored in offline mode. Reading it rather than skipping it keeps
/// the reader positioned for whatever follows.
pub fn read_hello(body: &[u8]) -> Result<(String, Uuid), DecodeError> {
    let mut r = body;

    let (VarInt(id), _) = VarInt::read(&mut r)?;
    if id != id::SERVERBOUND_HELLO {
        return Err(DecodeError::TooLong);
    }

    // 16 characters: `ClientboundHelloPacket` reads the name with that cap,
    // and the client refuses to send a longer one.
    let (VarInt(len), _) = VarInt::read(&mut r)?;
    if len < 0 || len as usize > r.len() {
        return Err(DecodeError::StringTooLong {
            len: len.max(0) as usize,
            available: r.len(),
        });
    }
    let name = std::str::from_utf8(&r[..len as usize])
        .map_err(|_| DecodeError::NotUtf8)?
        .to_owned();
    r = &r[len as usize..];

    let uuid = Uuid::read(&mut r)?;

    Ok((name, uuid))
}
