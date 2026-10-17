//! Tables generated from Minecraft's own data generator.
//!
//! The contents are build-script output and are not edited by hand. Regenerate
//! with:
//!
//! ```text
//! powershell -ExecutionPolicy Bypass -File tools/generate-reports.ps1
//! python tools/build_tables.py
//! ```
//!
//! See `tools/generate-reports.ps1` for why the reports come from the vanilla
//! server jar rather than from a hand-written list.
//!
//! The tags table is the exception: it comes from a capture of a running server
//! rather than from a report, because the data generator does not record the
//! registry order the tag packet depends on. `tags` carries its own note on how
//! to regenerate it.

mod blocks;
mod registries;
mod tags;

pub use blocks::{
    BlockDef, BlockState, Property, BLOCKS, BLOCK_COUNT, BLOCK_STATE_COUNT,
};
pub use registries::{REGISTRY_DATA_COUNT, REGISTRY_DATA_FRAMES};
pub use tags::{TAG_COUNTS, TAG_TOTAL, UPDATE_TAGS_BODY};

#[cfg(test)]
mod tests;