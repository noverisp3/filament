//! Multithreaded Minecraft server, written from scratch in Rust.
//!
//! Minecraft's own server ticks chunks, entities and light on a single thread.
//! That is the weakness this exists to fix, so everything here is built around
//! one rule: **no work that can be parallel may run on the main thread.**
//!
//! The data layout is the second reason this is not just a port. Blocks, light
//! and biomes are stored as separate layers (SoA) because the hot operations
//! genuinely differ: a skylight sweep walks one attribute across thousands of
//! blocks and would be ruined by interleaving it with the others.
//!
//! Height is Minecraft's own: -64 to 320, 384 blocks, 24 sections.

pub mod generated;
pub mod render;
pub mod world;