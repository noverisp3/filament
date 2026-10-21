//! The configuration phase: the server tells the client how the world is set
//! up, and the client answers with what it wants.
//!
//! The registries and the tags go out first, because the client cannot build
//! anything without them, and the game event that says "start waiting for
//! chunks" goes last.

pub mod client_information_c2s;
pub mod custom_payload_s2c;
pub mod finish_configuration_s2c;
pub mod registry_data_s2c;
pub mod update_enabled_features_s2c;
pub mod update_tags_s2c;

use crate::protocol::{DecodeError, VarInt};

pub use client_information_c2s::ClientInformation;
pub use custom_payload_s2c::write_brand;
pub use finish_configuration_s2c::write_finish_configuration;
pub use registry_data_s2c::{registry_data_count, registry_data_frames, RegistryDataFrames};
pub use update_enabled_features_s2c::write_update_enabled_features;
pub use update_tags_s2c::{update_tags_summary, write_update_tags};

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

    pub const SERVERBOUND_CLIENT_INFORMATION: i32 = 0;
    pub const SERVERBOUND_COOKIE_RESPONSE: i32 = 1;
    pub const SERVERBOUND_CUSTOM_PAYLOAD: i32 = 2;
    pub const SERVERBOUND_FINISH_CONFIGURATION: i32 = 3;
    pub const SERVERBOUND_KEEP_ALIVE: i32 = 4;
    pub const SERVERBOUND_PONG: i32 = 5;
    pub const SERVERBOUND_RESOURCE_PACK: i32 = 6;
    pub const SERVERBOUND_SELECT_KNOWN_PACKS: i32 = 7;
}

/// Reads a serverbound configuration packet id and leaves the rest alone.
pub fn read_packet_id(body: &[u8]) -> Result<i32, DecodeError> {
    let mut r = body;
    Ok(VarInt::read(&mut r)?.0.0)
}

#[cfg(test)]
mod tests;
