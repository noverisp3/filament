//! What a player does with what they are carrying.
//!
//! The client does not describe the click, it reports the *result* of it: every
//! slot that changed goes back with its new contents, along with whatever is
//! left on the cursor. This server never has to understand a drag to know what
//! a drag did.
//!
//! It does not mean the client gets the last word. Vanilla reads those reported
//! slots as a prediction and compares, and then sends its own value wherever
//! the two disagree (`AbstractContainerMenu.synchronizeSlotToRemote`). The
//! server checks the totals against what the player was issued, and the client
//! is told only when its guess was wrong.

use super::item::ItemStack;
use crate::protocol::primitive::DecodeError;
use crate::protocol::VarInt;

/// What the client says it did.
///
/// The ids are `ContainerInput`'s, and they are what the click is *about* rather
/// than how it was made -- `SWAP` covers both the number keys and dragging onto
/// the hotbar, `THROW` covers the drop key and dropping outside the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerInput {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
    /// An id the game does not have. Refused rather than guessed at.
    Unknown(u8),
}

impl ContainerInput {
    fn from_id(id: i32) -> Self {
        match id {
            0 => Self::Pickup,
            1 => Self::QuickMove,
            2 => Self::Swap,
            3 => Self::Clone,
            4 => Self::Throw,
            5 => Self::QuickCraft,
            6 => Self::PickupAll,
            other => Self::Unknown(other.clamp(0, u8::MAX as i32) as u8),
        }
    }

    /// Whether this is a drag, which is answered in stages rather than at once.
    pub fn is_drag(self) -> bool {
        matches!(self, Self::QuickCraft)
    }
}

/// `ServerboundContainerClickPacket`, read.
///
/// ```text
/// containerId   = readVarInt()
/// stateId       = readVarInt()
/// slotNum       = readShort()
/// buttonNum     = readByte()
/// containerInput= readByte()          // ContainerInput, not a VarInt
/// changedSlots  = readCount(128), then that many of (short, HashedStack)
/// carriedItem   = HashedStack
/// ```
///
/// and `HashedStack` is an optional -- a raw boolean byte, and only if it is set
/// the item itself -- so an empty one costs a single byte.
#[derive(Debug, Clone)]
pub struct ContainerClickPacket {
    pub container_id: i32,
    pub state_id: i32,
    pub slot: i16,
    pub button: u8,
    pub input: ContainerInput,
    /// What the client believes each slot holds now.
    pub changed: Vec<(i16, Option<ItemStack>)>,
    /// What the client believes is on the cursor.
    pub carried: Option<ItemStack>,
}

/// A VarInt as a plain `i32`.
///
/// `VarInt::read` hands back the value and the bytes it took, and every field
/// here wants only the value. Going through one name for it means the packet
/// below reads as the field list it is, rather than as a pile of tuple indexing.
fn read_varint(body: &mut &[u8]) -> Result<i32, DecodeError> {
    Ok(VarInt::read(body)?.0 .0)
}

impl ContainerClickPacket {
    pub fn read(frame: &[u8]) -> Result<Self, DecodeError> {
        // Step over the id first, and check it. A decoder that starts on the id
        // reads 18 as the container id, which is a small perfectly ordinary
        // number, so nothing fails until two fields later.
        let mut body =
            crate::protocol::play::serverbound_body(frame, crate::protocol::play::id::SERVERBOUND_CONTAINER_CLICK)?;

        let container_id = read_varint(&mut body)?;
        let state_id = read_varint(&mut body)?;
        let slot = read_short(&mut body)?;
        let button = read_byte(&mut body)?;
        // A VarInt, not a byte: `ContainerInput.STREAM_CODEC` is
        // `ByteBufCodecs.idMapper`. Every id the game has is under 128, so it
        // happens to occupy one byte either way, which makes the difference
        // invisible until the first id that does not.
        let input = ContainerInput::from_id(read_varint(&mut body)?);

        let count = read_count(&mut body, MAX_CHANGED)?;
        let mut changed = Vec::with_capacity(count.min(64) as usize);
        for _ in 0..count {
            let slot = read_short(&mut body)?;
            changed.push((slot, read_hashed_stack(&mut body)?));
        }

        let carried = read_hashed_stack(&mut body)?;

        Ok(ContainerClickPacket { container_id, state_id, slot, button, input, changed, carried })
    }
}

/// `changedSlots` is built with a limit of 128 and the components with 256.
const MAX_CHANGED: i32 = 128;
const MAX_COMPONENTS: i32 = 256;

/// `ServerboundSetCarriedItemPacket`, read: one `Short`.
///
/// The slot the player has selected on the hotbar, as a window slot -- 36 to 44.
/// Sent every time the scroll wheel turns or a number key goes down, which is the
/// only way the server ever finds out which item is about to be used.
///
/// Steps over the packet id for the same reason
/// [`ContainerClickPacket::read`] does.
pub fn read_set_carried_item(frame: &[u8]) -> Result<u16, DecodeError> {
    let mut body = crate::protocol::play::serverbound_body(
        frame,
        crate::protocol::play::id::SERVERBOUND_SET_CARRIED_ITEM,
    )?;
    Ok(read_short(&mut body)? as u16)
}

/// `ByteBufCodecs.readCount`: a VarInt, refused if it claims more than the limit.
///
/// A count that big is a hostile or broken client, and the point of refusing it
/// here is that it is refused *before* the loop that would try to reserve it.
fn read_count(body: &mut &[u8], max: i32) -> Result<i32, DecodeError> {
    let count = read_varint(body)?;
    // Refused here, before the loop that would try to reserve it: a hostile or
    // broken client can announce a collection of any size at all, and reading it
    // as "that many" is how a packet turns into an allocation the size of a
    // number off the wire.
    if count < 0 || count > max {
        return Err(DecodeError::TooLong);
    }
    Ok(count)
}

/// One reported stack.
///
/// `HashedStack.STREAM_CODEC` is `ByteBufCodecs.optional(...)`, which is a plain
/// boolean byte and then the value, so nothing at all is one byte.
///
/// The components are read rather than skipped, because skipping past a length
/// that was never checked is how a packet lands one field out. A stack
/// carrying components is refused: this server has nothing that produces one,
/// so meeting one means the two sides disagree about what an item is.
fn read_hashed_stack(body: &mut &[u8]) -> Result<Option<ItemStack>, DecodeError> {
    if !read_bool(body)? {
        return Ok(None);
    }
    let item = read_varint(body)?;
    let count = read_varint(body)?;
    if count <= 0 {
        return Ok(None);
    }

    let added = read_count(body, MAX_COMPONENTS)?;
    for _ in 0..added {
        read_varint(body)?; // the component's type
        skip(body, 4)?; // its hash
    }
    let removed = read_count(body, MAX_COMPONENTS)?;
    for _ in 0..removed {
        read_varint(body)?;
    }
    if added > 0 || removed > 0 {
        // Nothing this server produces carries components, so meeting one means
        // the two sides disagree about what an item is. Refusing says so;
        // dropping it silently would leave the player's stack quietly wrong.
        return Err(DecodeError::TooLong);
    }

    Ok(Some(ItemStack::new(item, count)))
}

fn read_bool(body: &mut &[u8]) -> Result<bool, DecodeError> {
    let (&byte, rest) = body
        .split_first()
        .ok_or(DecodeError::UnexpectedEof { wanted: "a boolean", got: 0 })?;
    *body = rest;
    Ok(byte != 0)
}

fn read_byte(body: &mut &[u8]) -> Result<u8, DecodeError> {
    let (&byte, rest) = body
        .split_first()
        .ok_or(DecodeError::UnexpectedEof { wanted: "a byte", got: 0 })?;
    *body = rest;
    Ok(byte)
}

/// A `Short` as the game writes one.
///
/// Big endian: `FriendlyByteBuf.writeShort` calls netty's `writeShort`, and the
/// `ShortLE` variants are the separate little-endian ones. A slot read
/// little-endian is a different number, usually out of range, and the click
/// that follows it is read from the wrong place.
fn read_short(body: &mut &[u8]) -> Result<i16, DecodeError> {
    if body.len() < 2 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a short",
            got: body.len(),
        });
    }
    let (head, rest) = body.split_at(2);
    *body = rest;
    Ok(i16::from_be_bytes([head[0], head[1]]))
}

fn skip(body: &mut &[u8], n: usize) -> Result<(), DecodeError> {
    if body.len() < n {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a component hash",
            got: body.len(),
        });
    }
    *body = &body[n..];
    Ok(())
}



#[cfg(test)]
mod tests;
