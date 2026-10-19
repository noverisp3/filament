//! The world folder: `level.dat` in, region files out.
//!
//! Two things here are not where anyone would put them, and both were found by
//! looking at a world the game had actually saved rather than at the
//! decompiled reference:
//!
//! * Terrain is not in `world/region/`. It is in
//!   `world/dimensions/minecraft/overworld/region/`. The reference's region
//!   code never says where it lives, because in the game the caller decides,
//!   and the caller has moved on since.
//! * `level.dat` has no seed in it. Seed and generator moved into the
//!   dimension's own file.
//!
//! Regions are held in memory for as long as the store lives. One of them is
//! 12 KiB of header and megabytes of chunks; reading it once per block access
//! would be absurd, and writing it out on every edit would turn placing a
//! block into an fsync.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use super::block::BlockRegistry;
use super::chunk::Chunk;
use super::chunk_nbt;
use super::nbt::{Compound, Tag};
use super::pos::ChunkPos;
use super::region::{ChunkKey, RegionFile};

/// `world/level.dat`, and the fields of it this build cares about.
const LEVEL_NAME: &str = "level.dat";

/// Overworld, relative to the world folder.
const OVERWORLD: &str = "dimensions/minecraft/overworld";

/// How a block that filament has never heard of should be drawn.
///
/// This is not format -- it is entirely presentational, and it only ever runs
/// on a block read out of a file another program wrote. Getting a colour
/// wrong shows a grey lump where a tree should be; it cannot change what is
/// saved, because the palette name is what goes back on disk.
fn appearance_for(name: &str) -> (bool, [f32; 3]) {
    match name {
        "stone" | "andesite" | "granite" | "diorite" | "cobblestone" | "calcite" => {
            (true, [0.50, 0.50, 0.52])
        }
        "deepslate" => (true, [0.30, 0.30, 0.33]),
        "tuff" => (true, [0.42, 0.40, 0.36]),
        "dirt" | "mud" => (true, [0.45, 0.32, 0.20]),
        "gravel" => (true, [0.45, 0.43, 0.42]),
        "sand" | "sandstone" => (true, [0.85, 0.80, 0.60]),
        "grass_block" => (true, [0.36, 0.62, 0.28]),
        "water" => (false, [0.20, 0.35, 0.75]),
        "lava" => (false, [0.90, 0.35, 0.05]),
        "bedrock" => (true, [0.15, 0.15, 0.15]),
        "snow" | "snow_block" => (true, [0.95, 0.95, 0.97]),
        "ice" => (false, [0.65, 0.75, 0.90]),
        "oak_log" => (true, [0.42, 0.32, 0.18]),
        "oak_leaves" => (false, [0.25, 0.45, 0.15]),
        "clay" => (true, [0.62, 0.66, 0.72]),
        "moss_block" => (true, [0.32, 0.42, 0.22]),
        // Ore sits in stone visually; the shape is all anyone sees at this
        // stage and the colour matters less than not being neon.
        other if other.ends_with("_ore") => (true, [0.42, 0.40, 0.38]),
        _ => (true, [0.55, 0.52, 0.48]),
    }
}

/// A world folder on disk.
pub struct WorldStore {
    root: PathBuf,
    /// The name a real `level.dat` carries, which is the folder's own name.
    name: String,
    /// Open regions, by the coordinates of the region file.
    regions: HashMap<(i32, i32), Option<RegionFile>>,
}

impl WorldStore {
    /// Opens the folder. A folder that is not a world yet is not an error: it
    /// is what a fresh server has, and `level.dat` is written on the way out.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "filament".to_string());
        Ok(Self {
            root,
            name,
            regions: HashMap::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The world's name, as a real `level.dat` carries it: the folder's own.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn region_path(&self, pos: ChunkPos) -> PathBuf {
        ChunkKey::of(pos.x, pos.z).path_in(&self.overworld())
    }

    fn overworld(&self) -> PathBuf {
        self.root.join(OVERWORLD)
    }

    fn level_path(&self) -> PathBuf {
        self.root.join(LEVEL_NAME)
    }

    /// The region containing `pos`, opened once and kept.
    ///
    /// `None` means the file is not there yet, which is cached too: without
    /// that, every block in a chunk the server is generating would ask the
    /// filesystem the same question again.
    fn region_ref(&mut self, pos: ChunkPos) -> Option<&RegionFile> {
        let key = (pos.x.div_euclid(32), pos.z.div_euclid(32));
        let path = ChunkKey::of(pos.x, pos.z).path_in(&self.overworld());
        self.regions
            .entry(key)
            .or_insert_with(|| {
                if !path.exists() {
                    return None;
                }
                match RegionFile::read(&path) {
                    Ok(region) => Some(region),
                    Err(e) => {
                        eprintln!("cannot read {}: {e}", path.display());
                        None
                    }
                }
            })
            .as_ref()
    }

    /// Reads a chunk, if the world has one.
    ///
    /// `registry` is grown as blocks turn up. A world written by the game has
    /// hundreds of block types in it and filament starts with five, so this is
    /// the normal path, not an edge case. Unknown blocks are registered on
    /// sight rather than dropped: dropping them turns a mountain into a hole,
    /// and the name is still what gets written back.
    pub fn load(
        &mut self,
        pos: ChunkPos,
        registry: &mut BlockRegistry,
    ) -> Option<Chunk> {
        let (local_x, local_z) = ChunkKey::local_of(pos.x, pos.z);
        let region = self.region_ref(pos)?;
        let compound = region.get(local_x, local_z)?;

        let chunk = chunk_nbt::read_lenient(compound, registry, &appearance_for)?;

        debug_assert_eq!(chunk.pos(), pos);
        Some(chunk)
    }

    /// Whether the world already has this chunk, without decoding it.
    pub fn has(&mut self, pos: ChunkPos) -> bool {
        let (local_x, local_z) = ChunkKey::local_of(pos.x, pos.z);
        self.region_ref(pos)
            .is_some_and(|region| region.has(local_x, local_z))
    }

    /// Puts a chunk back into its region and writes the region out.
    pub fn save(
        &mut self,
        pos: ChunkPos,
        chunk: &Chunk,
        registry: &BlockRegistry,
        game_time: u64,
    ) -> io::Result<()> {
        let (local_x, local_z) = ChunkKey::local_of(pos.x, pos.z);
        let compound = chunk_nbt::write(chunk, registry, game_time, 0);

        let path = self.region_path(pos);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
    // A read that missed has cached `None` for this region, but a save may be
    // the first thing ever to touch the file: here a cached miss is retried
    // and a fresh region file is born, rather than being treated as an open
    // that already failed.
    let slot = self
        .regions
        .entry((pos.x.div_euclid(32), pos.z.div_euclid(32)))
        .or_insert(None);
    if slot.is_none() {
        *slot = Some(RegionFile::empty());
    }
    let region = slot.as_mut().expect("the slot was filled in just above");
    region.put(local_x, local_z, compound);

        let region = self
            .regions
            .get(&(pos.x.div_euclid(32), pos.z.div_euclid(32)))
            .and_then(|r| r.as_ref())
            .ok_or_else(|| {
                io::Error::other("the region file could not be written")
            })?;
        region.write(&path)
    }

    /// Reads `level.dat` as stored, if there is one.
    pub fn read_level(&self) -> Option<Compound> {
        let bytes = std::fs::read(self.level_path()).ok()?;
        let bytes = decompress(&bytes).ok()?;
        super::nbt::read(&bytes).ok()
    }

    /// Writes `level.dat`.
    ///
    /// The game wraps it in gzip and names the root `Data`, and puts a lone
    /// End tag after it. All three matter: a reader that expects an unnamed
    /// root finds nothing, and an unwritten End tag means the next tag read
    /// runs into whatever follows.
    pub fn write_level(&self, level: &Compound) -> io::Result<()> {
        let bytes = super::nbt::write_named(level, "Data");
        let mut out = Vec::new();
        {
            use std::io::Write;
            flate2::write::GzEncoder::new(&mut out, flate2::Compression::default())
                .write_all(&bytes)
                .map_err(io::Error::other)?;
        }
        std::fs::create_dir_all(&self.root)?;
        atomic_write(&self.level_path(), &out)
    }
}

/// Writes to a temporary file and moves it into place.
///
/// The level file is written while the server is running, so a crash halfway
/// through must not leave a world that cannot be opened.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension("dat.tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

/// Gunzips if it is gzipped, and returns the bytes if it is not.
fn decompress(bytes: &[u8]) -> io::Result<Vec<u8>> {
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Ok(bytes.to_vec());
    }
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(bytes), &mut out)?;
    Ok(out)
}

/// A `level.dat` for a new world, matching what a saved one looks like.
///
/// Read off a world the game had written rather than composed from the
/// reference, because the reference and the file disagree about the block
/// palette and a disagreement like that is better settled by a file.
pub fn default_level(world_name: &str, game_time: u64) -> Compound {
    Compound::new()
        .put("DataVersion", Tag::Int(chunk_nbt::DATA_VERSION))
        .put("GameType", Tag::Int(0))
        .put("LastPlayed", Tag::Long(0))
        .put("LevelName", Tag::String(world_name.to_string()))
        .put("Time", Tag::Long(game_time as i64))
        .put("version", Tag::Int(19133))
        .put("allowCommands", Tag::Byte(0))
        .put("initialized", Tag::Byte(1))
        .put("WasModded", Tag::Byte(0))
        .put("hardcore", Tag::Byte(0))
        .put("difficulty_settings", {
            Tag::Compound(
                Compound::new()
                    .put("difficulty", Tag::String("normal".into()))
                    .put("hardcore", Tag::Byte(0))
                    .put("locked", Tag::Byte(0)),
            )
        })
        .put(
            "Version",
            Tag::Compound(
                Compound::new()
                    .put("Id", Tag::Int(chunk_nbt::DATA_VERSION))
                    .put("Name", Tag::String("26.3".into()))
                    .put("Series", Tag::String("main".into()))
                    .put("Snapshot", Tag::Byte(0)),
            ),
        )
        .put("spawn", {
            Tag::Compound(
                Compound::new()
                    .put("dimension", Tag::String("minecraft:overworld".into()))
                    .put("pos", Tag::IntArray(vec![0, 64, 0]))
                    .put("angle", Tag::Float(0.0)),
            )
        })
}
