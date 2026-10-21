//! Tests over the generated tables.
//!
//! These check the table is internally consistent and matches what Minecraft
//! reports. They cannot check it matches a running client, which is the one
//! check that matters, but they catch the failure mode that is otherwise
//! invisible: a table that parses cleanly and is quietly wrong.

use std::collections::HashSet;

use super::*;

/// Every block's states have to occupy a distinct range of ids.
///
/// This is the invariant the whole table exists to satisfy. A server sends these
/// ids and the client looks them up, so two blocks sharing an id means one of
/// them renders as the other, with no error on either side.
#[test]
fn no_two_states_share_an_id() {
    let mut seen = HashSet::new();
    let mut total = 0usize;

    for block in BLOCKS {
        for state in block.states {
            total += 1;
            assert!(
                seen.insert(state.id),
                "state id {} appears twice; block {} collides with an earlier one",
                state.id,
                block.name
            );
        }
    }

    assert_eq!(
        total, BLOCK_STATE_COUNT as usize,
        "BLOCK_STATE_COUNT disagrees with the number of states listed"
    );
    assert_eq!(
        total,
        seen.len(),
        "duplicate ids mean the table has fewer states than it claims"
    );
}

/// Ids must run from zero to the highest with nothing missing.
///
/// A gap is as fatal as a duplicate: the client would have a state the server
/// can never name.
#[test]
fn state_ids_are_contiguous_from_zero() {
    let mut max: u32 = 0;
    let mut count = 0usize;

    for block in BLOCKS {
        for state in block.states {
            max = max.max(state.id);
            count += 1;
        }
    }

    assert_eq!(
        count, max as usize + 1,
        "state ids are not contiguous: highest is {max} but there are {count} states, \
         so {} ids are unused",
        (max as i64 + 1) - count as i64
    );
}

/// Every block must list a default state that it actually contains.
#[test]
fn every_default_state_belongs_to_its_block() {
    for block in BLOCKS {
        assert!(
            block.states.iter().any(|s| s.id == block.default_state),
            "block {} declares default {} which is not one of its states",
            block.name,
            block.default_state
        );
    }
}

/// A block with more than one state must say what distinguishes them.
///
/// A multi-state block whose states all carry empty property lists cannot be
/// looked up by property, which is the only way to convert a state into
/// something a player can hold.
#[test]
fn a_multi_state_block_describes_its_states() {
    for block in BLOCKS {
        if block.states.len() <= 1 {
            continue;
        }
        for state in block.states {
            assert!(
                !state.properties.is_empty(),
                "block {} has {} states but state {} has no properties, so it cannot be \
                 identified by anything other than its bare id",
                block.name,
                block.states.len(),
                state.id
            );
        }
    }
}

/// Two states of the same block must not claim the same property values.
///
/// This would make the block unnameable: a player holding "oak stairs facing
/// north, top half" could match two different states.
#[test]
fn no_two_states_of_one_block_are_indistinguishable() {
    for block in BLOCKS {
        let mut seen: HashSet<&[Property]> = HashSet::new();

        for state in block.states {
            if state.properties.is_empty() {
                continue;
            }
            assert!(
                seen.insert(state.properties),
                "block {} has two states with identical properties: {:?}",
                block.name,
                state.properties
            );
        }
    }
}

/// Names must be namespaced and lower case.
///
/// Names come from Minecraft's own identifiers, but they end up in lookup
/// tables and packet payloads where an inconsistent case is a silent mismatch.
#[test]
fn block_names_are_well_formed() {
    for block in BLOCKS {
        assert!(
            block.name.contains(':'),
            "block name {} is missing its namespace",
            block.name
        );
        assert!(
            block.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ":_/".contains(c)),
            "block name {} has unexpected characters",
            block.name
        );
    }
}

/// Air must exist and be at id zero.
///
/// Chunk data is a paletted container whose first entry is whatever the server
/// puts there, and an empty section is filled with the default. If air were not
/// zero, every empty section would be filled with something else.
#[test]
fn air_is_block_zero() {
    let air = BLOCKS
        .iter()
        .find(|b| b.name == "minecraft:air")
        .expect("the registry must contain air");

    assert_eq!(
        air.default_state, 0,
        "air must own state id 0 so empty sections default to it"
    );
    assert_eq!(
        air.states.len(),
        1,
        "air has exactly one state, otherwise empty chunks are ambiguous"
    );
}

/// The table must be as large as Minecraft's.
///
/// A truncated parse is the most likely silent failure here: the generator would
/// still emit valid Rust, just fewer entries.
///
/// Comparing the counts against a constant would not catch it, because both
/// sides would move together and clippy is right to call that a constant
/// assertion. What catches it is checking for blocks the server cannot run
/// without.
#[test]
fn the_registry_contains_the_blocks_the_server_needs() {
    for required in [
        "minecraft:air",
        "minecraft:stone",
        "minecraft:dirt",
        "minecraft:grass_block",
        "minecraft:bedrock",
        "minecraft:water",
        "minecraft:sand",
        "minecraft:gravel",
        "minecraft:deepslate",
        "minecraft:oak_log",
    ] {
        assert!(
            BLOCKS.iter().any(|b| b.name == required),
            "{required} is missing, which means the registry was parsed only in part"
        );
    }
}

/// The blocks that stop a player from breathing are the ones the game says.
///
/// The game asks whether a block is in the `causes_suffocation` tag, which is
/// the blocks a player's head cannot be inside: stone and wood and wool, and
/// not glass and not water and not leaves. A table that had water in it would
/// drown a player who is swimming, and a table that had leaves in it would
/// suffocate a player under a tree.
#[test]
fn the_blocks_that_stop_a_player_breathing_are_the_games_ones() {
    let state_of = |name: &str| {
        BLOCKS
            .iter()
            .find(|b| b.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from the registry"))
            .default_state as i32
    };

    for stops in [
        "minecraft:stone",
        "minecraft:dirt",
        "minecraft:oak_planks",
        "minecraft:white_wool",
        "minecraft:farmland",
    ] {
        assert!(
            causes_suffocation(state_of(stops)),
            "{stops} stops a player from breathing"
        );
    }

    for does_not_stop in [
        "minecraft:air",
        "minecraft:water",
        "minecraft:glass",
        "minecraft:torch",
        "minecraft:oak_leaves",
    ] {
        assert!(
            !causes_suffocation(state_of(does_not_stop)),
            "{does_not_stop} does not stop a player from breathing"
        );
    }
}

/// Every state of a block stops a player the same way.
///
/// The table used to hold only the default state of each block, so a log that
/// was not standing up did not stop a player from breathing, and a player
/// walled in by one suffocated very slowly or not at all.
#[test]
fn every_state_of_a_block_stops_a_player_the_same_way() {
    let states_of = |name: &str| {
        BLOCKS
            .iter()
            .find(|b| b.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from the registry"))
            .states
            .iter()
            .map(|s| s.id as i32)
            .collect::<Vec<_>>()
    };

    for name in ["minecraft:oak_log", "minecraft:oak_slab", "minecraft:oak_stairs"] {
        let states = states_of(name);
        assert!(states.len() > 1, "{name} has one state, so this proves nothing");
        for state in states {
            assert!(
                causes_suffocation(state),
                "{name} in state {state} stops a player from breathing"
            );
        }
    }

    for name in ["minecraft:oak_leaves", "minecraft:glass"] {
        for state in states_of(name) {
            assert!(
                !causes_suffocation(state),
                "{name} in state {state} does not stop a player from breathing"
            );
        }
    }
}

/// The light a block takes away, the way the game takes it away.
///
/// The rules are the game's own: a full cube that hides what is behind it
/// takes all fifteen, a full cube that does not hide takes one, a block that
/// is not a full cube but holds a fluid takes one, and a block that is not a
/// full cube at all takes none, because the light goes down through it.
#[test]
fn the_light_a_block_takes_away_is_the_games_own() {
    let state_of = |name: &str| {
        BLOCKS
            .iter()
            .find(|b| b.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from the registry"))
            .default_state as i32
    };

    for (name, takes) in [
        ("minecraft:stone", 15),
        ("minecraft:dirt", 15),
        ("minecraft:grass_block", 15),
        ("minecraft:oak_log", 15),
        ("minecraft:white_wool", 15),
        ("minecraft:glass", 1),
        ("minecraft:oak_leaves", 1),
        ("minecraft:water", 1),
        ("minecraft:lava", 1),
        ("minecraft:oak_slab", 0),
        ("minecraft:torch", 0),
        ("minecraft:short_grass", 0),
        ("minecraft:air", 0),
    ] {
        assert_eq!(
            light_dampening(state_of(name)),
            takes,
            "{name} takes {takes} of the light"
        );
    }
}

/// The blocks that hide what is behind them, and the ones that do not.
///
/// Glass and leaves let the light through and take one level doing it, and
/// a plant lets it through and takes nothing.
#[test]
fn the_blocks_that_hide_are_the_games_ones() {
    let state_of = |name: &str| {
        BLOCKS
            .iter()
            .find(|b| b.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from the registry"))
            .default_state as i32
    };

    for hides in [
        "minecraft:stone",
        "minecraft:dirt",
        "minecraft:oak_leaves",
        "minecraft:glass",
    ] {
        let state = state_of(hides);
        if hides == "minecraft:oak_leaves" || hides == "minecraft:glass" {
            assert!(!occludes(state), "{hides} does not hide what is behind it");
        } else {
            assert!(occludes(state), "{hides} hides what is behind it");
        }
    }

    for full in [
        "minecraft:stone",
        "minecraft:glass",
        "minecraft:oak_leaves",
        "minecraft:oak_log",
    ] {
        assert!(
            is_full_cube(state_of(full)),
            "{full} is a whole cell of a block"
        );
    }

    for not_full in [
        "minecraft:oak_slab",
        "minecraft:torch",
        "minecraft:short_grass",
        "minecraft:water",
    ] {
        assert!(
            !is_full_cube(state_of(not_full)),
            "{not_full} is not a whole cell of a block"
        );
    }

    for fluid in ["minecraft:water", "minecraft:lava"] {
        assert!(has_fluid(state_of(fluid)), "{fluid} holds a fluid");
    }
    assert!(!has_fluid(state_of("minecraft:stone")));
}
