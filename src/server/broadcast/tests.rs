use super::*;

/// A registration with the parts these tests do not care about filled in.
///
/// A uuid that is all zeroes is not a uuid a real player would have, and
/// nothing here looks at one. Naming the helper rather than spelling out
/// four arguments at each call keeps the tests about the queue, which is
/// what they are actually about.
fn join(wire: &Broadcast, name: &str) -> Registration {
    wire.register(name, [0u8; 16], 1, crate::world::terrain::spawn_point())
}

/// Reads the x delta out of a `move_entity_pos_rot` body.
fn read_delta_x(body: &[u8]) -> f64 {
    let mut b = body;
    assert_eq!(
        crate::protocol::play::id::MOVE_ENTITY_POS_ROT as u8,
        b[0],
        "the packet is a movement"
    );
    b = &b[1..];
    // The entity id and the properties.
    b = skip_varints(b);
    b = skip_varints(b);
    let dx = i16::from_be_bytes([b[0], b[1]]);
    f64::from(dx) / DELTA_SCALE
}

fn skip_varints(mut b: &[u8]) -> &[u8] {
    while b[0] & 0x80 != 0 {
        b = &b[1..];
    }
    &b[1..]
}

#[test]
fn a_player_arriving_is_told_about_the_ones_already_here() {
    let wire = std::sync::Arc::new(Broadcast::new());
    let first = wire.register("first", [1u8; 16], 7, (0.0, 64.0, 0.0));
    let second = wire.register("second", [2u8; 16], 9, (0.0, 64.0, 0.0));

    let seen = wire.snapshot();
    assert_eq!(
        seen.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["first", "second"],
        "the second arrival is handed both, including the one before it"
    );

    // Each entry carries the things the receiving side cannot work out for
    // itself: the name, the profile uuid, and the entity id the client will
    // address the body by.
    let me = seen.iter().find(|p| p.name == "second").expect("the second arrival");
    assert_eq!(me.uuid, [2u8; 16]);
    assert_eq!(me.entity_id, 9);
    assert_eq!(me.id, second.id);
    assert_ne!(
        me.id, me.entity_id as u64,
        "the registration id and the entity id are unrelated numbers, and \
         sending one where the other belongs addresses the wrong player"
    );
    let _ = first;
}


#[test]
fn a_push_reaches_everyone_but_the_one_who_caused_it() {
    let wire = Broadcast::new();
    let mine = join(&wire, "digger");
    let theirs = join(&wire, "watcher");

    wire.send_others(mine.id, b"a hole".to_vec());

    assert_eq!(
        theirs.inbox.try_recv().expect("the watcher was told"),
        b"a hole"
    );
    assert!(
        mine.inbox.try_recv().is_err(),
        "the digger already knows, and is told by its own connection"
    );
}

#[test]
fn a_removed_player_is_reached_by_nothing_further() {
    let wire = Broadcast::new();
    let gone = join(&wire, "leaving");
    let stayer = join(&wire, "staying");

    wire.remove(gone.id);
    wire.send_others(stayer.id, b"hello".to_vec());

    assert!(
        gone.inbox.try_recv().is_err(),
        "and their queue is closed, so a drain ends rather than waits"
    );
    assert_eq!(wire.player_count(), 1);
}

#[test]
fn ids_do_not_repeat_when_someone_comes_back() {
    let wire = Broadcast::new();
    let first = join(&wire, "first");
    wire.remove(first.id);
    let second = join(&wire, "second");

    assert_ne!(first.id, second.id, "or a stale queue could be addressed as it");
}

#[test]
fn a_push_carries_the_packet_id_and_the_body_together() {
    // A pushed body is handed to the receiver's own encoder, which adds a
    // length and nothing else. So the id has to be inside it. Sending the
    // body without the id makes the packet arrive with the first byte of
    // whatever field follows as its id, which for a block update is a byte
    // of the position: id 8, which is bundle_delimiter, a packet with no
    // body at all, and the client reports the position as eight bytes it
    // did not expect.
    let wire = Broadcast::new();
    let mine = join(&wire, "digger");
    let theirs = join(&wire, "watcher");

    let mut body = Vec::new();
    crate::protocol::play::write_block_update(&mut body, 1, 2, 3, 4);
    wire.send_others(mine.id, body.clone());

    let received = theirs.inbox.try_recv().expect("the watcher was told");
    assert_eq!(
        received,
        body,
        "what goes in is what comes out, byte for byte"
    );
    assert_eq!(
        received[0],
        crate::protocol::play::id::BLOCK_UPDATE as u8,
        "and the first byte is still the packet id"
    );
}

#[test]
fn two_observers_who_joined_at_different_times_get_different_deltas() {
    // The reason "seen" is per observer and the reason the wire carries a
    // delta rather than a position.
    //
    // The watcher has been told the mover is at 0,0,0 and is sent a move to
    // 1,0,0, so its delta is one block. The latecomer is only now being
    // told where the mover is — at 1,0,0 — and then is sent the same move
    // to 2,0,0, so its delta is one block too.
    //
    // A server that kept one copy of "where the mover is" would send the
    // second observer a delta of two, and that observer would jump.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    let late = wire.register("late", [3u8; 16], 3, (0.0, 64.0, 0.0));

    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    // First move: only the watcher was told about the mover.
    wire.update_pose(mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);
    let first = watcher.inbox.try_recv().expect("the watcher was told");
    assert_eq!(read_delta_x(&first), 1.0, "one block from where it was told");

    // The latecomer arrives and is given the mover's current position.
    wire.note_told(late.id, mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);

    // Second move.
    wire.update_pose(mover.id, (2.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);

    let to_watcher = watcher.inbox.try_recv().expect("the watcher again");
    let to_late = late.inbox.try_recv().expect("the latecomer too");

    // Exactly one each. This assertion is the one that was missing, and its
    // absence is why a server with two observers sent everybody else's
    // movement as well: each observer received one packet per observer, so
    // the body advanced twice as fast as it was told it had and walked off
    // into the sky. The delta values were all correct, which is why the
    // earlier version of this test passed while the game was visibly wrong.
    assert!(
        watcher.inbox.try_recv().is_err(),
        "one packet per movement, however many are watching"
    );
    assert!(
        late.inbox.try_recv().is_err(),
        "and the same for the other one"
    );

    assert_eq!(
        read_delta_x(&to_watcher),
        1.0,
        "the watcher is told the move from where it last was"
    );
    assert_eq!(
        read_delta_x(&to_late),
        1.0,
        "and so is the latecomer, from where it was introduced"
    );
    // Both packets address the mover and both say one block, so they come
    // out identical here. That is the point: each was measured from what
    // that observer knew, and the observers happen to agree. Had the server
    // kept one copy of the mover's position, one of these would have been a
    // two block jump.
    assert_eq!(to_watcher, to_late);
}

#[test]
fn every_observer_gets_one_packet_and_not_one_per_observer() {
    // Three watchers, so the mistake would have tripled the speed.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let mut watchers = Vec::new();
    for i in 0..3 {
        let w = wire.register(&format!("watcher{i}"), [2u8; 16], 100 + i, (0.0, 64.0, 0.0));
        wire.note_told(w.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);
        watchers.push(w);
    }

    wire.update_pose(mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);

    for w in &watchers {
        assert_eq!(
            read_delta_x(&w.inbox.try_recv().unwrap_or_else(|_| {
                panic!("watcher {} got nothing", w.name)
            })),
            1.0,
            "watcher {} was told the move",
            w.name
        );
        assert!(
            w.inbox.try_recv().is_err(),
            "watcher {} was told it more than once, and will walk at three \
             times the speed it was told about",
            w.name
        );
    }
}

#[test]
fn a_player_standing_still_sends_nothing() {
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    for _ in 0..20 {
        wire.send_movement(mover.id, true);
    }
    assert!(
        watcher.inbox.try_recv().is_err(),
        "twenty identical movements a second is twenty packets a second \
         that every client decodes and throws away"
    );
}

#[test]
fn a_movement_too_small_to_see_is_not_sent() {
    // The client rounds to 1/4096, so anything under half of that arrives
    // as zero. The test has to be on the rounded value, because that is
    // what the client ends up holding.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    // A quarter of a step, which rounds away to nothing.
    wire.update_pose(mover.id, (0.0 + 1.0 / 16384.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);
    assert!(watcher.inbox.try_recv().is_err(), "a quarter of a step");

    // Half a step, which rounds up to one and is worth sending.
    wire.update_pose(mover.id, (1.0 / 8192.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);
    assert_eq!(
        read_delta_x(&watcher.inbox.try_recv().unwrap()),
        1.0 / 4096.0,
        "which arrives as exactly one step"
    );

    // And a real movement.
    wire.update_pose(mover.id, (0.5, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);
    assert_eq!(read_delta_x(&watcher.inbox.try_recv().unwrap()), 0.5);
}

#[test]
fn a_movement_is_not_sent_to_somebody_who_has_never_heard_of_the_player() {
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let stranger = wire.register("stranger", [2u8; 16], 2, (0.0, 64.0, 0.0));

    wire.update_pose(mover.id, (5.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);

    assert!(
        stranger.inbox.try_recv().is_err(),
        "a delta from nowhere is not a position"
    );
}

#[test]
fn a_movement_is_not_sent_across_the_room() {
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let far = wire.register("far", [2u8; 16], 2, (1000.0, 64.0, 0.0));
    wire.note_told(far.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    wire.update_pose(mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);
    assert_eq!(
        far.inbox.try_recv().expect("the body is removed, which is news")
            [0],
        crate::protocol::play::id::REMOVE_ENTITIES as u8,
        "a kilometre is past seeing, and past being in the world"
    );
}

#[test]
fn turning_in_place_is_worth_a_packet() {
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    wire.update_pose(mover.id, (0.0, 64.0, 0.0), 90.0, 0.0);
    wire.send_movement(mover.id, true);

    let body = watcher.inbox.try_recv().expect("a turn is news");
    // Two VarInts, then six bytes of delta, then the two angles.
    assert_eq!(body[9], 64, "yaw east");
}

#[test]
fn a_movement_on_one_axis_only_is_still_a_movement() {
    // Three axes, and the check that decides whether to send has to look at
    // all of them. It once looked at the first one only, which is why
    // walking worked, falling did not, and neither was noticed on flat
    // ground.
    for (name, to) in [
        ("across x", (3.0, 64.0, 0.0)),
        ("up and down y", (0.0, 70.0, 0.0)),
        ("along z", (0.0, 64.0, -3.0)),
        ("diagonally", (3.0, 67.0, -3.0)),
    ] {
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
        wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

        wire.update_pose(mover.id, to, 0.0, 0.0);
        wire.send_movement(mover.id, true);

        assert!(
            watcher.inbox.try_recv().is_ok(),
            "moving {} is worth a packet",
            name
        );
    }
}

#[test]
fn a_turn_sends_the_head_separately_from_the_body() {
    // move_entity_pos_rot carries yRot and xRot, and yRot is the body's.
    // The head is a different packet, and a player looking sideways while
    // walking straight ahead is exactly the case that needs it.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    wire.update_pose(mover.id, (1.0, 64.0, 0.0), 180.0, 0.0);
    wire.send_movement(mover.id, true);

    let first = watcher.inbox.try_recv().expect("the movement");
    assert_eq!(
        first[0],
        crate::protocol::play::id::MOVE_ENTITY_POS_ROT as u8,
        "the body, carrying the body angle"
    );

    let second = watcher.inbox.try_recv().expect("and then the head");
    assert_eq!(
        second[0],
        crate::protocol::play::id::ROTATE_HEAD as u8,
        "the head, which the movement packet has no room for"
    );
    assert_eq!(
        second[2], 128,
        "pointing the other way: 180 degrees is 128"
    );
    assert!(watcher.inbox.try_recv().is_err(), "and only those two");
}

#[test]
fn a_straight_walk_does_not_send_a_head_packet() {
    // The head only moves when the body angle changes, and a walk down a
    // straight line does not change it. Sending one anyway would be a
    // packet per observer per movement packet for nothing.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 90.0, 0.0);

    wire.update_pose(mover.id, (1.0, 64.0, 0.0), 90.0, 0.0);
    wire.send_movement(mover.id, true);

    let body = watcher.inbox.try_recv().expect("the movement");
    assert_eq!(body[0], crate::protocol::play::id::MOVE_ENTITY_POS_ROT as u8);
    assert!(
        watcher.inbox.try_recv().is_err(),
        "and no head packet, because the body has not turned"
    );
}

#[test]
fn a_player_straight_above_you_is_still_visible() {
    // The reason the distance is measured horizontally.
    //
    // A three-dimensional check is the obvious one to write and it hides
    // two players standing three blocks apart on a hillside, which is the
    // case somebody on the ridge below most wants to be able to see. The
    // game measures x and z and ignores y entirely.
    let wire = Broadcast::new();
    let lower = wire.register("lower", [1u8; 16], 1, (0.0, 0.0, 0.0));
    let upper = wire.register("upper", [2u8; 16], 2, (3.0, 300.0, 0.0));
    wire.set_view_distance(lower.id, 8);
    wire.note_told(lower.id, upper.id, (3.0, 300.0, 0.0), 0.0, 0.0);

    wire.update_pose(upper.id, (3.0, 305.0, 0.0), 0.0, 0.0);
    wire.send_movement(upper.id, false);

    assert!(
        lower.inbox.try_recv().is_ok(),
        "three blocks away horizontally, three hundred vertically"
    );
}

#[test]
fn how_far_a_player_is_visible_is_the_observers_own_view_distance() {
    // Not a fixed constant. One player's render distance deciding
    // everybody's is not how the game does it.
    for (chunks, expect_visible, expect_gone) in
        [(2, 32.0, 33.0), (8, 128.0, 129.0), (16, 256.0, 257.0)]
    {
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
        wire.set_view_distance(watcher.id, chunks);
        wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

        // Walk out in small steps: a delta carries under eight blocks, so a
        // jump further than that arrives as a position instead and this
        // test is about the range, not about the step size.
        let mut at = 0.0;
        while at + 4.0 < expect_visible {
            at += 4.0;
            wire.update_pose(mover.id, (at, 64.0, 0.0), 0.0, 0.0);
            wire.send_movement(mover.id, true);
            assert!(
                watcher.inbox.try_recv().is_ok(),
                "walking out to {} blocks",
                at
            );
        }

        wire.update_pose(mover.id, (expect_visible, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);
        let inside = watcher
            .inbox
            .try_recv()
            .unwrap_or_else(|_| panic!("nothing at {expect_visible} blocks"));
        assert_eq!(
            inside[0],
            crate::protocol::play::id::MOVE_ENTITY_POS_ROT as u8,
            "at {} blocks it is still a movement",
            expect_visible
        );
        assert!(watcher.inbox.try_recv().is_err(), "and only that");

        wire.update_pose(mover.id, (expect_gone, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);
        let outside = watcher
            .inbox
            .try_recv()
            .unwrap_or_else(|_| panic!("at {expect_gone} blocks, out of range"));
        assert_eq!(
            outside[0],
            crate::protocol::play::id::REMOVE_ENTITIES as u8,
            "and just past {} blocks the body is taken away",
            chunks * 16
        );
        assert!(watcher.inbox.try_recv().is_err(), "once");
    }
}

#[test]
fn walking_out_of_range_removes_the_body_rather_than_freezing_it() {
    // Stopping the updates looks identical from the server and completely
    // different from the client: the client still holds the entity and
    // keeps drawing it, so the player stands frozen in mid-air for as long
    // as they care to walk away. The body has to be taken away.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    wire.update_pose(mover.id, (200.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);

    let gone = watcher
        .inbox
        .try_recv()
        .expect("the body is taken away, not abandoned");
    assert_eq!(
        gone[0],
        crate::protocol::play::id::REMOVE_ENTITIES as u8,
        "so it is a remove_entities"
    );
}

#[test]
fn going_still_out_of_range_does_not_keep_saying_it() {
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    wire.update_pose(mover.id, (200.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);
    assert!(watcher.inbox.try_recv().is_ok(), "the first crossing removes it");

    for _ in 0..20 {
        wire.send_movement(mover.id, true);
    }
    assert!(
        watcher.inbox.try_recv().is_err(),
        "and a player standing far away is not removed twenty times a second"
    );
}

#[test]
fn walking_back_puts_them_down_where_they_actually_are() {
    // Not a delta from where they left. That entry is hundreds of blocks
    // stale by now, and a delta from it would put them somewhere neither
    // of them has ever been.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    wire.update_pose(mover.id, (200.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);
    watcher.inbox.try_recv().expect("removed");

    wire.update_pose(mover.id, (10.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);

    let back = watcher.inbox.try_recv().expect("and put back");
    assert_eq!(
        back[0],
        crate::protocol::play::id::ADD_ENTITY as u8,
        "as a position, not a movement"
    );
    // Entity id, then the uuid, then the type: the x coordinate follows.
    let x = f64::from_be_bytes(back[2 + 16 + 2..2 + 16 + 2 + 8].try_into().unwrap());
    assert_eq!(x, 10.0, "and it is where they are now");

    // And the next movement is measured from there.
    wire.update_pose(mover.id, (11.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);
    assert_eq!(
        read_delta_x(&watcher.inbox.try_recv().unwrap()),
        1.0,
        "not from where they were when they left"
    );
}

#[test]
fn a_player_who_was_never_introduced_is_not_spawned_into_a_client_that_has_not_been_told_their_name() {
    // The window between one connection registering and the other
    // finishing its chunks. Distinct from having been removed.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let stranger = wire.register("stranger", [2u8; 16], 2, (0.0, 64.0, 0.0));

    wire.update_pose(mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, true);

    assert!(
        stranger.inbox.try_recv().is_err(),
        "nothing at all, rather than a body with no name"
    );
}

#[test]
fn a_move_that_cannot_be_a_delta_arrives_as_a_position() {
    // The self-healing path: a player who fell, or was teleported, or was
    // simply too far, is put down again rather than nudged. This is what a
    // player arriving used to need and did not get.
    let wire = Broadcast::new();
    let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
    let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
    wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

    // A fall of forty blocks, which no short can carry.
    wire.update_pose(mover.id, (0.0, 24.0, 0.0), 0.0, 0.0);
    wire.send_movement(mover.id, false);

    let body = watcher
        .inbox
        .try_recv()
        .expect("something was sent, even though it could not be a delta");
    assert_eq!(
        body[0],
        crate::protocol::play::id::ADD_ENTITY as u8,
        "so it is an add_entity, carrying the position outright"
    );
    assert!(
        watcher.inbox.try_recv().is_err(),
        "once"
    );
}

#[test]
fn nobody_listening_is_not_a_problem() {
    let wire = Broadcast::new();
    wire.send_others(0, b"into the void".to_vec());
    assert_eq!(wire.player_count(), 0);
}

#[test]
fn names_come_back_for_the_log() {
    let wire = Broadcast::new();
    join(&wire, "noverisp3");
    join(&wire, "notrealyet_");
    assert_eq!(wire.names(), vec!["noverisp3", "notrealyet_"]);
}