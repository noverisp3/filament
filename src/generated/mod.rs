//! Tables generated from Minecraft's own data generator.
//!
//! The contents are build-script output and are not edited by hand. Regenerate
//! with:
//!
//! ```text
//! powershell -ExecutionPolicy Bypass -File tools/generate-reports.ps1
//! python tools/build_tables.py
//! python tools/extract_hardness.py
//! ```
//!
//! See `tools/generate-reports.ps1` for why the reports come from the vanilla
//! server jar rather than from a hand-written list.
//!
//! Two tables are the exception, and each carries its own note on where it comes
//! from. The tags table comes from a capture of a running server rather than from
//! a report, because the data generator does not record the registry order the
//! tag packet depends on. The hardness table comes from the decompiled source
//! rather than from either, because hardness is static data compiled into the
//! game jars and neither the reports nor the registry frames sent at login
//! contain it.

mod blocks;
mod hardness;
mod items;
mod registries;
mod tags;

pub use blocks::{
    BlockDef, BlockState, Property, BLOCKS, BLOCK_COUNT, BLOCK_STATE_COUNT,
};
pub use hardness::{HARDNESS, REQUIRES_TOOL, hardness_of, requires_tool};
pub use items::{ITEMS, item_id, item_name};
pub use registries::{REGISTRY_DATA_COUNT, REGISTRY_DATA_FRAMES};
pub use tags::{TAG_COUNTS, TAG_TOTAL, UPDATE_TAGS_BODY};

#[cfg(test)]
mod tests;