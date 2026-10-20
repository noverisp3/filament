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

/// One of the six faces of a block, and the step across it.
///
/// The numbering is the wire's, and Y is the axis that flips first: DOWN is
/// 0, UP is 1, NORTH is 2, SOUTH is 3, WEST is 4, EAST is 5. Getting it
/// wrong does not fail loudly, it just puts the block somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    /// The face the wire named, or `None` for an id the game does not have.
    pub fn from_3d_data_value(value: u8) -> Option<Self> {
        match value {
            0 => Some(Direction::Down),
            1 => Some(Direction::Up),
            2 => Some(Direction::North),
            3 => Some(Direction::South),
            4 => Some(Direction::West),
            5 => Some(Direction::East),
            _ => None,
        }
    }

    /// One block across this face.
    pub fn step(self) -> (i32, i32, i32) {
        match self {
            Direction::Down => (0, -1, 0),
            Direction::Up => (0, 1, 0),
            Direction::North => (0, 0, -1),
            Direction::South => (0, 0, 1),
            Direction::West => (-1, 0, 0),
            Direction::East => (1, 0, 0),
        }
    }

    /// The face on the other side.
    pub fn opposite(self) -> Self {
        match self {
            Direction::Down => Direction::Up,
            Direction::Up => Direction::Down,
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
            Direction::East => Direction::West,
        }
    }
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The block one step across a face.
    pub fn offset(self, direction: Direction) -> Self {
        let (dx, dy, dz) = direction.step();
        BlockPos::new(self.x + dx, self.y + dy, self.z + dz)
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
        // Y occupies 12 raw bits and must be sign-extended, or every negative
        // height decodes as a positive one.
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
    /// This is Minecraft's layout (`Strategy.getIndex`, 26.3), and it keeps a
    /// column of blocks contiguous for the skylight sweep.
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
    /// Every axis must be below 16: light propagation adds offsets to the axes,
    /// so an unchecked value would index out of the section.
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
mod tests;
