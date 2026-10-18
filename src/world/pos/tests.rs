use super::*;

#[test]
fn local_index_is_a_bijection() {
    for index in 0..SECTION_VOLUME {
        assert_eq!(LocalPos::from_index(index).index(), index, "index {index}");
    }
    for x in 0..16u8 {
        for y in 0..16u8 {
            for z in 0..16u8 {
                let pos = LocalPos { x, y, z };
                assert_eq!(LocalPos::from_index(pos.index()), pos);
            }
        }
    }
}

#[test]
fn vertical_neighbours_are_contiguous() {
    // The property that makes y-major the right order for skylight.
    for x in 0..16usize {
        for z in 0..16usize {
            for y in 0..15usize {
                let below = LocalPos {
                    x: x as u8,
                    y: y as u8,
                    z: z as u8,
                }
                .index();
                let above = LocalPos {
                    x: x as u8,
                    y: y as u8 + 1,
                    z: z as u8,
                }
                .index();
                assert_eq!(above - below, 256, "x={x} y={y} z={z}");
            }
        }
    }
}

#[test]
fn block_pos_pack_round_trips() {
    let cases = [
        BlockPos::new(0, 0, 0),
        BlockPos::new(-1, 0, -1),
        BlockPos::new(33_554_431, 2047, -33_554_431),
        BlockPos::new(-33_554_431, -2048, 33_554_431),
        BlockPos::new(12345, -777, -98765),
    ];
    for pos in cases {
        assert_eq!(BlockPos::unpack(pos.pack()), pos, "{pos:?}");
    }
}

#[test]
fn chunk_pos_pack_round_trips() {
    for (x, z) in [
        (0, 0),
        (-1, -1),
        (1_234_567, -89_012_345),
        (i32::MIN, i32::MAX),
    ] {
        let pos = ChunkPos::new(x, z);
        assert_eq!(ChunkPos::unpack(pos.pack()), pos, "{pos:?}");
    }
}

#[test]
fn world_position_decomposes_across_negative_coordinates() {
    // The classic off-by-one: truncating division instead of flooring.
    for (x, y, z) in [
        (0, 0, 0),
        (-1, 0, -1),
        (-16, -1, -16),
        (-17, -2048, 15),
        (33, 5, -33),
    ] {
        let pos = BlockPos::new(x, y, z);
        let section = pos.section_pos();
        let local = pos.local_pos();

        assert!(local.x < 16 && local.y < 16 && local.z < 16, "{pos:?}");
        assert_eq!(
            section.min_block_pos().x + local.x as i32,
            x,
            "x for {pos:?}"
        );
        assert_eq!(
            section.min_block_pos().y + local.y as i32,
            y,
            "y for {pos:?}"
        );
        assert_eq!(
            section.min_block_pos().z + local.z as i32,
            z,
            "z for {pos:?}"
        );
        assert_eq!(pos.chunk_pos(), ChunkPos::new(section.x, section.z));
    }
}

#[test]
fn index_matches_the_minecraft_formula() {
    // Written out longhand on purpose: this is the layout we are matching.
    for x in [0u8, 7, 15] {
        for y in [0u8, 3, 15] {
            for z in [0u8, 11, 15] {
                let pos = LocalPos { x, y, z };
                assert_eq!(pos.index(), x as usize + z as usize * 16 + y as usize * 256);
            }
        }
    }
}