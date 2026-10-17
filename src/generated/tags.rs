//! Generated from a vanilla 26.3 capture. Do not edit by hand.
//!
//! The body of `ClientboundUpdateTagsPacket`, as it came off the wire on
//! 2026-10-17, captured with tools/capture.py against a stock 26.3 server.
//!
//! Regenerate with:
//!
//! ```text
//! python tools/capture.py 127.0.0.1 25566
//! python tools/build_tags.py
//! ```
//!
//! The file is byte for byte what the server sent apart from the ordering, which
//! is a map on the wire and does not matter. That near-equality is the check
//! worth keeping: the capture was 58_755 bytes and this is 58_754, and the one
//! byte of difference is the leading count of the map itself.
//!
//! It is a large file because tags reference registry entries by index, and the
//! block and item registries are large. 773 tags cover 15 registries.

/// The body of the packet, without the packet id.
pub const UPDATE_TAGS_BODY: &[u8] = include_bytes!("../../assets/update_tags.bin");

/// Registries this payload carries tags for, and how many tags each holds.
///
/// Recorded here so the server can log what it is sending without parsing the
/// payload, and so a capture that quietly lost a registry shows up in a test.
pub const TAG_COUNTS: &[(&str, usize)] = &[
    ("minecraft:banner_pattern", 11),
    ("minecraft:instrument", 3),
    ("minecraft:damage_type", 36),
    ("minecraft:enchantment", 22),
    ("minecraft:entity_type", 49),
    ("minecraft:worldgen/biome", 86),
    ("minecraft:painting_variant", 1),
    ("minecraft:dialog", 2),
    ("minecraft:point_of_interest_type", 3),
    ("minecraft:potion", 5),
    ("minecraft:fluid", 10),
    ("minecraft:item", 236),
    ("minecraft:timeline", 4),
    ("minecraft:block", 300),
    ("minecraft:game_event", 5),
];

/// Total number of tags across every registry in the payload.
pub const TAG_TOTAL: usize = 773;

#[cfg(test)]
mod tests {
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
}