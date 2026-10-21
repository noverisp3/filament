//! The light engine: the driver that lights every loaded chunk at once.
//!
//! `world::light` knows the *rules* for one section; this knows the *order*.
//! Light crosses section and chunk boundaries, so lighting each section on its
//! own leaves a visible seam at every border.
//!
//! The loaded world is flattened into one grid with a chunk of opaque border
//! around it: a neighbour is then a fixed arithmetic offset away, and light
//! cannot run off the edge without a special case. Light falls off after
//! fifteen blocks and a chunk is sixteen wide, so one chunk of border is
//! comfortably enough.

use super::chunk::{MIN_BLOCK_Y, SECTIONS_PER_CHUNK, WORLD_HEIGHT};
use super::light::MAX_LIGHT;
use super::pos::{BlockPos, ChunkPos};
use super::pos::{LocalPos, SECTION_DIM, SectionPos};
use super::level::World;

/// Lowest section Y, which is `MIN_BLOCK_Y` rounded down to a section.
const MIN_SECTION: i32 = MIN_BLOCK_Y / SECTION_DIM;

#[cfg(test)]
thread_local! {
    /// Whether a section of one block is read in one go rather than cell by
    /// cell.
    ///
    /// Off by a test, so the two ways can be compared on the same world. It is
    /// per thread, because the tests run side by side and one of them turning
    /// the fast path off must not turn it off for the others.
    static FAST_PATH: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    /// How many sections have been read the fast way.
    static FAST_PATH_CELLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether the fast path is in use.
#[inline]
fn fast_path() -> bool {
    #[cfg(test)]
    {
        FAST_PATH.with(|on| on.get())
    }
    #[cfg(not(test))]
    {
        true
    }
}

/// Turns the fast path off, for a test that compares the two.
#[cfg(test)]
pub(crate) fn set_fast_path(on: bool) {
    FAST_PATH.with(|flag| flag.set(on));
}

/// How many sections the fast path has read since the counter was last read.
#[cfg(test)]
pub(crate) fn fast_path_cells() -> usize {
    FAST_PATH_CELLS.with(|count| count.replace(0))
}

impl World {
    /// Recomputes the light of every loaded chunk.
    ///
    /// Three passes over a flat grid: sunlight falls down each column until an
    /// opaque block stops it, the cells that could brighten a neighbour are
    /// queued into sixteen buckets, and the buckets drain from level 15 down.
    ///
    /// Draining in level order is what makes this correct rather than usually
    /// right: a plain queue would visit a dim cell first and never revisit it
    /// once a brighter path arrived.
    ///
    /// The sections whose light moved are returned and recorded, so that a
    /// client is told about them.
    pub fn compute_light(&mut self) -> Vec<SectionPos> {
        let mut grid = Grid::new(self.chunk_positions());
        grid.fill_opacity(self);
        grid.fall_sunlight();
        grid.spread_sideways();
        grid.spread_block_light();
        let dirty = grid.write_back(self);
        self.light_changes.extend(dirty.iter().copied());
        self.light_stale = false;
        dirty
    }

    /// Whether the light of the world is out of date.
    pub fn light_is_stale(&self) -> bool {
        self.light_stale
    }

    /// Sky light at a world position, or 0 outside the loaded world.
    pub fn sky_light(&self, pos: super::pos::BlockPos) -> u8 {
        self.chunk(pos.chunk_pos())
            .map_or(0, |chunk| chunk.sky_light(pos))
    }

    /// Block light at a world position, or 0 outside the loaded world.
    pub fn block_light(&self, pos: super::pos::BlockPos) -> u8 {
        self.chunk(pos.chunk_pos())
            .map_or(0, |chunk| chunk.block_light(pos))
    }

    /// The sideways spread of the light around a chunk that has just arrived.
    ///
    /// The columns are right the moment a chunk arrives and the sideways
    /// spread is not: light that comes in from a lit column next door needs
    /// somewhere to come in to.
    pub fn spread_chunk(&mut self, pos: ChunkPos) {
        // The middle of the chunk, and a reach that covers the chunk itself
        // and a lit neighbour's worth of ground around it.
        let center = super::pos::BlockPos::new(
            pos.x * SECTION_DIM + SECTION_DIM / 2,
            0,
            pos.z * SECTION_DIM + SECTION_DIM / 2,
        );
        let half_chunk = SECTION_DIM / 2;
        let mut region = Region::around_with_reach(self, center, half_chunk + LIGHT_REACH);
        self.light_work += region.cells();
        region.recompute();
        region.write_back(self);
    }

    /// Works the light out again on both sides of the border between two chunks.
    ///
    /// The cells whose light can have changed are the ones within reach of the
    /// border, so the region is centred on the border and not on either chunk.
    /// A region around a whole chunk reaches sixteen blocks past its own edge
    /// and so covers the chunk next door twice over.
    pub fn spread_border(&mut self, from: ChunkPos, to: ChunkPos) {
        // The middle of the border: half way between the two chunks' centres.
        let middle_x = from.x * SECTION_DIM + SECTION_DIM / 2 + (to.x - from.x) * SECTION_DIM / 2;
        let middle_z = from.z * SECTION_DIM + SECTION_DIM / 2 + (to.z - from.z) * SECTION_DIM / 2;
        let border = super::pos::BlockPos::new(middle_x, 0, middle_z);

        // Sixteen blocks either side of the border: a lamp's light travels
        // fifteen, and a chunk is sixteen wide, so that covers both chunks the
        // border is between.
        let mut region = Region::around_with_reach(self, border, LIGHT_REACH + 1);
        self.light_work += region.cells();
        region.recompute();
        region.write_back(self);
    }

    /// Recomputes sky light around one edit and reports which sections moved.
    ///
    /// Called after every block change, because the mesher reads light per
    /// vertex: without it a dug hole keeps the shading of solid rock and shows
    /// up as a black pit with no sun in it.
    pub fn relight_around(&mut self, center: super::pos::BlockPos) -> Vec<SectionPos> {
        let mut region = Region::around(self, center);
        self.light_work += region.cells();
        region.recompute();
        region.write_back(self)
    }

    /// The sky light of one chunk, computed the way the game computes it when
    /// a chunk arrives.
    ///
    /// The game does not keep the light in the chunk file: it works it out
    /// again when the chunk is loaded, because the light of a chunk depends on
    /// the chunks around it.
    ///
    /// It is the columns only. The light that comes in sideways from a lit
    /// column next door is worked out by [`Self::relight_around`] when a block
    /// changes, and by [`Self::spread_chunk`] when a chunk arrives.
    pub(crate) fn light_chunk_columns(&mut self, pos: ChunkPos) {
        // For each column, the height at which the sun stops: the first
        // opaque block from the top, or the bottom of the world if there is
        // none.
        let stops: Vec<i32> = {
            let Some(chunk) = self.chunk(pos) else {
                return;
            };
            let registry = self.registry();
            let mut stops = vec![i32::MAX; SECTION_DIM as usize * SECTION_DIM as usize];
            for lz in 0..SECTION_DIM {
                for lx in 0..SECTION_DIM {
                    let mut stop = i32::MAX;
                    'outer: for slot in (0..SECTIONS_PER_CHUNK).rev() {
                        let section = &chunk.sections()[slot];
                        for ly in (0..SECTION_DIM).rev() {
                            let local = LocalPos::from_array([lx as u8, ly as u8, lz as u8]);
                            if registry.is_opaque(section.get(local)) {
                                stop = (MIN_SECTION + slot as i32) * SECTION_DIM + ly;
                                break 'outer;
                            }
                        }
                    }
                    stops[lz as usize * SECTION_DIM as usize + lx as usize] = stop;
                }
            }
            stops
        };

        // And the light written back: fifteen above the stop, nothing below.
        //
        // A section that is entirely lit or entirely dark is filled in one
        // go, because a section is uniform most of the time and setting
        // sixteen thousand cells one at a time is the slow way to say so.
        for slot in 0..SECTIONS_PER_CHUNK {
            let section_y = MIN_SECTION + slot as i32;
            let section_pos = SectionPos::new(pos.x, section_y, pos.z);
            let base_y = section_y * SECTION_DIM;
            let top = base_y + SECTION_DIM;

            // The sun stops at the opaque block itself, so everything above it
            // is lit and everything from it down is not.
            let all_lit = stops.iter().all(|stop| *stop < base_y);
            let all_dark = stops.iter().all(|stop| *stop >= top);

            let Some(chunk) = self.chunk_mut(pos) else {
                return;
            };
            let Some(light) = chunk.section_light_mut(section_pos) else {
                continue;
            };
            if all_lit {
                light.fill_sky(MAX_LIGHT);
                continue;
            }
            if all_dark {
                light.fill_sky(0);
                continue;
            }
            for lz in 0..SECTION_DIM {
                for lx in 0..SECTION_DIM {
                    let stop = stops[lz as usize * SECTION_DIM as usize + lx as usize];
                    for ly in 0..SECTION_DIM {
                        let y = base_y + ly;
                        let value = if y > stop { MAX_LIGHT } else { 0 };
                        light.set_sky(
                            LocalPos::from_array([lx as u8, ly as u8, lz as u8]),
                            value,
                        );
                    }
                }
            }
        }
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
    /// The light the blocks themselves give off, before it spreads.
    emission: Vec<u8>,
    sky: Vec<u8>,
    block: Vec<u8>,
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
            emission: vec![0; width * depth * height],
            sky: vec![0; width * depth * height],
            block: vec![0; width * depth * height],
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
            (x + 1 < self.width).then(|| cell + 1),
            (x > 0).then(|| cell - 1),
            (y + 1 < self.height).then(|| cell + self.plane),
            (y > 0).then(|| cell - self.plane),
            (z + 1 < self.depth).then(|| cell + self.width),
            (z > 0).then(|| cell - self.width),
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
                            self.emission[row + local_x] = registry.light_emission(block);
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

    /// Spreads the light the blocks themselves give off.
    ///
    /// Block light is not the sun: it starts at the blocks that glow and loses
    /// one level for every block it crosses, downwards included. A glowstone is
    /// fifteen, so the cell next to it is fourteen and fifteen cells away there
    /// is nothing left.
    ///
    /// The drain is by level, from the brightest down, for the same reason the
    /// sun's spread is: a cell only ever pushes into a strictly lower level.
    fn spread_block_light(&mut self) {
        let mut buckets: Vec<Vec<usize>> = (0..=MAX_LIGHT as usize).map(|_| Vec::new()).collect();

        // The sources first: a block that glows is lit as brightly as it glows,
        // whatever else is around it.
        for cell in 0..self.block.len() {
            let emission = self.emission[cell];
            if emission > 0 {
                self.block[cell] = emission;
                buckets[emission as usize].push(cell);
            }
        }

        for level in (2..=MAX_LIGHT as usize).rev() {
            let dimmed = level - 1;
            let mut next = 0;
            while next < buckets[level].len() {
                let cell = buckets[level][next];
                next += 1;

                // Queued at `level` but improved since, so a brighter path has
                // already dealt with it.
                if self.block[cell] as usize != level {
                    continue;
                }

                for target in self.neighbors(cell).into_iter().flatten() {
                    if self.opacity[target] == 0 && (self.block[target] as usize) < dimmed {
                        self.block[target] = dimmed as u8;
                        buckets[dimmed].push(target);
                    }
                }
            }
        }
    }

    /// Copies the computed light back into each chunk's sections.
    ///
    /// The sections whose light moved are returned, because a client that was
    /// told the old light is holding a world that no longer exists.
    fn write_back(&self, world: &mut World) -> Vec<SectionPos> {
        let mut dirty: Vec<SectionPos> = Vec::new();
        for pos in world.chunk_positions() {
            let base_x = (pos.x * SECTION_DIM - self.origin_x) as usize;
            let base_z = (pos.z * SECTION_DIM - self.origin_z) as usize;

            let Some(chunk) = world.chunk_mut(pos) else {
                continue;
            };

            for slot in 0..SECTIONS_PER_CHUNK {
                let section_pos = SectionPos::new(pos.x, MIN_SECTION + slot as i32, pos.z);
                let base_y = (section_pos.y * SECTION_DIM - MIN_BLOCK_Y) as usize;
                let mut section_dirty = false;

                for local_y in 0..SECTION_DIM as usize {
                    for local_z in 0..SECTION_DIM as usize {
                        let row = self.index(base_x, base_y + local_y, base_z + local_z);
                    for local_x in 0..SECTION_DIM as usize {
                        let value = self.sky[row + local_x];
                        let local = LocalPos::from_array([
                            local_x as u8,
                            local_y as u8,
                            local_z as u8,
                        ]);
                        if chunk.sky_light_in(section_pos, local) != value {
                            chunk.sky_light_mut(section_pos, local, value);
                            section_dirty = true;
                        }

                        let value = self.block[row + local_x];
                        if chunk.block_light_in(section_pos, local) != value {
                            chunk.block_light_mut(section_pos, local, value);
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

/// How far sky light can travel, in blocks.
///
/// Light loses one level per step and tops out at [`MAX_LIGHT`], so no change
/// can reach further than this. The region to relight and the width of its
/// border both follow from it.
pub const LIGHT_REACH: i32 = MAX_LIGHT as i32;

/// A sub-volume of the world, used to relight the neighbourhood of one edit.
///
/// The whole world as a single [`Grid`] costs a couple of hundred milliseconds,
/// which is fine at startup and useless when a click should feel instant.
///
/// The margin is at least [`LIGHT_REACH`] wider than the edited cell, because a
/// cell further away cannot be connected to it by any light path. That ring
/// keeps the value it already had and serves as a source for light coming in.
struct Region {
    /// World X and Z of column 0 and row 0.
    origin_x: i32,
    origin_z: i32,
    width: usize,
    depth: usize,
    height: usize,
    plane: usize,
    opacity: Vec<u8>,
    emission: Vec<u8>,
    sky: Vec<u8>,
    block: Vec<u8>,
    /// Cells in the margin ring, which are not written back.
    fixed: Vec<bool>,
}

impl Region {
    /// A region around one cell, reaching as far as light can.
    fn around(world: &World, center: BlockPos) -> Self {
        Self::around_with_reach(world, center, LIGHT_REACH)
    }

    /// A region around one cell, reaching `reach` blocks in each direction.
    ///
    /// A cell further away than the reach cannot be connected to the centre by
    /// any light path, so its value cannot have changed.
    fn around_with_reach(world: &World, center: BlockPos, reach: i32) -> Self {
        // One wider than the reach on each side, so the edited cell is never in
        // the margin.
        let half = reach + 1;
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
            emission: vec![0; width * depth * height],
            sky: vec![0; width * depth * height],
            block: vec![0; width * depth * height],
            fixed: vec![false; width * depth * height],
        };

        region.fill_from_world(world);
        region
    }

    #[inline]
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (y * self.depth + z) * self.width + x
    }

    /// How many cells the region holds.
    ///
    /// The light is worked out for every one of them, so this is the work the
    /// region is: a million cells is a chunk arriving next to a lamp.
    fn cells(&self) -> usize {
        self.width * self.depth * self.height
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
            (x + 1 < self.width).then(|| cell + 1),
            (x > 0).then(|| cell - 1),
            (y + 1 < self.height).then(|| cell + self.plane),
            (y > 0).then(|| cell - self.plane),
            (z + 1 < self.depth).then(|| cell + self.width),
            (z > 0).then(|| cell - self.width),
        ]
    }

    /// Reads opacity and the current light for every cell of the region,
    /// straight out of the chunk data rather than through the world queries,
    /// which would be a hash lookup per cell.
    ///
    /// The sky is not read: the pass that works the sun's column out writes
    /// every cell of the region anyway, so a sky read here would be thrown away
    /// a moment later.
    fn fill_from_world(&mut self, world: &World) {
        let registry = world.registry();
        let last_x = self.width - 1;
        let last_z = self.depth - 1;

        for (chunk_x, chunk_z) in self.chunks_covered() {
            let Some(chunk) = world.chunk(ChunkPos::new(chunk_x, chunk_z)) else {
                continue;
            };
            let Some((x0, x1, z0, z1)) = self.overlap(chunk_x, chunk_z) else {
                continue;
            };
            // The chunk's own first column and row, in region coordinates.
            //
            // A chunk that hangs over the edge of the region starts before it,
            // so the cells it covers inside are not its first columns.
            // Measuring from the chunk's own edge is what puts the light back
            // in the column it belongs to.
            let (chunk_x0, chunk_z0) = self.chunk_origin(chunk_x, chunk_z);

            for (slot, section) in chunk.sections().iter().enumerate() {
                let section_y = MIN_SECTION + slot as i32;
                let base_y = (section_y * SECTION_DIM - MIN_BLOCK_Y) as usize;
                if base_y >= self.height {
                    continue;
                }
                let light = chunk.section_light(SectionPos::new(chunk_x, section_y, chunk_z));

                // The common case: one block and one light value for the whole
                // section, so the answers are looked up once for the section
                // rather than once for each of its sixteen thousand cells.
                let uniform = if !fast_path() {
                    None
                } else {
                    match (section.uniform_block(), light) {
                        (Some(block), Some(light)) => light
                            .uniform_block_light()
                            .map(|block_light| {
                                (
                                    u8::from(registry.is_opaque(block)),
                                    registry.light_emission(block),
                                    block_light,
                                )
                            }),
                        (Some(block), None) => Some((
                            u8::from(registry.is_opaque(block)),
                            registry.light_emission(block),
                            0,
                        )),
                        _ => None,
                    }
                };

                #[cfg(test)]
                if uniform.is_some() {
                    FAST_PATH_CELLS.with(|count| count.set(count.get() + 1));
                }

                for gy in base_y..(base_y + SECTION_DIM as usize).min(self.height) {
                    let local_y = (gy - base_y) as u8;
                    for gz in z0..z1 {
                        let row = self.index(x0, gy, gz);
                        for gx in x0..x1 {
                            let cell = row + (gx - x0);
                            match uniform {
                                Some((opacity, emission, block_light)) => {
                                    self.opacity[cell] = opacity;
                                    self.emission[cell] = emission;
                                    self.block[cell] = block_light;
                                }
                                None => {
                                    let local = LocalPos::from_array([
                                        (gx as i32 - chunk_x0) as u8,
                                        local_y,
                                        (gz as i32 - chunk_z0) as u8,
                                    ]);
                                    let block = section.get(local);
                                    self.opacity[cell] = u8::from(registry.is_opaque(block));
                                    self.emission[cell] = registry.light_emission(block);
                                    self.block[cell] =
                                        light.map_or(0, |light| light.block(local));
                                }
                            }

                            // The margin ring keeps whatever it already had, so
                            // it only ever acts as a source for light coming in.
                            self.fixed[cell] = gx == 0 || gz == 0 || gx == last_x || gz == last_z;
                        }
                    }
                }
            }
        }
    }

    /// The half-open range of region cells this chunk covers, as
    /// `(x0, x1, z0, z1)`, or `None` when it does not reach the region.
    ///
    /// Clamping the ranges up front keeps a chunk sitting partly off the edge
    /// from indexing before the bounds check.
    fn overlap(&self, chunk_x: i32, chunk_z: i32) -> Option<(usize, usize, usize, usize)> {
        let dim = SECTION_DIM;

        // The part of the chunk that is inside the region, in world coordinates
        // and then in the region's own. A chunk that hangs over the edge still
        // covers the part of it that is inside: refusing it whole left the light
        // stopping dead at the chunk boundary.
        let span = |chunk_start: i32, origin: i32, size: usize| -> Option<(usize, usize)> {
            let start = chunk_start.max(origin);
            let end = (chunk_start + dim).min(origin + size as i32);
            if start >= end {
                return None;
            }
            Some(((start - origin) as usize, (end - origin) as usize))
        };

        let (x0, x1) = span(chunk_x * dim, self.origin_x, self.width)?;
        let (z0, z1) = span(chunk_z * dim, self.origin_z, self.depth)?;
        Some((x0, x1, z0, z1))
    }

    /// The region cell that holds a chunk's first column and first row.
    ///
    /// The region's own origin is wherever the edit put it, usually in the
    /// middle of a chunk, so a chunk that hangs over the edge of the region
    /// starts before the region does.
    fn chunk_origin(&self, chunk_x: i32, chunk_z: i32) -> (i32, i32) {
        (
            chunk_x * SECTION_DIM - self.origin_x,
            chunk_z * SECTION_DIM - self.origin_z,
        )
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

    /// Recomputes the light inside the region, leaving the margin untouched.
    ///
    /// The downward pass runs over the region's full height so the sun column
    /// is exact rather than seeded from a guess.
    fn recompute(&mut self) {
        self.recompute_sky();
        self.recompute_block();
    }

    /// The light the blocks themselves give off, worked out again inside the
    /// region.
    ///
    /// The margin keeps the value it had, so it is the light arriving from
    /// outside the region, and the sources inside it are the blocks that glow.
    /// Everything else is worked out from those two.
    fn recompute_block(&mut self) {
        // The interior is worked out again; the margin keeps what it had.
        for cell in 0..self.block.len() {
            if !self.fixed[cell] {
                self.block[cell] = 0;
            }
        }

        let mut buckets: Vec<Vec<usize>> = (0..=MAX_LIGHT as usize).map(|_| Vec::new()).collect();

        // The sources: a block that glows is lit as brightly as it glows.
        for cell in 0..self.block.len() {
            let emission = self.emission[cell];
            if emission > 0 && self.block[cell] < emission {
                self.block[cell] = emission;
            }
        }

        // Everything that is lit seeds the spread, the margin included.
        for cell in 0..self.block.len() {
            let level = self.block[cell] as usize;
            if level >= 2 {
                buckets[level].push(cell);
            }
        }

        for level in (2..=MAX_LIGHT as usize).rev() {
            let dimmed = level - 1;
            let mut next = 0;
            while next < buckets[level].len() {
                let cell = buckets[level][next];
                next += 1;
                if self.block[cell] as usize != level {
                    continue;
                }
                for target in self.neighbors(cell).into_iter().flatten() {
                    if self.opacity[target] == 0 && (self.block[target] as usize) < dimmed {
                        self.block[target] = dimmed as u8;
                        buckets[dimmed].push(target);
                    }
                }
            }
        }
    }

    fn recompute_sky(&mut self) {
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
            // The chunk's own first column and row, in region coordinates: the
            // light goes back to the column it was read from, not to the
            // column the region happens to start at.
            let (chunk_x0, chunk_z0) = self.chunk_origin(chunk_x, chunk_z);

            // The chunk once, not once per cell.
            //
            // A region is a million cells, and looking the chunk up in the
            // world's map for every one of them is a million hash lookups for
            // a thing that is the same a million times over. It was the whole
            // of the cost of writing the light back.
            let Some(chunk) = world.chunk_mut(chunk_pos) else {
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
                        let row = self.index(x0, gy, gz);
                        for gx in x0..x1 {
                            let cell = row + (gx - x0);
                            if self.fixed[cell] {
                                continue;
                            }
                            let value = self.sky[cell];
                            let local = LocalPos::from_array([
                                (gx as i32 - chunk_x0) as u8,
                                local_y,
                                (gz as i32 - chunk_z0) as u8,
                            ]);
                            if chunk.sky_light_in(section_pos, local) != value {
                                chunk.sky_light_mut(section_pos, local, value);
                                section_dirty = true;
                            }

                            let value = self.block[cell];
                            if chunk.block_light_in(section_pos, local) != value {
                                chunk.block_light_mut(section_pos, local, value);
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
