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