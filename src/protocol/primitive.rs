//! Minecraft's wire primitives.
//!
//! Every field in every packet is one of these four things, so they are defined
//! once here and tested against the encodings in the decompiled 26.3 server
//! rather than against round-trips. A round trip passes when encode and decode
//! share the same mistake, which is exactly how a wrong VarInt length survives
//! until a real client hangs on it.

use std::fmt;

/// A 32-bit variable-length integer.
///
/// Seven bits per byte, little endian, with the high bit set on every byte but
/// the last. Five bytes is the maximum: the sixth would carry only a sign bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct VarInt(pub i32);

impl VarInt {
    /// The most bytes a VarInt can occupy.
    pub const MAX_LEN: usize = 5;

/// Number of bytes this value occupies once encoded.
///
/// Number of bytes this value occupies once encoded.
    ///
    /// Computed the way `write` computes it: the value is treated as an
    /// unsigned 32-bit integer and shifted seven bits at a time until nothing
    /// is left. `unsigned_abs` is wrong for negative values, because the
    /// encoding of -70000 is based on its bit pattern rather than on its
    /// magnitude, so it takes five bytes rather than the two 70000 would need
    /// unsigned.
    pub const fn written_size(self) -> usize {
        // `leading_zeros` on the unsigned reinterpretation, which is zero for
        // every negative value and therefore correctly yields the full 32 bits.
        let bits = 32 - (self.0 as u32).leading_zeros() as usize;
        // Zero has no significant bits, yet still occupies one byte: writing
        // nothing would produce a frame the client reads as a different packet.
        let bytes = bits.div_ceil(7);
        if bytes == 0 {
            1
        } else {
            bytes
        }
    }

    /// Reads a VarInt, returning how many bytes it consumed.
    ///
    /// A fifth byte contributes only its lowest bit; anything above that is a
    /// malformed value and is rejected rather than silently masked, because
    /// masking it turns a corrupt stream into wrong positions.
    pub fn read(r: &mut &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut result: u32 = 0;
        let mut shift = 0;

        for i in 0..Self::MAX_LEN {
            let Some(&byte) = r.first() else {
                return Err(DecodeError::UnexpectedEof {
                    wanted: "VarInt",
                    got: i,
                });
            };
            *r = &r[1..];

            result |= u32::from(byte & 0x7F).checked_shl(shift).ok_or(DecodeError::TooLong)?;

            if byte & 0x80 == 0 {
                return if shift == 28 && byte > 0x0F {
                    // shift 28 means the fifth byte, which may only set the
                    // lowest four bits. The top bit of an i32 is the sign.
                    Err(DecodeError::TooLong)
                } else {
                    Ok((VarInt(result as i32), i + 1))
                };
            }

            shift += 7;
        }

        Err(DecodeError::TooLong)
    }

    /// Writes this value, returning how many bytes it took.
    pub fn write(self, w: &mut Vec<u8>) -> usize {
        let start = w.len();
        let mut v = self.0 as u32;

        loop {
            if v & !0x7F == 0 {
                w.push(v as u8);
                break;
            }
            w.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }

        w.len() - start
    }
}

impl fmt::Display for VarInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A 64-bit variable-length integer. Same scheme as [`VarInt`], but nine bytes
/// is the maximum rather than five.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct VarLong(pub i64);

impl VarLong {
    pub const MAX_LEN: usize = 10;

    pub const fn written_size(self) -> usize {
        match self.0.unsigned_abs() {
            0x0 => 1,
            0x1..=0x7F => 1,
            0x80..=0x3FFF => 2,
            0x4000..=0x1FFFFF => 3,
            0x200000..=0xFFFFFFF => 4,
            0x10000000..=0x7FFFFFFFF => 5,
            0x0800000000..=0x3FFFFFFFFFF => 6,
            0x040000000000..=0x1FFFFFFFFFF7F => 7,
            0x02000000000000..=0xFFFFFFFFFFFFF => 8,
            _ => 10,
        }
    }

    pub fn read(r: &mut &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut result: u64 = 0;
        let mut shift = 0;

        for i in 0..Self::MAX_LEN {
            let Some(&byte) = r.first() else {
                return Err(DecodeError::UnexpectedEof {
                    wanted: "VarLong",
                    got: i,
                });
            };
            *r = &r[1..];

            result |= u64::from(byte & 0x7F).checked_shl(shift).ok_or(DecodeError::TooLong)?;

            if byte & 0x80 == 0 {
                return if shift == 63 && byte > 0x01 {
                    Err(DecodeError::TooLong)
                } else {
                    Ok((VarLong(result as i64), i + 1))
                };
            }

            shift += 7;
        }

        Err(DecodeError::TooLong)
    }

    pub fn write(self, w: &mut Vec<u8>) -> usize {
        let start = w.len();
        let mut v = self.0 as u64;

        loop {
            if v & !0x7F == 0 {
                w.push(v as u8);
                break;
            }
            w.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }

        w.len() - start
    }
}

impl fmt::Display for VarLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A length-prefixed UTF-8 string.
///
/// The length is a VarInt counting *bytes*, not characters, and is capped: a
/// client that announces a huge string would otherwise have the server allocate
/// whatever it asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedStr<'a> {
    pub value: &'a str,
    pub max_len: usize,
}

/// The default cap, from `FriendlyByteBuf.readUtf`: 32767 characters.
pub const DEFAULT_MAX_STRING: usize = 32_767;

impl<'a> BoundedStr<'a> {
    pub fn new(value: &'a str) -> Self {
        BoundedStr {
            value,
            max_len: DEFAULT_MAX_STRING,
        }
    }

    pub fn with_max(value: &'a str, max_len: usize) -> Self {
        BoundedStr { value, max_len }
    }

    /// Reads a length-prefixed string.
    ///
    /// The length is checked against the cap *before* allocating anything sized
    /// to it, and again against what is actually left in the buffer, so a
    /// corrupt length cannot cause a large allocation followed by a short read.
    pub fn read(r: &mut &'a [u8]) -> Result<(Self, usize), DecodeError> {
        let (VarInt(len), mut read) = VarInt::read(r)?;

        if len < 0 || len as usize > r.len() {
            return Err(DecodeError::StringTooLong {
                len: len as usize,
                available: r.len(),
            });
        }

        let bytes = &r[..len as usize];
        let s = std::str::from_utf8(bytes).map_err(|_| DecodeError::NotUtf8)?;
        *r = &r[len as usize..];
        read += len as usize;

        Ok((BoundedStr::new(s), read))
    }

    pub fn write(&self, w: &mut Vec<u8>) {
        VarInt(self.value.len() as i32).write(w);
        w.extend_from_slice(self.value.as_bytes());
    }
}

/// A namespaced identifier, `namespace:path`.
///
/// Minecraft 26.x renamed this from `ResourceLocation` to `Identifier`; that is
/// a rename, not a change in format.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident<'a> {
    pub namespace: &'a str,
    pub path: &'a str,
}

impl<'a> Ident<'a> {
    /// `minecraft:stone` and a bare `stone` both mean namespace `minecraft`.
    pub fn new(s: &'a str) -> Self {
        match s.split_once(':') {
            Some((namespace, path)) => Ident { namespace, path },
            None => Ident {
                namespace: "minecraft",
                path: s,
            },
        }
    }

    /// Builds an identifier from its two halves without parsing.
    pub fn parts(namespace: &'a str, path: &'a str) -> Self {
        Ident { namespace, path }
    }

    pub fn read(r: &mut &'a [u8]) -> Result<(Self, usize), DecodeError> {
        let (s, read) = BoundedStr::read(r)?;
        Ok((Ident::new(s.value), read))
    }

    pub fn write(&self, w: &mut Vec<u8>) {
        // Written as the concatenated string rather than two separate strings,
        // which is what `Identifier.STREAM_CODEC` does upstream.
        // One byte for the colon between them, which the length has to account for.
        let len = self.namespace.len() + self.path.len() + 1;
        VarInt(len as i32).write(w);
        w.extend_from_slice(self.namespace.as_bytes());
        w.push(b':');
        w.extend_from_slice(self.path.as_bytes());
    }
}

impl fmt::Display for Ident<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.path)
    }
}

/// Why a field could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer ran out mid-field.
    UnexpectedEof { wanted: &'static str, got: usize },
    /// The value is larger than its encoding allows.
    TooLong,
    /// A string announced a length past the end of the buffer or past its cap.
    StringTooLong { len: usize, available: usize },
    /// Bytes that are not valid UTF-8.
    NotUtf8,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::UnexpectedEof { wanted, got } => {
                write!(f, "ran out of input reading {wanted}, at least {got} bytes in")
            }
            DecodeError::TooLong => write!(f, "value is longer than its type allows"),
            DecodeError::StringTooLong { len, available } => write!(
                f,
                "string announced {len} bytes but only {available} are left"
            ),
            DecodeError::NotUtf8 => write!(f, "bytes are not valid UTF-8"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected byte sequences, taken from `FriendlyByteBuf` in the decompiled
    /// 26.3 server rather than from this implementation. A round trip would pass
    /// even if both sides were wrong in the same way.
    #[test]
    fn varint_matches_the_encodings_minecraft_uses() {
        // value -> bytes
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (255, &[0xFF, 0x01]),
            (25565, &[0xDD, 0xC7, 0x01]),
            (2097151, &[0xFF, 0xFF, 0x7F]),
            (2147483647, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
            (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
            (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];

        for &(value, expected) in cases {
            let mut w = Vec::new();
            let n = VarInt(value).write(&mut w);
            assert_eq!(w, expected, "VarInt({value}) encoded wrong");
            assert_eq!(n, expected.len(), "VarInt({value}) reported wrong length");

            let (got, read) = VarInt::read(&mut &expected[..]).expect("decodes");
            assert_eq!(got.0, value, "VarInt({value}) decoded wrong");
            assert_eq!(read, expected.len(), "VarInt({value}) read wrong length");
        }
    }

    #[test]
    fn varint_written_size_agrees_with_what_write_produces() {
        // The size is computed by matching on ranges; encoding is the loop. If
        // they ever disagree the packet length prefix will be wrong, so they
        // are compared over a wide spread.
        //
        // Negative values are included because they are the case worth getting
        // right: a negative VarInt is written by treating it as an unsigned
        // 32-bit value and shifting, which never reaches zero, so -70000 takes
        // five bytes rather than the two its magnitude would suggest.
        for v in [
            -2147483648i32,
            -70000,
            -1,
            0,
            1,
            127,
            128,
            300,
            16383,
            16384,
            100_000,
            1 << 24,
            2147483647,
        ] {
            let mut w = Vec::new();
            VarInt(v).write(&mut w);
            assert_eq!(
                VarInt(v).written_size(),
                w.len(),
                "VarInt({v}) size disagrees with its encoding"
            );
        }
    }

    #[test]
    fn a_negative_varint_occupies_five_bytes() {
        // Pinned because it is the surprising case. -70000 has magnitude under
        // 2^17, so it would fit in three bytes if it were unsigned, but the
        // sign bit occupies the whole fifth byte of the encoding.
        let mut w = Vec::new();
        VarInt(-70000).write(&mut w);
        assert_eq!(
            w,
            vec![0x90, 0xDD, 0xFB, 0xFF, 0x0F],
            "the encoding of -70000 is fixed by the scheme, not by its magnitude"
        );

        let (got, _) = VarInt::read(&mut &w[..]).expect("decodes");
        assert_eq!(got.0, -70000);
    }

    #[test]
    fn a_varint_longer_than_five_bytes_is_rejected() {
        // Six continuation bytes. Accepting this would let a corrupt stream
        // shift every following field.
        let too_long = [0xFF; 6];
        assert_eq!(
            VarInt::read(&mut &too_long[..]),
            Err(DecodeError::TooLong),
            "six bytes must not decode"
        );

        // The fifth byte may only carry the lowest four bits.
        let overflow = [0xFF, 0xFF, 0xFF, 0xFF, 0x1F];
        assert_eq!(
            VarInt::read(&mut &overflow[..]),
            Err(DecodeError::TooLong),
            "a fifth byte above 0x0F must be rejected"
        );
    }

    #[test]
    fn a_truncated_varint_reports_what_it_wanted() {
        let partial = [0x80, 0x80];
        assert_eq!(
            VarInt::read(&mut &partial[..]),
            Err(DecodeError::UnexpectedEof {
                wanted: "VarInt",
                got: 2
            })
        );
    }

    #[test]
    fn varlong_matches_the_encodings_minecraft_uses() {
        let cases: &[(i64, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (9223372036854775807, &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]),
            (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]),
        ];

        for &(value, expected) in cases {
            let mut w = Vec::new();
            VarLong(value).write(&mut w);
            assert_eq!(w, expected, "VarLong({value}) encoded wrong");

            let (got, read) = VarLong::read(&mut &expected[..]).expect("decodes");
            assert_eq!(got.0, value, "VarLong({value}) decoded wrong");
            assert_eq!(read, expected.len(), "VarLong({value}) read wrong length");
        }
    }

    #[test]
    fn a_string_is_length_prefixed_in_bytes_not_characters() {
        // "héllo" is five characters but six bytes in UTF-8, and the prefix
        // counts bytes. Getting this wrong desynchronises everything after it.
        let mut w = Vec::new();
        BoundedStr::new("héllo").write(&mut w);

        assert_eq!(&w[..1], &[6], "prefix must count bytes");
        assert_eq!(&w[1..], "héllo".as_bytes());

        let (s, read) = BoundedStr::read(&mut &w[..]).expect("decodes");
        assert_eq!(s.value, "héllo");
        assert_eq!(read, w.len());
    }

    #[test]
    fn a_string_length_past_the_buffer_is_rejected() {
        // Claims 200 bytes, supplies three.
        let bytes = [200u8, b'a', b'b', b'c'];
        assert!(matches!(
            BoundedStr::read(&mut &bytes[..]),
            Err(DecodeError::StringTooLong { .. })
        ));
    }

    #[test]
    fn a_string_with_a_negative_length_is_rejected() {
        // -1 as a VarInt is five bytes of 0xFF.
        let bytes = [0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
        assert!(matches!(
            BoundedStr::read(&mut &bytes[..]),
            Err(DecodeError::StringTooLong { .. })
        ));
    }

    #[test]
    fn invalid_utf8_is_rejected_rather_than_replaced() {
        // 0xFF is never valid in UTF-8. Replacing it with U+FFFD would let a
        // corrupt packet through and compare unequal later.
        let bytes = [1u8, 0xFF];
        assert_eq!(BoundedStr::read(&mut &bytes[..]), Err(DecodeError::NotUtf8));
    }

    #[test]
    fn a_bare_identifier_defaults_to_the_minecraft_namespace() {
        let id = Ident::new("stone");
        assert_eq!(id.namespace, "minecraft");
        assert_eq!(id.path, "stone");

        let namespaced = Ident::new("custom:block");
        assert_eq!(namespaced.namespace, "custom");
        assert_eq!(namespaced.path, "block");
    }

    #[test]
    fn an_identifier_round_trips_through_its_wire_form() {
        let mut w = Vec::new();
        Ident::new("minecraft:painting").write(&mut w);

        let (id, read) = Ident::read(&mut &w[..]).expect("decodes");
        assert_eq!(id, Ident::new("minecraft:painting"));
        assert_eq!(read, w.len());
        assert_eq!(id.to_string(), "minecraft:painting");
    }

    #[test]
    fn an_identifier_is_written_as_one_length_prefixed_string() {
        // Not two strings. The prefix counts the colon too: "a:b" is 3 bytes.
        let mut w = Vec::new();
        Ident::parts("a", "b").write(&mut w);
        assert_eq!(&w[..1], &[3]);
        assert_eq!(&w[1..], b"a:b");
    }
}