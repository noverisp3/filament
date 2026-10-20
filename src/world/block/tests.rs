use super::*;

#[test]
fn ids_are_dense_and_stable() {
    let mut registry = BlockRegistry::new();
    let air = registry.register("air", false, [0.0, 0.0, 0.0]);
    let stone = registry.register("stone", true, [0.50, 0.50, 0.52]);
    let dirt = registry.register("dirt", true, [0.45, 0.32, 0.20]);

    assert_eq!(air.index(), 0);
    assert_eq!(stone.index(), 1);
    assert_eq!(dirt.index(), 2);
    assert_eq!(registry.len(), 3);
    assert_eq!(registry.name(stone), "stone");
    assert!(registry.get(stone).expect("registered").opaque);
    assert!(!registry.get(air).expect("registered").opaque);
}

#[test]
fn re_registering_a_name_reuses_the_id() {
    let mut registry = BlockRegistry::new();
    let first = registry.register("stone", true, [0.50, 0.50, 0.52]);
    let again = registry.register("stone", false, [1.0, 0.0, 1.0]);
    assert_eq!(first, again);
    assert_eq!(registry.len(), 1);
    // The first definition wins, so opacity is not silently changed.
    assert!(registry.get(first).expect("registered").opaque);
}

#[test]
fn emission_is_stored_and_defaults_to_nothing() {
    let mut registry = BlockRegistry::new();
    let stone = registry.register("stone", true, [0.50, 0.50, 0.52]);
    let glowstone = registry.register_emitting("glowstone", true, [0.96, 0.85, 0.55], 15);

    assert_eq!(
        registry.light_emission(stone),
        0,
        "plain blocks emit nothing"
    );
    assert_eq!(registry.light_emission(glowstone), MAX_LIGHT);
    // Emission is independent of opacity: both happen to be opaque here,
    // but nothing forces them to be.
    assert!(registry.is_opaque(glowstone));
}

#[test]
fn an_unregistered_id_emits_nothing() {
    let registry = BlockRegistry::new();
    assert_eq!(registry.light_emission(BlockId::from_index(999)), 0);
}

#[test]
#[should_panic(expected = "exceeds the maximum")]
fn emission_above_the_maximum_is_a_bug_not_a_clamp() {
    let mut registry = BlockRegistry::new();
    registry.register_emitting("broken", true, [1.0, 1.0, 1.0], MAX_LIGHT + 1);
}

#[test]
fn re_registering_a_name_keeps_the_id_and_the_strongest_light() {
    let mut registry = BlockRegistry::new();
    let first = registry.register_emitting("glowstone", true, [1.0, 1.0, 1.0], 15);
    let again = registry.register_emitting("glowstone", true, [1.0, 1.0, 1.0], 1);
    assert_eq!(first, again, "the same name is the same block");
    assert_eq!(
        registry.light_emission(first),
        15,
        "a block that was said to glow brightly does not stop glowing because
         it was heard of again more quietly"
    );
}

#[test]
fn the_lamps_of_the_game_light_as_the_game_lights_them() {
    // The numbers are the game's, not guesses: a torch is fourteen and a
    // redstone torch is seven, and a glowstone is fifteen.
    assert_eq!(emission_for_name("minecraft:glowstone"), 15);
    assert_eq!(emission_for_name("glowstone"), 15, "and the namespace is optional");
    assert_eq!(emission_for_name("minecraft:torch"), 14);
    assert_eq!(emission_for_name("minecraft:redstone_torch"), 7);
    assert_eq!(emission_for_name("minecraft:magma_block"), 3);
    assert_eq!(emission_for_name("minecraft:stone"), 0);
    assert_eq!(emission_for_name("minecraft:air"), 0);
}

#[test]
fn unknown_lookups_are_not_panics() {
    let mut registry = BlockRegistry::new();
    let stone = registry.register("stone", true, [0.50, 0.50, 0.52]);
    assert_eq!(registry.id("missing"), None);
    assert_eq!(registry.name(BlockId::from_index(999)), "<unknown>");
    assert!(registry.get(BlockId::from_index(999)).is_none());
    assert_eq!(registry.name(stone), "stone");
}