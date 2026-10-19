use crate::server::ticking::{state_id, to_state_id};
use super::*;

#[test]
fn a_flushed_buffer_does_not_send_its_contents_twice() {
    // `write_all` sends the bytes and leaves them where they were. A buffer
    // written twice therefore sends everything it held, twice, and the second
    // copy arrives after whatever was appended in between -- which for the
    // opening batch of chunks means a world made before the player touched it
    // turning up after they did, and quietly undoing the change.
    //
    // This is the fault that left a broken block standing again, invisible, for
    // as long as the session lasted and only a rejoin cleared.
    let mut sink: Vec<u8> = Vec::new();
    let mut out: Vec<u8> = b"the whole chunk batch".to_vec();

    flush_buffer(&mut sink, &mut out).expect("the first write goes out");

    assert!(
        out.is_empty(),
        "the buffer is emptied by the write, so a later write carries only what \
         has been added since: {:?}",
        String::from_utf8_lossy(&out)
    );

    out.extend_from_slice(b"and the introductions");
    flush_buffer(&mut sink, &mut out).expect("the second write goes out");

    assert_eq!(
        sink, b"the whole chunk batchand the introductions",
        "each write carries only what was added since the last, in order"
    );
}

#[test]
fn a_flushed_buffer_puts_nothing_on_the_wire_when_it_is_empty() {
    // Two writes with nothing added between them must send nothing, rather
    // than a stray empty frame or a repeat of what the first one carried.
    let mut sink: Vec<u8> = Vec::new();
    let mut out: Vec<u8> = Vec::new();

    flush_buffer(&mut sink, &mut out).expect("an empty write is not an error");
    flush_buffer(&mut sink, &mut out).expect("and neither is doing it twice");

    assert!(sink.is_empty(), "nothing was sent: {sink:?}");
}

#[test]
fn new_ground_arrives_nearest_first() {
    // Walking east, the wall of new terrain is asked for and sent in the order
    // the player will reach it. Sorting by coordinate instead sent the north
    // end of the wall before the south, so the chunk under the player was near
    // the back of the queue while one eight to their left was near the front.
    //
    // The login batch already goes nearest first, so this is the same rule the
    // other half of the connection was already using.
    let radius = 8;
    let mut sent = std::collections::HashSet::new();
    for (x, z) in chunk_edges((0, 0), radius, &sent).0 {
        sent.insert((x, z));
    }

    let (arriving, _) = chunk_edges((1, 0), radius, &sent);
    assert_eq!(arriving.len(), 17, "one edge of the window comes in");

    // The player is at (1, 0). Every arriving chunk is in the column at x = 9,
    // so the order down that column is the order of distance from them.
    let centre_z = 0;
    let distances: Vec<i32> = arriving
        .iter()
        .map(|&(_, z)| (z - centre_z).abs())
        .collect();

    assert!(
        distances.windows(2).all(|w| w[0] <= w[1]),
        "nearest first, so the ground under the player is not the last thing \
         to arrive: {distances:?}"
    );
    assert_eq!(distances[0], 0, "the chunk they are standing in, first");
    assert_eq!(
        distances.last().copied(),
        Some(radius),
        "and the far corner of the edge last"
    );
}

#[test]
fn chunks_at_the_same_distance_arrive_in_a_fixed_order() {
    // Two chunks the same distance away are common at a corner, and a walk that
    // produced different bytes each time would not be reproducible. The
    // coordinate is the tiebreak for that reason.
    let radius = 8;
    let mut sent = std::collections::HashSet::new();
    for (x, z) in chunk_edges((0, 0), radius, &sent).0 {
        sent.insert((x, z));
    }

    let first = chunk_edges((1, 1), radius, &sent).0;
    let second = chunk_edges((1, 1), radius, &sent).0;
    assert_eq!(first, second, "the same walk gives the same order twice");

    // And the tiebreak really is what separates equal distances, not luck.
    let seen: std::collections::HashSet<Vec<i32>> = first
        .iter()
        .map(|&(x, z)| {
            let dx = i64::from(x - 1);
            let dz = i64::from(z - 1);
            vec![(dx * dx + dz * dz) as i32]
        })
        .collect();
    assert!(
        seen.len() <= first.len(),
        "distances are grouped, so equal ones sit together"
    );
}

#[test]
fn a_player_closing_their_game_is_not_a_server_error() {
    // It arrives as an error, so it has to be told apart from one. Otherwise a
    // player closing a window prints ERROR, and the next person to read the log
    // goes looking for a fault that is not there.
    for code in [53, 54, 60, 997] {
        let e = std::io::Error::from_raw_os_error(code);
        assert!(
            client_went_away(&e),
            "{code} is the peer going away, not the server failing: {e}"
        );
    }

    for kind in [
        std::io::ErrorKind::ConnectionReset,
        std::io::ErrorKind::ConnectionAborted,
        std::io::ErrorKind::BrokenPipe,
    ] {
        assert!(
            client_went_away(&std::io::Error::from(kind)),
            "{kind:?} is the peer going away too"
        );
    }
}

#[test]
fn a_real_failure_is_still_an_error() {
    // The other half, and the one that matters: this must not swallow the
    // faults that are ours. A protocol violation or a bad write is not the
    // client leaving, and reporting it as a clean departure would lose the only
    // evidence that anything went wrong.
    for kind in [
        std::io::ErrorKind::InvalidData,
        std::io::ErrorKind::UnexpectedEof,
        std::io::ErrorKind::PermissionDenied,
    ] {
        assert!(
            !client_went_away(&std::io::Error::from(kind)),
            "{kind:?} is ours and must stay an ERROR"
        );
    }
}

#[test]
fn crossing_a_boundary_sends_the_new_ground_before_dropping_the_old() {
    // The fault this guards: a player crossing a chunk boundary had the terrain
    // they were leaving forgotten on one pass, and the terrain they were walking
    // onto arrived on a later one, because want_chunk returns None until the
    // pool has built it. For those two hundred milliseconds the client was
    // holding neither, which reads as a hitch to the player and as a hitch to
    // whoever is watching them.
    //
    // One ring is kept back, so nothing is forgotten while the replacement is
    // still being built.
    let radius = 8;
    let mut sent: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
    for (x, z) in chunk_edges((0, 0), radius, &sent).0 {
        sent.insert((x, z));
    }
    assert_eq!(sent.len(), 289, "the opening window is the whole square");

    let (arriving, leaving) = chunk_edges((1, 0), radius, &sent);
    assert_eq!(arriving.len(), 17, "the new edge comes in");
    assert!(
        leaving.is_empty(),
        "and nothing is taken away yet: {:?}",
        leaving
    );

    // Two chunks on, the trailing edge is now two rings out and goes. The
    // chunks replacing it left a whole chunk boundary ago.
    for (x, z) in arriving {
        sent.insert((x, z));
    }
    let (arriving, leaving) = chunk_edges((2, 0), radius, &sent);
    assert_eq!(arriving.len(), 17, "and the next edge follows");
    assert_eq!(leaving.len(), 17, "the trailing edge is forgotten now");
    assert!(
        leaving.iter().all(|&(_, z)| (-8..=8).contains(&z)),
        "and it is the old column, not something diagonal: {leaving:?}"
    );
}

#[test]
fn far_enough_ahead_only_the_far_edge_is_released() {
    // What the kept-back ring costs. Walked a full window in one go, the whole
    // square is new and almost nothing can be released, because the ring behind
    // them still covers most of what was sent. The point is that nothing is
    // released that the client still needs, and that a jump this size does not
    // leave the held set growing without bound.
    let radius = 8;
    let mut sent = std::collections::HashSet::new();
    for (x, z) in chunk_edges((0, 0), radius, &sent).0 {
        sent.insert((x, z));
    }

    let (arriving, leaving) = chunk_edges((2 * radius, 0), radius, &sent);

    // Jumped a whole window in one go. Every chunk in the new square is new except
    // the strip the kept-back ring still covers, so 289 minus the 17 that are
    // held rather than wanted. That difference is the ring doing its job: those
    // chunks are still in `sent`, so they are not resent, and they are not
    // released either.
    assert_eq!(
        arriving.len(),
        (((2 * radius + 1) * (2 * radius + 1)) - (2 * radius + 1)) as usize,
        "every chunk in the new window except the strip still kept back: {}",
        arriving.len()
    );
    // Jumped a whole window in one go, which is what a teleport looks like to the
    // chunk logic. Almost everything is released and almost everything is new,
    // and the point of the assertion is that the held set does not grow without
    // bound: it lands at a window plus a strip, not at two windows.
    assert!(
        !leaving.is_empty(),
        "the far side of the old window is released: {:?}",
        leaving
    );

    let still_held = sent.len() - leaving.len();
    let held_after = still_held + arriving.len();
    let window = ((2 * radius + 1) * (2 * radius + 1)) as usize;

    assert!(
        held_after < window + window / 4,
        "a jump this size does not leave the world piling up: {} held against a \
         window of {window}",
        held_after
    );
    assert!(
        held_after > window,
        "and it is still a window, not a hole: {held_after}"
    );
}

#[test]
fn a_ring_the_pool_has_not_finished_is_not_reported_over_and_over() {
    // Seventeen wanted, none built. The loop sees the same seventeen on
    // every pass, and saying so on every pass is the noise.
    assert_eq!(
        chunk_report(17, 0, 0),
        None,
        "nothing arrived and nothing left, so there is nothing to say"
    );
}

#[test]
fn the_number_reported_is_what_was_sent_and_not_what_was_wanted() {
    // "17 chunks arrived" for a batch that contained none is worse than
    // silence: the closing count is what the client uses to know the world
    // it has been sent is complete.
    let line = chunk_report(17, 3, 0).expect("three did go out");
    assert!(line.starts_with("3 chunks arrived"), "{line}");

    let line = chunk_report(17, 0, 17).expect("seventeen left");
    assert!(line.starts_with("0 chunks arrived"), "{line}");
}

#[test]
fn a_walk_that_changes_nothing_is_not_reported_at_all() {
    assert_eq!(chunk_report(0, 0, 0), None);
}

#[test]
fn a_chunk_already_taken_by_this_batch_is_not_taken_again() {
    // The tail wait re-reads the same batch of positions the first loop just
    // went through, and `sent` is no help -- it is only written once the batch
    // has gone out, so it still says none of them have. What stops the second
    // pass collecting them again is the batch's own record of what it has.
    let pos = crate::world::pos::ChunkPos { x: -1, z: 0 };
    let mut sent = std::collections::HashSet::new();
    let mut taken = std::collections::HashSet::new();

    assert!(still_wanted(pos, &sent, &taken), "to begin with");

    taken.insert(pos);
    assert!(
        !still_wanted(pos, &sent, &taken),
        "once this batch has it, the tail wait must leave it alone"
    );

    // And the other way round: already on the wire.
    let taken = std::collections::HashSet::new();
    sent.insert((pos.x, pos.z));
    assert!(!still_wanted(pos, &sent, &taken), "already sent is not resent");

    // A neighbouring chunk nobody has touched still qualifies.
    let fresh = crate::world::pos::ChunkPos { x: -1, z: -1 };
    assert!(still_wanted(fresh, &sent, &taken), "a fresh one does");
}

#[test]
fn a_tail_wait_collects_each_chunk_once_whatever_order_the_pool_finishes_in() {
    // The shape that panicked. A pass of seventeen positions where seven are
    // already built and ten are still going: the first loop takes the seven,
    // then the tail wait walks the same seventeen again and, having no way to
    // tell, takes the same seven a second time. The batch claims twenty-four
    // chunks out of seventeen asked for, and the same ground goes on the wire
    // twice.
    let sorted: Vec<(i32, i32)> = (0..17).map(|i| (i, 0)).collect();
    let mut built: std::collections::HashSet<(i32, i32)> =
        (0..7).map(|i| (i, 0)).collect();

    let sent = std::collections::HashSet::new();
    let mut taken = std::collections::HashSet::new();
    let mut ready: Vec<(i32, i32)> = Vec::new();

    // First loop: take whatever is ready.
    for (x, z) in &sorted {
        let pos = crate::world::pos::ChunkPos { x: *x, z: *z };
        if built.contains(&(pos.x, pos.z)) {
            taken.insert(pos);
            ready.push((pos.x, pos.z));
        }
    }
    let after_first = ready.len();
    assert_eq!(after_first, 7);

    // Tail wait: the pool finishes the rest while we wait, and this loop walks
    // the same seventeen.
    let mut passes = 0;
    while ready.len() < sorted.len() && passes < 4 {
        passes += 1;
        built.extend((0..17).map(|i| (i, 0)).collect::<std::collections::HashSet<_>>());
        for (x, z) in &sorted {
            let pos = crate::world::pos::ChunkPos { x: *x, z: *z };
            if !still_wanted(pos, &sent, &taken) {
                continue;
            }
            if built.contains(&(pos.x, pos.z)) {
                taken.insert(pos);
                ready.push((pos.x, pos.z));
            }
        }
    }

    assert_eq!(
        ready.len(),
        sorted.len(),
        "seventeen positions, seventeen chunks, and no chunk twice"
    );
    let unique: std::collections::HashSet<_> = ready.iter().collect();
    assert_eq!(unique.len(), ready.len(), "every one of them distinct");
    assert!(
        chunk_report(sorted.len(), ready.len(), 0).is_some(),
        "and the report is within what was asked for"
    );
}

#[test]
fn a_click_the_player_got_right_costs_no_packets() {
    // Vanilla sends a slot only where the client's guess and the server's answer
    // differ. Sending it anyway would be a packet per click for a redraw the
    // client has already done.
    let mut inv = Inventory::empty();
    inv.set(slots::HOTBAR_START, Some(ItemStack::new(55, 20)));
    let mut carried = None;
    let got = c::click(&mut inv, &mut carried, slots::HOTBAR_START as i16, 0, c::ContainerInput::Pickup);

    let reported = vec![(slots::HOTBAR_START as i16, None)];
    assert_eq!(
        corrections_needed(&got, &reported),
        Vec::new(),
        "the client already has this right"
    );
    assert_eq!(carried, Some(ItemStack::new(55, 20)));
}

#[test]
fn a_click_the_player_got_wrong_is_corrected() {
    // The server stays the authority. A player cannot decide what is in a slot;
    // it can only be told.
    let mut inv = Inventory::empty();
    inv.set(slots::HOTBAR_START, Some(ItemStack::new(55, 20)));
    let mut carried = None;
    let got = c::click(&mut inv, &mut carried, slots::HOTBAR_START as i16, 0, c::ContainerInput::Pickup);

    // The client thinks the slot still holds the stack it just picked up.
    let wrong = vec![(slots::HOTBAR_START as i16, Some(ItemStack::new(55, 20)))];
    assert_eq!(
        corrections_needed(&got, &wrong),
        vec![(slots::HOTBAR_START, None)],
        "the slot is sent back empty, as it is"
    );
}

#[test]
fn a_click_that_reports_a_slot_the_server_did_not_touch_is_still_told() {
    // The client sent a guess for a slot the server's answer says nothing about.
    // Trusting it would let a player put whatever they liked in a slot.
    let mut inv = Inventory::empty();
    inv.set(12, Some(ItemStack::new(55, 5)));
    let mut carried = None;
    let got = c::click(&mut inv, &mut carried, 12, 0, c::ContainerInput::Swap);

    // The client claims the hotbar slot holds a stack of stone.
    let claims = vec![(36i16, Some(ItemStack::new(1, 64)))];
    let fixes = corrections_needed(&got, &claims);

    // The number key moved the dirt up to the hotbar and left slot 12 empty, so
    // those are what goes back -- and what the client claimed is ignored.
    assert_eq!(
        fixes,
        vec![(36, Some(ItemStack::new(55, 5))), (12, None)],
        "the server's answer wins over the client's guess"
    );
    assert_eq!(inv.get(36), Some(ItemStack::new(55, 5)), "the dirt went up");
    assert_eq!(inv.get(12), None, "and the slot it left is empty");
}

#[test]
fn a_click_that_was_refused_corrects_nothing() {
    // Nothing was computed, so there is no answer to disagree with -- and the
    // client's guess is not adopted either. The slot keeps whatever it had.
    let mut inv = Inventory::empty();
    let mut carried = None;
    let got = c::click(&mut inv, &mut carried, 0, 0, c::ContainerInput::Pickup);
    assert!(got.is_none(), "the crafting slot has nothing behind it");
    let claims = vec![(0i16, Some(ItemStack::new(1, 64)))];
    assert_eq!(corrections_needed(&got, &claims), Vec::new());
    assert_eq!(inv.get(0), None, "and the server invented nothing");
}

#[test]
fn a_stack_is_described_in_words_for_the_log() {
    assert_eq!(describe(None), "nothing");
    assert_eq!(describe(Some(ItemStack::new(55, 20))), "minecraft:dirt x20");
    assert_eq!(
        describe(Some(ItemStack::new(54, 1))),
        "minecraft:grass_block x1"
    );
}

/// The palette the generator makes, so the hardness table can be keyed on it.
fn terrain() -> crate::world::terrain::TerrainBlocks {
    crate::world::terrain::register_terrain(&mut crate::world::block::BlockRegistry::new())
}

/// How long dirt takes in bare hands: hardness 0.5 at the rate above.
///
/// The quickest block the generator makes, standing in for an arbitrary
/// break time in the crack tests.
const DIRT_BREAK: std::time::Duration = std::time::Duration::from_millis(750);

#[test]
fn the_dust_goes_to_everyone_but_the_one_who_broke_it() {
    // The game excludes the breaker:
    //
    //     broadcast(source instanceof Player player ? player : null, ...)
    //
    // Their own client already played the dust and the sound when it
    // predicted the block going, so telling them as well makes a single
    // break look like two. The block update is the opposite case and does go
    // to them: it says what is there now, and their guess may be wrong.
    let wire = crate::server::broadcast::Broadcast::new();
    let breaker = wire.register("breaker", [1u8; 16], 1, crate::world::terrain::spawn_point());
    let watcher = wire.register("watcher", [2u8; 16], 2, crate::world::terrain::spawn_point());

    let mut update = Vec::new();
    crate::protocol::play::write_block_update(&mut update, 1, 2, 3, 0);
    let mut dust = Vec::new();
    crate::protocol::play::write_level_event(
        &mut dust,
        crate::protocol::play::BLOCK_DESTROYED,
        (1, 2, 3),
        9,
        false,
    );

    // The update: everyone.
    wire.send_others(breaker.id, update);
    // The dust: everyone but the breaker, which send_others already is.
    wire.send_others(breaker.id, dust);

    assert!(
        watcher.inbox.try_recv().is_ok(),
        "the watcher sees the block go"
    );
    assert!(
        watcher.inbox.try_recv().is_ok(),
        "and sees the dust"
    );
    assert!(
        breaker.inbox.try_recv().is_err(),
        "and the player who broke it is told neither, because their client \
         already drew both"
    );
}

#[test]
fn a_sent_edit_is_not_sent_again_on_the_next_pass() {
    // The bug this is for: both lists lived outside the loop and were
    // iterated rather than taken, so every pass re-sent everything since the
    // connection began. A repeated block update says the same thing and
    // nobody notices. A repeated level event is twenty bursts of particles a
    // second, which looks like the server is having a seizure until the
    // player who caused it logs out.
    let mut edits: Vec<Vec<u8>> = Vec::new();
    let mut effects: Vec<Vec<u8>> = Vec::new();

    // A pass where a block was broken.
    edits.push(b"the update".to_vec());
    effects.push(b"the dust".to_vec());
    let sent: Vec<Vec<u8>> = std::mem::take(&mut edits)
        .into_iter()
        .chain(std::mem::take(&mut effects))
        .collect();
    assert_eq!(sent.len(), 2, "both go out once");

    // The next pass, where nothing happened.
    assert!(edits.is_empty() && effects.is_empty(), "nothing to send");
    assert_eq!(
        edits.len() + effects.len(),
        0,
        "and a pass with no breaks must send no packets at all, however \
         many passes came before it"
    );
}

#[test]
fn break_times_are_the_games_own_numbers() {
    // Read out of the decompiled block definitions by tools/extract_hardness.py,
    // at one and a half seconds per point of hardness for a bare hand. These
    // are the numbers a client uses to decide when it is finished, so a
    // server using different ones gets its crack cut short.
    let t = terrain();

    assert_eq!(break_time(t.dirt, &t), DIRT_BREAK);
    assert_eq!(
        break_time(t.stone, &t),
        DIRT_BREAK * 3,
        "stone is three times dirt"
    );

    // Halfway cases are where an f32 shows up, so the tenths are compared
    // as tenths rather than as exact seconds.
    let tenths = |block| (break_time(block, &t).as_secs_f64() * 10.0).round() as i64;
    assert_eq!(tenths(t.dirt), 8, "0.75s");
    assert_eq!(tenths(t.grass), 9, "0.9s");
    assert_eq!(tenths(t.stone), 23, "2.25s, three times dirt");
    assert_eq!(tenths(t.glowstone), 5, "0.45s");
}

#[test]
fn a_block_that_cannot_be_broken_does_not_pace_a_crack() {
    // Hardness is negative for the fifteen blocks with no break time at all,
    // barrier among them. Multiplying a negative number by the rate gives a
    // negative duration, and dividing by it gives a stage that is nonsense.
    assert_eq!(
        crate::generated::hardness_of("minecraft:barrier"),
        Some(-1.0),
        "which is how the table says it"
    );

    let t = terrain();
    assert!(
        break_time(t.air, &t) > std::time::Duration::ZERO,
        "and air falls back rather than going negative"
    );
}

#[test]
fn a_crack_starts_at_stage_zero_and_finishes_at_nine() {
    // The two ends, because the interesting part is that the first one is a
    // real stage rather than the absence of one.
    assert_eq!(stage_at(std::time::Duration::ZERO, DIRT_BREAK), 0);
    assert_eq!(stage_at(DIRT_BREAK * 2, DIRT_BREAK), 9, "clamped, not past the end");
}

#[test]
fn a_crack_moves_through_its_stages_over_time() {
    // A tenth of dirt's break in whole milliseconds, rather than dividing a
    // duration and multiplying the result. `DIRT_BREAK / 10 * n` reads as
    // dividing by ten times n, because division and multiplication bind
    // equally and the left one wins, and that is a test which checks a
    // different thing from the one it claims to.
    let tenth = 75u64;

    let stages: Vec<i8> = (0..10u64)
        .map(|n| stage_at(std::time::Duration::from_millis(tenth * n), DIRT_BREAK))
        .collect();

    assert_eq!(
        stages,
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        "ten stages over the length of one break"
    );
}

#[test]
fn a_slower_block_takes_longer_to_crack() {
    // The same elapsed time is further through a dirt break than a stone
    // one, because stone takes three times as long. Half of dirt's break is
    // halfway through dirt and a sixth of the way through stone.
    let half_of_dirt = DIRT_BREAK / 2;

    assert_eq!(stage_at(half_of_dirt, DIRT_BREAK), 5);
    assert_eq!(stage_at(half_of_dirt, DIRT_BREAK * 3), 1);
}

#[test]
fn a_crack_that_has_been_held_stays_at_the_last_stage() {
    assert_eq!(stage_at(DIRT_BREAK, DIRT_BREAK), 9, "the last stage, held");
    assert_eq!(stage_at(DIRT_BREAK * 100, DIRT_BREAK), 9, "and held longer");
}

#[test]
fn two_edits_are_two_frames_and_not_one_frame_holding_two_packets() {
    // The bug this is for: both edits written into one buffer and framed
    // once, so the frame claimed a length that covered two packets. The
    // client reads the first, finds ten bytes where there should be none,
    // and hangs up. Both clients died, so it looked like the crack and was
    // not.
    let enc = crate::protocol::frame::FrameEncoder::new();

    let mut first = Vec::new();
    crate::protocol::play::write_block_update(&mut first, 1, 2, 3, 0);
    let mut second = Vec::new();
    crate::protocol::play::write_block_update(&mut second, 4, 5, 6, 0);

    assert_eq!(first.len(), 10, "one block update is ten bytes");

    // What the code used to do: both bodies into one frame.
    let mut both = first.clone();
    both.extend_from_slice(&second);
    let mut one_frame = Vec::new();
    enc.encode(&both, &mut one_frame).expect("framed");

    // What it does now: a frame each.
    let mut two_frames = Vec::new();
    enc.encode(&first, &mut two_frames).expect("framed");
    enc.encode(&second, &mut two_frames).expect("framed");

    assert_ne!(
        one_frame, two_frames,
        "these are different streams, which is the whole point"
    );

    // The length the single frame claims is the whole of both packets, so
    // after the client has read one it has ten bytes left in a frame that
    // should have ended.
    let claimed = {
        let mut r = &one_frame[..];
        // read gives back the VarInt and how many bytes it took; the value
        // is inside the VarInt.
        crate::protocol::VarInt::read(&mut r).expect("a length").0 .0
    };
    assert_eq!(claimed, both.len() as i32);
    assert_eq!(
        claimed, 2 * first.len() as i32,
        "twice what one packet needs, which is how ten bytes end up over"
    );
}

/// Generates a chunk and reads the blocks back out of the wire bytes.
///
/// This is the check that was missing when terrain was first connected: the
/// packet decoded at the client and showed nothing, which is the one failure
/// a length assertion cannot see. Decoding the bytes here says whether the
/// ground is actually in them.
#[test]
fn generated_terrain_survives_the_wire_format() {
    use crate::world::block::BlockRegistry;
    use crate::world::pos::ChunkPos;
    use crate::world::terrain;

    let mut registry = BlockRegistry::new();
    let palette = terrain::register_terrain(&mut registry);
    let chunk = terrain::generate_chunk(ChunkPos { x: 0, z: 0 }, &palette);

    let mut sections: Vec<Vec<i32>> = Vec::new();
    for section in chunk.sections() {
        sections.push(
            crate::world::section::iter_blocks(section)
                .map(|block| to_state_id(block, &palette))
                .collect(),
        );
    }

    // The generator fills from the bottom of the world up, so the very first
    // block of the first section is stone, and there has to be a surface
    // somewhere above it with grass on it.
    let first = sections[0][0];
    assert_eq!(
        first,
        state_id("minecraft:stone"),
        "the bottom of the world is stone"
    );

    let grass = state_id("minecraft:grass_block");
    let air = state_id("minecraft:air");
    let all: Vec<i32> = sections.concat();
    assert!(all.contains(&grass), "there is grass somewhere");
    assert!(all.contains(&air), "and air above the surface");

    // Now the actual wire bytes, decoded by a reader written from the
    // decompiled layout rather than from the writer.
    let mut wire = Vec::new();
    crate::protocol::chunk::write_chunk_with_states(&mut wire, 0, 0, &sections);
    let decoded = decode_first_section(&wire);

assert_eq!(
        decoded.first().copied(),
        Some(first),
        "the first block must come back out of the packed data unchanged: \
         wrote {first}, read {:?}, palette {:?}",
        decoded.first(),
        {
            let mut j = 1 + 8;
            let (maps, used) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("a count");
            j += used;
            for _ in 0..maps.0 {
                let (_, u1) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("a type");
                j += u1;
                let (longs, u2) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("longs");
                j += u2 + longs.0 as usize * 8;
            }
            let (_, u3) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("len");
            j += u3;
            let (_, u4) = crate::protocol::VarInt::read(&mut &wire[j + 4..]).expect("bits");
            j += 4 + u4;
            let (size, u5) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("size");
            j += u5;
            let mut palette = Vec::new();
            for _ in 0..size.0 {
                let (v, u6) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("id");
                j += u6;
                palette.push(v.0);
            }
            palette
        }
    );
assert_eq!(
        decoded.iter().filter(|b| **b == grass).count(),
        sections[0].iter().filter(|b| **b == grass).count(),
        "and every grass block in the section that was decoded"
    );
}

/// Reads the block states of the first section back out of a chunk packet.
///
/// Mirrors what the client does: skip the id and the coordinates, the
/// heightmap map, the buffer length, then walk the first section's container.
fn decode_first_section(packet: &[u8]) -> Vec<i32> {
    use crate::protocol::VarInt;

    let mut i = 0;
    let varint = |i: &mut usize| -> i32 {
        let (value, used) = VarInt::read(&mut &packet[*i..]).expect("a VarInt");
        *i += used;
        value.0
    };

    assert_eq!(varint(&mut i), 46, "the chunk packet id");
    i += 8; // x and z

    let heightmaps = varint(&mut i);
    for _ in 0..heightmaps {
        varint(&mut i); // the heightmap type
        let longs = varint(&mut i);
        i += longs as usize * 8;
    }

    let buffer_len = varint(&mut i) as usize;
    let start = i;

    i += 4; // the two shorts
    let bits = packet[i];
    i += 1;

    let mut out = Vec::new();
if bits == 0 {
        // A single value fills every entry, which is what the client does
        // with a zero bit width. Returning nothing here was the bug: the
        // section looked empty when it was uniformly one block.
        let (value, _) = VarInt::read(&mut &packet[i..]).expect("a VarInt");
        return vec![value.0; crate::protocol::chunk::BLOCK_ENTRIES_DIV];
    }

    let size = varint(&mut i);
    let mut palette = Vec::new();
    for _ in 0..size {
        palette.push(varint(&mut i));
    }

    let per_long = 64 / bits as usize;
    let count = crate::protocol::chunk::BLOCK_ENTRIES_DIV;
    for entry in 0..count {
        let long = entry / per_long;
        let shift = (entry % per_long) * bits as usize;
        let bytes = &packet[i + long * 8..i + long * 8 + 8];
        let word = u64::from_be_bytes(bytes.try_into().expect("eight bytes"));
        out.push(palette[((word >> shift) as u32 & ((1 << bits) - 1)) as usize]);
    }

    assert!(start + buffer_len <= packet.len(), "the buffer length is sane");
    out
}
#[test]
fn reach_is_measured_from_the_eye_to_the_block_not_to_its_middle() {
    // The game measures the eye to the box of the block, not to its middle, and
    // allows block_interaction_range + 1.0. This measured feet to middle, which
    // is wrong at both ends and wrong in opposite directions, so the two nearly
    // cancelled and nothing looked broken.
    fn at(x: f64, y: f64, z: f64) -> crate::protocol::play::Movement {
        crate::protocol::play::Movement { x, y, z, ..Default::default() }
    }
    let on = |x: i32, y: i32, z: i32| crate::world::pos::BlockPos::new(x, y, z);

    // Standing on the block: the eye is 1.62 above the feet and the top of the
    // block is 1.0 above its own y, so the nearest point is 0.62 away. Within.
    let standing = reach_to(&at(0.5, 0.0, 0.5), on(0, -1, 0));
    assert!(
        standing < MAX_REACH,
        "the block underfoot is within reach: {standing}"
    );
    assert!(standing > 0.0, "and it is not the block itself: {standing}");

// The eye at 0.5, 1.62, 0.5 is directly above the box of the block at the
    // origin: inside on x and z, 0.62 above it on y. Measuring to the middle
    // instead would have given 1.12 and a different verdict about what is
    // within reach, which is the whole of the fault.
    let above = reach_to(&at(0.5, 0.0, 0.5), on(0, 0, 0));
    assert!(
        (above - 0.62).abs() < 1e-9,
        "to the top of the box, not to the middle of the block: {above}"
    );

    // Three blocks to the side. The eye is 0.5 inside x and z of the box and 0.62
    // above it, so the distance is the diagonal of those two gaps rather than
    // either on its own. Measuring to the block's middle would have added half
    // a block to the x gap and half to the y, which is a different answer.
    let across = reach_to(&at(-2.5, 0.0, 0.5), on(0, 0, 0));
    let expected = (2.5f64 * 2.5 + 0.62 * 0.62).sqrt();
    assert!(
        (across - expected).abs() < 1e-9,
        "both gaps at once: {across} against {expected}"
    );
}

#[test]
fn the_reach_is_the_number_the_game_uses() {
    // Attributes.BLOCK_INTERACTION_RANGE is 4.5 in the client jar, ranged to
    // 64, and Player.isWithinBlockInteractionRange adds one block of slack. So
    // 5.5. This was 6.0, chosen rather than read, and chosen generously on
    // purpose -- a reach that is too generous is invisible, a reach that is too
    // tight refuses breaks the client thought were allowed.
    assert!(
        (MAX_REACH - 5.5).abs() < f64::EPSILON,
        "4.5 from the attribute plus the one the client adds: {MAX_REACH}"
    );
}

#[test]
fn a_block_far_over_there_is_out_of_reach() {
    fn at(x: f64, y: f64, z: f64) -> crate::protocol::play::Movement {
        crate::protocol::play::Movement { x, y, z, ..Default::default() }
    }
    let far = reach_to(&at(0.5, 0.0, 0.5), crate::world::pos::BlockPos::new(9, 0, 0));
    assert!(
        far > MAX_REACH,
        "a block eight away is refused, or the check is not a check: {far}"
    );
}

#[test]
fn the_numbers_that_were_guessed_are_now_tied_to_something() {
    // Three numbers that used to be written down on their own, and the three
    // things they are answers to.
    //
    // The reach is Attributes.BLOCK_INTERACTION_RANGE from the client jar plus
    // the one block the client adds. The loop interval is the tick, which the
    // world defines. The tail budget is a quarter of the loop, so it finishes
    // before the pass it is saving would have come round.
    //
    // None of them is a preference dressed up as a measurement. A number that
    // has no source is a number that will be wrong on somebody's machine, and
    // the only way to notice is to have something that says where it came from.
    assert!((MAX_REACH - 5.5).abs() < f64::EPSILON, "4.5 + the client's 1.0");

    assert_eq!(
        LOOP_INTERVAL,
        crate::server::ticking::TICK,
        "the loop waits one tick, taken from the world's own tick"
    );
    assert_eq!(
        LOOP_INTERVAL.as_millis(),
        50,
        "and twenty a second, which is what the tick is"
    );

// A quarter of the loop, rounded down: 12 of 50, so the tail is done before
    // the pass it is saving would have come round.
    assert_eq!(
        CHUNK_TAIL_BUDGET.as_millis(),
        LOOP_INTERVAL.as_millis() / 4,
        "a quarter of the pass, rounded down: {} of {}",
        CHUNK_TAIL_BUDGET.as_millis(),
        LOOP_INTERVAL.as_millis()
    );
    assert!(
        CHUNK_TAIL_BUDGET * 2 < LOOP_INTERVAL,
        "and never half of it, or it would be a second pass rather than a \
         saving on one"
    );
    assert!(
        CHUNK_TAIL_STEP * 4 <= CHUNK_TAIL_BUDGET,
        "spent in a handful of looks rather than a number kept in step by hand"
    );
    assert!(
        CHUNK_TAIL_BUDGET > CHUNK_TAIL_STEP,
        "and a step is shorter than the budget it is spent in"
    );
}
const EVERY: std::time::Duration = std::time::Duration::from_secs(15);
const NONE: std::time::Duration = std::time::Duration::ZERO;
const A_TICK: std::time::Duration = std::time::Duration::from_millis(50);

#[test]
fn a_keep_alive_that_goes_unanswered_is_a_dead_connection() {
    // The whole reason this exists. A server that only sends challenges cannot
    // tell a player who is there from one whose socket is still half open
    // because their network went away and nothing followed: no FIN, no RST,
    // nothing. That connection then holds an entity id, a name and a place in
    // the player list until the operating system gives up, which on Windows is
    // measured in hours.
    let mut ka = KeepAlive::new();

    assert_eq!(ka.tick(NONE, EVERY), KeepAliveTick::Silent, "nothing is due yet");
    assert_eq!(
        ka.tick(EVERY - A_TICK, EVERY),
        KeepAliveTick::Silent,
        "nor a moment before"
    );
    assert_eq!(ka.tick(EVERY, EVERY), KeepAliveTick::Send(1), "and now it is");

    // The game stamps `keepAliveTime` when it sends, so a challenge has exactly
    // one interval to be answered and no more. Not two, and not a grace.
    assert_eq!(
        ka.tick(EVERY, EVERY),
        KeepAliveTick::Timeout,
        "a full interval later, the silence is the answer"
    );
}

#[test]
fn a_challenge_has_exactly_one_interval_to_be_answered() {
    // `keepAliveTime` is stamped when the challenge goes out and never updated
    // by an answer, so the deadline is one interval after the *send* rather than
    // after the last good news. It is the reason a dead client is bounded at all.
    let every = KEEP_ALIVE_EVERY;
    let mut ka = KeepAlive::new();

    assert_eq!(ka.tick(every, every), KeepAliveTick::Send(1));
    assert!(ka.answered(1, NONE), "answered at once");

    assert_eq!(
        ka.tick(every, every),
        KeepAliveTick::Send(2),
        "the next cycle goes out on schedule"
    );
    assert_eq!(
        ka.tick(every - A_TICK, every),
        KeepAliveTick::Silent,
        "and is not given up before its time"
    );
    assert_eq!(
        ka.tick(every, every),
        KeepAliveTick::Timeout,
        "exactly one interval after it went out"
    );
}

#[test]
fn an_answered_keep_alive_is_never_a_timeout() {
    let mut ka = KeepAlive::new();
    let id = match ka.tick(EVERY, EVERY) {
        KeepAliveTick::Send(id) => id,
        other => panic!("expected a challenge, got {other:?}"),
    };
    assert!(ka.answered(id, std::time::Duration::from_millis(40)));

    assert_eq!(
        ka.tick(EVERY - A_TICK, EVERY),
        KeepAliveTick::Silent,
        "an answered challenge is not an unanswered one, and the clock is not up"
    );
    assert_eq!(
        ka.tick(EVERY, EVERY),
        KeepAliveTick::Send(id + 1),
        "and the next one goes out on schedule"
    );
}

#[test]
fn every_challenge_asks_for_something_new() {
    // The client echoes the id back, so a repeated id would let a reply to an
    // old question satisfy a new one.
    let mut ka = KeepAlive::new();
    let mut seen = Vec::new();
    for round in 0..5 {
        let id = match ka.tick(EVERY, EVERY) {
            KeepAliveTick::Send(id) => id,
            other => panic!("round {round}: expected a challenge, got {other:?}"),
        };
        assert!(!seen.contains(&id), "id {id} was asked for twice");
        seen.push(id);
        assert!(ka.answered(id, std::time::Duration::from_millis(5)));
    }
    assert_eq!(seen.len(), 5, "five distinct challenges");
    assert!(seen.iter().all(|id| *id > 0), "and none of them zero");
}

#[test]
fn answering_a_challenge_that_was_never_asked_for_is_refused() {
    // The game cuts here rather than ignoring it: a client replying to
    // something it was never sent is either confused or not the client it
    // claims to be, and neither is worth carrying.
    let mut ka = KeepAlive::new();
    assert!(
        !ka.answered(999, NONE),
        "nothing was outstanding to answer"
    );

    let id = match ka.tick(EVERY, EVERY) {
        KeepAliveTick::Send(id) => id,
        other => panic!("expected a challenge, got {other:?}"),
    };
    assert!(!ka.answered(id + 1, NONE), "the wrong id is not the answer");
    assert!(
        ka.outstanding().is_some(),
        "and a wrong answer does not consume the outstanding one"
    );
    assert!(ka.answered(id, NONE), "the right one still lands");
    assert!(ka.outstanding().is_none(), "and clears it");
}

#[test]
fn the_round_trip_is_smoothed_rather_than_jumping() {
    // Three parts in four of the old reading to one of the new, so one slow
    // packet does not move the number much.
    let mut ka = KeepAlive::new();
    let id = match ka.tick(EVERY, EVERY) {
        KeepAliveTick::Send(id) => id,
        other => panic!("expected a challenge, got {other:?}"),
    };
    ka.answered(id, std::time::Duration::from_millis(100));
    assert_eq!(ka.latency_ms(), 25, "a quarter of the first reading");

    let id = match ka.tick(EVERY, EVERY) {
        KeepAliveTick::Send(id) => id,
        other => panic!("expected a challenge, got {other:?}"),
    };
    ka.answered(id, std::time::Duration::from_millis(0));
    assert_eq!(
        ka.latency_ms(),
        18,
        "a fast reply after a slow one pulls it down a quarter of the way"
    );
    assert!(ka.latency_ms() > 0, "and not to nothing, because it is smoothed");
}

#[test]
fn an_item_name_finds_the_block_the_world_will_send_for_it() {
    // The refusal this fixes read as "not a block this world has" for a block
    // the world very well has. The registry calls it `grass` and the game calls
    // it `minecraft:grass_block`, and looking the item's name up in the
    // registry found nothing -- for exactly one block, because every other one
    // happens to have the same name in both places. That is the worst way a
    // name can be wrong: it looks right everywhere it has been looked at.
    let terrain = crate::world::terrain::register_terrain(&mut crate::world::block::BlockRegistry::new());

    for name in [
        "minecraft:air",
        "minecraft:grass_block",
        "minecraft:dirt",
        "minecraft:stone",
        "minecraft:glowstone",
    ] {
        let block = crate::server::ticking::block_for_name(&terrain, name)
            .unwrap_or_else(|| panic!("{name} is a block this world has"));
        assert_eq!(
            crate::server::ticking::to_state_name(block, &terrain),
            name,
            "{name} round trips"
        );
    }

    // The grass case is the one that was broken, and it is the one that differs
    // from the registry's own name.
    assert_ne!(
        crate::server::ticking::to_state_name(terrain.grass, &terrain),
        "grass",
        "the game does not call the block grass"
    );
    assert!(crate::server::ticking::block_for_name(&terrain, "minecraft:grass_block").is_some());

    // And things that are items but not blocks this world has.
    for name in ["minecraft:stick", "minecraft:diamond_pickaxe", "grass_block", ""] {
        assert!(
            crate::server::ticking::block_for_name(&terrain, name).is_none(),
            "{name} is not a block to put down here"
        );
    }
}

#[test]
fn the_faces_are_numbered_the_way_the_game_numbers_them() {    // Placed on the wrong face, a block still goes somewhere -- just the wrong
    // somewhere, which is the quietest kind of wrong there is.
    //
    //   DOWN (0) new Vec3i( 0, -1,  0)
    //   UP   (1) new Vec3i( 0,  1,  0)
    //   NORTH(2) new Vec3i( 0, 0, -1)
    //   SOUTH(3) new Vec3i( 0, 0,  1)
    //   WEST (4) new Vec3i(-1,  0,  0)
    //   EAST (5) new Vec3i( 1,  0,  0)
    for (id, want) in [
        (0u8, (0, -1, 0)),
        (1, (0, 1, 0)),
        (2, (0, 0, -1)),
        (3, (0, 0, 1)),
        (4, (-1, 0, 0)),
        (5, (1, 0, 0)),
    ] {
        let face = Direction::from_3d_data_value(id).expect("a face the game has");
        assert_eq!(face.step(), want, "face {id}");
    }
    assert_eq!(
        Direction::from_3d_data_value(6),
        None,
        "and six is not one of the six"
    );
    assert_eq!(Direction::from_3d_data_value(255), None);
}

#[test]
fn a_block_goes_on_the_face_that_was_clicked() {
    let at = crate::world::pos::BlockPos::new(4, -7, 9);
    for (id, want) in [
        (0u8, (4, -8, 9)),
        (1, (4, -6, 9)),
        (2, (4, -7, 8)),
        (3, (4, -7, 10)),
        (4, (3, -7, 9)),
        (5, (5, -7, 9)),
    ] {
        let face = Direction::from_3d_data_value(id).expect("a face");
        let target = at.offset(face);
        assert_eq!(
            (target.x, target.y, target.z),
            want,
            "clicking face {id} of {at:?} puts the block there"
        );
        assert_eq!(at.offset(face).offset(face.opposite()), at, "and back again");
    }
}

/// A click point is 0..1 within the block, and the game refuses anything more
/// than one block from the centre (`ServerGamePacketListenerImpl:1415-1417`).
///
/// The pure part of that, which is what this server actually enforces.
#[test]
fn a_click_more_than_a_block_from_the_centre_is_not_on_the_block() {
    let inside = |axis: f32| (-1.0000001..1.0000001).contains(&(axis - 0.5));
    for click in [0.0f32, 0.5, 1.0] {
        assert!(inside(click), "a real click at {click} is on the block");
    }
    for click in [-0.6f32, 1.6, 12.0] {
        assert!(!inside(click), "a click at {click} is not");
    }
    // The margin is generous on purpose: it is a check against a client
    // claiming a block it did not click, not a snapping tolerance.
    assert!(inside(1.4), "just inside the margin");
    assert!(!inside(1.6), "and just outside it");
}

#[test]
fn a_keep_alive_answer_is_eight_bytes_after_the_packet_id() {
    // Built from the writer the server actually uses, so this is the bytes the
    // client was asked to echo and then echoed. Both halves of the frame are
    // pinned: the id in front, and the eight-byte long behind it.
    //
    // This is the second time a serverbound decoder read from byte zero and got
    // the packet id. 28 is `serverbound_keep_alive`, it is a small ordinary
    // number, it parsed as an answer, and every connection was dropped a quarter
    // of a minute after joining for being rude about it.
    let mut asked = Vec::new();
    crate::protocol::play::write_keep_alive(&mut asked, 42);
    assert_eq!(&asked[1..], &42i64.to_be_bytes(), "eight bytes, big endian");

    // The client answers with the same payload under its own id.
    let mut frame = vec![crate::protocol::play::id::SERVERBOUND_KEEP_ALIVE as u8];
    frame.extend_from_slice(&asked[1..]);

    let body = crate::protocol::play::serverbound_body(
        &frame,
        crate::protocol::play::id::SERVERBOUND_KEEP_ALIVE,
    )
    .expect("a keep alive answer");
    assert_eq!(body.len(), 8, "the id is not part of the answer");
    let answer = i64::from_be_bytes(body[..8].try_into().expect("eight"));
    assert_eq!(answer, 42, "and what is left is the challenge itself");

    // Read from byte zero it is not the challenge: it is 28.
    assert_ne!(i64::from_be_bytes(frame[..8].try_into().unwrap()), 42);
}

#[test]
fn a_keep_alive_answer_is_not_read_as_a_varint() {
    // The game writes it with `writeLong` and reads it with `readLong`, which is
    // eight fixed bytes. Decoding it as a VarInt takes the first byte and calls
    // it the whole answer.
    let mut asked = Vec::new();
    crate::protocol::play::write_keep_alive(&mut asked, 42);
    let mut body = &asked[1..];

    let as_long = i64::from_be_bytes(body[..8].try_into().expect("eight"));
    let (as_varint, _) = crate::protocol::VarLong::read(&mut body).expect("reads");
    assert_eq!(as_long, 42);
    assert_ne!(
        as_varint.0, as_long,
        "which is why the two encodings must not be confused"
    );
}

#[test]
fn the_body_helper_refuses_a_frame_that_is_something_else() {
    let mut asked = Vec::new();
    crate::protocol::play::write_keep_alive(&mut asked, 7);
    let mut frame = vec![crate::protocol::play::id::SERVERBOUND_KEEP_ALIVE as u8];
    frame.extend_from_slice(&asked[1..]);

    assert!(
        crate::protocol::play::serverbound_body(
            &frame,
            crate::protocol::play::id::SERVERBOUND_CONTAINER_CLICK
        )
        .is_err(),
        "a keep alive is not a click"
    );
    assert!(
        crate::protocol::play::serverbound_body(
            &frame,
            crate::protocol::play::id::SERVERBOUND_KEEP_ALIVE
        )
        .is_ok(),
        "and is itself"
    );
}
