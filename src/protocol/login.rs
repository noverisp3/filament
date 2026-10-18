//! Packets of the login phase.
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

use crate::protocol::{DecodeError, VarInt};

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

/// A 128-bit UUID, written as two `u64` big-endian halves.
///
/// Big-endian again, and unlike a VarInt in both directions: Java's
/// `UUIDUtil.STREAM_CODEC` writes each half with `writeLong`, which is
/// big-endian. Getting this wrong produces a valid-looking packet that gives
/// every player the same identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Uuid(pub u128);

impl Uuid {
    /// The UUID an offline player gets: any stable value, since there is no
    /// account to look one up from.
    ///
    /// Derived from the name the way Mojang does for offline servers, so the
    /// same name always produces the same id. That matters because the client
    /// caches the identity between sessions and a changing one shows up as the
    /// player's skin and name resetting.
    pub fn offline_for(name: &str) -> Self {
        // `UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))`
        // upstream, which is MD5 of that string with version 3 stamped into the
        // result. Matching it exactly is what makes a name look the same here as
        // on any other offline-mode server.
        use md5::{Digest, Md5};

        let input = format!("OfflinePlayer:{name}");
        let digest = Md5::digest(input.as_bytes());

        let mut out = [0u8; 16];
        out.copy_from_slice(&digest);

        // Version 3, and the IETF variant. Without these a strict client treats
        // the id as malformed rather than as an offline player.
        out[6] = (out[6] & 0x0F) | 0x30;
        out[8] = (out[8] & 0x3F) | 0x80;

        Uuid(u128::from_be_bytes(out))
    }

    /// The same id as the sixteen big-endian bytes the play phase carries it as.
    ///
    /// A `u128` here because that is what the login phase's string form is
    /// parsed from and compared against, and keeping one representation avoids a
    /// conversion at every call site. The play phase wants raw bytes, and a
    /// conversion that exists only to be forgotten at one of its call sites is
    /// a conversion worth naming once.
    pub fn bytes(&self) -> [u8; 16] {
        self.0.to_be_bytes()
    }

    pub fn read(r: &mut &[u8]) -> Result<Self, DecodeError> {
        if r.len() < 16 {
            return Err(DecodeError::UnexpectedEof {
                wanted: "UUID",
                got: r.len(),
            });
        }
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&r[..16]);
        *r = &r[16..];
        Ok(Uuid(u128::from_be_bytes(bytes)))
    }

    pub fn write(self, w: &mut Vec<u8>) {
        w.extend_from_slice(&self.0.to_be_bytes());
    }
}

impl std::fmt::Display for Uuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 8-4-4-4-12, the form the client displays.
        let h = format!("{:032x}", self.0);
        write!(
            f,
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }
}

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

/// Writes `ClientboundLoginFinishedPacket`.
///
/// An empty property list is correct rather than a placeholder: the list is a
/// counted collection, and zero is a valid count. The client reads it and finds
/// no textures, which is what an offline player has.
pub fn write_login_finished(
    out: &mut Vec<u8>,
    profile_id: Uuid,
    name: &str,
    session_id: Uuid,
) {
    VarInt(id::LOGIN_FINISHED).write(out);

    profile_id.write(out);

    // The name is capped at 16 upstream.
    VarInt(name.len() as i32).write(out);
    out.extend_from_slice(name.as_bytes());

    // Property count: zero. A `readCount` cap of 16 applies, and zero is under it.
    VarInt(0).write(out);

    session_id.write(out);
}

/// Writes `ClientboundLoginCompressionPacket`.
pub fn write_login_compression(out: &mut Vec<u8>, threshold: i32) {
    VarInt(id::LOGIN_COMPRESSION).write(out);
    VarInt(threshold).write(out);
}

/// The body of a login acknowledgement, which is only its id.
pub fn login_acknowledged_body() -> Vec<u8> {
    VarInt(id::SERVERBOUND_LOGIN_ACKNOWLEDGED).into_raw()
}

/// Extension that turns a VarInt into its own byte representation.
trait IntoRaw {
    fn into_raw(self) -> Vec<u8>;
}

impl IntoRaw for VarInt {
    fn into_raw(self) -> Vec<u8> {
        let mut v = Vec::new();
        self.write(&mut v);
        v
    }
}

#[cfg(test)]
mod tests;