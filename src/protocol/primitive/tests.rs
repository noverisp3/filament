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