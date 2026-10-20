use super::*;

/// Reads a VarInt back out of a body, for tests that care about what a
/// packet actually says rather than that it was produced.
fn take_varint(b: &mut &[u8]) -> i32 {
    let mut value = 0i32;
    let mut shift = 0;
    loop {
        let byte = b[0];
        *b = &b[1..];
        value |= ((byte & 0x7f) as i32) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

#[test]
fn the_news_of_a_death_is_a_player_and_a_message() {
    // The client does not decide on its own that a health of zero means death:
    // it is told, and this is the telling. The message is a component, and a
    // component in a packet is one NBT tag -- a plain text one is a string tag.
    let mut body = Vec::new();
    write_player_combat_kill(&mut body, 7, "You died!");

    let mut r = &body[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("a packet id");
    assert_eq!(packet, id::PLAYER_COMBAT_KILL);
    let (VarInt(player), _) = VarInt::read(&mut r).expect("a player id");
    assert_eq!(player, 7);
    assert_eq!(r[0], 0x08, "a string tag");
    let len = u16::from_be_bytes([r[1], r[2]]) as usize;
    assert_eq!(&r[3..3 + len], b"You died!");
    assert_eq!(r.len(), 3 + len, "and nothing after the message");
}

#[test]
fn the_pong_carries_the_ping_back_unchanged() {
    // The client pings every tick and works the round trip out of the
    // difference between the two. The payload is a fixed long on both sides,
    // so a VarLong here would be one byte and the client would read the
    // rest of the packet as part of it.
    let mut body = Vec::new();
    write_pong_response(&mut body, 0x0102_0304_0506_0708);

    let mut rest = &body[..];
    let id = take_varint(&mut rest);
    assert_eq!(id, id::PONG_RESPONSE);
    assert_eq!(rest.len(), 8, "eight bytes of long and nothing else");
    assert_eq!(
        i64::from_be_bytes(rest.try_into().unwrap()),
        0x0102_0304_0506_0708
    );
}

#[test]
fn a_sound_is_a_registry_id_a_source_a_place_and_two_numbers() {
    // The order the game writes: the sound, the source, the place it comes
    // from, how loud, how high pitched, and a seed.
    let mut body = Vec::new();
    write_sound(
        &mut body,
        &Sound {
            id: sound::PLAYER_HURT,
            source: sound::SOURCE_PLAYER,
            x: 1,
            y: 2,
            z: 3,
            volume: 0.5,
            pitch: 1.0,
            seed: 0,
        },
    );

    let mut r = &body[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("a packet id");
    assert_eq!(packet, id::SOUND);
    let (VarInt(which), _) = VarInt::read(&mut r).expect("the sound");
    assert_eq!(which, sound::PLAYER_HURT);
    let (VarInt(source), _) = VarInt::read(&mut r).expect("the source");
    assert_eq!(source, sound::SOURCE_PLAYER);
    assert_eq!(i32::from_be_bytes(r[..4].try_into().unwrap()), 1, "x");
    assert_eq!(i32::from_be_bytes(r[4..8].try_into().unwrap()), 2, "y");
    assert_eq!(i32::from_be_bytes(r[8..12].try_into().unwrap()), 3, "z");
    assert_eq!(f32::from_be_bytes(r[12..16].try_into().unwrap()), 0.5, "the volume");
    assert_eq!(f32::from_be_bytes(r[16..20].try_into().unwrap()), 1.0, "the pitch");
    assert_eq!(i64::from_be_bytes(r[20..28].try_into().unwrap()), 0, "the seed");
    assert_eq!(r.len(), 28, "and nothing after it");
}

#[test]
fn a_line_of_chat_is_a_tag_and_a_byte() {
    // The content of a chat line is a component, and a component in a packet
    // is one NBT tag with no name in front of it. A plain text one is a
    // string tag: the id, the byte count, the bytes. The last byte says the
    // line is not an overlay.
    let mut body = Vec::new();
    write_system_chat(&mut body, "hi");

    let mut r = &body[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("a packet id");
    assert_eq!(packet, id::SYSTEM_CHAT);

    assert_eq!(r[0], 0x08, "a string tag");
    let len = u16::from_be_bytes([r[1], r[2]]) as usize;
    assert_eq!(&r[3..3 + len], b"hi");
    assert_eq!(r[3 + len], 0, "not an overlay");
    assert_eq!(r.len(), 4 + len, "and nothing else");
}

#[test]
fn the_hearts_are_a_float_a_number_and_a_float() {
    // The order is the game's: health, then food, then saturation. Reading it
    // in another order puts the hunger bar at half a heart.
    let mut body = Vec::new();
    write_set_health(&mut body, 20.0, 20, 5.0);

    let mut r = &body[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("a packet id");
    assert_eq!(packet, id::SET_HEALTH);
    assert_eq!(f32::from_be_bytes(r[..4].try_into().unwrap()), 20.0);
    r = &r[4..];
    let (VarInt(food), _) = VarInt::read(&mut r).expect("the food");
    assert_eq!(food, 20);
    assert_eq!(f32::from_be_bytes(r[..4].try_into().unwrap()), 5.0);
    assert_eq!(r.len(), 4, "and nothing after the saturation");
}

#[test]
fn the_experience_is_progress_level_and_total() {
    // The game writes the progress, then the level, then the total, even
    // though it reads them back in the other order.
    let mut body = Vec::new();
    write_set_experience(&mut body, 0.5, 3, 120);

    let mut r = &body[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("a packet id");
    assert_eq!(packet, id::SET_EXPERIENCE);
    assert_eq!(f32::from_be_bytes(r[..4].try_into().unwrap()), 0.5);
    r = &r[4..];
    let (VarInt(level), _) = VarInt::read(&mut r).expect("the level");
    assert_eq!(level, 3);
    let (VarInt(total), _) = VarInt::read(&mut r).expect("the total");
    assert_eq!(total, 120);
}

#[test]
fn a_respawn_carries_the_spawn_information_and_what_to_keep() {
    // The tail is the same as the login packet's, and the byte after it says
    // what the client keeps: three is all of it.
    let mut body = Vec::new();
    write_respawn(
        &mut body,
        &SpawnInfo {
            dimension: "minecraft:overworld",
            seed: 7,
            game_type: 0,
            is_flat: true,
            portal_cooldown: 0,
            sea_level: -64,
        },
        true,
    );

    let mut r = &body[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("a packet id");
    assert_eq!(packet, id::RESPAWN);
    // The dimension holder: a zero byte, then the key.
    assert_eq!(r[0], 0, "the holder is sent by key");
    let (VarInt(len), _) = VarInt::read(&mut &r[1..]).expect("the key length");
    assert_eq!(&r[2..2 + len as usize], b"minecraft:overworld");
    assert_eq!(
        *r.last().expect("a last byte"),
        3,
        "and the client keeps everything"
    );
}

#[test]
fn a_hurt_flash_is_the_entity_and_the_direction() {
    let mut body = Vec::new();
    write_hurt_animation(&mut body, 42, 90.0);

    let mut r = &body[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("a packet id");
    assert_eq!(packet, id::HURT_ANIMATION);
    let (VarInt(entity), _) = VarInt::read(&mut r).expect("an entity id");
    assert_eq!(entity, 42);
    assert_eq!(f32::from_be_bytes(r[..4].try_into().unwrap()), 90.0);
}

#[test]
fn an_angle_is_a_two_hundred_and_fifty_sixth_of_a_turn() {    assert_eq!(angle_byte(0.0), 0);
    assert_eq!(angle_byte(90.0), 64);
    assert_eq!(angle_byte(180.0), 128);
    assert_eq!(angle_byte(-90.0), 192);
    assert_eq!(angle_byte(360.0), 0, "a full turn is back to the start");
    // It rounds rather than truncates, or every face sits one step below
    // where it should be.
    assert_eq!(angle_byte(1.40625), 1, "one step is 1.40625 degrees");
}

#[test]
fn the_player_entity_type_is_the_one_from_the_registry() {
    // Off by one here and the client tries to spawn whatever is next in the
    // registry, or nothing at all.
    assert_eq!(PLAYER_ENTITY_TYPE, 159);
}

#[test]
fn the_tab_list_entry_says_the_four_things_a_client_needs() {
    let mut body = Vec::new();
    write_player_info_add(&mut body, [3u8; 16], "Steve");

    let mut b = &body[..];
    assert_eq!(take_varint(&mut b), id::PLAYER_INFO_UPDATE);

    // One byte, not a VarInt. Eight actions, so ceil(8/8) is one, and a
    // VarInt here reads the entry count out of the action bits.
    let mask = b[0];
    b = &b[1..];
    assert_eq!(mask & (1 << 0), 1 << 0, "ADD_PLAYER");
    assert_eq!(mask & (1 << 2), 1 << 2, "UPDATE_GAME_MODE");
    assert_eq!(mask & (1 << 3), 1 << 3, "UPDATE_LISTED");
    assert_eq!(mask & (1 << 4), 1 << 4, "UPDATE_LATENCY");
    assert_eq!(mask, 0b0001_1101, "and nothing else is set");

    assert_eq!(take_varint(&mut b), 1, "one entry");

    // The uuid comes first and is sixteen bytes. It is not an action and it
    // is not the entity id, and putting an entity id here is sixteen wrong
    // bytes that end the connection.
    assert_eq!(&b[..16], &[3u8; 16][..], "the profile uuid");
    b = &b[16..];

    // Then the action fields, in enum order.
    assert_eq!(take_varint(&mut b), 5, "the name is five bytes");
    assert_eq!(&b[..5], b"Steve");
    b = &b[5..];
    assert_eq!(take_varint(&mut b), 0, "an offline profile owns no properties");
    assert_eq!(take_varint(&mut b), 0, "survival");
    assert_eq!(b[0], 1, "and listed, or the tab list does not show them");
    b = &b[1..];
    assert_eq!(take_varint(&mut b), 0, "no latency");
    assert!(b.is_empty(), "and nothing else, or the client reads past it");
}

/// Decodes a `player_info_update` the way the game does, and returns how
/// many bytes it got through.
///
/// Written as a decoder rather than as more assertions on the writer
/// because both bugs that reached a client were of the shape "the writer
/// produced something well formed and the wrong thing". Checking the bytes
/// a writer emits confirms it emitted the bytes its author believed in.
/// Running the reader is the only check that agrees with the client.
fn decode_player_info_as_client(body: &[u8]) -> usize {
    let mut b = body;
    assert_eq!(take_varint(&mut b), id::PLAYER_INFO_UPDATE);

    // readEnumSet: a fixed width bit set of one byte, for eight actions.
    let mask = b[0];
    b = &b[1..];

    let entries = take_varint(&mut b) as usize;

    for _ in 0..entries {
        // The profile uuid, before any action.
        assert!(b.len() >= 16, "a uuid was expected");
        b = &b[16..];

        // Then the fields of the set actions, in ordinal order.
        if mask & (1 << action::ADD_PLAYER) != 0 {
            let len = take_varint(&mut b) as usize;
            assert!(b.len() >= len, "a name was expected");
            std::str::from_utf8(&b[..len]).expect("the name is utf-8");
            assert!(len <= 48, "the client refuses a longer one: {len}");
            b = &b[len..];

            let properties = take_varint(&mut b) as usize;
            for _ in 0..properties {
                let klen = take_varint(&mut b) as usize;
                b = &b[klen..];
                let vlen = take_varint(&mut b) as usize;
                b = &b[vlen..];
            }
        }
        if mask & (1 << 1) != 0 {
            // A nullable chat session: one byte of presence.
            let present = b[0];
            b = &b[1..];
            assert_eq!(present, 0, "an offline player has no chat session");
        }
        if mask & (1 << action::UPDATE_GAME_MODE) != 0 {
            let _ = take_varint(&mut b);
        }
        if mask & (1 << action::UPDATE_LISTED) != 0 {
            b = &b[1..];
        }
        if mask & (1 << action::UPDATE_LATENCY) != 0 {
            let _ = take_varint(&mut b);
        }
    }

    body.len() - b.len()
}

#[test]
fn the_client_reads_the_tab_list_packet_and_lands_exactly_at_the_end() {
    for name in ["Steve", "Tiến", "a", "noverisp3"] {
        let mut body = Vec::new();
        write_player_info_add(&mut body, [7u8; 16], name);
        let read = decode_player_info_as_client(&body);
        assert_eq!(
            read,
            body.len(),
            "the reader stopped {read} bytes in on {name:?}, or ran past the end"
        );
    }
}

#[test]
fn a_two_byte_mask_is_what_crashed_two_clients() {
    // Kept as a test rather than a comment because it is the shape of the
    // mistake: a VarInt bitmask in place of the fixed width bit set shifts
    // every field after it by a byte, and the first field to notice is the
    // name, which is read out of the middle of a uuid.
    let mut body = Vec::new();
    VarInt(id::PLAYER_INFO_UPDATE).write(&mut body);
    VarInt((1 << 0) | (1 << 6) | (1 << 8)).write(&mut body); // two bytes
    VarInt(1).write(&mut body);
    body.extend_from_slice(&[3u8; 16]);
    VarInt(5).write(&mut body);
    body.extend_from_slice(b"Steve");

    let mut b = &body[..];
    take_varint(&mut b);
    take_varint(&mut b);
    take_varint(&mut b);
    // The count came out of the high byte of the mask.
    assert_ne!(
        take_varint(&mut b),
        1,
        "which is the entry count, and that is the whole failure"
    );
}

#[test]
fn a_delta_too_big_to_carry_is_refused_rather_than_truncated() {
    // Three shorts at 1/4096 a block is a little under eight blocks. Past
    // that the movement cannot be expressed, and writing a clamped one
    // would be worse than writing none: the body would move a plausible
    // wrong amount and stay there, because every later delta is measured
    // from where the client now thinks it is.
    let mut body = Vec::new();
    assert!(
        write_move_entity_pos_rot(&mut body, 1, (0.0, 0.0, 0.0), 0.0, 0.0, true),
        "a movement that fits is written"
    );
    let before = body.len();
    assert!(
        !write_move_entity_pos_rot(&mut body, 1, (0.0, -40.0, 0.0), 0.0, 0.0, true),
        "and a forty block fall is not, which is what a player arriving \
         from forty blocks up used to send"
    );
    assert_eq!(
        body.len(),
        before,
        "half a packet is worse than none: the client would read the missing \
         entity id out of whatever followed"
    );
}

#[test]
fn the_largest_delta_a_short_can_carry_is_about_eight_blocks() {
    // Three shorts of 1/4096 a block. Not a round eight, because the shorts
    // stop one short of it, and the number is the threshold that decides
    // when a movement has to be sent as a position instead.
    let computed = 32767.0 / DELTA_SCALE;
    assert!(
        (computed - MAX_DELTA_BLOCKS).abs() < f64::EPSILON,
        "got {computed} against {MAX_DELTA_BLOCKS}"
    );
    assert!(
        computed > 7.9 && computed < 8.0,
        "and it really is just under eight"
    );
}

#[test]
fn a_player_spawns_on_the_ground_and_not_forty_blocks_above_it() {
    // The old constant was 64.0 and this terrain's surface is -16..24, so
    // every player fell on arrival.
    let (x, y, z) = crate::world::terrain::spawn_point();
    let surface = crate::world::terrain::surface_y(x, z);
    assert_eq!(y, surface as f64 + 1.0, "feet are one above the top solid block");
    assert!(surface < 64, "and nowhere near where the old constant put them");
}

#[test]
fn the_action_ordinals_are_the_ones_the_game_uses() {
    // Read out of the enum, not inferred. Guessing these is what put a
    // two-byte VarInt where one byte belongs. The whole enum, so that a
    // future version adding a ninth is visible here:
    //
    //     0 ADD_PLAYER      4 UPDATE_LATENCY
    //     1 INITIALIZE_CHAT 5 UPDATE_DISPLAY_NAME
    //     2 UPDATE_GAME_MODE 6 UPDATE_LIST_ORDER
    //     3 UPDATE_LISTED   7 UPDATE_HAT
    //
    assert_eq!(action::ADD_PLAYER, 0);
    assert_eq!(action::UPDATE_GAME_MODE, 2);
    assert_eq!(action::UPDATE_LISTED, 3);
    assert_eq!(action::UPDATE_LATENCY, 4);
    assert_eq!(8usize.div_ceil(8), 1, "eight actions, so the mask is one byte");
}

#[test]
fn taking_a_player_off_the_tab_list_is_by_uuid() {
    let mut body = Vec::new();
    write_player_info_remove(&mut body, &[[1u8; 16], [2u8; 16]]);

    let mut b = &body[..];
    assert_eq!(take_varint(&mut b), id::PLAYER_INFO_REMOVE);
    assert_eq!(take_varint(&mut b), 2);
    assert_eq!(&b[..16], &[1u8; 16][..]);
    assert_eq!(&b[16..32], &[2u8; 16][..]);
    assert!(b[32..].is_empty());
}

#[test]
fn a_name_is_measured_in_bytes_and_not_in_characters() {
    let mut body = Vec::new();
    write_player_info_add(&mut body, [0u8; 16], "Tiến");
    let mut b = &body[..];
    // Packet id, the one-byte mask, and the entry count, then the uuid.
    for _ in 0..2 {
        take_varint(&mut b);
    }
    b = &b[1..];
    b = &b[16..];
    // Four characters, six bytes of UTF-8.
    assert_eq!(take_varint(&mut b), 6);
}

/// Decodes an `add_entity` the way the game does, and reports whether it
/// landed exactly at the end.
/// Reads it the way the client does, and returns where it stopped along with the
/// three angles under the names the client gives them.
///
/// The same reasoning as the tab list: a writer can be well formed and
/// wrong, and only running the reader agrees with the client. This one is
/// here because a delta written as a packed long is eight bytes too long,
/// and the client says so by name.
///
/// The angles come back rather than being skipped. A read-back test that steps
/// over them proves there are three of them and says nothing about which is
/// which, which is exactly how a pair written the wrong way round passes.
fn decode_add_entity_as_client(body: &[u8]) -> (usize, (u8, u8, u8)) {
    let mut b = body;
    assert_eq!(take_varint(&mut b), id::ADD_ENTITY);
    let _entity_id = take_varint(&mut b);

    assert!(b.len() >= 16, "a uuid was expected");
    b = &b[16..];

    let _type = take_varint(&mut b);

    for _ in 0..3 {
        assert!(b.len() >= 8, "three doubles were expected");
        b = &b[8..];
    }

    // The delta: a zero is one byte, and reading it as a long is what
    // leaves eight bytes over at the end of the packet.
    let first = b[0];
    b = &b[1..];
    if first != 0 {
        assert!(b.len() >= 5, "a six byte delta was expected");
        b = &b[5..];
        // A partial scale follows, and this writer never produces one.
    }

    // The three angles, read by name and in the game's order. Skipping them
    // without reading them is what let the first two be written the wrong way
    // round for as long as this test existed.
    assert!(b.len() >= 3, "three angles were expected");
    let x_rot = b[0];
    let y_rot = b[1];
    let y_head_rot = b[2];
    b = &b[3..];

    let _data = take_varint(&mut b);
    (body.len() - b.len(), (x_rot, y_rot, y_head_rot))
}

#[test]
fn the_client_reads_add_entity_and_lands_exactly_at_the_end() {
    let mut body = Vec::new();
    write_add_entity(
        &mut body,
        7,
        [9u8; 16],
        PLAYER_ENTITY_TYPE,
        (1.5, 64.0, -2.5),
        90.0,
        0.0,
    );
    let (read, _) = decode_add_entity_as_client(&body);
    assert_eq!(
        read,
        body.len(),
        "the reader stopped {read} bytes in, or the packet is {} bytes too long",
        body.len() - read
    );
}

#[test]
fn add_entity_puts_the_pitch_where_the_pitch_goes() {
    // The symptom, which is what made this worth fixing: another player arrives
    // facing wherever the server thought they were looking, with their head
    // stuck at whatever a fresh head defaults to, and the whole thing corrects
    // itself the moment they move. One step fixes it because the movement packet
    // has the pair the right way round and this one did not.
    //
    // Two different angles, chosen so that swapping them cannot go unnoticed.
    let mut body = Vec::new();
    write_add_entity(
        &mut body,
        7,
        [9u8; 16],
        PLAYER_ENTITY_TYPE,
        (0.0, 64.0, 0.0),
        90.0,  // facing east
        45.0,  // looking forty-five degrees down
    );
    let (read, (x_rot, y_rot, y_head_rot)) = decode_add_entity_as_client(&body);
    assert_eq!(read, body.len(), "and the packet is still well formed");

    let unpack = |b: u8| (b as f32 / 256.0) * 360.0;
    assert!(
        (unpack(x_rot) - 45.0).abs() < 2.25,
        "xRot is the pitch: got {}, wanted 45",
        unpack(x_rot)
    );
    assert!(
        (unpack(y_rot) - 90.0).abs() < 2.25,
        "yRot is the yaw: got {}, wanted 90",
        unpack(y_rot)
    );
    assert!(
        (unpack(y_head_rot) - 90.0).abs() < 2.25,
        "and the head follows the body: got {}",
        unpack(y_head_rot)
    );
    assert_ne!(
        x_rot, y_rot,
        "the two are different, so a swap could not pass unnoticed"
    );
}

#[test]
fn an_entity_is_exactly_the_fields_the_game_reads() {
    // This exists because the read-back test passed with four angles.
    //
    // A round trip proves the writer and the reader agree. It cannot prove
    // they agree with the *game*, because a wrong reading written down
    // twice is still self-consistent: both halves believed there were four
    // angles and neither of them was wrong on its own terms. Counting the
    // reads in the constructor is the only check that consults the game.
    //
    // Upstream, in order:
    //
    //   readVarInt    id
    //   readUUID      uuid
    //   registry      type
    //   readDouble    x, y, z
    //   LP delta      movement
    //   readByte      xRot, yRot, yHeadRot      <- three, not four
    //   readVarInt    data
    //
    const READS: usize = 1 + 1 + 1 + 3 + 1 + 3 + 1;
    assert_eq!(READS, 11, "and the angles are the three in the middle");

    let mut body = Vec::new();
    write_add_entity(
        &mut body,
        7,
        [9u8; 16],
        PLAYER_ENTITY_TYPE,
        (0.0, 64.0, 0.0),
        0.0,
        0.0,
    );
    // The registry id is 159, and 159 does not fit in one VarInt byte. It
    // is two, which is worth being explicit about: it is the one field here
    // whose length is not visible from the number alone.
    let mut reg = Vec::new();
    VarInt(PLAYER_ENTITY_TYPE).write(&mut reg);
    assert_eq!(reg.len(), 2, "159 is a two byte VarInt");

    // Two one-byte VarInts, a uuid, a two-byte registry id, three doubles,
    // a one-byte delta, three angles, a one-byte data value.
    let expected = 2 + 16 + 2 + 24 + 1 + 3 + 1;
    assert_eq!(
        body.len(),
        expected,
        "the packet should be {expected} bytes and is {}",
        body.len()
    );
    assert_eq!(
        decode_add_entity_as_client(&body).0,
        body.len(),
        "and the game's reader gets through all of it"
    );
}

#[test]
fn an_entity_carries_a_zero_delta_because_it_has_not_moved_yet() {
    let mut body = Vec::new();
    write_add_entity(
        &mut body,
        7,
        [9u8; 16],
        PLAYER_ENTITY_TYPE,
        (1.5, 64.0, -2.5),
        90.0,
        0.0,
    );

    let mut b = &body[..];
    assert_eq!(take_varint(&mut b), id::ADD_ENTITY);
    assert_eq!(take_varint(&mut b), 7);
    assert_eq!(&b[..16], &[9u8; 16][..]);
    b = &b[16..];
    assert_eq!(take_varint(&mut b), PLAYER_ENTITY_TYPE);
    assert_eq!(&b[..8], &1.5f64.to_be_bytes()[..], "x");
    assert_eq!(&b[8..16], &64.0f64.to_be_bytes()[..], "y");
    assert_eq!(&b[16..24], &(-2.5f64).to_be_bytes()[..], "z");
    b = &b[24..];
    assert_eq!(b[0], 0, "a zero delta is one byte, not a packed long");
    b = &b[1..];
    // Pitch, then yaw, then the head. The order was the bug: this asserted
    // yaw-then-pitch and so agreed with it.
    assert_eq!(b[0], 0, "pitch level, and pitch comes first");
    assert_eq!(b[1], 64, "then yaw east");
    assert_eq!(b[2], 64, "and the head turns with the body");
    assert_eq!(&b[3..], &[1u8][..], "and a data value of one, for a living thing");
}

#[test]
fn removing_nobody_is_still_a_well_formed_packet() {
    let mut body = Vec::new();
    write_remove_entities(&mut body, &[]);
    assert_eq!(body[0], id::REMOVE_ENTITIES as u8);
    assert_eq!(body[1], 0, "a count of zero, not a missing one");
}

const LEVELS: [&str; 3] = ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"];

/// A login as the capture recorded it: entity 6, survival, all three
/// dimensions, twenty players, a flat overworld.
fn login_6() -> Login<'static> {
    Login {
        entity_id: 6,
        hardcore: false,
        levels: &LEVELS,
        max_players: 20,
        view_distance: 10,
        simulation_distance: 10,
        dimension: "minecraft:overworld",
        seed: -8_775_607_066_745_405_828,
        game_type: 0,
        is_flat: true,
        portal_cooldown: 0,
        sea_level: -64,
    }
}

/// The same thing with the fields that vary taken at zero, so a test can
/// count to an offset.
fn login_zeroed() -> Login<'static> {
    Login {
        entity_id: 0,
        seed: 0,
        ..login_6()
    }
}

/// The three dimensions a 26.3 server offers, as the capture shows them.
#[test]
fn login_matches_the_bytes_the_capture_recorded() {
    // The bytes are the ones captured from a stock server, so a match here
    // means the layout is right rather than merely self-consistent.
    let mut w = Vec::new();
    write_login(&mut w, &login_6());

    assert_eq!(w[0], id::LOGIN as u8, "play login is id 50");
    assert_eq!(&w[1..5], &6i32.to_be_bytes(), "entity id, network order");
    assert_eq!(w[5], 0, "not hardcore");
    assert_eq!(w[6], 3, "three dimensions, as the real server sends");
    assert_eq!(w[7], 19, "then the first key's byte length");
    assert_eq!(&w[8..8 + 19], b"minecraft:overworld");
}

#[test]
fn the_dimension_holder_carries_a_leading_zero() {
    // This is the field that is easiest to get wrong and the capture is
    // what settles it: the key is preceded by a single zero byte, which
    // marks the holder as being sent by key rather than inline.
    let mut w = Vec::new();
    write_login(&mut w, &login_zeroed());

    // 1 id + 4 entity + 1 hardcore + 1 count + 43 for the three names,
    // + 3 VarInts + 3 bools puts the holder at 56.
    let at = 1 + 4 + 1 + 1 + (20 + 21 + 18) + 3 + 3;
    assert_eq!(w[at], 0, "the holder is sent by key");
    assert_eq!(w[at + 1], 19, "then the key length");
    assert_eq!(&w[at + 2..at + 21], b"minecraft:overworld");
}

#[test]
fn login_ends_with_both_session_flags_off() {
    let mut w = Vec::new();
    write_login(&mut w, &login_zeroed());

    assert_eq!(&w[w.len() - 2..], &[0, 0], "offline mode, no secure chat");
}

#[test]
fn the_cooldown_and_sea_level_are_varints() {
    // This is the field pair that cost a run. Writing them as the ints the
    // record declares them produces a login the client reads six bytes short
    // of, and reports as "login was larger than I expected, found 6 bytes
    // extra". The capture shows the real encoding: a cooldown of zero is one
    // byte, and the sea level is a five byte VarInt because it is negative.
    let login = Login {
        portal_cooldown: 0,
        sea_level: -63,
        ..login_zeroed()
    };
    let mut w = Vec::new();
    write_login(&mut w, &login);

    assert_eq!(
        &w[w.len() - 8..],
        &[0, 0xc1, 0xff, 0xff, 0xff, 0x0f, 0, 0],
        "cooldown as one byte, sea level as five, then the two session flags"
    );
}

#[test]
fn a_login_is_the_length_the_capture_recorded() {
    // 114 bytes for the packet as a stock server sent it: one id, four for
    // the entity, one for hardcore, one count, the three dimension names,
    // three distances, three flags, the holder and its key, the seed, the
    // five spawn fields, two VarInts and the two session flags.
    let mut w = Vec::new();
    write_login(&mut w, &login_6());

    assert_eq!(
        w.len(),
        114,
        "a mismatch here means a field is being written at the wrong width"
    );
}

#[test]
fn player_position_is_sixty_two_bytes_of_the_shape_the_capture_showed() {
    // Captured: teleport id 1, position (6.5, -60.0, -5.5), no delta,
    // no rotation, four zero bytes.
    let captured = Movement {
        x: 6.5,
        y: -60.0,
        z: -5.5,
        ..Movement::default()
    };
    let mut w = Vec::new();
    write_player_position(&mut w, 1, &captured);

    assert_eq!(w[0], id::PLAYER_POSITION as u8);
    assert_eq!(w[1], 1, "the teleport id the client echoes back");
    assert_eq!(&w[2..10], &6.5f64.to_be_bytes());
    assert_eq!(&w[10..18], &(-60.0f64).to_be_bytes());
    assert_eq!(&w[18..26], &(-5.5f64).to_be_bytes());
    assert_eq!(&w[26..50], &[0u8; 24], "the delta vector, still sent");
    assert_eq!(&w[50..58], &[0u8; 8], "yaw and pitch");
    assert_eq!(&w[58..], &[0, 0, 0, 0], "no relative fields");
    assert_eq!(
        w.len(),
        62,
        "62 bytes is what the capture recorded for this packet"
    );
}

#[test]
fn abilities_are_a_bitfield_and_two_floats() {
    // The capture recorded exactly ten bytes: 41 00 3d 4c cc cd 3d cc cc
    // cd. That is the id, a zero bitfield, 0.05 and 0.1. An earlier version
    // wrote eleven bytes of body and the client reported "found 2 bytes
    // extra" on this packet.
    let mut w = Vec::new();
    write_player_abilities(&mut w);

    assert_eq!(
        w,
        vec![
            id::PLAYER_ABILITIES as u8,
            0x00,
            0x3D, 0x4C, 0xCC, 0xCD,
            0x3D, 0xCC, 0xCC, 0xCD,
        ],
        "byte for byte what the capture recorded"
    );
    assert_eq!(w.len(), 10, "the capture's length for this packet");
}

#[test]
fn difficulty_is_two_bytes_after_the_id() {
    // The capture recorded "0a 01 00": peaceful is 0, and the byte after is
    // whether the difficulty is locked.
    let mut w = Vec::new();
    write_difficulty(&mut w, 1);

    assert_eq!(w, vec![id::CHANGE_DIFFICULTY as u8, 1, 0]);
}

#[test]
fn a_chunk_batch_reports_how_many_it_held() {
    // A VarInt and nothing else. The float this used to be made the packet
    // three bytes too long: five sent, two read.
    let mut w = Vec::new();
    write_chunk_batch_finished(&mut w, 81);

    assert_eq!(w, vec![id::CHUNK_BATCH_FINISHED as u8, 81]);
    assert_eq!(w.len(), 2, "an id and a one byte count");
}

#[test]
fn the_batch_start_is_only_its_id() {
    let mut w = Vec::new();
    write_chunk_batch_start(&mut w);
    assert_eq!(w, vec![id::CHUNK_BATCH_START as u8]);
}

#[test]
fn game_event_13_is_the_one_that_holds_the_loading_screen() {
    // Captured as "27 0d 00 00 00 00".
    let mut w = Vec::new();
    write_game_event(&mut w, 13, 0.0);

    assert_eq!(
        w,
        vec![id::GAME_EVENT as u8, 13, 0, 0, 0, 0],
        "the bytes the capture recorded, byte for byte"
    );
}

#[test]
fn the_chunk_cache_centre_is_two_varints() {
    let mut w = Vec::new();
    write_chunk_cache_center(&mut w, 0, 0);

    assert_eq!(w, vec![id::SET_CHUNK_CACHE_CENTER as u8, 0, 0]);
}

#[test]
fn the_default_spawn_is_a_key_a_block_pos_and_two_angles() {
    // Thirty-seven bytes in the capture, and three doubles for the position
    // would make it forty-six.
    let mut w = Vec::new();
    write_set_default_spawn(&mut w, "minecraft:overworld", 0, 64, 0);

    assert_eq!(w[0], id::SET_DEFAULT_SPAWN as u8);
    assert_eq!(w[1], 19, "key length");
    assert_eq!(&w[2..21], b"minecraft:overworld");
    assert_eq!(w.len(), 37, "the capture's length for this packet");
}

#[test]
fn a_level_event_is_four_four_eight_four_one_bytes() {
    let mut out = Vec::new();
    write_level_event(&mut out, BLOCK_DESTROYED, (1, 2, 3), 7, false);

    // One for the packet id, four for the event, eight for the position,
    // four for the data, one for the flag.
    assert_eq!(out.len(), 18);

    assert_eq!(out[0], id::LEVEL_EVENT as u8);
    assert_eq!(&out[1..5], &BLOCK_DESTROYED.to_be_bytes(), "a plain int");
    assert_eq!(&out[5..13], &pack_block_pos(1, 2, 3).to_be_bytes());
    assert_eq!(&out[13..17], &7i32.to_be_bytes(), "also a plain int");
    assert_eq!(out[17], 0, "not global");
}

#[test]
fn a_level_event_that_was_a_varint_would_be_misshapen() {
    // The check that earns its keep. Every other field in this file is a
    // VarInt, and these two are not, so writing them as VarInts produces a
    // packet the right length in the wrong place: the client reads the first
    // bytes of the four as a length and calls the rest trailing.
    let mut as_varint = Vec::new();
    VarInt(BLOCK_DESTROYED).write(&mut as_varint);

    assert_eq!(
        as_varint.len(),
        2,
        "2001 is two bytes as a VarInt, where the wire wants four"
    );

    let mut right = Vec::new();
    write_level_event(&mut right, BLOCK_DESTROYED, (0, 0, 0), 0, false);
    assert_eq!(
        right.len(),
        18,
        "and the packet it is written into is the size the client expects"
    );
}

#[test]
fn a_block_destruction_is_a_breaker_a_position_and_a_stage() {
    let mut out = Vec::new();
    write_block_destruction(&mut out, 42, (1, 2, 3), 5);

    // One for the packet id, one for the breaker (42 fits in a VarInt byte),
    // eight for the position, one for the stage.
    assert_eq!(out.len(), 11);
    assert_eq!(out[0], id::BLOCK_DESTRUCTION as u8);
    assert_eq!(out[1], 42);
    assert_eq!(&out[2..10], &pack_block_pos(1, 2, 3).to_be_bytes());
    assert_eq!(out[10], 5);
}

#[test]
fn a_breaker_over_a_hundred_and_twenty_seven_costs_two_bytes() {
    let mut out = Vec::new();
    write_block_destruction(&mut out, 200, (0, 0, 0), 0);

    // Entity ids are a shared counter, so they pass 127 eventually and the
    // packet grows by a byte when they do. Written out so that a reader who
    // assumes one byte finds out here.
    assert_eq!(out.len(), 12, "the breaker is now two bytes");
    assert_eq!(&out[1..3], &[0xC8, 0x01], "200 as a VarInt");
}

#[test]
fn clearing_a_crack_is_a_stage_of_minus_one() {
    let mut out = Vec::new();
    write_block_destruction(&mut out, 1, (0, 0, 0), NO_CRACK);

    // Signed on the way out, so it is 255 on the wire. The client reads it
    // back through an unsigned read and compares it against -1 there, so
    // writing 0 instead would leave a permanent crack rather than clearing it.
    assert_eq!(
        *out.last().expect("a stage was written"),
        0xFF,
        "and not a zero, which would be stage zero"
    );
}

#[test]
fn a_forget_packet_carries_a_chunk_position_in_one_long() {
    let mut out = Vec::new();
    write_forget_chunk(&mut out, -7, 12);

    // One byte for the id and eight for the position, and no more: a
    // VarInt pair here decodes as far as the id.
    assert_eq!(out.len(), 9);

    let mut r = &out[..];
    let (id, used) = VarInt::read(&mut r).expect("an id");
    assert_eq!(id.0, id::FORGET_LEVEL_CHUNK);
    assert_eq!(used, 1);

    // x in the low half, z in the high half.
    let packed = i64::from_be_bytes(r.try_into().expect("eight bytes"));
    assert_eq!((packed & 0xFFFF_FFFF) as i32, -7);
    assert_eq!((packed >> 32) as i32, 12);
}

#[test]
fn a_block_update_is_a_whole_packet_in_a_fixed_shape() {
    let mut out = Vec::new();
    write_block_update(&mut out, 0, 63, 0, 0);

    // One byte for the id, eight for the position, one for the state.
    assert_eq!(
        out.len(),
        10,
        "a block position is a fixed eight byte long; a VarLong here is one \
         byte and the client reads the rest of the packet as coordinates"
    );
    assert_eq!(out[0], id::BLOCK_UPDATE as u8);
    assert_eq!(&out[1..9], &pack_block_pos(0, 63, 0).to_be_bytes());
    assert_eq!(out[9], 0, "state zero is air");

    // Two updates in one buffer is two packets back to back, which is what
    // the caller encodes as separate frames.
    let mut two = Vec::new();
    write_block_update(&mut two, 1, 2, 3, 4);
    write_block_update(&mut two, 5, 6, 7, 8);
    assert_eq!(two.len(), 20);
    assert_eq!(two[10], id::BLOCK_UPDATE as u8);
}

#[test]
fn a_break_action_reads_the_block_it_names() {
    let mut frame = Vec::new();
    VarInt(id::SERVERBOUND_PLAYER_ACTION).write(&mut frame);
    VarInt(3).write(&mut frame); // STOP_DESTROY_BLOCK
    frame.extend_from_slice(&pack_block_pos(10, 64, -3).to_be_bytes());
    frame.push(1); // the top face
    VarInt(7).write(&mut frame); // the client's sequence number

    let action = PlayerActionPacket::read(&frame).expect("well formed");

    assert_eq!(action.action, PlayerAction::StopDestroy);
    assert_eq!(unpack_block_pos(action.pos), (10, 64, -3));
    assert_eq!(action.direction, 1);
}

#[test]
fn an_action_that_is_not_a_break_is_named_as_such() {
    let mut frame = Vec::new();
    VarInt(id::SERVERBOUND_PLAYER_ACTION).write(&mut frame);
    VarInt(4).write(&mut frame); // DROP_ALL_ITEMS
    frame.extend_from_slice(&0i64.to_be_bytes());
    frame.push(0);
    VarInt(0).write(&mut frame);

    let action = PlayerActionPacket::read(&frame).expect("well formed");
    assert_eq!(action.action, PlayerAction::Other);
}

#[test]
fn a_block_position_packs_into_one_eight_byte_value() {
    assert_eq!(pack_block_pos(0, 0, 0), 0);
    // y occupies the low twelve bits, so it sits at the bottom.
    assert_eq!(pack_block_pos(0, 1, 0), 1);
    // z starts at bit twelve.
    assert_eq!(pack_block_pos(0, 0, 1), 1 << 12);
    // x starts at bit thirty-eight.
    assert_eq!(pack_block_pos(1, 0, 0), 1 << 38);
}

#[test]
fn a_block_position_survives_the_round_trip() {
    // The spawn, and then its negative neighbours: the world runs from -64
    // upward in y, so the twelve bit field has to carry a negative number
    // without spilling into z.
    for (x, y, z) in [(0, 64, 0), (-1, -64, -1), (30_000_000, 320, -30_000_000)] {
        assert_eq!(
            unpack_block_pos(pack_block_pos(x, y, z)),
            (x, y, z),
            "packing and unpacking must give back the position that went in"
        );
    }
}

#[test]
fn the_border_walks_to_exactly_forty_bytes() {
    // Four doubles, then a VarLong and three VarInts. Seven doubles would be
    // fifty-six.
    let mut w = Vec::new();
    write_initialize_border(&mut w);

    assert_eq!(w[0], id::INITIALIZE_BORDER as u8);
    assert_eq!(w.len(), 41, "the capture's length for this packet");
    assert_eq!(
        &w[1..17],
        &[0u8; 16],
        "the two sizes are zero, so the whole world is playable"
    );
    assert_eq!(
        f64::from_be_bytes(w[17..25].try_into().unwrap()),
        MAX_BORDER,
        "then the two centres, parked at the vanilla maximum"
    );
}

#[test]
fn the_held_slot_is_one_varint() {
    // Two bytes for the whole packet in the capture.
    let mut w = Vec::new();
    write_set_held_slot(&mut w, 0);

    assert_eq!(w, vec![id::SET_HELD_SLOT as u8, 0]);
}

#[test]
fn keep_alive_echoes_the_id_it_was_given() {
    let mut w = Vec::new();
    write_keep_alive(&mut w, 0x0102030405060708);

    assert_eq!(w[0], id::KEEP_ALIVE as u8);
    assert_eq!(&w[1..9], &[1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn the_play_ids_are_the_ones_the_game_uses() {
    // From packets.json, produced by the data generator from the same jar.
    // Pinned because they are positional on the wire.
    assert_eq!(id::LOGIN, 50);
    assert_eq!(id::PLAYER_POSITION, 73);
    assert_eq!(id::GAME_EVENT, 39);
    assert_eq!(id::CHANGE_DIFFICULTY, 10);
    assert_eq!(id::SET_CHUNK_CACHE_CENTER, 96);
    assert_eq!(id::SET_DEFAULT_SPAWN, 99);
    assert_eq!(id::INITIALIZE_BORDER, 44);
    assert_eq!(id::SET_TIME, 115);
    assert_eq!(id::KEEP_ALIVE, 45);
    assert_eq!(id::SERVERBOUND_KEEP_ALIVE, 28);
    assert_eq!(id::SERVERBOUND_CLIENT_TICK_END, 13);
    assert_eq!(id::MOVE_PLAYER_ROT, 32);
    }

    /// Checks every id this module names against the game's own report.
    ///
    /// The list above pins some numbers by hand, which is how the swing packet
    /// came to be written with the right shape and the wrong number: two is
    /// `animate`, a packet with an entity and a byte, and one hundred and
    /// twenty-three is `swing_animation`. Reading the shape from
    /// `ClientboundSwingAnimationPacket` and the number from the packet called
    /// `animate` produces something that decodes as the other one and leaves
    /// the client two bytes short of the end.
    ///
    /// So the expected values here are not written by hand at all. They come
    /// out of the data generator's report, the same file that produced every
    /// other number in this module, and a constant that is renamed or given the
    /// wrong number fails rather than costing somebody a crash report.
    #[test]
    fn an_acknowledged_change_is_the_update_followed_by_the_ack() {
        // The order is the whole of it. The client draws the change straight
        // away and keeps the shape the block used to have until the ack says
        // otherwise, so an ack that arrives before the update settles nothing,
        // and an ack that never arrives leaves a block that is invisible and
        // still solid.
        let mut body = Vec::new();
        write_block_update(&mut body, 1, 2, 3, 0);
        write_block_changed_ack(&mut body, 1);

        let mut b = &body[..];
        assert_eq!(take_varint(&mut b), id::BLOCK_UPDATE, "the change first");
        // A block position is a fixed eight byte long and not three VarInts.
        // Compared against the project's own packing rather than a constant
        // worked out by hand, since what this test is about is the order of the
        // two packets and not how a position is laid out.
        assert_eq!(
            &b[..8],
            &pack_block_pos(1, 2, 3).to_be_bytes()[..],
            "which is its position"
        );
        b = &b[8..];
        assert_eq!(take_varint(&mut b), 0, "and the state it became, which is air");

        assert_eq!(take_varint(&mut b), id::BLOCK_CHANGED_ACK, "then the ack");
        assert_eq!(take_varint(&mut b), 1, "and the sequence it settles");
        assert!(b.is_empty(), "and nothing else");
    }

    #[test]
    fn the_sequence_goes_up() {
        // The client reads it as a counter. What it must not do is repeat, or
        // go backwards, because either of those reads as an old change being
        // settled again.
        let mut last = 0;
        for step in 1..=5 {
            let mut body = Vec::new();
            write_block_changed_ack(&mut body, step);
            let mut b = &body[..];
            assert_eq!(take_varint(&mut b), id::BLOCK_CHANGED_ACK);
            let sequence = take_varint(&mut b);
            assert!(sequence > last, "{sequence} must be above {last}");
            last = sequence;
        }
    }

    #[test]
    fn a_player_action_carries_the_clients_own_prediction_number() {
        // The last field on the wire, and the one that was being dropped.
        //
        // A player that breaks a block has already applied the change locally
        // and is waiting to be told it was right. It tags the guess with a
        // number it owns, and the server hands that number back. Reading it as
        // something else, or not reading it at all, leaves the block drawn and
        // solid at the same time.
        let mut frame = Vec::new();
        VarInt(id::SERVERBOUND_PLAYER_ACTION).write(&mut frame);
        VarInt(3).write(&mut frame); // stop destroy
        frame.extend_from_slice(&0i64.to_be_bytes()); // the position
        frame.push(0); // the face
        VarInt(17).write(&mut frame); // the prediction number

        let action = PlayerActionPacket::read(&frame).expect("it is a player action");
        assert_eq!(action.action, PlayerAction::StopDestroy);
        assert_eq!(
            action.sequence, 17,
            "and the number the client tagged its guess with"
        );
    }

    #[test]
    fn the_acknowledgement_uses_the_number_the_client_sent() {
        // Not one of our own. The client ends its predictions up to the number
        // it is given, so a number of ours settles nothing, and a number of
        // ours that happened to be larger would settle guesses that had not
        // been made yet.
        let mut body = Vec::new();
        write_block_changed_ack(&mut body, 17);

        let mut b = &body[..];
        assert_eq!(take_varint(&mut b), id::BLOCK_CHANGED_ACK);
        assert_eq!(take_varint(&mut b), 17, "the client's own number, unchanged");
        assert!(b.is_empty());
    }

    #[test]
    fn a_level_event_carries_its_numbers_as_plain_ints() {
        // Not VarInts, which is what makes this packet worth a test of its own:
        // every other number in these packets is a VarInt, and `readInt` here
        // reads four bytes and stops.
        //
        // A log that decoded this packet as a VarInt reported event 0 at
        // position 8004 0 0 for a block broken at 0 -7 1, which read like a
        // fault in the server and was a fault in the log instead.
        let mut body = Vec::new();
        write_level_event(&mut body, BLOCK_DESTROYED, (0, -7, 1), 9, false);

        let mut b = &body[..];
        assert_eq!(take_varint(&mut b), id::LEVEL_EVENT);

        assert_eq!(
            i32::from_be_bytes(b[..4].try_into().unwrap()),
            BLOCK_DESTROYED,
            "the event as a plain int"
        );
        b = &b[4..];

        let pos = i64::from_be_bytes(b[..8].try_into().unwrap());
        assert_eq!(
            unpack_block_pos(pos),
            (0, -7, 1),
            "and the position is read from the byte after all four of it"
        );
        b = &b[8..];

        assert_eq!(
            i32::from_be_bytes(b[..4].try_into().unwrap()),
            9,
            "the data is a plain int too"
        );
        assert_eq!(b[4], 0, "global false is a boolean");
        b = &b[5..];
        assert!(b.is_empty(), "nothing left over: {b:?}");
    }

    #[test]
    fn every_id_here_is_the_one_the_generator_reports() {
        let report = include_str!("../../../extracted/packets.json");

        // Scoped to one phase, because a name is not unique across the file.
        // `keep_alive` is 4 in an earlier phase and 45 in play, and searching
        // the whole report finds the first one.
        let play = report.find("\"play\":").expect("the report has a play phase");
        let cb = report[play..]
            .find("\"clientbound\":")
            .map(|i| play + i)
            .expect("play has a clientbound half");
        let sb = report[play..]
            .find("\"serverbound\":")
            .map(|i| play + i)
            .expect("play has a serverbound half");
        let status = report[sb..]
            .find("\"status\":")
            .map(|i| sb + i)
            .unwrap_or(report.len());
        let halves: [(bool, &str); 2] = [(true, &report[cb..sb]), (false, &report[sb..status])];

        // (our constant, the name the generator knows it by, clientbound)
        let cases: &[(i32, &str, bool)] = &[
            (id::BLOCK_UPDATE, "block_update", true),
            (id::BLOCK_CHANGED_ACK, "block_changed_ack", true),
            (id::BLOCK_DESTRUCTION, "block_destruction", true),
            (id::ADD_ENTITY, "add_entity", true),
            (id::REMOVE_ENTITIES, "remove_entities", true),
            (id::SWING_ANIMATION, "swing_animation", true),
            (id::ROTATE_HEAD, "rotate_head", true),
            (id::MOVE_ENTITY_POS_ROT, "move_entity_pos_rot", true),
            (id::LEVEL_EVENT, "level_event", true),
            (id::SET_ENTITY_DATA, "set_entity_data", true),
            (id::PLAYER_INFO_UPDATE, "player_info_update", true),
            (id::PLAYER_INFO_REMOVE, "player_info_remove", true),
            (id::FORGET_LEVEL_CHUNK, "forget_level_chunk", true),
            (id::LOGIN, "login", true),
            (id::PLAYER_POSITION, "player_position", true),
            (id::GAME_EVENT, "game_event", true),
            (id::CHANGE_DIFFICULTY, "change_difficulty", true),
            (id::SET_CHUNK_CACHE_CENTER, "set_chunk_cache_center", true),
            (id::SET_DEFAULT_SPAWN, "set_default_spawn_position", true),
            (id::INITIALIZE_BORDER, "initialize_border", true),
            (id::SET_TIME, "set_time", true),
            (id::KEEP_ALIVE, "keep_alive", true),
            (id::LEVEL_CHUNK_WITH_LIGHT, "level_chunk_with_light", true),
            (id::CHUNK_BATCH_START, "chunk_batch_start", true),
            (id::CHUNK_BATCH_FINISHED, "chunk_batch_finished", true),
            (id::CHUNKS_BIOMES, "chunks_biomes", true),
            (id::PLAYER_ABILITIES, "player_abilities", true),
            (id::SERVERBOUND_KEEP_ALIVE, "keep_alive", false),
            (id::SERVERBOUND_PLAYER_INPUT, "player_input", false),
            (id::SERVERBOUND_PUNCH, "punch", false),
            (id::SERVERBOUND_PLAYER_ACTION, "player_action", false),
            (id::SERVERBOUND_USE_ITEM_ON, "use_item_on", false),
            (id::SERVERBOUND_CLIENT_TICK_END, "client_tick_end", false),
            (id::SERVERBOUND_PLAYER_LOADED, "player_loaded", false),
            (id::MOVE_PLAYER_POS, "move_player_pos", false),
            (id::MOVE_PLAYER_POS_ROT, "move_player_pos_rot", false),
            (id::MOVE_PLAYER_ROT, "move_player_rot", false),
            (id::MOVE_PLAYER_STATUS_ONLY, "move_player_status_only", false),
            (id::SERVERBOUND_DEBUG_SUBSCRIPTION, "debug_subscription_request", false),
        ];

        for (ours, name, clientbound) in cases {
            let phase = if *clientbound { "clientbound" } else { "serverbound" };
            let half = halves
                .iter()
                .find(|(is_cb, _)| *is_cb == *clientbound)
                .map(|(_, text)| *text)
                .expect("both halves are there");

            let needle = format!("\"minecraft:{name}\":");
            let at = half
                .find(&needle)
                .unwrap_or_else(|| panic!("play {phase} {name} is not in the report at all"));

            // Only up to the end of this entry. Looking a fixed distance ahead
            // runs into the next packet and reads that one's id.
            let rest = &half[at..];
            let entry = &rest[..rest.find('}').unwrap_or(rest.len())];

            let key = entry.find("\"protocol_id\"").unwrap_or_else(|| {
                panic!("play {phase} {name} is in the report but has no protocol id")
            });
            let digits: String = entry[key..]
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();

            assert_eq!(
                digits.parse::<i32>().unwrap_or(-1),
                *ours,
                "play {phase} {name}: the report says {digits}, this module says {ours}"
            );
        }
    }

/// One movement packet, hand built the way the client builds them.
fn movement_packet(id: i32, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    VarInt(id).write(&mut frame);
    frame.extend_from_slice(body);
    frame
}

#[test]
fn a_position_packet_reads_its_position_and_flags() {
    let mut body = Vec::new();
    body.extend_from_slice(&8.5f64.to_be_bytes());
    body.extend_from_slice(&64.0f64.to_be_bytes());
    body.extend_from_slice(&(-2.5f64).to_be_bytes());
    body.push(1); // on the ground, no collision

    let moved = ClientMovement::read(&movement_packet(id::MOVE_PLAYER_POS, &body))
        .expect("the packet is well formed");

    assert_eq!(moved.x, Some(8.5));
    assert_eq!(moved.y, Some(64.0));
    assert_eq!(moved.z, Some(-2.5));
    assert_eq!(moved.yaw, None, "a position packet carries no rotation");
    assert!(moved.on_ground);
    assert!(!moved.horizontal_collision);
}

#[test]
fn a_rotation_packet_carries_only_the_angles() {
    let mut body = Vec::new();
    body.extend_from_slice(&90.0f32.to_be_bytes());
    body.extend_from_slice(&(-45.0f32).to_be_bytes());
    body.push(3); // on the ground and into a wall

    let moved = ClientMovement::read(&movement_packet(id::MOVE_PLAYER_ROT, &body))
        .expect("the packet is well formed");

    assert_eq!(moved.x, None, "a rotation packet carries no position");
    assert_eq!(moved.yaw, Some(90.0));
    assert_eq!(moved.pitch, Some(-45.0));
    assert!(moved.on_ground);
    assert!(moved.horizontal_collision);
}

#[test]
fn a_status_only_packet_is_one_byte_of_flags() {
    let moved =
        ClientMovement::read(&movement_packet(id::MOVE_PLAYER_STATUS_ONLY, &[2]))
            .expect("the packet is well formed");

    assert_eq!(moved.x, None);
    assert_eq!(moved.yaw, None);
    assert!(!moved.on_ground);
    assert!(moved.horizontal_collision);
}

#[test]
fn a_packet_that_cuts_off_mid_position_is_rejected() {
    let short = &8.5f64.to_be_bytes()[..6];

    assert!(ClientMovement::read(&movement_packet(
        id::MOVE_PLAYER_POS,
        short
    ))
    .is_err());
}