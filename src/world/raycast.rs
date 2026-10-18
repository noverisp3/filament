//! Picking a block by walking a ray through the voxel grid.
//!
//! Amanatides & Woo grid traversal: step from voxel to voxel along whichever
//! axis reaches its next boundary first, rather than sampling at fixed
//! intervals and hoping not to miss a block.
//!
//! The traversal is kept free of world types. It takes a closure that answers
//! "is this block solid", so the stepping arithmetic can be tested against a
//! hand-written grid without building a world.

use crate::world::pos::BlockPos;

/// A block the ray stopped inside of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayHit {
    /// The block that stopped the ray.
    pub block: BlockPos,
    /// Face that was entered, pointing back toward the ray origin. This is the
    /// face a new block would be placed against.
    pub normal: [i32; 3],
}

/// Casts `ray` and returns the first solid block it enters.
///
/// `direction` must be normalised; `max_distance` bounds the walk so a ray
/// pointing away from the world still terminates.
pub fn raycast(
    origin: [f64; 3],
    direction: [f64; 3],
    max_distance: f64,
    is_solid: &dyn Fn(BlockPos) -> bool,
) -> Option<RayHit> {
    // A zero or non-finite direction has no defined traversal. NaN in particular
    // must be rejected: every comparison against it is false, so the walk would
    // never terminate and would spin until the cell coordinates overflowed.
    if !direction.iter().all(|d| d.is_finite()) || direction.iter().all(|d| *d == 0.0) {
        return None;
    }

    let mut cell = [
        origin[0].floor() as i32,
        origin[1].floor() as i32,
        origin[2].floor() as i32,
    ];

    // Starting inside something solid counts as a hit, so the player cannot
    // place a block inside themselves.
    if is_solid(BlockPos::new(cell[0], cell[1], cell[2])) {
        return Some(RayHit {
            block: BlockPos::new(cell[0], cell[1], cell[2]),
            normal: [0, 0, 0],
        });
    }

    // Step one voxel along each axis per boundary crossing.
    let step = [
        if direction[0] > 0.0 { 1 } else { -1 },
        if direction[1] > 0.0 { 1 } else { -1 },
        if direction[2] > 0.0 { 1 } else { -1 },
    ];

    // Distance along the ray between successive boundaries on each axis. A
    // component of zero never crosses, so its delta is infinite and it never
    // wins the comparison.
    let t_delta = [
        delta_for(direction[0]),
        delta_for(direction[1]),
        delta_for(direction[2]),
    ];

    // Distance to the first boundary on each axis.
    let mut t_max = [0.0f64; 3];
    for axis in 0..3 {
        t_max[axis] = if direction[axis] == 0.0 {
            f64::INFINITY
        } else {
            let boundary = if direction[axis] > 0.0 {
                (cell[axis] + 1) as f64
            } else {
                cell[axis] as f64
            };
            (boundary - origin[axis]) / direction[axis]
        };
    }

    // Invariant: t_max[axis] is always the next crossing, and t_delta is the
    // gap to the one after it.
    loop {
        let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };

        if t_max[axis] > max_distance {
            return None;
        }

        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];

        let pos = BlockPos::new(cell[0], cell[1], cell[2]);
        if is_solid(pos) {
            let mut normal = [0i32; 3];
            normal[axis] = -step[axis];
            return Some(RayHit { block: pos, normal });
        }
    }
}

/// Distance along the ray between two consecutive voxel boundaries.
#[inline]
fn delta_for(direction: f64) -> f64 {
    if direction == 0.0 {
        f64::INFINITY
    } else {
        (1.0 / direction).abs()
    }
}

#[cfg(test)]
mod tests;
