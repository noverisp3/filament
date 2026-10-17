//! Registry contents captured from a vanilla 26.3 server. Do not edit by hand.
//!
//! Each frame is a whole `ClientboundRegistryDataPacket` body, packet id
//! included, so the encoder is handed it without anything having to be rebuilt.
//! The frames sit one after another, each behind a VarInt length, which is the
//! same framing `tools/capture.py` writes to `captured/packets.bin`.
//!
//! Regenerate with:
//!
//! ```text
//! python tools/capture.py 127.0.0.1 25566
//! python tools/extract_registries.py
//! ```
//!
//! Why this file is needed, when a client is supposed to fall back to the
//! registries it was built with: the fallback is taken when no registry_data
//! arrives, and on that path the tags are resolved against the client's built-in
//! registries. Those are the ones baked into the jar, which is not the same set.
//! `minecraft:dialog` is a datapack registry and is not among them, so the very
//! first tag on a datapack registry throws:
//!
//! ```text
//! Missing registry: ResourceKey[minecraft:root / minecraft:dialog]
//! ```
//!
//! Sending the registries also fixes the other half: tag contents are registry
//! indices, so without these the client has no order to resolve an index
//! against.

/// Every captured frame, each behind a VarInt length.
pub const REGISTRY_DATA_FRAMES: &[u8] = include_bytes!("../../assets/registry_data.bin");

/// How many frames the file holds.
///
/// Recorded rather than computed so a file that was truncated or regenerated in
/// a different order shows up as a failing test instead of as a client that
/// hangs up with no explanation.
pub const REGISTRY_DATA_COUNT: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_bytes_are_the_registry_capture() {
        // include_bytes! is resolved when the crate compiles, so a stale build
        // can embed a file that has since been regenerated. Pin the head so a
        // failure here says what was actually embedded.
        assert_eq!(
            REGISTRY_DATA_FRAMES[0] & 0x7F,
            0x11,
            "the low seven bits of the first byte are the first frame's length"
        );
        assert_eq!(
            &REGISTRY_DATA_FRAMES[3..11],
            &[0x07, 0x18, 0x6D, 0x69, 0x6E, 0x65, 0x63, 0x72],
            "after the three-byte length comes the registry_data id and the \
             start of minecraft:worldgen/biome"
        );
        assert_eq!(REGISTRY_DATA_FRAMES.len(), 157904);
    }

    /// Walks the framing and returns the length of every frame.
    fn frames() -> Vec<usize> {
        let data = REGISTRY_DATA_FRAMES;
        let mut out = Vec::new();
        let mut i = 0;

        while i < data.len() {
            let start = i;
            let mut result = 0i32;
            let mut shift = 0u32;
            loop {
                assert!(i < data.len(), "length VarInt ran past the end");
                let b = data[i];
                i += 1;
                result |= ((b & 0x7F) as i32) << shift;
                if b & 0x80 == 0 {
                    break;
                }
                shift += 7;
                assert!(shift <= 35, "length VarInt too long");
            }
            assert!(result >= 0, "a negative frame length at byte {start}");
            let end = i + result as usize;
            assert!(end <= data.len(), "frame at {start} runs past the end");
            out.push(end - i);
            i = end;
        }

        out
    }

    #[test]
    fn the_file_holds_the_expected_number_of_frames() {
        assert_eq!(
            frames().len(),
            REGISTRY_DATA_COUNT,
            "captured from a stock 26.3 server, which synchronises 32 registries"
        );
    }

    #[test]
    fn every_frame_is_a_registry_data_packet() {
        let data = REGISTRY_DATA_FRAMES;
        let mut i = 0;
        let mut checked = 0;

        while i < data.len() {
            let mut result = 0i32;
            let mut shift = 0u32;
            loop {
                let b = data[i];
                i += 1;
                result |= ((b & 0x7F) as i32) << shift;
                if b & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            let body = &data[i..i + result as usize];

            // The packet id, as a VarInt, has to be 7.
            assert_eq!(
                body[0], 7,
                "frame {checked} does not start with the registry_data id"
            );

            // Then the registry identifier as a length-prefixed string. The
            // names are checked rather than the count, because a wrong name
            // would mean the wrong registry got captured.
            let name_len = body[1] as usize;
            let name = std::str::from_utf8(&body[2..2 + name_len]).expect("utf-8");
            assert!(
                name.starts_with("minecraft:"),
                "frame {checked} names a registry outside minecraft: {name}"
            );

            i += result as usize;
            checked += 1;
        }
    }

    #[test]
    fn the_frames_are_not_all_the_same() {
        // A capture that failed halfway would still walk, and 32 copies of one
        // frame would too. Distinct sizes are a cheap sign the file is really
        // thirty-two different registries.
        let sizes = frames();
        let mut sorted = sizes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert!(
            sorted.len() > 20,
            "only {} distinct sizes across {REGISTRY_DATA_COUNT} frames",
            sorted.len()
        );
        assert!(sorted.iter().max().unwrap() > &10_000);
    }

    #[test]
    fn the_file_is_large_because_the_contents_are() {
        // The capture was taken while answering select_known_packs with an
        // empty list, which tells the server the client has none of its packs
        // and so sends every entry's contents rather than just its name. That
        // is what makes the capture self-contained.
        assert!(
            REGISTRY_DATA_FRAMES.len() > 100_000,
            "expected the full contents, got {} bytes",
            REGISTRY_DATA_FRAMES.len()
        );
    }
}