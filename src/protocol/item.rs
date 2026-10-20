//! What a player is carrying, and the wire it travels on.
//!
//! An item is a count, an id into the item registry, and a patch of data
//! components. The patch is the part that would have made this large, and it is
//! two bytes when it is empty:
//!
//! ```text
//! DataComponentPatch.STREAM_CODEC reads positiveCount and negativeCount as
//! VarInts and returns EMPTY when both are zero
//! ```
//!
//! So an ordinary stack -- sixty-four of something, nothing special about it --
//! is a VarInt, a VarInt and two zeroes. Nothing here can represent an enchanted
//! or a renamed item, and that is a deliberate line rather than an oversight:
//! nothing in this server produces one yet, and a codec that reads what it
//! cannot be given a half-written version of is worse than one that does not
//! claim to.
//!
//! Ids come from the same report the block states do, `extracted/registries.json`,
//! where `minecraft:item` gives dirt 55 and grass block 54. They are not block
//! state ids and not the same numbers -- dirt is block state 10.
//!
//! Nor are they the entries' places in that report: it is keyed by identifier,
//! so it arrives alphabetically and its 613th line is light grey stained glass,
//! which is also item 613. The number an item goes by is the entry's own
//! `protocol_id`.


use std::collections::HashMap;

/// What a player is entitled to be holding, by item id.
///
/// The right-hand side of `held <= issued`. It grows when the server gives
/// something and shrinks when it leaves the player's possession -- placed,
/// dropped, eaten, crafted away. Movement between slots never touches it,
/// which is the entire point.
///
/// The server keeps the right-hand side and never has to know the left-hand
/// side as anything more than a sum. That is what makes the check cheap: the
/// server does not simulate what the player does with their things, it only
/// knows how many there are.
#[derive(Clone, Debug, Default)]
pub struct Allowance {
    by_item: HashMap<u32, i32>,
}

impl Allowance {
    pub fn new() -> Self {
        Self::default()
    }

    /// The server gives something.
    pub fn given(&mut self, item: u32, count: i32) {
        if count <= 0 {
            return;
        }
        *self.by_item.entry(item).or_insert(0) += count;
    }

    /// The player loses something: placed down, dropped, eaten.
    ///
    /// The allowance never goes below zero. A player cannot spend what they
    /// were never given, and a server that believes otherwise would be a
    /// server that believes it owes them something.
    pub fn spent(&mut self, item: u32, count: i32) {
        if count <= 0 {
            return;
        }
        let entry = self.by_item.entry(item).or_insert(0);
        *entry -= count;
        if *entry <= 0 {
            self.by_item.remove(&item);
        }
    }

    /// What the player may hold of this item.
    pub fn entitled(&self, item: u32) -> i32 {
        self.by_item.get(&item).copied().unwrap_or(0)
    }

    /// What the player is holding, by item id: every slot and the cursor.
    ///
    /// The cursor is counted because the player is holding it. A thing parked
    /// on the mouse is still theirs, and a check that left it out would be a
    /// check the client could walk around.
    pub fn held_by(&self, inventory: &Inventory, carried: Option<ItemStack>) -> HashMap<u32, i32> {
        let mut totals: HashMap<u32, i32> = HashMap::new();
        for slot in 0..slots::COUNT {
            if let Some(stack) = inventory.get(slot) {
                *totals.entry(stack.item as u32).or_insert(0) += stack.count;
            }
        }
        if let Some(stack) = carried
            && !stack.is_empty()
        {
            *totals.entry(stack.item as u32).or_insert(0) += stack.count;
        }
        totals
    }

    /// Checks the shadow against the allowance and says which slots to correct.
    ///
    /// The slot the client put the surplus in is the one that is emptied: the
    /// surplus came from the client's own report and there is nowhere else for
    /// it to go. What the player were legitimately given is untouched, so the
    /// correction costs a cheater nothing but the thing they made up.
    ///
    /// The cursor is corrected last, and only for what the slots could not
    /// absorb: a stack on the mouse is worth less than a stack in a pocket,
    /// and emptying a pocket over a cursor that is already empty would be
    /// taking something away for nothing.
    pub fn corrections(
        &self,
        inventory: &mut Inventory,
        carried: Option<ItemStack>,
    ) -> (Vec<(usize, Option<ItemStack>)>, Option<ItemStack>) {
        let mut out: Vec<(usize, Option<ItemStack>)> = Vec::new();
        let mut carried = carried;

        let totals = self.held_by(inventory, carried);
        for (item, held) in totals {
            let entitled = self.entitled(item);
            if held <= entitled {
                continue;
            }
            let mut surplus = held - entitled;

            // The slots first, in order, from the end of the hotbar back:
            // the client put the surplus somewhere, and the last thing it
            // touched is the most likely place.
            for slot in (0..slots::COUNT).rev() {
                if surplus <= 0 {
                    break;
                }
                let Some(stack) = inventory.get(slot) else {
                    continue;
                };
                if stack.item as u32 != item {
                    continue;
                }
                let take = surplus.min(stack.count);
                let left = stack.count - take;
                surplus -= take;
                let new = if left > 0 {
                    Some(ItemStack::new(stack.item, left))
                } else {
                    None
                };
                inventory.set(slot, new);
                out.push((slot, new));
            }

            // And then the cursor, for whatever is left.
            if surplus > 0
                && let Some(stack) = carried
                && stack.item as u32 == item
            {
                let take = surplus.min(stack.count);
                let left = stack.count - take;
                carried = if left > 0 {
                    Some(ItemStack::new(stack.item, left))
                } else {
                    None
                };
            }
        }
        (out, carried)
    }
}

/// A stack of one item type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemStack {
    /// Index into the item registry, or [`ItemStack::EMPTY`]'s id for none.
    pub item: i32,
    /// How many. Never zero: a slot with nothing is `None`, not a stack of zero.
    pub count: i32,
}

impl ItemStack {
    pub const EMPTY: ItemStack = ItemStack { item: 0, count: 0 };

    pub fn is_empty(self) -> bool {
        self.count <= 0
    }

    pub fn new(item: i32, count: i32) -> Self {
        ItemStack { item, count }
    }

/// Writes one stack the way ItemStack.OPTIONAL_STREAM_CODEC reads it.
///
/// ```text
/// count = readVarInt(); if (count <= 0) return EMPTY
/// item  = Item.STREAM_CODEC.decode(input)
/// patch = DataComponentPatch.STREAM_CODEC.decode(input)
/// ```
///
/// and Item.STREAM_CODEC is a plain registry index, so the item is a VarInt.
/// The patch is always empty, which is the two zeroes below.
///
/// Returns the bytes written, as every other writer here does.
pub fn write(&self, out: &mut Vec<u8>) -> usize {
        let start = out.len();
        if self.is_empty() {
            crate::protocol::VarInt(0).write(out);
            return out.len() - start;
        }
        crate::protocol::VarInt(self.count).write(out);
        crate::protocol::VarInt(self.item).write(out);
        // positiveCount and negativeCount, both zero: no components.
        out.push(0);
        out.push(0);
        out.len() - start
    }

    /// Reads one stack, for the same codec `write` produces.
    ///
    /// A patch with anything in it is refused rather than skipped: skipping it
    /// would leave the rest of the packet unread, and everything after this one
    /// would be read out of step.
    pub fn read(mut r: &[u8]) -> Result<Self, DecodeError> {
        let count = take_varint(&mut r)?;
        if count <= 0 {
            return Ok(ItemStack::EMPTY);
        }
        let item = take_varint(&mut r)?;
        let positive = take_varint(&mut r)?;
        let negative = take_varint(&mut r)?;
        if positive != 0 || negative != 0 {
            return Err(DecodeError::UnexpectedEof {
                wanted: "an item with no components",
                got: positive.max(negative) as usize,
            });
        }
        Ok(ItemStack { item, count })
    }
}

/// Reads one VarInt and steps past it.
///
/// `VarInt::read` advances the slice itself and returns the length as
/// information, so stepping past it here as well would skip the following field
/// and every one after it.
fn take_varint(r: &mut &[u8]) -> Result<i32, crate::protocol::primitive::DecodeError> {
    let (v, _used) = crate::protocol::VarInt::read(r)?;
    Ok(v.0)
}

/// What could not be read off the wire, which is the same thing every other
/// packet in this module reports.
pub use crate::protocol::primitive::DecodeError;

/// The window a player's own inventory lives in.
///
/// `InventoryMenu.CONTAINER_ID` is zero in the server source, and the layout is
/// fixed there too, which is what makes it safe to hard-code: nothing about this
/// server changes it.
pub mod slots {
    /// The window id the player's inventory is.
    pub const CONTAINER_ID: i32 = 0;

    /// How many slots the window has, and so how many this sends.
    pub const COUNT: usize = 46;

    pub const RESULT: usize = 0;
    pub const CRAFT_START: usize = 1;
    pub const CRAFT_END: usize = 5;
    pub const ARMOR_START: usize = 5;
    pub const ARMOR_END: usize = 9;
    /// The twenty-seven carried slots.
    pub const INVENTORY_START: usize = 9;
    pub const INVENTORY_END: usize = 36;
    /// The nine on the hotbar, which is what a player actually sees.
    pub const HOTBAR_START: usize = 36;
    pub const HOTBAR_END: usize = 45;
    pub const OFF_HAND: usize = 45;
}

/// What one player is carrying.
#[derive(Clone)]
pub struct Inventory {
    slots: Vec<Option<ItemStack>>,
}

impl Inventory {
    pub fn empty() -> Self {
        Inventory {
            slots: vec![None; slots::COUNT],
        }
    }

    pub fn get(&self, slot: usize) -> Option<ItemStack> {
        self.slots.get(slot).copied().flatten()
    }

    /// What changed in a slot, if anything. Used to tell a broadcast from a
    /// silent write, so an empty slot that was already empty does not put a
    /// packet on the wire.
    pub fn set(&mut self, slot: usize, stack: Option<ItemStack>) -> bool {
        let Some(entry) = self.slots.get_mut(slot) else {
            return false;
        };
        if *entry == stack {
            return false;
        }
        *entry = stack.filter(|s| !s.is_empty());
        true
    }

    /// The stack in a slot, minus one, and what is left in it.
    ///
    /// Taking the last one out empties the slot rather than leaving a stack of
    /// zero, which is a different thing and the client draws it differently.
    pub fn take_one(&mut self, slot: usize) -> Option<ItemStack> {
        let held = self.get(slot)?;
        if held.count <= 1 {
            self.slots[slot] = None;
            None
        } else {
            let left = ItemStack::new(held.item, held.count - 1);
            self.slots[slot] = Some(left);
            Some(left)
        }
    }

    /// Adds a stack to the inventory, into the first slot that will take it.
    ///
    /// Merging into what is already there is what the game does and what stops
    /// a hotbar filling up one slot at a time when a player breaks twenty
    /// blocks. What does not fit is dropped: nothing in this server has anywhere to
    /// put a dropped stack, and inventing somewhere to put it would be deciding
    /// what happens on overflow before it can happen.
    ///
    /// Returns every slot it changed, so the caller can be told about exactly
    /// those. A caller that worked this out for itself by scanning for the same
    /// item would be running a second, quieter copy of the rules below, and the
    /// two drift -- which is what happened: the scan answered slot 0, `add` used
    /// the hotbar, and the client was told slot 0 had changed, which it had not.
    pub fn add(&mut self, mut stack: ItemStack) -> Vec<usize> {
        let mut changed = Vec::new();
        if stack.is_empty() {
            return changed;
        }
        for slot in 0..slots::COUNT {
            if let Some(held) = self.slots[slot]
                && held.item == stack.item
                && held.count < MAX_STACK
            {
                let room = MAX_STACK - held.count;
                let moved = room.min(stack.count);
                self.slots[slot] = Some(ItemStack::new(held.item, held.count + moved));
                changed.push(slot);
                if moved == stack.count {
                    return changed;
                }
                stack.count -= moved;
            }
        }
        for slot in slots::HOTBAR_START..slots::HOTBAR_END {
            if self.slots[slot].is_none() {
                let taken = stack.count.min(MAX_STACK);
                self.slots[slot] = Some(ItemStack::new(stack.item, taken));
                changed.push(slot);
                stack.count -= taken;
                if stack.count <= 0 {
                    return changed;
                }
            }
        }
        // Anything still here has nowhere to go. See the note on `add`.
        changed
    }


    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }
}

/// Item.ABSOLUTE_MAX_STACK_SIZE in the client jar. The game caps at 99.
pub const MAX_STACK: i32 = 99;

/// Writes `container_set_content`: every slot of the window at once.
///
/// This is what a player is sent when they join, so that what they see in their
/// hand is the server's idea of it rather than their own. The client keeps its
/// own copy and shows it immediately, which is why a server that never sends
/// this shows an empty hand no matter what the player picked up.
pub fn write_container_set_content(out: &mut Vec<u8>, inventory: &Inventory, state_id: i32) {
    crate::protocol::VarInt(crate::protocol::play::id::CONTAINER_SET_CONTENT).write(out);
    crate::protocol::VarInt(slots::CONTAINER_ID).write(out);
    crate::protocol::VarInt(state_id).write(out);

    // A VarInt of how many follow, then that many stacks. An empty slot is the
    // single zero VarInt, not a stack with no count.
    crate::protocol::VarInt(slots::COUNT as i32).write(out);
    for slot in inventory.slots() {
        match slot {
            Some(stack) => { stack.write(out); }
            None => { crate::protocol::VarInt(0).write(out); }
        }
    }

    // The carried item, the one held by the cursor when a screen is open. There
    // is no screen, so there is nothing on the cursor.
    crate::protocol::VarInt(0).write(out);
}

/// Writes `container_set_slot`: one slot, changed.
///
/// `state_id` is the menu's own identifier, which the client uses to tell a
/// change it asked for from one it did not. The inventory passes the player's
/// position, the same number vanilla does, so a change the client made itself
/// is not argued with.
pub fn write_container_set_slot(
    out: &mut Vec<u8>,
    slot: usize,
    stack: Option<ItemStack>,
    state_id: i32,
) {
    crate::protocol::VarInt(crate::protocol::play::id::CONTAINER_SET_SLOT).write(out);
    crate::protocol::VarInt(slots::CONTAINER_ID).write(out);
    crate::protocol::VarInt(state_id).write(out);
    out.extend_from_slice(&(slot as i16).to_be_bytes());
    match stack {
        Some(stack) => { stack.write(out); }
        None => { crate::protocol::VarInt(0).write(out); }
    }
}

/// A right-click on a block face: `ServerboundUseItemOnPacket`.
///
/// Nine fields, and the three that are not obvious are the ones that matter.
/// The click point is three floats *relative to the corner of the block*, not
/// absolute -- the client sends the offset and the server puts the corner back,
/// so the value is only useful as an answer to "was the click inside the block
/// they named". And the sequence is the prediction number again, the same one
/// block_changed_ack settles, so this packet needs acknowledging even when the
/// placement is refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UseItemOn {
    /// Which hand. Zero is the main one.
    pub hand: i32,
    /// The block that was clicked.
    pub pos: crate::world::pos::BlockPos,
    /// Which face. The block goes on the other side of it.
    pub face: u8,
    /// Where on that face, from 0 to 1 on each axis.
    pub click: [f32; 3],
    /// Whether the click was inside the block rather than on a face.
    pub inside: bool,
    /// Whether it was the world border that was hit.
    pub world_border: bool,
    /// The client's prediction number, to be handed back in an ack.
    pub sequence: i32,
}

impl UseItemOn {
    pub fn read(frame: &[u8]) -> Result<Self, DecodeError> {
        let mut r =
            crate::protocol::play::serverbound_body(frame, crate::protocol::play::id::SERVERBOUND_USE_ITEM_ON)?;


        let hand = take_varint(&mut r)?;
        // Eight bytes: the block position, the same packed long block_update
        // carries and unpack_block_pos puts back together.
        if r.len() < 8 {
            return Err(DecodeError::UnexpectedEof {
                wanted: "a block position",
                got: r.len(),
            });
        }
        let packed = i64::from_be_bytes(r[..8].try_into().expect("eight bytes"));
        r = &r[8..];

        let face = take_varint(&mut r)? as u8;
        let mut click = [0.0f32; 3];
        for slot in &mut click {
            if r.len() < 4 {
                return Err(DecodeError::UnexpectedEof {
                    wanted: "a click position",
                    got: r.len(),
                });
            }
            *slot = f32::from_be_bytes(r[..4].try_into().expect("four bytes"));
            r = &r[4..];
        }
        let inside = take_bool(&mut r)?;
        let world_border = take_bool(&mut r)?;
        let sequence = take_varint(&mut r)?;

        let (x, y, z) = crate::protocol::play::unpack_block_pos(packed);
        Ok(UseItemOn {
            hand,
            pos: crate::world::pos::BlockPos::new(x, y, z),
            face,
            click,
            inside,
            world_border,
            sequence,
        })
    }

    /// Whether the click was on the face of the block it named.
    ///
    /// The server checks this before it places anything: the click point arrives
    /// relative to the corner of the block, so a value outside zero to one is a
    /// client claiming to have clicked something it could not have reached.
    /// ServerGamePacketListenerImpl compares each axis against the middle of
    /// the block with a slack of 1.0000001, which is this with the offset turned
    /// into a distance from the centre.
    pub fn click_is_inside_the_face(&self) -> bool {
        const SLACK: f32 = 1.0000001;
        (self.click[0] - 0.5).abs() < SLACK
            && (self.click[1] - 0.5).abs() < SLACK
            && (self.click[2] - 0.5).abs() < SLACK
    }
}

fn take_bool(r: &mut &[u8]) -> Result<bool, DecodeError> {
    let Some((&byte, rest)) = r.split_first() else {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a boolean",
            got: r.len(),
        });
    };
    *r = rest;
    Ok(byte != 0)
}
#[cfg(test)]
mod tests;

