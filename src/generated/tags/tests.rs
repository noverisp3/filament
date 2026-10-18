use super::*;

/// Reads a VarInt the same way the protocol does.
fn varint_at(data: &[u8], mut i: usize) -> (i32, usize) {
    let mut result = 0i32;
    let mut shift = 0u32;
    loop {
        assert!(i < data.len(), "VarInt ran past the end of the payload");
        let b = data[i];
        i += 1;
        result |= ((b & 0x7F) as i32) << shift;
        if b & 0x80 == 0 {
            return (result, i);
        }
        shift += 7;
        assert!(shift <= 35, "VarInt too long");
    }
}

fn string_at(data: &[u8], mut i: usize) -> (&str, usize) {
    let (len, next) = varint_at(data, i);
    assert!(len >= 0);
    let start = next;
    let end = start + len as usize;
    assert!(end <= data.len(), "string ran past the end of the payload");
    i = end;
    (std::str::from_utf8(&data[start..end]).expect("identifiers are utf-8"), i)
}

#[test]
fn the_payload_starts_with_the_registry_count() {
    let (count, after) = varint_at(UPDATE_TAGS_BODY, 0);
    assert_eq!(
        count as usize,
        TAG_COUNTS.len(),
        "the leading count has to match the number of registries recorded"
    );
    assert!(after > 0);
}

#[test]
fn every_registry_and_tag_walks_to_the_end() {
    // The important check: the payload is walked all the way through and
    // lands exactly on its last byte. A capture that stopped early, which
    // is what happened more than once here, leaves a mismatch.
    let data = UPDATE_TAGS_BODY;
    let mut i = 0;

    let (registry_count, next) = varint_at(data, i);
    i = next;

    let mut registries = 0;
    let mut tags = 0;

    for _ in 0..registry_count {
        let (registry, next) = string_at(data, i);
        i = next;

        let (tag_count, next) = varint_at(data, i);
        i = next;

        for _ in 0..tag_count {
            let (_tag, next) = string_at(data, i);
            i = next;

            let (id_count, next) = varint_at(data, i);
            i = next;

            for _ in 0..id_count {
                let (_id, next) = varint_at(data, i);
                i = next;
            }
            tags += 1;
        }

        assert!(
            TAG_COUNTS.iter().any(|(name, _)| *name == registry),
            "unexpected registry in the payload: {registry}"
        );
        registries += 1;
    }

    assert_eq!(
        i,
        data.len(),
        "the walk should end on the last byte, not {i} of {}",
        data.len()
    );
    assert_eq!(registries as usize, TAG_COUNTS.len());
    assert_eq!(tags, TAG_TOTAL);
}

#[test]
fn damage_type_carries_the_tag_the_client_demanded() {
    // The crash this file exists to fix:
    //   Missing tag TagKey[minecraft:damage_type / minecraft:is_fire]
    // So is_fire has to be in here, under the damage_type registry, with
    // eight ids. Pinned against the capture rather than recomputed.
    let data = UPDATE_TAGS_BODY;
    let mut i = 0;

    let (registry_count, next) = varint_at(data, i);
    i = next;

    for _ in 0..registry_count {
        let (registry, next) = string_at(data, i);
        i = next;

        let (tag_count, next) = varint_at(data, i);
        i = next;

        for _ in 0..tag_count {
            let (tag, next) = string_at(data, i);
            i = next;

            let (id_count, next) = varint_at(data, i);
            i = next;

            let mut ids = Vec::with_capacity(id_count.max(0) as usize);
            for _ in 0..id_count {
                let (id, next) = varint_at(data, i);
                i = next;
                ids.push(id);
            }

            if registry == "minecraft:damage_type" && tag == "minecraft:is_fire" {
                assert_eq!(
                    ids,
                    vec![21, 3, 31, 24, 20, 42, 47, 14],
                    "the ids came off the wire, so they are not derivable \
                     from the tag files: alphabetically in_fire is the 18th \
                     entry, not the 21st"
                );
                return;
            }
        }
    }

    panic!("minecraft:is_fire is not in the payload, which is the bug this fixes");
}

#[test]
fn the_payload_is_larger_than_the_compression_threshold() {
    // Worth knowing rather than discovering: at 58 KiB this frame is
    // compressed by FrameEncoder, so it cannot be compared against a raw
    // capture without decompressing it first.
    assert!(
        UPDATE_TAGS_BODY.len() > 256,
        "expected well past the threshold, got {}",
        UPDATE_TAGS_BODY.len()
    );
}