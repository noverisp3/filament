//! Fixed-width bit-packed array: `len` logical entries, `bits` bits each.
//!
//! This is the storage primitive under [`crate::world::palette::Paletted`].
//! Every entry is a palette index, so `bits` only has to be wide enough for the
//! palette, never for the block data itself.
//!
//! Values routinely straddle the 64-bit word boundary: entry `i` starts at bit
//! `i * bits`, so unless `bits` divides 64 there is a position where a value
//! spans two words. The tests at the bottom of this file sweep every
//! `(bits, bit position)` combination specifically to pin that down.

/// Number of entries a [`BitArray`] can address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitArray {
    words: Vec<u64>,
    bits: u8,
    len: usize,
}

/// Widest entry supported. 32 keeps `mask_for` and the two-word read path free
/// of 64-bit shift overflow; palette indices never need more than 16 in practice.
pub const MAX_BITS: u8 = 32;

#[inline]
fn mask_for(bits: u8) -> u64 {
    debug_assert!((1..=MAX_BITS).contains(&bits));
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

impl BitArray {
    /// A zero-filled array of `len` entries, `bits` bits each.
    pub fn new(len: usize, bits: u8) -> Self {
        Self {
            words: vec![0; (len * bits as usize).div_ceil(64)],
            bits,
            len,
        }
    }

    /// Rebuilds an array directly from its backing words, for loading from disk.
    pub fn from_words(words: Vec<u64>, bits: u8, len: usize) -> Self {
        let needed = (len * bits as usize).div_ceil(64);
        assert_eq!(
            words.len(),
            needed,
            "word count does not match len={len} bits={bits}"
        );
        Self { words, bits, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bits(&self) -> u8 {
        self.bits
    }

    /// Bytes the backing store occupies.
    pub fn nbytes(&self) -> usize {
        self.words.len() * size_of::<u64>()
    }

    pub fn words(&self) -> &[u64] {
        &self.words
    }

    #[inline]
    pub fn get(&self, index: usize) -> u32 {
        assert!(index < self.len, "index {index} out of range");
        self.read_at(index * self.bits as usize, self.bits)
    }

    #[inline]
    pub fn set(&mut self, index: usize, value: u32) {
        assert!(index < self.len, "index {index} out of range");
        self.write_at(index * self.bits as usize, self.bits, value);
    }

    /// Reads `bits` bits starting at absolute bit position `bit`.
    ///
    /// Split out from [`Self::get`] so tests can aim at an exact bit position
    /// instead of only the positions entry indices happen to land on.
    #[inline]
    fn read_at(&self, bit: usize, bits: u8) -> u32 {
        debug_assert!((1..=MAX_BITS).contains(&bits));
        if bits == 0 {
            return 0;
        }

        let mask = mask_for(bits);
        let word = bit / 64;
        let offset = bit % 64;
        let bits = bits as usize;

        let value = if offset + bits <= 64 {
            (self.words[word] >> offset) & mask
        } else {
            // Straddles: take the high part of this word and the low part of
            // the next. `offset > 0` here because `bits <= 32`, so `64 - offset`
            // is always a valid (non-overflowing) shift.
            debug_assert!(offset > 0);
            (self.words[word] >> offset) | (self.words[word + 1] << (64 - offset))
        };

        (value & mask) as u32
    }

    #[inline]
    fn write_at(&mut self, bit: usize, bits: u8, value: u32) {
        debug_assert!((1..=MAX_BITS).contains(&bits));
        if bits == 0 {
            debug_assert_eq!(value, 0, "zero-width array can only hold zero");
            return;
        }

        let mask = mask_for(bits);
        let word = bit / 64;
        let offset = bit % 64;
        let bits = bits as usize;
        let value = (value as u64) & mask;

        if offset + bits <= 64 {
            self.words[word] = (self.words[word] & !(mask << offset)) | (value << offset);
        } else {
            let low_bits = 64 - offset;
            let low_mask = mask & ((1u64 << low_bits) - 1);
            let high_mask = mask >> low_bits;

            self.words[word] =
                (self.words[word] & !(low_mask << offset)) | ((value & low_mask) << offset);
            self.words[word + 1] = (self.words[word + 1] & !high_mask) | (value >> low_bits);
        }
    }

    /// Reinterprets this array at a wider entry size, preserving values.
    ///
    /// `self` is consumed because callers must not keep a stale view of it.
    pub fn widen(self, new_bits: u8) -> Self {
        assert!(
            new_bits > self.bits,
            "widening must increase width ({} -> {new_bits})",
            self.bits
        );
        assert!(new_bits <= MAX_BITS);

        let old_bits = self.bits;
        let mut widened = BitArray::new(self.len, new_bits);

        if old_bits > 0 {
            for index in 0..self.len {
                let value = self.get(index);
                widened.set(index, value);
            }
        }

        widened
    }
}

#[cfg(test)]
mod tests;
