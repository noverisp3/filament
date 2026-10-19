//! Decodes a chunk out of a real region file with this build's own reader.
//!
//! Everything else in this project is tested against data filament wrote
//! itself, which proves a round trip works and says nothing about whether the
//! arrangement on disk is the one the game expects. This reads a chunk the
//! game wrote, so a mistake shows up as something impossible to miss:
//!
//! * a palette index outside its own palette, which cannot happen if the bit
//!   layout is right, and happens immediately if it is not;
//! * terrain that is not terrain -- a surface line of noise, or a world of
//!   floating dirt with no stone under it.
//!
//!     cargo run --example inspect_chunk -- <r.X.Z.mca> <chunk-x> <chunk-z>
//!
//! Reads only; never writes to the world.

use std::path::Path;

use filament::world::bitpack::BitArray;
use filament::world::chunk_nbt::{from_vanilla_words, palette_name};
use filament::world::nbt::{Compound, Tag};
use filament::world::chunk_nbt::block_bits_for_palette_len;
use filament::world::region::RegionFile;

const SECTION_VOLUME: usize = 4096;

fn palette_of(states: &Compound) -> Vec<String> {
    states
        .list("palette")
        .expect("block_states has no palette")
        .iter()
        .map(|entry| {
            palette_name(entry).unwrap_or_else(|| {
                panic!("palette entry names neither a string nor a compound: {entry:?}")
            })
            .to_string()
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (path, cx, cz) = match (args.get(1), args.get(2), args.get(3)) {
        (Some(p), Some(x), Some(z)) => (p.clone(), x.clone(), z.clone()),
        _ => {
            eprintln!("usage: inspect_chunk <r.X.Z.mca> <chunk-x> <chunk-z>");
            std::process::exit(2);
        }
    };

    let region = RegionFile::read(Path::new(&path)).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        std::process::exit(1);
    });
    let chunk = region
        .get(cx.parse().unwrap(), cz.parse().unwrap())
        .unwrap_or_else(|| {
            eprintln!("no chunk {cx},{cz} in {path}");
            std::process::exit(1);
        });

    println!("{path} chunk {cx},{cz}");

    let sections = chunk.list("sections").expect("chunk has no sections");
    let mut bad_indices = 0usize;
    let mut total = 0usize;
    let mut decoded: Vec<Option<(i32, Vec<String>, BitArray)>> = Vec::new();

    for entry in sections {
        let section = entry.as_compound().expect("section is not a compound");
        let y = match section.get("Y") {
            Some(Tag::Byte(b)) => *b as i32,
            other => panic!("section has no Y: {other:?}"),
        };

        let Some(states) = section.compound("block_states") else {
            decoded.push(None);
            continue;
        };
        let palette = palette_of(states);

        let Some(Tag::LongArray(words)) = states.get("data") else {
            // One palette entry means no index array at all: every block in
            // the section is that block.
            decoded.push(Some((y, palette, BitArray::new(SECTION_VOLUME, 1))));
            continue;
        };

        // The width is not stored. It has to come from the palette, which is
        // the only place it survives.
        let bits = block_bits_for_palette_len(palette.len());
        let flat = BitArray::from_words(from_vanilla_words(bits, words, SECTION_VOLUME), bits, SECTION_VOLUME);

        for i in 0..SECTION_VOLUME {
            total += 1;
            if flat.get(i) as usize >= palette.len() {
                bad_indices += 1;
            }
        }
        decoded.push(Some((y, palette, flat)));
    }

    println!(
        "\n{total} blocks decoded, {bad_indices} with an index outside their palette"
    );
    if bad_indices > 0 {
        println!("the bit layout does not match -- this chunk is not being read correctly");
        std::process::exit(1);
    }
    println!("every index is inside its palette, so the packing is right");

    // A cross-section through one column. If the layout were wrong this would
    // be noise; with it right, the same few blocks stack in the same order
    // every time: bedrock, stone, dirt, grass, then air.
    const X: usize = 8;
    const Z: usize = 8;
    let mut column: Vec<(i32, String)> = Vec::new();
    for (y, palette, flat) in decoded.iter().flatten() {
        for local_y in 0..16usize {
            let index = (local_y << 8) | (Z << 4) | X;
            let name = palette[flat.get(index) as usize]
                .strip_prefix("minecraft:")
                .unwrap_or(&palette[flat.get(index) as usize])
                .to_string();
            column.push((y * 16 + local_y as i32, name));
        }
    }
    column.sort_by_key(|(y, _)| -*y);

    println!("\ncolumn x={X} z={Z}, top of world downwards:");
    let mut last = String::new();
    for (y, name) in &column {
        if *name != last {
            println!("  y={y:>4}  {name}");
            last = name.clone();
        }
    }
}
