//! Prints an NBT file as a tree.
//!
//! An NBT file is usually gzipped and definitely binary, so reading one to find
//! out what is in it means either an external tool or twenty minutes of
//! `Format-Hex`. This is the second option, using the same reader the server
//! uses -- which means a real Minecraft file that this prints is also a test
//! that the reader is right.
//!
//!     cargo run --example dump_nbt -- <path> [max-depth]
//!
//! `level.dat` and every `.mca` in `region/` are gzipped on disk; both are
//! accepted.

use std::io::Read;

use filament::world::nbt::{Compound, NbtError, Tag};

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .map_err(|e| format!("gzip: {e}"))?;
    Ok(out)
}

fn describe(tag: &Tag) -> String {
    match tag {
        Tag::Byte(v) => format!("{v}b"),
        Tag::Short(v) => format!("{v}s"),
        Tag::Int(v) => v.to_string(),
        Tag::Long(v) => format!("{v}L"),
        Tag::Float(v) => format!("{v}f"),
        Tag::Double(v) => format!("{v}d"),
        Tag::ByteArray(v) => format!("{} bytes", v.len()),
        Tag::String(v) => format!("\"{v}\""),
        Tag::List { element, items } => format!("[{}] x{}", kind(*element), items.len()),
        Tag::Compound(_) => "{...}".to_string(),
        Tag::IntArray(v) => format!("[int] x{}", v.len()),
        Tag::LongArray(v) => format!("[long] x{}", v.len()),
    }
}

fn kind(id: u8) -> &'static str {
    match id {
        0 => "end",
        1 => "byte",
        2 => "short",
        3 => "int",
        4 => "long",
        5 => "float",
        6 => "double",
        7 => "byte[]",
        8 => "string",
        9 => "list",
        10 => "compound",
        11 => "int[]",
        12 => "long[]",
        _ => "?",
    }
}

fn print_compound(compound: &Compound, indent: usize, depth: usize, max: usize) {
    for (key, tag) in compound.iter() {
        let pad = "  ".repeat(indent);
        match tag {
            Tag::Compound(inner) => {
                println!("{pad}{key}: compound ({} entries)", inner.len());
                if depth < max {
                    print_compound(inner, indent + 1, depth + 1, max);
                }
            }
            Tag::List { element, items } => {
                println!("{pad}{key}: list of {} x{}", kind(*element), items.len());
                if depth < max {
                    for (index, item) in items.iter().enumerate().take(8) {
                        let pad = "  ".repeat(indent + 1);
                        match item {
                            Tag::Compound(inner) => {
                                println!("{pad}[{index}] compound ({} entries)", inner.len());
                                if depth + 1 < max {
                                    print_compound(inner, indent + 2, depth + 2, max);
                                }
                            }
                            other => println!("{pad}[{index}] {}", describe(other)),
                        }
                    }
                    if items.len() > 8 {
                        println!("{pad}  ... {} more", items.len() - 8);
                    }
                }
            }
            other => println!("{pad}{key}: {}", describe(other)),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = match args.get(1) {
        Some(p) => p.clone(),
        None => {
            eprintln!("usage: dump_nbt <path> [max-depth] [chunk-x chunk-z]");
            std::process::exit(2);
        }
    };
    let max_depth: usize = args.get(2).and_then(|d| d.parse().ok()).unwrap_or(6);

    // A `.mca` is a container of NBT files rather than one, so it needs the
    // region reader to get at any of it.
    if path.ends_with(".mca") {
        dump_region(&path, max_depth, args.get(3), args.get(4));
        return;
    }

    let raw = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(2);
        }
    };

    println!("{path}: {} bytes on disk", raw.len());

    // Gzip's magic is checked rather than assumed: some tools write these
    // files uncompressed, and failing on the header would hide a real problem
    // behind a wrong guess.
    let bytes = if raw.starts_with(&[0x1f, 0x8b]) {
        match gunzip(&raw) {
            Ok(b) => {
                println!("{} bytes once decompressed", b.len());
                b
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    } else {
        println!("not gzipped");
        raw
    };

    match filament::world::nbt::read(&bytes) {
        Ok(root) => {
            println!("root: {} entries\n", root.len());
            print_compound(&root, 0, 0, max_depth);
        }
        Err(NbtError(message)) => {
            eprintln!("could not read the tag: {message}");
            std::process::exit(1);
        }
    }
}

/// Prints chunks out of a real region file, using this build's own reader.
fn dump_region(path: &str, max_depth: usize, x: Option<&String>, z: Option<&String>) {
    let region = match filament::world::region::RegionFile::read(std::path::Path::new(path)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    println!("{path}: {} chunks", region.len());

    let wanted = match (x, z) {
        (Some(x), Some(z)) => match (x.parse::<i32>(), z.parse::<i32>()) {
            (Ok(x), Ok(z)) => Some((x, z)),
            _ => {
                eprintln!("chunk x and z must be numbers");
                std::process::exit(2);
            }
        },
        _ => None,
    };

    for region_x in -1..=1 {
        for region_z in -1..=1 {
            for local_x in 0..filament::world::region::CHUNKS_ACROSS {
                for local_z in 0..filament::world::region::CHUNKS_ACROSS {
                    let (cx, cz) = (region_x * 32 + local_x, region_z * 32 + local_z);
                    if !region.has(cx, cz) {
                        continue;
                    }
                    if wanted.is_some_and(|want| want != (cx, cz)) {
                        continue;
                    }
                    let chunk = region.get(cx, cz).unwrap();
                    println!("\n=== chunk {cx},{cz} ===");
                    print_compound(chunk, 0, 0, max_depth);
                }
            }
        }
    }
}
