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