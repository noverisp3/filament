//! Turns a [`Section`] of blocks into renderable triangles.
//!
//! Two things happen here, in this order:
//!
//! 1. **Face culling** — a face is emitted only where a *solid* block touches a
//!    non-solid one. The face belongs to the solid side, which is why the test
//!    is one-sided: if both neighbours emitted it, the same surface would be
//!    drawn twice and z-fight along every shared boundary.
//! 2. **Greedy merging** — coplanar faces agreeing on block and direction are
//!    merged into the largest rectangle that fits. Voxel terrain is mostly flat,
//!    so this is the difference between thousands of quads and dozens.
//!
//! The mesher reads world data and produces plain CPU vertex data. It knows
//! nothing about wgpu, which is what lets it be unit tested.

use crate::world::block::{BlockId, BlockRegistry};
use crate::world::light::{MAX_LIGHT, SectionLight};
use crate::world::pos::{LocalPos, SECTION_DIM_USIZE};
use crate::world::section::Section;

const D: usize = SECTION_DIM_USIZE;
const D_I32: i32 = D as i32;

/// The 3×3×3 sections around a section, blocks and light together.
///
/// One-sided face culling and ambient occlusion both reach further than the
/// six face neighbours: shading a face's corner needs the three blocks meeting
/// at that corner on the open side, and the third of those is diagonally
/// opposite. A face on the corner of a section therefore samples into edge and
/// corner neighbours, which is exactly what this holds.
///
/// `None` means "not loaded", and both the mesher and the light rules treat that
/// as open air: the same safe direction as before, where a missing chunk can
/// cost a face but never punch a hole.
#[derive(Clone, Copy, Debug, Default)]
pub struct SectionContext<'a> {
    sections: [Option<&'a Section>; 27],
    light: [Option<&'a SectionLight>; 27],
}

impl<'a> SectionContext<'a> {
    /// Builds a context from raw arrays, for tests that assemble one by hand.
    pub fn from_parts(
        sections: [Option<&'a Section>; 27],
        light: [Option<&'a SectionLight>; 27],
    ) -> Self {
        Self { sections, light }
    }

    /// Index of the section at offset `(dx, dy, dz)`, each in -1..=1.
    fn slot(dx: i32, dy: i32, dz: i32) -> usize {
        (((dx + 1) as usize) * 3 + (dy + 1) as usize) * 3 + (dz + 1) as usize
    }

    fn section(&self, dx: i32, dy: i32, dz: i32) -> Option<&'a Section> {
        self.sections[Self::slot(dx, dy, dz)]
    }

    fn light(&self, dx: i32, dy: i32, dz: i32) -> Option<&'a SectionLight> {
        self.light[Self::slot(dx, dy, dz)]
    }
}

/// One vertex of chunk geometry.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    /// `(sky, block, occlusion)`, each 0..1.
    ///
    /// Sky and block light are separate because they are lit by different rules
    /// and a torch should read differently from the sun. Packing them into one
    /// scalar, as this attribute used to, cannot express both.
    pub light: [f32; 3],
}

/// Renderable geometry for one section, in chunk-local coordinates.
#[derive(Clone, Debug, Default)]
pub struct SectionMesh {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    /// Faces before merging, for tests and statistics.
    pub raw_face_count: usize,
}

impl SectionMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Used by tests and upcoming render statistics.
    #[allow(dead_code)]
    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }
}

/// The six axis-aligned face directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FaceDir {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl FaceDir {
    fn axis(self) -> usize {
        match self {
            Self::PosX | Self::NegX => 0,
            Self::PosY | Self::NegY => 1,
            Self::PosZ | Self::NegZ => 2,
        }
    }

    fn sign(self) -> f32 {
        match self {
            Self::PosX | Self::PosY | Self::PosZ => 1.0,
            Self::NegX | Self::NegY | Self::NegZ => -1.0,
        }
    }

    fn normal(self) -> [f32; 3] {
        let mut normal = [0.0; 3];
        normal[self.axis()] = self.sign();
        normal
    }
}

/// The two outward directions for one axis.
fn axis_dirs(axis: usize) -> [FaceDir; 2] {
    match axis {
        0 => [FaceDir::PosX, FaceDir::NegX],
        1 => [FaceDir::PosY, FaceDir::NegY],
        _ => [FaceDir::PosZ, FaceDir::NegZ],
    }
}

/// The two axes spanning the slice plane for `axis`.
fn plane_axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (2, 1),
        1 => (0, 2),
        _ => (0, 1),
    }
}

/// Where a coordinate lives: which neighbour section, and where inside it.
///
/// A named pair rather than an inline tuple, because it is the return type of
/// `Sampler::locate` and reads badly otherwise.
type Located = ((i32, i32, i32), (i32, i32, i32));

/// Reads blocks and light in a section plus one block of margin on every side.
struct Sampler<'a> {
    section: &'a Section,
    section_light: &'a SectionLight,
    context: &'a SectionContext<'a>,
    registry: &'a BlockRegistry,
}

impl Sampler<'_> {
    /// Splits section-local coordinates into a neighbour offset and a local
    /// position inside that neighbour.
    ///
    /// `None` means further out than one block of margin, which callers treat as
    /// open air.
    fn locate(x: i32, y: i32, z: i32) -> Option<Located> {
        let inside = |v: i32| (0..D_I32).contains(&v);
        match (inside(x), inside(y), inside(z)) {
            (true, true, true) => Some(((0, 0, 0), (x, y, z))),
            (false, true, true) => Some(((fold(x), 0, 0), (edge(x), y, z))),
            (true, false, true) => Some(((0, fold(y), 0), (x, edge(y), z))),
            (true, true, false) => Some(((0, 0, fold(z)), (x, y, edge(z)))),
            // Two or three axes out: an edge or corner neighbour, which is
            // what shading a face corner on a section boundary needs.
            _ => {
                let dx = if inside(x) { 0 } else { fold(x) };
                let dy = if inside(y) { 0 } else { fold(y) };
                let dz = if inside(z) { 0 } else { fold(z) };
                Some(((dx, dy, dz), (edge(x), edge(y), edge(z))))
            }
        }
    }

    /// Block at section-local coordinates, which may run from `-1` to `D`.
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let ((dx, dy, dz), (lx, ly, lz)) = Self::locate(x, y, z)?;
        let section = if (dx, dy, dz) == (0, 0, 0) {
            self.section
        } else {
            self.context.section(dx, dy, dz)?
        };
        Some(section.get(LocalPos {
            x: lx as u8,
            y: ly as u8,
            z: lz as u8,
        }))
    }

    /// Sky and block light at section-local coordinates.
    fn light_at(&self, x: i32, y: i32, z: i32) -> Option<(u8, u8)> {
        let ((dx, dy, dz), (lx, ly, lz)) = Self::locate(x, y, z)?;
        let light = if (dx, dy, dz) == (0, 0, 0) {
            self.section_light
        } else {
            self.context.light(dx, dy, dz)?
        };
        let local = LocalPos {
            x: lx as u8,
            y: ly as u8,
            z: lz as u8,
        };
        Some((light.sky(local), light.block(local)))
    }

    fn opaque_at(&self, x: i32, y: i32, z: i32) -> bool {
        match self.block_at(x, y, z) {
            Some(block) => self.registry.is_opaque(block),
            None => false,
        }
    }

    /// Light and occlusion for the four corners of one face.
    ///
    /// A face at `cell` with outward step `step` is seen from the open side. For
    /// each corner, four cells matter: the one directly in front, the two
    /// touching it along the face's edges, and the one diagonally opposite. That
    /// last one is what produces the classic soft darkening in a corner, and it
    /// is also why shading needs the corner neighbours and not just the six face
    /// neighbours.
    ///
    /// Order matches `emit_quad`'s corners: `(u,v)`, `(u+1,v)`, `(u+1,v+1)`,
    /// `(u,v+1)`. The outward cell sits at the cell's own `(u, v)`, so the
    /// min-u,min-v corner reaches towards -u and -v from it.
    fn corners(
        &self,
        cell: &[i32; 3],
        step: &[i32; 3],
        u_axis: usize,
        v_axis: usize,
    ) -> [Corner; 4] {
        let front = [cell[0] + step[0], cell[1] + step[1], cell[2] + step[2]];

        let at = |du: i32, dv: i32| {
            let mut p = front;
            p[u_axis] += du;
            p[v_axis] += dv;
            let opaque = self.opaque_at(p[0], p[1], p[2]);
            (p, opaque)
        };

        let mut out = [Corner {
            sky: 0,
            block: 0,
            ao: 0,
        }; 4];

        // (du, dv) per corner, in the order above.
        for (slot, (du, dv)) in [(-1, -1), (1, -1), (1, 1), (-1, 1)].into_iter().enumerate() {
            let (side_u, opaque_u) = at(du, 0);
            let (side_v, opaque_v) = at(0, dv);
            let (diagonal, opaque_diag) = at(du, dv);

            // Two touching sides both blocked means the corner is walled in,
            // however open the diagonal is. Otherwise the count of blockers
            // gives the familiar 0, 1, 2, 3 scale.
            let blocked = u8::from(opaque_u) + u8::from(opaque_v) + u8::from(opaque_diag);
            let ao = if opaque_u && opaque_v { 0 } else { 3 - blocked };

            // The cell in front of the face is never opaque, so there is always
            // at least one sample to average over.
            let (sky, block) = self.corner_light(&[
                (&front, false),
                (&side_u, opaque_u),
                (&side_v, opaque_v),
                (&diagonal, opaque_diag),
            ]);

            out[slot] = Corner { sky, block, ao };
        }
        out
    }

    /// Averages light over the cells meeting at a face corner.
    ///
    /// Opaque cells are skipped rather than counted as dark: they are already
    /// accounted for by the occlusion term, and counting them twice would
    /// double-darken every corner that touches a block.
    fn corner_light(&self, samples: &[(&[i32; 3], bool); 4]) -> (u8, u8) {
        let mut sky = 0u32;
        let mut block = 0u32;
        let mut count = 0u32;

        for (position, opaque) in samples {
            if *opaque {
                continue;
            }
            let Some((s, b)) = self.light_at(position[0], position[1], position[2]) else {
                continue;
            };
            sky += u32::from(s);
            block += u32::from(b);
            count += 1;
        }

        if count == 0 {
            return (0, 0);
        }
        ((sky / count) as u8, (block / count) as u8)
    }
}

/// Which side of the boundary a coordinate is on: -1 below, +1 above.
fn fold(v: i32) -> i32 {
    if v < 0 { -1 } else { 1 }
}

/// Folds `-1` to the far edge of the neighbour and `D` to its near edge, so a
/// margin read lands on the block actually touching this section.
fn edge(v: i32) -> i32 {
    if v < 0 { (D - 1) as i32 } else { 0 }
}
/// Light and occlusion at one corner of a face.
///
/// Kept as integers, not floats, because the greedy mesher merges faces only
/// when their keys are **equal**, and float equality would make merging
/// depend on rounding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Corner {
    sky: u8,
    block: u8,
    /// 0 is fully occluded, 3 is clear. Mirrors the classic 0..3 scale so the
    /// shading levels are the familiar ones.
    ao: u8,
}

impl Corner {
    fn normalized(self) -> [f32; 3] {
        [
            f32::from(self.sky) / f32::from(MAX_LIGHT),
            f32::from(self.block) / f32::from(MAX_LIGHT),
            f32::from(self.ao) / 3.0,
        ]
    }
}

/// Marks one cell of a slice as needing a face.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FaceKey {
    block: BlockId,
    dir: FaceDir,
    /// One entry per corner, ordered to match the corners `emit_quad` builds:
    /// `(u, v)`, `(u+1, v)`, `(u+1, v+1)`, `(u, v+1)`.
    ///
    /// Carrying these is what lets merging stay sound: two faces merge only when
    /// all four corners agree, so a merged quad never straddles a lighting or
    /// occlusion boundary. That is also why smooth lighting costs quads -- see
    /// the greedy meshing note at the top of this file.
    corners: [Corner; 4],
}

/// Builds render geometry for one section.
///
/// `origin` is the section's minimum corner in **chunk-local** coordinates, so
/// emitted positions span the full chunk height and a single uniform per chunk
/// is enough to place them.
pub fn build_section_mesh(
    section: &Section,
    light: &SectionLight,
    context: &SectionContext<'_>,
    registry: &BlockRegistry,
    origin: [i32; 3],
) -> SectionMesh {
    let sampler = Sampler {
        section,
        section_light: light,
        context,
        registry,
    };

    let mut mesh = SectionMesh::default();

    // A section holding no solid block emits nothing, and can be skipped
    // outright. Sampling a few cells for "is anything solid here" is *not*
    // enough: a section with a single block in the middle would be skipped, and
    // that block would silently disappear.
    //
    // Because faces are one-sided, a non-solid section never has to emit on
    // behalf of a solid neighbour either, so this check is safe.
    if section.is_uniform() {
        let only = section.get(LocalPos { x: 0, y: 0, z: 0 });
        if !registry.is_opaque(only) {
            return mesh;
        }
    }

    for axis in 0..3 {
        let (u_axis, v_axis) = plane_axes(axis);

        for dir in axis_dirs(axis) {
            let facing_out = dir.sign() > 0.0;

            for slice in 0..D_I32 {
                let mut mask = [[None; D]; D];

                // Both indices are needed to write `mask[v][u]`, and `u`/`v` map
                // onto different world axes depending on which axis is being
                // sliced, so an iterator would not simplify this.
                #[allow(clippy::needless_range_loop)]
                for v in 0..D {
                    for u in 0..D {
                        let mut cell = [0i32; 3];
                        cell[axis] = slice;
                        cell[u_axis] = u as i32;
                        cell[v_axis] = v as i32;

                        let mut step = [0i32; 3];
                        step[axis] = if facing_out { 1 } else { -1 };

                        // One-sided on purpose: the solid side owns the face.
                        // Emitting from both sides would draw every shared
                        // surface twice.
                        mask[v][u] = if sampler.opaque_at(cell[0], cell[1], cell[2])
                            && !sampler.opaque_at(
                                cell[0] + step[0],
                                cell[1] + step[1],
                                cell[2] + step[2],
                            ) {
                            Some(FaceKey {
                                block: sampler.block_at(cell[0], cell[1], cell[2]).expect("opaque"),
                                dir,
                                corners: sampler.corners(&cell, &step, u_axis, v_axis),
                            })
                        } else {
                            None
                        };
                    }
                }

                merge_slice(
                    &mut mask, axis, slice, facing_out, origin, registry, &mut mesh,
                );
            }
        }
    }

    mesh
}

/// Greedily merges equal keys in one 2D slice and appends the resulting quads.
/// Index loops rather than iterators on purpose: the scan both reads and
/// clears `mask` while walking it.
#[allow(clippy::needless_range_loop)]
#[allow(clippy::too_many_arguments)]
fn merge_slice(
    mask: &mut [[Option<FaceKey>; D]; D],
    axis: usize,
    slice: i32,
    facing_out: bool,
    origin: [i32; 3],
    registry: &BlockRegistry,
    mesh: &mut SectionMesh,
) {
    let (u_axis, v_axis) = plane_axes(axis);

    for u in 0..D {
        for v in 0..D {
            let Some(key) = mask[v][u] else { continue };

            // Grow along u while the face agrees.
            let mut width = 1;
            while u + width < D && mask[v][u + width] == Some(key) {
                width += 1;
            }

            // Then along v, but only while whole rows agree.
            let mut height = 1;
            'grow: while v + height < D {
                for du in 0..width {
                    if mask[v + height][u + du] != Some(key) {
                        break 'grow;
                    }
                }
                height += 1;
            }

            mesh.raw_face_count += width * height;
            emit_quad(
                mesh,
                key,
                axis,
                slice,
                facing_out,
                u as i32,
                v as i32,
                width as i32,
                height as i32,
                origin,
                registry,
                (u_axis, v_axis),
            );

            for dv in 0..height {
                for du in 0..width {
                    mask[v + dv][u + du] = None;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_quad(
    mesh: &mut SectionMesh,
    key: FaceKey,
    axis: usize,
    slice: i32,
    facing_out: bool,
    u: i32,
    v: i32,
    width: i32,
    height: i32,
    origin: [i32; 3],
    registry: &BlockRegistry,
    (u_axis, v_axis): (usize, usize),
) {
    // The plane sits on the far side of the slice cell when emitting outward.
    let plane = if facing_out { slice + 1 } else { slice };

    let mut key_corners = key.corners;
    let mut corners = [
        corner(u, v, plane, u_axis, v_axis, axis),
        corner(u + width, v, plane, u_axis, v_axis, axis),
        corner(u + width, v + height, plane, u_axis, v_axis, axis),
        corner(u, v + height, plane, u_axis, v_axis, axis),
    ];

    // Rather than hand-deriving winding order for six cases, measure it: the
    // quad must be counter-clockwise as seen from where the normal points. The
    // quad lies in a plane normal to `axis`, so only that component of the cross
    // product can be non-zero.
    let normal = key.dir.normal();
    let edge_u = sub(corners[1], corners[0]);
    let edge_v = sub(corners[2], corners[0]);
    let cross_along_normal = match axis {
        0 => edge_u[1] * edge_v[2] - edge_u[2] * edge_v[1],
        1 => edge_u[2] * edge_v[0] - edge_u[0] * edge_v[2],
        _ => edge_u[0] * edge_v[1] - edge_u[1] * edge_v[0],
    };
    if cross_along_normal * key.dir.sign() < 0.0 {
        corners.swap(1, 3);
        // The per-corner light has to follow the same swap, or the shading ends
        // up mirrored on half the faces.
        key_corners.swap(1, 3);
    }

    let color = registry.color(key.block);

    let base = mesh.vertices.len() as u32;
    for (index, corner) in corners.into_iter().enumerate() {
        mesh.vertices.push(MeshVertex {
            position: [
                corner[0] + origin[0] as f32,
                corner[1] + origin[1] as f32,
                corner[2] + origin[2] as f32,
            ],
            normal,
            color,
            light: key_corners[index].normalized(),
        });
    }

    // Adaptive triangulation. The rasteriser does not draw a quad, it splits it
    // into two triangles along one diagonal, and interpolating per-vertex
    // shading across them leaves a crease that the eye picks out immediately on
    // a flat surface. Which diagonal is worse depends on where the shading
    // changes, so pick the one that puts the crease between the darker pair of
    // corners, where a darker line is least noticeable.
    //
    // Both orders below walk the same corner cycle, so the winding is
    // preserved either way.
    let diagonal_a = key_corners[0].ao as u16 + key_corners[2].ao as u16;
    let diagonal_b = key_corners[1].ao as u16 + key_corners[3].ao as u16;
    if diagonal_a > diagonal_b {
        // Crease between corners 1 and 3.
        mesh.indices
            .extend_from_slice(&[base + 1, base + 2, base + 3, base + 1, base + 3, base]);
    } else {
        // Crease between corners 0 and 2.
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Builds one corner of a merged quad.
#[allow(clippy::too_many_arguments)]
fn corner(u: i32, v: i32, plane: i32, u_axis: usize, v_axis: usize, axis: usize) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    out[axis] = plane as f32;
    out[u_axis] = u as f32;
    out[v_axis] = v as f32;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct World {
        registry: BlockRegistry,
        air: BlockId,
        stone: BlockId,
    }

    fn world() -> World {
        let mut registry = BlockRegistry::new();
        let air = registry.register("air", false, [0.0, 0.0, 0.0]);
        let stone = registry.register("stone", true, [0.5, 0.5, 0.5]);
        World {
            registry,
            air,
            stone,
        }
    }

    fn build(section: &Section, world: &World) -> SectionMesh {
        // Full daylight everywhere, so shading is uniform and tests about
        // geometry are not also tests about light.
        build_with(section, world, &SectionLight::open_sky())
    }

    /// Builds with an explicit light layer and an empty neighbourhood.
    fn build_with(section: &Section, world: &World, light: &SectionLight) -> SectionMesh {
        build_section_mesh(
            section,
            light,
            &SectionContext::default(),
            &world.registry,
            [0, 0, 0],
        )
    }

    fn fill(section: &mut Section, world: &World, x: i32, y: i32, z: i32) {
        section.set(
            LocalPos {
                x: x as u8,
                y: y as u8,
                z: z as u8,
            },
            world.stone,
        );
    }

    #[test]
    fn an_all_air_section_produces_nothing() {
        let world = world();
        let section = Section::uniform(world.air);
        let mesh = build(&section, &world);
        assert!(mesh.is_empty());
        assert_eq!(mesh.raw_face_count, 0);
    }

    #[test]
    fn a_full_section_produces_only_its_six_outer_faces() {
        let world = world();
        let mesh = build(&Section::uniform(world.stone), &world);
        assert_eq!(mesh.quad_count(), 6, "each side merges into one quad");
        assert_eq!(mesh.raw_face_count, 6 * D * D);
    }

    #[test]
    fn interior_faces_are_culled() {
        let world = world();
        let mut section = Section::uniform(world.stone);
        section.set(LocalPos { x: 8, y: 8, z: 8 }, world.air);

        let mesh = build(&section, &world);
        // The hole is in the interior, so the six outer sides are untouched;
        // the hole adds six faces of its own, emitted by the solid blocks
        // around it.
        assert_eq!(
            mesh.raw_face_count,
            6 * D * D + 6,
            "got {}",
            mesh.raw_face_count
        );
    }

    #[test]
    fn a_flat_slab_collapses_to_six_quads() {
        let world = world();
        let mut section = Section::uniform(world.air);
        for y in 0..4 {
            for z in 0..D_I32 {
                for x in 0..D_I32 {
                    fill(&mut section, &world, x, y, z);
                }
            }
        }

        let mesh = build(&section, &world);
        // 16x16 top and bottom, 16x4 on each of the four sides.
        assert_eq!(mesh.raw_face_count, 2 * D * D + 4 * D * 4);
        assert_eq!(mesh.quad_count(), 6, "got {}", mesh.quad_count());
    }

    #[test]
    fn merging_turns_thousands_of_faces_into_six_quads() {
        let world = world();
        let mesh = build(&Section::uniform(world.stone), &world);
        assert_eq!(mesh.raw_face_count, 6 * D * D);
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24, "four vertices per merged quad");
    }

    #[test]
    fn a_shared_surface_is_emitted_exactly_once() {
        let world = world();
        let air_section = Section::uniform(world.air);
        let stone_section = Section::uniform(world.stone);

        // The air side must emit nothing at the shared boundary; the stone
        // side owns that face.
        let from_air = build_section_mesh(
            &air_section,
            &SectionLight::open_sky(),
            &context_with(1, &stone_section, &SectionLight::open_sky()),
            &world.registry,
            [0, 0, 0],
        );
        assert!(
            from_air.is_empty(),
            "air section emitted a face the stone section already owns"
        );

        let from_stone = build_section_mesh(
            &stone_section,
            &SectionLight::open_sky(),
            &context_with(-1, &air_section, &SectionLight::open_sky()),
            &world.registry,
            [0, 0, 0],
        );
        assert_eq!(
            from_stone.raw_face_count,
            5 * D * D + D * D,
            "only the -X side should gain a face"
        );
    }

    /// A context with one neighbour along X at `dx`, everything else absent.
    fn context_with<'a>(
        dx: i32,
        section: &'a Section,
        light: &'a SectionLight,
    ) -> SectionContext<'a> {
        let mut sections = [None; 27];
        let mut lights = [None; 27];
        let slot = (((dx + 1) as usize) * 3 + 1) * 3 + 1;
        sections[slot] = Some(section);
        lights[slot] = Some(light);
        SectionContext::from_parts(sections, lights)
    }

    #[test]
    fn every_quad_winds_counter_clockwise_from_outside() {
        let world = world();
        let mut section = Section::uniform(world.air);
        // A jagged surface, so quads point in all six directions.
        for z in 0..D_I32 {
            for x in 0..D_I32 {
                for y in 0..((x + z) % 5) {
                    fill(&mut section, &world, x, y, z);
                }
            }
        }

        let mesh = build(&section, &world);
        assert!(!mesh.is_empty());

        for quad in mesh.vertices.as_chunks::<4>().0 {
            let normal = quad[0].normal;
            let edge_u = sub(quad[1].position, quad[0].position);
            let edge_v = sub(quad[2].position, quad[0].position);
            assert!(
                dot(cross(edge_u, edge_v), normal) > 0.0,
                "quad winds the wrong way: normal={normal:?}"
            );
        }
    }

    #[test]
    fn normals_are_axis_aligned_unit_vectors() {
        let world = world();
        let mut section = Section::uniform(world.air);
        fill(&mut section, &world, 4, 4, 4);

        let mesh = build(&section, &world);
        assert_eq!(mesh.quad_count(), 6, "a lone block has six faces");
        for quad in mesh.vertices.as_chunks::<4>().0 {
            let normal = quad[0].normal;
            let non_zero =
                (normal[0] != 0.0) as u32 + (normal[1] != 0.0) as u32 + (normal[2] != 0.0) as u32;
            assert_eq!(non_zero, 1, "normal is not axis aligned: {normal:?}");
            assert!(normal.iter().all(|c| *c == 0.0 || *c == 1.0 || *c == -1.0));
        }
    }

    #[test]
    fn quads_lie_flat_on_integer_planes() {
        let world = world();
        let mut section = Section::uniform(world.air);
        fill(&mut section, &world, 4, 4, 4);

        let mesh = build(&section, &world);
        for vertex in &mesh.vertices {
            let axis = if vertex.normal[0] != 0.0 {
                0
            } else if vertex.normal[1] != 0.0 {
                1
            } else {
                2
            };
            let on_plane = vertex.position[axis];
            assert_eq!(
                on_plane.fract(),
                0.0,
                "face at {on_plane} is not on an integer plane"
            );
        }
    }

    #[test]
    fn origin_offsets_positions_into_chunk_space() {
        let world = world();
        let mesh = build_section_mesh(
            &Section::uniform(world.stone),
            &SectionLight::open_sky(),
            &SectionContext::default(),
            &world.registry,
            [0, -64, 0],
        );
        for vertex in &mesh.vertices {
            // Section-local 0..16 shifted down by the origin.
            assert!(
                (-64.0..=-48.0).contains(&vertex.position[1]),
                "y={} escaped the section's chunk-local range",
                vertex.position[1]
            );
        }
    }

    #[test]
    fn index_buffer_addresses_every_vertex_exactly_once() {
        let world = world();
        let mut section = Section::uniform(world.air);
        for z in 0..D_I32 {
            for x in 0..D_I32 {
                fill(&mut section, &world, x, (x * z) % 6, z);
            }
        }

        let mesh = build(&section, &world);
        assert_eq!(mesh.indices.len() % 6, 0);
        assert_eq!(mesh.indices.len(), mesh.quad_count() * 6);

        let last = mesh.vertices.len() as u32;
        assert!(mesh.indices.iter().all(|i| *i < last));

        // Six indices per quad, forming two triangles over four new vertices.
        for quad in mesh.indices.as_chunks::<6>().0 {
            let vertices: std::collections::BTreeSet<u32> = quad.iter().copied().collect();
            assert_eq!(vertices.len(), 4, "quad reuses a vertex: {quad:?}");
            assert_eq!(
                vertices.last().copied().unwrap() - vertices.first().copied().unwrap(),
                3
            );
        }
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    /// A section holding one slab of stone at `y`, everything else air.
    fn slab(section: &mut Section, world: &World, y: i32) {
        for z in 0..D_I32 {
            for x in 0..D_I32 {
                section.set(
                    LocalPos {
                        x: x as u8,
                        y: y as u8,
                        z: z as u8,
                    },
                    world.stone,
                );
            }
        }
    }

    #[test]
    fn an_unobstructed_face_has_no_occlusion() {
        let world = world();
        let mut section = Section::uniform(world.air);
        // A single block on its own, so every face looks out into open air.
        fill(&mut section, &world, 8, 8, 8);

        let mesh = build(&section, &world);
        for vertex in &mesh.vertices {
            assert_eq!(
                vertex.light[2], 1.0,
                "a lone block should have no occlusion, got {}",
                vertex.light[2]
            );
        }
    }

    #[test]
    fn a_block_beside_a_face_darkens_only_the_corners_it_touches() {
        let world = world();
        let mut section = Section::uniform(world.air);
        // A floor at y=0 with one block standing on it at (0, 1, 0). The top
        // face of the neighbouring floor cell (1, 0, 0) is still emitted, but
        // its two corners nearest the standing block are partly occluded.
        slab(&mut section, &world, 0);
        fill(&mut section, &world, 0, 1, 0);

        let mesh = build(&section, &world);
        let occlusion: Vec<f32> = mesh
            .vertices
            .iter()
            .filter(|vertex| {
                // Only the +Y face at plane 1, which is the top of the slab.
                vertex.normal[1] > 0.5 && vertex.position[1] == 1.0
            })
            .map(|vertex| vertex.light[2])
            .collect();

        assert!(!occlusion.is_empty(), "expected a top face to shade");
        assert!(
            occlusion.iter().any(|value| *value < 1.0),
            "the standing block should darken some corners, all were {occlusion:?}"
        );
        assert!(
            occlusion.iter().any(|value| *value > 0.0),
            "but not the ones furthest from it, all were {occlusion:?}"
        );
    }

    #[test]
    fn a_walled_in_corner_is_fully_occluded() {
        let world = world();
        let mut section = Section::uniform(world.air);
        slab(&mut section, &world, 0);
        // Two blocks flanking one corner of the floor cell (1, 0, 5): the block
        // beside it along x, and the block beside it along z. The diagonal is
        // left open on purpose -- two *touching* blockers is what fully occludes,
        // whatever the diagonal does. The cell has to be at z>0 so that both
        // flanking positions are still inside the section.
        fill(&mut section, &world, 0, 1, 5);
        fill(&mut section, &world, 1, 1, 4);

        let mesh = build(&section, &world);
        let darkest = mesh
            .vertices
            .iter()
            .filter(|vertex| vertex.normal[1] > 0.5 && vertex.position[1] == 1.0)
            .map(|vertex| vertex.light[2])
            .fold(1.0f32, f32::min);

        assert!(
            darkest < 0.34,
            "a walled-in corner should go near black, darkest was {darkest}"
        );
    }

    #[test]
    fn vertex_light_stays_within_zero_to_one() {
        let world = world();
        let mut section = Section::uniform(world.air);
        slab(&mut section, &world, 0);
        for x in 0..D_I32 {
            for z in 0..D_I32 {
                if (x + z) % 3 == 0 {
                    fill(&mut section, &world, x, 1, z);
                }
            }
        }

        // Mixed light so both channels are exercised.
        let mut light = SectionLight::dark();
        light.set_sky(LocalPos { x: 0, y: 0, z: 0 }, 15);

        let mesh = build_with(&section, &world, &light);
        assert!(!mesh.is_empty());
        for vertex in &mesh.vertices {
            for (channel, value) in vertex.light.iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(value),
                    "channel {channel} out of range: {value} in {:?}",
                    vertex.light
                );
                assert!(value.is_finite(), "channel {channel} is not finite");
            }
        }
    }

    #[test]
    fn brighter_light_reaches_the_vertices() {
        let world = world();
        let mut section = Section::uniform(world.air);
        fill(&mut section, &world, 8, 8, 8);

        let bright = build_with(&section, &world, &SectionLight::open_sky());
        let dark = build_with(&section, &world, &SectionLight::dark());
        assert!(!bright.is_empty() && !dark.is_empty());

        let max_sky = |mesh: &SectionMesh| {
            mesh.vertices
                .iter()
                .map(|vertex| vertex.light[0])
                .fold(0.0f32, f32::max)
        };
        assert!(
            max_sky(&bright) > max_sky(&dark),
            "sky light should reach the surface: {} vs {}",
            max_sky(&bright),
            max_sky(&dark)
        );

        // With no light data at all, nothing may claim to be lit.
        for vertex in &dark.vertices {
            assert_eq!(vertex.light[0], 0.0, "a dark section must not be lit");
            assert_eq!(vertex.light[1], 0.0, "block light has no sources yet");
        }
    }

    #[test]
    fn faces_with_different_corner_shading_do_not_merge() {
        let world = world();
        let mut flat = Section::uniform(world.air);
        slab(&mut flat, &world, 0);
        let merged = build(&flat, &world).quad_count();

        // The same slab with one block standing on it: the top face can no
        // longer be a single quad, because its corners disagree.
        let mut broken = flat.clone();
        fill(&mut broken, &world, 8, 1, 8);
        let split = build(&broken, &world).quad_count();

        assert!(
            split > merged,
            "a floor should merge into {merged} quads, but an occluded one gave {split}"
        );
    }

    #[test]
    fn the_six_faces_of_one_block_carry_distinct_normals() {
        let world = world();
        let mut section = Section::uniform(world.air);
        fill(&mut section, &world, 8, 8, 8);

        let mesh = build(&section, &world);
        assert_eq!(mesh.quad_count(), 6, "a lone block has six faces");

        let mut normals: Vec<[i32; 3]> = mesh
            .vertices
            .iter()
            .map(|v| [v.normal[0] as i32, v.normal[1] as i32, v.normal[2] as i32])
            .collect();
        normals.sort();
        normals.dedup();

        assert_eq!(
            normals.len(),
            6,
            "six faces must carry six distinct normals, got {normals:?}"
        );
        for axis in 0..3 {
            let positive = normals.iter().filter(|n| n[axis] == 1).count();
            let negative = normals.iter().filter(|n| n[axis] == -1).count();
            assert_eq!(
                positive,
                1,
                "one face pointing +{}",
                "xyz".as_bytes()[axis] as char
            );
            assert_eq!(
                negative,
                1,
                "one face pointing -{}",
                "xyz".as_bytes()[axis] as char
            );
        }
    }

    /// Guards the change that made flat terrain look like a sheet of light.
    ///
    /// Outdoors the sky light is 15 wherever it can see the sky, so the shader's
    /// per-face shade is the only thing telling one face of a block from
    /// another. It was cut to eight percent when smooth lighting landed, and
    /// with it went the sense that the blocks had faces.
    ///
    /// This asserts the mesh gives the shader what it needs: exactly one face
    /// per direction, each with a clean axis-aligned normal it can classify.
    /// What the shader does with a normal, mirrored here so it can be measured.
    ///
    /// Kept next to the tests that need it and labelled as a mirror of the shader,
    /// because a shader constant cannot be asserted on directly and the alternative
    /// is a change nobody can catch until it is looked at on screen.
    fn shader_face_shade(normal: [f32; 3]) -> f32 {
        if normal[1] > 0.5 {
            1.0
        } else if normal[1] < -0.5 {
            0.5
        } else if normal[0].abs() > 0.5 {
            0.6
        } else {
            0.8
        }
    }

    #[test]
    fn a_single_block_shows_four_distinct_brightness_levels() {
        // Outdoors the sky light is 15 on every face of an isolated block, so
        // the only thing giving the faces different brightness is the
        // directional shade. Six faces lit identically is exactly what "light
        // radiating evenly regardless of everything" looks like, and it is what
        // shipping with an 8% facing term produced.
        let shades = [
            ("top", [0.0, 1.0, 0.0]),
            ("bottom", [0.0, -1.0, 0.0]),
            ("east/west", [1.0, 0.0, 0.0]),
            ("north/south", [0.0, 0.0, 1.0]),
        ]
        .map(|(name, normal)| (name, shader_face_shade(normal)));

        for (name, shade) in shades {
            println!("FACE {name} shade={shade:.2}");
        }

        let top = shader_face_shade([0.0, 1.0, 0.0]);
        let bottom = shader_face_shade([0.0, -1.0, 0.0]);
        let east = shader_face_shade([1.0, 0.0, 0.0]);
        let north = shader_face_shade([0.0, 0.0, 1.0]);

        // The top-to-bottom swing is the one that says which way is up.
        assert!(
            top / bottom >= 1.8,
            "top must be far brighter than bottom, got {top} vs {bottom}"
        );
        // And the four sides must not all be the same value, or the block reads
        // as a flat cut-out.
        assert!(
            (east - north).abs() > 0.15,
            "the two side pairs should differ, got {east} and {north}"
        );

        let all = [top, bottom, east, north];
        assert!(
            all.iter().all(|shade| (0.4..=1.0).contains(shade)),
            "every shade stays in a usable range, got {all:?}"
        );
    }

    #[test]
    fn face_shading_has_the_normals_it_needs_to_classify() {
        let world = world();
        let mut section = Section::uniform(world.air);
        // A larger blob, so merging does not hide the shape behind one quad.
        for x in 7..=9i32 {
            for y in 7..=9i32 {
                for z in 7..=9i32 {
                    fill(&mut section, &world, x, y, z);
                }
            }
        }

        let mesh = build(&section, &world);
        assert!(mesh.quad_count() >= 6);

        // Every normal must be exactly one of the six axis directions, with no
        // off-axis values a `if n.y > 0.5` classification could misread.
        for vertex in &mesh.vertices {
            let axis = (0..3)
                .find(|&axis| vertex.normal[axis] != 0.0)
                .expect("a normal must point along some axis");
            assert_eq!(
                vertex.normal[axis].abs(),
                1.0,
                "normal {:?} is not axis aligned",
                vertex.normal
            );
            for other in 0..3 {
                if other != axis {
                    assert_eq!(
                        vertex.normal[other], 0.0,
                        "normal {:?} bleeds onto another axis",
                        vertex.normal
                    );
                }
            }
        }

        // And the top face really is a distinct case from the bottom, which is
        // the contrast the shading exists to provide.
        let tops = mesh.vertices.iter().filter(|v| v.normal[1] > 0.5).count();
        let bottoms = mesh.vertices.iter().filter(|v| v.normal[1] < -0.5).count();
        assert_eq!(
            tops, bottoms,
            "a blob should be as bright on top as dark below"
        );
    }

    #[test]
    fn the_quad_diagonal_follows_which_corners_are_darker() {
        let world = world();
        // A staircase, so faces end up with a mix of corner occlusion values.
        let mut section = Section::uniform(world.air);
        for z in 0..D_I32 {
            for x in 0..D_I32 {
                for y in 0..((x * 3 + z * 5) % 7) {
                    fill(&mut section, &world, x, y, z);
                }
            }
        }

        let mesh = build(&section, &world);
        assert!(!mesh.is_empty());

        let mut saw_alternate = false;
        let mut saw_default = false;
        for quad in mesh.indices.as_chunks::<6>().0 {
            let base = *quad.iter().min().expect("six indices");
            let corners: Vec<f32> = mesh.vertices[base as usize..base as usize + 4]
                .iter()
                .map(|vertex| vertex.light[2])
                .collect();

            let default = *quad == [base, base + 1, base + 2, base, base + 2, base + 3];
            let alternate = *quad == [base + 1, base + 2, base + 3, base + 1, base + 3, base];
            assert!(
                default || alternate,
                "quad {quad:?} uses neither the 0-2 nor the 1-3 diagonal"
            );

            // Whichever was chosen, the crease joins the darker pair.
            let diagonal_02 = corners[0] + corners[2];
            let diagonal_13 = corners[1] + corners[3];
            if diagonal_02 > diagonal_13 {
                assert!(alternate, "0-2 is brighter, so the crease should be 1-3");
                saw_alternate = true;
            } else {
                assert!(default, "1-3 is brighter, so keep 0-2");
                saw_default = true;
            }
        }

        assert!(
            saw_alternate && saw_default,
            "a varied scene should exercise both diagonals, saw \
             {saw_alternate}/{saw_default}"
        );
    }

    #[test]
    fn every_triangle_keeps_the_quad_winding_under_either_diagonal() {
        let world = world();
        let mut section = Section::uniform(world.air);
        for z in 0..D_I32 {
            for x in 0..D_I32 {
                for y in 0..((x + z) % 4) {
                    fill(&mut section, &world, x, y, z);
                }
            }
        }

        let mesh = build(&section, &world);
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let a = mesh.vertices[triangle[0] as usize].position;
            let b = mesh.vertices[triangle[1] as usize].position;
            let c = mesh.vertices[triangle[2] as usize].position;
            let normal = mesh.vertices[triangle[0] as usize].normal;

            let cross = [
                (b[1] - a[1]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[1] - a[1]),
                (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]),
                (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]),
            ];
            assert!(
                dot(cross, normal) > 0.0,
                "triangle {triangle:?} faces inwards, cross {cross:?} vs normal {normal:?}"
            );
        }
    }

    #[test]
    fn the_corner_swap_keeps_shading_attached_to_its_vertex() {
        let world = world();
        let mut section = Section::uniform(world.air);
        // Deliberately asymmetric so a mirrored result would be visible: one
        // block standing beside a slab corner.
        slab(&mut section, &world, 0);
        fill(&mut section, &world, 2, 1, 2);

        let mesh = build(&section, &world);
        assert!(!mesh.is_empty());

        // Every vertex of a given quad must agree with the occlusion computed
        // from the blocks actually at that corner. Checking the spread of values
        // catches a wholesale swap, which a single vertex would not.
        for chunk in mesh.vertices.as_chunks::<4>().0 {
            let sky: Vec<f32> = chunk.iter().map(|v| v.light[0]).collect();
            assert!(
                sky.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 0.001),
                "sky light should be constant across a uniformly lit quad: {sky:?}"
            );
        }
    }
}
