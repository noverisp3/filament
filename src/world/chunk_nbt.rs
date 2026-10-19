//! A chunk as the NBT the region file stores, and back.
//!
//! The shape is not invented. `SerializableChunkData.write()` decides the keys
//! and `SimpleBitStorage` decides how the bits sit inside the longs, and both
//! are easy to get subtly wrong in a way that still loads -- the file is
//! readable, it just describes a different world. So the notes below are only
//! where something is surprising.
//!
//! What is written: position, status, one entry per section with its blocks,
//! biomes and Y, plus the empty scaffolding the reader expects to be present
//! (`block_entities`, `block_ticks`, `fluid_ticks`, `PostProcessing`,
//! `structures`). Light is not written -- `isLightOn` is false, so the reader
//! knows to relight rather than trust what is in the file.
//!
//! What is not written: entities, structures' contents, `UpgradeData`,
//! `blending_data`. Filament has none of them, and leaving them out is what the
//! game does for a field it has nothing for.

use super::block::{BlockId, BlockRegistry};
use super::chunk::{Chunk, MIN_BLOCK_Y, MIN_SECTION_Y, SECTIONS_PER_CHUNK};
use super::nbt::{Compound, Tag};
use super::palette::bits_for_palette_len;
use super::pos::{ChunkPos, LocalPos, SECTION_DIM_USIZE, SECTION_VOLUME};
use super::section::Section;

/// `DataVersion` for this game, from `DetectedVersion.java`.
///
/// A chunk written with the wrong one still parses. The number decides which
/// upgrades run on load, so a stale value is a world that is quietly migrated
/// when it should not be.
pub const DATA_VERSION: i32 = 5023;

/// `ChunkStatus.FULL`, the last a chunk is complete enough to save.
const STATUS_FULL: &str = "minecraft:full";

/// The one biome filament places. Biomes are 4x4x4 and a palette of one entry
/// carries them with no index array at all, which is the same trick a
/// single-value palette plays on blocks.
const BIOME: &str = "minecraft:plains";

/// Heightmaps are one value per 16x16 column: how high the world reaches.
const HEIGHTMAP_COLUMNS: usize = SECTION_DIM_USIZE * SECTION_DIM_USIZE;

/// Width for a height. The tallest column can be `WORLD_HEIGHT` above the
/// bottom, and the bottom is negative, so `WORLD_HEIGHT + 1 - MIN_BLOCK_Y`
/// distinct values -- ten of them needs ten bits.
const HEIGHTMAP_BITS: u8 = 10;

/// Filament names its blocks the way the renderer thinks of them. Two of those
/// are not the names on disk: the renderer wants `grass` for the green-topped
/// block, and the file wants `minecraft:grass_block`.
fn vanilla_name(short: &str) -> String {
    match short {
        "grass" => "minecraft:grass_block".to_string(),
        other => format!("minecraft:{other}"),
    }
}

/// The other half: a name off disk back to the registry key.
///
/// `None` for a block filament has never heard of, which is what a world
/// edited by a different game would contain. Callers decide what to do about
/// it; this only refuses to guess.
fn registry_name(vanilla: &str) -> Option<&str> {
    let bare = vanilla.strip_prefix("minecraft:").unwrap_or(vanilla);
    match bare {
        "grass_block" => Some("grass"),
        other => Some(other),
    }
}

/// Repacks filament's bit layout into the game's.
///
/// This is not a cast. Filament's [`super::bitpack::BitArray`] puts entry `i` at
/// bit `i * bits` of a flat stream, so an entry can straddle two words. The
/// game packs `64 / bits` entries into each long and never lets one cross the
/// boundary (`SimpleBitStorage.valuesPerLong`). Both are correct in their own
/// world; writing one as the other produces a file that decodes to noise
/// without any error anywhere.
fn to_vanilla_words(bits: u8, src: &[u64], len: usize) -> Vec<i64> {
    if bits == 0 {
        return Vec::new();
    }
    let per_long = (64 / bits) as usize;
    // As many longs as the game's layout needs: one per `per_long` entries.
    let mut out = vec![0u64; len.div_ceil(per_long)];
    let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };

    for index in 0..len {
        // Read from the flat stream: entry `index` starts at bit `index * bits`.
        let start = index * bits as usize;
        let word = start / 64;
        let offset = start % 64;
        // Two words only ever needed: bits <= 32 fits in 64 bits, and the
        // palette never widens past that in practice.
        let mut value = src[word] >> offset;
        if offset + bits as usize > 64 {
            value |= src[word + 1] << (64 - offset);
        }
        let value = value & mask;

        let cell = index / per_long;
        let shift = (index % per_long) * bits as usize;
        out[cell] |= value << shift;
    }

    out.into_iter().map(|w| w as i64).collect()
}

/// The inverse of [`to_vanilla_words`].
fn from_vanilla_words(bits: u8, src: &[i64], len: usize) -> Vec<u64> {
    if bits == 0 {
        return Vec::new();
    }
    let per_long = (64 / bits) as usize;
    // Back into the flat stream, whose length is `bits` per entry across the
    // whole thing. This is *not* the same as the vanilla count above: at three
    // bits, 196 longs become 192.
    let mut out = vec![0u64; (len * bits as usize).div_ceil(64)];
    let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };

    for index in 0..len {
        let cell = index / per_long;
        let shift = (index % per_long) * bits as usize;
        let value = ((src[cell] as u64) >> shift) & mask;

        let start = index * bits as usize;
        let word = start / 64;
        let offset = start % 64;
        out[word] |= value << offset;
        if offset + bits as usize > 64 {
            out[word + 1] |= value >> (64 - offset);
        }
    }

    out
}

/// One block as the palette entry the game stores.
///
/// Just the name: `{"Name": "minecraft:dirt"}`. `Properties` is only written
/// when a block has states, and filament's blocks have none, so an empty
/// compound would be a field the game reads as "no properties" anyway.
fn block_state(registry: &BlockRegistry, id: BlockId) -> Compound {
    Compound::new().put("Name", Tag::String(vanilla_name(registry.name(id))))
}

/// Packs flat values into the game's bit layout, for the heightmaps.
fn pack_heightmap(values: &[u32]) -> Vec<i64> {
    let per_long = (64 / HEIGHTMAP_BITS) as usize;
    let mut words = vec![0u64; values.len().div_ceil(per_long)];
    for (index, value) in values.iter().enumerate() {
        let cell = index / per_long;
        let shift = (index % per_long) * HEIGHTMAP_BITS as usize;
        words[cell] |= u64::from(*value) << shift;
    }
    words.into_iter().map(|w| w as i64).collect()
}

/// Height of the highest non-air block in each column, plus one.
///
/// `None` becomes the bottom of the world: an empty chunk has a heightmap full
/// of zero, which is the same answer, and writing it that way keeps the value
/// inside the ten bits the format gives us.
fn heights(chunk: &Chunk, skip: impl Fn(BlockId) -> bool) -> Vec<u32> {
    let mut out = vec![0u32; HEIGHTMAP_COLUMNS];

    for (slot, section) in chunk.sections().iter().enumerate() {
        let base_y = (MIN_SECTION_Y + slot as i32) * 16;
        for y in 0..16usize {
            for z in 0..16usize {
                for x in 0..16usize {
                    let block = section.get(LocalPos::from_array([x as u8, y as u8, z as u8]));
                    if skip(block) {
                        continue;
                    }
                    let column = z * SECTION_DIM_USIZE + x;
                    let height = (base_y + y as i32 + 1 - MIN_BLOCK_Y) as u32;
                    out[column] = out[column].max(height);
                }
            }
        }
    }

    out
}

/// Writes a chunk into the compound a region file stores.
pub fn write(chunk: &Chunk, registry: &BlockRegistry, last_update: u64, inhabited: u64) -> Compound {
    let pos = chunk.pos();

    let mut sections = Vec::with_capacity(SECTIONS_PER_CHUNK);
    for (slot, section) in chunk.sections().iter().enumerate() {
        let blocks = section.blocks();
        let (bits, words) = blocks.bits_and_words();
        let palette = blocks
            .palette()
            .iter()
            .map(|&id| Tag::Compound(block_state(registry, id)))
            .collect();

        // `put` consumes and returns, so the optional field is added with
        // `set` rather than chained on.
        let mut block_states = Compound::new().put("palette", Tag::list(palette));
        if bits != 0 {
            block_states.set("data", Tag::LongArray(to_vanilla_words(bits, &words, SECTION_VOLUME)));
        }

        let biomes = Compound::new().put("palette", Tag::list(vec![Tag::String(BIOME.to_string())]));

        sections.push(Tag::Compound(
            Compound::new()
                .put("block_states", Tag::Compound(block_states))
                .put("biomes", Tag::Compound(biomes))
                // `Y` is the section's own height, not its slot: slot 0 is
                // section -4, so writing the slot would put the world at sea
                // level sixteen times too high.
                .put("Y", Tag::Byte((MIN_SECTION_Y + slot as i32) as i8)),
        ));
    }

    let mut heightmaps = Compound::new();
    let surface = heights(chunk, |b| registry.name(b) == "air");
    let solid = heights(chunk, |b| registry.name(b) == "air" || !registry.is_opaque(b));
    for (key, values) in [
        ("MOTION_BLOCKING", &surface),
        ("MOTION_BLOCKING_NO_LEAVES", &surface),
        ("WORLD_SURFACE", &surface),
        ("WORLD_SURFACE_WG", &surface),
        ("OCEAN_FLOOR", &solid),
        ("OCEAN_FLOOR_WG", &solid),
    ] {
        heightmaps.set(key, Tag::LongArray(pack_heightmap(values)));
    }

    Compound::new()
        .put("DataVersion", Tag::Int(DATA_VERSION))
        .put("xPos", Tag::Int(pos.x))
        .put("yPos", Tag::Int(MIN_SECTION_Y))
        .put("zPos", Tag::Int(pos.z))
        .put("LastUpdate", Tag::Long(last_update as i64))
        .put("InhabitedTime", Tag::Long(inhabited as i64))
        .put("Status", Tag::String(STATUS_FULL.to_string()))
        .put("sections", Tag::list(sections))
        .put("block_entities", Tag::list(Vec::new()))
        .put("block_ticks", Tag::list(Vec::new()))
        .put("fluid_ticks", Tag::list(Vec::new()))
        .put("PostProcessing", Tag::list(Vec::new()))
        .put("Heightmaps", Tag::Compound(heightmaps))
        .put(
            "structures",
            Tag::Compound(
                Compound::new()
                    .put("starts", Tag::Compound(Compound::new()))
                    .put("References", Tag::Compound(Compound::new())),
            ),
        )
}

/// Reads a chunk back out of a stored compound.
///
/// `None` when the compound is not a chunk at all, or names a block this build
/// has never heard of. Half a chunk is worse than none: it would look loaded
/// and be wrong in a place nobody thought to look.
pub fn read(tag: &Compound, registry: &BlockRegistry) -> Option<Chunk> {
    let pos = ChunkPos::new(tag.int("xPos")?, tag.int("zPos")?);
    let air = registry.id("air")?;
    let mut chunk = Chunk::uniform(pos, air);

    for entry in tag.list("sections")? {
        let section_tag = entry.as_compound()?;
        let y = match section_tag.get("Y")? {
            Tag::Byte(y) => *y as i32,
            _ => return None,
        };
        let slot = y - MIN_SECTION_Y;
        if slot < 0 || slot as usize >= SECTIONS_PER_CHUNK {
            continue;
        }

        let states = section_tag.compound("block_states")?;
        let palette = states
            .list("palette")?
            .iter()
            .map(|entry| {
                let name = entry.as_compound()?.get("Name")?.clone();
                let Tag::String(name) = name else { return None };
                let id = registry.id(registry_name(&name)?)?;
                Some(id)
            })
            .collect::<Option<Vec<BlockId>>>()?;
        if palette.is_empty() {
            return None;
        }

        let (bits, words) = match states.get("data") {
            // The array does not record its own width, so the palette is the
            // only place it survives.
            Some(Tag::LongArray(words)) => {
                let bits = bits_for_palette_len(palette.len());
                (bits, from_vanilla_words(bits, words, SECTION_VOLUME))
            }
            _ => (0, Vec::new()),
        };

        chunk.sections_mut()[slot as usize] = Section::from_parts(palette, bits, words);
    }

    Some(chunk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::terrain::{TerrainBlocks, register_terrain};

    fn registry() -> (BlockRegistry, TerrainBlocks) {
        let mut registry = BlockRegistry::new();
        let terrain = register_terrain(&mut registry);
        (registry, terrain)
    }

    fn local(x: u8, y: u8, z: u8) -> LocalPos {
        LocalPos::from_array([x, y, z])
    }

    /// The layout conversion is the part that fails silently, so it is tested
    /// on its own first: every width the palette can pick, where none of them
    /// divides 64 and a spanning encoder would show up immediately.
    #[test]
    fn the_two_bit_layouts_agree_at_every_width_a_palette_can_pick() {
        for bits in [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] {
            let len = 4096usize;
            let max = if bits >= 64 { u32::MAX } else { (1u32 << bits) - 1 };

            // A pattern with no period the word size could hide: it walks the
            // whole range and crosses every boundary on the way.
            let mut spanning = crate::world::bitpack::BitArray::new(len, bits);
            for i in 0..len {
                spanning.set(i, (i as u32 * 7 + 3) & max);
            }

            let vanilla = to_vanilla_words(bits, spanning.words(), len);
            let back = from_vanilla_words(bits, &vanilla, len);
            let restored = crate::world::bitpack::BitArray::from_words(back, bits, len);

            for i in 0..len {
                assert_eq!(
                    restored.get(i),
                    (i as u32 * 7 + 3) & max,
                    "bits={bits} entry {i}"
                );
            }
        }
    }

    /// A hand-checked case, because the round trip above cannot tell two
    /// consistent mistakes apart.
    ///
    /// At five bits the game fits `64 / 5 = 12` entries per long, so entry 12
    /// starts a *new* word at bit 0. Filament's flat stream puts entry 12 at
    /// bit 60, where it straddles the first word. One value, set, and the two
    /// layouts put it in two different places:
    ///
    ///     vanilla:  [0, 1, 0, 0, ...]     entry 12 at word 1, bit 0
    ///     filament: [0x1000_0000_0000_0000, 0, ...]
    ///
    /// `SimpleBitStorage.valuesPerLong` and `BitArray.index` are the two
    /// lines of reference this rests on.
    #[test]
    fn the_conversion_is_checked_against_hand_computed_words() {
        let len = 4096usize;
        let mut array = crate::world::bitpack::BitArray::new(len, 5);
        array.set(12, 1);

        let vanilla = to_vanilla_words(5, array.words(), len);
        assert_eq!(vanilla[0], 0, "first long stays empty");
        assert_eq!(vanilla[1], 1, "entry 12 begins a new long at bit 0");

        // And the flat stream really does put it at bit 60 of the first.
        assert_eq!(array.words()[0], 1u64 << 60);
    }

    #[test]
    fn a_chunk_survives_a_trip_through_nbt() {
        let (registry, terrain) = registry();
        let pos = ChunkPos::new(-3, 7);
        let mut chunk = Chunk::uniform(pos, terrain.air);

        // One section filled with everything the build has, so the palette is
        // wide enough to need a real index array.
        {
            let section = chunk.section_mut(super::super::pos::SectionPos::new(-3, 1, 7)).unwrap();
            let ids = [terrain.air, terrain.dirt, terrain.stone, terrain.grass];
            for i in 0..4096usize {
                let p = LocalPos::from_index(i);
                section.set(p, ids[(i * 31 + p.x as usize) % ids.len()]);
            }
        }

        let tag = write(&chunk, &registry, 12345, 678);
        let bytes = crate::world::nbt::write(&tag);
        let back = crate::world::nbt::read(&bytes).unwrap();
        let restored = read(&back, &registry).expect("round trip");

        assert_eq!(restored.pos(), pos);
        for i in 0..4096usize {
            let p = LocalPos::from_index(i);
            let ids = [terrain.air, terrain.dirt, terrain.stone, terrain.grass];
            assert_eq!(
                restored.section(section_pos()).unwrap().get(p),
                ids[(i * 31 + p.x as usize) % ids.len()],
                "block {i}"
            );
        }
    }

    fn section_pos() -> crate::world::pos::SectionPos {
        crate::world::pos::SectionPos::new(-3, 1, 7)
    }

    #[test]
    fn section_y_is_the_height_and_not_its_slot() {
        let (registry, terrain) = registry();
        let chunk = Chunk::uniform(ChunkPos::new(0, 0), terrain.air);
        let tag = write(&chunk, &registry, 0, 0);

        // Slot 0 is the bottom of the world. Writing the slot instead is the
        // kind of mistake that reads back fine and puts everything sixteen
        // times too high.
        assert_eq!(tag.int("yPos"), Some(MIN_SECTION_Y));
        let first = &tag.list("sections").unwrap()[0];
        assert_eq!(first.as_compound().unwrap().get("Y"), Some(&Tag::Byte(-4)));
    }

    #[test]
    fn blocks_go_to_disk_under_the_names_the_game_uses() {
        let (registry, terrain) = registry();
        let chunk = Chunk::uniform(ChunkPos::new(0, 0), terrain.air);
        let tag = write(&chunk, &registry, 0, 0);

        let section = tag.list("sections").unwrap()[0].as_compound().unwrap();
        let states = section.compound("block_states").unwrap();
        let names: Vec<&str> = states
            .list("palette")
            .unwrap()
            .iter()
            .map(|e| match e.as_compound().unwrap().get("Name").unwrap() {
                Tag::String(s) => s.as_str(),
                other => panic!("palette entry is {other:?}"),
            })
            .collect();
        assert_eq!(names, ["minecraft:air"]);

        // The name on disk is not the name in the renderer. If this ever
        // becomes `minecraft:grass`, a world saved with grass in it is one the
        // game cannot place.
        let mut with_grass = chunk.clone();
        with_grass
            .section_mut(section_pos_zero())
            .unwrap()
            .set(local(0, 0, 0), terrain.grass);
        let tag = write(&with_grass, &registry, 0, 0);
        let section = tag.list("sections").unwrap()[4].as_compound().unwrap();
        let names: Vec<String> = section
            .compound("block_states")
            .unwrap()
            .list("palette")
            .unwrap()
            .iter()
            .map(|e| match e.as_compound().unwrap().get("Name").unwrap() {
                Tag::String(s) => s.clone(),
                other => panic!("palette entry is {other:?}"),
            })
            .collect();
        assert!(names.contains(&"minecraft:grass_block".to_string()), "{names:?}");
    }

    fn section_pos_zero() -> crate::world::pos::SectionPos {
        crate::world::pos::SectionPos::new(0, 0, 0)
    }

    #[test]
    fn a_uniform_section_costs_no_index_array_at_all() {
        let (registry, terrain) = registry();
        let chunk = Chunk::uniform(ChunkPos::new(0, 0), terrain.air);
        let tag = write(&chunk, &registry, 0, 0);
        let section = tag.list("sections").unwrap()[0].as_compound().unwrap();

        assert!(!section.compound("block_states").unwrap().contains("data"));
    }

    #[test]
    fn a_column_keeps_its_height_through_the_heightmap() {
        let (registry, terrain) = registry();
        let mut chunk = Chunk::uniform(ChunkPos::new(0, 0), terrain.air);
        // One block at y=70 in the corner, and 200 is where it has to read
        // back: 64 below the floor of the world plus the block plus one.
        let pos = crate::world::pos::BlockPos::new(0, 70, 0);
        chunk.set_block(pos, terrain.stone);

        let tag = write(&chunk, &registry, 0, 0);
        let back = crate::world::nbt::read(&crate::world::nbt::write(&tag)).unwrap();
        let words = match back
            .compound("Heightmaps")
            .unwrap()
            .get("MOTION_BLOCKING")
            .unwrap()
        {
            Tag::LongArray(w) => w.clone(),
            other => panic!("heightmap is {other:?}"),
        };

        // Column (0, 0) is entry 0: the first long, at bit 0.
        let height = (words[0] as u64) & ((1u64 << HEIGHTMAP_BITS) - 1);
        assert_eq!(height, (70 + 1 - MIN_BLOCK_Y) as u64);
    }

    #[test]
    fn a_block_this_build_has_never_heard_of_stops_the_load() {
        let (registry, terrain) = registry();
        let chunk = Chunk::uniform(ChunkPos::new(0, 0), terrain.air);
        let mut tag = write(&chunk, &registry, 0, 0);

        let mut section = tag.list("sections").unwrap()[0].as_compound().unwrap().clone();
        let states = section.get("block_states").unwrap().as_compound().unwrap().clone();
        let palette: Vec<Tag> = states
            .list("palette")
            .unwrap()
            .iter()
            .map(|_| Tag::Compound(Compound::new().put("Name", Tag::String("minecraft:netherrack".into()))))
            .collect();
        section.set("block_states", Tag::Compound(states.clone().put("palette", Tag::list(palette))));

        let sections: Vec<Tag> = tag
            .list("sections")
            .unwrap()
            .iter()
            .map(|_| Tag::Compound(section.clone()))
            .collect();
        tag.set("sections", Tag::list(sections));

        assert_eq!(read(&tag, &registry), None);
        let _ = terrain;
    }

    #[test]
    fn something_that_is_not_a_chunk_is_refused() {
        let (registry, _) = registry();
        assert_eq!(read(&Compound::new(), &registry), None);
    }
}
