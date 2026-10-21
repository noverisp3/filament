//! Framing tests.
//!
//! These build frames with [`FrameEncoder`] and read them back with
//! [`FrameDecoder`], and separately check against hand-written byte sequences,
//! because a round trip between two halves of the same bug proves nothing.

use super::*;
use crate::protocol::VarInt;

/// Builds one uncompressed frame the way the wire format requires: a VarInt
/// length, then the body.
fn raw_frame(body: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    VarInt(body.len() as i32).write(&mut v);
    v.extend_from_slice(body);
    v
}

#[test]
fn an_uncompressed_frame_round_trips() {
    let mut dec = FrameDecoder::new();
    dec.queue(&raw_frame(b"hello"));

    assert_eq!(dec.next_frame().expect("decodes"), Some(b"hello".to_vec()));
}

#[test]
fn nothing_is_produced_until_the_whole_frame_arrives() {
    let mut dec = FrameDecoder::new();
    let frame = raw_frame(b"hello world");

    // One byte at a time. The frame must appear only on the last one, or a
    // reader would hand out a truncated packet.
    for i in 0..frame.len() - 1 {
        dec.queue(&frame[i..i + 1]);
        assert_eq!(dec.next_frame().expect("decodes"), None, "after {} bytes", i + 1);
    }

    dec.queue(&frame[frame.len() - 1..]);
    assert_eq!(
        dec.next_frame().expect("decodes"),
        Some(b"hello world".to_vec())
    );
}

#[test]
fn several_frames_in_one_read_are_returned_one_at_a_time() {
    let mut dec = FrameDecoder::new();

    // All three arrive in a single read, which is normal on TCP.
    let mut all = raw_frame(b"first");
    all.extend(raw_frame(b"second"));
    all.extend(raw_frame(b"third"));
    dec.queue(&all);

    assert_eq!(dec.next_frame().expect("decodes"), Some(b"first".to_vec()));
    assert_eq!(dec.next_frame().expect("decodes"), Some(b"second".to_vec()));
    assert_eq!(dec.next_frame().expect("decodes"), Some(b"third".to_vec()));
    assert_eq!(dec.next_frame().expect("decodes"), None);
}

#[test]
fn a_frame_is_reassembled_across_a_split_in_the_middle_of_its_body() {
    let mut dec = FrameDecoder::new();
    let frame = raw_frame(&[9u8; 300]);

    // Split inside the body, not on a byte boundary that happens to line up.
    dec.queue(&frame[..120]);
    assert_eq!(dec.next_frame().expect("decodes"), None);

    dec.queue(&frame[120..]);
    let got = dec.next_frame().expect("decodes").expect("a frame");
    assert_eq!(got.len(), 300);
    assert!(got.iter().all(|&b| b == 9));
}

#[test]
fn a_length_beyond_the_ceiling_is_refused() {
    let mut dec = FrameDecoder::new();

    // Claims four megabytes, which is over MAX_FRAME_SIZE. This must not
    // allocate four megabytes on the strength of two bytes from the network.
    let mut v = Vec::new();
    VarInt(4_000_000).write(&mut v);
    dec.queue(&v);

    assert!(matches!(dec.next_frame(), Err(FrameError::TooLarge)));
}

#[test]
fn a_compressed_frame_round_trips() {
    let mut enc = FrameEncoder::new();
    enc.set_compression(DEFAULT_COMPRESSION_THRESHOLD);
    let mut dec = FrameDecoder::new();
    dec.set_compression(DEFAULT_COMPRESSION_THRESHOLD);

    // Highly compressible, so it crosses the threshold.
    let body = vec![7u8; 4096];
    let mut wire = Vec::new();
    enc.encode(&body, &mut wire).expect("encodes");
    dec.queue(&wire);

    let got = dec.next_frame().expect("decodes").expect("a frame");
    assert_eq!(got, body);
}

#[test]
fn a_body_under_the_threshold_still_carries_a_zero_inner_length() {
    let mut enc = FrameEncoder::new();
    enc.set_compression(DEFAULT_COMPRESSION_THRESHOLD);

    let body = b"short";
    let mut wire = Vec::new();
    enc.encode(body, &mut wire).expect("encodes");

    // Length of the frame, then an inner length of zero, then the payload. The
    // zero is what tells the client not to inflate, so leaving it out is how a
    // real client ends up trying to inflate five ASCII bytes.
    let mut expected = Vec::new();
    VarInt((body.len() + 1) as i32).write(&mut expected);
    VarInt(0).write(&mut expected);
    expected.extend_from_slice(body);

    assert_eq!(wire, expected);
}

#[test]
fn the_threshold_is_inclusive() {
    // `CompressionEncoder` takes the deflate branch when `uncompressedLength >=
    // threshold`, so a body of exactly the threshold is deflated and one byte
    // less is sent raw. Getting this backwards costs one byte at the boundary
    // and nothing visible, which is why it is pinned rather than left implicit.
    let mut enc = FrameEncoder::new();
    enc.set_compression(64);

    // Long enough that compressing wins, so the wire size tells the two apart
    // without decoding anything.
    let payload = vec![0u8; 64];

    let mut at = Vec::new();
    enc.encode(&payload, &mut at).expect("encodes");

    let mut below = Vec::new();
    enc.encode(&payload[..63], &mut below).expect("encodes");

    assert!(
        at.len() < below.len(),
        "a body exactly at the threshold must be deflated: {} vs {}",
        at.len(),
        below.len()
    );
}

#[test]
fn a_compressed_frame_smaller_than_it_would_be_uncompressed_is_still_compressed() {
    // Minecraft compresses on length alone, not on whether it would help. Once
    // the threshold is passed the client always expects the inner length, so
    // deciding by whether compression helped would desynchronise the stream.
    let mut enc = FrameEncoder::new();
    enc.set_compression(4);

    let body = b"abcd"; // exactly at the threshold, and incompressible
    let mut wire = Vec::new();
    enc.encode(body, &mut wire).expect("encodes");

    assert_ne!(
        wire,
        raw_frame(body),
        "a body at the threshold must carry the compressed framing"
    );
    assert!(wire.len() > body.len(), "incompressible data grows once framed");
}

#[test]
fn a_compressed_frame_carrying_a_zero_length_is_read_uncompressed() {
    // This is the form the server sends for packets below the threshold after
    // compression is enabled: inner length zero, then the payload.
    let mut dec = FrameDecoder::new();
    dec.set_compression(DEFAULT_COMPRESSION_THRESHOLD);

    let mut inner = Vec::new();
    VarInt(0).write(&mut inner);
    inner.extend_from_slice(b"tiny");
    dec.queue(&raw_frame(&inner));

    assert_eq!(
        dec.next_frame().expect("decodes"),
        Some(b"tiny".to_vec()),
        "an inner length of zero means the payload follows uncompressed"
    );
}

#[test]
fn compression_applies_only_after_it_is_announced() {
    // Before the threshold is set, a frame is just a length and a body.
    let mut plain = FrameDecoder::new();
    plain.queue(&raw_frame(b"payload"));
    assert_eq!(plain.next_frame().expect("decodes"), Some(b"payload".to_vec()));

    // After it is set, the same bytes mean something else: the first byte of
    // the payload is now read as the inner length. b'p' is 0x70 = 112, so the
    // decoder announces it needs 112 decompressed bytes from 6 bytes of input
    // and fails, rather than handing back a silently truncated packet.
    let mut compressed = FrameDecoder::new();
    compressed.set_compression(DEFAULT_COMPRESSION_THRESHOLD);
    compressed.queue(&raw_frame(b"payload"));

    assert!(
        matches!(compressed.next_frame(), Err(FrameError::Inflate(_))),
        "an uncompressed frame read as compressed must fail, not truncate"
    );
}

#[test]
fn a_frame_that_inflates_to_the_wrong_length_is_an_error() {
    let mut dec = FrameDecoder::new();
    dec.set_compression(DEFAULT_COMPRESSION_THRESHOLD);

    // Claims 1000 decompressed bytes but carries almost nothing. `read_exact`
    // must report this rather than returning a short buffer.
    let mut inner = Vec::new();
    VarInt(1000).write(&mut inner);
    inner.extend_from_slice(&[0u8; 4]);
    dec.queue(&raw_frame(&inner));

    assert!(matches!(dec.next_frame(), Err(FrameError::Inflate(_))));
}

#[test]
fn many_frames_of_mixed_sizes_stay_aligned() {
    let mut enc = FrameEncoder::new();
    let mut dec = FrameDecoder::new();

    let bodies: Vec<Vec<u8>> = vec![
        vec![1, 2, 3],
        vec![9; 300],
        vec![0; 5000],
        b"x".to_vec(),
        vec![255; 255],
    ];

    // Threshold zero, so every frame goes out in the compressed form: an inner
    // length then the payload. That makes this a test of alignment under
    // compression rather than a mix of the two framings, which
    // `a_compressed_frame_carrying_a_zero_length_is_read_uncompressed` covers.
    enc.set_compression(0);
    dec.set_compression(0);

    let mut wire = Vec::new();
    for b in &bodies {
        enc.encode(b, &mut wire).expect("encodes");
    }
    dec.queue(&wire);

    // Fed in ever larger reads, because a socket makes no promise about how a
    // stream lands: one byte, a partial body, or three frames at once. The loop
    // is bounded by the wire length so it cannot spin when everything has
    // already been delivered.
    let mut got = Vec::new();
    let mut pos = 0;
    let mut chunk = 1;

    while got.len() < bodies.len() {
        let end = (pos + chunk).min(wire.len());
        dec.queue(&wire[pos..end]);
        pos = end;

        while let Some(frame) = dec.next_frame().expect("decodes") {
            got.push(frame);
        }

        if pos == wire.len() && got.len() < bodies.len() {
            panic!(
                "wire is exhausted after {} of {} frames: a frame was lost",
                got.len(),
                bodies.len()
            );
        }

        chunk = (chunk * 2).min(4096);
    }

    assert_eq!(got.len(), bodies.len(), "lost or duplicated a frame");
    for (i, (a, b)) in got.iter().zip(bodies.iter()).enumerate() {
        assert_eq!(a, b, "frame {i} came back wrong");
    }
}
#[test]
fn frames_already_buffered_are_available_without_reading_the_socket() {
    // The client sends its handshake and its status request in one write, so by
    // the time the first frame has been taken, the second is already sitting in
    // the decoder. A reader that calls read() before draining blocks on a socket
    // that has nothing coming.
    let mut dec = FrameDecoder::new();
    let mut wire = raw_frame(b"first");
    wire.extend(raw_frame(b"second"));

    dec.queue(&wire);

    assert_eq!(dec.next_frame().expect("decodes"), Some(b"first".to_vec()));
    // No read() in between: the second frame must be ready right away.
    assert_eq!(
        dec.next_frame().expect("decodes"),
        Some(b"second".to_vec()),
        "a frame already in the buffer must not wait on the socket"
    );
    assert_eq!(dec.next_frame().expect("decodes"), None, "and then it is empty");
}

#[test]
fn a_partner_decoder_still_sees_everything_a_handover_missed() {
    // The other half of the same bug: starting a fresh decoder at a phase
    // boundary throws away whatever arrived behind the packet that ended the
    // previous phase. This checks the fix, which is to hand the decoder over.
    let mut dec = FrameDecoder::new();
    let mut wire = raw_frame(b"hello");
    wire.extend(raw_frame(b"hello_again"));
    dec.queue(&wire);

    assert_eq!(dec.next_frame().expect("decodes"), Some(b"hello".to_vec()));
    // Same decoder, not a new one.
    assert_eq!(
        dec.next_frame().expect("decodes"),
        Some(b"hello_again".to_vec())
    );
}
