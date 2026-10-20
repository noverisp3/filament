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
fn seventeen_states_widen_to_five_bits() {
    // A section of a real world holds more than sixteen kinds of block, and
    // the four-bit linear palette holds exactly sixteen entries. The game's
    // own answer at seventeen is the five-bit hashmap palette; writing four
    // bits here was a LinearPalette crash on the second player to join.
    let mut states = vec![0i32; BLOCK_ENTRIES];
    for (at, state) in states.iter_mut().enumerate() {
        *state = (at % 17) as i32;
    }

    let mut w = Vec::new();
    write_block_states(&mut w, &states);

    assert_eq!(w[0], 5, "five bits once the palette passes sixteen entries");
    let (len, used) = VarInt::read(&mut &w[1..]).expect("a palette length");
    assert_eq!(len.0, 17);
    // Every id here is a one-byte VarInt, then the data with no count in
    // front: the reader sizes it from the width and the section's entries.
    let data_start = 1 + used + 17;
    let first_long = u64::from_be_bytes(w[data_start..data_start + 8].try_into().unwrap());
    assert_eq!(first_long & 0x1f, 0, "states[0] is palette index zero");
    // 4096 entries at five bits: twelve to a long, 342 longs and four spare bits.
    assert_eq!(w.len(), data_start + 342 * 8);
}

#[test]
fn past_two_hundred_fifty_six_the_palette_is_global() {
    // The hashmap palette stops at eight bits. Past that the container keeps
    // no list at all and the packed entries are registry ids; the reader's
    // GlobalPalette.read is empty, so there is no length to skip either.
    let mut states = vec![0i32; BLOCK_ENTRIES];
    for (at, state) in states.iter_mut().enumerate() {
        *state = (at % 300) as i32;
    }

    let mut w = Vec::new();
    write_block_states(&mut w, &states);

    assert_eq!(w[0], GLOBAL_BITS as u8, "sixteen bits, the registry's own width");
    // Nothing between the width and the data: no palette length, no long
    // count, both derived by the reader.
    let first_long = u64::from_be_bytes(w[1..9].try_into().unwrap());
    assert_eq!(first_long & 0xffff, 0, "states[0] is state id zero");

    let per_long = 64 / GLOBAL_BITS;
    assert_eq!(
        w.len(),
        1 + BLOCK_ENTRIES.div_ceil(per_long) * 8,
        "4096 entries at sixteen bits is 1024 longs"
    );
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
    write_chunk_with_states(&mut w, 0, 0, &ground, &[], &[]);

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
fn the_light_carries_the_sections_that_have_it() {
    // Two sections with light in them and one that is dark: the dark one is
    // named in the empty mask and no data is sent for it, which is what the
    // game does and what keeps a dark cave from carrying two thousand bytes
    // of zero.
    let lit = vec![0xFFu8; 2048];
    let dark = vec![0u8; 2048];
    let sky = vec![Some(lit.clone()), Some(dark.clone()), None];

    let mut w = Vec::new();
    write_light(&mut w, &sky, &[]);

    // A bit set is a byte count and then the bytes, so the first section is
    // one bit in one byte.
    assert_eq!(w[0], 1, "the sky mask is one byte");
    assert_eq!(w[1], 0b0000_0010, "the first section has sky light");

    assert_eq!(w[2], 0, "the block mask is empty, not zero");

    // The empty sky mask names the second section.
    assert_eq!(w[3], 1, "the empty sky mask is one byte");
    assert_eq!(w[4], 0b0000_0100, "the second section is dark");

    assert_eq!(w[5], 0, "the empty block mask is empty");

    // And one update, for the one section that had light.
    let (updates, after) = VarInt::read(&mut &w[6..]).expect("an update count");
    assert_eq!(updates.0, 1);
    let (len, _) = VarInt::read(&mut &w[6 + after..]).expect("a length");
    assert_eq!(len.0, 2048, "a section of light is two thousand and forty-eight bytes");
}

#[test]
fn the_worlds_bottom_section_is_bit_one() {
    // The game's light engine numbers its sections from one below the world,
    // so that light can spread off the bottom edge:
    //
    //     LevelLightEngine.getMinLightSection() = getMinSectionY() - 1
    //
    // The world's own bottom section is therefore bit 1, and bit 0 is the
    // padding section below the world. A server that counts from the world's
    // bottom instead puts every section one too low, and the light of a lamp
    // lands on the floor beneath it.
    //
    // The capture of a stock server agrees, and it can be checked against the
    // terrain rather than taken on trust: the chunk is a flat world whose
    // ground is in its lowest section, so the section with the ground is the
    // one the sky mask names, and the sky in it is lit everywhere above the
    // ground and dark in it.
    let lit = vec![0xFFu8; 2048];
    let sky = vec![Some(lit)];

    let mut w = Vec::new();
    write_light(&mut w, &sky, &[]);

    assert_eq!(w[0], 1, "one byte of mask");
    assert_eq!(
        w[1],
        0b0000_0010,
        "the bottom section of the world is bit one, not bit zero"
    );
}

#[test]
fn a_light_update_carries_one_array_for_each_section_it_names() {
    // The client walks the mask and takes the next array for each bit it
    // finds, so the list has to have exactly one array per section the mask
    // names. A section with no light in it is named in the empty mask and
    // gets no array at all, which is what the capture shows: two sections lit,
    // one empty, two arrays.
    let lit = vec![0xFFu8; 2048];
    let dark = vec![0u8; 2048];
    let sections = vec![(0, lit.clone()), (1, dark.clone()), (2, lit.clone())];

    let mut w = Vec::new();
    write_light_update(&mut w, 4, 5, &sections, &[]);

    let mut r = &w[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("a packet id");
    assert_eq!(packet, id::LIGHT_UPDATE);
    let (VarInt(x), _) = VarInt::read(&mut r).expect("a chunk x");
    assert_eq!(x, 4);
    let (VarInt(z), _) = VarInt::read(&mut r).expect("a chunk z");
    assert_eq!(z, 5);

    // Four masks: the sections with sky light, the ones with block light, the
    // ones empty of sky light, the ones empty of block light.
    let mut masks = Vec::new();
    for _ in 0..4 {
        let (VarInt(len), _) = VarInt::read(&mut r).expect("a mask length");
        masks.push(r[..len as usize].to_vec());
        r = &r[len as usize..];
    }
    // The first mask names the two lit sections, the third the dark one.
    assert_eq!(masks[0], vec![0b0000_1010], "sections zero and two have light");
    assert_eq!(masks[2], vec![0b0000_0100], "section one is empty of it");

    // And the list holds one array for each of the two lit sections.
    let (VarInt(layers), _) = VarInt::read(&mut r).expect("a layer count");
    assert_eq!(layers, 2, "one array for each section the mask names");
    for _ in 0..layers {
        let (VarInt(len), _) = VarInt::read(&mut r).expect("a layer length");
        assert_eq!(len, 2048, "a section of light is two thousand and forty-eight bytes");
        r = &r[len as usize..];
    }

    // No block light updates.
    let (VarInt(block_layers), _) = VarInt::read(&mut r).expect("a block layer count");
    assert_eq!(block_layers, 0);
    assert!(r.is_empty(), "and the walk ends on the last byte");
}

#[test]
fn the_light_of_the_blocks_travels_with_the_light_of_the_sun() {
    // A chunk carries two layers of light: the sun's and the one the blocks
    // give off. The client reads four masks and two lists, and the second
    // layer is not a part of the first: a lamp in a sunlit room is not the
    // sun.
    let sky = vec![Some(vec![0xFFu8; 2048]), Some(vec![0u8; 2048]), None];
    let block = vec![Some(vec![0x0Fu8; 2048]), Some(vec![0u8; 2048]), None];

    let mut w = Vec::new();
    write_light(&mut w, &sky, &block);

    // The sky mask names the first section, and the block mask names it too.
    assert_eq!(w[0], 1, "the sky mask is one byte");
    assert_eq!(w[1], 0b0000_0010, "the first section has sky light");
    assert_eq!(w[2], 1, "the block mask is one byte");
    assert_eq!(w[3], 0b0000_0010, "and the first section has block light");

    // Then the two empty masks, naming the second section each.
    assert_eq!(w[4], 1, "the empty sky mask is one byte");
    assert_eq!(w[5], 0b0000_0100, "the second section has no sky light");
    assert_eq!(w[6], 1, "the empty block mask is one byte");
    assert_eq!(w[7], 0b0000_0100, "and no block light either");

    // And one array for the sun and one for the lamps.
    let mut r = &w[8..];
    let (sky_layers, _) = VarInt::read(&mut r).expect("a layer count");
    assert_eq!(sky_layers.0, 1, "one array of sky light");
    let (len, _) = VarInt::read(&mut r).expect("a length");
    assert_eq!(len.0, 2048);
    r = &r[2048..];

    let (block_layers, _) = VarInt::read(&mut r).expect("a block layer count");
    assert_eq!(block_layers.0, 1, "one array of block light");
    let (len, _) = VarInt::read(&mut r).expect("a length");
    assert_eq!(len.0, 2048);
}

#[test]
fn a_light_update_names_each_section_once_and_carries_one_array_for_it() {
    // The mask names the sections that have light and the arrays follow it,
    // one for each bit. A section named twice in the list is an array the
    // client did not expect, and everything after it is read from the wrong
    // place -- which is a client holding half of one section's light and half
    // of another's, and a world that looks lit in patches.
    //
    // The caller is the one that keeps the list free of repeats: see
    // `group_light`, which is where a section that is reported twice -- by a
    // block that changed and by the world-wide pass -- is named once.
    let lit = vec![0xFFu8; 2048];
    let dark = vec![0u8; 2048];

    let sections = vec![(3, lit.clone()), (4, dark.clone())];
    let mut w = Vec::new();
    write_light_update(&mut w, 1, 2, &sections, &[]);

    let mut r = &w[..];
    let _ = VarInt::read(&mut r).expect("a packet id");
    let _ = VarInt::read(&mut r).expect("a chunk x");
    let _ = VarInt::read(&mut r).expect("a chunk z");

    // The head of the packet, byte for byte: the id, the chunk, the four
    // masks and the count of the arrays.
    //
    //   31          the packet
    //   01 02       the chunk
    //   01 10       the sky mask: one byte, section three
    //   00          the block mask: nothing
    //   01 20       the empty sky mask: one byte, section four
    //   00          the empty block mask: nothing
    //   01          one array
    //   80 10       two thousand and forty-eight bytes
    assert_eq!(
        &w[..12],
        &[
            0x31, 0x01, 0x02, // the packet and the chunk
            0x01, 0x10, // the sky mask names section three
            0x00, // the block mask names nothing
            0x01, 0x20, // the empty sky mask names section four
            0x00, // and the empty block mask names nothing
            0x01, // one array
            0x80, 0x10, // two thousand and forty-eight bytes
        ],
        "the mask and the arrays have to line up"
    );
}

#[test]
fn a_bit_set_of_sections_is_the_shortest_form() {
    // BitSet.toByteArray is the shortest form that reaches the highest bit,
    // so twenty-four sections is three bytes and not eight.
    let all = vec![0xFFu8; 3];

    let mut w = Vec::new();
    write_bit_set(&mut w, &all);
    assert_eq!(w, vec![3, 0xFF, 0xFF, 0xFF]);

    let mut empty = Vec::new();
    write_bit_set(&mut empty, &[]);
    assert_eq!(empty, vec![0], "an empty bit set is a zero length");
}

#[test]
fn the_sky_updates_are_one_layer_per_section_that_has_light() {
    // Twenty-four sections, all with light in them: one update each, and the
    // data is the section's own light rather than a blanket fifteen.
    let lit: Vec<Option<Vec<u8>>> = (0..SECTIONS)
        .map(|_| Some(vec![0xFFu8; LIGHT_BYTES]))
        .collect();

    let mut w = Vec::new();
    write_light(&mut w, &lit, &[]);

    // Four masks (eight bytes: the sky mask carries four and the other three
    // carry none), then a count, then a layer each: a VarInt length of 2048,
    // which is two bytes, and 2048 bytes of light.
    //
    // The sky mask is four bytes because the sections are numbered from one
    // below the world: the top section of the world is bit 24, and bit 24 is
    // in the fourth byte.
    let expected = 8 + 1 + SECTIONS * (2 + LIGHT_BYTES) + 1;
    assert_eq!(
        w.len(),
        expected,
        "four masks, then a count, {SECTIONS} layers of {LIGHT_BYTES} bytes \
         each, then an empty block list"
    );
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