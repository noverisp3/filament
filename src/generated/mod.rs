//! Tables generated from Minecraft's own data generator.
//!
//! The contents are build-script output and are not edited by hand. Regenerate
//! with:
//!
//! ```text
//! powershell -ExecutionPolicy Bypass -File tools/generate-reports.ps1
//! python tools/build_tables.py
//! python tools/extract_hardness.py
//! python tools/extract_suffocation.py
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
//! contain it. The light table is the same: how bright a block is is static
//! data in the block definitions, and it is read out of them for the same
//! reason. The suffocation table is the same too, and for the same reason: the
//! tag that says which blocks stop a player from breathing is in the game and
//! is not in any report, and the copy in this repository's `extracted/` is
//! from an older capture that stops at a block state this build is past. The
//! heightmap tables are the same for the same reason: which blocks stop a
//! player, and which blocks are air, are in the game's own tags and in its own
//! source.

mod blocks;
mod hardness;
mod heightmaps;
mod items;
mod light;
mod registries;
mod suffocation;
mod tags;

pub use blocks::{
    BlockDef, BlockState, Property, BLOCKS, BLOCK_COUNT, BLOCK_STATE_COUNT,
};
pub use hardness::{HARDNESS, REQUIRES_TOOL, hardness_of, requires_tool};
pub use heightmaps::{
    is_air, motion_blocking, motion_blocking_no_leaves, AIR, MOTION_BLOCKING,
    MOTION_BLOCKING_NO_LEAVES,
};
pub use items::{ITEMS, item_id, item_name};
pub use light::LIGHT;
pub use registries::{REGISTRY_DATA_COUNT, REGISTRY_DATA_FRAMES};
pub use suffocation::{causes_suffocation, CAUSES_SUFFOCATION};
pub use tags::{TAG_COUNTS, TAG_TOTAL, UPDATE_TAGS_BODY};

#[cfg(test)]
mod tests;