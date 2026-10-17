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
mod tests {
    use super::*;

    /// A hand-built grid of opaque cells, so the rules can be tested without
    /// a registry or a generated world.
    struct Grid {
        opaque: Vec<bool>,
    }

    impl Grid {
        fn empty() -> Self {
            Self {
                opaque: vec![false; SECTION_VOLUME],
            }
        }

        /// Fills a solid slab at `y`, spanning the whole section.
        fn slab(&mut self, y: u8) {
            for x in 0..LAST + 1 {
                for z in 0..LAST + 1 {
                    self.opaque[LocalPos::from_array([x, y, z]).index()] = true;
                }
            }
        }

        fn set(&mut self, x: u8, y: u8, z: u8, opaque: bool) {
            self.opaque[LocalPos::from_array([x, y, z]).index()] = opaque;
        }

        fn sky(light: &SectionLight, x: u8, y: u8, z: u8) -> u8 {
            light.sky(LocalPos::from_array([x, y, z]))
        }
    }

    impl BlockOpacity for Grid {
        fn opaque(&self, pos: LocalPos) -> bool {
            self.opaque[pos.index()]
        }
    }

    fn compute(grid: &Grid, above_open: bool, neighbors: &LightNeighbors<'_>) -> SectionLight {
        let mut light = SectionLight::dark();
        compute_sky(&mut light, grid, neighbors, above_open);
        light
    }

    #[test]
    fn an_open_section_under_open_sky_is_fully_lit() {
        let grid = Grid::empty();
        let light = compute(&grid, true, &LightNeighbors::default());

        for y in 0..LAST + 1 {
            assert_eq!(Grid::sky(&light, 8, y, 8), MAX_LIGHT);
        }
    }

    #[test]
    fn a_section_under_a_ceiling_gets_no_sunlight() {
        let mut grid = Grid::empty();
        grid.slab(LAST);
        let light = compute(&grid, true, &LightNeighbors::default());

        // The slab blocks it and nothing arrives from the sides.
        assert_eq!(Grid::sky(&light, 8, 8, 8), 0);
    }

    #[test]
    fn sunlight_stops_at_the_first_opaque_block_and_stays_stopped() {
        let mut grid = Grid::empty();
        grid.slab(8);

        let above = compute(&grid, true, &LightNeighbors::default());
        assert_eq!(Grid::sky(&above, 8, 15, 8), MAX_LIGHT, "above the slab");
        assert_eq!(Grid::sky(&above, 8, 8, 8), 0, "the slab itself is dark");
        assert_eq!(Grid::sky(&above, 8, 0, 8), 0, "under the slab");

        // Filling the section below from a dark section must not invent light.
        let below = compute(&grid, false, &LightNeighbors::default());
        assert_eq!(Grid::sky(&below, 8, 15, 8), 0);
    }

    #[test]
    fn sideways_spread_loses_exactly_one_level_per_block() {
        let mut grid = Grid::empty();
        // A roof over everything except the z=15 edge, so the only light under
        // it has to travel in from that one opening.
        for z in 0..LAST {
            for x in 0..LAST + 1 {
                grid.set(x, LAST, z, true);
            }
        }
        let light = compute(&grid, true, &LightNeighbors::default());

        assert_eq!(Grid::sky(&light, 8, 8, LAST), MAX_LIGHT, "the opening");

        // Sixteen blocks deep is exactly what the 15 levels can cover: the cell
        // furthest under the roof is dim but not black. Checking the whole
        // gradient rather than one dark spot is what makes this an assertion
        // about the rule instead of about one convenient number.
        for z in 0..LAST {
            assert_eq!(
                Grid::sky(&light, 8, 8, z),
                z,
                "at z={z}, {LAST} steps of travel from the opening"
            );
        }
    }

    #[test]
    fn light_does_not_pass_through_an_opaque_cell() {
        let mut grid = Grid::empty();
        // Solid rock with a hollow 3x3x3 room sealed inside it. The room has
        // no opening anywhere, so any light inside would mean light is passing
        // through a block.
        for y in 0..LAST + 1 {
            for z in 0..LAST + 1 {
                for x in 0..LAST + 1 {
                    let inside =
                        (7..=9).contains(&x) && (7..=9).contains(&y) && (7..=9).contains(&z);
                    grid.set(x, y, z, !inside);
                }
            }
        }

        let light = compute(&grid, true, &LightNeighbors::default());

        for x in 7..=9 {
            for y in 7..=9 {
                for z in 7..=9 {
                    assert_eq!(
                        Grid::sky(&light, x, y, z),
                        0,
                        "the sealed room lit up at [{x}, {y}, {z}]"
                    );
                }
            }
        }
    }

    #[test]
    fn light_crosses_a_section_boundary_from_a_neighbour() {
        let grid = Grid::empty();
        let neighbor = SectionLight::open_sky();

        let neighbors = LightNeighbors {
            pos_x: Some(&neighbor),
            ..LightNeighbors::default()
        };
        // Nothing above, so the only light available is the neighbour's.
        let light = compute(&grid, false, &neighbors);

        assert_eq!(
            Grid::sky(&light, 15, 8, 8),
            MAX_LIGHT - 1,
            "the cell against the boundary takes the neighbour's light"
        );
        assert_eq!(
            Grid::sky(&light, 14, 8, 8),
            MAX_LIGHT - 2,
            "and the next one loses another level"
        );
        assert_eq!(
            Grid::sky(&light, 0, 8, 8),
            0,
            "fifteen steps of dimming exhausts the levels, so the far face is dark"
        );
    }

    #[test]
    fn only_the_faces_with_a_loaded_neighbour_take_light_from_it() {
        let grid = Grid::empty();
        let neighbors = LightNeighbors {
            pos_x: Some(&SectionLight::open_sky()),
            // The other five stay None, which is what an unloaded chunk or an
            // unwritten section looks like. If any of them were treated as
            // open sky, light would appear on a face it has no business
            // reaching.
            ..LightNeighbors::default()
        };
        let light = compute(&grid, false, &neighbors);

        // The whole x=15 plane touches the loaded neighbour.
        for (y, z) in [(8, 0), (8, 8), (8, 15), (0, 8), (15, 8)] {
            assert_eq!(
                Grid::sky(&light, 15, y, z),
                MAX_LIGHT - 1,
                "x=15 at [{y}, {z}] borders the loaded section"
            );
        }
        // The far face cannot be reached: 14 levels spent over 15 steps.
        assert_eq!(Grid::sky(&light, 0, 8, 8), 0, "nothing loaded reaches x=0");
    }

    #[test]
    fn neighbour_cells_are_remapped_across_the_boundary() {
        let mut grid = Grid::empty();
        // Solid everywhere except the single cell being tested, so the only way
        // it can be lit is by being seeded directly. Leaving its neighbours
        // clear would let it light up its own column and blur the result.
        for y in 0..LAST + 1 {
            for z in 0..LAST + 1 {
                for x in 0..LAST + 1 {
                    grid.set(x, y, z, !(x == 15 && y == 5 && z == 9));
                }
            }
        }

        // A neighbour lit at its own (x=0, y=5, z=9) must light our
        // (x=15, y=5, z=9). Remapping z wrongly would light our z=8 or z=10
        // instead, which still looks plausible but is visibly offset.
        let mut neighbor = SectionLight::dark();
        neighbor.set_sky(LocalPos::from_array([0, 5, 9]), 10);

        let neighbors = LightNeighbors {
            pos_x: Some(&neighbor),
            ..LightNeighbors::default()
        };
        let light = compute(&grid, false, &neighbors);

        assert_eq!(
            Grid::sky(&light, 15, 5, 9),
            9,
            "our x=15 faces the neighbour's x=0 at the same y and z"
        );
        for z in [0u8, 5, 8, 10, 15] {
            assert_eq!(
                Grid::sky(&light, 15, 5, z),
                0,
                "z={z} is not where the neighbour's light came from"
            );
        }
        for y in [0u8, 4, 6, 15] {
            assert_eq!(
                Grid::sky(&light, 15, y, 9),
                0,
                "y={y} is not where the neighbour's light came from"
            );
        }
    }

    #[test]
    fn a_uniform_section_costs_no_dense_storage() {
        assert!(SectionLight::open_sky().is_uniform());
        assert!(SectionLight::dark().is_uniform());
        assert_eq!(SectionLight::dark().nbytes(), 0);

        let mut varied = SectionLight::dark();
        varied.set_sky(LocalPos::from_array([0, 0, 0]), 3);
        assert!(!varied.is_uniform());
        assert!(varied.nbytes() > 0);
    }

    #[test]
    fn combined_light_takes_the_brighter_layer() {
        let mut light = SectionLight::dark();
        let pos = LocalPos::from_array([4, 4, 4]);

        light.set_sky(pos, 12);
        assert_eq!(light.combined(pos), 12);

        light.set_block(pos, 3);
        assert_eq!(light.combined(pos), 12, "the sun still wins when brighter");

        light.set_sky(pos, 1);
        assert_eq!(light.combined(pos), 3, "but a torch beats a dark sky");
    }
}
