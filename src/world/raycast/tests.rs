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