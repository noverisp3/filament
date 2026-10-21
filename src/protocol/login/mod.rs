//! The login phase: the client names itself, the server answers.
//!
//! Offline mode only. There is no encryption here, so the exchange is: the
//! client names itself, the server answers with a profile, optionally turns
//! compression on, and the client acknowledges before the connection moves to
//! the configuration phase.
//!
//! Encryption is not a simplification for its own sake. It is skipped because
//! offline mode has nothing to authenticate: there is no Mojang session to
//! verify, so the RSA key exchange and the AES step that would follow protect
//! nothing and cost a large dependency.

pub mod hello_c2s;
pub mod login_acknowledged_c2s;
pub mod login_compression_s2c;
pub mod login_finished_s2c;
pub mod uuid;

pub use hello_c2s::read_hello;
pub use login_acknowledged_c2s::login_acknowledged_body;
pub use login_compression_s2c::write_login_compression;
pub use login_finished_s2c::write_login_finished;
pub use uuid::Uuid;

use crate::protocol::VarInt;

/// Packet ids in the login state, read from `packets.json`.
pub mod id {
    pub const LOGIN_DISCONNECT: i32 = 0;
    pub const HELLO: i32 = 1;
    pub const LOGIN_FINISHED: i32 = 2;
    pub const LOGIN_COMPRESSION: i32 = 3;
    pub const CUSTOM_QUERY: i32 = 4;
    pub const COOKIE_REQUEST: i32 = 5;

    pub const SERVERBOUND_HELLO: i32 = 0;
    pub const SERVERBOUND_KEY: i32 = 1;
    pub const SERVERBOUND_CUSTOM_QUERY_ANSWER: i32 = 2;
    pub const SERVERBOUND_LOGIN_ACKNOWLEDGED: i32 = 3;
    pub const SERVERBOUND_COOKIE_RESPONSE: i32 = 4;
}

/// `ClientboundHelloPacket`, which a real server sends to start encryption.
///
/// A server that is not going to encrypt must never send this: the client
/// replies with a key packet and waits for an encryption response, so sending
/// it and then going quiet looks like the server hanging.
pub const HELLO_WITH_PUBLIC_KEY: i32 = 1;

#[cfg(test)]
mod tests;
