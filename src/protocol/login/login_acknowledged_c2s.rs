//! The client's acknowledgement, which is only its id.

use super::id;
use crate::protocol::VarInt;

/// The body of a login acknowledgement, which is only its id.
pub fn login_acknowledged_body() -> Vec<u8> {
    VarInt(id::SERVERBOUND_LOGIN_ACKNOWLEDGED).into_raw()
}

/// Extension that turns a VarInt into its own byte representation.
trait IntoRaw {
    fn into_raw(self) -> Vec<u8>;
}

impl IntoRaw for VarInt {
    fn into_raw(self) -> Vec<u8> {
        let mut v = Vec::new();
        self.write(&mut v);
        v
    }
}
