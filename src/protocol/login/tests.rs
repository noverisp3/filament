use super::*;
use crate::protocol::VarInt;

#[test]
fn an_offline_uuid_is_stable_for_a_name() {
    // The client caches the identity between sessions, so a UUID that
    // changes between connections shows up as the player's skin resetting.
    let a = Uuid::offline_for("Notch");
    let b = Uuid::offline_for("Notch");
    assert_eq!(a, b);

    assert_ne!(a, Uuid::offline_for("Herobrine"));
}

#[test]
fn an_offline_uuid_is_version_three_and_rfc_4122() {
    // Name-based UUIDs are version 3, and Mojang stamps them the same way.
    // A server that gets this wrong is rejected by strict clients.
    let uuid = Uuid::offline_for("Notch");

    let bytes = uuid.0.to_be_bytes();
    assert_eq!(bytes[6] >> 4, 3, "version nibble must be 3");
    assert_eq!(bytes[8] >> 6, 0b10, "variant must be IETF");
}

#[test]
fn an_offline_uuid_matches_the_value_mojang_produces() {
    // Pinned so a change to the hashing shows up here rather than as a
    // player appearing to be someone else.
    assert_eq!(
        Uuid::offline_for("Notch").to_string(),
        // Verified against `hashlib.md5` independently: the first 20 hex
        // digits are the MD5 of "OfflinePlayer:Notch" and the rest follow
        // from the version and variant bits being stamped in. This is
        // Mojang's value, so the same name reads as the same player here as
        // on any other offline-mode server.
        "b50ad385-829d-3141-a216-7e7d7539ba7f"
    );
}

#[test]
fn a_uuid_is_written_as_two_big_endian_halves() {
    let uuid = Uuid(0x0123456789abcdef_fedcba9876543210);
    let mut w = Vec::new();
    uuid.write(&mut w);

    assert_eq!(w.len(), 16);
    assert_eq!(w[0], 0x01, "the high half is written most significant first");
    assert_eq!(w[15], 0x10);

    let got = Uuid::read(&mut &w[..]).expect("decodes");
    assert_eq!(got, uuid);
}

#[test]
fn a_hello_is_read_with_its_id() {
    let mut w = Vec::new();
    VarInt(id::SERVERBOUND_HELLO).write(&mut w);
    VarInt(5).write(&mut w);
    w.extend_from_slice(b"Notch");
    Uuid(7).write(&mut w);

    let (name, uuid) = read_hello(&w).expect("decodes");
    assert_eq!(name, "Notch");
    assert_eq!(uuid, Uuid(7));
}

#[test]
fn a_hello_with_the_wrong_id_is_refused() {
    let mut w = Vec::new();
    VarInt(id::SERVERBOUND_KEY).write(&mut w);
    VarInt(5).write(&mut w);
    w.extend_from_slice(b"Notch");
    Uuid(7).write(&mut w);

    assert!(read_hello(&w).is_err());
}

#[test]
fn a_login_finished_has_the_shape_the_client_reads() {
    let mut w = Vec::new();
    write_login_finished(&mut w, Uuid(1), "Notch", Uuid(2));

    let mut r = &w[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("id");
    assert_eq!(packet, id::LOGIN_FINISHED);

    assert_eq!(Uuid::read(&mut r).expect("profile id"), Uuid(1));

    let (VarInt(name_len), _) = VarInt::read(&mut r).expect("name length");
    let name = std::str::from_utf8(&r[..name_len as usize]).expect("utf8");
    assert_eq!(name, "Notch");
    r = &r[name_len as usize..];

    let (VarInt(properties), _) = VarInt::read(&mut r).expect("property count");
    assert_eq!(properties, 0, "an offline player has no profile properties");

    assert_eq!(Uuid::read(&mut r).expect("session id"), Uuid(2));
    assert!(r.is_empty(), "the packet has no trailing bytes");
}

#[test]
fn login_compression_is_a_single_varint() {
    let mut w = Vec::new();
    write_login_compression(&mut w, 256);

    let mut r = &w[..];
    let (VarInt(packet), _) = VarInt::read(&mut r).expect("id");
    assert_eq!(packet, id::LOGIN_COMPRESSION);

    let (VarInt(threshold), _) = VarInt::read(&mut r).expect("threshold");
    assert_eq!(threshold, 256);
    assert!(r.is_empty());
}

#[test]
fn the_acknowledgement_is_only_its_id() {
    let body = login_acknowledged_body();
    assert_eq!(body, vec![id::SERVERBOUND_LOGIN_ACKNOWLEDGED as u8]);
}