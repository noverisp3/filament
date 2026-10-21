//! The tags the client needs, captured from a stock server.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundUpdateTagsPacket`.
///
/// The body comes from `UPDATE_TAGS_BODY` rather than being assembled from a
/// list here, because a tag's contents are registry indices and nothing in the
/// game reports what those indices are. It is the same bytes a vanilla server
/// sends, so the client resolves every tag it asks for.
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
