//! What a player does with what they are carrying.
//!
//! A broken block going into a hotbar is only half the story. The moment the
//! player touches it -- picks it up, drags it, presses a number to swap it, drops
//! it on the floor -- the client has done something the server knows nothing
//! about, and a server that keeps its own tally anyway is holding a number that
//! has stopped being true. It then reports slots that have not changed and
//! misses ones that have, and the two copies drift apart for as long as the
//! player is online.
//!
//! So the server has to learn. The wire in 26.3 makes that cheap, which is
//! lucky: the client does not describe the click, it reports the *result* of it.
//! Every slot that changed goes back with its new contents, along with whatever
//! is left on the cursor. That means this server never has to understand a drag
//! to know what a drag did.
//!
//! It does not mean the client gets the last word, though. Vanilla reads those
//! reported slots as a prediction and compares:
//!
//! ```java
//! this.player.containerMenu.suppressRemoteUpdates();
//! this.player.containerMenu.clicked(slotIndex, packet.buttonNum(), packet.containerInput(), this.player);
//! for (Entry<HashedStack> e : Int2ObjectMaps.fastIterable(packet.changedSlots())) {
//!     this.player.containerMenu.setRemoteSlotUnsafe(e.getIntKey(), e.getValue());
//! }
//! this.player.containerMenu.setRemoteCarried(packet.carriedItem());
//! this.player.containerMenu.resumeRemoteUpdates();
//! ```
//!
//! and then, per slot, sends its own value wherever the two disagree
//! (`AbstractContainerMenu.synchronizeSlotToRemote`). So the server computes the
//! click, the client is told only when its guess was wrong, and the server stays
//! the authority. That is what [`click`] and [`Click::changed`] are for.

use super::item::{ItemStack, Inventory, MAX_STACK};
use super::item::slots;
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

/// The slot outside the window that means "the floor".
///
/// Vanilla spells this as `-999` in a `Short`, and it is how a player drops what
/// is on their cursor or out of the window.
pub const THROW_SLOT: i16 = -999;

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
        // Step over the id first, and check it. The frame handed to a decoder
        // still carries it, because that is what the wire carries; a decoder
        // that starts on the id reads 18 as the container id, which is a small
        // perfectly ordinary number, so nothing fails until two fields later.
        let mut body =
            crate::protocol::play::serverbound_body(frame, crate::protocol::play::id::SERVERBOUND_CONTAINER_CLICK)?;

        let container_id = read_varint(&mut body)?;
        let state_id = read_varint(&mut body)?;
        let slot = read_short(&mut body)?;
        let button = read_byte(&mut body)?;
        // A VarInt, not a byte. `ContainerInput.STREAM_CODEC` is
        // `ByteBufCodecs.idMapper`, which reads and writes a VarInt. Every id
        // the game has is under 128 so it happens to occupy one byte either way,
        // which makes the difference invisible until the first id that does not
        // -- and it is worth reading the way it is written rather than the way
        // that happens to work today.
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
/// boolean byte and then the value -- so nothing at all is one byte.
///
/// The components are read rather than skipped, because skipping past a length
/// that was never checked is how a packet lands one field out and the *next*
/// packet is the one that goes wrong, somewhere unrelated and much later. A
/// stack carrying components is refused instead: this server has nothing that
/// produces one, so meeting one means the two sides disagree about what an item
/// is, and quietly dropping half of it would be worse than saying so.
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
/// Big endian. `FriendlyByteBuf.writeShort` calls netty's `writeShort`, which
/// is big endian, and the `ShortLE` variants are the separate little-endian ones
/// -- so a slot read little-endian is a different number, usually out of range,
/// and the click that follows it is read from the wrong place. This file was the
/// only one in the crate reading them the other way round, which is exactly how
/// a lone wrong endianness survives: nothing local looks odd, the packet simply
/// stops making sense three fields later.
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

/// What a click did, as far as the wire is concerned.
///
/// The slots are reported so the caller can send them, and the cursor comes back
/// because it is not a slot: it is the one thing the player holds that lives
/// nowhere in the window, and it is what a click most often changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Click {
    pub changed: Vec<(usize, Option<ItemStack>)>,
    pub carried: Option<ItemStack>,
    /// What was dropped, for the log. Nothing on the wire until items exist.
    pub dropped: Option<ItemStack>,
}

/// Applies a click the way the game does.
///
/// **The server does not use this any more, and it is not to be used again.**
///
/// The game simulates the click and then corrects the client wherever the two
/// disagree: it is authoritative over every slot, which is a strong guarantee
/// bought with code that has to model crafting grids, armour slots, fuel rules
/// and stack limits -- none of which exist here. It was also wrong three times
/// before it ever ran against a real client.
///
/// What the server does instead is take the client's word for the layout -- the
/// slots that changed, with what is in them now -- and check the totals against
/// what the player was issued. That is [`crate::protocol::item::Allowance`], and
/// the check is one number per item instead of a simulation of a window.
///
/// So this is here as the record of what was replaced, and it is the wrong shape
/// for this server: a server that models every slot has to know what a click
/// means, and this one does not. It is not the authority over the player's
/// pockets here, and nothing in the server calls it.
///
/// Only the twenty-seven carried slots and the nine on the hotbar are live. The
/// crafting result, the four crafting slots, the armour and the off hand are
/// wired up on the client and have nothing behind them here, so a click on one is
/// refused rather than answered from an imaginary inventory -- and refusing it
/// makes the server send its own answer, which empties the slot on the player's
/// screen instead of leaving something there the server has never heard of.
pub fn click(
    inventory: &mut Inventory,
    carried: &mut Option<ItemStack>,
    slot: i16,
    button: u8,
    input: ContainerInput,
) -> Option<Click> {
    if input.is_drag() {
        // A drag arrives in three stages with the cursor's state changing
        // between them, and answering one stage on its own would put items
        // somewhere the next stage contradicts. Say nothing and let the server's
        // view stand; this is a server without recipes and a drag fills slots
        // that have nothing to be filled with.
        return None;
    }
    match input {
        ContainerInput::Pickup => pickup(inventory, carried, slot, button),
        ContainerInput::QuickMove | ContainerInput::PickupAll => quick_move(inventory, slot),
        ContainerInput::Swap => swap(inventory, slot, button),
        ContainerInput::Throw => throw(inventory, carried, slot, button),
        // Middle-click in creative duplicates a stack. There is no creative mode
        // here to duplicate into, and there is no per-player permission to allow
        // it, so it is refused like anything else it cannot answer.
        ContainerInput::Clone => None,
        ContainerInput::QuickCraft | ContainerInput::Unknown(_) => None,
    }
}

/// The twenty-seven and the nine, and nothing else.
fn is_carried_slot(slot: i16) -> Option<usize> {
    let slot = usize::try_from(slot).ok()?;
    (slots::INVENTORY_START..slots::HOTBAR_END)
        .contains(&slot)
        .then_some(slot)
}

/// Left and right click.
///
/// ```java
/// if (clicked.isEmpty()) { ... insert all or one of the carried ... }
/// else if (carried.isEmpty()) { ... take all, or half rounding up ... }
/// else if (slot.mayPlace(carried)) {
///    if (same item) { ... insert all or one ... }
///    else if (carried.getCount() <= slot.getMaxStackSize(carried)) { ... swap ... }
/// }
/// ```
fn pickup(
    inventory: &mut Inventory,
    carried: &mut Option<ItemStack>,
    slot: i16,
    button: u8,
) -> Option<Click> {
    let slot = is_carried_slot(slot)?;
    let primary = button == 0;
    let clicked = inventory.get(slot).unwrap_or(ItemStack::EMPTY);
    let held = carried.unwrap_or(ItemStack::EMPTY);

    if clicked.is_empty() {
        if !held.is_empty() {
            let amount = if primary { held.count } else { 1 }.min(held.count);
            inventory.set(slot, Some(ItemStack::new(held.item, amount)));
            *carried = rest_of(held, amount);
        }
    } else if held.is_empty() {
        // Left takes the lot; right takes half, rounding up so a stack of one
        // still yields something to carry.
        let amount = if primary { clicked.count } else { (clicked.count + 1) / 2 };
        *carried = Some(taken_from(clicked, amount));
        inventory.set(slot, rest_of(clicked, amount));
    } else if clicked.item == held.item {
        let amount = if primary { held.count } else { 1 };
        let room = MAX_STACK - clicked.count;
        let moved = amount.min(room).min(held.count);
        if moved > 0 {
            inventory.set(
                slot,
                Some(ItemStack::new(clicked.item, clicked.count + moved)),
            );
            *carried = rest_of(held, moved);
        }
    } else if held.count <= MAX_STACK {
        // Two different items, and the one in hand is small enough to go where
        // the other is: they change places.
        inventory.set(slot, Some(held));
        *carried = Some(clicked);
    }

    Some(Click {
        changed: vec![(slot, inventory.get(slot))],
        carried: *carried,
        dropped: None,
    })
}

/// Shift-click, and the keyboard form of it.
///
/// A stack in the twenty-seven goes to the nine and a stack in the nine goes to
/// the twenty-seven. Anything that will not fit stays where it is, which is what
/// vanilla does rather than dropping it.
fn quick_move(inventory: &mut Inventory, slot: i16) -> Option<Click> {
    let from = is_carried_slot(slot)?;
    let stack = inventory.get(from)?;
    let (dest_start, dest_end) = if from >= slots::HOTBAR_START {
        (slots::INVENTORY_START, slots::INVENTORY_END)
    } else {
        (slots::HOTBAR_START, slots::HOTBAR_END)
    };

    let mut left = stack;
    let changed = move_item_stack_to(inventory, &mut left, dest_start, dest_end);
    if left.count == stack.count {
        return None; // nowhere to go, so nothing happened
    }
    inventory.set(from, rest_of(stack, stack.count - left.count));
    let mut changed = changed;
    changed.push((from, inventory.get(from)));
    Some(Click { changed, carried: None, dropped: None })
}

/// Moves a stack into a range of slots, the way `moveItemStackTo` does.
///
/// Two passes, and the order matters. Every slot already holding the same item
/// is topped up first, and only what is left over goes into empty ones. Doing it
/// in a single pass over the slots would fill an empty slot one along while a
/// later slot sat half full with the same thing, which puts dirt in one corner
/// and an empty hotbar rather than where a player would have put it.
fn move_item_stack_to(
    inventory: &mut Inventory,
    stack: &mut ItemStack,
    start: usize,
    end: usize,
) -> Vec<(usize, Option<ItemStack>)> {
    let mut changed = Vec::new();
    let mut left = *stack;

    for dest in start..end {
        if left.is_empty() {
            break;
        }
        if let Some(there) = inventory.get(dest)
            && there.item == left.item
            && there.count < MAX_STACK
        {
            let room = MAX_STACK - there.count;
            let moved = room.min(left.count);
            inventory.set(dest, Some(ItemStack::new(there.item, there.count + moved)));
            changed.push((dest, inventory.get(dest)));
            left = taken_from(left, left.count - moved);
        }
    }

    for dest in start..end {
        if left.is_empty() {
            break;
        }
        if inventory.get(dest).is_none() {
            let moved = MAX_STACK.min(left.count);
            inventory.set(dest, Some(ItemStack::new(left.item, moved)));
            changed.push((dest, inventory.get(dest)));
            left = taken_from(left, left.count - moved);
        }
    }

    *stack = left;
    changed
}

/// The number keys, and dragging onto a hotbar slot.
///
/// ```java
/// if (source.isEmpty()) { hotbar = target; target = EMPTY }
/// else if (target.isEmpty()) { if (source.count > maxStack) target = source.split(maxStack)
///                             else { hotbar = EMPTY; target = source } }
/// else { ... swap ... }
/// ```
fn swap(inventory: &mut Inventory, slot: i16, button: u8) -> Option<Click> {
    let to = is_carried_slot(slot)?;
    let from = slots::HOTBAR_START.checked_add(button as usize)?;
    if !(slots::HOTBAR_START..slots::HOTBAR_END).contains(&from) || from == to {
        return None;
    }
    let source = inventory.get(from);
    let target = inventory.get(to);
    if source.is_none() && target.is_none() {
        return None;
    }

    match source {
        None => {
            inventory.set(from, target);
            inventory.set(to, None);
        }
        Some(source) => match target {
            None => {
                // A stack bigger than a slot leaves the rest behind rather than
                // being refused, which is what `source.split(maxStack)` does.
                if source.count > MAX_STACK {
                    inventory.set(to, Some(ItemStack::new(source.item, MAX_STACK)));
                    inventory.set(
                        from,
                        Some(ItemStack::new(source.item, source.count - MAX_STACK)),
                    );
                } else {
                    inventory.set(from, None);
                    inventory.set(to, Some(source));
                }
            }
            Some(target) => {
                inventory.set(from, Some(target));
                inventory.set(to, Some(source));
            }
        },
    }

    Some(Click {
        changed: vec![(from, inventory.get(from)), (to, inventory.get(to))],
        carried: None,
        dropped: None,
    })
}

/// The drop key, and clicking outside the window.
///
/// ```java
/// if (slotIndex == -999) { ... drop all, or one ... }
/// else if (containerInput == THROW && carried.isEmpty()) {
///    int amount = buttonNum == 0 ? 1 : slot.getItem().getCount();
///    ... and for button 1, keep going while the slot holds the same item ...
/// }
/// ```
fn throw(
    inventory: &mut Inventory,
    carried: &mut Option<ItemStack>,
    slot: i16,
    button: u8,
) -> Option<Click> {
    if slot == THROW_SLOT {
        let held = carried.unwrap_or(ItemStack::EMPTY);
        if held.is_empty() {
            return None;
        }
        let dropped = if button == 0 { held } else { ItemStack::new(held.item, 1) };
        *carried = rest_of(held, dropped.count);
        return Some(Click { changed: Vec::new(), carried: *carried, dropped: Some(dropped) });
    }

    let slot = is_carried_slot(slot)?;
    if carried.is_some_and(|s| !s.is_empty()) {
        return None; // the game drops the cursor, not the slot, when both are full
    }
    let stack = inventory.get(slot)?;
    let dropped = if button == 0 {
        ItemStack::new(stack.item, 1)
    } else {
        // All of it, and then any more of the same item the slot holds -- which
        // is how shift-throwing a full slot of gravel empties it.
        stack
    };
    inventory.set(slot, if dropped.count >= stack.count { None } else { rest_of(stack, dropped.count) });
    Some(Click { changed: vec![(slot, inventory.get(slot))], carried: *carried, dropped: Some(dropped) })
}

/// What is left of a stack after `amount` is taken off it.
///
/// `None` once it is empty, because an empty slot and a slot holding nothing are
/// the same thing and the window has only one of them.
fn rest_of(stack: ItemStack, amount: i32) -> Option<ItemStack> {
    let left = stack.count - amount;
    (left > 0).then(|| ItemStack::new(stack.item, left))
}

/// The first `amount` of a stack, which is what picking something up gives you.
fn taken_from(stack: ItemStack, amount: i32) -> ItemStack {
    ItemStack::new(stack.item, amount.clamp(0, stack.count))
}

#[cfg(test)]
mod tests;
