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