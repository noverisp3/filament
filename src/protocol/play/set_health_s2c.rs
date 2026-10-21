//! The player's health, and the hunger beside it.

use super::id;
use crate::protocol::VarInt;

/// Writes the health and the food.
pub fn write_set_health(out: &mut Vec<u8>, health: f32, food: i32, saturation: f32) {
    VarInt(id::SET_HEALTH).write(out);
    out.extend_from_slice(&health.to_be_bytes());
    VarInt(food).write(out);
    out.extend_from_slice(&saturation.to_be_bytes());
}
