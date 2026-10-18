use super::*;

#[test]
fn finish_configuration_is_only_its_id() {
    let mut w = Vec::new();
    write_finish_configuration(&mut w);
    assert_eq!(w, vec![id::FINISH_CONFIGURATION as u8]);
}

#[test]
fn an_empty_feature_set_is_a_zero_count() {
    let mut w = Vec::new();
    write_update_enabled_features(&mut w, &[]);

    assert_eq!(
        w,
        vec![id::UPDATE_ENABLED_FEATURES as u8, 0],
        "the count is present even when it is zero, because the client \
         reads it positionally"
    );
}

#[test]
fn a_feature_flag_is_written_as_a_length_prefixed_identifier() {
    let mut w = Vec::new();
    write_update_enabled_features(&mut w, &["minecraft:vanilla"]);

    assert_eq!(w[0], id::UPDATE_ENABLED_FEATURES as u8);
    assert_eq!(w[1], 1, "one flag");
    assert_eq!(w[2], 17, "byte length of minecraft:vanilla");
    assert_eq!(&w[3..], b"minecraft:vanilla");
}

#[test]
fn client_information_reads_a_body_the_client_would_send() {
    // Built by hand from `ClientInformation.write` in the decompiled server.
    let mut w = Vec::new();
    VarInt(id::SERVERBOUND_CLIENT_INFORMATION).write(&mut w);
    VarInt(5).write(&mut w);
    w.extend_from_slice(b"en_us");
    w.push(8); // view distance: one byte
    VarInt(0).write(&mut w); // chat visibility
    w.push(1); // chat colours
    w.push(0); // model customisation
    VarInt(1).write(&mut w); // main hand
    w.push(0); // text filtering
    w.push(1); // allows listing
    VarInt(0).write(&mut w); // particle status

    let info = ClientInformation::read(&w).expect("decodes");

    assert_eq!(info.language, "en_us");
    assert_eq!(info.view_distance, 8);
    assert_eq!(info.main_hand, 1);
    assert!(info.allows_listing);
    assert!(info.chat_colors);
}

#[test]
fn client_information_refuses_the_wrong_id() {
    let mut w = Vec::new();
    VarInt(id::SERVERBOUND_KEEP_ALIVE).write(&mut w);
    assert!(ClientInformation::read(&w).is_err());
}

#[test]
fn every_configuration_id_is_the_one_the_game_uses() {
    // Read from packets.json, which the data generator produced from the
    // same jar. Pinned because these are positional on the wire.
    assert_eq!(id::FINISH_CONFIGURATION, 3);
    assert_eq!(id::REGISTRY_DATA, 7);
    assert_eq!(id::UPDATE_ENABLED_FEATURES, 13);
    assert_eq!(id::UPDATE_TAGS, 14);
    assert_eq!(id::SELECT_KNOWN_PACKS, 15);
    assert_eq!(id::SERVERBOUND_FINISH_CONFIGURATION, 3);
    assert_eq!(id::SERVERBOUND_CLIENT_INFORMATION, 0);
}

#[test]
fn update_tags_is_the_id_followed_by_the_captured_body() {
    let mut w = Vec::new();
    write_update_tags(&mut w);

    assert_eq!(
        w[0],
        id::UPDATE_TAGS as u8,
        "the id is 14, confirmed by the capture: the packet that carries \
         58 KiB of tags arrived as id 14"
    );
    assert_eq!(&w[1..], crate::generated::UPDATE_TAGS_BODY);
    assert_eq!(w.len(), 1 + crate::generated::UPDATE_TAGS_BODY.len());
}

#[test]
fn update_tags_starts_with_fifteen_registries() {
    // The leading VarInt in the captured body. Worth pinning separately
    // because a truncated capture would still start with a plausible
    // number and the failure would only show up as a client disconnect.
    let mut w = Vec::new();
    write_update_tags(&mut w);
    assert_eq!(w[1], 15);
}

#[test]
fn registry_data_yields_one_frame_per_registry_and_stops_clean() {
    let mut frames = registry_data_frames();
    let mut count = 0;
    let mut total = 0;

    for frame in &mut frames {
        let frame = frame.expect("the embedded capture should walk");
        // Every frame carries its own packet id, which is what gets handed
        // to the encoder without being touched.
        assert_eq!(
            frame.first().copied(),
            Some(id::REGISTRY_DATA as u8),
            "frame {count} starts with {frame:02x?}, expected the registry_data id"
        );
        count += 1;
        total += frame.len();
    }

    assert_eq!(count, registry_data_count());
    assert!(count > 0);
    assert_eq!(
        frames.failure(),
        None,
        "the walk should end at the end of the file, not on an error"
    );

    let mut again = registry_data_frames();
    assert_eq!(
        again.by_ref().count(),
        count,
        "the iterator is borrowed from a static, so a second pass sees the same frames"
    );
    assert!(total > 100_000, "expected the full contents, got {total}");
}

#[test]
fn the_tag_summary_names_what_is_being_sent() {
    let summary = update_tags_summary();
    assert!(summary.contains("15 registries"), "{summary}");
    assert!(summary.contains("773 tags"), "{summary}");
}