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

mod blocks;

pub use blocks::{
    BlockDef, BlockState, Property, BLOCKS, BLOCK_COUNT, BLOCK_STATE_COUNT,
};

#[cfg(test)]
mod tests;