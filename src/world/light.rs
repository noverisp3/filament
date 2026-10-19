//! Per-block light, kept as two layers that propagate by different rules.
//!
//! Sky light comes from the sun and is column-driven: it falls straight down
//! without weakening, and loses one level per block when it spreads sideways.
//! Block light comes from emissive blocks such as glowstone, and loses one level
//! per block in every direction, downwards included.
//!
//! The layers live apart from the block palette on purpose. Light changes far
//! more often than blocks do, and it changes in ways that say nothing about
//! which block sits at a position, so folding it into the palette would mean
//! rewriting a paletted section on every lighting update.
//!
//! This module holds one section's light and the rule for filling it. Deciding
//! which sections to fill, in what order, and how to keep several chunks
//! consistent is a world-level concern and lives in `world::World`.

use std::collections::VecDeque;

use super::palette::Paletted;
use super::pos::{LocalPos, SECTION_DIM, SECTION_VOLUME};

/// Highest value either layer can hold.
pub const MAX_LIGHT: u8 = 15;

/// The six directions light travels, in the order [`LightNeighbors::get`]
/// indexes them.
pub const DIRECTIONS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// The last local coordinate on each axis.
const LAST: u8 = (SECTION_DIM - 1) as u8;

/// The two light layers for one section.
///
/// Backed by [`Paletted`] like the block data, because almost every section is
/// uniform: everything above the terrain is open sky, everything deep
/// underground is black. A dense pair of arrays would spend nearly all its
/// bytes storing one repeated value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionLight {
    sky: Paletted<u8>,
    block: Paletted<u8>,
}

impl SectionLight {
    /// No light anywhere.
    pub fn dark() -> Self {
        Self {
            sky: Paletted::uniform(0, SECTION_VOLUME),
            block: Paletted::uniform(0, SECTION_VOLUME),
        }
    }

    /// Full sky light and no block light, which is what open air above the
    /// terrain looks like.
    pub fn open_sky() -> Self {
        Self {
            sky: Paletted::uniform(MAX_LIGHT, SECTION_VOLUME),
            block: Paletted::uniform(0, SECTION_VOLUME),
        }
    }

    #[inline]
    pub fn sky(&self, pos: LocalPos) -> u8 {
        self.sky.get(pos.index())
    }

    #[inline]
    pub fn block(&self, pos: LocalPos) -> u8 {
        self.block.get(pos.index())
    }

    #[inline]
    pub fn set_sky(&mut self, pos: LocalPos, light: u8) {
        self.sky.set(pos.index(), light);
    }

    /// Sets the sky light of every cell at once.
    ///
    /// A section is lit or dark as a whole most of the time, and saying so
    /// in one call is the difference between a section costing a moment and a
    /// section costing sixteen thousand.
    pub fn fill_sky(&mut self, light: u8) {
        self.sky.fill(light);
    }

    #[inline]
    pub fn set_block(&mut self, pos: LocalPos, light: u8) {
        self.block.set(pos.index(), light);
    }

    /// Brightest of the two layers at a cell.
    ///
    /// A glowstone in a sunlit cave must not read as darker than the daylight
    /// reaching the same cell, so the renderer wants the maximum rather than
    /// one channel winning over the other.
    pub fn combined(&self, pos: LocalPos) -> u8 {
        self.sky(pos).max(self.block(pos))
    }

    /// The sky layer as the game packs it: two cells to a byte, the even cell
    /// in the low nibble.
    ///
    /// The game keeps a section's light in two thousand and forty-eight bytes
    /// of nibbles, and that is what goes on the wire. The even cell is the low
    /// nibble: `DataLayer.get` reads `data[index >> 1]` and shifts by
    /// `(index & 1) << 2`.
    pub fn sky_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; SECTION_VOLUME / 2];
        for (index, value) in self.sky.iter().enumerate() {
            let nibble = value & 0x0F;
            if index % 2 == 0 {
                out[index / 2] = nibble;
            } else {
                out[index / 2] |= nibble << 4;
            }
        }
        out
    }

    /// The block layer as the game packs it, the same way as the sky.
    pub fn block_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; SECTION_VOLUME / 2];
        for (index, value) in self.block.iter().enumerate() {
            let nibble = value & 0x0F;
            if index % 2 == 0 {
                out[index / 2] = nibble;
            } else {
                out[index / 2] |= nibble << 4;
            }
        }
        out
    }

    /// Dense index-array bytes for both layers. Excludes the palettes.
    pub fn nbytes(&self) -> usize {
        self.sky.nbytes() + self.block.nbytes()
    }

    /// True when both layers are uniform, which costs no dense storage.
    pub fn is_uniform(&self) -> bool {
        self.sky.bits() == 0 && self.block.bits() == 0
    }
}

/// Whether a cell stops light.
///
/// A trait rather than `&Section`, because opacity needs the block registry and
/// this module should not have to know about it. It also lets the rules be
/// tested against hand-built grids without generating a world.
pub trait BlockOpacity {
    fn opaque(&self, pos: LocalPos) -> bool;
}

/// A section's light plus, optionally, its six neighbours'.
///
/// Same shape and the same reasoning as `world::section::SectionNeighbors`:
/// `None` means "not loaded", and light treats that as opaque, so an unloaded
/// neighbour stops light instead of leaking daylight through a hole.
#[derive(Clone, Copy, Debug, Default)]
pub struct LightNeighbors<'a> {
    pub pos_x: Option<&'a SectionLight>,
    pub neg_x: Option<&'a SectionLight>,
    pub pos_y: Option<&'a SectionLight>,
    pub neg_y: Option<&'a SectionLight>,
    pub pos_z: Option<&'a SectionLight>,
    pub neg_z: Option<&'a SectionLight>,
}

impl<'a> LightNeighbors<'a> {
    /// The neighbour for direction `index`, where the index is a position in
    /// [`DIRECTIONS`].
    pub fn get(&self, index: usize) -> Option<&'a SectionLight> {
        match index {
            0 => self.pos_x,
            1 => self.neg_x,
            2 => self.pos_y,
            3 => self.neg_y,
            4 => self.pos_z,
            _ => self.neg_z,
        }
    }

    /// The cell in that neighbour which sits across the boundary from `local`.
    ///
    /// Returns `None` when `local` is not on the face bordering that neighbour,
    /// which is the common case: most of a section is not on any of its six faces.
    /// Checking here rather than at the call site means a caller cannot forget,
    /// and forgetting lights the whole section from one neighbour.
    ///
    /// Crossing +X leaves the neighbour's x at 0 and keeps y and z; crossing +Y
    /// leaves its y at 0 and keeps x and z. Getting the remap wrong lights the
    /// wrong wall, which looks plausible enough to survive a casual look.
    pub fn across(&self, index: usize, local: LocalPos) -> Option<(LocalPos, &'a SectionLight)> {
        let neighbor = self.get(index)?;
        let [x, y, z] = local.to_array();

        let on_face = match index {
            0 => x == LAST,
            1 => x == 0,
            2 => y == LAST,
            3 => y == 0,
            4 => z == LAST,
            _ => z == 0,
        };
        if !on_face {
            return None;
        }

        let remapped = match index {
            0 => [0, y, z],
            1 => [LAST, y, z],
            2 => [x, 0, z],
            3 => [x, LAST, z],
            4 => [x, y, 0],
            _ => [x, y, LAST],
        };
        Some((LocalPos::from_array(remapped), neighbor))
    }
}

/// Fills `light` with the sky light for one section.
///
/// `open_above` says whether the section above is open sky all the way up,
/// which is the only thing that can start a full-strength column. The
/// neighbours are read but never written: propagation across sections is the
/// driver's job, because it has to keep several chunks consistent while doing
/// it.
///
/// The three steps are: sunlight falls down each column, whatever light arrives
/// from the six neighbours seeds the boundary, and that seed spreads inward.
pub fn compute_sky(
    light: &mut SectionLight,
    blocks: &dyn BlockOpacity,
    neighbors: &LightNeighbors<'_>,
    open_above: bool,
) {
    *light = SectionLight::dark();

    sunlight_columns(light, blocks, open_above);
    seed_from_neighbors(light, blocks, neighbors);
    spread_inward(light, blocks);
}

/// Sunlight falls straight down without weakening, until something opaque stops
/// it.
///
/// This is why a cave mouth is bright at the entrance and dark one block
/// further in, and why an overhang casts a shadow with a soft edge: the
/// sideways pass runs afterwards and fills that edge back in.
fn sunlight_columns(light: &mut SectionLight, blocks: &dyn BlockOpacity, open_above: bool) {
    for x in 0..LAST + 1 {
        for z in 0..LAST + 1 {
            let mut falling = open_above;
            for y in (0..=LAST).rev() {
                let pos = LocalPos::from_array([x, y, z]);
                if blocks.opaque(pos) {
                    // The block itself is dark, and so is everything below it.
                    falling = false;
                    light.set_sky(pos, 0);
                    continue;
                }
                if !falling {
                    // Reached an opaque cell: stop the column, leaving the
                    // cells below for the sideways pass to deal with.
                    break;
                }
                light.set_sky(pos, MAX_LIGHT);
            }
        }
    }
}

/// Light already present next door seeps in, losing one level crossing over.
fn seed_from_neighbors(
    light: &mut SectionLight,
    blocks: &dyn BlockOpacity,
    neighbors: &LightNeighbors<'_>,
) {
    for x in 0..LAST + 1 {
        for z in 0..LAST + 1 {
            for y in 0..LAST + 1 {
                let pos = LocalPos::from_array([x, y, z]);
                if blocks.opaque(pos) {
                    continue;
                }

                for (index, _) in DIRECTIONS.iter().enumerate() {
                    // `across` returns None unless this cell is on the face
                    // bordering that neighbour.
                    let Some((across, neighbor)) = neighbors.across(index, pos) else {
                        continue;
                    };
                    let arriving = neighbor.sky(across);
                    if arriving == 0 {
                        continue;
                    }
                    let carried = arriving - 1;
                    if light.sky(pos) < carried {
                        light.set_sky(pos, carried);
                    }
                }
            }
        }
    }
}

/// Floods light from every lit cell to its darker neighbours, staying inside
/// this section and never passing through an opaque cell.
fn spread_inward(light: &mut SectionLight, blocks: &dyn BlockOpacity) {
    let mut queue: VecDeque<LocalPos> = VecDeque::new();

    for x in 0..LAST + 1 {
        for z in 0..LAST + 1 {
            for y in 0..LAST + 1 {
                let pos = LocalPos::from_array([x, y, z]);
                if light.sky(pos) > 1 {
                    queue.push_back(pos);
                }
            }
        }
    }

    while let Some(pos) = queue.pop_front() {
        let value = light.sky(pos);
        if value <= 1 {
            // Nothing to give: the brightest result of spreading here is 0,
            // which is what the target already holds.
            continue;
        }
        let dimmed = value - 1;
        let [x, y, z] = pos.to_array();
        // i32 for the arithmetic: an offset can point outside the section, and
        // u8 would wrap around instead of failing the bounds check below.
        let (x, y, z) = (i32::from(x), i32::from(y), i32::from(z));

        for (dx, dy, dz) in DIRECTIONS {
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            if nx < 0
                || ny < 0
                || nz < 0
                || nx > i32::from(LAST)
                || ny >= SECTION_DIM
                || nz > i32::from(LAST)
            {
                continue;
            }
            let target = LocalPos::from_array([nx as u8, ny as u8, nz as u8]);
            if blocks.opaque(target) || light.sky(target) >= dimmed {
                continue;
            }
            light.set_sky(target, dimmed);
            queue.push_back(target);
        }
    }
}

#[cfg(test)]
mod tests;
