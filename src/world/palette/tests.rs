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