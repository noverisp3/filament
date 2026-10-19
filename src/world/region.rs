//! Region files: `world/region/r.X.Z.mca`, thirty-two by thirty-two chunks each.
//!
//! The layout is a consequence of one decision and nothing else: a region file
//! has to be **written while it is being read**, so the index of where each
//! chunk lives cannot be stored inside the chunks. It goes in a fixed-size
//! header at the front, and everything else in the file is dead space that used
//! to hold chunks that have since moved or grown.
//!
//! ```text
//! byte 0        4096 bytes   offsets   -- 1024 i32, see below
//! byte 4096     4096 bytes   timestamps -- 1024 i32, unix seconds
//! byte 8192     4096 each    sector 2, sector 3, ...
//! ```
//!
//! A chunk at `(x, z)` inside a region lives at index `(x & 31) + (z & 31) * 32`,
//! which is why region coordinates are divided by 32 and chunk coordinates are
//! only ever taken modulo 32.
//!
//! **The offset word packs two numbers.** `RegionFile.getSectorNumber` and
//! `getNumSectors` read it as `offset >> 8` for where and `offset & 0xFF` for
//! how many sectors -- 256 would collide with the sign bit, which is why a
//! chunk of 256 sectors or more goes to its own file instead of here.
//!
//! A zero offset means the chunk is not there. That is the only absence marker,
//! and it is why sector 2 is the first sector any chunk can occupy: sector 0
//! and sector 1 are the header.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use super::nbt::{self, Compound};

/// Bytes per sector. The unit everything in this file is counted in.
pub const SECTOR_BYTES: usize = 4096;

/// `SECTOR_INTS`: how many `i32` the header is, which is also how many chunks a
/// region holds.
pub const SECTOR_INTS: usize = 1024;

/// Chunks along one side of a region.
pub const CHUNKS_ACROSS: i32 = 32;

/// Chunks in one region, all of them.
pub const CHUNKS_PER_REGION: usize = (CHUNKS_ACROSS * CHUNKS_ACROSS) as usize;

/// Bytes before the first sector can hold a chunk.
const HEADER_BYTES: usize = SECTOR_INTS * 4 * 2;

/// A chunk this big or larger gets its own file rather than a region, because
/// 256 sectors does not fit in the low byte of the offset word.
const EXTERNAL_CHUNK_THRESHOLD: i32 = 256;

/// The compression schemes a chunk record may name.
///
/// Written as zlib, which is what the game has written for years. Read as
/// whatever the byte says, so an old world still opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression1 {
    Gzip,
    Zlib,
    None,
}

impl Compression1 {
    fn to_byte(self) -> u8 {
        match self {
            Compression1::Gzip => 1,
            Compression1::Zlib => 2,
            Compression1::None => 3,
        }
    }

    fn from_byte(b: u8) -> Option<Compression1> {
        match b {
            1 => Some(Compression1::Gzip),
            2 => Some(Compression1::Zlib),
            3 => Some(Compression1::None),
            _ => None,
        }
    }

    fn compress(self, data: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Compression1::None => Ok(data.to_vec()),
            Compression1::Zlib => {
                let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
                io::Write::write_all(&mut e, data)?;
                e.finish()
            }
            Compression1::Gzip => {
                let mut e =
                    flate2::write::GzEncoder::new(Vec::new(), Compression::default());
                io::Write::write_all(&mut e, data)?;
                e.finish()
            }
        }
    }

    fn decompress(self, data: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Compression1::None => Ok(data.to_vec()),
            Compression1::Zlib => {
                let mut out = Vec::new();
                io::Read::read_to_end(&mut ZlibDecoder::new(data), &mut out)?;
                Ok(out)
            }
            Compression1::Gzip => {
                let mut out = Vec::new();
                io::Read::read_to_end(
                    &mut flate2::read::GzDecoder::new(data),
                    &mut out,
                )?;
                Ok(out)
            }
        }
    }
}

/// Where a chunk sits in the world, in region and chunk coordinates.
///
/// Signed, because a world has chunks at negative coordinates and a `u32` here
/// would make the west half of the world unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkKey {
    pub region_x: i32,
    pub region_z: i32,
}

impl ChunkKey {
    /// The region containing `(x, z)`, with rounding toward negative infinity.
    ///
    /// `div_euclid` rather than `/`, because `-1 / 32` is zero and the chunk at
    /// `-1` belongs to region `-1`. Getting this wrong puts every chunk west of
    /// the origin in the wrong file, where nothing will find them.
    pub fn of(x: i32, z: i32) -> ChunkKey {
        ChunkKey {
            region_x: x.div_euclid(CHUNKS_ACROSS),
            region_z: z.div_euclid(CHUNKS_ACROSS),
        }
    }

    pub fn name(&self) -> String {
        format!("r.{}.{}.mca", self.region_x, self.region_z)
    }

    /// The chunk's position inside its own region, which is what the header
    /// is indexed by: 0..32, never negative.
    pub fn local_of(x: i32, z: i32) -> (i32, i32) {
        (
            x.rem_euclid(CHUNKS_ACROSS),
            z.rem_euclid(CHUNKS_ACROSS),
        )
    }

    /// Where this region lives inside a world folder.
    pub fn path_in(self, root: &Path) -> PathBuf {
        root.join("region").join(self.name())
    }
}

/// The header's index for a chunk, which is also the slot in the offset table.
fn slot(x: i32, z: i32) -> usize {
    (x.rem_euclid(CHUNKS_ACROSS) + z.rem_euclid(CHUNKS_ACROSS) * CHUNKS_ACROSS) as usize
}

/// One sector-aligned record: how many sectors, starting where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    sector: usize,
    sectors: usize,
}

impl Slot {
    /// Packed the way `RegionFile` packs it: count in the low eight bits, sector
    /// number above it.
    fn pack(&self) -> i32 {
        ((self.sector as i32) << 8) | (self.sectors as i32 & 0xFF)
    }

    fn unpack(word: i32) -> Option<Slot> {
        if word == 0 {
            return None;
        }
        let sectors = (word & 0xFF) as usize;
        let sector = ((word >> 8) & 0x00FF_FFFF) as usize;
        if sector == 0 {
            // Only sector 0 is the header, and a chunk never starts there, so a
            // word claiming to is a corrupt index rather than a chunk.
            return None;
        }
        Some(Slot { sector, sectors })
    }
}

/// How many sectors `size` bytes take, rounding up.
fn sectors_for(size: usize) -> usize {
    size.div_ceil(SECTOR_BYTES)
}

/// A region file, held whole in memory.
///
/// Every chunk is at most a few hundred kilobytes and a region is at most a few
/// hundred of them, so a region is small enough that reading one is cheaper than
/// seeking around in it. Writing therefore becomes "lay the records out again and
/// write the whole file", which is far simpler than allocating and freeing
/// sectors as chunks grow, and a server that saves on quit and on chunk unload
/// does not care.
#[derive(Debug, Clone, Default)]
pub struct RegionFile {
    chunks: BTreeMap<usize, Compound>,
    /// Unix seconds, kept so a chunk that has not moved still looks written.
    timestamps: BTreeMap<usize, i32>,
}

impl RegionFile {
    pub fn empty() -> RegionFile {
        RegionFile::default()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn has(&self, x: i32, z: i32) -> bool {
        self.chunks.contains_key(&slot(x, z))
    }

    pub fn get(&self, x: i32, z: i32) -> Option<&Compound> {
        self.chunks.get(&slot(x, z))
    }

    pub fn put(&mut self, x: i32, z: i32, chunk: Compound) {
        let i = slot(x, z);
        self.chunks.insert(i, chunk);
        self.timestamps.insert(i, (now_unix() / 1000) as i32);
    }

    /// Reads a region file.
    ///
    /// A file that does not exist reads as an empty region rather than an
    /// error: the first run of a new world has no region files and that is not
    /// a failure.
    pub fn read(path: &Path) -> io::Result<RegionFile> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(RegionFile::empty()),
            Err(e) => return Err(e),
        };
        RegionFile::from_bytes(&bytes)
    }

    /// Parses a region file's bytes.
    ///
    /// A file shorter than its own header is refused rather than padded. A short
    /// file means a write was cut off, and carrying on would mean chunks read as
    /// empty -- a world that quietly loses everything it had.
    pub fn from_bytes(bytes: &[u8]) -> io::Result<RegionFile> {
        if bytes.is_empty() {
            return Ok(RegionFile::empty());
        }
        if bytes.len() < HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "a region file is {HEADER_BYTES} bytes of header before anything else, this is {}",
                    bytes.len()
                ),
            ));
        }

        let mut region = RegionFile::empty();

        for z in 0..CHUNKS_ACROSS {
            for x in 0..CHUNKS_ACROSS {
                let i = slot(x, z);
                let word = i32::from_be_bytes([
                    bytes[i * 4],
                    bytes[i * 4 + 1],
                    bytes[i * 4 + 2],
                    bytes[i * 4 + 3],
                ]);
                let Some(where_) = Slot::unpack(word) else {
                    continue;
                };
                let ts_at = SECTOR_INTS * 4 + i * 4;
                let timestamp = i32::from_be_bytes([
                    bytes[ts_at],
                    bytes[ts_at + 1],
                    bytes[ts_at + 2],
                    bytes[ts_at + 3],
                ]);

                // Sectors are counted from the start of the file, so sector 2 is
                // byte 8192 and sector 0 is the header's first half. Adding the
                // header again here is how every chunk reads as past the end.
                let start = where_.sector * SECTOR_BYTES;
                let end = start + where_.sectors * SECTOR_BYTES;
                if end > bytes.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "chunk at {x},{z} claims sectors {}..{} of a {} byte file",
                            where_.sector,
                            where_.sector + where_.sectors,
                            bytes.len()
                        ),
                    ));
                }

                let sector = &bytes[start..end];
                if sector.len() < 5 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("chunk at {x},{z} has no header"),
                    ));
                }
                let len = u32::from_be_bytes([sector[0], sector[1], sector[2], sector[3]])
                    as usize;
                let compression = Compression1::from_byte(sector[4]).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("chunk at {x},{z} names compression {}", sector[4]),
                    )
                })?;

                if len > sector.len() - 5 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "chunk at {x},{z} says {len} bytes in a {} byte sector",
                            sector.len() - 5
                        ),
                    ));
                }

                let raw = compression.decompress(&sector[5..5 + len])?;
                match nbt::read(&raw) {
                    Ok(c) => {
                        region.chunks.insert(i, c);
                        region.timestamps.insert(i, timestamp);
                    }
                    // One unreadable chunk does not condemn the other thousand.
                    Err(e) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("chunk at {x},{z}: {e}"),
                        ));
                    }
                }
            }
        }

        Ok(region)
    }

    /// Lays the file out and writes it.
    ///
    /// Always writes the whole thing, header and all, because the header is
    /// derived from where the records ended up and so cannot be written first.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let bytes = self.to_bytes()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write beside the target and rename over it. A crash midway leaves the
        // old file intact rather than a half-written one, which is the
        // difference between a world that lost nothing and a world that lost a
        // region.
        let tmp = path.with_extension("mca.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Builds the file's bytes.
    pub fn to_bytes(&self) -> io::Result<Vec<u8>> {
        let mut out = vec![0u8; HEADER_BYTES];
        // Sectors start after the header. Sector 0 and sector 1 are the header
        // itself, so the first sector a chunk can occupy is 2.
        let mut next_sector = 2usize;

        // Collect first, then lay out in slot order. Assigning sectors while
        // walking the map gives the records numbers in key order while writing
        // them in slot order, and the two disagree as soon as any chunk has a
        // negative coordinate -- the header then points at the wrong bytes and
        // a chunk reads back as a different one.
        let mut pending: Vec<(usize, Vec<u8>)> = Vec::new();
        for (i, chunk) in &self.chunks {
            let raw = nbt::write(chunk);
            let payload = Compression1::Zlib.compress(&raw)?;
            pending.push((*i, payload));
        }
        pending.sort_by_key(|(i, _)| *i);

        // Sector numbers are only meaningful once the order is fixed, so they
        // are handed out here rather than above.
        let mut ordered: Vec<(usize, Slot, Vec<u8>)> = Vec::new();
        for (i, payload) in pending {
            let needed = sectors_for(5 + payload.len());
            if needed >= EXTERNAL_CHUNK_THRESHOLD as usize {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "a chunk needs {needed} sectors, and {} is where the offset \
                         word runs out",
                        EXTERNAL_CHUNK_THRESHOLD
                    ),
                ));
            }
            let s = Slot {
                sector: next_sector,
                sectors: needed,
            };
            next_sector += needed;
            ordered.push((i, s, payload));
        }

        for (i, s, _) in &ordered {
            let at = i * 4;
            out[at..at + 4].copy_from_slice(&s.pack().to_be_bytes());

            let ts = self.timestamps.get(i).copied().unwrap_or(0);
            let ts_at = SECTOR_INTS * 4 + i * 4;
            out[ts_at..ts_at + 4].copy_from_slice(&ts.to_be_bytes());
        }

        for (_, _, payload) in ordered {
            let start = out.len();
            let needed = sectors_for(5 + payload.len());
            out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            out.push(Compression1::Zlib.to_byte());
            out.extend_from_slice(&payload);
            out.resize(start + needed * SECTOR_BYTES, 0);
        }

        Ok(out)
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::nbt::Tag;

    fn named(name: &str) -> Compound {
        Compound::new().put("name", Tag::String(name.into()))
    }

    #[test]
    fn a_region_file_is_eight_kilobytes_of_header_before_anything_else() {
        let empty = RegionFile::empty().to_bytes().unwrap();
        assert_eq!(
            empty.len(),
            HEADER_BYTES,
            "even with no chunks, the header is written in full"
        );
        assert!(empty.iter().all(|b| *b == 0), "and it is all zero");
    }

    #[test]
    fn a_chunk_lands_in_the_slot_its_coordinates_name() {
        // `(x & 31) + (z & 31) * 32`, and getting this wrong moves every chunk
        // in the region without anything looking wrong.
        assert_eq!(slot(0, 0), 0);
        assert_eq!(slot(1, 0), 1);
        assert_eq!(slot(0, 1), 32);
        assert_eq!(slot(31, 31), 1023);
        // Negative world coordinates wrap into the region rather than off it.
        assert_eq!(slot(-1, 0), 31);
        assert_eq!(slot(0, -1), 992);
    }

    #[test]
    fn the_offset_word_packs_where_and_how_many() {
        // `offset >> 8` is where, `offset & 0xFF` is how many. Both halves are
        // read back by a real server, so both halves have to survive a round
        // trip through a file.
        let s = Slot {
            sector: 2,
            sectors: 3,
        };
        let word = s.pack();
        assert_eq!(word >> 8, 2, "where, above the low byte");
        assert_eq!(word & 0xFF, 3, "how many, in the low byte");
        assert_eq!(Slot::unpack(word), Some(s));

        assert_eq!(Slot::unpack(0), None, "zero means not present");
        // Sector 0 is the header and no chunk is ever there, so a word claiming
        // it is a corrupt index and is refused rather than read as the header.
        assert_eq!(Slot::unpack(1), None);
    }

    #[test]
    fn negative_coordinates_round_to_the_right_region() {
        // `/` truncates toward zero, which would put the chunk at -1 in the
        // region at 0, next to thirty-one other chunks, where nothing finds it.
        assert_eq!(ChunkKey::of(0, 0), ChunkKey { region_x: 0, region_z: 0 });
        assert_eq!(ChunkKey::of(31, 31), ChunkKey { region_x: 0, region_z: 0 });
        assert_eq!(ChunkKey::of(32, 0), ChunkKey { region_x: 1, region_z: 0 });
        assert_eq!(ChunkKey::of(-1, -1), ChunkKey { region_x: -1, region_z: -1 });
        assert_eq!(ChunkKey::of(-32, -32), ChunkKey { region_x: -1, region_z: -1 });
        assert_eq!(ChunkKey::of(-33, 0), ChunkKey { region_x: -2, region_z: 0 });
        assert_eq!(ChunkKey { region_x: -1, region_z: 2 }.name(), "r.-1.2.mca");
    }

    #[test]
    fn chunks_survive_a_region_file_round_trip() {
        let mut r = RegionFile::empty();
        r.put(0, 0, named("spawn"));
        r.put(31, 31, named("far corner"));
        r.put(-1, 5, named("west"));

        let bytes = r.to_bytes().unwrap();
        let back = RegionFile::from_bytes(&bytes).unwrap();

        assert_eq!(back.len(), 3);
        assert_eq!(back.get(0, 0).unwrap().string("name"), Some("spawn"));
        assert_eq!(
            back.get(31, 31).unwrap().string("name"),
            Some("far corner")
        );
        assert_eq!(back.get(-1, 5).unwrap().string("name"), Some("west"));
    }

    #[test]
    fn every_record_starts_on_a_sector_and_the_file_is_a_whole_number_of_sectors() {
        let mut r = RegionFile::empty();
        for i in 0..40i32 {
            // Thirty-two across, so a region cannot hold more than 32x32 and the
            // fortieth chunk goes on the second row rather than wrapping onto
            // the first and overwriting what is already there.
            let (x, z) = (i % CHUNKS_ACROSS, i / CHUNKS_ACROSS);
            // A long name, so the compressed length is not a round number and
            // the padding has something to pad.
            r.put(x, z, named(&"n".repeat(i as usize + 1)));
        }
        let bytes = r.to_bytes().unwrap();
        assert_eq!(
            bytes.len() % SECTOR_BYTES,
            0,
            "a region file is a whole number of sectors: {}",
            bytes.len()
        );
        assert_eq!(RegionFile::from_bytes(&bytes).unwrap().len(), 40);
    }

    #[test]
    fn the_first_chunk_starts_after_the_header() {
        let mut r = RegionFile::empty();
        r.put(0, 0, named("first"));
        let bytes = r.to_bytes().unwrap();
        let word = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let s = Slot::unpack(word).expect("a chunk is there");
        assert_eq!(s.sector, 2, "sector 0 and 1 are the header");
        assert!(s.sectors >= 1);
    }

    #[test]
    fn an_empty_file_reads_as_an_empty_region() {
        // The first run of a new world has no region files, and that is not a
        // failure to report.
        assert!(RegionFile::from_bytes(&[]).unwrap().is_empty());
        assert_eq!(bytes_of_empty_read_back(), 0);
    }

    fn bytes_of_empty_read_back() -> usize {
        let r = RegionFile::empty();
        RegionFile::from_bytes(&r.to_bytes().unwrap()).unwrap().len()
    }

    #[test]
    fn a_file_too_short_to_have_a_header_is_refused() {
        // A short file means a write was cut off. Padding it would mean reading
        // its chunks as empty, which is a world that quietly loses everything.
        let err = RegionFile::from_bytes(&[0u8; 100]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_file_that_ends_mid_chunk_is_refused_rather_than_read_as_empty() {
        let mut r = RegionFile::empty();
        r.put(0, 0, named("here"));
        let bytes = r.to_bytes().unwrap();
        let truncated = &bytes[..bytes.len() - SECTOR_BYTES];
        // Dropping a whole sector leaves the header claiming a chunk that is no
        // longer there. That has to be an error, not an empty chunk.
        assert!(
            RegionFile::from_bytes(truncated).is_err(),
            "a header that points past the end of the file"
        );
    }

    #[test]
    fn writing_and_reading_a_file_works_on_disk() {
        let dir = std::env::temp_dir().join(format!("filament-region-{}", now_unix()));
        let path = ChunkKey::of(-5, 9).path_in(&dir);
        assert_eq!(path.file_name().unwrap(), "r.-1.0.mca");

        let mut r = RegionFile::empty();
        r.put(3, 9, named("on disk"));
        r.write(&path).unwrap();

        let back = RegionFile::read(&path).unwrap();
        assert_eq!(back.get(3, 9).unwrap().string("name"), Some("on disk"));

        // A file that is not there reads as empty rather than failing.
        let missing = dir.join("region").join("r.9.9.mca");
        assert!(RegionFile::read(&missing).unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rewriting_replaces_rather_than_appends() {
        let dir = std::env::temp_dir().join(format!("filament-region2-{}", now_unix()));
        let path = ChunkKey::of(0, 0).path_in(&dir);

        let mut first = RegionFile::empty();
        for x in 0..10 {
            first.put(x, 0, named(&"x".repeat(500)));
        }
        first.write(&path).unwrap();
        let big = std::fs::metadata(&path).unwrap().len();

        // Save the same chunks again, with less in them. The file must shrink,
        // or the dead space of every past save accumulates forever.
        let mut second = RegionFile::empty();
        second.put(0, 0, named("small"));
        second.write(&path).unwrap();
        let small = std::fs::metadata(&path).unwrap().len();

        assert!(small < big, "{small} should be smaller than {big}");
        assert_eq!(
            RegionFile::read(&path).unwrap().len(),
            1,
            "and only what was saved is in it"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_oversized_chunk_is_refused_rather_than_written_wrong() {
        // 256 sectors does not fit in the low byte of the offset word. The game
        // writes those to their own file; filament says no, which is honest and
        // is caught at save time instead of at load time.
        //
        // The bytes have to be ones that do not compress, or the chunk is small
        // again by the time it is measured. A million of the same letter is a
        // few hundred bytes on the wire and never gets near the limit.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut noise = Vec::with_capacity(EXTERNAL_CHUNK_THRESHOLD as usize * SECTOR_BYTES * 4);
        for _ in 0..noise.capacity() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            noise.push(seed as u8);
        }
        let mut r = RegionFile::empty();
        r.put(0, 0, Compound::new().put("noise", Tag::ByteArray(
            noise.into_iter().map(|b| b as i8).collect(),
        )));

        // Asserted without printing the value: a successful save here is a
        // hundred thousand lines of bytes in the failure message.
        assert!(
            r.to_bytes().is_err(),
            "a chunk this size cannot be written into a region"
        );
    }
}
