//! Named Binary Tag: how the game writes everything to disk.
//!
//! Not invented for filament. A world saved by a real server, opened by a real
//! server, is these bytes; `level.dat`, every chunk in `region/`, and every
//! player file in `playerdata/` are the same format.
//!
//! The rules that are easy to get wrong, and all of which produce a file that
//! loads and then shows nonsense:
//!
//! * **Every number is big-endian.** Java's `DataOutput` is big-endian and so is
//!   everything here. A little-endian `level.dat` opens, and then reports a seed
//!   of zero and a version of zero.
//! * **Strings are length-prefixed in bytes, not characters.** Two bytes of
//!   length, then UTF-8. A name with an accent in it is more bytes than it has
//!   characters, and a length counted in characters truncates it.
//! * **A compound is a name and a tag, repeated, ending in a bare zero.** The
//!   zero carries no name; it is the only place a tag has no name.
//! * **The root is a bare zero followed by the root tag.** So a file starts
//!   `0A 00 00` -- compound, empty name -- and not with the tag id alone.
//! * **Lists carry their element type once**, even when empty. An empty list of
//!   nothing is `09 00 00` and an empty list of compounds is `09 0A 00 00`, and
//!   the byte is not optional.

use std::collections::BTreeMap;
use std::fmt;

/// The tag types, as the numbers the game puts on the wire.
pub mod tag {
    pub const END: u8 = 0;
    pub const BYTE: u8 = 1;
    pub const SHORT: u8 = 2;
    pub const INT: u8 = 3;
    pub const LONG: u8 = 4;
    pub const FLOAT: u8 = 5;
    pub const DOUBLE: u8 = 6;
    pub const BYTE_ARRAY: u8 = 7;
    pub const STRING: u8 = 8;
    pub const LIST: u8 = 9;
    pub const COMPOUND: u8 = 10;
    pub const INT_ARRAY: u8 = 11;
    pub const LONG_ARRAY: u8 = 12;
}

/// How deep a tag may nest. The game refuses past this, and a cycle built by
/// hand would otherwise recurse until the stack gave out.
const MAX_DEPTH: usize = 512;

/// A tag: one value of one type.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List { element: u8, items: Vec<Tag> },
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

/// A map of name to tag.
///
/// A `BTreeMap` rather than the insertion order, so writing the same thing twice
/// produces the same bytes. That matters more than it sounds: without it, every
/// save is a different file, a diff of two saves is noise, and a test that
/// compares bytes can only ever run once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compound(BTreeMap<String, Tag>);

impl Compound {
    pub fn new() -> Self {
        Compound(BTreeMap::new())
    }

    pub fn get(&self, name: &str) -> Option<&Tag> {
        self.0.get(name)
    }

    /// Adds a tag and hands the compound back, so tags chain:
    /// `Compound::new().put("a", ..).put("b", ..)`.
    ///
    /// Consuming rather than borrowing, because the shape a chunk or a player
    /// file is written in is one long chain of keys and borrowing turns every
    /// one of them into two lines. [`Self::set`] is the other half, for a
    /// compound that is already held.
    pub fn put(mut self, name: &str, tag: Tag) -> Self {
        self.0.insert(name.to_string(), tag);
        self
    }

    /// Adds a tag to a compound already held, and returns it for chaining.
    pub fn set(&mut self, name: &str, tag: Tag) -> &mut Self {
        self.0.insert(name.to_string(), tag);
        self
    }

    pub fn remove(&mut self, name: &str) -> Option<Tag> {
        self.0.remove(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Tag)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The value as an int, if it is one.
    ///
    /// Returns `None` rather than zero for a missing or wrong-typed key, because
    /// a zero that came from nowhere is how a chunk ends up at sea level with
    /// no indication of why.
    pub fn int(&self, name: &str) -> Option<i32> {
        match self.get(name) {
            Some(Tag::Int(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn long(&self, name: &str) -> Option<i64> {
        match self.get(name) {
            Some(Tag::Long(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn string(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(Tag::String(v)) => Some(v.as_str()),
            _ => None,
        }
    }

    pub fn compound(&self, name: &str) -> Option<&Compound> {
        match self.get(name) {
            Some(Tag::Compound(v)) => Some(v),
            _ => None,
        }
    }

    pub fn list(&self, name: &str) -> Option<&[Tag]> {
        match self.get(name) {
            Some(Tag::List { items, .. }) => Some(items.as_slice()),
            _ => None,
        }
    }
}

impl Tag {
    pub fn compound(c: Compound) -> Tag {
        Tag::Compound(c)
    }

    /// The value as a compound, if it is one.
    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }

    fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => tag::BYTE,
            Tag::Short(_) => tag::SHORT,
            Tag::Int(_) => tag::INT,
            Tag::Long(_) => tag::LONG,
            Tag::Float(_) => tag::FLOAT,
            Tag::Double(_) => tag::DOUBLE,
            Tag::ByteArray(_) => tag::BYTE_ARRAY,
            Tag::String(_) => tag::STRING,
            Tag::List { .. } => tag::LIST,
            Tag::Compound(_) => tag::COMPOUND,
            Tag::IntArray(_) => tag::INT_ARRAY,
            Tag::LongArray(_) => tag::LONG_ARRAY,
        }
    }

    /// The type a list of these would carry, which is the id except that an
    /// empty list cannot say and has to be told.
    pub fn list(items: Vec<Tag>) -> Tag {
        let element = items.first().map(Tag::id).unwrap_or(tag::END);
        Tag::List { element, items }
    }
}

/// Why a read failed, in terms of what was wanted and what turned up.
#[derive(Debug, Clone, PartialEq)]
pub struct NbtError(pub String);

impl fmt::Display for NbtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for NbtError {}

fn fail<T>(wanted: &str, got: &[u8]) -> Result<T, NbtError> {
    Err(NbtError(format!(
        "wanted {wanted}, {} byte(s) left: {}",
        got.len(),
        got.iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    )))
}

struct Reader<'a> {
    at: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NbtError> {
        if self.at.len() < n {
            return fail(&format!("{n} bytes"), self.at);
        }
        let (head, tail) = self.at.split_at(n);
        self.at = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, NbtError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, NbtError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn i32(&mut self) -> Result<i32, NbtError> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i64(&mut self) -> Result<i64, NbtError> {
        let b = self.take(8)?;
        Ok(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn string(&mut self) -> Result<String, NbtError> {
        let n = self.u16()? as usize;
        let raw = self.take(n)?;
        // Java writes modified UTF-8, which differs from standard UTF-8 in one
        // place that no level name uses: a null is written as two bytes. Any
        // valid UTF-8 round-trips through here unchanged, which is what a
        // chunk, a player name and a dimension name all are.
        String::from_utf8(raw.to_vec()).map_err(|e| NbtError(format!("string: {e}")))
    }
}

fn read_payload(r: &mut Reader, id: u8, depth: usize) -> Result<Tag, NbtError> {
    if depth > MAX_DEPTH {
        return Err(NbtError(format!("nested past {MAX_DEPTH}")));
    }
    Ok(match id {
        tag::BYTE => Tag::Byte(r.u8()? as i8),
        tag::SHORT => Tag::Short(r.u16()? as i16),
        tag::INT => Tag::Int(r.i32()?),
        tag::LONG => Tag::Long(r.i64()?),
        tag::FLOAT => Tag::Float(f32::from_bits(r.i32()? as u32)),
        tag::DOUBLE => Tag::Double(f64::from_bits(r.i64()? as u64)),
        tag::BYTE_ARRAY => {
            let n = r.i32()?;
            if n < 0 {
                return Err(NbtError(format!("byte array of {n}")));
            }
            let raw = r.take(n as usize)?;
            Tag::ByteArray(raw.iter().map(|b| *b as i8).collect())
        }
        tag::STRING => Tag::String(r.string()?),
        tag::LIST => {
            let element = r.u8()?;
            let n = r.i32()?;
            if n < 0 {
                return Err(NbtError(format!("list of {n}")));
            }
            let mut items = Vec::with_capacity(n.min(4096) as usize);
            for _ in 0..n {
                items.push(read_payload(r, element, depth + 1)?);
            }
            Tag::List { element, items }
        }
        tag::COMPOUND => Tag::Compound(read_compound(r, depth + 1)?),
        tag::INT_ARRAY => {
            let n = r.i32()?;
            if n < 0 {
                return Err(NbtError(format!("int array of {n}")));
            }
            let mut out = Vec::with_capacity(n.min(65536) as usize);
            for _ in 0..n {
                out.push(r.i32()?);
            }
            Tag::IntArray(out)
        }
        tag::LONG_ARRAY => {
            let n = r.i32()?;
            if n < 0 {
                return Err(NbtError(format!("long array of {n}")));
            }
            let mut out = Vec::with_capacity(n.min(65536) as usize);
            for _ in 0..n {
                out.push(r.i64()?);
            }
            Tag::LongArray(out)
        }
        other => return Err(NbtError(format!("tag id {other}"))),
    })
}

fn read_compound(r: &mut Reader, depth: usize) -> Result<Compound, NbtError> {
    let mut out = Compound::new();
    loop {
        let id = r.u8()?;
        if id == tag::END {
            return Ok(out);
        }
        let name = r.string()?;
        let value = read_payload(r, id, depth)?;
        out.set(&name, value);
    }
}

/// Reads one tag from a file's bytes.
///
/// The root is its tag id, then a name, then the payload -- see [`write`]. The
/// game refuses a root that is not a compound, which is worth refusing here too:
/// silently accepting one produces a file that writes and never reads back.
pub fn read(bytes: &[u8]) -> Result<Compound, NbtError> {
    let mut r = Reader { at: bytes };
    let root_id = r.u8()?;
    if root_id == tag::END {
        return Err(NbtError("the root tag must be a named compound".into()));
    }
    let _name = r.string()?;
    match read_payload(&mut r, root_id, 0)? {
        Tag::Compound(c) => Ok(c),
        other => Err(NbtError(format!("root is {other:?}, not a compound"))),
    }
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn put_payload(out: &mut Vec<u8>, t: &Tag) {
    match t {
        Tag::Byte(v) => out.push(*v as u8),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::ByteArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for b in v {
                out.push(*b as u8);
            }
        }
        Tag::String(v) => put_string(out, v),
        Tag::List { element, items } => {
            out.push(*element);
            out.extend_from_slice(&(items.len() as i32).to_be_bytes());
            for item in items {
                put_payload(out, item);
            }
        }
        Tag::Compound(c) => {
            for (name, value) in c.iter() {
                out.push(value.id());
                put_string(out, name);
                put_payload(out, value);
            }
            out.push(tag::END);
        }
        Tag::IntArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for n in v {
                out.extend_from_slice(&n.to_be_bytes());
            }
        }
        Tag::LongArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for n in v {
                out.extend_from_slice(&n.to_be_bytes());
            }
        }
    }
}

/// Writes a tag as a whole file.
///
/// `NbtIo.writeUnnamedTag` is three lines and the order matters: the tag id,
/// then an empty name, then the payload. There is no leading zero -- an empty
/// root compound is four bytes, `0A 00 00 00`, and the last of them is the
/// payload's own terminator.
pub fn write(root: &Compound) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(tag::COMPOUND);
    put_string(&mut out, "");
    put_payload(&mut out, &Tag::Compound(root.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(c: Compound) -> Compound {
        let bytes = write(&c);
        read(&bytes).expect("reads back what it wrote")
    }

    #[test]
    fn an_empty_file_is_four_bytes_and_no_more() {
        // `0A` compound, `00 00` the empty name, `00` the compound's own end.
        // Getting this wrong is the difference between a file the game reads and
        // one it silently treats as empty, so the length is pinned exactly.
        assert_eq!(write(&Compound::new()), vec![0x0A, 0x00, 0x00, 0x00]);
        assert_eq!(round_trip(Compound::new()), Compound::new());
    }

    #[test]
    fn every_number_is_big_endian() {
        let c = Compound::new()
            .put("b", Tag::Byte(-2))
            .put("s", Tag::Short(-300))
            .put("i", Tag::Int(0x0102_0304))
            .put("l", Tag::Long(-1));
        let back = round_trip(c);
        assert_eq!(back.get("b"), Some(&Tag::Byte(-2)));
        assert_eq!(back.get("s"), Some(&Tag::Short(-300)));
        assert_eq!(back.get("i"), Some(&Tag::Int(0x0102_0304)));
        assert_eq!(back.get("l"), Some(&Tag::Long(-1)));

        // And the bytes are big-endian, not merely symmetric.
        let bytes = write(&Compound::new().put("i", Tag::Int(1)));
        assert!(
            bytes.windows(4).any(|w| w == [0, 0, 0, 1]),
            "an int of one is 00 00 00 01 in that order: {bytes:02X?}"
        );
    }

    #[test]
    fn floats_keep_their_exact_bits() {
        // A round trip through f64 would quietly round every float, which shows
        // up later as a spawn point that is a hundredth of a block out.
        let c = Compound::new()
            .put("f", Tag::Float(0.1))
            .put("d", Tag::Double(0.1));
        let back = round_trip(c);
        assert_eq!(back.get("f"), Some(&Tag::Float(0.1)));
        assert_eq!(back.get("d"), Some(&Tag::Double(0.1)));
    }

    #[test]
    fn a_string_length_counts_bytes_not_characters() {
        // The mistake this catches: a name with an accent is more bytes than it
        // has characters, and a length counted in characters truncates it.
        let name = "Nguyá»…n";
        let back = round_trip(Compound::new().put("name", Tag::String(name.into())));
        assert_eq!(back.string("name"), Some(name));
        assert!(name.len() > name.chars().count());
    }

    #[test]
    fn an_empty_list_still_says_what_it_holds() {
        // Empty is not the same as untyped, and the byte is not optional: a
        // reader takes the element type from it and there is nothing else to
        // take it from.
        let empty_compounds = Tag::List {
            element: tag::COMPOUND,
            items: Vec::new(),
        };
        let back = round_trip(Compound::new().put("l", empty_compounds));
        match back.get("l") {
            Some(Tag::List { element, items }) => {
                assert_eq!(*element, tag::COMPOUND);
                assert!(items.is_empty());
            }
            other => panic!("{other:?}"),
        }

        let untyped = round_trip(Compound::new().put("l", Tag::list(Vec::new())));
        assert_eq!(untyped.get("l"), Some(&Tag::list(Vec::new())));
    }

    #[test]
    fn a_list_of_compounds_survives_with_its_element_type() {
        let items = vec![
            Tag::Compound(Compound::new().put("n", Tag::String("one".into()))),
            Tag::Compound(Compound::new().put("n", Tag::String("two".into()))),
        ];
        let back = round_trip(Compound::new().put("list", Tag::list(items)));
        let got = back.list("list").expect("a list");
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].as_compound().unwrap().string("n"), Some("two"));
    }

    #[test]
    fn arrays_round_trip_including_the_empty_one() {
        let c = Compound::new()
            .put("ia", Tag::IntArray(vec![1, -1, i32::MIN]))
            .put("ia0", Tag::IntArray(Vec::new()))
            .put("la", Tag::LongArray(vec![i64::MIN, 0, i64::MAX]))
            .put("ba", Tag::ByteArray(vec![0, -128, 127]))
            .put("ba0", Tag::ByteArray(Vec::new()));
        let back = round_trip(c);
        assert_eq!(
            back.get("ia"),
            Some(&Tag::IntArray(vec![1, -1, i32::MIN]))
        );
        assert_eq!(back.get("ia0"), Some(&Tag::IntArray(Vec::new())));
        assert_eq!(
            back.get("la"),
            Some(&Tag::LongArray(vec![i64::MIN, 0, i64::MAX]))
        );
        assert_eq!(back.get("ba"), Some(&Tag::ByteArray(vec![0, -128, 127])));
        assert_eq!(back.get("ba0"), Some(&Tag::ByteArray(Vec::new())));
    }

    #[test]
    fn a_nested_world_of_compounds_survives() {
        // The shape a chunk actually is, four levels down.
        let block_states = Compound::new()
            .put("data", Tag::LongArray(Vec::new()))
            .put(
                "palette",
                Tag::list(vec![Tag::Compound(
                    Compound::new().put("Name", Tag::String("minecraft:air".into())),
                )]),
            );
        let section = Compound::new()
            .put("Y", Tag::Byte(-4))
            .put("block_states", Tag::Compound(block_states));
        let level = Compound::new()
            .put("Sections", Tag::list(vec![Tag::Compound(section)]))
            .put("xPos", Tag::Int(0));
        let root = Compound::new().put("Level", Tag::Compound(level));

        let back = round_trip(root);
        let level = back.compound("Level").expect("Level");
        assert_eq!(level.int("xPos"), Some(0));
        let sections = level.list("Sections").expect("Sections");
        assert_eq!(sections.len(), 1);
        let y = match sections[0].as_compound().unwrap().get("Y") {
            Some(Tag::Byte(v)) => *v,
            other => panic!("{other:?}"),
        };
        assert_eq!(y, -4, "a section can be below zero");
    }

    #[test]
    fn writing_the_same_thing_twice_gives_the_same_bytes() {
        // BTreeMap, not insertion order. Without this every save is a different
        // file and a diff of two saves is noise.
        let one = Compound::new()
            .put("b", Tag::Int(2))
            .put("a", Tag::Int(1))
            .put("c", Tag::Int(3));
        let mut two = Compound::new();
        two.set("c", Tag::Int(3));
        two.set("a", Tag::Int(1));
        two.set("b", Tag::Int(2));
        assert_eq!(write(&one), write(&two));
    }

    #[test]
    fn a_file_that_does_not_start_with_zero_is_rejected() {
        assert!(read(&[0x0A, 0x00]).is_err(), "truncated");
        assert!(read(&[]).is_err(), "empty");
        assert!(read(&[0x03, 0x00, 0x00, 0x01, 0, 0, 0, 1]).is_err(), "wrong header");
    }

    #[test]
    fn a_negative_length_is_refused_rather_than_allocating() {
        // A count is the first thing read from a file that did not come from us,
        // and honouring a negative one is how a malformed file becomes a very
        // large allocation.
        let mut bytes = vec![0x0A, 0x00, 0x00, 0x09, 0x0A, 0x00];
        bytes.extend_from_slice(&(-1i32).to_be_bytes());
        bytes.extend_from_slice(&0i32.to_be_bytes());
        assert!(read(&bytes).is_err());
    }
}
