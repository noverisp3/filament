//! Block identity and the global block registry.

use std::collections::HashMap;

/// Light a named block gives off, 0 to 15.
///
/// The game keeps this on the block state, and the state is what the world
/// file names, so a name is all there is to go on here.
///
/// The numbers are the game's own, read out of the block definitions by
/// `tools/extract_light.py` into `src/generated/light.rs`: a torch is fourteen
/// and a redstone torch is seven, and a glowstone is fifteen like a sea
/// lantern and a jack o'lantern.
///
/// They used to be a list typed by hand here, and a list typed by hand is a
/// list that is always missing something -- and a lamp the server does not
/// know about is a lamp the server does not light, which is a dark lamp in
/// the world and no word about it.
///
/// A name with no namespace and a name with one are the same block, so the
/// namespace is dropped before the name is looked up.
pub fn emission_for_name(name: &str) -> u8 {
    let name = name.rsplit(':').next().unwrap_or(name);
    let mut full = String::with_capacity(name.len() + 10);
    full.push_str("minecraft:");
    full.push_str(name);
    crate::generated::LIGHT
        .iter()
        .find(|(block, _)| *block == full)
        .map_or(0, |(_, light)| *light)
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
