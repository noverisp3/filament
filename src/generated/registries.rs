//! Registry contents captured from a vanilla 26.3 server. Do not edit by hand.
//!
//! Each frame is a whole `ClientboundRegistryDataPacket` body, packet id
//! included, so the encoder is handed it without anything having to be rebuilt.
//! The frames sit one after another, each behind a VarInt length, which is the
//! same framing `tools/capture.py` writes to `captured/packets.bin`.
//!
//! Regenerate with:
//!
//! ```text
//! python tools/capture.py 127.0.0.1 25566
//! python tools/extract_registries.py
//! ```
//!
//! Why this file is needed, when a client is supposed to fall back to the
//! registries it was built with: the fallback is taken when no registry_data
//! arrives, and on that path the tags are resolved against the client's built-in
//! registries. Those are the ones baked into the jar, which is not the same set.
//! `minecraft:dialog` is a datapack registry and is not among them, so the very
//! first tag on a datapack registry throws:
//!
//! ```text
//! Missing registry: ResourceKey[minecraft:root / minecraft:dialog]
//! ```
//!
//! Sending the registries also fixes the other half: tag contents are registry
//! indices, so without these the client has no order to resolve an index
//! against.

/// Every captured frame, each behind a VarInt length.
pub const REGISTRY_DATA_FRAMES: &[u8] = include_bytes!("../../assets/registry_data.bin");

/// How many frames the file holds.
///
/// Recorded rather than computed so a file that was truncated or regenerated in
/// a different order shows up as a failing test instead of as a client that
/// hangs up with no explanation.
pub const REGISTRY_DATA_COUNT: usize = 32;

#[cfg(test)]
mod tests;