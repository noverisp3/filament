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