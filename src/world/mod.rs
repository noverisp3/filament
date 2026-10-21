//! World data: block storage, chunk layout, coordinates.
//!
//! A chunk is 16 by 384 by 16, split into 24 vertical **sections** of 16³
//! blocks. Each section's blocks live in one [`palette::Paletted`], which
//! holds a bit-packed array of palette indices plus the palette itself, and a
//! uniform section spends **zero** dense bytes.
//!
//! Light is deliberately **not** part of this layer. Light changes far more
//! often than block ids, so folding it into the palette would blow the
//! palette up. It lives in [`light`] as two independent layers.
//!
//! SoA/AoS has two different answers here, and mixing them up is the easy
//! mistake:
//!
//! * **Chunk storage is SoA at layer granularity** -- blocks and light are
//!   separate arrays because the hot operations genuinely differ.
//! * **The vertex buffer is interleaved (AoS)**, because the GPU reads all of a
//!   vertex's attributes together. See `render::mesh`.

pub mod bitpack;
pub mod block;
pub mod chunk;
pub mod chunk_nbt;
pub mod item_entity;
pub mod light;
pub mod light_engine;
pub mod nbt;
pub mod palette;
pub mod pos;
pub mod raycast;
pub mod region;
pub mod section;
pub mod ticks;
pub mod terrain;
// The game calls the live world a `Level` (`net.minecraft.world.level.Level`),
// and the name says what the module holds: the world as it is right now, with
// its chunks, its edits and its light, rather than the formats they are stored
// in.
pub mod level;
pub mod storage;

pub use level::{BlockEdit, World};
