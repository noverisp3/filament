//! The player's identity, which offline mode derives from the name.

use crate::protocol::DecodeError;

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
    /// conversion at every call site.
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
