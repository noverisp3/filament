//! Block identity and the global block registry.

use std::collections::HashMap;

/// A dense index into [`BlockRegistry`].
///
/// Deliberately a plain `u16` rather than Minecraft's shared `BlockState`
/// object: no identity comparison, no pointers, and a paletted section stores
/// these directly instead of 8-byte references.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(u16);

impl BlockId {
    pub const fn from_index(index: u16) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Light a block gives off, 0 to 15.
///
/// Separate from `opaque` on purpose: a glowstone block is opaque *and*
/// luminous, and a torch would be luminous without being opaque. Collapsing
/// the two would make "blocks sight" and "blocks emit" the same concept, which
/// they are not.
pub const MAX_LIGHT: u8 = 15;

/// What the engine needs to know about a block type.
///
/// `opaque` drives face culling, and `color` is per-vertex data. Both were
/// added in phase 3, which is the first consumer; retrofitting them later would
/// mean touching the registry, the mesher, and every stored world at once.
/// `light_emission` arrived with phase 4b for the same reason.
#[derive(Clone, Debug)]
pub struct BlockDef {
    pub name: String,
    pub opaque: bool,
    pub color: [f32; 3],
    /// Light this block emits, 0 to [`MAX_LIGHT`]. Feeds the block-light layer,
    /// which is computed separately from the sky layer.
    pub light_emission: u8,
}

/// Registry mapping block names to dense ids.
///
/// Frozen in practice: ids are baked into paletted sections, so a registry that
/// changed order would silently reinterpret saved worlds. Build it, then stop.
#[derive(Clone, Debug, Default)]
pub struct BlockRegistry {
    blocks: Vec<BlockDef>,
    by_name: HashMap<String, BlockId>,
}

impl BlockRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a block, returning its id. Registering an existing name
    /// returns the existing id rather than adding a duplicate.
    pub fn register(&mut self, name: impl Into<String>, opaque: bool, color: [f32; 3]) -> BlockId {
        self.register_emitting(name, opaque, color, 0)
    }

    /// Registers a block that gives off light.
    ///
    /// Emission above [`MAX_LIGHT`] is rejected rather than clamped: a caller
    /// that passes 16 has a bug, and silently storing 15 would hide it.
    pub fn register_emitting(
        &mut self,
        name: impl Into<String>,
        opaque: bool,
        color: [f32; 3],
        light_emission: u8,
    ) -> BlockId {
        assert!(
            light_emission <= MAX_LIGHT,
            "light emission {light_emission} exceeds the maximum {MAX_LIGHT}"
        );
        let name = name.into();
        if let Some(&existing) = self.by_name.get(&name) {
            return existing;
        }

        let id = BlockId::from_index(self.blocks.len() as u16);
        self.by_name.insert(name.clone(), id);
        self.blocks.push(BlockDef {
            name,
            opaque,
            color,
            light_emission,
        });
        id
    }

    pub fn id(&self, name: &str) -> Option<BlockId> {
        self.by_name.get(name).copied()
    }

    pub fn get(&self, id: BlockId) -> Option<&BlockDef> {
        self.blocks.get(id.index())
    }

    pub fn name(&self, id: BlockId) -> &str {
        self.get(id)
            .map(|block| block.name.as_str())
            .unwrap_or("<unknown>")
    }

    /// False for unregistered ids, which reads as "does not block sight".
    pub fn is_opaque(&self, id: BlockId) -> bool {
        self.get(id).is_some_and(|block| block.opaque)
    }

    /// Light emitted by a block, or 0 for an unregistered id.
    pub fn light_emission(&self, id: BlockId) -> u8 {
        self.get(id).map_or(0, |block| block.light_emission)
    }

    pub fn color(&self, id: BlockId) -> [f32; 3] {
        self.get(id)
            .map(|block| block.color)
            .unwrap_or([1.0, 0.0, 1.0])
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

#[cfg(test)]
mod tests {
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
    fn re_registering_a_name_keeps_the_original_emission() {
        let mut registry = BlockRegistry::new();
        let first = registry.register_emitting("glowstone", true, [1.0, 1.0, 1.0], 15);
        let again = registry.register_emitting("glowstone", true, [1.0, 1.0, 1.0], 1);
        assert_eq!(first, again);
        assert_eq!(
            registry.light_emission(first),
            15,
            "the first definition wins, as with opacity"
        );
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
}
