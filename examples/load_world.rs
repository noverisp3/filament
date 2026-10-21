//! Opens a world folder the game wrote and loads it the way the server does.
//!
//!     cargo run --example load_world -- <world-folder> [chunk-x chunk-z]
//!
//! Everything before this read files directly. This goes through `World`, so
//! it exercises the part that actually decides where a chunk comes from --
//! disk, or the generator -- which is the decision that was wrong for the
//! entire history of this project.
//
// Read-only unless something is passed as `--save`.

use std::path::Path;

use filament::world::pos::{BlockPos, ChunkPos};
use filament::world::storage::WorldStore;
use filament::world::World;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| {
            eprintln!("usage: load_world <world-folder> [chunk-x chunk-z]");
            std::process::exit(2);
        });
    let pos = ChunkPos::new(
        args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0),
        args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0),
    );

    let store = WorldStore::open(Path::new(&root)).unwrap_or_else(|e| {
        eprintln!("cannot open {root}: {e}");
        std::process::exit(1);
    });

    let mut world = World::with_store(Some(store));

    println!("{root}, chunk {},{}", pos.x, pos.z);
    if let Some(level) = world_store_level(&root) {
        println!("level.dat: {}", summarise(&level));
    }

    let already = world.ensure_chunk(pos);
    println!("chunk was already loaded: {already}");
    println!(
        "blocks registered so far: {}",
        world.registry_len()
    );

    // A column straight down, which is the cheapest thing that shows whether
    // this is the generator's hillside or somebody's ocean.
    println!("\ncolumn x=8 z=8:");
    let mut last = String::new();
    for y in (0..320).rev() {
        let at = BlockPos::new(pos.x * 16 + 8, y, pos.z * 16 + 8);
        let name = match world.block(at) {
            Some(id) => world.block_name(id).to_string(),
            None => "<unloaded>".to_string(),
        };
        if name != last {
            println!("  y={y:>4}  {name}");
            last = name;
        }
    }
}

/// Reads `level.dat` without disturbing anything else.
fn world_store_level(root: &str) -> Option<filament::world::nbt::Compound> {
    let bytes = std::fs::read(std::path::Path::new(root).join("level.dat")).ok()?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::GzDecoder::new(&bytes[..]),
        &mut out,
    )
    .ok()?;
    filament::world::nbt::read(&out).ok()
}

fn summarise(level: &filament::world::nbt::Compound) -> String {
    use filament::world::nbt::Tag;
    let data = level.compound("Data").unwrap_or(level);
    let name = match data.get("LevelName") {
        Some(Tag::String(s)) => s.clone(),
        _ => "?".into(),
    };
    let time = match data.get("Time") {
        Some(Tag::Long(t)) => *t,
        _ => 0,
    };
    let version = match data.get("DataVersion") {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    format!("{name}, data version {version}, time {time}")
}
