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
    write_chunk_with_states(out, x, z, &sections, &[]);
}

/// Writes the chunk, with each section's block states supplied.
///
/// `sections` has one entry per section, bottom first, and each is 4096 state
/// ids in wire order. This is the split between generating a world and putting it
/// on the wire: the world module knows about terrain and the wire module knows
/// about the protocol, and neither needs to know about the other.
pub fn write_chunk_with_states(
    out: &mut Vec<u8>,
    x: i32,
    z: i32,
    sections: &[Vec<i32>],
    sky: &[Option<Vec<u8>>],
) {
    VarInt(id::LEVEL_CHUNK_WITH_LIGHT).write(out);
    out.extend_from_slice(&x.to_be_bytes());
    out.extend_from_slice(&z.to_be_bytes());

    write_heightmaps(out);
    write_sections(out, sections);
    write_block_entities(out);
    write_light(out, sky);
}

/// Packet ids, from `packets.json`.
pub mod id {
    pub const LEVEL_CHUNK_WITH_LIGHT: i32 = 46;
    pub const LIGHT_UPDATE: i32 = 49;
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

/// Past this many entries the palette stops growing and the container goes
/// global: `getConfigurationForBitCount` maps anything above eight bits onto
/// the global palette, whose width is set by the registry, not the section.
pub const GLOBAL_BITS_THRESHOLD: usize = 8;

/// The global palette's width: ceil(log2(block states in the registry)).
///
/// Thirty-five thousand seven hundred and twenty-three states need sixteen
/// bits. With no palette list to name, the packed entries are state ids
/// themselves.
pub const GLOBAL_BITS: usize = 16;

/// ceil(log2(n)) for n >= 1, the game's `Mth.ceillog2`.
fn ceil_log2(n: usize) -> usize {
    (usize::BITS - (n - 1).leading_zeros()) as usize
}

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
    let mut indices: Vec<usize> = Vec::with_capacity(BLOCK_ENTRIES);

    for state in states {
        match palette.iter().position(|p| p == state) {
            Some(at) => indices.push(at),
            None => {
                palette.push(*state);
                indices.push(palette.len() - 1);
            }
        }
    }

    if palette.len() == 1 {
        write_single_value_palette(out, palette[0]);
        return;
    }

    // The width the game's own `Strategy.getConfigurationForBitCount` picks
    // for this many entries. One through sixteen share the four-bit linear
    // palette; seventeen to two hundred and fifty-six grow a hashmap palette
    // one bit at a time; past that the palette is dropped entirely and the
    // entries are global state ids. Writing one width while packing another
    // is the LinearPalette crash a second player's chunk produced: a real
    // world has more than sixteen kinds of block in one section, and the
    // four-bit index wrapped into the neighbouring entry.
    let bits = if palette.len() <= 16 {
        BLOCK_BITS
    } else if palette.len() <= 1 << GLOBAL_BITS_THRESHOLD {
        ceil_log2(palette.len())
    } else {
        GLOBAL_BITS
    };

    out.push(bits as u8);

    // A global palette carries no list: the reader's `GlobalPalette.read` is
    // empty, and the packed entries are registry ids rather than palette
    // indices.
    if bits < GLOBAL_BITS {
        VarInt(palette.len() as i32).write(out);
        for state in &palette {
            VarInt(*state).write(out);
        }
    }

    let per_long = 64 / bits;
    let longs = BLOCK_ENTRIES.div_ceil(per_long);
    let mut packed = vec![0u64; longs];

    // `SimpleBitStorage` never lets an entry straddle a long: each long holds
    // `64 / bits` whole entries and the leftover bits go unused. The source is
    // a palette index or, with no palette, the global state id itself.
    let source: Vec<u64> = if bits < GLOBAL_BITS {
        indices.iter().map(|index| *index as u64).collect()
    } else {
        states.iter().map(|state| *state as u64).collect()
    };
    for (entry, value) in source.iter().enumerate() {
        let long = entry / per_long;
        let shift = (entry % per_long) * bits;
        packed[long] |= value << shift;
    }

    // No length in front: the reader sizes the array from the width it just
    // read and the entries it knows the section holds, so a count here is
    // eight bytes of noise the first long would be read as.
    for value in &packed {
        out.extend_from_slice(&value.to_be_bytes());
    }
}/// Block entities: none.
fn write_block_entities(out: &mut Vec<u8>) {
    VarInt(0).write(out);
}

/// The light for the chunk.
///
/// Four bit sets and two lists. The bit sets say which sections carry light, and
/// which are empty of it; the lists hold the light itself, two thousand and
/// forty-eight bytes per section because each block gets four bits.
///
/// The sky light is the world's own, not a blanket fifteen. A blanket fifteen
/// is what the client used to be told, and it is why a hole dug in a lit room
/// stayed lit: the client was told the world was in full daylight, and it
/// believed it.
fn write_light(out: &mut Vec<u8>, sky: &[Option<Vec<u8>>]) {
    // The sections that carry sky light, and the ones that carry none of it.
    let mut sky_present: Vec<u8> = Vec::new();
    let mut sky_empty: Vec<u8> = Vec::new();
    let mut updates: Vec<Vec<u8>> = Vec::new();
    for (index, section) in sky.iter().enumerate() {
        match section {
            Some(bytes) if bytes.iter().any(|b| *b != 0) => {
                set_bit(&mut sky_present, index);
                updates.push(bytes.clone());
            }
            Some(_) => {
                // Every cell in it is dark: the game says so with the empty
                // mask and sends no data for it.
                set_bit(&mut sky_empty, index);
            }
            None => {}
        }
    }

    write_bit_set(out, &sky_present);
    // No block light anywhere: nothing in this world gives off light yet.
    write_bit_set(out, &[]);
    write_bit_set(out, &sky_empty);
    write_bit_set(out, &[]);

    VarInt(updates.len() as i32).write(out);
    for bytes in &updates {
        VarInt(bytes.len() as i32).write(out);
        out.extend_from_slice(bytes);
    }

    // And no block light updates, which is consistent with every section being
    // empty of it.
    VarInt(0).write(out);
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

/// Writes the light of some sections of a chunk, for the sections that moved.
///
/// The client keeps the light it was given with the chunk, so a hole dug in a
/// lit room is still lit on its screen until somebody says otherwise. This is
/// the saying otherwise.
///
/// The shape is the same as the light in the chunk packet: four bit sets and
/// two lists. The first says which sections carry sky light, the third which
/// carry none of it, and the list holds the light itself.
pub fn write_light_update(
    out: &mut Vec<u8>,
    chunk_x: i32,
    chunk_z: i32,
    sections: &[(i32, Vec<u8>)],
) {
    VarInt(id::LIGHT_UPDATE).write(out);
    VarInt(chunk_x).write(out);
    VarInt(chunk_z).write(out);

    // The sections with light in them, and the ones without.
    let mut present: Vec<u8> = Vec::new();
    let mut empty: Vec<u8> = Vec::new();
    for (index, bytes) in sections {
        if bytes.iter().any(|b| *b != 0) {
            set_bit(&mut present, *index as usize);
        } else {
            set_bit(&mut empty, *index as usize);
        }
    }

    write_bit_set(out, &present);
    // No block light: nothing in this world gives off light yet.
    write_bit_set(out, &[]);
    write_bit_set(out, &empty);
    write_bit_set(out, &[]);

    VarInt(sections.len() as i32).write(out);
    for (_, bytes) in sections {
        VarInt(bytes.len() as i32).write(out);
        out.extend_from_slice(bytes);
    }

    // And no block light updates, which is consistent with every section being
    // empty of it.
    VarInt(0).write(out);
}

/// Sets one bit in a bit set, which is a list of bytes with the least
/// significant bit first.
pub fn set_bit(bits: &mut Vec<u8>, index: usize) {
    let byte = index / 8;
    if bits.len() <= byte {
        bits.resize(byte + 1, 0);
    }
    bits[byte] |= 1 << (index % 8);
}
/// A seed for the world, kept here so the world module does not need to know
/// about the packet.
pub fn seed_from_string(name: &str) -> i64 {
    let _ = name;
    0
}

#[cfg(test)]
mod tests;
