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
fn a_chunk_works_out_its_own_light_when_it_arrives() {
    // The game does not keep the light in the chunk file: it works it out
    // again when the chunk is loaded. Without that, a chunk read from disk
    // arrives saying the whole world is in full daylight, and a cave in it
    // is as bright as its top.
    let mut world = World::generate(1);
    let surface = surface_y(8, 8);

    // A chunk that has just been generated, with no whole-world pass over it.
    let chunk = ChunkPos::new(0, 0);
    world.ensure_chunk(chunk);

    assert_eq!(
        world.sky_light(BlockPos::new(8, 300, 8)),
        MAX_LIGHT,
        "the sky above the ground sees the sun"
    );
    assert_eq!(
        world.sky_light(BlockPos::new(8, surface, 8)),
        0,
        "and the ground itself does not"
    );
    assert_eq!(
        world.sky_light(BlockPos::new(8, surface - 3, 8)),
        0,
        "nor does the ground under it"
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