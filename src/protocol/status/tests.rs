use super::*;

#[test]
fn the_status_json_has_the_fields_the_client_reads() {
    let s = status("filament", "filament 26.3", 777, 0, 20);
    let json = s.to_json();

    assert!(json.contains(r#""protocol":777"#));
    assert!(json.contains(r#""max":20"#));
    assert!(json.contains(r#""online":0"#));
    assert!(json.contains(r#""text":"filament""#));
    assert!(json.contains(r#""name":"filament 26.3""#));

    // Nobody on: no sample field at all, which the client reads as a list
    // it does not need to draw.
    assert!(!json.contains("sample"));
}

#[test]
fn the_sample_names_who_is_on() {
    let mut s = status("filament", "filament 26.3", 777, 1, 20);
    s.sample
        .push(("noverisp3".to_string(), "7bb8500454b03d8692341f2e09c8fe2a".to_string()));
    let json = s.to_json();

    assert!(json.contains(r#""sample":[{"name":"noverisp3","id":"7bb8500454b03d8692341f2e09c8fe2a"}]"#));
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
