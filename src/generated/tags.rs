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
mod tests;