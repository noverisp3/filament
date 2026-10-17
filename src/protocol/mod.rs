//! The wire: packet framing and the primitives every field is built from.
//!
//! Minecraft's framing is a length-prefixed frame containing a VarInt packet id
//! and a body. Once the server sends a compression threshold, frames carry a
//! second length and their contents are zlib-deflated, which is a wire contract
//! with the client rather than a local optimisation.

pub mod chunk;

#[cfg(test)]
mod chunk_reference;
pub mod configuration;
pub mod frame;
pub mod handshake;
pub mod login;
pub mod play;
pub mod primitive;

#[cfg(test)]
mod frame_tests;

pub use frame::{
    FrameDecoder, FrameEncoder, FrameError, DEFAULT_COMPRESSION_THRESHOLD, MAX_FRAME_SIZE,
};
pub use primitive::{
    BoundedStr, DecodeError, Ident, VarInt, VarLong, DEFAULT_MAX_STRING,
};