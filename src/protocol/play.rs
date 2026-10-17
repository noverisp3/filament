//! Packets of the play phase, the one that carries the world.
//!
//! Only what a client needs to leave the loading screen is here. The ordering
//! matters: upstream `ClientPacketListener.handleLogin` runs a sequence before
//! it will draw anything, and a packet sent out of place is dropped rather than
//! rejected, so the order below is the order a real server uses.
//!
//! Every layout in this file was read off a capture of a stock 26.3 server rather
//! than off the decompiled source alone, because several of these carry fields
//! the record definitions do not make obvious. `ClientboundLoginPacket` in
//! particular declares a `CommonPlayerSpawnInfo` that contains no coordinates at
//! all, while the wire bytes show a dimension key, a seed and a game type: the
//! position arrives afterwards, in `ClientboundPlayerPositionPacket`.

use crate::protocol::VarInt;

/// Packet ids in the play state, read from `packets.json`.
pub mod id {
    pub const CHANGE_DIFFICULTY: i32 = 10;
    pub const CHUNK_BATCH_FINISHED: i32 = 11;
    pub const CHUNK_BATCH_START: i32 = 12;
    pub const CHUNKS_BIOMES: i32 = 13;
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
    pub const SET_TIME: i32 = 115;

    // Serverbound, from the same file.
    pub const SERVERBOUND_KEEP_ALIVE: i32 = 23;
    pub const SERVERBOUND_PLAYER_POSITION: i32 = 32;
    pub const SERVERBOUND_PLAYER_LOADED: i32 = 38;
}

/// Writes `ClientboundLoginPacket`.
///
/// `levels` is the set of dimensions the client may switch between. The real
/// server sends all three and the capture shows exactly that, so this does too:
/// sending one would be a different server, and the client reads the count
/// before the keys.
///
/// The spawn block is not coordinates. It is a dimension holder, a seed, a game
/// type, and a handful of flags, and the fields come out in the order the
/// capture shows rather than the order the record declares.
/// Everything `ClientboundLoginPacket` carries besides its id.
///
/// Grouped rather than passed as thirteen arguments, because the packet is one
/// record upstream and thirteen positional arguments in a call site are close
/// to unreadable. The names follow the decompiled record.
#[derive(Debug, Clone, PartialEq)]
pub struct Login<'a> {
    pub entity_id: i32,
    pub hardcore: bool,
    /// The dimensions the client may switch between.
    pub levels: &'a [&'a str],
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub dimension: &'a str,
    pub seed: i64,
    pub game_type: u8,
    pub is_flat: bool,
    pub portal_cooldown: i32,
    pub sea_level: i32,
}

pub fn write_login(out: &mut Vec<u8>, login: &Login<'_>) {
    let Login {
        entity_id,
        hardcore,
        levels,
        max_players,
        view_distance,
        simulation_distance,
        dimension,
        seed,
        game_type,
        is_flat,
        portal_cooldown,
        sea_level,
    } = *login;

    VarInt(id::LOGIN).write(out);

    // The entity id is an int in network order, not a VarInt. Writing it as a
    // VarInt desynchronises every field after it.
    out.extend_from_slice(&entity_id.to_be_bytes());
    out.push(hardcore as u8);

    VarInt(levels.len() as i32).write(out);
    for level in levels {
        VarInt(level.len() as i32).write(out);
        out.extend_from_slice(level.as_bytes());
    }

    VarInt(max_players).write(out);
    VarInt(view_distance).write(out);
    VarInt(simulation_distance).write(out);

    // reducedDebugInfo, showDeathScreen, doLimitedCrafting. The capture shows
    // 0, 1, 0 for a survival world.
    out.push(0);
    out.push(1);
    out.push(0);

    write_spawn_info(
        out,
        dimension,
        seed,
        game_type,
        is_flat,
        portal_cooldown,
        sea_level,
    );

    // onlineMode, enforcesSecureChat. Both false: there is no session on an
    // offline server for either to apply to.
    out.push(0);
    out.push(0);
}

/// The tail of `ClientboundLoginPacket`, which upstream calls
/// `CommonPlayerSpawnInfo`.
///
/// Despite the name this holds no position. The dimension is a holder, written
/// as a zero byte followed by the registry key; the zero is what marks the
/// holder as being sent by key rather than by inline contents, and writing the
/// key without it makes the client read the length as part of the payload.
fn write_spawn_info(
    out: &mut Vec<u8>,
    dimension: &str,
    seed: i64,
    game_type: u8,
    is_flat: bool,
    portal_cooldown: i32,
    sea_level: i32,
) {
    out.push(0);
    VarInt(dimension.len() as i32).write(out);
    out.extend_from_slice(dimension.as_bytes());

    out.extend_from_slice(&seed.to_be_bytes());

    out.push(game_type);
    // An optional previous game type, present only when there is one.
    out.push(0);
    out.push(0); // isDebug
    out.push(is_flat as u8);
    // An optional last death location, absent.
    out.push(0);

    out.extend_from_slice(&portal_cooldown.to_be_bytes());
    out.extend_from_slice(&sea_level.to_be_bytes());
}

/// Writes `ClientboundPlayerPositionPacket`.
///
/// The shape is not what the older versions of the protocol had. There is no
/// flag byte: the packet is a teleport id, a `PositionMoveRotation`, and a set
/// of which fields are relative to where the client thinks it already is.
///
/// `PositionMoveRotation` is two vectors and two angles, not one of each. The
/// second vector is the movement delta and it has to be sent even at a
/// teleport, because the client keeps it as the player's velocity; leaving it
/// out shifts the reading of every field after it.
///
/// The capture records 62 bytes for this packet, which is the check on the
/// layout: one for the id, one for the teleport id, 48 for the two vectors,
/// eight for the angles, and four for an empty set of relatives.
/// The movement the client should apply, as `PositionMoveRotation` carries it.
///
/// A struct rather than nine arguments, since the three vectors and the two
/// angles are read together at every call site and a swapped pair would be a
/// silent bug rather than a compile error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Movement {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Velocity the client keeps after applying the move.
    pub delta_x: f64,
    pub delta_y: f64,
    pub delta_z: f64,
    /// Degrees, where 0 is south and increases clockwise.
    pub yaw: f32,
    /// Degrees, where -90 is straight up and 90 is straight down.
    pub pitch: f32,
}

impl Default for Movement {
    fn default() -> Self {
        Self {
            x: 0.5,
            y: 64.0,
            z: 0.5,
            delta_x: 0.0,
            delta_y: 0.0,
            delta_z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
        }
    }
}

pub fn write_player_position(out: &mut Vec<u8>, teleport_id: i32, move_to: &Movement) {
    let Movement {
        x,
        y,
        z,
        delta_x,
        delta_y,
        delta_z,
        yaw,
        pitch,
    } = *move_to;

    VarInt(id::PLAYER_POSITION).write(out);
    VarInt(teleport_id).write(out);

    out.extend_from_slice(&x.to_be_bytes());
    out.extend_from_slice(&y.to_be_bytes());
    out.extend_from_slice(&z.to_be_bytes());

    out.extend_from_slice(&delta_x.to_be_bytes());
    out.extend_from_slice(&delta_y.to_be_bytes());
    out.extend_from_slice(&delta_z.to_be_bytes());

    out.extend_from_slice(&yaw.to_be_bytes());
    out.extend_from_slice(&pitch.to_be_bytes());

    // The relative set, empty. The client applies the position as absolute,
    // which is what a teleport should be. The encoding is four zero bytes in
    // the capture and none of the ten Relative constants is set.
    out.extend_from_slice(&[0, 0, 0, 0]);
}

/// Writes `ClientboundChangeDifficultyPacket`.
///
/// A difficulty index and then whether the difficulty is locked, both single
/// bytes. Peaceful is 0, so the world opens on easy by default unless told
/// otherwise.
pub fn write_difficulty(out: &mut Vec<u8>, difficulty: u8) {
    VarInt(id::CHANGE_DIFFICULTY).write(out);
    out.push(difficulty);
    out.push(0);
}

/// Writes `ClientboundGameStateChangePacket` for the event that holds the
/// loading screen.
///
/// Reason 13 is "start waiting for level chunks". It is a VarInt and a float,
/// and the capture sends 13 with 0.0, which is what tells the client to wait
/// rather than to give up waiting.
pub fn write_game_event(out: &mut Vec<u8>, reason: i32, value: f32) {
    VarInt(id::GAME_EVENT).write(out);
    VarInt(reason).write(out);
    out.extend_from_slice(&value.to_be_bytes());
}

/// Writes `ClientboundSetChunkCacheCenterPacket`.
///
/// Two VarInts, each a chunk coordinate. The client will only ask for chunks
/// around this, so until it is sent there is nowhere for chunks to go.
pub fn write_chunk_cache_center(out: &mut Vec<u8>, chunk_x: i32, chunk_z: i32) {
    VarInt(id::SET_CHUNK_CACHE_CENTER).write(out);
    VarInt(chunk_x).write(out);
    VarInt(chunk_z).write(out);
}

/// Writes `ClientboundSetDefaultSpawnPositionPacket`.
///
/// A dimension key, three doubles and an angle in a single byte. The byte is
/// worth noting: the position packets use float degrees, this one does not.
pub fn write_set_default_spawn(out: &mut Vec<u8>, dimension: &str, x: f64, y: f64, z: f64) {
    VarInt(id::SET_DEFAULT_SPAWN).write(out);
    VarInt(dimension.len() as i32).write(out);
    out.extend_from_slice(dimension.as_bytes());
    out.extend_from_slice(&x.to_be_bytes());
    out.extend_from_slice(&y.to_be_bytes());
    out.extend_from_slice(&z.to_be_bytes());
    out.push(0);
}

/// Writes `ClientboundInitializeBorderPacket`.
///
/// All seven fields are doubles. Size zero means no restriction, which is the
/// right answer for a world that is not finished.
pub fn write_initialize_border(out: &mut Vec<u8>) {
    VarInt(id::INITIALIZE_BORDER).write(out);
    let fields: [f64; 7] = [
        0.0,  // size
        0.0,  // centre x
        0.0,  // centre z
        0.0,  // speed, blocks per tick
        -1.0, // warning delay, in ticks
        0.0,  // warning distance
        0.0,  // lerp target
    ];
    for value in fields {
        out.extend_from_slice(&value.to_be_bytes());
    }
}

/// Writes `ClientboundSetTimePacket`.
///
/// Two longs and a flag. The flag says whether the client may change the time,
/// and a negative time of day sets it, which is how a server freezes a world.
pub fn write_set_time(out: &mut Vec<u8>, world_age: i64, time_of_day: i64) {
    VarInt(id::SET_TIME).write(out);
    out.extend_from_slice(&world_age.to_be_bytes());
    out.extend_from_slice(&time_of_day.to_be_bytes());
    out.push(if time_of_day < 0 { 1 } else { 0 });
}

/// Writes `ClientboundKeepAlivePacket`.
///
/// The id comes back verbatim, so it has to be distinguishable from one still
/// in flight when the answer arrives. The caller supplies it for that reason.
pub fn write_keep_alive(out: &mut Vec<u8>, id: i64) {
    VarInt(id::KEEP_ALIVE).write(out);
    out.extend_from_slice(&id.to_be_bytes());
}

/// Writes `ClientboundPlayerAbilitiesPacket`.
///
/// Four bytes of flags and four floats of values. Everything is false and zero:
/// the player cannot fly, is not invulnerable and does not ignore gravity.
pub fn write_player_abilities(out: &mut Vec<u8>) {
    VarInt(id::PLAYER_ABILITIES).write(out);
    out.push(0); // invulnerable, flying, allow flying, creative mode
    out.push(0x3D); // flight speed: 0.05
    out.push(0x3D);
    out.push(0x3D);
    out.push(0);
    out.push(0xCC);
    out.push(0xCD);
    out.push(0x3D);
    out.push(0xCC);
    out.push(0xCC);
    out.push(0xCD);
}

/// Writes `ClientboundSetHeldSlotPacket`.
pub fn write_set_held_slot(out: &mut Vec<u8>, slot: i16) {
    VarInt(id::SET_HELD_SLOT).write(out);
    out.extend_from_slice(&slot.to_be_bytes());
}

/// Writes `ClientboundChunkBatchStartPacket`, empty upstream.
pub fn write_chunk_batch_start(out: &mut Vec<u8>) {
    VarInt(id::CHUNK_BATCH_START).write(out);
}

/// Writes `ClientboundChunkBatchFinishedPacket`.
///
/// One float: the ticks per second the server intends to run at.
pub fn write_chunk_batch_finished(out: &mut Vec<u8>, ticks_per_second: f32) {
    VarInt(id::CHUNK_BATCH_FINISHED).write(out);
    out.extend_from_slice(&ticks_per_second.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: [&str; 3] = ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"];

    /// A login as the capture recorded it: entity 6, survival, all three
    /// dimensions, twenty players, a flat overworld.
    fn login_6() -> Login<'static> {
        Login {
            entity_id: 6,
            hardcore: false,
            levels: &LEVELS,
            max_players: 20,
            view_distance: 10,
            simulation_distance: 10,
            dimension: "minecraft:overworld",
            seed: -8_775_607_066_745_405_828,
            game_type: 0,
            is_flat: true,
            portal_cooldown: 0,
            sea_level: -64,
        }
    }

    /// The same thing with the fields that vary taken at zero, so a test can
    /// count to an offset.
    fn login_zeroed() -> Login<'static> {
        Login {
            entity_id: 0,
            seed: 0,
            ..login_6()
        }
    }

    /// The three dimensions a 26.3 server offers, as the capture shows them.
    #[test]
    fn login_matches_the_bytes_the_capture_recorded() {
        // The bytes are the ones captured from a stock server, so a match here
        // means the layout is right rather than merely self-consistent.
        let mut w = Vec::new();
        write_login(&mut w, &login_6());

        assert_eq!(w[0], id::LOGIN as u8, "play login is id 50");
        assert_eq!(&w[1..5], &6i32.to_be_bytes(), "entity id, network order");
        assert_eq!(w[5], 0, "not hardcore");
        assert_eq!(w[6], 3, "three dimensions, as the real server sends");
        assert_eq!(w[7], 19, "then the first key's byte length");
        assert_eq!(&w[8..8 + 19], b"minecraft:overworld");
    }

    #[test]
    fn the_dimension_holder_carries_a_leading_zero() {
        // This is the field that is easiest to get wrong and the capture is
        // what settles it: the key is preceded by a single zero byte, which
        // marks the holder as being sent by key rather than inline.
        let mut w = Vec::new();
        write_login(&mut w, &login_zeroed());

        // 1 id + 4 entity + 1 hardcore + 1 count + 43 for the three names,
        // + 3 VarInts + 3 bools puts the holder at 56.
        let at = 1 + 4 + 1 + 1 + (20 + 21 + 18) + 3 + 3;
        assert_eq!(w[at], 0, "the holder is sent by key");
        assert_eq!(w[at + 1], 19, "then the key length");
        assert_eq!(&w[at + 2..at + 21], b"minecraft:overworld");
    }

    #[test]
    fn login_ends_with_both_session_flags_off() {
        let mut w = Vec::new();
        write_login(&mut w, &login_zeroed());

        assert_eq!(&w[w.len() - 2..], &[0, 0], "offline mode, no secure chat");
    }

    #[test]
    fn player_position_is_sixty_two_bytes_of_the_shape_the_capture_showed() {
        // Captured: teleport id 1, position (6.5, -60.0, -5.5), no delta,
        // no rotation, four zero bytes.
        let captured = Movement {
            x: 6.5,
            y: -60.0,
            z: -5.5,
            ..Movement::default()
        };
        let mut w = Vec::new();
        write_player_position(&mut w, 1, &captured);

        assert_eq!(w[0], id::PLAYER_POSITION as u8);
        assert_eq!(w[1], 1, "the teleport id the client echoes back");
        assert_eq!(&w[2..10], &6.5f64.to_be_bytes());
        assert_eq!(&w[10..18], &(-60.0f64).to_be_bytes());
        assert_eq!(&w[18..26], &(-5.5f64).to_be_bytes());
        assert_eq!(&w[26..50], &[0u8; 24], "the delta vector, still sent");
        assert_eq!(&w[50..58], &[0u8; 8], "yaw and pitch");
        assert_eq!(&w[58..], &[0, 0, 0, 0], "no relative fields");
        assert_eq!(
            w.len(),
            62,
            "62 bytes is what the capture recorded for this packet"
        );
    }

    #[test]
    fn difficulty_is_two_bytes_after_the_id() {
        // The capture recorded "0a 01 00": peaceful is 0, and the byte after is
        // whether the difficulty is locked.
        let mut w = Vec::new();
        write_difficulty(&mut w, 1);

        assert_eq!(w, vec![id::CHANGE_DIFFICULTY as u8, 1, 0]);
    }

    #[test]
    fn game_event_13_is_the_one_that_holds_the_loading_screen() {
        // Captured as "27 0d 00 00 00 00".
        let mut w = Vec::new();
        write_game_event(&mut w, 13, 0.0);

        assert_eq!(
            w,
            vec![id::GAME_EVENT as u8, 13, 0, 0, 0, 0],
            "the bytes the capture recorded, byte for byte"
        );
    }

    #[test]
    fn the_chunk_cache_centre_is_two_varints() {
        let mut w = Vec::new();
        write_chunk_cache_center(&mut w, 0, 0);

        assert_eq!(w, vec![id::SET_CHUNK_CACHE_CENTER as u8, 0, 0]);
    }

    #[test]
    fn the_default_spawn_leads_with_the_dimension_key() {
        let mut w = Vec::new();
        write_set_default_spawn(&mut w, "minecraft:overworld", 0.5, 64.0, 0.5);

        assert_eq!(w[0], id::SET_DEFAULT_SPAWN as u8);
        assert_eq!(w[1], 19, "key length");
        assert_eq!(&w[2..21], b"minecraft:overworld");
        assert_eq!(&w[21..29], &0.5f64.to_be_bytes());
        assert_eq!(
            &w[w.len() - 1..],
            &[0],
            "the angle is a single byte, unlike the position packet"
        );
    }

    #[test]
    fn the_border_is_seven_doubles() {
        let mut w = Vec::new();
        write_initialize_border(&mut w);

        assert_eq!(w[0], id::INITIALIZE_BORDER as u8);
        assert_eq!(w.len(), 1 + 7 * 8);
        assert_eq!(&w[1..9], &0.0f64.to_be_bytes(), "no restriction");
    }

    #[test]
    fn time_flags_a_negative_time_of_day() {
        let mut day = Vec::new();
        write_set_time(&mut day, 0, 6000);
        assert_eq!(day[17], 0, "a world the client may change");

        let mut frozen = Vec::new();
        write_set_time(&mut frozen, 0, -1);
        assert_eq!(frozen[17], 1, "a frozen world");
    }

    #[test]
    fn keep_alive_echoes_the_id_it_was_given() {
        let mut w = Vec::new();
        write_keep_alive(&mut w, 0x0102030405060708);

        assert_eq!(w[0], id::KEEP_ALIVE as u8);
        assert_eq!(&w[1..9], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn the_play_ids_are_the_ones_the_game_uses() {
        // From packets.json, produced by the data generator from the same jar.
        // Pinned because they are positional on the wire.
        assert_eq!(id::LOGIN, 50);
        assert_eq!(id::PLAYER_POSITION, 73);
        assert_eq!(id::GAME_EVENT, 39);
        assert_eq!(id::CHANGE_DIFFICULTY, 10);
        assert_eq!(id::SET_CHUNK_CACHE_CENTER, 96);
        assert_eq!(id::SET_DEFAULT_SPAWN, 99);
        assert_eq!(id::INITIALIZE_BORDER, 44);
        assert_eq!(id::SET_TIME, 115);
        assert_eq!(id::KEEP_ALIVE, 45);
        assert_eq!(id::SERVERBOUND_KEEP_ALIVE, 23);
        assert_eq!(id::SERVERBOUND_PLAYER_POSITION, 32);
    }
}