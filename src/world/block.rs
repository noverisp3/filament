//! Block identity and the global block registry.

use std::collections::HashMap;

/// Light a named block gives off, 0 to 15.
///
/// The game keeps this on the block state, and the state is what the world
/// file names, so a name is all there is to go on here. The list is the
/// game's own: a torch is fourteen and a redstone torch is seven, and a
/// glowstone is fifteen like a sea lantern and a jack o'lantern.
///
/// A name with no namespace and a name with one are the same block, so the
/// namespace is dropped before the name is looked up.
pub fn emission_for_name(name: &str) -> u8 {
    let name = name.rsplit(':').next().unwrap_or(name);
    match name {
        "glowstone" | "sea_lantern" | "lantern" | "jack_o_lantern" | "shroomlight"
        | "beacon" | "conduit" | "fire" | "campfire" | "froglight" | "ochre_froglight"
        | "verdant_froglight" | "pearlescent_froglight" | "chiseled_bookshelf" | "firefly_bush" => 15,
        "torch" | "end_rod" => 14,
        "soul_lantern" | "soul_torch" | "soul_campfire" | "crying_obsidian" => 10,
        "redstone_torch" | "ender_chest" | "enchanting_table" | "glow_lichen" => 7,
        "respawn_anchor" => 4,
        "magma_block" => 3,
        "nether_portal" => 11,
        "brewing_stand" | "dragon_egg" | "end_portal" | "end_portal_frame" | "end_gateway"
        | "sculk_shrieker" | "sea_pickle" | "mob_spawner" | "decorated_pot" | "torchflower"
        | "pink_petals" => 1,
        _ => 0,
    }
}

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
            // A block that is already known may have been heard of before its
            // light was: a file that says a block glows is taken at its word,
            // and a block that glows more than it was said to glows more.
            if let Some(block) = self.blocks.get_mut(existing.index())
                && light_emission > block.light_emission
            {
                block.light_emission = light_emission;
            }
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
mod tests;
