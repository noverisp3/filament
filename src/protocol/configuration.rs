//! Packets of the configuration phase.
//!
//! This is the phase between login and the world. The client waits here for the
//! server to say which registries and feature flags apply, and will not accept
//! a world without being told.
//!
//! None of it is sent. A Minecraft 26.3 client carries its own registries, built
//! from the same jar the server came from, and falls back to them when the server
//! sends nothing:
//!
//! ```java
//! // RegistryDataCollector, decompiled
//! if (this.contentsCollector != null) { ... }
//! else { registries = originalRegistries; }
//! ```
//!
//! `contentsCollector` is only created when a registry_data packet arrives, so a
//! server that sends none never creates it and the client uses its own. Sending
//! registries is only needed to override them, which a datapack does.
//!
//! That is why there is no NBT writer here. Writing thirty-one registries in NBT
//! would be a large amount of code for something a client this version does not
//! require.

use crate::protocol::{DecodeError, VarInt};

/// Packet ids in the configuration state, read from `packets.json`.
pub mod id {
    pub const COOKIE_REQUEST: i32 = 0;
    pub const CUSTOM_PAYLOAD: i32 = 1;
    pub const DISCONNECT: i32 = 2;
    pub const FINISH_CONFIGURATION: i32 = 3;
    pub const KEEP_ALIVE: i32 = 4;
    pub const PING: i32 = 5;
    pub const RESET_CHAT: i32 = 6;
    pub const REGISTRY_DATA: i32 = 7;
    pub const RESOURCE_PACK_POP: i32 = 8;
    pub const RESOURCE_PACK_PUSH: i32 = 9;
    pub const POST_EFFECTS: i32 = 10;
    pub const STORE_COOKIE: i32 = 11;
    pub const TRANSFER: i32 = 12;
    pub const UPDATE_ENABLED_FEATURES: i32 = 13;
    pub const UPDATE_TAGS: i32 = 14;
    pub const SELECT_KNOWN_PACKS: i32 = 15;
    pub const CUSTOM_REPORT_DETAILS: i32 = 16;
    pub const SERVER_LINKS: i32 = 17;
    pub const CLEAR_DIALOG: i32 = 18;
    pub const SHOW_DIALOG: i32 = 19;
    pub const CODE_OF_CONDUCT: i32 = 20;

    // Serverbound, from the same file.
    pub const SERVERBOUND_CLIENT_INFORMATION: i32 = 0;
    pub const SERVERBOUND_COOKIE_RESPONSE: i32 = 1;
    pub const SERVERBOUND_CUSTOM_PAYLOAD: i32 = 2;
    pub const SERVERBOUND_FINISH_CONFIGURATION: i32 = 3;
    pub const SERVERBOUND_KEEP_ALIVE: i32 = 4;
    pub const SERVERBOUND_PONG: i32 = 5;
    pub const SERVERBOUND_RESOURCE_PACK: i32 = 6;
    pub const SERVERBOUND_SELECT_KNOWN_PACKS: i32 = 7;
}

/// Writes `ClientboundUpdateEnabledFeaturesPacket`.
///
/// A count followed by that many identifiers. Zero is a valid count: a client
/// does not require any particular feature flag from the server, and sending one
/// the client does not know would be worse than sending none.
pub fn write_update_enabled_features(out: &mut Vec<u8>, features: &[&str]) {
    VarInt(id::UPDATE_ENABLED_FEATURES).write(out);
    VarInt(features.len() as i32).write(out);
    for f in features {
        VarInt(f.len() as i32).write(out);
        out.extend_from_slice(f.as_bytes());
    }
}

/// Writes `ClientboundFinishConfigurationPacket`.
///
/// No body upstream, so nothing after the id.
pub fn write_finish_configuration(out: &mut Vec<u8>) {
    VarInt(id::FINISH_CONFIGURATION).write(out);
}

/// Reads a serverbound configuration packet id and leaves the rest alone.
pub fn read_packet_id(body: &[u8]) -> Result<i32, DecodeError> {
    let mut r = body;
    Ok(VarInt::read(&mut r)?.0.0)
}

/// `ServerboundClientInformationPacket`.
///
/// Recorded because the client sends it during configuration and this server
/// has to acknowledge that it arrived rather than treating it as unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInformation<'a> {
    pub language: &'a str,
    /// A single byte, not a VarInt. That changed from 1.20 and copying the old
    /// field layout would desynchronise every field after it.
    pub view_distance: u8,
    pub chat_visibility: i32,
    pub chat_colors: bool,
    pub model_customisation: u8,
    pub main_hand: i32,
    pub text_filtering_enabled: bool,
    pub allows_listing: bool,
    pub particle_status: i32,
}

impl<'a> ClientInformation<'a> {
    /// Reads the packet body including its id.
    pub fn read(body: &'a [u8]) -> Result<Self, DecodeError> {
        let mut r = body;

        let (VarInt(packet), _) = VarInt::read(&mut r)?;
        if packet != id::SERVERBOUND_CLIENT_INFORMATION {
            return Err(DecodeError::TooLong);
        }

        let (VarInt(len), _) = VarInt::read(&mut r)?;
        if len < 0 || len as usize > r.len() {
            return Err(DecodeError::StringTooLong {
                len: len.max(0) as usize,
                available: r.len(),
            });
        }
        let language = std::str::from_utf8(&r[..len as usize]).map_err(|_| DecodeError::NotUtf8)?;
        r = &r[len as usize..];

        let take_u8 = |r: &mut &[u8]| -> Result<u8, DecodeError> {
            let (b, rest) = r
                .split_first()
                .ok_or(DecodeError::UnexpectedEof {
                    wanted: "byte",
                    got: 0,
                })?;
            *r = rest;
            Ok(*b)
        };
        let take_bool = |r: &mut &[u8]| Ok(take_u8(r)? != 0);

        let view_distance = take_u8(&mut r)?;
        let chat_visibility = VarInt::read(&mut r)?.0.0;
        let chat_colors = take_bool(&mut r)?;
        let model_customisation = take_u8(&mut r)?;
        let main_hand = VarInt::read(&mut r)?.0.0;
        let text_filtering_enabled = take_bool(&mut r)?;
        let allows_listing = take_bool(&mut r)?;
        let particle_status = VarInt::read(&mut r)?.0.0;

        Ok(ClientInformation {
            language,
            view_distance,
            chat_visibility,
            chat_colors,
            model_customisation,
            main_hand,
            text_filtering_enabled,
            allows_listing,
            particle_status,
        })
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(id::SERVERBOUND_FINISH_CONFIGURATION, 3);
        assert_eq!(id::SERVERBOUND_CLIENT_INFORMATION, 0);
    }
}