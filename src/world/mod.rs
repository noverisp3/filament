//! World data: block storage, chunk layout, coordinates.
//!
//! Storage layout, fixed in phase 2 and expensive to change later:
//!
//! * A chunk is 16×384×16, split into 24 vertical **sections** of 16³ blocks.
//! * Each section's blocks live in one [`palette::Paletted`], which holds a
//!   bit-packed array of palette indices plus the palette itself.
//! * A uniform section spends **zero** dense bytes; its palette holds one entry.
//! * Entry width grows only when the palette outgrows it, so widening is
//!   amortised O(1) per write.
//! * Light is deliberately **not** part of this layer. Light changes
//!   far more often than block ids, so folding it into the palette would blow
//!   the palette up and destroy the compression. It lives in [`light`] as two
//!   independent layers.
//!
//! SoA/AoS has two different answers here, and mixing them up is the easy
//! mistake:
//!
//! * **Chunk storage is SoA at layer granularity** — blocks, and later light
//!   and biomes, are separate arrays because the hot operations genuinely differ
//!   (skylight sweeps one attribute across thousands of blocks).
//! * **The vertex buffer is interleaved (AoS)**, because the GPU reads all of a
//!   vertex's attributes together. See `render.rs`.
//!
//! Re-exports are added as each type gets used by another module; until then the
//! canonical paths are `world::pos::BlockPos`, `world::block::BlockId`, and so
//! on.

// Everything here is exercised by `cargo test` and consumed by the renderer in
// phase 3. Until then nothing in the binary reaches it, so dead-code analysis
// has nothing to connect it to. Remove this once `render.rs` renders sections.
#![allow(dead_code)]

pub mod bitpack;
pub mod block;
pub mod chunk;
pub mod light;
pub mod lighting;
pub mod palette;
pub mod pos;
pub mod raycast;
pub mod section;
pub mod terrain;
// `world::world` reads oddly next to the other `world::*` modules, but every
// alternative name is worse. See re-exports below.
#[allow(clippy::module_inception)]
pub mod world;
