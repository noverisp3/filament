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

use crate::protocol::{DecodeError, VarInt, VarLong};

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

    // Serverbound, from the same file. The keep alive is 28 and not 23: 23 is
    // debug_subscription_request, which the id tables agree on and which looked
    // plausible enough to be worth checking.
    pub const SERVERBOUND_DEBUG_SUBSCRIPTION: i32 = 23;
    pub const SERVERBOUND_KEEP_ALIVE: i32 = 28;
    pub const SERVERBOUND_CLIENT_TICK_END: i32 = 13;
    pub const SERVERBOUND_PLAYER_LOADED: i32 = 44;

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

    // VarInts, not the ints the record declares them as. The declaration is what
    // led to writing four bytes each: the client reads one, stops on the first
    // zero byte of the pair, and reports the remaining six as trailing bytes on
    // the login packet. The capture settles it, and the capture writes the
    // cooldown as a single 00 and the sea level as a five byte VarInt.
    VarInt(portal_cooldown).write(out);
    VarInt(sea_level).write(out);
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

/// One movement packet from the client, decoded.
///
/// The four packets share a class upstream and a shape on the wire: the ones
/// that carry a position open with three doubles, the ones that carry a
/// rotation follow with two floats, and every one of them closes with a byte
/// of flags. Which fields a packet carries depends on its id, and the id is
/// what says which of them below are `Some`.
///
/// The flags byte packs two booleans: bit 0 is whether the player is on the
/// ground, bit 1 is whether it hit a wall on the way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientMovement {
    /// The position, when the packet carries one.
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    /// The rotation in degrees, when the packet carries one.
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    /// Bit 0 of the trailing flags byte.
    pub on_ground: bool,
    /// Bit 1 of the trailing flags byte.
    pub horizontal_collision: bool,
}

impl ClientMovement {
    /// Decodes one of the four movement packets.
    ///
    /// `frame` is the whole packet: the id VarInt first, then the body. The id
    /// decides which fields follow, and an id that is not one of the four is
    /// rejected rather than guessed at.
    pub fn read(frame: &[u8]) -> Result<Self, DecodeError> {
        let mut r: &[u8] = frame;
        let (id, _) = VarInt::read(&mut r)?;
        let id = id.0;

        let has_pos = matches!(id, id::MOVE_PLAYER_POS | id::MOVE_PLAYER_POS_ROT);
        let has_rot = matches!(id, id::MOVE_PLAYER_POS_ROT | id::MOVE_PLAYER_ROT);

        if !matches!(
            id,
            id::MOVE_PLAYER_POS
                | id::MOVE_PLAYER_POS_ROT
                | id::MOVE_PLAYER_ROT
                | id::MOVE_PLAYER_STATUS_ONLY
        ) {
            return Err(DecodeError::UnexpectedEof {
                wanted: "a movement packet id",
                got: id as usize,
            });
        }

        let mut movement = ClientMovement {
            x: None,
            y: None,
            z: None,
            yaw: None,
            pitch: None,
            on_ground: false,
            horizontal_collision: false,
        };

        if has_pos {
            movement.x = Some(read_f64(&mut r)?);
            movement.y = Some(read_f64(&mut r)?);
            movement.z = Some(read_f64(&mut r)?);
        }

        if has_rot {
            movement.yaw = Some(read_f32(&mut r)?);
            movement.pitch = Some(read_f32(&mut r)?);
        }

        let flags = read_u8(&mut r)?;
        movement.on_ground = flags & 1 != 0;
        movement.horizontal_collision = flags & 2 != 0;

        Ok(movement)
    }
}

/// A double straight off the wire.
fn read_f64(r: &mut &[u8]) -> Result<f64, DecodeError> {
    if r.len() < 8 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a double",
            got: r.len(),
        });
    }
    let bytes: [u8; 8] = r[..8].try_into().unwrap();
    *r = &r[8..];
    Ok(f64::from_be_bytes(bytes))
}

/// A float straight off the wire.
fn read_f32(r: &mut &[u8]) -> Result<f32, DecodeError> {
    if r.len() < 4 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a float",
            got: r.len(),
        });
    }
    let bytes: [u8; 4] = r[..4].try_into().unwrap();
    *r = &r[4..];
    Ok(f32::from_be_bytes(bytes))
}

/// One byte straight off the wire.
fn read_u8(r: &mut &[u8]) -> Result<u8, DecodeError> {
    let byte = *r.first().ok_or(DecodeError::UnexpectedEof {
        wanted: "a byte",
        got: r.len(),
    })?;
    *r = &r[1..];
    Ok(byte)
}

/// A signed 64-bit integer straight off the wire, which is how a block position
/// travels: one long holding the x, y and z packed together.
fn read_i64(r: &mut &[u8]) -> Result<i64, DecodeError> {
    if r.len() < 8 {
        return Err(DecodeError::UnexpectedEof {
            wanted: "a long",
            got: r.len(),
        });
    }
    let bytes: [u8; 8] = r[..8].try_into().unwrap();
    *r = &r[8..];
    Ok(i64::from_be_bytes(bytes))
}

/// What the client is asking the server to do to a block.
///
/// Only the three actions that matter for breaking are named; the rest of the
/// enum (dropping items, swapping hands, opening a chest) is not acted on yet,
/// so it reads as a raw number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerAction {
    StartDestroy,
    AbortDestroy,
    StopDestroy,
    Other,
}

/// One `player_action` from the client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerActionPacket {
    pub action: PlayerAction,
    /// The block being acted on, still packed as the wire sent it.
    pub pos: i64,
    /// Which face was clicked, 0 to 5. Unused for now.
    pub direction: u8,
}

impl PlayerActionPacket {
    /// Decodes a `player_action` frame.
    pub fn read(frame: &[u8]) -> Result<Self, DecodeError> {
        let mut r: &[u8] = frame;
        let (id, _) = VarInt::read(&mut r)?;
        if id.0 != id::SERVERBOUND_PLAYER_ACTION {
            return Err(DecodeError::UnexpectedEof {
                wanted: "player_action",
                got: id.0 as usize,
            });
        }

        // The action is an enum, which travels as a VarInt.
        let (raw, _) = VarInt::read(&mut r)?;
        let action = match raw.0 {
            0 => PlayerAction::StartDestroy,
            2 => PlayerAction::AbortDestroy,
            3 => PlayerAction::StopDestroy,
            _ => PlayerAction::Other,
        };

        let pos = read_i64(&mut r)?;
        let direction = read_u8(&mut r)?;

        Ok(Self {
            action,
            pos,
            direction,
        })
    }
}

/// Unpacks a block position from the long the wire carries.
///
/// Three coordinates in one sixty-four bit number. The decompiled source is
/// explicit about the layout and it is not the order the fields are read in:
///
/// ```text
/// PACKED_HORIZONTAL_LENGTH = 26      // enough for thirty million blocks
/// PACKED_Y_LENGTH         = 64 - 52 = 12
/// Y_OFFSET  = 0
/// Z_OFFSET  = 12
/// X_OFFSET  = 38
/// ```
///
/// So y is in the low twelve bits, z above it in twenty-six, and x in the top
/// twenty-six. Reading it as x, y, z top to bottom gives three wrong numbers for
/// three right ones, which is the worst kind of wrong.
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
/// and unpacked is the position that went in, or the block appears somewhere
/// else entirely and is never found again.
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    const HORIZONTAL: i64 = (1 << 26) - 1;
    const VERTICAL: i64 = (1 << 12) - 1;

    ((x as i64 & HORIZONTAL) << 38) | ((z as i64 & HORIZONTAL) << 12) | (y as i64 & VERTICAL)
}

/// Writes a `block_update` for one block.
///
/// The client learns a block changed from this packet alone: a position and the
/// new state at it. Nothing else tells it to rebuild the mesh at that spot, so
/// without it a break the server applied stays invisible.
pub fn write_block_update(out: &mut Vec<u8>, x: i32, y: i32, z: i32, state: i32) {
    VarInt(id::BLOCK_UPDATE).write(out);
    // Eight bytes, always. A block position is a fixed long and not a VarLong,
    // so the value is written out in full whatever its size: a VarLong would be
    // one byte here and the client would read the rest of the packet as the
    // position.
    out.extend_from_slice(&pack_block_pos(x, y, z).to_be_bytes());
    VarInt(state).write(out);
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

/// Writes `ClientboundInitializeBorderPacket`.
///
/// Two sizes, two centres and then four numbers that are not doubles. Reading
/// the record alone suggests seven doubles; the capture says four, followed by a
/// VarLong and three VarInts, and it walks to exactly forty bytes with no
/// remainder. Those are the numbers that matter, so the ordering here is the one
/// the capture shows rather than the one the field declaration implies.
///
/// A zero size means the whole world is playable, which is right for a world
/// that is not finished. The centres are the vanilla maximum, again to mean
/// "no restriction", and the warning fields are what the client would show
/// before the border was drawn at all.
pub fn write_initialize_border(out: &mut Vec<u8>) {
    VarInt(id::INITIALIZE_BORDER).write(out);

    let doubles = [
        0.0,         // old size
        0.0,         // new size
        MAX_BORDER,  // centre x
        MAX_BORDER,  // centre z
    ];
    for value in doubles {
        out.extend_from_slice(&value.to_be_bytes());
    }

    VarLong(0).write(out); // lerp time, in ticks
    VarInt(29_999_984).write(out); // absolute maximum size
    VarInt(5).write(out); // warning distance, in blocks
    VarInt(300).write(out); // warning delay, in ticks
}

/// The centre a border with no restriction is parked at.
///
/// 59_999_968 is the value a stock server sends. Any centre works when the size
/// is zero, but matching the capture keeps the packet byte-identical.
const MAX_BORDER: f64 = 59_999_968.0;

/// Writes `ClientboundSetDefaultSpawnPositionPacket`.
///
/// A dimension, a block position, then a yaw and a pitch. The position is not
/// three doubles: it is a block position, which is packed into a single eight
/// byte value, and writing coordinates as doubles here produces a packet nine
/// bytes too long.
///
/// The pack is x in the low 26 bits, z in the next 26 and y in the top 12.
pub fn write_set_default_spawn(out: &mut Vec<u8>, dimension: &str, x: i32, y: i32, z: i32) {
    VarInt(id::SET_DEFAULT_SPAWN).write(out);
    VarInt(dimension.len() as i32).write(out);
    out.extend_from_slice(dimension.as_bytes());
    out.extend_from_slice(&pack_block_pos(x, y, z).to_be_bytes());
    out.extend_from_slice(&0.0f32.to_be_bytes());
    out.extend_from_slice(&0.0f32.to_be_bytes());
}

/// Not written: `ClientboundSetTimePacket`.
///
/// Left unimplemented on purpose rather than sent wrong. The 26.3 packet is
/// thirty-two bytes for a capture that starts with a long and then carries a map
/// of world clock states, and the record does not make the shape obvious from
/// the outside. Sending an approximation would move the failure to the next
/// packet along and cost another round trip to find, whereas omitting it leaves
/// the client with its own clock, which is the lesser problem.
///
/// The id stays declared so the log can name the packet when it is needed.
#[allow(dead_code)]
fn set_time_is_not_implemented() {}

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
/// Writes `ClientboundPlayerAbilitiesPacket`.
///
/// A bitfield byte and two floats: the flying speed and the walking speed. There
/// is no field for a field of view and no optional marker, which is what an
/// earlier version of this wrote and why the client reported two bytes extra.
///
/// Nothing is granted. The bitfield stays zero, so the player is not
/// invulnerable, is not flying, may not fly and is not in creative mode.
pub fn write_player_abilities(out: &mut Vec<u8>) {
    VarInt(id::PLAYER_ABILITIES).write(out);

    // invulnerable | flying | may fly | instabuild
    out.push(0);

    out.extend_from_slice(&flying_speed().to_be_bytes());
    out.extend_from_slice(&walking_speed().to_be_bytes());
}

/// The default flying speed, 0.05.
///
/// The literal rather than a value read from the player, because there is no
/// player yet: this is what a fresh login is sent.
fn flying_speed() -> f32 {
    0.05
}

/// The default walking speed, 0.1.
fn walking_speed() -> f32 {
    0.1
}

/// Writes `ClientboundSetHeldSlotPacket`.
///
/// A VarInt, not the short an earlier version used. The capture is two bytes
/// for the whole packet, which leaves one for the slot.
pub fn write_set_held_slot(out: &mut Vec<u8>, slot: i32) {
    VarInt(id::SET_HELD_SLOT).write(out);
    VarInt(slot).write(out);
}

/// Writes `ClientboundChunkBatchStartPacket`, empty upstream.
pub fn write_chunk_batch_start(out: &mut Vec<u8>) {
    VarInt(id::CHUNK_BATCH_START).write(out);
}

/// Writes `ClientboundChunkBatchFinishedPacket`.
///
/// The field is how many chunks are in the batch that just finished, as a
/// VarInt. It was a float here for a while, on the assumption that it carried a
/// tick rate, and the client reported the packet as three bytes longer than it
/// expected: five bytes sent, two read. Nothing about it is a rate.
pub fn write_chunk_batch_finished(out: &mut Vec<u8>, batch_size: i32) {
    VarInt(id::CHUNK_BATCH_FINISHED).write(out);
    VarInt(batch_size).write(out);
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
    fn the_cooldown_and_sea_level_are_varints() {
        // This is the field pair that cost a run. Writing them as the ints the
        // record declares them produces a login the client reads six bytes short
        // of, and reports as "login was larger than I expected, found 6 bytes
        // extra". The capture shows the real encoding: a cooldown of zero is one
        // byte, and the sea level is a five byte VarInt because it is negative.
        let login = Login {
            portal_cooldown: 0,
            sea_level: -63,
            ..login_zeroed()
        };
        let mut w = Vec::new();
        write_login(&mut w, &login);

        assert_eq!(
            &w[w.len() - 8..],
            &[0, 0xc1, 0xff, 0xff, 0xff, 0x0f, 0, 0],
            "cooldown as one byte, sea level as five, then the two session flags"
        );
    }

    #[test]
    fn a_login_is_the_length_the_capture_recorded() {
        // 114 bytes for the packet as a stock server sent it: one id, four for
        // the entity, one for hardcore, one count, the three dimension names,
        // three distances, three flags, the holder and its key, the seed, the
        // five spawn fields, two VarInts and the two session flags.
        let mut w = Vec::new();
        write_login(&mut w, &login_6());

        assert_eq!(
            w.len(),
            114,
            "a mismatch here means a field is being written at the wrong width"
        );
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
    fn abilities_are_a_bitfield_and_two_floats() {
        // The capture recorded exactly ten bytes: 41 00 3d 4c cc cd 3d cc cc
        // cd. That is the id, a zero bitfield, 0.05 and 0.1. An earlier version
        // wrote eleven bytes of body and the client reported "found 2 bytes
        // extra" on this packet.
        let mut w = Vec::new();
        write_player_abilities(&mut w);

        assert_eq!(
            w,
            vec![
                id::PLAYER_ABILITIES as u8,
                0x00,
                0x3D, 0x4C, 0xCC, 0xCD,
                0x3D, 0xCC, 0xCC, 0xCD,
            ],
            "byte for byte what the capture recorded"
        );
        assert_eq!(w.len(), 10, "the capture's length for this packet");
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
    fn a_chunk_batch_reports_how_many_it_held() {
        // A VarInt and nothing else. The float this used to be made the packet
        // three bytes too long: five sent, two read.
        let mut w = Vec::new();
        write_chunk_batch_finished(&mut w, 81);

        assert_eq!(w, vec![id::CHUNK_BATCH_FINISHED as u8, 81]);
        assert_eq!(w.len(), 2, "an id and a one byte count");
    }

    #[test]
    fn the_batch_start_is_only_its_id() {
        let mut w = Vec::new();
        write_chunk_batch_start(&mut w);
        assert_eq!(w, vec![id::CHUNK_BATCH_START as u8]);
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
    fn the_default_spawn_is_a_key_a_block_pos_and_two_angles() {
        // Thirty-seven bytes in the capture, and three doubles for the position
        // would make it forty-six.
        let mut w = Vec::new();
        write_set_default_spawn(&mut w, "minecraft:overworld", 0, 64, 0);

        assert_eq!(w[0], id::SET_DEFAULT_SPAWN as u8);
        assert_eq!(w[1], 19, "key length");
        assert_eq!(&w[2..21], b"minecraft:overworld");
        assert_eq!(w.len(), 37, "the capture's length for this packet");
    }

    #[test]
    fn a_break_action_reads_the_block_it_names() {
        let mut frame = Vec::new();
        VarInt(id::SERVERBOUND_PLAYER_ACTION).write(&mut frame);
        VarInt(3).write(&mut frame); // STOP_DESTROY_BLOCK
        frame.extend_from_slice(&pack_block_pos(10, 64, -3).to_be_bytes());
        frame.push(1); // the top face
        VarInt(7).write(&mut frame); // the client's sequence number

        let action = PlayerActionPacket::read(&frame).expect("well formed");

        assert_eq!(action.action, PlayerAction::StopDestroy);
        assert_eq!(unpack_block_pos(action.pos), (10, 64, -3));
        assert_eq!(action.direction, 1);
    }

    #[test]
    fn an_action_that_is_not_a_break_is_named_as_such() {
        let mut frame = Vec::new();
        VarInt(id::SERVERBOUND_PLAYER_ACTION).write(&mut frame);
        VarInt(4).write(&mut frame); // DROP_ALL_ITEMS
        frame.extend_from_slice(&0i64.to_be_bytes());
        frame.push(0);
        VarInt(0).write(&mut frame);

        let action = PlayerActionPacket::read(&frame).expect("well formed");
        assert_eq!(action.action, PlayerAction::Other);
    }

    #[test]
    fn a_block_position_packs_into_one_eight_byte_value() {
        assert_eq!(pack_block_pos(0, 0, 0), 0);
        // y occupies the low twelve bits, so it sits at the bottom.
        assert_eq!(pack_block_pos(0, 1, 0), 1);
        // z starts at bit twelve.
        assert_eq!(pack_block_pos(0, 0, 1), 1 << 12);
        // x starts at bit thirty-eight.
        assert_eq!(pack_block_pos(1, 0, 0), 1 << 38);
    }

    #[test]
    fn a_block_position_survives_the_round_trip() {
        // The spawn, and then its negative neighbours: the world runs from -64
        // upward in y, so the twelve bit field has to carry a negative number
        // without spilling into z.
        for (x, y, z) in [(0, 64, 0), (-1, -64, -1), (30_000_000, 320, -30_000_000)] {
            assert_eq!(
                unpack_block_pos(pack_block_pos(x, y, z)),
                (x, y, z),
                "packing and unpacking must give back the position that went in"
            );
        }
    }

    #[test]
    fn the_border_walks_to_exactly_forty_bytes() {
        // Four doubles, then a VarLong and three VarInts. Seven doubles would be
        // fifty-six.
        let mut w = Vec::new();
        write_initialize_border(&mut w);

        assert_eq!(w[0], id::INITIALIZE_BORDER as u8);
        assert_eq!(w.len(), 41, "the capture's length for this packet");
        assert_eq!(
            &w[1..17],
            &[0u8; 16],
            "the two sizes are zero, so the whole world is playable"
        );
        assert_eq!(
            f64::from_be_bytes(w[17..25].try_into().unwrap()),
            MAX_BORDER,
            "then the two centres, parked at the vanilla maximum"
        );
    }

    #[test]
    fn the_held_slot_is_one_varint() {
        // Two bytes for the whole packet in the capture.
        let mut w = Vec::new();
        write_set_held_slot(&mut w, 0);

        assert_eq!(w, vec![id::SET_HELD_SLOT as u8, 0]);
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
        assert_eq!(id::SERVERBOUND_KEEP_ALIVE, 28);
        assert_eq!(id::SERVERBOUND_CLIENT_TICK_END, 13);
        assert_eq!(id::MOVE_PLAYER_ROT, 32);
    }

    /// One movement packet, hand built the way the client builds them.
    fn movement_packet(id: i32, body: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        VarInt(id).write(&mut frame);
        frame.extend_from_slice(body);
        frame
    }

    #[test]
    fn a_position_packet_reads_its_position_and_flags() {
        let mut body = Vec::new();
        body.extend_from_slice(&8.5f64.to_be_bytes());
        body.extend_from_slice(&64.0f64.to_be_bytes());
        body.extend_from_slice(&(-2.5f64).to_be_bytes());
        body.push(1); // on the ground, no collision

        let moved = ClientMovement::read(&movement_packet(id::MOVE_PLAYER_POS, &body))
            .expect("the packet is well formed");

        assert_eq!(moved.x, Some(8.5));
        assert_eq!(moved.y, Some(64.0));
        assert_eq!(moved.z, Some(-2.5));
        assert_eq!(moved.yaw, None, "a position packet carries no rotation");
        assert!(moved.on_ground);
        assert!(!moved.horizontal_collision);
    }

    #[test]
    fn a_rotation_packet_carries_only_the_angles() {
        let mut body = Vec::new();
        body.extend_from_slice(&90.0f32.to_be_bytes());
        body.extend_from_slice(&(-45.0f32).to_be_bytes());
        body.push(3); // on the ground and into a wall

        let moved = ClientMovement::read(&movement_packet(id::MOVE_PLAYER_ROT, &body))
            .expect("the packet is well formed");

        assert_eq!(moved.x, None, "a rotation packet carries no position");
        assert_eq!(moved.yaw, Some(90.0));
        assert_eq!(moved.pitch, Some(-45.0));
        assert!(moved.on_ground);
        assert!(moved.horizontal_collision);
    }

    #[test]
    fn a_status_only_packet_is_one_byte_of_flags() {
        let moved =
            ClientMovement::read(&movement_packet(id::MOVE_PLAYER_STATUS_ONLY, &[2]))
                .expect("the packet is well formed");

        assert_eq!(moved.x, None);
        assert_eq!(moved.yaw, None);
        assert!(!moved.on_ground);
        assert!(moved.horizontal_collision);
    }

    #[test]
    fn a_packet_that_cuts_off_mid_position_is_rejected() {
        let short = &8.5f64.to_be_bytes()[..6];

        assert!(ClientMovement::read(&movement_packet(
            id::MOVE_PLAYER_POS,
            short
        ))
        .is_err());
    }
}
