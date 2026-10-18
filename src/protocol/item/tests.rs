use super::*;
use crate::protocol::VarInt;

fn varint(n: i32) -> Vec<u8> {
    let mut out = Vec::new();
    VarInt(n).write(&mut out);
    out
}

#[test]
fn an_empty_stack_is_one_zero() {
    // ItemStack.OPTIONAL_STREAM_CODEC reads a count and returns EMPTY when it is
    // zero or less, so an empty slot is one byte on the wire and everything
    // after it stays in step.
    let mut out = Vec::new();
    ItemStack::EMPTY.write(&mut out);
    assert_eq!(out, vec![0], "and nothing else: {out:?}");

    let read = ItemStack::read(&out).expect("an empty stack reads");
    assert!(read.is_empty());
}

#[test]
fn a_plain_stack_is_a_count_an_id_and_two_zeroes() {
    // The whole reason this module is small enough to exist: the data component
    // patch, which is the part that would have needed a real codec, is two
    // zeroes when there are no components. DataComponentPatch.STREAM_CODEC
    // reads positiveCount and negativeCount and stops at both being zero.
    let mut out = Vec::new();
    ItemStack::new(484, 64).write(&mut out);
    assert_eq!(
        out,
        [0x40, 0xE4, 0x03, 0x00, 0x00],
        "64, dirt at 484, and an empty patch"
    );

    let read = ItemStack::read(&out).expect("a stack reads back");
    assert_eq!(read, ItemStack::new(484, 64));
}

#[test]
fn a_stack_with_components_is_refused_rather_than_half_read() {
    // Skipping the patch would leave everything after it read out of step, and
    // the packet after that would be the one that goes wrong -- somewhere else,
    // much later, in a way nobody connects back to here.
    let mut out = varint(1);
    out.extend(varint(484));
    out.extend(varint(1)); // one component
    out.extend(varint(1)); // ...and one removed

    let err = ItemStack::read(&out).expect_err("a component is not nothing");
    assert!(
        matches!(err, crate::protocol::primitive::DecodeError::UnexpectedEof { .. }),
        "refused, and said why: {err}"
    );
}

#[test]
fn the_slot_layout_is_the_games() {
    // InventoryMenu, in the server source: container 0, the crafting grid at 1
    // to 4, armour at 5 to 8, the twenty-seven carried slots at 9 to 35, the
    // hotbar at 36 to 44 and the off hand at 45. Nothing in this server changes
    // it, which is what makes it safe to write down.
    assert_eq!(slots::CONTAINER_ID, 0);
    assert_eq!(slots::COUNT, 46);
    assert_eq!(slots::RESULT, 0);
    assert_eq!(slots::CRAFT_START, 1);
    assert_eq!(slots::CRAFT_END, 5);
    assert_eq!(slots::ARMOR_START, 5);
    assert_eq!(slots::ARMOR_END, 9);
    assert_eq!(slots::INVENTORY_START, 9);
    assert_eq!(slots::INVENTORY_END, 36);
    assert_eq!(slots::HOTBAR_START, 36);
    assert_eq!(slots::HOTBAR_END, 45);
    assert_eq!(slots::OFF_HAND, 45);

    // They have to tile: a gap is a slot the client expects to hear about and
    // does not, which draws the wrong thing in the wrong place.
    for (start, end) in [
        (slots::CRAFT_START, slots::CRAFT_END),
        (slots::ARMOR_START, slots::ARMOR_END),
        (slots::INVENTORY_START, slots::INVENTORY_END),
        (slots::HOTBAR_START, slots::HOTBAR_END),
    ] {
        assert!(start < end, "every range is a range: {start}..{end}");
    }
    assert_eq!(slots::OFF_HAND + 1, slots::COUNT, "and they end at the last slot");
}

#[test]
fn the_hotbar_is_the_nine_slots_at_the_bottom() {
    // A player's view of this is nine slots under a two by two crafting grid,
    // and the hotbar is at 36 to 44. Getting it wrong puts the blocks they are
    // holding somewhere they cannot see them.
    assert_eq!(slots::HOTBAR_END - slots::HOTBAR_START, 9);
    assert_eq!(slots::INVENTORY_END - slots::INVENTORY_START, 27);
    assert_eq!(slots::INVENTORY_END, slots::HOTBAR_START, "they meet");
}

#[test]
fn taking_the_last_one_out_empties_the_slot() {
    // A stack of zero is not the same thing as an empty slot and the client
    // draws them differently, so the last one has to clear it rather than
    // leave a count of zero behind.
    let mut inv = Inventory::empty();
    inv.set(slots::HOTBAR_START, Some(ItemStack::new(484, 2)));

    let left = inv.take_one(slots::HOTBAR_START);
    assert_eq!(left, Some(ItemStack::new(484, 1)), "one left in the hand");
    assert_eq!(inv.get(slots::HOTBAR_START), Some(ItemStack::new(484, 1)));

    let left = inv.take_one(slots::HOTBAR_START);
    assert_eq!(left, None, "and nothing in the hand, not a stack of zero");
    assert_eq!(inv.get(slots::HOTBAR_START), None, "with the slot emptied");
}

#[test]
fn a_block_broken_twice_stacks_rather_than_filling_two_slots() {
    // This is the whole reason for merging. A player digging down breaks grass
    // over and over, and a hotbar that takes one block per slot fills after
    // nine. The game merges into what is already there.
    let mut inv = Inventory::empty();
    for _ in 0..20 {
        inv.add(ItemStack::new(613, 1));
    }

    let mut used = 0;
    for slot in 0..slots::COUNT {
        if inv.get(slot).is_some() {
            used += 1;
        }
    }
    assert_eq!(used, 1, "twenty blocks in one slot, not twenty slots");
    assert_eq!(
        inv.get(slots::HOTBAR_START).unwrap().count,
        20,
        "and on the hotbar, which is where a picked-up block goes in the game"
    );
}

#[test]
fn adding_a_block_names_the_slot_it_went_into_and_no_other() {
    // This is what the client is told about. A caller that worked it out for
    // itself by scanning for the same item ran a second copy of the rules and
    // the two disagreed: the scan answered slot 0 -- the crafting output, which
    // `add` never writes -- while `add` used the hotbar. The client was told
    // slot 0 had changed when it had not, and the hotbar stayed empty. So the
    // slot comes from `add` and there is nowhere else to get it.
    let mut inv = Inventory::empty();
    let used = inv.add(ItemStack::new(613, 1));

    assert_eq!(
        used,
        vec![slots::HOTBAR_START],
        "one block, one slot, and it is the hotbar"
    );
    assert_eq!(
        inv.get(slots::HOTBAR_START).map(|s| s.count),
        Some(1),
        "and that slot really does hold it"
    );

    // Merging into what is already there names the slot it merged into, once.
    let again = inv.add(ItemStack::new(613, 1));
    assert_eq!(again, vec![slots::HOTBAR_START]);
    assert_eq!(inv.get(slots::HOTBAR_START).unwrap().count, 2);
    assert_eq!(again.len(), 1, "one slot changed, so one slot is reported");

    // A different item goes to its own slot rather than over the first.
    let other = inv.add(ItemStack::new(484, 1));
    assert_eq!(other, vec![slots::HOTBAR_START + 1]);
    assert_ne!(other[0], slots::HOTBAR_START);

    // A stack too big for one slot names every slot it landed in.
    let mut inv = Inventory::empty();
    let split = inv.add(ItemStack::new(1370, 250));
    assert_eq!(
        split.len(),
        3,
        "250 is 99 + 99 + 52, and all three are named"
    );
    assert!(split.windows(2).all(|w| w[0] != w[1]), "and none named twice");
    for slot in &split {
        assert!(
            (slots::HOTBAR_START..slots::HOTBAR_END).contains(slot),
            "slot {slot} is in the hotbar"
        );
        assert!(inv.get(*slot).is_some(), "and really holds stone");
    }
}

#[test]
fn adding_what_fits_nowhere_names_no_slot_at_all() {
    // A full hotbar with nothing to merge into. There is nowhere to put it and
    // nowhere to report, which is different from reporting a slot that did not
    // change -- so the caller can say the block was lost rather than believing
    // it went somewhere.
    let mut inv = Inventory::empty();
    inv.add(ItemStack::new(1370, 990));

    assert_eq!(
        inv.add(ItemStack::new(613, 1)),
        Vec::<usize>::new(),
        "a different item with a full hotbar goes nowhere"
    );
    assert_eq!(
        inv.add(ItemStack::new(613, 1)),
        Vec::<usize>::new(),
        "and stays true on the next attempt"
    );
}

#[test]
fn a_stacked_hotbar_splits_across_the_nine_slots() {
    // The other half of it: merging fills one slot, and the rest has to go
    // somewhere. It goes to the hotbar, which is the part of the window the
    // player is looking at.
    let mut inv = Inventory::empty();
    inv.add(ItemStack::new(1370, 250));

    let on_hotbar = (slots::HOTBAR_START..slots::HOTBAR_END)
        .filter(|&s| inv.get(s).is_some())
        .count();
    // The game caps a stack at 99 -- Item.ABSOLUTE_MAX_STACK_SIZE in the jar --
    // so two hundred and fifty needs three slots and no more.
    assert_eq!(on_hotbar, 3, "three slots, because a stack holds ninety-nine");
    assert_eq!(inv.get(slots::HOTBAR_START).unwrap().count, MAX_STACK);
    assert_eq!(inv.get(slots::HOTBAR_START + 2).unwrap().count, 250 - 99 * 2);
    assert_eq!(
        inv.get(slots::HOTBAR_START + 3),
        None,
        "and no fourth: two hundred and fifty is ninety-nine twice and fifty-two"
    );

    let mut total = 0;
    for slot in 0..slots::COUNT {
        total += inv.get(slot).map_or(0, |s| s.count);
    }
    assert_eq!(total, 250, "and nothing lost");
}

#[test]
fn nothing_is_held_from_an_empty_inventory() {
    let mut inv = Inventory::empty();
    assert_eq!(inv.get(slots::HOTBAR_START), None);
    assert_eq!(inv.take_one(slots::HOTBAR_START), None, "and taking does nothing");
}

#[test]
fn setting_a_slot_that_is_already_what_it_should_be_writes_nothing() {
    // This is what decides whether a slot update goes on the wire. Sending one
    // for every pass would be a packet per player per frame for no change, and
    // the client would redraw what it already has.
    let mut inv = Inventory::empty();
    let block = Some(ItemStack::new(484, 3));

    assert!(inv.set(slots::HOTBAR_START, block), "the first write changes it");
    assert!(!inv.set(slots::HOTBAR_START, block), "the second does not");
    assert!(
        inv.set(slots::HOTBAR_START, None),
        "and emptying a slot that has something in it does change it"
    );
    assert!(
        !inv.set(slots::HOTBAR_START, None),
        "but emptying an empty one does not"
    );
}

#[test]
fn an_empty_stack_written_into_a_slot_is_the_same_as_empty() {
    // count zero is not a thing the client understands, so a caller offering
    // one must end up with an empty slot rather than a stack it cannot draw.
    let mut inv = Inventory::empty();
    inv.set(slots::HOTBAR_START, Some(ItemStack::new(484, 0)));
    assert_eq!(inv.get(slots::HOTBAR_START), None);
}

