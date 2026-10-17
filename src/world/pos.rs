//! Integer coordinates and the packed representations used as map keys.
//!
//! Packing matters more than it looks: `HashMap` keys are compared on every
//! lookup, and a 12-byte struct key is three words wide to compare where a
//! `u64` is one.

/// Side length of a chunk section in blocks.
pub const SECTION_DIM: i32 = 16;

/// Blocks per section edge, as a `usize`.
pub const SECTION_DIM_USIZE: usize = SECTION_DIM as usize;

/// Blocks in one section: 16³.
pub const SECTION_VOLUME: usize = SECTION_DIM_USIZE * SECTION_DIM_USIZE * SECTION_DIM_USIZE;

/// Bit width of each axis inside a [`LocalPos`] index (4 bits for 0..16).
const LOCAL_BITS: u32 = 4;

/// A block position in world space.
///
/// Packs to `u64` as 26 bits of X, 12 bits of Y, 26 bits of Z, matching
/// Minecraft's world limits: X/Z within ±33 554 431, Y within −2048..2047.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Y is signed 12-bit, X and Z are unsigned 26-bit biased by half the range.
const XZ_BIAS: i32 = 1 << 25;

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Packs into a single `u64`.
    ///
    /// # Panics
    /// If any component is outside the packed range.
    pub fn pack(self) -> u64 {
        let x = u32::try_from(self.x + XZ_BIAS).expect("x out of packed range");
        let z = u32::try_from(self.z + XZ_BIAS).expect("z out of packed range");
        assert!(x < (1 << 26), "x out of packed range");
        assert!(z < (1 << 26), "z out of packed range");
        assert!(
            (-(1 << 11)..(1 << 11)).contains(&self.y),
            "y out of packed range"
        );

        (u64::from(x) << 38) | ((self.y as u32 as u64) & 0xFFF) << 26 | u64::from(z)
    }

    /// Inverse of [`Self::pack`].
    pub fn unpack(packed: u64) -> Self {
        // Y occupies 12 raw bits and must be sign-extended; without that every
        // negative height decodes as a positive one. Shifting the field up and
        // back down by 20 is the cheap way to extend the sign bit.
        let y = (((packed >> 26) & 0xFFF) as i32) << 20 >> 20;
        Self {
            x: ((packed >> 38) & 0x3FF_FFFF) as u32 as i32 - XZ_BIAS,
            y,
            z: (packed & 0x3FF_FFFF) as u32 as i32 - XZ_BIAS,
        }
    }

    /// Chunk containing this block.
    pub const fn chunk_pos(self) -> ChunkPos {
        ChunkPos {
            x: self.x.div_euclid(SECTION_DIM),
            z: self.z.div_euclid(SECTION_DIM),
        }
    }

    /// Section containing this block.
    pub const fn section_pos(self) -> SectionPos {
        SectionPos {
            x: self.x.div_euclid(SECTION_DIM),
            y: self.y.div_euclid(SECTION_DIM),
            z: self.z.div_euclid(SECTION_DIM),
        }
    }

    /// Position within the containing section, each axis in 0..16.
    ///
    /// Uses `rem_euclid` so negative world coordinates land in range.
    pub const fn local_pos(self) -> LocalPos {
        LocalPos {
            x: self.x.rem_euclid(SECTION_DIM) as u8,
            y: self.y.rem_euclid(SECTION_DIM) as u8,
            z: self.z.rem_euclid(SECTION_DIM) as u8,
        }
    }
}

/// A chunk position on the XZ plane, packed as two biased 32-bit halves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub fn pack(self) -> u64 {
        ((u64::from(self.x as u32)) << 32) | u64::from(self.z as u32)
    }

    pub fn unpack(packed: u64) -> Self {
        Self {
            x: (packed >> 32) as u32 as i32,
            z: packed as u32 as i32,
        }
    }

    /// Lowest world block this chunk covers.
    pub const fn min_block_pos(self) -> BlockPos {
        BlockPos::new(
            self.x * SECTION_DIM,
            super::chunk::MIN_BLOCK_Y,
            self.z * SECTION_DIM,
        )
    }
}

/// A section position in world space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub const fn chunk_pos(self) -> ChunkPos {
        ChunkPos {
            x: self.x,
            z: self.z,
        }
    }

    /// The section one step away on each axis.
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            z: self.z + dz,
        }
    }

    /// World position of this section's `(0, 0, 0)` corner.
    pub const fn min_block_pos(self) -> BlockPos {
        BlockPos::new(
            self.x * SECTION_DIM,
            self.y * SECTION_DIM,
            self.z * SECTION_DIM,
        )
    }
}

/// A position inside a section, each axis in 0..16.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalPos {
    pub x: u8,
    pub y: u8,
    pub z: u8,
}

impl LocalPos {
    /// Linear index into a section, **y-major**: `x + z * 16 + y * 256`.
    ///
    /// This is Minecraft's layout (`Strategy.getIndex`, 26.3). The dominant
    /// vertical operation — skylight propagation — then walks contiguous memory
    /// straight up a column. At 4 bits per entry a whole section is 2 KiB and
    /// stays L1-resident, so this is a second-order concern either way.
    #[inline]
    pub const fn index(self) -> usize {
        (self.y as usize) << (LOCAL_BITS * 2) | (self.z as usize) << LOCAL_BITS | self.x as usize
    }

    /// Inverse of [`Self::index`].
    pub const fn from_index(index: usize) -> Self {
        Self {
            x: (index & 0xF) as u8,
            z: ((index >> LOCAL_BITS) & 0xF) as u8,
            y: ((index >> (LOCAL_BITS * 2)) & 0xF) as u8,
        }
    }

    /// Builds a position from `[x, y, z]`.
    ///
    /// Every axis must be below 16. Light propagation computes neighbouring
    /// cells by adding offsets to axes, so an unchecked value here would index
    /// out of the section instead of being caught where it was produced.
    pub fn from_array([x, y, z]: [u8; 3]) -> Self {
        assert!(
            x < 16 && y < 16 && z < 16,
            "local position [{x}, {y}, {z}] is outside a 16-cube section"
        );
        Self { x, y, z }
    }

    pub const fn to_array(self) -> [u8; 3] {
        [self.x, self.y, self.z]
    }
}

#[cfg(test)]
mod tests {
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
}
