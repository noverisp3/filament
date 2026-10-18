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
mod tests;