//! The world: chunks, the block registry, and the queries that read them.
//!
//! This sits between storage and rendering. The renderer never owns world
//! data; it borrows a `&World` when it needs geometry, which keeps the rule
//! that logic in `world/` does not depend on wgpu, and the reverse.

use std::collections::HashMap;

use super::block::{BlockId, BlockRegistry};
use super::chunk::{Chunk, MIN_SECTION_Y, SECTIONS_PER_CHUNK};
use super::pos::{BlockPos, ChunkPos, SECTION_DIM, SectionPos};
use super::raycast::{RayHit, raycast};
use super::section::{Section, SectionNeighbors};
use super::terrain::{TerrainBlocks, generate_chunk, register_terrain};

/// What changed after an edit, and therefore what has to be rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEdit {
    /// Every chunk holding a section that has to be rebuilt.
    ///
    /// More than one is normal: breaking a block on a chunk border reveals the
    /// neighbouring block's face, and that face belongs to the *other* chunk.
    /// Re-meshing only the edited chunk left a hole in the world there.
    pub chunks: Vec<ChunkPos>,
    /// Sections whose visible surface or shading may differ. Always includes the
    /// section holding the block, a neighbour when the block sits on a boundary,
    /// and every section whose light changed.
    pub sections: Vec<SectionPos>,
}

impl BlockEdit {
    pub fn touches(&self, section: SectionPos) -> bool {
        self.sections.contains(&section)
    }

    /// Chunks containing `section`, whether or not they need rebuilding.
    pub fn owns(&self, section: SectionPos) -> bool {
        self.chunks.contains(&section.chunk_pos())
    }
}

pub struct World {
    registry: BlockRegistry,
    terrain: TerrainBlocks,
    chunks: HashMap<ChunkPos, Chunk>,
}

impl Clone for World {
    /// Chunks and their light are plain data, so copying a world is copying two
    /// maps. Worth having for tests that need to run the same edit through a
    /// cheap path and an expensive one and compare the results.
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            terrain: self.terrain,
            chunks: self.chunks.clone(),
        }
    }
}

impl World {
    /// An empty world that generates chunks as they are asked for.
    ///
    /// `generate` below fills a square up front, which is what a test or a
    /// fixed map wants. A server does not know how big the map is: it learns the
    /// shape from where the player walks, so it needs a world that starts with
    /// nothing and grows.
    pub fn empty() -> Self {
        let mut registry = BlockRegistry::new();
        let terrain = register_terrain(&mut registry);
        Self {
            registry,
            terrain,
            chunks: HashMap::new(),
        }
    }

    /// The chunk at `pos`, generating and storing it if it is not there yet.
    ///
    /// The generator is deterministic, so this is not "generate on miss" in the
    /// cache-invalidating sense: a chunk generated now and the same chunk
    /// generated an hour ago are the same chunk, which is what lets a player walk
    /// away and come back to the ground they left. What changes that is a block
    /// the player placed, and that lives in the stored copy.
    ///
    /// Returns whether it was already there, so a caller can tell a fresh chunk
    /// from a stored one.
    pub fn ensure_chunk(&mut self, pos: ChunkPos) -> bool {
        if self.chunks.contains_key(&pos) {
            return true;
        }
        let chunk = generate_chunk(pos, &self.terrain);
        self.chunks.insert(pos, chunk);
        false
    }

    /// Drops a chunk and its edits.
    ///
    /// Called when the player walks out of range of it. The terrain comes back
    /// identically if they walk back, since the generator is deterministic, but
    /// anything the player built in it does not: that lived only in the stored
    /// copy, and this is the code that decides how long a build is kept.
    pub fn forget_chunk(&mut self, pos: ChunkPos) -> bool {
        self.chunks.remove(&pos).is_some()
    }

    /// Builds a world of generated terrain.
    ///
    /// Takes the chunk radius to generate along each axis.
    pub fn generate(radius: i32) -> Self {
        let mut registry = BlockRegistry::new();
        let terrain = register_terrain(&mut registry);

        let mut chunks = HashMap::new();
        for x in -radius..=radius {
            for z in -radius..=radius {
                let pos = ChunkPos::new(x, z);
                chunks.insert(pos, generate_chunk(pos, &terrain));
            }
        }

        Self {
            registry,
            terrain,
            chunks,
        }
    }

    pub fn registry(&self) -> &BlockRegistry {
        &self.registry
    }

    /// The terrain generator's block ids, for mapping a stored block onto the
    /// game's state id on the way out.
    ///
    /// The two registries are unrelated: air is zero in both by coincidence and
    /// nothing else lines up. The mapping belongs to whoever sends packets, not
    /// to storage, which is why this hands the ids over rather than doing it.
    pub fn terrain_blocks(&self) -> &TerrainBlocks {
        &self.terrain
    }

    /// Chunk positions in a stable order, so rebuilds are deterministic.
    pub fn chunk_positions(&self) -> Vec<ChunkPos> {
        let mut positions: Vec<ChunkPos> = self.chunks.keys().copied().collect();
        positions.sort_by_key(|pos| (pos.x, pos.z));
        positions
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// The block at a world position, or `None` outside the loaded world.
    pub fn block(&self, pos: BlockPos) -> Option<BlockId> {
        self.chunk(pos.chunk_pos())?.block(pos)
    }

    /// Whether a block stops a ray and hides the faces behind it.
    ///
    /// An unloaded position counts as transparent. That direction is the safe
    /// one: picking may select something in an unloaded chunk, but a hole can
    /// never be opened by editing nothing.
    pub fn is_solid(&self, pos: BlockPos) -> bool {
        match self.block(pos) {
            Some(block) => self.registry.is_opaque(block),
            None => false,
        }
    }

    /// The six sections around `section_pos`.
    ///
    /// `None` means "not loaded", which the mesher treats as air.
    pub fn section_neighbors(&self, section_pos: SectionPos) -> SectionNeighbors<'_> {
        let step = |dx: i32, dy: i32, dz: i32| -> Option<&Section> {
            let target =
                SectionPos::new(section_pos.x + dx, section_pos.y + dy, section_pos.z + dz);
            if target.y < MIN_SECTION_Y || target.y >= MIN_SECTION_Y + SECTIONS_PER_CHUNK as i32 {
                return None;
            }
            self.chunk(ChunkPos::new(target.x, target.z))?
                .section(target)
        };

        SectionNeighbors {
            pos_x: step(1, 0, 0),
            neg_x: step(-1, 0, 0),
            pos_y: step(0, 1, 0),
            neg_y: step(0, -1, 0),
            pos_z: step(0, 0, 1),
            neg_z: step(0, 0, -1),
        }
    }

    /// The 3×3×3 block of sections around `section_pos`, with their light.
    ///
    /// Six face neighbours are not enough: shading a face corner reads the
    /// diagonal block, so a face on a section boundary reaches into edge and
    /// corner neighbours. Without those, every section boundary would show a
    /// band where the shading stops.
    pub fn section_context(
        &self,
        section_pos: SectionPos,
    ) -> crate::render::mesh::SectionContext<'_> {
        use crate::render::mesh::SectionContext;

        let mut sections = [None; 27];
        let mut light = [None; 27];

        for dy in -1..=1i32 {
            for dz in -1..=1i32 {
                for dx in -1..=1i32 {
                    let target =
                        SectionPos::new(section_pos.x + dx, section_pos.y + dy, section_pos.z + dz);
                    let slot =
                        (((dx + 1) as usize) * 3 + (dy + 1) as usize) * 3 + (dz + 1) as usize;

                    // Outside the world's height is genuinely nothing, and must
                    // read as absent rather than as an empty section: the top of
                    // the world is open sky, so a phantom section there would
                    // turn it into a ceiling.
                    if target.y < MIN_SECTION_Y
                        || target.y >= MIN_SECTION_Y + SECTIONS_PER_CHUNK as i32
                    {
                        continue;
                    }
                    let Some(chunk) = self.chunk(ChunkPos::new(target.x, target.z)) else {
                        continue;
                    };
                    sections[slot] = chunk.section(target);
                    light[slot] = chunk.section_light(target);
                }
            }
        }

        SectionContext::from_parts(sections, light)
    }

    /// Writes a block and reports what has to be rebuilt.
    ///
    /// Returns `None` when the position is outside the loaded world or the
    /// write is a no-op, so an unchanged block never triggers a re-mesh.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) -> Option<BlockEdit> {
        let chunk_pos = pos.chunk_pos();
        if !self.chunks.contains_key(&chunk_pos) {
            return None;
        }
        if self.block(pos) == Some(block) {
            return None;
        }

        // `Chunk::set_block` refuses positions outside the world's Y range, and
        // ignoring that would report an edit that never happened.
        if !self.chunk_mut(chunk_pos)?.set_block(pos, block) {
            return None;
        }

        let section = pos.section_pos();
        let local = pos.local_pos();
        let mut sections = vec![section];

        // A block on a section boundary changes what the neighbour can see.
        if local.x == 0 {
            sections.push(section.offset(-1, 0, 0));
        }
        if local.x == (SECTION_DIM - 1) as u8 {
            sections.push(section.offset(1, 0, 0));
        }
        if local.y == 0 {
            sections.push(section.offset(0, -1, 0));
        }
        if local.y == (SECTION_DIM - 1) as u8 {
            sections.push(section.offset(0, 1, 0));
        }
        if local.z == 0 {
            sections.push(section.offset(0, 0, -1));
        }
        if local.z == (SECTION_DIM - 1) as u8 {
            sections.push(section.offset(0, 0, 1));
        }

        // Keep only sections that exist: a neighbour outside the world cannot
        // be rebuilt and has nothing to hide.
        sections.retain(|candidate| {
            self.chunk(ChunkPos::new(candidate.x, candidate.z))
                .is_some_and(|chunk| chunk.section(*candidate).is_some())
        });

        // Shading changed wherever the light moved, which reaches further than
        // the block edit itself.
        sections.extend(self.relight_around(pos));

        sections.sort_by_key(|section| (section.x, section.y, section.z));
        sections.dedup();

        // The chunks follow from the sections, so a section on a chunk border
        // cannot be rebuilt without its neighbour's chunk also being rebuilt.
        let mut chunks: Vec<ChunkPos> = sections
            .iter()
            .map(|section| ChunkPos::new(section.x, section.z))
            .collect();
        chunks.sort_by_key(|chunk| (chunk.x, chunk.z));
        chunks.dedup();

        Some(BlockEdit { chunks, sections })
    }

    /// Writes a block without relighting or reporting what to rebuild.
    ///
    /// For bulk edits. Building a structure one block at a time would relight the
    /// same region once per block, dozens of times over, for the same final
    /// answer. Call [`Self::relight_around`] once when the work is done.
    ///
    /// Using this for a single edit would leave stale shading, which is why the
    /// ordinary [`Self::set_block`] is the one gameplay uses.
    pub fn set_block_quiet(&mut self, pos: BlockPos, block: BlockId) -> bool {
        let Some(chunk) = self.chunk_mut(pos.chunk_pos()) else {
            return false;
        };
        chunk.set_block(pos, block)
    }

    /// Recomputes sky light around `center`, for use after [`Self::set_block_quiet`].
    pub fn relight(&mut self, center: BlockPos) {
        self.relight_around(center);
    }

    /// Breaks a block, turning it to air.
    pub fn break_block(&mut self, pos: BlockPos) -> Option<BlockEdit> {
        let air = self.registry.id("air")?;
        self.set_block(pos, air)
    }

    /// The first solid block along a ray.
    pub fn raycast(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        max_distance: f64,
    ) -> Option<RayHit> {
        raycast(origin, direction, max_distance, &|pos| self.is_solid(pos))
    }

    /// The position a block would occupy if placed against `hit`.
    ///
    /// `None` when the cell is already taken, or lies outside the loaded
    /// world. Both cases need explicit handling: air is a *block* like any
    /// other, so asking whether the cell holds anything would refuse every
    /// placement, and an unloaded position reports no block at all, which looks
    /// identical to an empty one.
    pub fn placement_for(&self, hit: &RayHit) -> Option<BlockPos> {
        let target = BlockPos::new(
            hit.block.x + hit.normal[0],
            hit.block.y + hit.normal[1],
            hit.block.z + hit.normal[2],
        );
        self.chunk(target.chunk_pos())?;
        if self.is_solid(target) {
            return None;
        }
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::MIN_BLOCK_Y;
    use crate::world::pos::LocalPos;

    fn test_world() -> World {
        World::generate(1)
    }

    fn stone(world: &World) -> BlockId {
        world.registry().id("stone").expect("registered")
    }

    fn air(world: &World) -> BlockId {
        world.registry().id("air").expect("registered")
    }

    /// Builds a world with a single known block, for deterministic tests.
    fn world_with_one(radius: i32, chunk: ChunkPos, local: LocalPos) -> (World, BlockId) {
        let mut world = World::generate(radius);
        let stone = stone(&world);
        world
            .set_block(
                BlockPos::new(
                    chunk.x * SECTION_DIM + local.x as i32,
                    local.y as i32,
                    chunk.z * SECTION_DIM + local.z as i32,
                ),
                stone,
            )
            .expect("inside the world");
        (world, stone)
    }

    /// The same world with a three-block pocket around `pos`, so generated
    /// terrain cannot decide whether a cell is free.
    fn pocket_world(pos: BlockPos) -> (World, BlockId, BlockId) {
        let mut world = World::generate(1);
        let stone = stone(&world);
        let air = air(&world);

        // Clear a column so the tests are not fighting the terrain generator.
        for y in pos.y - 2..=pos.y + 2 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    world.set_block_quiet(BlockPos::new(pos.x + dx, y, pos.z + dz), air);
                }
            }
        }
        world.set_block(pos, stone).expect("inside the world");
        (world, stone, air)
    }

    #[test]
    fn generated_world_has_the_requested_chunks() {
        let world = test_world();
        assert_eq!(world.chunk_count(), 9);
        assert!(world.chunk(ChunkPos::new(1, 1)).is_some());
        assert!(world.chunk(ChunkPos::new(2, 0)).is_none());
    }

    #[test]
    fn chunk_positions_are_deterministic() {
        let a = World::generate(2).chunk_positions();
        let b = World::generate(2).chunk_positions();
        assert_eq!(a, b);
        assert!(a.windows(2).all(|w| w[0] <= w[1]), "not sorted");
    }

    #[test]
    fn a_break_reports_the_section_it_landed_in() {
        let (mut world, block) =
            world_with_one(0, ChunkPos::new(0, 0), LocalPos { x: 4, y: 5, z: 6 });
        let pos = BlockPos::new(4, 5, 6);
        assert_eq!(world.block(pos), Some(block));

        let edit = world.break_block(pos).expect("should edit");
        assert_eq!(edit.chunks, vec![ChunkPos::new(0, 0)]);
        // The edited section is always included. Light reaches further than the
        // block itself, so the section below can be in the list too; what must
        // hold is that nothing outside this chunk is touched.
        assert!(
            edit.touches(SectionPos::new(0, 0, 0)),
            "the edited section should be in {:?}",
            edit.sections
        );
        assert!(
            edit.sections
                .iter()
                .all(|section| section.chunk_pos() == ChunkPos::new(0, 0)),
            "an interior break must stay inside one chunk, got {:?}",
            edit.sections
        );
        assert_eq!(world.block(pos), Some(air(&world)));
    }

    #[test]
    fn a_break_on_a_chunk_border_pulls_in_the_neighbouring_chunk() {
        let (mut world, _block) =
            world_with_one(1, ChunkPos::new(1, 0), LocalPos { x: 0, y: 5, z: 6 });
        // x=0 of chunk 1 is world x=16, hard against the border with chunk 0.
        let pos = BlockPos::new(16, 5, 6);

        let edit = world.break_block(pos).expect("should edit");

        assert!(
            edit.owns(SectionPos::new(0, 0, 0)),
            "the neighbour's section is the one whose face appears, so chunk (0,0) \
             must be rebuilt too, got {:?}",
            edit.chunks
        );
        assert!(edit.owns(SectionPos::new(1, 0, 0)), "and the edited chunk");
    }

    #[test]
    fn a_block_on_a_section_boundary_dirties_its_neighbour() {
        let (mut world, _block) =
            world_with_one(1, ChunkPos::new(0, 0), LocalPos { x: 0, y: 5, z: 6 });
        // Placing again at x=0 also touches the section to the -X, which is
        // the neighbouring chunk's section.
        let edit = world
            .set_block(BlockPos::new(0, 5, 6), air(&world))
            .expect("should edit");
        assert!(
            edit.sections.len() >= 2,
            "a boundary block must dirty the neighbour: {:?}",
            edit.sections
        );
        assert!(edit.touches(SectionPos::new(0, 0, 0)));
        assert!(
            edit.touches(SectionPos::new(-1, 0, 0)),
            "the -X neighbour section should be dirty: {:?}",
            edit.sections
        );
    }

    #[test]
    fn an_edit_outside_the_world_is_refused() {
        let mut world = test_world();
        assert_eq!(world.break_block(BlockPos::new(1000, 0, 0)), None);
        assert_eq!(
            world.set_block(BlockPos::new(0, MIN_BLOCK_Y - 1, 0), stone(&world)),
            None
        );
    }

    #[test]
    fn writing_the_same_block_reports_nothing() {
        let (mut world, block) =
            world_with_one(0, ChunkPos::new(0, 0), LocalPos { x: 4, y: 5, z: 6 });
        assert_eq!(
            world.set_block(BlockPos::new(4, 5, 6), block),
            None,
            "a no-op must not trigger a re-mesh"
        );
    }

    #[test]
    fn neighbours_are_reported_across_chunks_and_vertically() {
        let world = test_world();
        let centre = SectionPos::new(0, 0, 0);
        let neighbors = world.section_neighbors(centre);

        assert!(neighbors.neg_x.is_some(), "chunk -1 should be loaded");
        assert!(neighbors.pos_x.is_some());
        assert!(neighbors.pos_y.is_some());
        assert!(neighbors.neg_y.is_some(), "the world is 24 sections tall");

        // The bottom of the world has nothing below it, and neither does the
        // chunk beyond the loaded square.
        let bottom = SectionPos::new(0, MIN_SECTION_Y, 0);
        assert!(
            world.section_neighbors(bottom).neg_y.is_none(),
            "nothing exists below the world's floor"
        );
        let far = SectionPos::new(2, 0, 0);
        assert!(
            world.section_neighbors(far).pos_x.is_none(),
            "chunk 2 is outside the loaded square"
        );
    }

    #[test]
    fn a_ray_through_generated_terrain_hits_something() {
        let world = test_world();
        // Straight down from high above the surface.
        let hit = world
            .raycast([8.5, 200.0, 8.5], [0.0, -1.0, 0.0], 400.0)
            .expect("terrain should be hit");
        assert!(hit.block.y < 200);
        assert_eq!(hit.normal, [0, 1, 0], "entered through the top");
        assert!(world.is_solid(hit.block));
    }

    #[test]
    fn placement_lands_against_the_hit_face_and_refuses_occupied_cells() {
        let target_block = BlockPos::new(4, 8, 6);
        let (mut world, stone, _air) = pocket_world(target_block);

        let hit = RayHit {
            block: target_block,
            normal: [0, 1, 0],
        };

        let cell = world.placement_for(&hit).expect("the cell above is empty");
        assert_eq!(cell, BlockPos::new(4, 9, 6));

        // Against a face whose neighbour is solid, there is nowhere to put it.
        world
            .set_block(BlockPos::new(4, 7, 6), stone)
            .expect("inside the world");
        let blocked = RayHit {
            block: target_block,
            normal: [0, -1, 0],
        };
        assert_eq!(world.placement_for(&blocked), None);

        // Unloaded space refuses placement rather than extending the world.
        let outside = RayHit {
            block: BlockPos::new(100_000, 0, 0),
            normal: [1, 0, 0],
        };
        assert_eq!(
            world.placement_for(&outside),
            None,
            "placement must not reach outside the loaded world"
        );
    }

    #[test]
    fn unloaded_space_is_not_solid() {
        let world = test_world();
        assert!(!world.is_solid(BlockPos::new(100_000, 0, 0)));
        assert_eq!(world.block(BlockPos::new(100_000, 0, 0)), None);
    }
}

#[cfg(test)]
mod sky_light_tests {
    use super::*;
    use crate::render::mesh::build_section_mesh;
    use crate::world::chunk::{MIN_BLOCK_Y, SECTIONS_PER_CHUNK};
    use crate::world::light::MAX_LIGHT;
    use crate::world::terrain::height_at;
    use std::time::Instant;

    /// Surface height of the generated terrain, without needing the world.
    fn surface_y(x: i32, z: i32) -> i32 {
        MIN_BLOCK_Y + height_at(x as f64, z as f64) as i32
    }

    #[test]
    fn open_sky_above_the_terrain_is_fully_lit() {
        let mut world = World::generate(1);
        world.compute_light();

        // Radius 1 covers chunks -1..1, so world X and Z run from -16 to 32.
        // Well above the highest terrain, everything sees the sun.
        assert_eq!(world.sky_light(BlockPos::new(8, 300, 8)), MAX_LIGHT);
        assert_eq!(world.sky_light(BlockPos::new(-8, 300, 24)), MAX_LIGHT);
        assert_eq!(world.sky_light(BlockPos::new(24, 300, -8)), MAX_LIGHT);
    }

    #[test]
    fn a_column_under_its_own_surface_is_dark() {
        let mut world = World::generate(1);
        world.compute_light();

        let surface = surface_y(8, 8);
        // Solid, so no light at all.
        assert_eq!(world.sky_light(BlockPos::new(8, surface, 8)), 0);
        // The air just above the surface still sees the sky directly.
        assert_eq!(
            world.sky_light(BlockPos::new(8, surface + 1, 8)),
            MAX_LIGHT,
            "one block above the surface should be in full daylight"
        );
    }

    #[test]
    fn light_is_continuous_across_a_chunk_border() {
        let mut world = World::generate(1);
        world.compute_light();

        // x=15 and x=16 are either side of the boundary between chunk 0 and
        // chunk 1. Lighting each chunk independently would leave a step here;
        // comparing every pair of columns either side of the border catches a
        // seam at any height, not just one.
        for z in 0..16i32 {
            for dy in 0..120i32 {
                let y = 200 + dy;
                let left = world.sky_light(BlockPos::new(15, y, z));
                let right = world.sky_light(BlockPos::new(16, y, z));
                assert_eq!(
                    left, right,
                    "seam at z={z}, y={y}: x=15 is {left}, x=16 is {right}"
                );
            }
        }
    }

    #[test]
    fn light_is_continuous_across_a_section_border() {
        let mut world = World::generate(1);
        world.compute_light();

        // The border between two vertically stacked sections. 192 is a section
        // boundary; 200 is not, which is what made this fail the first time.
        let y = 192;
        assert_eq!(y % SECTION_DIM, 0);
        assert_eq!(world.sky_light(BlockPos::new(8, y - 1, 8)), MAX_LIGHT);
        assert_eq!(world.sky_light(BlockPos::new(8, y, 8)), MAX_LIGHT);
    }

    #[test]
    fn a_tunnel_is_lit_along_its_length_and_dims_with_distance() {
        let mut world = World::generate(0);
        let stone = world.registry().id("stone").expect("registered");
        let air = world.registry().id("air").expect("registered");

        // A wall from x=4 to x=15, five blocks tall, with a tunnel bored
        // straight through it along x. The tunnel has to be horizontal: a
        // vertical hole would simply let sunlight fall through undimmed, which
        // is a different rule and would make this assert the wrong thing.
        for x in 4..16i32 {
            for y in 200..205i32 {
                for z in 0..16i32 {
                    world.set_block_quiet(BlockPos::new(x, y, z), stone);
                }
            }
        }
        for x in 4..16i32 {
            world.set_block_quiet(BlockPos::new(x, 202, 8), air);
        }

        world.compute_light();

        // x=3 is open sky. Crossing into the tunnel costs one level, and each
        // further block along costs another.
        assert_eq!(
            world.sky_light(BlockPos::new(3, 202, 8)),
            MAX_LIGHT,
            "outside the wall is in full daylight"
        );
        assert_eq!(
            world.sky_light(BlockPos::new(4, 202, 8)),
            MAX_LIGHT - 1,
            "the mouth of the tunnel"
        );
        assert_eq!(
            world.sky_light(BlockPos::new(8, 202, 8)),
            MAX_LIGHT - 5,
            "four more blocks in, so five levels down"
        );

        // Rock around the tunnel stays dark, and the solid wall above it does
        // not pass light through.
        assert_eq!(world.sky_light(BlockPos::new(8, 202, 7)), 0);
        assert_eq!(world.sky_light(BlockPos::new(8, 201, 8)), 0);
        assert_eq!(world.sky_light(BlockPos::new(8, 203, 8)), 0);
    }

    #[test]
    fn a_sealed_room_stays_dark_even_with_sky_all_around_it() {
        let mut world = World::generate(0);
        let stone = world.registry().id("stone").expect("registered");
        let air = world.registry().id("air").expect("registered");

        let (cx, cy, cz) = (8i32, 200i32, 8i32);
        // Solid everywhere in a 7x7x7 shell, hollow 3x3x3 inside it.
        for x in (cx - 4)..=(cx + 4) {
            for y in (cy - 4)..=(cy + 4) {
                for z in (cz - 4)..=(cz + 4) {
                    let shell = x <= cx - 2
                        || x >= cx + 2
                        || y <= cy - 2
                        || y >= cy + 2
                        || z <= cz - 2
                        || z >= cz + 2;
                    world.set_block_quiet(BlockPos::new(x, y, z), if shell { stone } else { air });
                }
            }
        }

        world.compute_light();

        for x in (cx - 1)..=(cx + 1) {
            for y in (cy - 1)..=(cy + 1) {
                for z in (cz - 1)..=(cz + 1) {
                    assert_eq!(
                        world.sky_light(BlockPos::new(x, y, z)),
                        0,
                        "the sealed room lit up at [{x}, {y}, {z}]"
                    );
                }
            }
        }
    }

    #[test]
    fn recomputing_is_idempotent() {
        let mut world = World::generate(1);
        world.compute_light();

        let sample = |world: &World| -> Vec<u8> {
            let mut out = Vec::new();
            for x in 0..16i32 {
                for z in 0..16i32 {
                    for y in 0..300 {
                        out.push(world.sky_light(BlockPos::new(x, y, z)));
                    }
                }
            }
            out
        };

        let first = sample(&world);
        world.compute_light();
        let second = sample(&world);

        assert_eq!(first, second, "lighting the same world twice must agree");
    }

    #[test]
    fn an_edit_changes_the_light_it_affects() {
        let mut world = World::generate(0);
        world.compute_light();

        let floor = surface_y(8, 8);
        assert_eq!(world.sky_light(BlockPos::new(8, floor, 8)), 0);

        // Punch the floor out: the sky above it now reaches further down.
        let air = world.registry().id("air").expect("registered");
        world.set_block(BlockPos::new(8, floor, 8), air);
        world.compute_light();

        assert_eq!(
            world.sky_light(BlockPos::new(8, floor, 8)),
            MAX_LIGHT,
            "an open column should be lit all the way down"
        );
    }

    /// The bug that made a dug hole render pitch black: the edit path did not
    /// relight at all, so shading stayed whatever it was for solid rock.
    #[test]
    fn digging_relights_without_asking_for_a_full_pass() {
        let mut world = World::generate(0);
        world.compute_light();

        let floor = surface_y(8, 8);
        let _air = world.registry().id("air").expect("registered");
        assert_eq!(
            world.sky_light(BlockPos::new(8, floor, 8)),
            0,
            "solid rock starts unlit"
        );

        let edit = world
            .break_block(BlockPos::new(8, floor, 8))
            .expect("should edit");

        assert_eq!(
            world.sky_light(BlockPos::new(8, floor, 8)),
            MAX_LIGHT,
            "breaking the floor must relight the hole, not leave it black"
        );
        assert!(
            edit.touches(BlockPos::new(8, floor, 8).section_pos()),
            "the relit section has to be rebuilt, or the shading change is invisible"
        );
    }

    #[test]
    fn filling_a_hole_takes_the_light_away_again() {
        let mut world = World::generate(0);
        world.compute_light();
        let air = world.registry().id("air").expect("registered");
        let stone = world.registry().id("stone").expect("registered");

        let floor = surface_y(8, 8);
        world.set_block(BlockPos::new(8, floor, 8), air);
        assert_eq!(world.sky_light(BlockPos::new(8, floor, 8)), MAX_LIGHT);

        world.set_block(BlockPos::new(8, floor, 8), stone);
        assert_eq!(
            world.sky_light(BlockPos::new(8, floor, 8)),
            0,
            "putting the block back must put the darkness back"
        );
    }

    #[test]
    fn relighting_a_small_change_does_not_disturb_the_rest_of_the_world() {
        let mut world = World::generate(1);
        world.compute_light();

        // Somewhere far from the edit, well past what light can reach.
        let far = BlockPos::new(200, surface_y(200, 200) + 4, 200);
        let before = world.sky_light(far);

        let air = world.registry().id("air").expect("registered");
        world.set_block(BlockPos::new(8, surface_y(8, 8), 8), air);

        assert_eq!(
            world.sky_light(far),
            before,
            "an edit must not relight the whole world"
        );
    }

    #[test]
    fn relighting_matches_a_full_pass_over_the_edited_neighbourhood() {
        let mut incremental = World::generate(0);
        incremental.compute_light();

        let air = incremental.registry().id("air").expect("registered");
        let stone = incremental.registry().id("stone").expect("registered");
        let floor = surface_y(8, 8);

        // A local edit near the middle of the chunk: dig a chamber, then put one
        // block back. The chamber is dug quietly and the last block goes through
        // the ordinary path, which is what triggers the cheap relight.
        for y in floor..floor + 3 {
            for z in 6..=10 {
                for x in 6..=10 {
                    incremental.set_block_quiet(BlockPos::new(x, y, z), air);
                }
            }
        }

        // Clone *after* the chamber, so the two worlds start from the same place.
        // Cloning before it silently compared a dug world against a solid one.
        let mut rebuilt = incremental.clone();
        incremental.set_block(BlockPos::new(8, floor, 8), stone);

        rebuilt.set_block_quiet(BlockPos::new(8, floor, 8), stone);
        rebuilt.compute_light();

        // Compare everywhere in the chunk: the cheap local relight and the
        // expensive full pass must agree.
        for x in 0..16i32 {
            for z in 0..16i32 {
                for y in MIN_BLOCK_Y..MIN_BLOCK_Y + 120 {
                    let pos = BlockPos::new(x, y, z);
                    assert_eq!(
                        incremental.sky_light(pos),
                        rebuilt.sky_light(pos),
                        "sky light differs at {pos:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn light_stays_inside_the_world_and_does_not_leak_out() {
        let mut world = World::generate(1);
        world.compute_light();

        // Unloaded space reads as dark rather than panicking, matching how
        // is_solid treats it.
        assert_eq!(world.sky_light(BlockPos::new(100_000, 200, 0)), 0);
        assert_eq!(world.sky_light(BlockPos::new(8, MIN_BLOCK_Y - 1, 8)), 0);
        assert_eq!(
            world.sky_light(BlockPos::new(
                8,
                MIN_BLOCK_Y + SECTIONS_PER_CHUNK as i32 * 16,
                8
            )),
            0
        );
    }

    #[test]
    fn a_block_floating_over_ground_shadows_the_ground_underneath() {
        let mut world = World::generate(0);
        let stone = world.registry().id("stone").expect("registered");

        // A flat floor at y=40 with open sky above it.
        for x in 0..16i32 {
            for z in 0..16i32 {
                world.set_block_quiet(BlockPos::new(x, 40, z), stone);
                world.set_block_quiet(BlockPos::new(x, 39, z), stone);
            }
        }
        world.compute_light();

        let under = BlockPos::new(8, 41, 8);
        let beside = BlockPos::new(12, 41, 8);
        assert_eq!(world.sky_light(under), MAX_LIGHT, "open ground is lit");
        assert_eq!(world.sky_light(beside), MAX_LIGHT, "and its neighbour too");

        // Hang a block in the air, one above the floor.
        world.set_block(BlockPos::new(8, 42, 8), stone);

        assert_eq!(
            world.sky_light(BlockPos::new(8, 42, 8)),
            0,
            "the block itself is not lit"
        );
        // Not zero, and that is correct. The cell under the block is air with
        // open sky all round it, so sideways spread brings it 15 - 1. A single
        // floating block casts a soft shadow, not a black one, because light
        // reaches it from every side but below.
        assert_eq!(
            world.sky_light(under),
            MAX_LIGHT - 1,
            "light reaches the gap sideways, losing one level"
        );
        assert_eq!(
            world.sky_light(beside),
            MAX_LIGHT,
            "and the open ground beside it is untouched"
        );
    }

    /// A roof with real depth does make the ground under it properly dark, so
    /// the soft shadow above is about a one-block gap rather than about shading
    /// being broken in general.
    #[test]
    fn a_roof_several_blocks_up_does_put_the_ground_in_darkness() {
        let mut world = World::generate(0);
        let stone = world.registry().id("stone").expect("registered");

        for x in 0..16i32 {
            for z in 0..16i32 {
                world.set_block_quiet(BlockPos::new(x, 40, z), stone);
                world.set_block_quiet(BlockPos::new(x, 42, z), stone);
                world.set_block_quiet(BlockPos::new(x, 43, z), stone);
            }
        }
        world.compute_light();

        // A roof at y=42 and y=43 with an air gap at y=41: the gap is sealed on
        // every side by the roof above, so nothing reaches it.
        assert_eq!(
            world.sky_light(BlockPos::new(8, 41, 8)),
            0,
            "a sealed one-block gap should be dark"
        );
        assert_eq!(
            world.sky_light(BlockPos::new(8, 44, 8)),
            MAX_LIGHT,
            "and the top of the roof is still in full sun"
        );
    }

    /// What the player actually sees, not what the light data says.
    ///
    /// The block data can be right and the picture still be too flat to read as
    /// a shadow. This goes through the mesher and the same combination the
    /// shader does, so it measures the final brightness of the ground under a
    /// floating block against the open ground beside it.
    #[test]
    fn a_floating_block_produces_a_shadow_strong_enough_to_see() {
        let mut world = World::generate(0);
        let stone = world.registry().id("stone").expect("registered");

        let dirt = world.registry().id("dirt").expect("registered");
        // A two-block checkerboard. A uniform floor would be greedy-merged into
        // one enormous quad whose only vertices sit at the corners, leaving
        // nothing anywhere near the block to sample.
        for x in 0..16i32 {
            for z in 0..16i32 {
                let block = if (x + z) % 2 == 0 { stone } else { dirt };
                world.set_block_quiet(BlockPos::new(x, 40, z), block);
            }
        }
        // `set_block_quiet` deliberately does not relight, so a full pass is
        // needed here to turn the floor from "unlit rock" into "lit ground".
        // Without it the test would be measuring a world that never had light.
        world.compute_light();

        world.set_block(BlockPos::new(8, 42, 8), stone);
        let chunk = ChunkPos::new(0, 0);

        // Brightness of the floor's top face, averaged, before and after.
        let floor_brightness = |world: &World| -> f32 {
            let context = world.section_context(SectionPos::new(0, 2, 0));
            let chunk = world.chunk(chunk).expect("loaded");
            let mesh = build_section_mesh(
                chunk.section(SectionPos::new(0, 2, 0)).expect("section"),
                chunk
                    .section_light(SectionPos::new(0, 2, 0))
                    .expect("light"),
                &context,
                world.registry(),
                [0, 32, 0],
            );

            // Only the vertices actually inside the shadow count. Averaging over
            // the whole floor would dilute the effect into invisibility, which
            // is the other half of why the bug went unnoticed: the shadow was
            // there, just far too small a fraction of the surface to show up in
            // a screenshot of the whole room.
            let values: Vec<f32> = mesh
                .vertices
                .iter()
                .filter(|vertex| {
                    vertex.normal[1] > 0.5
                        && vertex.position[1] == 41.0
                        && (vertex.position[0] - 8.0).abs() <= 1.0
                        && (vertex.position[2] - 8.0).abs() <= 1.0
                })
                .map(|vertex| vertex.light[0])
                .collect();
            assert!(
                !values.is_empty(),
                "the floor under the block should have a top face"
            );
            values.iter().sum::<f32>() / values.len() as f32
        };

        // The block is already in place. The checkerboard guarantees a vertex
        // at every cell, so the sample below is genuinely inside its shadow.
        let shaded = floor_brightness(&world);

        // The same point on the floor with nothing above it, for the contrast.
        let mut clear = world.clone();
        clear.set_block_quiet(BlockPos::new(8, 42, 8), clear.registry().id("air").unwrap());
        clear.compute_light();
        let open = floor_brightness(&clear);

        // Apply the same curve the shader does, so this measures brightness and
        // not raw light levels. Without it the test would compare 15 against 14
        // and call a two percent change acceptable.
        let curve = |level: f32| level * (0.35 + 0.65 * level);
        let shaded = curve(shaded);
        let open = curve(open);

        // The shader also multiplies by an occlusion term; the checkerboard cells under
        // the block all sit next to it, so occlusion contributes too. Model the
        // same term here rather than only the light curve.
        let occlusion = |ao: f32| 0.30 + 0.70 * ao * (0.55 + 0.45 * ao);
        let shaded = curve(shaded) * occlusion(0.75);
        let open = curve(open) * occlusion(1.0);

        let ratio = shaded / open;
        println!(
            "SHADOW open={open:.4} shaded={shaded:.4} ratio={ratio:.4} \
             contrast={:.1}%",
            (1.0 - ratio) * 100.0
        );
        assert!(
            ratio < 0.93,
            "a shadow at ratio {ratio:.3} is invisible; need at least 7% darker"
        );
    }

    #[test]
    fn shadow_depth_follows_the_height_of_the_roof_above() {
        let mut world = World::generate(0);
        let stone = world.registry().id("stone").expect("registered");

        for x in 0..16i32 {
            for z in 0..16i32 {
                world.set_block_quiet(BlockPos::new(x, 40, z), stone);
            }
        }
        world.compute_light();

        // A roof four blocks up, open on every side.
        for x in 0..16i32 {
            for z in 0..16i32 {
                world.set_block_quiet(BlockPos::new(x, 44, z), stone);
            }
        }
        world.compute_light();

        // Right under the roof: dark, because light only reaches sideways and it
        // has to cross the whole chunk to find an opening.
        assert_eq!(
            world.sky_light(BlockPos::new(8, 43, 8)),
            0,
            "under the roof"
        );

        // Take the roof away and the same cell must come back to full sun.
        for x in 0..16i32 {
            for z in 0..16i32 {
                world.set_block_quiet(BlockPos::new(x, 44, z), world.registry().id("air").unwrap());
            }
        }
        world.compute_light();
        assert_eq!(
            world.sky_light(BlockPos::new(8, 43, 8)),
            MAX_LIGHT,
            "removing the roof must bring the light back"
        );
    }

    #[test]
    fn a_single_edit_relights_fast_enough_to_feel_instant() {
        use std::time::Instant;

        let mut world = World::generate(5);
        world.compute_light();
        let air = world.registry().id("air").expect("registered");

        let floor = surface_y(8, 8);
        // Warm up, then average a handful of edits at different heights.
        let mut elapsed = std::time::Duration::ZERO;
        let rounds = 8u32;
        for round in 0..rounds {
            let start = Instant::now();
            world.set_block(BlockPos::new(8, floor + round as i32, 8), air);
            elapsed += start.elapsed();
        }

        let per_edit = elapsed / rounds;
        println!(
            "RELIGHT chunks={} per_edit={per_edit:?}",
            world.chunk_count()
        );
        // A click also triggers a re-mesh, so the relight has to stay well under
        // a frame or the whole thing starts to feel laggy.
        assert!(
            per_edit < std::time::Duration::from_millis(16),
            "relight took {per_edit:?} per edit, over one frame"
        );
    }

    #[test]
    fn light_sits_in_the_chunks_not_in_a_parallel_map() {
        let mut world = World::generate(1);
        world.compute_light();

        // Reading through the chunk and through the world must agree, which only
        // holds if there is one copy rather than a shadow copy in the world.
        let pos = BlockPos::new(8, 300, 8);
        let chunk = world.chunk(ChunkPos::new(0, 0)).expect("loaded");
        assert_eq!(world.sky_light(pos), chunk.sky_light(pos));
    }

    #[test]
    fn lighting_the_whole_world_costs_about_this_much() {
        // Prints rather than asserts: the number is here to be compared against
        // the next time the light code changes, not to fail a build on a slow
        // machine.
        let start = Instant::now();
        let mut world = World::generate(5);
        let generated = start.elapsed();
        let lighting = Instant::now();
        world.compute_light();
        let lit = lighting.elapsed();

        let light_bytes: usize = world
            .chunk_positions()
            .iter()
            .filter_map(|pos| world.chunk(*pos))
            .map(|chunk| chunk.light_nbytes())
            .sum();
        let block_bytes: usize = world
            .chunk_positions()
            .iter()
            .filter_map(|pos| world.chunk(*pos))
            .map(|chunk| chunk.nbytes())
            .sum();

        println!(
            "LIGHTING chunks={} generate={generated:?} compute_light={lit:?} light_bytes={light_bytes} block_bytes={block_bytes}",
            world.chunk_count()
        );
    }
}
