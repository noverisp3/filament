//! The feature flags the world runs with.

use super::id;
use crate::protocol::VarInt;

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
