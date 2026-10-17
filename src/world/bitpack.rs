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
mod tests {
    use super::*;

    /// Sweeps every `(width, start bit)` pair and writes values that are the
    /// whole field, the whole field inverted, and a single low bit.
    ///
    /// The array is pre-filled with all-ones, so a missing mask on either side
    /// of a straddling value shows up immediately as a read-back mismatch.
    #[test]
    fn every_width_and_bit_position_round_trips() {
        for bits in 1..=16u8 {
            let mask = mask_for(bits) as u32;
            let values = [mask, 1, mask ^ 1];

            for bit_pos in 0..64usize {
                for value in values {
                    let mut array = BitArray {
                        words: vec![u64::MAX; 4],
                        bits,
                        len: 0,
                    };

                    array.write_at(bit_pos, bits, value);

                    assert_eq!(
                        array.read_at(bit_pos, bits),
                        value,
                        "bits={bits} bit_pos={bit_pos} value={value:#x}"
                    );
                }
            }
        }
    }

    /// Writing one entry must not disturb its neighbours, including across the
    /// word boundary.
    #[test]
    fn writing_leaves_neighbours_intact() {
        for bits in 1..=16u8 {
            let mask = mask_for(bits) as u32;
            for index in 0..200usize {
                let mut array = BitArray::new(200, bits);
                for i in 0..200 {
                    array.set(i, (i as u32).wrapping_mul(2_654_435_761) & mask);
                }

                let original = array.get(index);
                let replacement = original ^ 1;
                array.set(index, replacement);
                assert_eq!(array.get(index), replacement);

                for other in 0..200 {
                    if other == index {
                        continue;
                    }
                    let expected = (other as u32).wrapping_mul(2_654_435_761) & mask;
                    assert_eq!(array.get(other), expected, "bits={bits} index={other}");
                }
            }
        }
    }

    /// The real access path: entry `i` lives at bit `i * bits`, so indices
    /// naturally cover many different word alignments.
    #[test]
    fn entry_access_matches_reference_vec() {
        for bits in 1..=16u8 {
            let len = 4096;
            let mask = mask_for(bits) as u32;
            let reference: Vec<u32> = (0..len)
                .map(|i| ((i as u32).wrapping_mul(2_246_822_519) >> 3) & mask)
                .collect();

            let mut array = BitArray::new(len, bits);
            for (i, value) in reference.iter().enumerate() {
                array.set(i, *value);
            }
            for (i, value) in reference.iter().enumerate() {
                assert_eq!(array.get(i), *value, "bits={bits} index={i}");
            }

            assert_eq!(array.nbytes(), (len * bits as usize).div_ceil(8));
            assert_eq!(array.words().len(), (len * bits as usize).div_ceil(64));
        }
    }

    #[test]
    fn widening_preserves_values() {
        for from in 1..=12u8 {
            for to in from + 1..=20u8 {
                let len = 512;
                let mask = mask_for(from) as u32;
                let mut array = BitArray::new(len, from);
                let expected: Vec<u32> = (0..len).map(|i| (i as u32 * 37) & mask).collect();
                for (i, value) in expected.iter().enumerate() {
                    array.set(i, *value);
                }

                let widened = array.widen(to);
                assert_eq!(widened.bits(), to);
                for (i, value) in expected.iter().enumerate() {
                    assert_eq!(widened.get(i), *value, "from={from} to={to} index={i}");
                }
            }
        }
    }

    #[test]
    fn from_words_rejects_mismatched_length() {
        let array = BitArray::new(64, 5);
        let words = array.words().to_vec();
        assert_eq!(
            BitArray::from_words(words.clone(), 5, 64).words(),
            words.as_slice()
        );

        // 70 entries at 5 bits needs 6 words, not 5.
        assert!(std::panic::catch_unwind(move || BitArray::from_words(words, 5, 70)).is_err());
    }
}
