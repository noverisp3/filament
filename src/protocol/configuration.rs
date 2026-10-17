//! Packets of the configuration phase.
//!
//! This is the phase between login and the world. The client waits here for the
//! server to say which registries and feature flags apply, and will not accept
//! a world without being told.
//!
//! Registries are not sent. A Minecraft 26.3 client carries its own, built from
//! the same jar the server came from, and falls back to them when the server
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
//! Tags have to be sent regardless, and this is the part that is not optional.
//! The same collector keeps a second structure:
//!
//! ```java
//! if (this.tagCollector != null) { this.tagCollector.loadOnlyTags(...); }
//! ```
//!
//! `tagCollector` is null unless an update_tags packet arrives, so nothing is
//! loaded and the client's own tag list is never built. The first tag it then
//! looks for while assembling its components is `minecraft:damage_type/minecraft:is_fire`,
//! and the state it does not exist fails on the spot:
//!
//! ```text
//! java.lang.IllegalStateException:
//!   Missing tag TagKey[minecraft:damage_type / minecraft:is_fire]
//! ```
//!
//! That is why `write_update_tags` exists and why it embeds a capture rather
//! than a list of names. Tags hold registry *indices*, and the order a registry
//! is packed in is not recorded anywhere in the data generator's output:
//! `registries.json` covers 95 registries and none of the 31 that get
//! synchronised, so damage_type is missing from it altogether. Reading the
//! values off a live connection was the only way to get them.

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

/// One captured `ClientboundRegistryDataPacket` frame after the next.
///
/// Yields the frame body with its packet id, which is what `FrameEncoder::encode`
/// takes. The frames are walked rather than decoded: the ids are still in place
/// and nothing here needs to be inside one.
pub struct RegistryDataFrames<'a> {
    data: &'a [u8],
    offset: usize,
    /// Set when a length could not be read, so the caller can tell a truncated
    /// file apart from one that simply ended.
    failed: Option<DecodeError>,
}

impl RegistryDataFrames<'_> {
    /// The error that ended the iteration early, if any.
    ///
    /// Separate from `Iterator::next` because an error here is reported as an
    /// item, and a caller that stops on the first error never sees the end of
    /// the walk that would tell it whether there was one.
    pub fn failure(&self) -> Option<&DecodeError> {
        self.failed.as_ref()
    }
}

impl<'a> Iterator for RegistryDataFrames<'a> {
    type Item = Result<&'a [u8], DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.data.len() {
            return None;
        }

        let mut slice = &self.data[self.offset..];
        let (length, consumed) = match VarInt::read(&mut slice) {
            Ok(v) => v,
            Err(e) => {
                // The offset cannot be advanced past a frame whose length is
                // unknown, so end the iteration instead of looping.
                self.offset = self.data.len();
                self.failed = Some(e);
                return None;
            }
        };
        if length.0 < 0 {
            let available = slice.len();
            self.offset = self.data.len();
            return Some(Err(DecodeError::StringTooLong {
                len: length.0.unsigned_abs() as usize,
                available,
            }));
        }

        let length = length.0 as usize;
        let body = self.offset + consumed;
        let end = body + length;
        if end > self.data.len() {
            let available = self.data.len() - body;
            self.offset = self.data.len();
            return Some(Err(DecodeError::StringTooLong { len: length, available }));
        }

        self.offset = end;
        Some(Ok(&self.data[body..end]))
    }
}

/// Borrows an iterator over the captured frames.
///
/// The size hint is a lower bound only: how many frames there are would mean
/// walking the file, which is what the caller is about to do anyway.
pub fn registry_data_frames() -> RegistryDataFrames<'static> {
    RegistryDataFrames {
        data: crate::generated::REGISTRY_DATA_FRAMES,
        offset: 0,
        failed: None,
    }
}

/// How many `registry_data` frames the capture holds.
pub fn registry_data_count() -> usize {
    crate::generated::REGISTRY_DATA_COUNT
}

/// Writes `ClientboundFinishConfigurationPacket`.
///
/// No body upstream, so nothing after the id.
pub fn write_finish_configuration(out: &mut Vec<u8>) {
    VarInt(id::FINISH_CONFIGURATION).write(out);
}

/// Writes `ClientboundUpdateTagsPacket`.
///
/// The body comes from `UPDATE_TAGS_BODY` rather than being assembled from a
/// list here, because a tag's contents are registry indices and nothing in the
/// game reports what those indices are. It is the same bytes a vanilla server
/// sends, so the client resolves every tag it asks for.
///
/// Roughly 58 KiB, which is past the compression threshold, so the encoder
/// compresses it. That is fine and expected: the frame length says 58 KiB, the
/// body says the same, and a client that asked for 256 has asked for it.
pub fn write_update_tags(out: &mut Vec<u8>) {
    VarInt(id::UPDATE_TAGS).write(out);
    out.extend_from_slice(crate::generated::UPDATE_TAGS_BODY);
}

/// How many tags `write_update_tags` will send, for logging.
pub fn update_tags_summary() -> String {
    let registries = crate::generated::TAG_COUNTS.len();
    let tags = crate::generated::TAG_TOTAL;
    let bytes = crate::generated::UPDATE_TAGS_BODY.len();
    format!("{registries} registries, {tags} tags, {bytes} bytes")
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
}