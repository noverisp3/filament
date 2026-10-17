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

use crate::protocol::{DecodeError, VarInt, VarLong};

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
        let mut limit = r.len().min(MAX_HOST_LENGTH);
        let (VarInt(len_raw), _) = VarInt::read(&mut r)?;
        if len_raw < 0 {
            return Err(DecodeError::TooLong);
        }
        let len = len_raw as usize;
        if len > MAX_HOST_LENGTH || len > r.len() {
            return Err(DecodeError::StringTooLong {
                len,
                available: limit,
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
        format!(
            r#"{{"version":{{"name":{},"protocol":{}}},
"players":{{"max":{},"online":{}}},
"description":{{"text":{}}}}}"#,
            json_escape(&self.version_name),
            self.protocol_version,
            self.max,
            self.online,
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
pub fn ping_request_body(body: &[u8]) -> Result<PingRequest, DecodeError> {
    let mut r = body;
    // Leading packet id, then the payload the client wants echoed back.
    let (_, _) = VarInt::read(&mut r)?;
    let (VarLong(id), _) = VarLong::read(&mut r)?;
    Ok(PingRequest { id })
}

/// Writes a pong response body given a packet id of 1.
pub fn pong_response_body(ping: &PingRequest) -> Vec<u8> {
    let mut body = Vec::new();
    VarInt(1).write(&mut body);
    VarLong(ping.id).write(&mut body);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_intention_packet_round_trips() {
        let pkt = ClientIntention {
            protocol_version: VarInt(777),
            host: "localhost".to_string(),
            port: 25565,
            intent: ClientIntent::Login,
        };

        let mut w = Vec::new();
        pkt.write(&mut w);
        let got = ClientIntention::read(&w).expect("decodes");

        assert_eq!(got, pkt);
    }

    #[test]
    fn the_port_is_big_endian() {
        // 25565 is 0x63DD. Written little-endian it would be DD 63, and the
        // client would read 56669, showing the server on the wrong port.
        let pkt = ClientIntention {
            protocol_version: VarInt(777),
            host: String::new(),
            port: 25565,
            intent: ClientIntent::Status,
        };

        let mut w = Vec::new();
        pkt.write(&mut w);

        // Leading packet id (one byte), then a zero-length host, then the port. The
        // intent VarInt follows the port, so it is excluded.
        let port_bytes = &w[w.len() - 3..w.len() - 1];
        assert_eq!(
            port_bytes,
            &[0x63, 0xDD],
            "the port must be written most significant byte first"
        );
    }

    #[test]
    fn every_intention_has_the_id_the_game_uses() {
        // `ClientIntent` numbers these from 1, not 0. Off by one here and the
        // client is refused before anything else happens.
        assert_eq!(ClientIntent::Status.id(), 1);
        assert_eq!(ClientIntent::Login.id(), 2);
        assert_eq!(ClientIntent::Transfer.id(), 3);

        assert_eq!(ClientIntent::from_id(1), Some(ClientIntent::Status));
        assert_eq!(ClientIntent::from_id(2), Some(ClientIntent::Login));
        assert_eq!(ClientIntent::from_id(0), None);
        assert_eq!(ClientIntent::from_id(4), None);
    }

    #[test]
    fn a_host_longer_than_the_cap_is_refused() {
        // The handshake allows 255, not the 32767 used for other strings.
        let long = "a".repeat(300);
        let mut w = Vec::new();
        VarInt(HANDSHAKE_INTENTION_ID).write(&mut w);
        VarInt(777).write(&mut w);
        VarInt(long.len() as i32).write(&mut w);
        w.extend_from_slice(long.as_bytes());
        w.extend_from_slice(&[0, 255]);
        VarInt(1).write(&mut w);

        assert!(ClientIntention::read(&w).is_err());
    }

    #[test]
    fn the_status_json_has_the_fields_the_client_reads() {
        let s = status("filament", "filament 26.3", 777, 0, 20);
        let json = s.to_json();

        assert!(json.contains(r#""protocol":777"#));
        assert!(json.contains(r#""max":20"#));
        assert!(json.contains(r#""online":0"#));
        assert!(json.contains(r#""text":"filament""#));
        assert!(json.contains(r#""name":"filament 26.3""#));
    }

    #[test]
    fn a_description_with_quotes_stays_valid_json() {
        // The description is server-controlled text that ends up in a JSON
        // string. Unescaped it produces invalid JSON, and the client reports
        // the server as broken rather than the description as broken.
        let s = status("say \"hi\"\nnow", "v", 777, 0, 20);
        let json = s.to_json();

        assert!(json.contains(r#""text":"say \"hi\"\nnow""#));

        // Counting lines only proves the format did not grow a literal newline
        // where the description had one. What actually matters is that the
        // document is still well-formed, so it is parsed back: a description
        // with a quote and a newline in it is the case that breaks naive
        // string formatting, and a round trip through a parser is what catches
        // it.
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json)
                .ok()
                .and_then(|v| v["description"]["text"].as_str().map(String::from)),
            Some("say \"hi\"\nnow".to_string()),
            "the description must survive a round trip through a JSON parser"
        );
    }

    #[test]
    fn a_control_character_in_a_description_is_escaped() {
        let s = status("\u{1}\u{1f}", "v", 777, 0, 20);
        let json = s.to_json();
        assert!(json.contains(r#"\u0001"#));
        assert!(json.contains(r#"\u001f"#));
    }

    #[test]
    fn a_ping_round_trips() {
        let ping = PingRequest { id: 1234567890123 };
        let body = pong_response_body(&ping);

        let got = ping_request_body(&body).expect("decodes");
        assert_eq!(got.id, ping.id);
    }
}