use super::*;

#[test]
fn a_duration_reads_as_the_time_the_server_has_been_up() {
    let s = |n: u64| format_elapsed(std::time::Duration::from_secs(n));

    // Under a minute it is seconds with a tenth, because that is the range
    // where the gap between two lines is the interesting number.
    assert_eq!(s(0), "    0.0s");
    assert_eq!(s(12), "   12.0s");
    assert_eq!(
        format_elapsed(std::time::Duration::from_millis(12_450)),
        "   12.4s"
    );

    // Past a minute the tenths go, since whole seconds are what get compared.
    assert_eq!(s(60), "  1:00.0");
    assert_eq!(s(1800), " 30:00.0");

    // Past an hour there is no room left for a tenth.
    assert_eq!(s(3600), " 1:00:00");
    assert_eq!(s(3725), " 1:02:05");
}

#[test]
fn every_timestamp_is_the_same_width_so_the_messages_stay_in_a_column() {
    let widths: Vec<usize> = [0u64, 5, 45, 59, 60, 600, 3599, 3600, 86_400]
        .iter()
        .map(|n| format_elapsed(std::time::Duration::from_secs(*n)).len())
        .collect();
    assert!(
        widths.windows(2).all(|w| w[0] == w[1]),
        "widths were {widths:?}"
    );
}

#[test]
fn packet_logging_is_off_until_it_is_asked_for() {
    // The gate itself is what is being checked; what it prints goes to stdout
    // and is not worth capturing here.
    set_verbose(false);
    assert!(!verbose());

    set_verbose(true);
    assert!(verbose());

    // Left off, so a later test in this binary does not depend on order.
    set_verbose(false);
}

#[test]
fn the_rate_limiter_repeats_only_the_first_few() {
    let rl = RateLimiter::new();

    assert!(rl.should_log("x"), "first is always printed");
    assert!(rl.should_log("x"), "second is printed");
    assert!(rl.should_log("x"), "third is printed");
    assert!(!rl.should_log("x"), "fourth is suppressed");

    // A different message resets the count.
    assert!(rl.should_log("y"));
    assert!(rl.should_log("x"), "a different message makes this one new again");
}

#[test]
fn the_hundredth_repeat_is_printed() {
    let rl = RateLimiter::new();
    let mut printed = 0;

    for i in 1..=250 {
        if rl.should_log("same") {
            printed += 1;
            if i == 1 {
                assert_eq!(printed, 1);
            }
        }
    }

    // First three, then 100, 200.
    assert_eq!(printed, 5, "the count should reappear at each hundred");
}

#[test]
fn packet_names_distinguish_the_same_id_across_phases() {
    // Ids overlap completely between phases. 3 is login_compression in
    // login, login_acknowledged serverbound in login, and
    // finish_configuration in configuration. Logging an id without the phase
    // would be ambiguous exactly when it matters.
    assert_eq!(packet_name("login", 3), "login_compression");
    assert_eq!(packet_name("login-s2c", 3), "login_acknowledged");
    assert_eq!(packet_name("configuration", 3), "finish_configuration");
    assert_eq!(
        packet_name("configuration-s2c", 3),
        "finish_configuration"
    );
}

#[test]
fn an_unknown_id_still_reports_the_state_it_came_from() {
    assert_eq!(packet_name("play", 9999), "play:9999");
    assert_eq!(packet_name("nonsense", 1), "nonsense:1");
}

#[test]
fn every_play_name_comes_from_packets_json() {
    // The seven that were wrong before, all of them serverbound. Being
    // wrong about a name is not cosmetic: it sends whoever reads the log
    // looking for the wrong packet. tools/check_names.py checks the whole
    // table against packets.json; this keeps the interesting ones under test
    // so a regression fails in cargo rather than needing the script.
    for (id, expected) in [
        (0, "accept_teleportation"),
        (6, "chat_ack"),
        (13, "client_tick_end"),
        (22, "custom_payload"),
        (23, "debug_subscription_request"),
        (32, "move_player_rot"),
        (38, "ping_request"),
    ] {
        assert_eq!(packet_name("play-s2c", id), expected, "play serverbound {id}");
    }
}

#[test]
fn the_client_tick_packet_is_named_because_it_is_the_pulse() {
    // A client waiting on chunks sends this once a tick, one byte each, so a
    // run where it appears sixty times a second is the client reporting that
    // it is alive and still waiting. That is the signal to send chunks.
    assert_eq!(packet_name("play-s2c", 13), "client_tick_end");
}

#[test]
fn status_has_a_name_in_both_directions() {
    // Ids 0 and 1 exist on both sides with the other meaning, which is why
    // the state carries the direction.
    assert_eq!(packet_name("status-s2c", 0), "status_request");
    assert_eq!(packet_name("status", 0), "status_response");
    assert_eq!(packet_name("status-s2c", 1), "ping_request");
    assert_eq!(packet_name("status", 1), "pong_response");
}

#[test]
fn the_configuration_packets_this_server_sends_are_named() {
    // Without these, the log for the configuration phase reads
    // "configuration:14" and "configuration:15", which is what made the ids
    // worth confirming against a capture in the first place.
    assert_eq!(packet_name("configuration", 14), "update_tags");
    assert_eq!(packet_name("configuration", 15), "select_known_packs");
    assert_eq!(packet_name("configuration", 7), "registry_data");
    assert_eq!(
        packet_name("configuration", 13),
        "update_enabled_features"
    );
}

#[test]
fn select_known_packs_has_a_name_in_both_directions() {
    // 15 clientbound is the server asking about packs, and 7 serverbound is
    // the client answering. Same packet, different id, opposite direction.
    assert_eq!(packet_name("configuration", 15), "select_known_packs");
    assert_eq!(packet_name("configuration-s2c", 7), "select_known_packs");
}