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
///
/// Public because it is the only way back from a stored index array: the array
/// itself records no width, so a decoder has to recover it from the palette.
pub const fn bits_for_palette_len(palette_len: usize) -> u8 {
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
mod tests;
