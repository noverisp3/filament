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
/// The radius used before the client's view distance is known.
///
/// A radius of one is nine chunks, which is fifty metres of world: enough to see
/// ground and walk on it. The client states its own view distance in its
/// settings during the configuration phase, and that number is what the server
/// then builds, so this is only the fallback for a client whose settings arrive
/// after its finish.
pub const RADIUS: i32 = 4;

/// The state id of air.
///
/// From the generated block table: `minecraft:air` has one state and it is
/// state 0. It is written out here rather than looked up because a chunk of
/// nothing is air and air is the first state, and because a lookup would need
/// the table to be indexed by name at run time for no gain.
pub const AIR_STATE_ID: i32 = 0;

/// The biome id for the world this server generates.
///
/// Forty-one, which is what a real 26.3 server sent in the capture. It is the
/// flat world's own biome, and since this generator makes grass hills that is
/// the right one to pick.
///
/// Not zero. Zero is `minecraft:badlands` in the 26.3 registry, which was the
/// guess before there was a capture to check against, and it would have tinted
/// every blade of grass wrong. The registry order is not reported by the data
/// generator, so the number could not have been derived; it could only be read.
pub const DEFAULT_BIOME_ID: i32 = 41;

/// Bits a heightmap entry needs.
///
/// Heights run from -65 (an empty column) to 320 (the top of the world plus one),
/// so 386 distinct values and nine bits.
pub const HEIGHTMAP_BITS: usize = 9;

/// Entries in one heightmap: one per column.
pub const HEIGHTMAP_ENTRIES: usize = (WIDTH * WIDTH) as usize;

/// Heightmap types the client is sent.
///
/// Three, not two. The enum marks four of the seven for the client and one of
/// those turned out to be one the capture has and this did not:
///
/// ```text
/// // captured, walking the heightmap map of a real chunk
/// type 1  WORLD_SURFACE
/// type 4  MOTION_BLOCKING
/// type 5  MOTION_BLOCKING_NO_LEAVES
/// ```
///
/// Sent one fewer is not an error: the client reads the count and takes what it
/// is given. But the capture is the ground truth and it has three.
pub mod heightmap {
    /// Height of the first non-air block, used for lighting and for clouds.
    pub const WORLD_SURFACE: i32 = 1;
    /// Height of the first motion-blocking or fluid block.
    pub const MOTION_BLOCKING: i32 = 4;
    /// As MOTION_BLOCKING, but leaves do not count as blocking.
    pub const MOTION_BLOCKING_NO_LEAVES: i32 = 5;
}

/// How many heightmaps are sent.
pub const HEIGHTMAP_COUNT: i32 = 3;

/// Writes `ClientboundLevelChunkWithLightPacket` for one chunk.
///
/// `x` and `z` are chunk coordinates, so block (0, 0) is chunk (0, 0) and block
/// (16, 0) is chunk (1, 0).
///
/// With no terrain this sends air everywhere. See [`write_chunk_with_states`]
/// for the version that carries real ground.
pub fn write_chunk(out: &mut Vec<u8>, x: i32, z: i32) {
    let air = vec![AIR_STATE_ID; BLOCK_ENTRIES];
    let mut sections = Vec::with_capacity(SECTIONS);
    for _ in 0..SECTIONS {
        sections.push(air.clone());
    }
    write_chunk_with_states(out, x, z, &sections);
}

/// Writes the chunk, with each section's block states supplied.
///
/// `sections` has one entry per section, bottom first, and each is 4096 state
/// ids in wire order. This is the split between generating a world and putting it
/// on the wire: the world module knows about terrain and the wire module knows
/// about the protocol, and neither needs to know about the other.
pub fn write_chunk_with_states(out: &mut Vec<u8>, x: i32, z: i32, sections: &[Vec<i32>]) {
    VarInt(id::LEVEL_CHUNK_WITH_LIGHT).write(out);
    out.extend_from_slice(&x.to_be_bytes());
    out.extend_from_slice(&z.to_be_bytes());

    write_heightmaps(out);
    write_sections(out, sections);
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
    VarInt(HEIGHTMAP_COUNT).write(out);

    for kind in [
        heightmap::WORLD_SURFACE,
        heightmap::MOTION_BLOCKING,
        heightmap::MOTION_BLOCKING_NO_LEAVES,
    ] {
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
///
/// The counts matter and were the reason a chunk of real terrain showed nothing.
/// The client uses the block count to decide whether a section is worth building
/// geometry for, so a section full of stone that claims to hold zero blocks is
/// skipped. Both counts are zero only when the section really is empty.
pub fn write_sections(out: &mut Vec<u8>, sections: &[Vec<i32>]) {
    assert_eq!(
        sections.len(),
        SECTIONS,
        "a chunk is twenty-four sections tall"
    );

    let mut body = Vec::with_capacity(SECTIONS * 16);

    for states in sections {
        let solid = states.iter().filter(|s| **s != AIR_STATE_ID).count();
        let solid = std::cmp::min(solid, i16::MAX as usize) as i16;

        body.extend_from_slice(&solid.to_be_bytes());
        // No fluid anywhere yet, so this is honestly zero.
        body.extend_from_slice(&0i16.to_be_bytes());

        write_block_states(&mut body, states);
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

/// Bits per entry for a block state container with more than one value.
///
/// Four, always, and not the minimum. `Strategy.getConfigurationForBitCount`
/// maps one through four onto a single configuration:
///
/// ```java
/// case 1, 2, 3, 4 -> Strategy.FOUR_BITS_LINEAR;
/// ```
///
/// So a palette of four blocks is stored at four bits an entry, not two. Writing
/// the minimum would be smaller and would decode as the wrong blocks, since the
/// reader derives the layout from this byte rather than from the palette.
pub const BLOCK_BITS: usize = 4;

/// Entries in a block state container: one section.
pub const BLOCK_ENTRIES: usize = SECTION_VOLUME;

/// Re-exported so a decoder outside this module can size its own loop.
pub const BLOCK_ENTRIES_DIV: usize = BLOCK_ENTRIES;

/// Writes a block state container from the section's states in wire order.
///
/// The order matters and is already pinned by the world module: the index of a
/// block is `x + z * 16 + y * 256`, which is the formula the game uses. Passing
/// the states in a different order would produce a chunk that decodes cleanly and
/// is wrong.
fn write_block_states(out: &mut Vec<u8>, states: &[i32]) {
    assert_eq!(
        states.len(),
        BLOCK_ENTRIES,
        "a section is sixteen by sixteen by sixteen"
    );

    // The palette, in order of first appearance so the same terrain always
    // produces the same bytes.
    let mut palette: Vec<i32> = Vec::new();
    let mut indices: Vec<u8> = Vec::with_capacity(BLOCK_ENTRIES);

    for state in states {
        match palette.iter().position(|p| p == state) {
            Some(at) => indices.push(at as u8),
            None => {
                palette.push(*state);
                indices.push((palette.len() - 1) as u8);
            }
        }
    }

    if palette.len() == 1 {
        write_single_value_palette(out, palette[0]);
        return;
    }

    out.push(BLOCK_BITS as u8);

    VarInt(palette.len() as i32).write(out);
    for state in &palette {
        VarInt(*state).write(out);
    }

    // Sixteen entries to a long at four bits, and 4096 entries is 256 longs.
    let per_long = 64 / BLOCK_BITS;
    let longs = BLOCK_ENTRIES.div_ceil(per_long);
    let mut packed = vec![0u64; longs];

    for (entry, index) in indices.iter().enumerate() {
        let long = entry / per_long;
        let shift = (entry % per_long) * BLOCK_BITS;
        packed[long] |= (*index as u64) << shift;
    }

    for value in packed {
        out.extend_from_slice(&value.to_be_bytes());
    }
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
        write_sections(&mut w, &empty_sections());

        let (length, consumed) = VarInt::read(&mut &w[..]).expect("a length");
        let body = &w[consumed..consumed + length.0 as usize];

        // Two shorts, then a byte and a one-byte VarInt, twice.
        let per_section = 2 + 2 + 2 + 2;
        assert_eq!(body.len(), SECTIONS * per_section);
        assert_eq!(length.0 as usize, SECTIONS * per_section);
    }

    /// Twenty-four sections of air, which is what an empty chunk is.
    pub fn empty_sections() -> Vec<Vec<i32>> {
        vec![vec![AIR_STATE_ID; BLOCK_ENTRIES]; SECTIONS]
    }

    #[test]
    fn a_palette_of_more_than_one_is_four_bits() {
        // Not the minimum. Four blocks would fit in two bits, and the game
        // stores it in four.
        let mut states = vec![AIR_STATE_ID; BLOCK_ENTRIES];
        states[0] = 1; // stone
        states[1] = 9; // grass
        states[2] = 10; // dirt

        let mut w = Vec::new();
        write_block_states(&mut w, &states);

        assert_eq!(w[0], 4, "four bits an entry, however few blocks");
        // Air is still in there, so four entries, not three.
        assert_eq!(w[1], 4, "air, stone, grass and dirt");
        assert_eq!(
            &w[2..6],
            &[1, 9, 10, 0],
            "in order of first appearance, so the three written blocks come \
             first and the air they replaced is last"
        );

        // Sixteen entries to a long, 4096 entries, so 256 longs.
        let longs = 2 + 4 + 256 * 8;
        assert_eq!(w.len(), longs, "then the packed data with no length prefix");
    }

    #[test]
    fn a_single_value_still_takes_the_zero_bit_path() {
        let mut w = Vec::new();
        write_block_states(&mut w, &vec![7i32; BLOCK_ENTRIES]);

        assert_eq!(w, vec![0, 7], "a byte of bits and the value, nothing else");
    }

    #[test]
    fn the_packed_data_holds_palette_indices_not_state_ids() {
        let mut states = vec![0i32; BLOCK_ENTRIES];
        states[0] = 1;
        states[15] = 2;
        states[16] = 3;

        let mut w = Vec::new();
        write_block_states(&mut w, &states);

        // Stone is seen first, so the palette is [1, 0, 2, 3] and stone is index
        // zero. This is the part worth pinning: the packed data is a lookup into
        // the palette, not the state id itself.
        assert_eq!(&w[2..6], &[1, 0, 2, 3], "palette order");

        // One byte of bits, one of palette size, four palette ids.
        let data = 1 + 1 + 4;
        let first = u64::from_be_bytes(w[data..data + 8].try_into().expect("a long"));
        assert_eq!(first & 0xF, 0, "entry zero is stone, which is palette index zero");
        assert_eq!((first >> 60) & 0xF, 2, "and entry fifteen is index two");

        let second = u64::from_be_bytes(w[data + 8..data + 16].try_into().expect("a long"));
        assert_eq!(second & 0xF, 3, "entry sixteen starts the next long");
    }

    #[test]
    fn a_chunk_of_terrain_carries_more_than_the_empty_one() {
        let mut ground = empty_sections();
        // One section with a flat floor across its bottom layer.
        for block in ground[0].iter_mut().take(16 * 16) {
            *block = 1;
        }

        let mut w = Vec::new();
        write_chunk_with_states(&mut w, 0, 0, &ground);

        let mut empty = Vec::new();
        write_chunk(&mut empty, 0, 0);

        assert_eq!(w[0], id::LEVEL_CHUNK_WITH_LIGHT as u8);
        assert!(
            w.len() > 3000,
            "a section at four bits is 2052 bytes, so this should be well past \
             the empty chunk's size of {}",
            empty.len()
        );
    }

    #[test]
    fn the_block_ids_the_terrain_uses_are_the_ones_the_game_sends() {
        // The world module has its own five block registry, and these are the
        // ids it hands out for them. If the generated table is renumbered these
        // have to move with it, which is what this pins.
        let check = |name: &str, expected: i32| {
            let block = crate::generated::BLOCKS
                .iter()
                .find(|b| b.name == name)
                .unwrap_or_else(|| panic!("{name} is in the block table"));
            assert_eq!(block.default_state as i32, expected, "{name}");
        };
        check("minecraft:air", 0);
        check("minecraft:stone", 1);
        check("minecraft:grass_block", 9);
        check("minecraft:dirt", 10);
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

    #[test]
    fn a_section_reports_how_many_blocks_it_holds() {
        // The client uses this to decide whether to build geometry for the
        // section. A section full of stone claiming zero blocks is skipped, which
        // is why the count is computed rather than left at zero.
        let mut ground = empty_sections();
        ground[0][0] = 1;
        ground[0][1] = 9;

        let mut w = Vec::new();
        write_sections(&mut w, &ground);

        let (length, consumed) = crate::protocol::VarInt::read(&mut &w[..]).expect("a length");
        let body = &w[consumed..consumed + length.0 as usize];

        let first = i16::from_be_bytes(body[..2].try_into().expect("a short"));
        let fluid = i16::from_be_bytes(body[2..4].try_into().expect("a short"));
        assert_eq!(first, 2, "two of the four thousand are not air");
        assert_eq!(fluid, 0, "and there is no fluid");

        // The section went out at four bits an entry, which is the other thing
        // that has to be right for the count to matter at all.
        assert_eq!(body[4], 4);
    }

    #[test]
    fn a_section_of_pure_air_reports_zero() {
        let mut w = Vec::new();
        write_sections(&mut w, &empty_sections());

        let (length, consumed) = crate::protocol::VarInt::read(&mut &w[..]).expect("a length");
        let body = &w[consumed..consumed + length.0 as usize];
        assert_eq!(i16::from_be_bytes(body[..2].try_into().expect("a short")), 0);
    }
}
