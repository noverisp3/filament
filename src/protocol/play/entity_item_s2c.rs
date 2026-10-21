//! What an item on the ground is holding.

use super::id;
use super::ENTITY_DATA_END;
use crate::protocol::VarInt;

/// Which piece of an item entity's metadata holds the item.
///
/// Eight, and not zero. The ids of the metadata are not counted per entity but
/// along the line of ancestors: a class's ids start after the last id its
/// parent has, and `Entity` has eight of its own.
pub const ENTITY_DATA_ITEM: u8 = 8;

/// Which serializer reads an item stack, out of the table the game registers
/// them in.
///
/// Seven, counted from the static initialiser in `EntityDataSerializers`: byte,
/// int, long, float, string, component, optional component, and then the item
/// stack. The pose is twenty, which is what the other writer here uses, and
/// the two together are the check that the count is right.
pub const ENTITY_DATA_SERIALIZER_ITEM_STACK: i32 = 7;

/// Writes an item entity's stack, which is how a client learns what an item on
/// the ground is.///
/// ```text
/// VarInt   the entity id
/// byte     which piece of metadata this is
/// VarInt   which serializer reads what follows
/// VarInt   how many
/// VarInt   which item
/// VarInt   zero
/// VarInt   zero
/// byte     255, and nothing after it
/// ```
///
/// The two zeroes are the patch of data components, which is empty when there
/// is nothing to say about the stack.
pub fn write_entity_item(out: &mut Vec<u8>, entity_id: i32, stack: &crate::protocol::item::ItemStack) {
    VarInt(id::SET_ENTITY_DATA).write(out);
    VarInt(entity_id).write(out);
    out.push(ENTITY_DATA_ITEM);
    VarInt(ENTITY_DATA_SERIALIZER_ITEM_STACK).write(out);
    stack.write(out);
    out.push(ENTITY_DATA_END);
}
