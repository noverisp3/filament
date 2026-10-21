//! The play phase: the one that carries the world.
//!
//! One file per packet, named for the packet and the direction it travels.
//!
//! Only what a client needs to leave the loading screen is here. The ordering
//! matters: upstream `ClientPacketListener.handleLogin` runs a sequence before
//! it will draw anything, and a packet sent out of place is dropped rather than
//! rejected.
//!
//! Every layout was read off a capture of a stock 26.3 server rather than off
//! the decompiled source alone, because several of these carry fields the
//! record definitions do not make obvious. `ClientboundLoginPacket` in
//! particular declares a `CommonPlayerSpawnInfo` that contains no coordinates
//! at all, while the wire bytes show a dimension key, a seed and a game type:
//! the position arrives afterwards, in `ClientboundPlayerPositionPacket`.

use crate::protocol::{DecodeError, VarInt, VarLong};

pub mod add_entity_s2c;
pub mod block_changed_ack_s2c;
pub mod block_destruction_s2c;
pub mod block_update_s2c;
pub mod chunk_batch_finished_s2c;
pub mod chunk_batch_start_s2c;
pub mod chunk_cache_center_s2c;
pub mod chunk_cache_radius_s2c;
pub mod damage_event_s2c;
pub mod difficulty_s2c;
pub mod entity_item_s2c;
pub mod entity_pose_s2c;
pub mod forget_level_chunk_s2c;
pub mod game_event_s2c;
pub mod hurt_animation_s2c;
pub mod initialize_border_s2c;
pub mod keep_alive_s2c;
pub mod level_event_s2c;
pub mod login_s2c;
pub mod move_entity_pos_rot_s2c;
pub mod movement_c2s;
pub mod player_abilities_s2c;
pub mod player_action_c2s;
pub mod player_combat_kill_s2c;
pub mod player_info_add_s2c;
pub mod player_info_remove_s2c;
pub mod player_position_s2c;
pub mod pong_response_s2c;
pub mod remove_entities_s2c;
pub mod respawn_s2c;
pub mod rotate_head_s2c;
pub mod set_default_spawn_s2c;
pub mod set_experience_s2c;
pub mod set_health_s2c;
pub mod set_held_slot_s2c;
pub mod set_time_s2c;
pub mod sound_s2c;
pub mod spawn_info;
pub mod swing_animation_s2c;
pub mod system_chat_s2c;
pub mod take_item_entity_s2c;

pub use add_entity_s2c::{write_add_entity, ITEM_ENTITY_TYPE, PLAYER_ENTITY_TYPE};
pub use block_changed_ack_s2c::write_block_changed_ack;
pub use block_destruction_s2c::write_block_destruction;
pub use block_update_s2c::write_block_update;
pub use chunk_batch_finished_s2c::write_chunk_batch_finished;
pub use chunk_batch_start_s2c::write_chunk_batch_start;
pub use chunk_cache_center_s2c::write_chunk_cache_center;
pub use chunk_cache_radius_s2c::write_chunk_cache_radius;
pub use damage_event_s2c::write_damage_event;
pub use difficulty_s2c::write_difficulty;
pub use entity_item_s2c::{write_entity_item, ENTITY_DATA_ITEM, ENTITY_DATA_SERIALIZER_ITEM_STACK};pub use entity_pose_s2c::{
    write_entity_pose, ENTITY_DATA_POSE, ENTITY_DATA_SERIALIZER_POSE, POSE_CROUCHING,
    POSE_STANDING,
};
pub use forget_level_chunk_s2c::write_forget_chunk;
pub use game_event_s2c::write_game_event;
pub use hurt_animation_s2c::write_hurt_animation;
pub use initialize_border_s2c::write_initialize_border;
pub use keep_alive_s2c::write_keep_alive;
pub use level_event_s2c::{write_level_event, BLOCK_DESTROYED, BLOCK_DESTROYED_QUIET};
pub use login_s2c::{write_login, Login};
pub use move_entity_pos_rot_s2c::write_move_entity_pos_rot;
pub(crate) use move_entity_pos_rot_s2c::{DELTA_SCALE, MAX_DELTA_BLOCKS};
pub use movement_c2s::{ClientMovement, Movement};
pub use player_abilities_s2c::write_player_abilities;
pub use player_action_c2s::{PlayerAction, PlayerActionPacket};
pub use player_combat_kill_s2c::write_player_combat_kill;
pub use player_info_add_s2c::write_player_info_add;
pub(crate) use player_info_add_s2c::action;
pub use player_info_remove_s2c::write_player_info_remove;
pub use player_position_s2c::write_player_position;
pub use pong_response_s2c::write_pong_response;
pub use remove_entities_s2c::write_remove_entities;
pub use respawn_s2c::write_respawn;
pub use rotate_head_s2c::write_rotate_head;
pub use set_default_spawn_s2c::write_set_default_spawn;
pub use set_experience_s2c::write_set_experience;
pub use set_health_s2c::write_set_health;
pub use set_held_slot_s2c::write_set_held_slot;
pub use set_time_s2c::{write_set_time, write_set_time_bare};
pub use sound_s2c::{write_sound, Sound};
pub use spawn_info::SpawnInfo;
pub use swing_animation_s2c::{write_swing_animation, MAIN_HAND, OFF_HAND, SWING_DURATION, SWING_WHACK};
pub use system_chat_s2c::write_system_chat;
pub use take_item_entity_s2c::write_take_item_entity;
pub(crate) use initialize_border_s2c::MAX_BORDER;
pub(crate) use take_item_entity_s2c::TAKE_ITEM_ENTITY;

/// The damage types the game has, by the id the client was told.
pub use damage_event_s2c::damage;

/// The sounds this server knows by name.
pub use sound_s2c::sound;

#[cfg(test)]
mod tests;

/// Marks the end of an entity's metadata.
///
/// A byte, not a VarInt, and 255 rather than -1: the ids above it are unsigned
/// bytes and this is the one value left over.
pub const ENTITY_DATA_END: u8 = 255;

/// The bit in `player_input` that means shift is down.
pub const INPUT_SHIFT: u8 = 32;

/// The stage that says "no crack here any more".
pub const NO_CRACK: i8 = -1;

/// Packet ids in the play state, read from `packets.json`.
pub mod id {
    pub const CHANGE_DIFFICULTY: i32 = 10;
    pub const CHUNK_BATCH_FINISHED: i32 = 11;
    pub const CHUNK_BATCH_START: i32 = 12;
    pub const CHUNKS_BIOMES: i32 = 13;
    pub const DAMAGE_EVENT: i32 = 25;
    pub const GAME_EVENT: i32 = 39;
    pub const INITIALIZE_BORDER: i32 = 44;
    pub const KEEP_ALIVE: i32 = 45;
    pub const LEVEL_CHUNK_WITH_LIGHT: i32 = 46;
    pub const LOGIN: i32 = 50;
    pub const PLAYER_ABILITIES: i32 = 65;
    pub const PLAYER_INFO_REMOVE: i32 = 70;
    pub const PLAYER_INFO_UPDATE: i32 = 71;
    pub const PLAYER_POSITION: i32 = 73;
    pub const SET_CHUNK_CACHE_CENTER: i32 = 96;
    pub const SET_DEFAULT_SPAWN: i32 = 99;
    pub const SET_HELD_SLOT: i32 = 107;
    pub const TAKE_ITEM_ENTITY: i32 = 127;

    // The inventory. container_set_content replaces every slot of a window and
    // container_set_slot changes one, which is what a player watches when a
    // block they just broke appears in their hand.
    pub const CONTAINER_SET_CONTENT: i32 = 18;
    pub const CONTAINER_SET_SLOT: i32 = 20;
    pub const SET_TIME: i32 = 115;

    // Serverbound, from the same file. The keep alive is 28 and not 23: 23 is
    // debug_subscription_request, which the id tables agree on and which looked
    // plausible enough to be worth checking.
    pub const SERVERBOUND_DEBUG_SUBSCRIPTION: i32 = 23;
    pub const SERVERBOUND_KEEP_ALIVE: i32 = 28;
    pub const SERVERBOUND_CLIENT_TICK_END: i32 = 13;
    pub const SERVERBOUND_PLAYER_LOADED: i32 = 44;

    // The client answers a teleport with the position it ended up at: an id,
    // three doubles and two floats. Ignoring it leaves the server believing
    // the player is still where the teleport said, which is not the same place
    // once the client has fallen.
    pub const SERVERBOUND_ACCEPT_TELEPORTATION: i32 = 0;
    // The client pings the server every tick, to time the round trip for the
    // debug screen. The answer is the same eight bytes, sent back.
    pub const SERVERBOUND_PING_REQUEST: i32 = 38;
    // And the answer to it.
    pub const PONG_RESPONSE: i32 = 63;

    // Chat. The client sends what was typed and the server sends what
    // everyone sees.
    pub const SERVERBOUND_CHAT: i32 = 9;
    pub const SYSTEM_CHAT: i32 = 124;

    // A command, with or without the signature that a signed one carries.
    pub const SERVERBOUND_CHAT_COMMAND: i32 = 7;
    pub const SERVERBOUND_CHAT_COMMAND_SIGNED: i32 = 8;

    // The client answers every finished chunk batch with the rate it would
    // like, and the server keeps to it. One float.
    pub const SERVERBOUND_CHUNK_BATCH_RECEIVED: i32 = 11;

    // The four movement packets share one class upstream, which is why their
    // bodies are near identical: three doubles and two floats, and each subclass
    // dropping the fields it does not carry. The ids are consecutive because the
    // registrations are.
    pub const MOVE_PLAYER_POS: i32 = 30;
    pub const MOVE_PLAYER_POS_ROT: i32 = 31;
    pub const MOVE_PLAYER_ROT: i32 = 32;
    pub const MOVE_PLAYER_STATUS_ONLY: i32 = 33;

    // Breaking a block: the client sends this with START_DESTROY when the
    // button goes down and STOP_DESTROY when the block is gone.
    pub const SERVERBOUND_PLAYER_ACTION: i32 = 41;
    // Placing a block: the client sends this when the player clicks a face.
    pub const SERVERBOUND_USE_ITEM_ON: i32 = 66;

    // The clientbound packet that tells the client one block changed.
    pub const BLOCK_UPDATE: i32 = 8;
    // The player's health, and the hunger beside it. The game sends this on
    // arrival, so the hearts are the server's answer rather than the
    // client's own guess.
    pub const SET_HEALTH: i32 = 106;
    // The same for experience: how far into the level, the level, and the
    // total.
    pub const SET_EXPERIENCE: i32 = 105;
    // The player coming back to life. The client throws the world away and
    // asks for it again when this arrives.
    pub const RESPAWN: i32 = 84;
    // The red flash when a player is hurt: which entity, and which way.
    pub const HURT_ANIMATION: i32 = 43;
    // The news that a player died. This is what puts the death screen up: the
    // client does not decide on its own that a health of zero means death, it
    // is told, and this is the telling.
    pub const PLAYER_COMBAT_KILL: i32 = 69;
    // A sound, for a client to play.
    pub const SOUND: i32 = 119;
    // The client asking to come back to life, and asking for its statistics.
    pub const SERVERBOUND_CLIENT_COMMAND: i32 = 12;
    // And that this one is settled, not a guess.
    //
    // Clientbound only: there is no serverbound one, so the server opens this
    // and the client has nothing to reply with.
    pub const BLOCK_CHANGED_ACK: i32 = 4;
    // A hand going through a swing.
    //
    // 123, and not 2. Two is `animate`, a different packet that carries an
    // entity and a single byte: reading the shape from one and the number from
    // the other produces a packet the client reads two bytes short.
    pub const SWING_ANIMATION: i32 = 123;
    // The entity's metadata, which is how a pose travels.
    pub const SET_ENTITY_DATA: i32 = 101;
    // Which keys are down. One byte of bit flags and nothing else.
    pub const SERVERBOUND_PLAYER_INPUT: i32 = 43;
    // The same thing coming back the other way. It is a record with no fields,
    // so the packet is nothing but its id.
    pub const SERVERBOUND_PUNCH: i32 = 46;
    // How far through breaking a block the client should draw the crack.
    pub const BLOCK_DESTRUCTION: i32 = 5;
    // Putting another player into this client's world.
    pub const ADD_ENTITY: i32 = 1;
    // A click in the window the player is holding. The client reports the result
    // of the click rather than describing it, so this is how the server finds
    // out that a player moved something.
    pub const SERVERBOUND_CONTAINER_CLICK: i32 = 18;
    // Which of the nine hotbar slots is selected: one short, and the only time
    // the server hears which item the player is holding.
    pub const SERVERBOUND_SET_CARRIED_ITEM: i32 = 54;
    // Taking them out of it again.
    pub const REMOVE_ENTITIES: i32 = 78;
    // Which way they are looking.
    pub const ROTATE_HEAD: i32 = 85;
    // The tab list: who is here and what they are called.
    // (PLAYER_INFO_UPDATE and PLAYER_INFO_REMOVE are declared above, and both
    // are off by one from each other.)
    // Where they moved, as a delta rather than an absolute position.
    pub const MOVE_ENTITY_POS_ROT: i32 = 55;
    // The drop of a block when it is destroyed.
    pub const LEVEL_EVENT: i32 = 47;
    // The clientbound packet that tells the client to drop a chunk.
    pub const FORGET_LEVEL_CHUNK: i32 = 38;
    // How many chunks the client should keep, in each direction.
    pub const SET_CHUNK_CACHE_RADIUS: i32 = 97;
}

/// Steps over the packet id of a serverbound frame and returns what is left.
///
/// The frame a decoder is handed still carries its id, because that is what the
/// wire carries. Every serverbound decoder here has to step over it first, and
/// every one that forgot was wrong in the same way: the id parses as an
/// ordinary first field and nothing fails until two fields later.
pub fn serverbound_body(frame: &[u8], expected: i32) -> Result<&[u8], DecodeError> {
    let mut body = frame;
    let (id, _) = VarInt::read(&mut body)?;
    if id.0 != expected {
        return Err(DecodeError::UnexpectedEof {
            wanted: "the expected packet",
            got: id.0.unsigned_abs() as usize,
        });
    }
    Ok(body)
}

/// Unpacks a block position from the long the wire carries.
///
/// Three coordinates in one sixty-four bit number, and the order the fields
/// are read in is not the order they are declared in: y is in the low twelve
/// bits, z above it in twenty-six, and x in the top twenty-six.
pub fn unpack_block_pos(packed: i64) -> (i32, i32, i32) {
    // The masks are written as shifts rather than constants so that the sign
    // extension is visible: y's twelve bits reach the top of the word before
    // being shifted back down.
    let x = (packed >> 38) as i32;
    let y = ((packed << 52) >> 52) as i32;
    let z = ((packed << 26) >> 38) as i32;
    (x, y, z)
}

/// Packs a block position back into the long the wire carries.
///
/// The inverse of `unpack_block_pos`, and it has to round trip: a position packed
/// and unpacked is the position that went in.
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    const HORIZONTAL: i64 = (1 << 26) - 1;
    const VERTICAL: i64 = (1 << 12) - 1;

    ((x as i64 & HORIZONTAL) << 38) | ((z as i64 & HORIZONTAL) << 12) | (y as i64 & VERTICAL)
}

/// An angle in the byte the game packs one into.
pub(crate) fn angle_byte(degrees: f32) -> u8 {
    let scaled = (degrees / 360.0 * 256.0).round() as i32;
    (scaled.rem_euclid(256)) as u8
}
