//! Chunk meshing.
//!
//! The server needs this, not the client: chunk data sent to a player is
//! already meshed, so meshing belongs on this side of the wire. Keeping it here
//! also means it can be unit tested with no window and no GPU, which it was
//! designed for from the start — it imports nothing but `world::`.

pub mod mesh;