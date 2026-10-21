//! The first packet any client sends.

use crate::protocol::{DecodeError, VarInt};

/// What the client intends to do after the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientIntent {
    /// List the server in the multiplayer browser.
    Status,
    /// Join the server.
    Login,
    /// Move this connection to another server without re-handshaking.
    Transfer,
}

impl ClientIntent {
    /// The ids `ClientIntent` assigns, which are 1-based rather than 0-based.
    pub fn from_id(id: i32) -> Option<Self> {
        match id {
            1 => Some(ClientIntent::Status),
            2 => Some(ClientIntent::Login),
            3 => Some(ClientIntent::Transfer),
            _ => None,
        }
    }

    pub const fn id(self) -> i32 {
        match self {
            ClientIntent::Status => 1,
            ClientIntent::Login => 2,
            ClientIntent::Transfer => 3,
        }
    }
}

/// `MAX_HOST_LENGTH` is 255, unlike the 32767 used everywhere else, because the
/// host is a hostname and not free text.
pub const MAX_HOST_LENGTH: usize = 255;

/// The only packet id in the handshaking state.
pub const HANDSHAKE_INTENTION_ID: i32 = 0;

/// `ClientIntentionPacket`, packet id 0 in the handshaking state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIntention {
    pub protocol_version: VarInt,
    pub host: String,
    /// The port the client believes it connected to. Big-endian on the wire.
    pub port: u16,
    pub intent: ClientIntent,
}

impl ClientIntention {
    /// Reads the packet body, given a slice that starts after the packet id.
    pub fn read(mut r: &[u8]) -> Result<Self, DecodeError> {
        // The frame body begins with the packet id, which for this packet is
        // always 0. It has to be consumed here: skipping it is not optional,
        // because reading the protocol version from the id silently succeeds
        // and then reads the real version as the host length.
        let (VarInt(id), _) = VarInt::read(&mut r)?;
        if id != HANDSHAKE_INTENTION_ID {
            return Err(DecodeError::TooLong);
        }

        let (protocol_version, _) = VarInt::read(&mut r)?;

        // 255, not the 32767 other strings allow: this field is a hostname.
        let (VarInt(len_raw), _) = VarInt::read(&mut r)?;
        if len_raw < 0 {
            return Err(DecodeError::TooLong);
        }
        let len = len_raw as usize;
        if len > MAX_HOST_LENGTH || len > r.len() {
            return Err(DecodeError::StringTooLong {
                len,
                available: r.len(),
            });
        }
        let host = std::str::from_utf8(&r[..len])
            .map_err(|_| DecodeError::NotUtf8)?
            .to_owned();
        r = &r[len..];

        // Big-endian, unlike a VarInt: `writeShort` in Netty writes the most
        // significant byte first.
        if r.len() < 2 {
            return Err(DecodeError::UnexpectedEof {
                wanted: "port",
                got: r.len(),
            });
        }
        let port = u16::from_be_bytes([r[0], r[1]]);
        r = &r[2..];

        let (VarInt(intent), _) = VarInt::read(&mut r)?;
        let intent = ClientIntent::from_id(intent).ok_or(DecodeError::TooLong)?;

        Ok(ClientIntention {
            protocol_version,
            host,
            port,
            intent,
        })
    }

    pub fn write(&self, w: &mut Vec<u8>) {
        VarInt(HANDSHAKE_INTENTION_ID).write(w);
        self.protocol_version.write(w);
        VarInt(self.host.len() as i32).write(w);
        w.extend_from_slice(self.host.as_bytes());
        w.extend_from_slice(&self.port.to_be_bytes());
        VarInt(self.intent.id()).write(w);
    }
}
