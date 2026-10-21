//! The server's brand, which the client shows in its debug screen.

use super::id;
use crate::protocol::VarInt;

/// Writes the server's brand, the packet the client shows as "Server brand"
/// in its debug screen.
///
/// The game sends it first thing in the configuration phase
/// (`ServerConfigurationPacketListenerImpl.startConfiguration`), and the
/// client keeps whatever arrives: with none, the debug screen says the server
/// is a null one.
///
/// The shape is a custom payload: the channel as a string, then the payload
/// itself, which for the brand is another string.
pub fn write_brand(out: &mut Vec<u8>, brand: &str) {
    VarInt(id::CUSTOM_PAYLOAD).write(out);
    write_utf(out, "minecraft:brand");
    write_utf(out, brand);
}

/// A string the way the game writes one: a byte count, then the bytes.
fn write_utf(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    VarInt(bytes.len() as i32).write(out);
    out.extend_from_slice(bytes);
}
