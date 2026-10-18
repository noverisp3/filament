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
mod tests {
    use super::*;

    #[test]
    fn an_offline_uuid_is_stable_for_a_name() {
        // The client caches the identity between sessions, so a UUID that
        // changes between connections shows up as the player's skin resetting.
        let a = Uuid::offline_for("Notch");
        let b = Uuid::offline_for("Notch");
        assert_eq!(a, b);

        assert_ne!(a, Uuid::offline_for("Herobrine"));
    }

    #[test]
    fn an_offline_uuid_is_version_three_and_rfc_4122() {
        // Name-based UUIDs are version 3, and Mojang stamps them the same way.
        // A server that gets this wrong is rejected by strict clients.
        let uuid = Uuid::offline_for("Notch");

        let bytes = uuid.0.to_be_bytes();
        assert_eq!(bytes[6] >> 4, 3, "version nibble must be 3");
        assert_eq!(bytes[8] >> 6, 0b10, "variant must be IETF");
    }

    #[test]
    fn an_offline_uuid_matches_the_value_mojang_produces() {
        // Pinned so a change to the hashing shows up here rather than as a
        // player appearing to be someone else.
        assert_eq!(
            Uuid::offline_for("Notch").to_string(),
            // Verified against `hashlib.md5` independently: the first 20 hex
            // digits are the MD5 of "OfflinePlayer:Notch" and the rest follow
            // from the version and variant bits being stamped in. This is
            // Mojang's value, so the same name reads as the same player here as
            // on any other offline-mode server.
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn a_uuid_is_written_as_two_big_endian_halves() {
        let uuid = Uuid(0x0123456789abcdef_fedcba9876543210);
        let mut w = Vec::new();
        uuid.write(&mut w);

        assert_eq!(w.len(), 16);
        assert_eq!(w[0], 0x01, "the high half is written most significant first");
        assert_eq!(w[15], 0x10);

        let got = Uuid::read(&mut &w[..]).expect("decodes");
        assert_eq!(got, uuid);
    }

    #[test]
    fn a_hello_is_read_with_its_id() {
        let mut w = Vec::new();
        VarInt(id::SERVERBOUND_HELLO).write(&mut w);
        VarInt(5).write(&mut w);
        w.extend_from_slice(b"Notch");
        Uuid(7).write(&mut w);

        let (name, uuid) = read_hello(&w).expect("decodes");
        assert_eq!(name, "Notch");
        assert_eq!(uuid, Uuid(7));
    }

    #[test]
    fn a_hello_with_the_wrong_id_is_refused() {
        let mut w = Vec::new();
        VarInt(id::SERVERBOUND_KEY).write(&mut w);
        VarInt(5).write(&mut w);
        w.extend_from_slice(b"Notch");
        Uuid(7).write(&mut w);

        assert!(read_hello(&w).is_err());
    }

    #[test]
    fn a_login_finished_has_the_shape_the_client_reads() {
        let mut w = Vec::new();
        write_login_finished(&mut w, Uuid(1), "Notch", Uuid(2));

        let mut r = &w[..];
        let (VarInt(packet), _) = VarInt::read(&mut r).expect("id");
        assert_eq!(packet, id::LOGIN_FINISHED);

        assert_eq!(Uuid::read(&mut r).expect("profile id"), Uuid(1));

        let (VarInt(name_len), _) = VarInt::read(&mut r).expect("name length");
        let name = std::str::from_utf8(&r[..name_len as usize]).expect("utf8");
        assert_eq!(name, "Notch");
        r = &r[name_len as usize..];

        let (VarInt(properties), _) = VarInt::read(&mut r).expect("property count");
        assert_eq!(properties, 0, "an offline player has no profile properties");

        assert_eq!(Uuid::read(&mut r).expect("session id"), Uuid(2));
        assert!(r.is_empty(), "the packet has no trailing bytes");
    }

    #[test]
    fn login_compression_is_a_single_varint() {
        let mut w = Vec::new();
        write_login_compression(&mut w, 256);

        let mut r = &w[..];
        let (VarInt(packet), _) = VarInt::read(&mut r).expect("id");
        assert_eq!(packet, id::LOGIN_COMPRESSION);

        let (VarInt(threshold), _) = VarInt::read(&mut r).expect("threshold");
        assert_eq!(threshold, 256);
        assert!(r.is_empty());
    }

    #[test]
    fn the_acknowledgement_is_only_its_id() {
        let body = login_acknowledged_body();
        assert_eq!(body, vec![id::SERVERBOUND_LOGIN_ACKNOWLEDGED as u8]);
    }
}