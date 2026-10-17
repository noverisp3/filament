//! World-level sky light: the driver that lights every loaded chunk at once.
//!
//! `world::light` knows the *rules* for one section. This knows the *order*, and
//! the order is the whole difficulty: light crosses section and chunk
//! boundaries, so lighting each section independently leaves a visible seam at
//! every border.
//!
//! The approach is to stop thinking in sections. The loaded world is flattened
//! into one grid with a one-chunk border of opaque cells around it, which buys
//! two things: a neighbour is a fixed arithmetic offset away, and light cannot
//! run off the edge of the loaded area without a special case. Light falls off
//! after fifteen blocks and a chunk is sixteen wide, so one chunk of border is
//! comfortably more than enough.

use super::chunk::{MIN_BLOCK_Y, SECTIONS_PER_CHUNK, WORLD_HEIGHT};
use super::light::MAX_LIGHT;
use super::pos::{BlockPos, ChunkPos};
use super::pos::{LocalPos, SECTION_DIM, SectionPos};
use super::world::World;

/// Lowest section Y, which is `MIN_BLOCK_Y` rounded down to a section.
const MIN_SECTION: i32 = MIN_BLOCK_Y / SECTION_DIM;

impl World {
    /// Recomputes sky light for every loaded chunk.
    ///
    /// Three passes over a flat grid:
    ///
    /// 1. Sunlight falls straight down each column, undimmed, until an opaque
    ///    block stops it.
    /// 2. Cells that could still brighten a neighbour are queued into sixteen
    ///    buckets, one per level.
    /// 3. Buckets drain from level 15 down to 2, each cell pushing one level
    ///    lower into its six neighbours.
    ///
    /// Draining in level order is what makes this correct rather than merely
    /// usually right. A plain queue would visit a dim cell first, find nothing
    /// better, and never revisit it once a brighter path arrived.
    pub fn compute_light(&mut self) {
        let mut grid = Grid::new(self.chunk_positions());
        grid.fill_opacity(self);
        grid.fall_sunlight();
        grid.spread_sideways();
        grid.write_back(self);
    }

    /// Sky light at a world position, or 0 outside the loaded world.
    pub fn sky_light(&self, pos: super::pos::BlockPos) -> u8 {
        self.chunk(pos.chunk_pos())
            .map_or(0, |chunk| chunk.sky_light(pos))
    }

    /// Block light at a world position, or 0 outside the loaded world.
    ///
    /// Always 0 for now: no block emits light yet, so the layer exists but has
    /// no sources.
    pub fn block_light(&self, pos: super::pos::BlockPos) -> u8 {
        self.chunk(pos.chunk_pos())
            .map_or(0, |chunk| chunk.block_light(pos))
    }

    /// Recomputes sky light around one edit and reports which sections moved.
    ///
    /// Called after every block change, because the mesher reads light per
    /// vertex: without it a dug hole keeps the shading of solid rock and shows
    /// up as a black pit with no sun in it.
    pub(crate) fn relight_around(&mut self, center: super::pos::BlockPos) -> Vec<SectionPos> {
        let mut region = Region::around(self, center);
        region.recompute();
        region.write_back(self)
    }
}

/// A flat, chunk-free view of the loaded world.
struct Grid {
    /// World X of grid column 0.
    origin_x: i32,
    /// World Z of grid row 0.
    origin_z: i32,
    width: usize,
    depth: usize,
    height: usize,
    /// Cells in one horizontal slice.
    plane: usize,
    /// Non-zero stops light. Starts entirely opaque, so the border and anything
    /// not overwritten by `fill_opacity` blocks light.
    opacity: Vec<u8>,
    sky: Vec<u8>,
}

impl Grid {
    fn new(positions: Vec<super::pos::ChunkPos>) -> Self {
        let min_x = positions.iter().map(|p| p.x).min().unwrap_or(0) - 1;
        let max_x = positions.iter().map(|p| p.x).max().unwrap_or(0) + 1;
        let min_z = positions.iter().map(|p| p.z).min().unwrap_or(0) - 1;
        let max_z = positions.iter().map(|p| p.z).max().unwrap_or(0) + 1;

        let width = (max_x - min_x + 1) as usize * SECTION_DIM as usize;
        let depth = (max_z - min_z + 1) as usize * SECTION_DIM as usize;
        let height = WORLD_HEIGHT as usize;

        Self {
            origin_x: min_x * SECTION_DIM,
            origin_z: min_z * SECTION_DIM,
            width,
            depth,
            height,
            plane: width * depth,
            opacity: vec![1; width * depth * height],
            sky: vec![0; width * depth * height],
        }
    }

    #[inline]
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (y * self.depth + z) * self.width + x
    }

    /// The six neighbours of a cell, with `None` where the grid ends.
    ///
    /// The grid border is opaque, so a `None` never has to be treated
    /// specially during the flood: the neighbour simply is not there.
    fn neighbors(&self, cell: usize) -> [Option<usize>; 6] {
        let x = cell % self.width;
        let rest = cell / self.width;
        let z = rest % self.depth;
        let y = rest / self.depth;

        [
            (x + 1 < self.width).then_some(cell + 1),
            (x > 0).then_some(cell - 1),
            (y + 1 < self.height).then_some(cell + self.plane),
            (y > 0).then_some(cell - self.plane),
            (z + 1 < self.depth).then_some(cell + self.width),
            (z > 0).then_some(cell - self.width),
        ]
    }

    /// Marks every cell belonging to a loaded chunk as blocking or not.
    fn fill_opacity(&mut self, world: &World) {
        let registry = world.registry();
        for pos in world.chunk_positions() {
            let Some(chunk) = world.chunk(pos) else {
                continue;
            };
            let base_x = (pos.x * SECTION_DIM - self.origin_x) as usize;
            let base_z = (pos.z * SECTION_DIM - self.origin_z) as usize;

            for (slot, section) in chunk.sections().iter().enumerate() {
                let section_y = MIN_SECTION + slot as i32;
                let base_y = (section_y * SECTION_DIM - MIN_BLOCK_Y) as usize;

                for local_y in 0..SECTION_DIM as usize {
                    for local_z in 0..SECTION_DIM as usize {
                        let row = self.index(base_x, base_y + local_y, base_z + local_z);
                        for local_x in 0..SECTION_DIM as usize {
                            let block = section.get(LocalPos::from_array([
                                local_x as u8,
                                local_y as u8,
                                local_z as u8,
                            ]));
                            self.opacity[row + local_x] = u8::from(registry.is_opaque(block));
                        }
                    }
                }
            }
        }
    }

    /// Sunlight falls down every column, undimmed, until a block stops it.
    ///
    /// Walks one horizontal slice at a time rather than one column at a time:
    /// descending a column would stride across the whole grid sixteen times over
    /// and miss cache on every access, whereas a slice is contiguous and the
    /// per-column state is a few tens of kilobytes.
    fn fall_sunlight(&mut self) {
        // The level arriving from the slice above, one per column.
        let mut falling = vec![MAX_LIGHT; self.plane];

        for y in (0..self.height).rev() {
            let base = y * self.plane;
            for (column, slot) in falling.iter_mut().enumerate() {
                let cell = base + column;
                if self.opacity[cell] != 0 {
                    // Opaque: this cell is dark, and so is everything below it.
                    *slot = 0;
                    self.sky[cell] = 0;
                } else {
                    self.sky[cell] = *slot;
                }
            }
        }
    }

    /// Floods light sideways and downwards, losing one level per step.
    ///
    /// Only cells that could actually brighten a neighbour are queued. Nearly
    /// the whole world is either fully lit or fully dark, and seeding all of it
    /// would push tens of millions of no-op entries through the queue.
    fn spread_sideways(&mut self) {
        let mut buckets: Vec<Vec<usize>> = (0..=MAX_LIGHT as usize).map(|_| Vec::new()).collect();

        for cell in 0..self.sky.len() {
            if self.can_brighten(cell) {
                buckets[self.sky[cell] as usize].push(cell);
            }
        }

        // Highest level first. A cell only ever pushes into a strictly lower
        // bucket, which has not been drained yet, so a bucket never grows while
        // it is being drained and needs no revisiting.
        for level in (2..=MAX_LIGHT as usize).rev() {
            let dimmed = level - 1;
            let mut next = 0;
            while next < buckets[level].len() {
                let cell = buckets[level][next];
                next += 1;

                // Queued at `level` but improved since, so a brighter path has
                // already dealt with it.
                if self.sky[cell] as usize != level {
                    continue;
                }

                for target in self.neighbors(cell).into_iter().flatten() {
                    if self.opacity[target] == 0 && (self.sky[target] as usize) < dimmed {
                        self.sky[target] = dimmed as u8;
                        buckets[dimmed].push(target);
                    }
                }
            }
        }
    }

    /// Whether any neighbour of `cell` is darker than one level below it.
    ///
    /// Cells at 0 and 1 cannot brighten anything, so they are never queued.
    fn can_brighten(&self, cell: usize) -> bool {
        let level = self.sky[cell] as usize;
        if level <= 1 {
            return false;
        }
        let dimmed = level - 1;
        self.neighbors(cell)
            .into_iter()
            .flatten()
            .any(|target| self.opacity[target] == 0 && (self.sky[target] as usize) < dimmed)
    }

    /// Copies the computed light back into each chunk's sections.
    fn write_back(&self, world: &mut World) {
        for pos in world.chunk_positions() {
            let base_x = (pos.x * SECTION_DIM - self.origin_x) as usize;
            let base_z = (pos.z * SECTION_DIM - self.origin_z) as usize;

            let Some(chunk) = world.chunk_mut(pos) else {
                continue;
            };

            for slot in 0..SECTIONS_PER_CHUNK {
                let section_pos = SectionPos::new(pos.x, MIN_SECTION + slot as i32, pos.z);
                let base_y = (section_pos.y * SECTION_DIM - MIN_BLOCK_Y) as usize;

                for local_y in 0..SECTION_DIM as usize {
                    for local_z in 0..SECTION_DIM as usize {
                        let row = self.index(base_x, base_y + local_y, base_z + local_z);
                        for local_x in 0..SECTION_DIM as usize {
                            chunk.sky_light_mut(
                                section_pos,
                                LocalPos::from_array([local_x as u8, local_y as u8, local_z as u8]),
                                self.sky[row + local_x],
                            );
                        }
                    }
                }
            }
        }
    }
}

/// How far sky light can travel, in blocks.
///
/// Light loses one level per step and tops out at [`MAX_LIGHT`], so no change
/// can reach further than this. Everything else follows from it: the region to
/// relight, and how wide its border must be.
pub const LIGHT_REACH: i32 = MAX_LIGHT as i32;

/// A sub-volume of the world, used to relight the neighbourhood of one edit.
///
/// The whole world as a single [`Grid`] costs a couple of hundred milliseconds,
/// which is fine at startup and useless when a click should feel instant. This
/// covers only what an edit can actually reach.
///
/// The margin has to be at least [`LIGHT_REACH`] wider than the edited cell, and
/// that is not a guess: a cell further away than fifteen steps from the edit
/// cannot be connected to it by any light path, so its value cannot have
/// changed. The extra ring of cells therefore keeps the value it already had and
/// only serves as a source for light spreading inwards.
struct Region {
    /// World X and Z of column 0 and row 0.
    origin_x: i32,
    origin_z: i32,
    width: usize,
    depth: usize,
    height: usize,
    plane: usize,
    opacity: Vec<u8>,
    sky: Vec<u8>,
    /// Cells in the margin ring, which are not written back.
    fixed: Vec<bool>,
}

impl Region {
    fn around(world: &World, center: BlockPos) -> Self {
        // One wider than the reach on each side, so the edited cell is never in
        // the margin.
        let half = LIGHT_REACH + 1;
        let width = (2 * half + 1) as usize;
        let depth = (2 * half + 1) as usize;
        let height = WORLD_HEIGHT as usize;

        let origin_x = center.x - half;
        let origin_z = center.z - half;
        let plane = width * depth;

        let mut region = Self {
            origin_x,
            origin_z,
            width,
            depth,
            height,
            plane,
            opacity: vec![1; width * depth * height],
            sky: vec![0; width * depth * height],
            fixed: vec![false; width * depth * height],
        };

        region.fill_from_world(world);
        region
    }

    #[inline]
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (y * self.depth + z) * self.width + x
    }

    #[inline]
    fn x_of(&self, cell: usize) -> usize {
        cell % self.width
    }

    #[inline]
    fn z_of(&self, cell: usize) -> usize {
        (cell / self.width) % self.depth
    }

    #[inline]
    fn y_of(&self, cell: usize) -> usize {
        cell / self.plane
    }

    fn neighbors(&self, cell: usize) -> [Option<usize>; 6] {
        let x = self.x_of(cell);
        let z = self.z_of(cell);
        let y = self.y_of(cell);
        [
            (x + 1 < self.width).then_some(cell + 1),
            (x > 0).then_some(cell - 1),
            (y + 1 < self.height).then_some(cell + self.plane),
            (y > 0).then_some(cell - self.plane),
            (z + 1 < self.depth).then_some(cell + self.width),
            (z > 0).then_some(cell - self.width),
        ]
    }

    /// Reads opacity and the current light for every cell of the region,
    /// straight out of the chunk data rather than through the world queries,
    /// which would be a hash lookup per cell.
    fn fill_from_world(&mut self, world: &World) {
        let registry = world.registry();

        for (chunk_x, chunk_z) in self.chunks_covered() {
            let Some(chunk) = world.chunk(ChunkPos::new(chunk_x, chunk_z)) else {
                continue;
            };
            let Some((x0, x1, z0, z1)) = self.overlap(chunk_x, chunk_z) else {
                continue;
            };

            for (slot, section) in chunk.sections().iter().enumerate() {
                let section_y = MIN_SECTION + slot as i32;
                let base_y = (section_y * SECTION_DIM - MIN_BLOCK_Y) as usize;
                if base_y >= self.height {
                    continue;
                }
                let light = chunk.section_light(SectionPos::new(chunk_x, section_y, chunk_z));

                for gy in base_y..(base_y + SECTION_DIM as usize).min(self.height) {
                    let local_y = (gy - base_y) as u8;
                    for gz in z0..z1 {
                        let local_z = (gz - z0) as u8;
                        let row = self.index(x0, gy, gz);
                        for gx in x0..x1 {
                            let local = LocalPos::from_array([(gx - x0) as u8, local_y, local_z]);
                            let cell = row + (gx - x0);
                            self.opacity[cell] = u8::from(registry.is_opaque(section.get(local)));
                            self.sky[cell] = light.map_or(0, |light| light.sky(local));

                            // The margin ring keeps whatever it already had, so
                            // it only ever acts as a source for light coming in.
                            self.fixed[cell] =
                                gx == 0 || gz == 0 || gx == self.width - 1 || gz == self.depth - 1;
                        }
                    }
                }
            }
        }
    }

    /// The half-open range of region cells this chunk covers, as
    /// `(x0, x1, z0, z1)`, or `None` when it does not reach the region.
    ///
    /// Clamping the ranges up front is what keeps a chunk sitting partly off the
    /// left or bottom edge from indexing before the bounds check.
    fn overlap(&self, chunk_x: i32, chunk_z: i32) -> Option<(usize, usize, usize, usize)> {
        let clamp = |value: i32, origin: i32, size: usize| -> Option<usize> {
            let offset = value - origin;
            if offset < 0 || offset >= size as i32 {
                None
            } else {
                Some(offset as usize)
            }
        };

        let dim = SECTION_DIM;
        let x0 = clamp(chunk_x * dim, self.origin_x, self.width)?;
        let x1 = clamp(chunk_x * dim + dim - 1, self.origin_x, self.width)? + 1;
        let z0 = clamp(chunk_z * dim, self.origin_z, self.depth)?;
        let z1 = clamp(chunk_z * dim + dim - 1, self.origin_z, self.depth)? + 1;
        Some((x0, x1, z0, z1))
    }

    /// Chunk coordinates overlapping the region. The margin is under three
    /// chunks wide, so this is a handful.
    fn chunks_covered(&self) -> Vec<(i32, i32)> {
        let first_x = (self.origin_x as f64 / SECTION_DIM as f64).floor() as i32;
        let last_x =
            ((self.origin_x + self.width as i32 - 1) as f64 / SECTION_DIM as f64).floor() as i32;
        let first_z = (self.origin_z as f64 / SECTION_DIM as f64).floor() as i32;
        let last_z =
            ((self.origin_z + self.depth as i32 - 1) as f64 / SECTION_DIM as f64).floor() as i32;

        let mut out = Vec::new();
        for x in first_x..=last_x {
            for z in first_z..=last_z {
                out.push((x, z));
            }
        }
        out
    }

    /// Recomputes sky light inside the region, leaving the margin untouched.
    ///
    /// The downward pass runs over the region's full height so the sun column is
    /// exact rather than seeded from a guess.
    fn recompute(&mut self) {
        let mut falling = vec![MAX_LIGHT; self.plane];
        for y in (0..self.height).rev() {
            let base = y * self.plane;
            for (column, slot) in falling.iter_mut().enumerate() {
                let cell = base + column;
                if self.opacity[cell] != 0 {
                    *slot = 0;
                    self.sky[cell] = 0;
                } else {
                    self.sky[cell] = *slot;
                }
            }
        }

        // Only spread inward from cells that can still brighten something. The
        // margin keeps the value it had, which is exactly what light arriving
        // from outside the region would have produced.
        let mut buckets: Vec<Vec<usize>> = (0..=MAX_LIGHT as usize).map(|_| Vec::new()).collect();
        for cell in 0..self.sky.len() {
            let level = self.sky[cell] as usize;
            if level <= 1 {
                continue;
            }
            if self
                .neighbors(cell)
                .into_iter()
                .flatten()
                .any(|target| self.opacity[target] == 0 && (self.sky[target] as usize) < level - 1)
            {
                buckets[level].push(cell);
            }
        }

        for level in (2..=MAX_LIGHT as usize).rev() {
            let dimmed = level - 1;
            let mut next = 0;
            while next < buckets[level].len() {
                let cell = buckets[level][next];
                next += 1;
                if self.sky[cell] as usize != level {
                    continue;
                }
                for target in self.neighbors(cell).into_iter().flatten() {
                    if self.opacity[target] == 0 && (self.sky[target] as usize) < dimmed {
                        self.sky[target] = dimmed as u8;
                        buckets[dimmed].push(target);
                    }
                }
            }
        }
    }

    /// Writes the recomputed light back, returning the sections that changed.
    fn write_back(&mut self, world: &mut World) -> Vec<SectionPos> {
        let mut dirty: Vec<SectionPos> = Vec::new();

        for (chunk_x, chunk_z) in self.chunks_covered() {
            let chunk_pos = ChunkPos::new(chunk_x, chunk_z);
            let Some((x0, x1, z0, z1)) = self.overlap(chunk_x, chunk_z) else {
                continue;
            };

            for slot in 0..SECTIONS_PER_CHUNK {
                let section_pos = SectionPos::new(chunk_x, MIN_SECTION + slot as i32, chunk_z);
                let base_y = (section_pos.y * SECTION_DIM - MIN_BLOCK_Y) as usize;
                if base_y >= self.height {
                    continue;
                }
                let mut section_dirty = false;

                for gy in base_y..(base_y + SECTION_DIM as usize).min(self.height) {
                    let local_y = (gy - base_y) as u8;
                    for gz in z0..z1 {
                        let local_z = (gz - z0) as u8;
                        let row = self.index(x0, gy, gz);
                        for gx in x0..x1 {
                            let cell = row + (gx - x0);
                            if self.fixed[cell] {
                                continue;
                            }
                            let value = self.sky[cell];
                            let local = LocalPos::from_array([(gx - x0) as u8, local_y, local_z]);
                            let Some(chunk) = world.chunk_mut(chunk_pos) else {
                                break;
                            };
                            if chunk.sky_light_in(section_pos, local) != value {
                                chunk.sky_light_mut(section_pos, local, value);
                                section_dirty = true;
                            }
                        }
                    }
                }

                if section_dirty && !dirty.contains(&section_pos) {
                    dirty.push(section_pos);
                }
            }
        }

        dirty
    }
}
