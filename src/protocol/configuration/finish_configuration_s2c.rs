//! The end of the configuration phase.

use super::id;
use crate::protocol::VarInt;

/// Writes `ClientboundFinishConfigurationPacket`.
///
/// No body upstream, so nothing after the id.
pub fn write_finish_configuration(out: &mut Vec<u8>) {
    VarInt(id::FINISH_CONFIGURATION).write(out);
}
