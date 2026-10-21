use super::*;
use crate::protocol::VarInt;

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
