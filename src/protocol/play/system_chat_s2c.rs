//! A line of chat, for everyone to see.

use super::id;
use crate::protocol::VarInt;

/// Writes a line of chat.
///
/// The content is a component, and a component in a packet is one NBT tag:
/// a plain text one is a string tag, which is what a whole line of chat is.
/// The byte after it says whether the line is an overlay above the world
/// rather than in the chat box, and it is not one.
pub fn write_system_chat(out: &mut Vec<u8>, text: &str) {
    VarInt(id::SYSTEM_CHAT).write(out);
    crate::world::nbt::write_string_tag(out, text);
    out.push(0);
}
