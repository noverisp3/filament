//! Pins the chunk packet against a capture of a real 26.3 server.
//!
//! The packet was written from the decompiled source, and eight of the fields in
//! the play phase turned out to be wrong when a real client read them: an int
//! where the game sends a VarInt, a bit set encoded as words instead of bytes,
//! four bits where the minimum was two. Every one of them decoded cleanly at
//! this end and failed at the other.
//!
//! So the shapes here are taken from bytes, not from the source. The reference
//! is `captured/play/chunk_reference.bin`, one whole
//! `ClientboundLevelChunkWithLightPacket` recorded from a stock server on
//! 2026-10-17, and it walks to exactly its own length with nothing left over.
//! A layout that disagrees with it disagrees with the game.

use super::chunk::{
    id, write_sections, BLOCK_BITS, BLOCK_ENTRIES, DEFAULT_BIOME_ID, HEIGHTMAP_BITS,
    HEIGHTMAP_COUNT, HEIGHTMAP_ENTRIES, LIGHT_BYTES, SECTIONS,
};
use crate::protocol::VarInt;

/// The captured packet, verbatim.
const REFERENCE: &[u8] = include_bytes!("../../captured/play/chunk_reference.bin");

/// Reads a VarInt at `at`, returning the value and the position after it.
fn varint_at(data: &[u8], at: usize) -> (i32, usize) {
    let mut value = 0i32;
    let mut shift = 0u32;
    let mut i = at;
    loop {
        assert!(i < data.len(), "VarInt ran past the end");
        let b = data[i];
        i += 1;
        value |= ((b & 0x7F) as i32) << shift;
        if b & 0x80 == 0 {
            return (value, i);
        }
        shift += 7;
    }
}

#[test]
fn the_reference_packet_is_the_shape_this_module_writes() {
    let mut i = 0;

    let (id, next) = varint_at(REFERENCE, i);
    assert_eq!(id, id::LEVEL_CHUNK_WITH_LIGHT, "the captured packet is a chunk");
    i = next;

    // Two int32 coordinates, in network order.
    let x = i32::from_be_bytes(REFERENCE[i..i + 4].try_into().unwrap());
    let z = i32::from_be_bytes(REFERENCE[i + 4..i + 8].try_into().unwrap());
    assert!(x != 0 || z != 0, "a real chunk is not the origin, and this one is not");
    i += 8;

    let (maps, next) = varint_at(REFERENCE, i);
    assert_eq!(
        maps,
        HEIGHTMAP_COUNT,
        "three heightmaps, which is what this module sends"
    );
    i = next;

    let mut types = Vec::new();
    for _ in 0..maps {
        let (kind, next) = varint_at(REFERENCE, i);
        types.push(kind);
        i = next;

        let (longs, next) = varint_at(REFERENCE, i);
        i = next;
        assert_eq!(
            longs as usize,
            HEIGHTMAP_ENTRIES.div_ceil(64 / HEIGHTMAP_BITS),
            "thirty-seven longs for nine bit entries: seven to a long"
        );
        i += longs as usize * 8;
    }
    assert_eq!(types, vec![1, 4, 5], "the three the game sends");

    let (buffer_len, next) = varint_at(REFERENCE, i);
    i = next;
    let _ = &REFERENCE[i..i + buffer_len as usize]; // the section buffer, walked below
    i += buffer_len as usize;

    // A section with more than one block is exactly this many bytes: two
    // shorts, a byte of bits, a size, four palette entries, 256 longs, and a
    // two byte biome container at the end. An empty one drops the size and the
    // longs: a single value palette has no count, just the value.
    let multi = 2 + 2 + 1 + 1 + 4 + 256 * 8 + 2;
    let single = 2 + 2 + 1 + 1 + 2;
    assert_eq!(multi, 2060, "a four bit section");
    assert_eq!(single, 8, "a uniform section");
    assert_eq!(
        buffer_len as usize,
        multi + 23 * single,
        "the captured buffer is one populated section and twenty-three empty ones"
    );

    let (entities, next) = varint_at(REFERENCE, i);
    assert_eq!(entities, 0, "a flat world has no block entities");
    i = next;

    // Four bit sets, each a byte count and then that many bytes. One byte is
    // enough for any of the twenty-four sections a chunk has.
    for (name, expected_len) in [("sky", 1), ("block", 0), ("empty sky", 1), ("empty block", 1)] {
        let (len, next) = varint_at(REFERENCE, i);
        i = next;
        assert!(
            len <= 1,
            "the {name} mask is {} bytes; twenty-four bits fit in three and the \
             game sends one, so anything longer is not a section mask",
            len
        );
        assert!(
            len == expected_len || len == 0,
            "the {name} mask is {len} bytes"
        );
        i += len as usize;
    }

    let (layers, next) = varint_at(REFERENCE, i);
    i = next;
    for _ in 0..layers {
        let (len, next) = varint_at(REFERENCE, i);
        i = next;
        assert_eq!(len as usize, LIGHT_BYTES, "a light layer is 2048 bytes");
        i += len as usize;
    }

    let (block_layers, next) = varint_at(REFERENCE, i);
    i = next;
    for _ in 0..block_layers {
        let (len, next) = varint_at(REFERENCE, i);
        i = next;
        i += len as usize;
    }

    assert_eq!(
        i,
        REFERENCE.len(),
        "the walk ends on the last byte, which is the check that the layout \
         above is the game's and not an approximation of it"
    );
}

#[test]
fn the_biome_is_the_one_the_captured_server_used() {
    // Read past the coordinates and the heightmaps, then to the first section's
    // biome container.
    let mut i = 0;
    let (_, next) = varint_at(REFERENCE, i);
    i = next + 8;

    let (maps, next) = varint_at(REFERENCE, i);
    i = next;
    for _ in 0..maps {
        let (_, n1) = varint_at(REFERENCE, i);
        let (longs, n2) = varint_at(REFERENCE, n1);
        i = n2 + longs as usize * 8;
    }

    let (buffer_len, next) = varint_at(REFERENCE, i);
    i = next;
    let buffer = &REFERENCE[i..i + buffer_len as usize];

    // The first section's block states come first, and they are four bits wide.
    let mut j = 4;
    assert_eq!(buffer[j], BLOCK_BITS as u8, "the captured section is four bits");
    j += 1;
    let (size, next) = varint_at(buffer, j);
    assert_eq!(size, 4, "and it has four distinct blocks");
    j = next + 4; // the palette entries
    j += 256 * 8; // the packed data

    assert_eq!(buffer[j], 0, "one biome for the whole section");
    let (biome, _) = varint_at(buffer, j + 1);
    assert_eq!(
        biome,
        DEFAULT_BIOME_ID,
        "forty-one, not the zero that would have been badlands"
    );
}

#[test]
fn a_populated_section_is_the_size_the_game_sends() {
    // Two distinct blocks make a four bit palette of two entries, so the
    // section is two shorts, a bits byte, a size, two palette entries, 256
    // longs, and a two byte biome container. An empty one drops the size and
    // the longs: a single value palette has no count, just the value.
    let populated = 2 + 2 + 1 + 1 + 2 + 256 * 8 + 2;
    let uniform = 2 + 2 + 1 + 1 + 2;
    assert_eq!(populated, 2058);
    assert_eq!(uniform, 8);

    let mut sections = vec![vec![0i32; BLOCK_ENTRIES]; SECTIONS];
    for block in sections[3][..BLOCK_ENTRIES / 4].iter_mut() {
        *block = 1;
    }

    let mut w = Vec::new();
    write_sections(&mut w, &sections);

    let (length, consumed) = VarInt::read(&mut &w[..]).expect("a length");
    let body = &w[consumed..consumed + length.0 as usize];

    assert_eq!(body.len(), populated + 23 * uniform);

    // Three uniform sections, then the populated one.
    let at = 3 * uniform;
    let non_empty = i16::from_be_bytes([body[at], body[at + 1]]);
    assert_eq!(non_empty as usize, BLOCK_ENTRIES / 4, "it counts its blocks");
    assert_eq!(body[at + 4], 4, "four bits an entry");

    let (palette_len, after_len) = varint_at(body, at + 5);
    assert_eq!(palette_len, 2, "two distinct blocks, two palette entries");
    let (first, _) = varint_at(body, after_len);
    assert_eq!(first, 1, "stone fills the section first, so it is palette index zero");
}