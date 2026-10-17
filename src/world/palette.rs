//! Palette-indirect storage: a [`BitArray`] of palette indices plus the palette.
//!
//! This is the design Minecraft uses for chunk contents, and the same shape is
//! right here. The dense array holds *indices*, never block data, so entry
//! width only has to fit the palette.
//!
//! Two storage variants, matching Minecraft's two useful cases:
//!
//! * [`Storage::Zero`] — every entry is `palette[0]`. Costs **zero bytes** for
//!   the dense part. Underground sections and empty air are almost entirely
//!   this case.
//! * [`Storage::Packed`] — indices in a [`BitArray`].
//!
//! Entry width grows only when the palette outgrows the current width, and each
//! growth doubles capacity, so widening is amortised O(1) per `set`.

use super::bitpack::{BitArray, MAX_BITS};

/// Number of distinct values a palette of `bits` width can address.
const fn capacity_for_bits(bits: u8) -> usize {
    1usize << bits
}

/// Smallest width that can address `palette_len` distinct values.
const fn bits_for_palette_len(palette_len: usize) -> u8 {
    // Branch-free ceil(log2), with 0 for a single value (handled by Zero).
    if palette_len <= 1 {
        return 1;
    }
    let bits = usize::BITS - (palette_len - 1).leading_zeros();
    bits as u8
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Storage {
    Zero,
    Packed(BitArray),
}

/// A fixed-length array of `T` compressed through a palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paletted<T> {
    storage: Storage,
    palette: Vec<T>,
    len: usize,
}

impl<T: Copy + PartialEq> Paletted<T> {
    /// An array of `len` copies of `value`, taking no dense storage at all.
    pub fn uniform(value: T, len: usize) -> Self {
        Self {
            storage: Storage::Zero,
            palette: vec![value],
            len,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Current entry width. `0` while the array is still uniform.
    pub fn bits(&self) -> u8 {
        match &self.storage {
            Storage::Zero => 0,
            Storage::Packed(array) => array.bits(),
        }
    }

    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    pub fn palette(&self) -> &[T] {
        &self.palette
    }

    /// Bytes held by the dense index array. The palette is counted separately.
    pub fn nbytes(&self) -> usize {
        match &self.storage {
            Storage::Zero => 0,
            Storage::Packed(array) => array.nbytes(),
        }
    }

    #[inline]
    pub fn get(&self, index: usize) -> T {
        assert!(index < self.len, "index {index} out of range");
        match &self.storage {
            Storage::Zero => self.palette[0],
            Storage::Packed(array) => self.palette[array.get(index) as usize],
        }
    }

    /// Writes `value`, growing the palette and widening the index array if the
    /// value has not been seen before.
    pub fn set(&mut self, index: usize, value: T) {
        assert!(index < self.len, "index {index} out of range");

        let slot = self.slot_for(value);

        if self.palette.len() == 1 {
            // Single-entry palette: index 0 everywhere, no width needed.
            if let Storage::Packed(array) = &mut self.storage {
                array.set(index, 0);
            }
            return;
        }

        self.ensure_capacity_for_palette();
        match &mut self.storage {
            Storage::Packed(array) => array.set(index, slot as u32),
            // A palette past one entry always forces a packed array, and the
            // single-entry case returned above.
            Storage::Zero => unreachable!("palette grew but storage stayed uniform"),
        }
    }

    /// Overwrites every entry, resetting to the uniform fast path when possible.
    pub fn fill(&mut self, value: T) {
        self.storage = Storage::Zero;
        self.palette.clear();
        self.palette.push(value);
    }

    /// Drops palette entries no longer referenced and tightens entry width.
    ///
    /// Without this the palette only ever grows, so a long session that places
    /// many block types permanently pays for all of them.
    pub fn compact(&mut self) {
        let old = match &self.storage {
            Storage::Zero => return,
            Storage::Packed(array) => array,
        };

        let mut used = vec![false; self.palette.len()];
        for index in 0..self.len {
            used[old.get(index) as usize] = true;
        }

        let mut remap = vec![0usize; self.palette.len()];
        let mut new_palette = Vec::with_capacity(used.iter().filter(|u| **u).count());
        for (slot, is_used) in used.iter().enumerate() {
            if *is_used {
                remap[slot] = new_palette.len();
                new_palette.push(self.palette[slot]);
            }
        }

        let bits = bits_for_palette_len(new_palette.len());

        // Everything collapsed to one value: drop the dense array entirely
        // rather than keeping a 1-bit array that stores nothing but zeroes.
        // Over 24 sections per chunk that is 12 KiB of pure waste.
        if new_palette.len() == 1 {
            self.palette = new_palette;
            self.storage = Storage::Zero;
            return;
        }

        let mut array = BitArray::new(self.len, bits);
        for index in 0..self.len {
            array.set(index, remap[old.get(index) as usize] as u32);
        }

        self.palette = new_palette;
        self.storage = Storage::Packed(array);
    }

    /// Borrows the backing index array, for serialisation.
    pub fn bits_and_words(&self) -> (u8, Vec<u64>) {
        match &self.storage {
            Storage::Zero => (0, Vec::new()),
            Storage::Packed(array) => (array.bits(), array.words().to_vec()),
        }
    }

    /// Rebuilds from a serialised palette and index array.
    pub fn from_parts(palette: Vec<T>, bits: u8, words: Vec<u64>, len: usize) -> Self {
        let mut this = Self {
            storage: Storage::Zero,
            palette,
            len,
        };
        if bits == 0 {
            assert_eq!(
                this.palette.len(),
                1,
                "zero-width form must carry exactly one palette entry"
            );
            return this;
        }
        this.storage = Storage::Packed(BitArray::from_words(words, bits, len));
        this
    }

    /// Iterates values in index order.
    pub fn iter(&self) -> PalettedIter<'_, T> {
        PalettedIter {
            owner: self,
            cursor: 0,
        }
    }

    /// Slot of `value` in the palette, appending if unseen.
    ///
    /// A linear scan is deliberate: palettes are small, and the whole palette
    /// stays in L1 while a hash map would not.
    #[inline]
    fn slot_for(&mut self, value: T) -> usize {
        if let Some(slot) = self.palette.iter().position(|p| *p == value) {
            return slot;
        }

        assert!(
            self.palette.len() < capacity_for_bits(MAX_BITS),
            "palette exhausted at {} entries",
            self.palette.len()
        );

        self.palette.push(value);
        self.palette.len() - 1
    }

    /// Widens the index array when the palette no longer fits the current width.
    ///
    /// `set` calls this after every palette growth, so the bound check here is
    /// what keeps the entry-index invariant.
    fn ensure_capacity_for_palette(&mut self) {
        let needed = bits_for_palette_len(self.palette.len());
        let current = match &self.storage {
            Storage::Zero => 0,
            Storage::Packed(array) => array.bits(),
        };
        if needed <= current {
            return;
        }

        let widened = match std::mem::replace(&mut self.storage, Storage::Zero) {
            Storage::Packed(array) => array.widen(needed),
            Storage::Zero => BitArray::new(self.len, needed),
        };
        self.storage = Storage::Packed(widened);
    }
}

pub struct PalettedIter<'a, T> {
    owner: &'a Paletted<T>,
    cursor: usize,
}

impl<'a, T: Copy + PartialEq> Iterator for PalettedIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.cursor == self.owner.len {
            return None;
        }
        let value = self.owner.get(self.cursor);
        self.cursor += 1;
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.owner.len - self.cursor;
        (remaining, Some(remaining))
    }
}

impl<T: Copy + PartialEq> ExactSizeIterator for PalettedIter<'_, T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_costs_no_dense_storage() {
        let array = Paletted::uniform(7u32, 4096);
        assert_eq!(array.nbytes(), 0);
        assert_eq!(array.bits(), 0);
        assert_eq!(array.palette_len(), 1);
        for i in 0..4096 {
            assert_eq!(array.get(i), 7);
        }
    }

    #[test]
    fn first_difference_materialises_the_array() {
        let mut array = Paletted::uniform(0u32, 64);
        array.set(5, 1);
        assert_eq!(array.bits(), 1);
        assert_eq!(array.nbytes(), 8);
        assert_eq!(array.get(5), 1);
        for i in 0..64 {
            let expected = if i == 5 { 1 } else { 0 };
            assert_eq!(array.get(i), expected, "index {i}");
        }
    }

    #[test]
    fn matches_a_reference_vec_under_random_writes() {
        // Deterministic LCG: same failures reproduce exactly.
        let mut seed = 0x1234_5678u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed >> 16
        };

        for bits_expected in [1u8, 2, 4, 8, 16] {
            let len = 4096;
            let value_count = capacity_for_bits(bits_expected) as u32;
            let mut array = Paletted::uniform(0u32, len);
            let mut reference = vec![0u32; len];

            for _ in 0..20_000 {
                let index = next() as usize % len;
                let value = next() % value_count;
                array.set(index, value);
                reference[index] = value;
            }

            for (i, expected) in reference.iter().enumerate() {
                assert_eq!(array.get(i), *expected, "value_count={value_count} i={i}");
            }
            assert!(
                array.palette_len() <= capacity_for_bits(array.bits()),
                "palette {} exceeds capacity of {} bits",
                array.palette_len(),
                array.bits()
            );
        }
    }

    #[test]
    fn width_grows_only_at_power_of_two_boundaries() {
        let mut array = Paletted::uniform(0u32, 16);

        array.set(0, 1);
        assert_eq!(array.bits(), 1, "palette of 2 fits in 1 bit");

        array.set(0, 2);
        assert_eq!(array.bits(), 2, "palette of 3 needs 2 bits");

        array.set(0, 3);
        assert_eq!(array.bits(), 2, "palette of 4 still fits in 2 bits");

        array.set(0, 4);
        assert_eq!(array.bits(), 3, "palette of 5 needs 3 bits");

        // Width must double exactly when capacity is exhausted, not before.
        for value in 5..8u32 {
            array.set(0, value);
            assert_eq!(array.bits(), 3, "palette of {} fits in 3 bits", value + 1);
        }
        array.set(0, 8);
        assert_eq!(array.bits(), 4, "palette of 9 needs 4 bits");
    }

    #[test]
    fn compact_removes_unused_entries_and_shrinks() {
        let mut array = Paletted::uniform(0u32, 64);
        // Leave index 0 on the last value written, so it is the only entry
        // holding 99.
        for value in 0..100u32 {
            array.set(0, value);
        }
        array.set(1, 0);
        assert!(array.palette_len() >= 100, "palette grew monotonically");

        array.compact();

        assert_eq!(array.palette_len(), 2, "only 0 and 99 are still referenced");
        assert_eq!(array.bits(), 1, "two entries fit in one bit");
        assert_eq!(array.get(0), 99, "index 0 kept its last written value");
        assert_eq!(array.get(1), 0);
        for i in 2..64 {
            assert_eq!(array.get(i), 0, "index {i}");
        }
    }

    #[test]
    fn serialise_round_trips_exactly() {
        let len = 4096;
        let mut array = Paletted::uniform(0u16, len);
        let mut reference = vec![0u16; len];

        let mut seed = 0xABCD_1234u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed >> 20
        };
        for i in 0..len {
            let value = (next() % 40) as u16;
            array.set(i, value);
            *reference.get_mut(i).expect("in range") = value;
        }

        let (bits, words) = array.bits_and_words();
        let palette = array.palette().to_vec();
        assert!(bits > 0, "a mixed section must not serialise as uniform");

        let restored = Paletted::from_parts(palette, bits, words, len);
        assert_eq!(restored.len(), array.len());
        assert_eq!(restored.bits(), array.bits());
        assert_eq!(restored.palette_len(), array.palette_len());
        assert_eq!(restored.nbytes(), array.nbytes());

        for (i, expected) in reference.iter().enumerate() {
            assert_eq!(restored.get(i), *expected, "index {i}");
        }

        // Restored container must still be writable and keep its invariant.
        let mut restored = restored;
        restored.set(0, 1234);
        assert_eq!(restored.get(0), 1234);
        assert!(restored.palette_len() <= capacity_for_bits(restored.bits()));
    }

    #[test]
    fn uniform_section_round_trips_through_the_zero_form() {
        let len = 4096;
        let array = Paletted::uniform(5u16, len);
        let (bits, words) = array.bits_and_words();
        assert_eq!(bits, 0);
        assert!(words.is_empty());

        let restored = Paletted::from_parts(array.palette().to_vec(), bits, words, len);
        assert_eq!(restored.nbytes(), 0);
        for i in 0..len {
            assert_eq!(restored.get(i), 5);
        }
    }
}
