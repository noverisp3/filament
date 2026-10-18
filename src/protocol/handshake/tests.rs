use super::*;

#[test]
fn an_intention_packet_round_trips() {
    let pkt = ClientIntention {
        protocol_version: VarInt(777),
        host: "localhost".to_string(),
        port: 25565,
        intent: ClientIntent::Login,
    };

    let mut w = Vec::new();
    pkt.write(&mut w);
    let got = ClientIntention::read(&w).expect("decodes");

    assert_eq!(got, pkt);
}

#[test]
fn the_port_is_big_endian() {
    // 25565 is 0x63DD. Written little-endian it would be DD 63, and the
    // client would read 56669, showing the server on the wrong port.
    let pkt = ClientIntention {
        protocol_version: VarInt(777),
        host: String::new(),
        port: 25565,
        intent: ClientIntent::Status,
    };

    let mut w = Vec::new();
    pkt.write(&mut w);

    // Leading packet id (one byte), then a zero-length host, then the port. The
    // intent VarInt follows the port, so it is excluded.
    let port_bytes = &w[w.len() - 3..w.len() - 1];
    assert_eq!(
        port_bytes,
        &[0x63, 0xDD],
        "the port must be written most significant byte first"
    );
}

#[test]
fn every_intention_has_the_id_the_game_uses() {
    // `ClientIntent` numbers these from 1, not 0. Off by one here and the
    // client is refused before anything else happens.
    assert_eq!(ClientIntent::Status.id(), 1);
    assert_eq!(ClientIntent::Login.id(), 2);
    assert_eq!(ClientIntent::Transfer.id(), 3);

    assert_eq!(ClientIntent::from_id(1), Some(ClientIntent::Status));
    assert_eq!(ClientIntent::from_id(2), Some(ClientIntent::Login));
    assert_eq!(ClientIntent::from_id(0), None);
    assert_eq!(ClientIntent::from_id(4), None);
}

#[test]
fn a_host_longer_than_the_cap_is_refused() {
    // The handshake allows 255, not the 32767 used for other strings.
    let long = "a".repeat(300);
    let mut w = Vec::new();
    VarInt(HANDSHAKE_INTENTION_ID).write(&mut w);
    VarInt(777).write(&mut w);
    VarInt(long.len() as i32).write(&mut w);
    w.extend_from_slice(long.as_bytes());
    w.extend_from_slice(&[0, 255]);
    VarInt(1).write(&mut w);

    assert!(ClientIntention::read(&w).is_err());
}

#[test]
fn the_status_json_has_the_fields_the_client_reads() {
    let s = status("filament", "filament 26.3", 777, 0, 20);
    let json = s.to_json();

    assert!(json.contains(r#""protocol":777"#));
    assert!(json.contains(r#""max":20"#));
    assert!(json.contains(r#""online":0"#));
    assert!(json.contains(r#""text":"filament""#));
    assert!(json.contains(r#""name":"filament 26.3""#));
}

#[test]
fn a_description_with_quotes_stays_valid_json() {
    // The description is server-controlled text that ends up in a JSON
    // string. Unescaped it produces invalid JSON, and the client reports
    // the server as broken rather than the description as broken.
    let s = status("say \"hi\"\nnow", "v", 777, 0, 20);
    let json = s.to_json();

    assert!(json.contains(r#""text":"say \"hi\"\nnow""#));

    // Counting lines only proves the format did not grow a literal newline
    // where the description had one. What actually matters is that the
    // document is still well-formed, so it is parsed back: a description
    // with a quote and a newline in it is the case that breaks naive
    // string formatting, and a round trip through a parser is what catches
    // it.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|v| v["description"]["text"].as_str().map(String::from)),
        Some("say \"hi\"\nnow".to_string()),
        "the description must survive a round trip through a JSON parser"
    );
}

#[test]
fn a_control_character_in_a_description_is_escaped() {
    let s = status("\u{1}\u{1f}", "v", 777, 0, 20);
    let json = s.to_json();
    assert!(json.contains(r#"\u0001"#));
    assert!(json.contains(r#"\u001f"#));
}

#[test]
fn a_ping_round_trips() {
    let ping = PingRequest { id: 1234567890123 };
    let body = pong_response_body(&ping);

    let got = ping_request_body(&body).expect("decodes");
    assert_eq!(got.id, ping.id);
}