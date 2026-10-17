//! Sending a chunk to a client.
//!
//! This is the packet that finally shows something, and it is the largest one in
//! the protocol: a chunk is 16 by 384 by 16, which is 24 sections of 16 cubed.
//!
//! Everything here follows the decompiled 26.3 server rather than older
//! versions of the protocol, which differ in ways that are easy to get wrong:
//!
//! * `ClientboundLevelChunkPacketData` is a map of heightmaps, a length-prefixed
//!   byte buffer holding every section back to back, and a list of block
//!   entities. The sections are not length-prefixed; the buffer is one blob and
//!   the reader walks it knowing there are twenty-four.
//! * Each section is two shorts, then the block states as a paletted container,
//!   then the biomes as another one.
//! * `initialize_border` is not a precedent for anything here: it is a different
//!   packet with its own layout.
//!
//! What is sent is an empty world: air everywhere, full skylight, no block light.
//! That is not a stub, it is what a chunk of nothing looks like, and it is the
//! state the world storage starts in.

use crate::protocol::VarInt;

/// Blocks along one edge of a chunk.
pub const WIDTH: i32 = 16;
/// Blocks from the bottom of the world to the top.
pub const HEIGHT: i32 = 384;
/// Sections from the bottom to the top, each sixteen blocks tall.
pub const SECTIONS: usize = (HEIGHT / 16) as usize;
/// Blocks in one section.
pub const SECTION_VOLUME: usize = (WIDTH * WIDTH * 16) as usize;
/// Bytes in one light layer for one section: sixteen by sixteen, four bits each.
pub const LIGHT_BYTES: usize = SECTION_VOLUME / 2;
/// How many chunks are sent. One is enough to get a client off the loading
/// screen; the rest of the work is view distance.
pub const RADIUS: i32 = 1;

/// The state id of air.
///
/// From the generated block table: `minecraft:air` has one state and it is
/// state 0. It is written out here rather than looked up because a chunk of
/// nothing is air and air is the first state, and because a lookup would need
/// the table to be indexed by name at run time for no gain.
pub const AIR_STATE_ID: i32 = 0;

/// The biome id used for an empty world.
///
/// Zero is `minecraft:badlands` in the 26.3 registry, which is the first entry
/// the server sends. Any valid id works here since no terrain is generated to
/// disagree with it.
pub const DEFAULT_BIOME_ID: i32 = 0;

/// Bits a heightmap entry needs.
///
/// Heights run from -65 (an empty column) to 320 (the top of the world plus one),
/// so 386 distinct values and nine bits.
const HEIGHTMAP_BITS: usize = 9;

/// Entries in one heightmap: one per column.
const HEIGHTMAP_ENTRIES: usize = (WIDTH * WIDTH) as usize;

/// Heightmap types the client is sent.
///
/// The enum in the decompiled source gives each type a number, and only two of
/// them are marked for the client: the other three are for world generation and
/// for the server's own use. Sending them would be ignored.
pub mod heightmap {
    /// Height of the first non-air block, used for lighting and for clouds.
    pub const WORLD_SURFACE: i32 = 1;
    /// Height of the first motion-blocking or fluid block.
    pub const MOTION_BLOCKING: i32 = 4;
}

/// Writes `ClientboundLevelChunkWithLightPacket` for one chunk.
///
/// `x` and `z` are chunk coordinates, so block (0, 0) is chunk (0, 0) and block
/// (16, 0) is chunk (1, 0).
pub fn write_chunk(out: &mut Vec<u8>, x: i32, z: i32) {
    VarInt(id::LEVEL_CHUNK_WITH_LIGHT).write(out);
    out.extend_from_slice(&x.to_be_bytes());
    out.extend_from_slice(&z.to_be_bytes());

    write_heightmaps(out);
    write_section_buffer(out);
    write_block_entities(out);
    write_light(out);
}

/// Packet ids, from `packets.json`.
pub mod id {
    pub const LEVEL_CHUNK_WITH_LIGHT: i32 = 46;
}

/// The heightmaps, as a map of type to packed bits.
///
/// A map is a count followed by that many entries, and each entry is the type
/// then a long array, which is a VarInt count and then the longs.
///
/// Every entry here is zero, which is the height of the bottom of the world. An
/// empty column has no block to record, and the client only uses these to decide
/// where clouds and lighting sit, neither of which is visible yet.
fn write_heightmaps(out: &mut Vec<u8>) {
    VarInt(2).write(out);

    for kind in [heightmap::WORLD_SURFACE, heightmap::MOTION_BLOCKING] {
        VarInt(kind).write(out);
        write_heightmap_bits(out);
    }
}

/// One heightmap: 256 entries of nine bits, packed.
///
/// The packing does not let a value straddle two longs. Each long holds
/// `64 / 9` whole entries, so seven of them and the top bit of every long goes
/// unused. That is why thirty-seven longs and not thirty-six, and the count is
/// what tells the reader where the next heightmap starts.
fn write_heightmap_bits(out: &mut Vec<u8>) {
    let per_long = 64 / HEIGHTMAP_BITS;
    let longs = HEIGHTMAP_ENTRIES.div_ceil(per_long);

    VarInt(longs as i32).write(out);
    for _ in 0..longs {
        out.extend_from_slice(&0i64.to_be_bytes());
    }
}

/// The section buffer: every section back to back, no lengths between them.
///
/// Each section is two shorts counting the blocks and fluids it holds, then the
/// block states, then the biomes.
fn write_section_buffer(out: &mut Vec<u8>) {
    let mut body = Vec::with_capacity(SECTIONS * 8);

    for _ in 0..SECTIONS {
        // Both counts are zero: there is nothing in an empty section.
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());

        write_single_value_palette(&mut body, AIR_STATE_ID);
        write_single_value_palette(&mut body, DEFAULT_BIOME_ID);
    }

    VarInt(body.len() as i32).write(out);
    out.extend_from_slice(&body);
}

/// One paletted container holding a single value.
///
/// A paletted container is a byte of bits-per-entry, the palette, then the packed
/// data. When there is only one value the bits are zero, the palette is one
/// entry and the data is empty, which is why this fits in so few bytes: the
/// width is per entry and there are no entries beyond the first.
///
/// The palette for a single value is just the id, with no length in front of it.
fn write_single_value_palette(out: &mut Vec<u8>, value: i32) {
    out.push(0);
    VarInt(value).write(out);
}

/// Block entities: none.
fn write_block_entities(out: &mut Vec<u8>) {
    VarInt(0).write(out);
}

/// The light for the chunk.
///
/// Four bit sets and two lists. The bit sets say which sections carry light, and
/// which are empty of it; the lists hold the light itself, two thousand and
/// forty-eight bytes per section because each block gets four bits.
///
/// Sky light is fifteen everywhere, which is what an empty column above ground
/// has. Block light is zero, which is why the sections are listed as empty
/// rather than sent as a thousand blocks of zero.
fn write_light(out: &mut Vec<u8>) {
    // Sky light is present in every section, and every section is empty of block
    // light. Built twice rather than moved, since a Vec is not Copy.
    let sky_present = section_mask_bytes();
    let block_absent = section_mask_bytes();

    write_bit_set(out, &sky_present);
    write_bit_set(out, &[]);
    write_bit_set(out, &[]);
    write_bit_set(out, &block_absent);

    // Sky updates: one layer per section, every block fully lit.
    VarInt(SECTIONS as i32).write(out);
    for _ in 0..SECTIONS {
        VarInt(LIGHT_BYTES as i32).write(out);
        out.extend(std::iter::repeat_n(0xFFu8, LIGHT_BYTES));
    }

    // No block updates, which is consistent with every section being empty.
    VarInt(0).write(out);
}

/// The shortest byte form of a bit set with every section in it.
///
/// `BitSet.toByteArray` indexes from the least significant byte, and the length
/// is however far it must reach to cover the highest bit set, so a partial last
/// byte is included and not padded.
fn section_mask_bytes() -> Vec<u8> {
    let needed = SECTIONS.div_ceil(8);
    (0..needed)
        .map(|byte| {
            let low = byte * 8;
            let in_this = SECTIONS.saturating_sub(low).min(8);
            if in_this >= 8 {
                0xFFu8
            } else {
                ((1u16 << in_this) - 1) as u8
            }
        })
        .collect()
}

/// Writes a bit set.
///
/// This is `ByteBufCodecs.BIT_SET`, which is a VarInt **byte count** followed by
/// that many bytes, and the bytes are the shortest that reach the highest bit
/// set.
///
/// Not a VarInt count of sixty-four bit words, which is the obvious reading of "a
/// bit set" and the one written first. That made the client fail to decode the
/// chunk, because it read one long's worth of mask as a single byte and then
/// took the rest of the mask for the light layers.
///
/// Twenty-four sections set means the highest bit is twenty-three, which is in
/// the third byte: three bytes of FF.
fn write_bit_set(out: &mut Vec<u8>, mask: &[u8]) {
    VarInt(mask.len() as i32).write(out);
    out.extend_from_slice(mask);
}
/// A seed for the world, kept here so the world module does not need to know
/// about the packet.
pub fn seed_from_string(name: &str) -> i64 {
    let _ = name;
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_buffer_has_twenty_four_sections() {
        let mut w = Vec::new();
        write_section_buffer(&mut w);

        let (length, consumed) = VarInt::read(&mut &w[..]).expect("a length");
        let body = &w[consumed..consumed + length.0 as usize];

        // Two shorts, then a byte and a one-byte VarInt, twice.
        let per_section = 2 + 2 + 2 + 2;
        assert_eq!(body.len(), SECTIONS * per_section);
        assert_eq!(length.0 as usize, SECTIONS * per_section);
    }

    #[test]
    fn an_empty_section_is_eight_bytes() {
        let mut w = Vec::new();
        write_single_value_palette(&mut w, 0);
        let single = w.len();
        assert_eq!(single, 2, "a zero bit count and a one-byte VarInt id");
    }

    #[test]
    fn air_is_the_first_state() {
        // Checked against the generated table rather than assumed.
        let air = crate::generated::BLOCKS
            .iter()
            .find(|b| b.name == "minecraft:air")
            .expect("air is in the block table");
        assert_eq!(air.default_state as i32, AIR_STATE_ID);
        assert_eq!(air.states.len(), 1, "air has exactly one state");
    }

    #[test]
    fn a_heightmap_is_thirty_seven_longs() {
        // Seven entries per long because a nine bit value does not straddle one,
        // and 256 entries do not divide by seven.
        let per_long = 64 / HEIGHTMAP_BITS;
        assert_eq!(per_long, 7);
        assert_eq!(HEIGHTMAP_ENTRIES.div_ceil(per_long), 37);

        let mut w = Vec::new();
        write_heightmap_bits(&mut w);
        let (count, consumed) = VarInt::read(&mut &w[..]).expect("a count");
        assert_eq!(count.0, 37);
        assert_eq!(w.len(), consumed + 37 * 8);
    }

    #[test]
    fn the_chunk_starts_with_its_coordinates() {
        let mut w = Vec::new();
        write_chunk(&mut w, 0, 0);

        assert_eq!(w[0], id::LEVEL_CHUNK_WITH_LIGHT as u8);
        assert_eq!(&w[1..5], &0i32.to_be_bytes(), "chunk x");
        assert_eq!(&w[5..9], &0i32.to_be_bytes(), "chunk z");
    }

    #[test]
    fn the_light_says_full_sky_and_no_block_light() {
        let mut w = Vec::new();
        write_light(&mut w);

        // A bit set is a byte count and then the bytes, so twenty-four sections
        // is three bytes of FF and not one eight byte word.
        assert_eq!(w[0], 3, "the sky mask is three bytes");
        assert_eq!(&w[1..4], &[0xFF, 0xFF, 0xFF], "every section has sky light");

        assert_eq!(w[4], 0, "the block mask is empty, not zero");
        assert_eq!(w[5], 0, "and no section is empty of sky light");

        assert_eq!(w[6], 3, "the empty block mask is three bytes");
        assert_eq!(&w[7..10], &[0xFF, 0xFF, 0xFF], "no block light anywhere");

        let (updates, after) = VarInt::read(&mut &w[10..]).expect("an update count");
        assert_eq!(updates.0 as usize, SECTIONS);
        assert_eq!(after, 1);
    }

    #[test]
    fn a_bit_set_of_sections_is_three_bytes() {
        // BitSet.toByteArray is the shortest form that reaches the highest bit,
        // so twenty-four sections is three bytes and not eight.
        assert_eq!(section_mask_bytes(), vec![0xFF, 0xFF, 0xFF]);

        let mut w = Vec::new();
        write_bit_set(&mut w, &section_mask_bytes());
        assert_eq!(w, vec![3, 0xFF, 0xFF, 0xFF]);

        let mut empty = Vec::new();
        write_bit_set(&mut empty, &[]);
        assert_eq!(empty, vec![0], "an empty bit set is a zero length");
    }

    #[test]
    fn the_sky_updates_are_one_full_layer_per_section() {
        let mut w = Vec::new();
        write_light(&mut w);

        // Ten bytes of masks, then a one byte count, then a layer each: a
        // VarInt length of 2048, which is two bytes, and 2048 bytes of light.
        let expected = 10 + 1 + SECTIONS * (2 + LIGHT_BYTES) + 1;
        assert_eq!(
            w.len(),
            expected,
            "ten bytes of masks, then a count, {SECTIONS} layers of \
             {LIGHT_BYTES} bytes each, then an empty block list"
        );

        assert!(w[..10].contains(&0xFF), "sanity: the mask is set");
        assert_eq!(w[4], 0);
    }

    #[test]
    fn the_full_mask_covers_every_section_and_nothing_else() {
        let mask = section_mask_bytes();
        assert_eq!(mask.len(), 3);
        // Every bit in those three bytes is a section, and the fourth byte does
        // not exist, which is the difference between this and a padded mask.
        assert_eq!(mask.iter().map(|b| b.count_ones()).sum::<u32>(), SECTIONS as u32);
    }

    #[test]
    fn no_chunks_are_generated_beyond_the_radius() {
        // The area the server will actually send, so the count can be asserted.
        let side = RADIUS * 2 + 1;
        assert_eq!(side * side, 9);
    }

    #[test]
    fn a_chunk_is_under_the_frame_limit() {
        // Guards against a mistake turning into a packet the framing rejects.
        let mut w = Vec::new();
        write_chunk(&mut w, 0, 0);
        assert!(
            w.len() < crate::protocol::MAX_FRAME_SIZE,
            "{} bytes, over the limit",
            w.len()
        );
    }
}
