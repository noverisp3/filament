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

    /// The number of quads the mesh ended up with.
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
    /// is why shading needs the corner neighbours and not just the six face
    /// neighbours.
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
    // corners.
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
mod tests;
