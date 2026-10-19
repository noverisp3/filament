//! Packets of the handshake and status phases.
//!
//! The handshake is one packet, and it decides everything that follows: a
//! client asking to be listed in the server browser wants status, and a client
//! that wants to join is in the same connection one packet later. Both are
//! answered here, which is the smallest thing a server can be that a real
//! client will still talk to.
//!
//! Field layouts come from `ClientIntentionPacket` and `ClientboundStatusResponsePacket`
//! in the decompiled 26.3 server. The one that catches people is the port: it is
//! a `writeShort`, which Netty writes big-endian, while VarInts are little
//! endian seven-bit groups. Getting that backwards does not corrupt anything
//! obvious, it just makes the server appear on the wrong port.

use std::borrow::Cow;

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

/// `ClientIntentionPacket`, packet id 0 in the handshaking state.
///
/// `MAX_HOST_LENGTH` is 255, unlike the 32767 used everywhere else, because the
/// host is a hostname and not free text.
pub const MAX_HOST_LENGTH: usize = 255;

/// The only packet id in the handshaking state.
pub const HANDSHAKE_INTENTION_ID: i32 = 0;

/// The first packet any client sends.
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
        // because reading the protocol version from the id silently succeeds and
        // then reads the real version as the host length. The symptom is a
        // "string announced 777 bytes" error, because 777 is the version.
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

/// The JSON a server answers with when a client asks for its status.
///
/// Only the fields the client actually needs are here. `favicon` is left out
/// entirely rather than sent as null: it is optional upstream, and a null where
/// the client expects a string is the kind of thing that shows up as a blank
/// server icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub description: String,
    /// Shown as `N / max` in the browser.
    pub online: u32,
    pub max: u32,
    /// Who is on, shown when the browser list is hovered. Empty sends no
    /// `sample` field at all rather than an empty list, which the client
    /// renders as a server that forgot to say.
    pub sample: Vec<(String, String)>,
    /// The name in the browser, e.g. `filament 26.3`.
    pub version_name: String,
    pub protocol_version: i32,
}

impl ServerStatus {
    /// Renders the status as the JSON string that goes on the wire.
    ///
    /// Written by hand rather than through a JSON library because the shape is
    /// fixed and the escaping rules are narrow: only the description and the
    /// version name are free text. Every character that needs escaping in a JSON
    /// string is replaced, so a server description containing a quote or a
    /// newline cannot produce invalid JSON, which the client treats as a broken
    /// server rather than as a broken description.
    pub fn to_json(&self) -> String {
        // The player list, if there is anyone to name. Written as a fragment
        // so the shape without one stays exactly what it was.
        let sample = if self.sample.is_empty() {
            String::new()
        } else {
            let entries: Vec<String> = self
                .sample
                .iter()
                .map(|(name, uuid)| {
                    format!(
                        r#"{{"name":{},"id":{}}}"#,
                        json_escape(name),
                        json_escape(uuid)
                    )
                })
                .collect();
            format!(r#","sample":[{}]"#, entries.join(","))
        };

        format!(
            r#"{{"version":{{"name":{},"protocol":{}}},
"players":{{"max":{},"online":{}{}}},
"description":{{"text":{}}}}}"#,
            json_escape(&self.version_name),
            self.protocol_version,
            self.max,
            self.online,
            sample,
            json_escape(&self.description),
        )
    }
}

/// Escapes a Rust string into a JSON string literal, including the quotes.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control characters have no short escape and must not go through
            // raw, or they produce invalid JSON.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Writes a status response body, given a packet id of 0.
pub fn status_response_body(status: &ServerStatus) -> Vec<u8> {
    let json = status.to_json();
    let mut body = Vec::with_capacity(json.len() + 8);

    VarInt(0).write(&mut body);
    VarInt(json.len() as i32).write(&mut body);
    body.extend_from_slice(json.as_bytes());
    body
}

/// Reads a status request body. It is empty, so only the id is present.
pub fn status_request_id(body: &[u8]) -> Result<VarInt, DecodeError> {
    let mut r = body;
    VarInt::read(&mut r).map(|(v, _)| v)
}

/// A ping, which the client sends after the status response to measure latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingRequest {
    /// Echoed back verbatim; the client uses it to time the round trip.
    pub id: i64,
}

/// Reads a ping request body given a packet id of 1.
///
/// The payload is a fixed eight-byte long, not a VarLong: the game's own
/// packet reads a long on both sides.
pub fn ping_request_body(body: &[u8]) -> Result<PingRequest, DecodeError> {
    let mut r = body;
    // Leading packet id, then the payload the client wants echoed back.
    let (_, _) = VarInt::read(&mut r)?;
    if r.len() < 8 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "an eight-byte ping payload",
            got: r.len(),
        });
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&r[..8]);
    Ok(PingRequest {
        id: i64::from_be_bytes(bytes),
    })
}

/// Writes a pong response body given a packet id of 1.
pub fn pong_response_body(ping: &PingRequest) -> Vec<u8> {
    let mut body = Vec::new();
    VarInt(1).write(&mut body);
    body.extend_from_slice(&ping.id.to_be_bytes());
    body
}

/// Convenience for building a status from parts, used by the server.
pub fn status(
    description: impl Into<Cow<'static, str>>,
    version_name: impl Into<Cow<'static, str>>,
    protocol_version: i32,
    online: u32,
    max: u32,
) -> ServerStatus {
    ServerStatus {
        description: description.into().into_owned(),
        version_name: version_name.into().into_owned(),
        protocol_version,
        online,
        max,
        sample: Vec::new(),
    }
}

#[cfg(test)]
mod tests;