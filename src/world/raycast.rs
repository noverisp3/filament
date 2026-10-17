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
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A solid block set; everything else is transparent.
    fn world(solid: &HashSet<(i32, i32, i32)>) -> impl Fn(BlockPos) -> bool + '_ {
        move |pos: BlockPos| solid.contains(&(pos.x, pos.y, pos.z))
    }

    #[test]
    fn a_ray_into_empty_space_misses() {
        let none = HashSet::new();
        assert_eq!(
            raycast([0.5, 0.5, 0.5], [0.0, 0.0, 1.0], 64.0, &world(&none)),
            None
        );
    }

    #[test]
    fn a_ray_stops_at_the_first_solid_block() {
        let mut solid = HashSet::new();
        solid.insert((0, 0, 5));
        let hit =
            raycast([0.5, 0.5, 0.5], [0.0, 0.0, 1.0], 64.0, &world(&solid)).expect("should hit");
        assert_eq!((hit.block.x, hit.block.y, hit.block.z), (0, 0, 5));
        assert_eq!(hit.normal, [0, 0, -1], "entered through the -Z face");
    }

    #[test]
    fn the_normal_points_back_toward_the_ray() {
        let cases = [
            ([1.0, 0.0, 0.0], [3, 0, 0], [-1, 0, 0]),
            ([-1.0, 0.0, 0.0], [-3, 0, 0], [1, 0, 0]),
            ([0.0, 1.0, 0.0], [0, 3, 0], [0, -1, 0]),
            ([0.0, -1.0, 0.0], [0, -3, 0], [0, 1, 0]),
            ([0.0, 0.0, 1.0], [0, 0, 3], [0, 0, -1]),
            ([0.0, 0.0, -1.0], [0, 0, -3], [0, 0, 1]),
        ];

        for (direction, target, expected) in cases {
            let mut solid = HashSet::new();
            solid.insert((target[0], target[1], target[2]));
            let hit =
                raycast([0.5, 0.5, 0.5], direction, 64.0, &world(&solid)).expect("should hit");
            assert_eq!(
                hit.normal, expected,
                "direction {direction:?} should report normal {expected:?}"
            );
        }
    }

    #[test]
    fn a_diagonal_ray_crosses_both_axes() {
        let mut solid = HashSet::new();
        // (2,2,2) is the first cell along a 45-degree ray from (0.5,0.5,0.5).
        solid.insert((2, 2, 2));
        let hit = raycast(
            [0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0].map(|c| c / 3f64.sqrt()),
            64.0,
            &world(&solid),
        )
        .expect("should hit");
        assert_eq!((hit.block.x, hit.block.y, hit.block.z), (2, 2, 2));
    }

    #[test]
    fn max_distance_stops_the_walk() {
        let mut solid = HashSet::new();
        solid.insert((0, 0, 30));
        // The block is 29.5 units away, but the walk is capped at 10.
        assert_eq!(
            raycast([0.5, 0.5, 0.5], [0.0, 0.0, 1.0], 10.0, &world(&solid)),
            None
        );
        assert!(raycast([0.5, 0.5, 0.5], [0.0, 0.0, 1.0], 40.0, &world(&solid)).is_some());
    }

    #[test]
    fn starting_inside_solid_hits_immediately() {
        let mut solid = HashSet::new();
        solid.insert((0, 0, 0));
        let hit =
            raycast([0.5, 0.5, 0.5], [0.0, 0.0, 1.0], 64.0, &world(&solid)).expect("should hit");
        assert_eq!((hit.block.x, hit.block.y, hit.block.z), (0, 0, 0));
        assert_eq!(hit.normal, [0, 0, 0], "no face was crossed");
    }

    #[test]
    fn an_axis_aligned_ray_never_costs_extra_steps() {
        // A straight +X ray must not wander into neighbouring Y or Z cells, or
        // a thin wall would be hit from the side.
        let mut solid = HashSet::new();
        solid.insert((10, 0, 0));
        // A wall around x = 7, with the one cell the ray actually passes
        // through left open. A straight ray must not wander sideways into it.
        for y in -3..=3 {
            for z in -3..=3 {
                if y == 0 && z == 0 {
                    continue;
                }
                solid.insert((7, y, z));
            }
        }
        let hit =
            raycast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 64.0, &world(&solid)).expect("should hit");
        assert_eq!((hit.block.x, hit.block.y, hit.block.z), (10, 0, 0));
    }

    #[test]
    fn negative_coordinates_are_handled() {
        // A wall at x = -3 spanning every neighbouring y and z. Firing straight
        // down -X from a cell centre must reach (-3, 0, 0) and enter it through
        // its +X face.
        let mut solid = HashSet::new();
        for y in -3..=3 {
            for z in -3..=3 {
                solid.insert((-3, y, z));
            }
        }

        let hit =
            raycast([0.5, 0.5, 0.5], [-1.0, 0.0, 0.0], 64.0, &world(&solid)).expect("should hit");

        assert_eq!((hit.block.x, hit.block.y, hit.block.z), (-3, 0, 0));
        assert_eq!(hit.normal, [1, 0, 0], "entered through the +X face");
    }

    /// A ray heading into negative space on several axes at once still has to
    /// land inside solid space.
    #[test]
    fn a_diagonal_ray_reaches_negative_space() {
        // A large solid cube in the negative octant. The axis ratios differ
        // enough that a thin slab would be stepped over, so this is deliberately
        // generous about what counts as a hit.
        let mut solid = HashSet::new();
        for x in -20..=-3 {
            for y in -20..=-3 {
                for z in -20..=-3 {
                    solid.insert((x, y, z));
                }
            }
        }

        let length = 3f64.sqrt();
        let hit = raycast(
            [0.5, 0.5, 0.5],
            [-1.0 / length, -1.0 / length, -1.0 / length],
            64.0,
            &world(&solid),
        )
        .expect("should hit the cube");

        assert!(
            hit.block.x < 0 && hit.block.y < 0 && hit.block.z < 0,
            "expected a cell in negative space, got {:?}",
            hit.block
        );
        // All three axes advance together, so all three are crossed in the same
        // step and exactly one of them supplies the face.
        assert_eq!(
            hit.normal.iter().filter(|n| **n != 0).count(),
            1,
            "exactly one face is reported"
        );
    }

    #[test]
    fn a_zero_direction_component_does_not_divide_by_zero() {
        let mut solid = HashSet::new();
        solid.insert((0, 0, 20));
        // Perfectly horizontal: Y and Z t_max are infinite, X must win every
        // comparison without producing NaN.
        let hit =
            raycast([0.5, 0.5, 0.5], [0.0, 0.0, 1.0], 64.0, &world(&solid)).expect("should hit");
        assert_eq!((hit.block.x, hit.block.y, hit.block.z), (0, 0, 20));

        let none = HashSet::new();
        assert_eq!(
            raycast([0.5, 0.5, 0.5], [0.0, 0.0, 1.0], 64.0, &world(&none)),
            None
        );
    }

    #[test]
    fn a_degenerate_direction_is_rejected() {
        let none = HashSet::new();
        assert_eq!(
            raycast([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 64.0, &world(&none)),
            None
        );
        // NaN would make every comparison false and hang the walk.
        assert_eq!(
            raycast([0.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0], 64.0, &world(&none)),
            None
        );
        assert_eq!(
            raycast(
                [0.0, 0.0, 0.0],
                [f64::INFINITY, 0.0, 0.0],
                64.0,
                &world(&none)
            ),
            None
        );
    }
}
