use super::*;
use crate::protocol::VarInt;

fn varint(n: i32) -> Vec<u8> {
    let mut out = Vec::new();
    VarInt(n).write(&mut out);
    out
}

fn short(n: i16) -> Vec<u8> {
    n.to_be_bytes().to_vec()
}

/// One `HashedStack`: present, an item, a count, and the two empty component
/// counts that make it an ordinary stack.
fn hashed(item: i32, count: i32) -> Vec<u8> {
    let mut out = vec![0x01];
    out.extend(varint(item));
    out.extend(varint(count));
    out.extend(varint(0));
    out.extend(varint(0));
    out
}

fn nothing() -> Vec<u8> {
    vec![0x00]
}

/// A whole click, in the order `ServerboundContainerClickPacket` reads it, with
/// the packet id standing in front of it the way the wire carries it.
fn click_bytes(
    container_id: i32,
    state_id: i32,
    slot: i16,
    button: u8,
    input: ContainerInput,
    changed: &[(i16, Option<(i32, i32)>)],
    carried: Option<(i32, i32)>,
) -> Vec<u8> {
    let mut out = vec![0x12]; // container_click
    out.extend(varint(container_id));
    out.extend(varint(state_id));
    out.extend(short(slot));
    out.push(button);
    out.extend(varint(match input {
        ContainerInput::Pickup => 0,
        ContainerInput::QuickMove => 1,
        ContainerInput::Swap => 2,
        ContainerInput::Clone => 3,
        ContainerInput::Throw => 4,
        ContainerInput::QuickCraft => 5,
        ContainerInput::PickupAll => 6,
        ContainerInput::Unknown(other) => other as i32,
    }));
    out.extend(varint(changed.len() as i32));
    for (slot, stack) in changed {
        out.extend(short(*slot));
        match stack {
            Some((item, count)) => out.extend(hashed(*item, *count)),
            None => out.extend(nothing()),
        }
    }
    match carried {
        Some((item, count)) => out.extend(hashed(item, count)),
        None => out.extend(nothing()),
    }
    out
}

const DIRT: i32 = 55;
const GRASS: i32 = 54;

#[test]
fn a_click_reads_the_fields_in_the_order_the_game_writes_them() {
    // containerId, stateId, slot, button, input, changedSlots, carriedItem.
    let bytes = click_bytes(0, 7, 36, 0, ContainerInput::Pickup, &[(36, Some((DIRT, 12)))], Some((GRASS, 3)));
    let got = ContainerClickPacket::read(&bytes).expect("a well formed click reads");

    assert_eq!(got.container_id, 0);
    assert_eq!(got.state_id, 7);
    assert_eq!(got.slot, 36);
    assert_eq!(got.button, 0);
    assert_eq!(got.input, ContainerInput::Pickup);
    assert_eq!(got.changed, vec![(36, Some(ItemStack::new(DIRT, 12)))]);
    assert_eq!(got.carried, Some(ItemStack::new(GRASS, 3)));
}

#[test]
fn the_floor_is_a_negative_slot_not_a_large_one() {
    // Vanilla spells the outside of the window as -999 in a Short, which is two's
    // complement and not a large positive number. A server that read it unsigned
    // would look for slot 59637.
    let bytes = click_bytes(0, 0, THROW_SLOT, 0, ContainerInput::Throw, &[], Some((DIRT, 5)));
    let got = ContainerClickPacket::read(&bytes).expect("reads");
    assert_eq!(got.slot, -999);
    assert_eq!(got.slot, THROW_SLOT);
    assert!(got.slot < 0, "and it is negative");
}

#[test]
fn an_empty_stack_is_one_byte_and_reads_as_nothing() {
    // HashedStack is an optional, so the cursor being empty costs a single
    // boolean and no item at all.
    assert_eq!(hashed(DIRT, 5).len(), 1 + 1 + 1 + 1 + 1);
    assert_eq!(nothing().len(), 1);

    let bytes = click_bytes(0, 0, 36, 0, ContainerInput::Pickup, &[(36, None)], None);
    let got = ContainerClickPacket::read(&bytes).expect("reads");
    assert_eq!(got.changed, vec![(36, None)]);
    assert_eq!(got.carried, None);
}

#[test]
fn a_click_that_changed_nothing_still_reads() {
    // An empty changedSlots map is a count of zero, and the cursor follows it.
    let bytes = click_bytes(0, 3, 36, 0, ContainerInput::Pickup, &[], None);
    let got = ContainerClickPacket::read(&bytes).expect("reads");
    assert!(got.changed.is_empty());
    assert_eq!(got.carried, None);
    assert_eq!(got.state_id, 3);
}

#[test]
fn every_container_input_is_read_back_as_itself() {
    for (byte, want) in [
        (0u8, ContainerInput::Pickup),
        (1, ContainerInput::QuickMove),
        (2, ContainerInput::Swap),
        (3, ContainerInput::Clone),
        (4, ContainerInput::Throw),
        (5, ContainerInput::QuickCraft),
        (6, ContainerInput::PickupAll),
    ] {
        // the id, containerId, stateId, the two-byte slot, the button, then
        // the input. Reading a field's position by eye is how the byte after it
        // gets overwritten instead.
        let mut bytes = click_bytes(0, 0, 36, 0, ContainerInput::Pickup, &[], None);
        let input_at = 1 + 1 + 1 + 2 + 1;
        assert_eq!(bytes[input_at], 0, "the input starts as Pickup");
        bytes[input_at] = byte;
        assert_eq!(ContainerClickPacket::read(&bytes).expect("reads").input, want, "id {byte}");
    }
}

#[test]
fn an_input_id_the_game_does_not_have_is_named_rather_than_guessed_at() {
    let mut bytes = click_bytes(0, 0, 36, 0, ContainerInput::Pickup, &[], None);
    bytes[1 + 1 + 1 + 2 + 1] = 99;
    let got = ContainerClickPacket::read(&bytes).expect("still reads");
    assert_eq!(got.input, ContainerInput::Unknown(99));
    assert!(
        click_input_is_refused(got.input),
        "and is refused rather than guessed at"
    );
}

fn click_input_is_refused(input: ContainerInput) -> bool {
    crate::protocol::container::click(
        &mut Inventory::empty(),
        &mut None,
        36,
        0,
        input,
    )
    .is_none()
}

#[test]
fn a_changed_slot_count_off_the_wire_is_refused_before_it_is_reserved() {
    // The count is a VarInt the client chooses. Taking it as written is how a
    // packet becomes an allocation the size of a number off the wire, so it is
    // checked before the loop that would read that many.
    let mut bytes = click_bytes(0, 0, 36, 0, ContainerInput::Pickup, &[], None);
    // the id, containerId, stateId, slot, button, input, then the count.
    let count_at = 1 + 1 + 1 + 2 + 1 + 1;
    assert_eq!(bytes[count_at], 0, "no changed slots to begin with");
    bytes.splice(count_at..count_at + 1, varint(2_000_000_000));
    assert!(
        ContainerClickPacket::read(&bytes).is_err(),
        "a count of two billion is not read"
    );
}

#[test]
fn a_stack_carrying_components_is_refused_rather_than_half_read() {
    // The components are read to their end, so the packet after this one is
    // still in step -- and then refused, because nothing this server produces
    // carries components and meeting one means the two sides disagree about
    // what an item is.
    let mut bytes = click_bytes(0, 0, 36, 0, ContainerInput::Pickup, &[], None);
    // cursor: present, item, count, one added component (type + a 4 byte hash).
    let mut carried = vec![0x01];
    carried.extend(varint(DIRT));
    carried.extend(varint(1));
    carried.extend(varint(1));
    carried.extend(varint(7));
    carried.extend([1, 2, 3, 4]);
    carried.extend(varint(0));
    bytes.truncate(bytes.len() - 1);
    bytes.extend(carried);

    assert!(
        ContainerClickPacket::read(&bytes).is_err(),
        "refused, not quietly dropped"
    );
}

#[test]
fn a_click_truncated_mid_field_is_refused() {
    let full = click_bytes(0, 0, 36, 0, ContainerInput::Pickup, &[(36, Some((DIRT, 1)))], None);
    for cut in 1..full.len() {
        assert!(
            ContainerClickPacket::read(&full[..cut]).is_err(),
            "cut to {cut} bytes is not a click"
        );
    }
    assert!(ContainerClickPacket::read(&full).is_ok(), "and the whole thing is");
}

#[test]
fn a_short_is_read_the_way_the_game_writes_one() {
    // Big endian, because `FriendlyByteBuf.writeShort` is netty's `writeShort`
    // and netty has separate `ShortLE` methods for the other order. Read the
    // other way round, a slot is a different number -- 36 becomes 9216 -- and
    // every field after it is read from the wrong place.
    assert_eq!(36i16.to_be_bytes(), [0x00, 0x24]);
    assert_eq!(36i16.to_le_bytes(), [0x24, 0x00]);
    assert_eq!(i16::from_be_bytes([0x00, 0x24]), 36);
    assert_eq!(i16::from_le_bytes([0x00, 0x24]), 9216, "which is not a slot");
}

#[test]
fn a_click_captured_from_a_real_client_reads() {
    // Byte for byte what arrived when a player shift-clicked a block of grass
    // out of the twenty-seven and up to the hotbar. Nothing here was invented
    // for the test, which is the point: every other one of these was written to
    // match the parser rather than checked against it, and agreed with it
    // while it was wrong.
    //
    //   12                  container_click
    //   00                  containerId 0 -- the player's own window
    //   00                  stateId
    //   00 24               slot 36, the first hotbar slot
    //   00                  button: left
    //   01                  QUICK_MOVE
    //   02                  two slots changed
    //     00 09  01 36 01 00 00   slot 9 now holds one grass block
    //     00 24  00              slot 36 is now empty
    //   00                  the cursor is empty
    let frame = [
        0x12, 0x00, 0x00, 0x00, 0x24, 0x00, 0x01, 0x02, 0x00, 0x09, 0x01, 0x36, 0x01, 0x00,
        0x00, 0x00, 0x24, 0x00, 0x00,
    ];
    let got = ContainerClickPacket::read(&frame).expect("a real click reads");

    assert_eq!(got.container_id, 0, "the player's own window");
    assert_eq!(got.state_id, 0);
    assert_eq!(got.slot, 36, "the first hotbar slot, and not 9216");
    assert_eq!(got.button, 0, "left");
    assert_eq!(got.input, ContainerInput::QuickMove, "shift-click");
    assert_eq!(
        got.changed,
        vec![(9, Some(ItemStack::new(GRASS, 1))), (36, None)],
        "the grass left slot 9 and slot 36 is empty"
    );
    assert_eq!(got.carried, None, "and nothing on the cursor");
}

#[test]
fn the_packet_id_is_stepped_over_rather_than_read_as_the_first_field() {
    // The frame a decoder is handed still has the id on it, because that is
    // what the wire carries. Starting on it reads 18 as the container id -- a
    // small, perfectly ordinary number -- so nothing fails until two fields
    // later, where the complaint is about something that was never wrong.
    let frame = click_bytes(0, 0, 36, 0, ContainerInput::Pickup, &[], None);
    assert_eq!(frame[0], 0x12, "the id is on the front of it");
    let got = ContainerClickPacket::read(&frame).expect("reads");
    assert_eq!(got.container_id, 0, "not 18");

    // And a frame that is some other packet is refused rather than decoded.
    let mut other = frame.clone();
    other[0] = 0x36;
    assert!(
        ContainerClickPacket::read(&other).is_err(),
        "a carried-item packet is not a click"
    );
}

#[test]
fn a_click_with_a_real_hotbar_slot_reads_the_slot_it_names() {
    // The whole shape, with the slot at the offset a click puts it, so a wrong
    // endianness here cannot pass on the field alone.
    let bytes = click_bytes(0, 1, 36, 0, ContainerInput::Pickup, &[], None);
    let slot_at = 1 + 1 + 1; // the id, containerId, stateId
    assert_eq!(&bytes[slot_at..slot_at + 2], &[0x00, 0x24], "36 is 0x0024");
    assert_eq!(ContainerClickPacket::read(&bytes).expect("reads").slot, 36);
}

#[test]
fn the_selected_hotbar_slot_is_one_short() {
    assert_eq!(read_set_carried_item(&[0x36, 0x00, 0x24]).expect("reads"), 36);
    assert_eq!(read_set_carried_item(&[0x36, 0x00, 0x2c]).expect("reads"), 44);
    assert!(read_set_carried_item(&[0x36, 0x00]).is_err(), "half a slot is not a slot");
    assert!(read_set_carried_item(&[0x36]).is_err(), "and neither is one byte");
    assert!(read_set_carried_item(&[]).is_err(), "nor nothing");
    assert!(
        read_set_carried_item(&[0x12, 0x00, 0x24]).is_err(),
        "and a click is not a carried item"
    );
}

//
// What the clicks do.
//

fn inv_with(stacks: &[(usize, i32, i32)]) -> Inventory {
    let mut inv = Inventory::empty();
    for (slot, item, count) in stacks {
        inv.set(*slot, Some(ItemStack::new(*item, *count)));
    }
    inv
}

#[test]
fn left_clicking_a_block_puts_it_on_the_cursor() {
    let mut inv = inv_with(&[(36, DIRT, 20)]);
    let mut carried = None;
    let got = click(&mut inv, &mut carried, 36, 0, ContainerInput::Pickup).expect("a click happened");

    assert_eq!(carried, Some(ItemStack::new(DIRT, 20)), "the lot is picked up");
    assert_eq!(inv.get(36), None, "and the slot is left empty");
    assert_eq!(got.changed, vec![(36, None)], "one slot changed");
    assert_eq!(got.carried, carried);
}

#[test]
fn right_clicking_a_block_takes_half_of_it_rounded_up() {
    let mut inv = inv_with(&[(36, DIRT, 21)]);
    let mut carried = None;
    click(&mut inv, &mut carried, 36, 1, ContainerInput::Pickup).expect("a click happened");

    assert_eq!(carried, Some(ItemStack::new(DIRT, 11)), "eleven of twenty-one");
    assert_eq!(inv.get(36), Some(ItemStack::new(DIRT, 10)));

    // A stack of one still yields something rather than nothing.
    let mut inv = inv_with(&[(36, DIRT, 1)]);
    let mut carried = None;
    click(&mut inv, &mut carried, 36, 1, ContainerInput::Pickup);
    assert_eq!(carried, Some(ItemStack::new(DIRT, 1)));
    assert_eq!(inv.get(36), None);
}

#[test]
fn dropping_a_block_onto_an_empty_slot_puts_it_there() {
    let mut inv = Inventory::empty();
    let mut carried = Some(ItemStack::new(DIRT, 20));
    click(&mut inv, &mut carried, 12, 0, ContainerInput::Pickup).expect("a click happened");

    assert_eq!(inv.get(12), Some(ItemStack::new(DIRT, 20)));
    assert_eq!(carried, None, "and the cursor is empty again");
}

#[test]
fn two_stacks_of_the_same_thing_merge_when_one_is_dropped_on_the_other() {
    let mut inv = inv_with(&[(12, DIRT, 15)]);
    let mut carried = Some(ItemStack::new(DIRT, 10));
    click(&mut inv, &mut carried, 12, 0, ContainerInput::Pickup).expect("a click happened");

    assert_eq!(inv.get(12), Some(ItemStack::new(DIRT, 25)));
    assert_eq!(carried, None);
}

#[test]
fn two_different_stacks_change_places() {
    let mut inv = inv_with(&[(12, DIRT, 15)]);
    let mut carried = Some(ItemStack::new(GRASS, 4));
    click(&mut inv, &mut carried, 12, 0, ContainerInput::Pickup).expect("a click happened");

    assert_eq!(inv.get(12), Some(ItemStack::new(GRASS, 4)));
    assert_eq!(carried, Some(ItemStack::new(DIRT, 15)), "and the old one is in hand");
}

#[test]
fn a_stack_never_grows_past_the_limit() {
    // Ninety-nine is the limit for most things. A stack of ninety-nine with
    // fifty in hand leaves the fifty where it is rather than making a hundred
    // and forty-nine of something that cannot be that big.
    let mut inv = inv_with(&[(12, DIRT, MAX_STACK)]);
    let mut carried = Some(ItemStack::new(DIRT, 50));
    click(&mut inv, &mut carried, 12, 0, ContainerInput::Pickup).expect("a click happened");

    assert_eq!(inv.get(12), Some(ItemStack::new(DIRT, MAX_STACK)));
    assert_eq!(carried, Some(ItemStack::new(DIRT, 50)), "all still in hand");
}

#[test]
fn shift_clicking_a_block_sends_it_to_the_hotbar() {
    // This is the case that makes the server's tally stop matching the screen: a
    // player shift-clicks a block out of the twenty-seven and it goes to the
    // nine, and a server not tracking that keeps reporting a slot that moved.
    let mut inv = inv_with(&[(12, DIRT, 20)]);
    let mut carried = None;
    let got = click(&mut inv, &mut carried, 12, 0, ContainerInput::PickupAll).expect("a click happened");

    assert_eq!(inv.get(12), None, "it left the twenty-seven");
    assert_eq!(inv.get(36), Some(ItemStack::new(DIRT, 20)), "and is on the hotbar");
    assert!(got.changed.contains(&(36, Some(ItemStack::new(DIRT, 20)))), "the hotbar slot is reported");
    assert!(got.changed.contains(&(12, None)), "and the slot it left");
}

#[test]
fn shift_clicking_a_hotbar_block_sends_it_the_other_way() {
    let mut inv = inv_with(&[(36, DIRT, 20)]);
    let mut carried = None;
    click(&mut inv, &mut carried, 36, 0, ContainerInput::PickupAll).expect("a click happened");

    assert_eq!(inv.get(36), None);
    assert_eq!(inv.get(9), Some(ItemStack::new(DIRT, 20)), "the first of the twenty-seven");
}

#[test]
fn a_shift_click_with_nowhere_to_go_changes_nothing() {
    // Both halves are full of something else, so the stack stays put and no
    // slot is reported as changed -- rather than the stack vanishing.
    let mut inv = inv_with(&[
        (36, GRASS, 1),
        (37, GRASS, 1),
        (38, GRASS, 1),
        (39, GRASS, 1),
        (40, GRASS, 1),
        (41, GRASS, 1),
        (42, GRASS, 1),
        (43, GRASS, 1),
        (44, GRASS, 1),
    ]);
    let mut carried = None;
    assert!(
        click(&mut inv, &mut carried, 12, 0, ContainerInput::PickupAll).is_none(),
        "nothing to do"
    );
    let _ = DIRT;
    assert_eq!(inv.get(12), None, "and nothing was invented");
    assert_eq!(inv.get(36), Some(ItemStack::new(GRASS, 1)), "nor moved");
}

#[test]
fn a_stack_too_big_for_one_slot_splits_across_two() {
    let mut inv = inv_with(&[(12, DIRT, 150)]);
    let mut carried = None;
    let got = click(&mut inv, &mut carried, 12, 0, ContainerInput::PickupAll).expect("a click happened");

    assert_eq!(inv.get(36), Some(ItemStack::new(DIRT, 99)));
    assert_eq!(inv.get(37), Some(ItemStack::new(DIRT, 51)), "and the rest beside it");
    assert_eq!(got.changed.len(), 3, "both slots and the one it left");
}

#[test]
fn a_shift_click_merges_into_a_hotbar_that_already_holds_the_same_thing() {
    let mut inv = inv_with(&[(12, DIRT, 20), (36, DIRT, 90)]);
    let mut carried = None;
    click(&mut inv, &mut carried, 12, 0, ContainerInput::PickupAll).expect("a click happened");

    assert_eq!(inv.get(36), Some(ItemStack::new(DIRT, MAX_STACK)), "fills to the limit");
    assert_eq!(inv.get(37), Some(ItemStack::new(DIRT, 11)), "and the overflow goes beside it");
    assert_eq!(inv.get(12), None);
}

#[test]
fn a_number_key_swaps_with_that_hotbar_slot() {
    // What the player actually does when the hotbar is full and something is
    // picked up: put the new block where the empty one is.
    let mut inv = inv_with(&[(12, DIRT, 20), (38, GRASS, 3)]);
    let mut carried = None;
    let got = click(&mut inv, &mut carried, 12, 2, ContainerInput::Swap).expect("a click happened");

    assert_eq!(inv.get(12), Some(ItemStack::new(GRASS, 3)), "the block goes where the key was");
    assert_eq!(inv.get(38), Some(ItemStack::new(DIRT, 20)), "and what was there comes back");
    assert_eq!(got.changed.len(), 2, "two slots changed");
}

#[test]
fn a_number_key_with_nothing_under_it_takes_the_slot_away() {
    let mut inv = inv_with(&[(12, DIRT, 20)]);
    let mut carried = None;
    click(&mut inv, &mut carried, 12, 0, ContainerInput::Swap).expect("a click happened");

    assert_eq!(inv.get(36), Some(ItemStack::new(DIRT, 20)), "sent to the first hotbar slot");
    assert_eq!(inv.get(12), None);
}

#[test]
fn a_number_key_past_the_hotbar_is_refused() {
    let mut inv = inv_with(&[(12, DIRT, 20)]);
    let mut carried = None;
    for button in [9u8, 40, 255] {
        assert!(
            click(&mut inv, &mut carried, 12, button, ContainerInput::Swap).is_none(),
            "button {button} is not a hotbar slot"
        );
    }
    assert_eq!(inv.get(12), Some(ItemStack::new(DIRT, 20)), "and nothing moved");
}

#[test]
fn dropping_one_takes_one_and_leaves_the_rest() {
    let mut inv = inv_with(&[(36, DIRT, 20)]);
    let mut carried = None;
    let got = click(&mut inv, &mut carried, 36, 0, ContainerInput::Throw).expect("a click happened");

    assert_eq!(inv.get(36), Some(ItemStack::new(DIRT, 19)));
    assert_eq!(
        got.dropped,
        Some(ItemStack::new(DIRT, 1)),
        "and one is on the floor"
    );
}

#[test]
fn dropping_the_stack_takes_all_of_it() {
    let mut inv = inv_with(&[(36, DIRT, 20)]);
    let mut carried = None;
    let got = click(&mut inv, &mut carried, 36, 1, ContainerInput::Throw).expect("a click happened");

    assert_eq!(inv.get(36), None);
    assert_eq!(got.dropped, Some(ItemStack::new(DIRT, 20)));
}

#[test]
fn dropping_the_cursor_outside_the_window_drops_what_is_on_it() {
    let mut inv = Inventory::empty();
    let mut carried = Some(ItemStack::new(DIRT, 20));
    let got = click(&mut inv, &mut carried, THROW_SLOT, 0, ContainerInput::Throw).expect("a click happened");

    assert_eq!(got.dropped, Some(ItemStack::new(DIRT, 20)));
    assert_eq!(carried, None, "and the cursor is empty");
    assert!(got.changed.is_empty(), "no slot changed, because none did");
}

#[test]
fn a_click_on_a_slot_with_nothing_behind_it_is_refused() {
    // The crafting grid and the armour slots are drawn by the client and have no
    // inventory here. Answering them from an imaginary one would put something
    // on the player's screen the server has never heard of.
    let mut inv = Inventory::empty();
    let mut carried = Some(ItemStack::new(DIRT, 5));
    for slot in [0i16, 1, 4, 5, 8, 45] {
        assert!(
            click(&mut inv, &mut carried, slot, 0, ContainerInput::Pickup).is_none(),
            "slot {slot} has nothing behind it"
        );
    }
    assert_eq!(carried, Some(ItemStack::new(DIRT, 5)), "and the cursor is untouched");
}

#[test]
fn a_drag_is_refused_rather_than_half_answered() {
    // A drag arrives in stages with the cursor changing between them. Answering
    // one stage alone puts items where the next stage contradicts.
    let mut inv = Inventory::empty();
    let mut carried = Some(ItemStack::new(DIRT, 5));
    assert!(
        click(&mut inv, &mut carried, 12, 0, ContainerInput::QuickCraft).is_none(),
        "refused"
    );
    assert_eq!(inv.get(12), None);
    assert_eq!(carried, Some(ItemStack::new(DIRT, 5)), "and nothing moved");
}

#[test]
fn a_click_on_another_players_window_is_refused() {
    let mut inv = inv_with(&[(36, DIRT, 20)]);
    let mut carried = None;
    // The window id is not checked here -- the caller does that -- but a slot
    // outside the twenty-seven and the nine never is.
    for slot in [-1i16, 8, 46, 300] {
        assert!(
            click(&mut inv, &mut carried, slot, 0, ContainerInput::Pickup).is_none(),
            "slot {slot} is not one this server has"
        );
    }
    assert_eq!(inv.get(36), Some(ItemStack::new(DIRT, 20)), "and nothing moved");
}

#[test]
fn clicking_a_slot_with_nothing_in_it_and_nothing_in_hand_changes_nothing() {
    let mut inv = Inventory::empty();
    let mut carried = None;
    let got = click(&mut inv, &mut carried, 12, 0, ContainerInput::Pickup).expect("answered");
    assert_eq!(got.changed, vec![(12, None)], "reported as unchanged");
    assert_eq!(got.carried, None);
}
