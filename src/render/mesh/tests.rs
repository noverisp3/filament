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