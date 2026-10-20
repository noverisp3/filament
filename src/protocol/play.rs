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
    // 123, and not 2. Two is `animate`, a different packet that happens to
    // sound related: it carries an entity and a single byte. Reading the shape
    // from `ClientboundSwingAnimationPacket` and the number from the packet
    // called `animate` produces a packet that decodes as the other one and
    // leaves the client two bytes short of the end, which is how this was
    // found.
    pub const SWING_ANIMATION: i32 = 123;
    // The entity's metadata, which is how a pose — and so a crouch — travels.
    pub const SET_ENTITY_DATA: i32 = 101;
    // Which keys are down. One byte of bit flags and nothing else.
    pub const SERVERBOUND_PLAYER_INPUT: i32 = 43;
    // The same thing coming back the other way. It is a record with no fields,
    // so the packet is nothing but its id — which is worth knowing before
    // writing a decoder for it and finding nothing to read.
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
    // (PLAYER_INFO_UPDATE and PLAYER_INFO_REMOVE are declared above, next to the
    // other clientbound ids, and both are off by one from each other.)
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
/// wire carries and what `FrameEncoder::encode` wants back. Every serverbound
/// decoder here has to step over it first, and every one that forgot was wrong in
/// the same way: the id is a small number, so it parses as a perfectly ordinary
/// first field, and nothing fails until two or three fields later, where the
/// complaint is about something that was never wrong.
///
/// Three separate decoders made this mistake, so it is written once here rather
/// than left to be remembered four times. Checking the id as well as stepping
/// over it means a packet that is not the one being decoded is refused rather
/// than decoded as if it were.
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

/// Writes `set_chunk_cache_radius`, telling the client how far the world goes.
///
/// Without it the client is never told how many chunks to hold, and falls back
/// to a radius of its own that has nothing to do with what this server sends.
/// It then keeps chunks that were never streamed and drops ones that were, and
/// which of the two it does depends on the radius it happens to assume rather
/// than the one the world is actually built to.
///
/// One VarInt, read straight back out of the packet by the client.
pub fn write_chunk_cache_radius(out: &mut Vec<u8>, radius: i32) {
    VarInt(id::SET_CHUNK_CACHE_RADIUS).write(out);
    VarInt(radius).write(out);
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
        &SpawnInfo {
            dimension,
            seed,
            game_type,
            is_flat,
            portal_cooldown,
            sea_level,
        },
    );

    // onlineMode, enforcesSecureChat. Both false: there is no session on an
    // offline server for either to apply to.
    out.push(0);
    out.push(0);
}

/// The spawn information a client is given about the world it is joining.
///
/// The same shape in the login packet and in the respawn packet: the game
/// calls it `CommonPlayerSpawnInfo` and it holds the dimension, the seed and
/// the rules of the world rather than a position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnInfo<'a> {
    pub dimension: &'a str,
    pub seed: i64,
    pub game_type: u8,
    pub is_flat: bool,
    pub portal_cooldown: i32,
    pub sea_level: i32,
}

/// Writes the tail of `ClientboundLoginPacket`, which upstream calls
/// `CommonPlayerSpawnInfo`.
///
/// Despite the name this holds no position. The dimension is a holder, written
/// as a zero byte followed by the registry key; the zero is what marks the
/// holder as being sent by key rather than by inline contents, and writing the
/// key without it makes the client read the length as part of the payload.
fn write_spawn_info(out: &mut Vec<u8>, info: &SpawnInfo<'_>) {
    let SpawnInfo {
        dimension,
        seed,
        game_type,
        is_flat,
        portal_cooldown,
        sea_level,
    } = *info;
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
    /// The client's own prediction number for this action.
    ///
    /// The last field on the wire, and the one that was being dropped: a
    /// player that breaks a block has already applied the change locally and is
    /// waiting to be told it was right. It tags its guess with a number it owns,
    /// and the server has to hand that number back for the guess to be settled
    /// and the shape the block used to have to be thrown away.
    ///
    /// Without it the block is drawn gone and stays solid, and a player stands
    /// on air.
    pub sequence: i32,
}

impl PlayerActionPacket {
    /// Decodes a `player_action` frame.
    pub fn read(frame: &[u8]) -> Result<Self, DecodeError> {
        let mut r = serverbound_body(frame, id::SERVERBOUND_PLAYER_ACTION)?;

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

        // The client's prediction number, last on the wire. It was being
        // dropped, and dropping it is what left a broken block drawn but solid.
        let (sequence, _) = VarInt::read(&mut r)?;

        Ok(Self {
            action,
            pos,
            direction,
            sequence: sequence.0,
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

/// Writes a `forget_level_chunk`, telling the client to drop a chunk.
///
/// Without this the client's chunk cache only ever grows, and the server's
/// memory goes with it: every chunk ever visited stays loaded on both sides
/// for the length of the session.
///
/// The coordinate is one long, not two VarInts. That is not a guess about this
/// packet being different: the neighbouring ones genuinely differ.
/// `set_chunk_cache_center` is two VarInts and `level_chunk_with_light` is two
/// int32s, and this is a `ChunkPos`, which packs into a fixed eight bytes with x
/// in the low half and z in the high half. Writing VarInts here makes a packet
/// that decodes as far as the id and no further.
pub fn write_forget_chunk(out: &mut Vec<u8>, chunk_x: i32, chunk_z: i32) {
    VarInt(id::FORGET_LEVEL_CHUNK).write(out);
    let packed = (chunk_x as i64 & 0xFFFF_FFFF) | ((chunk_z as i64 & 0xFFFF_FFFF) << 32);
    out.extend_from_slice(&packed.to_be_bytes());
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

pub fn write_pong_response(out: &mut Vec<u8>, time: i64) {
    VarInt(id::PONG_RESPONSE).write(out);
    // The same eight bytes the client sent, unchanged: the payload is a fixed
    // long on both sides, and the client works the round trip out of the
    // difference between the two.
    out.extend_from_slice(&time.to_be_bytes());
}

/// Writes a line of chat.
///
/// The content is a component, and a component in a packet is one NBT tag:
/// a plain text one is a string tag, which is what a whole line of chat is.
/// The byte after it says whether the line is an overlay above the world
/// rather than in the chat box, and it is not one.
pub fn write_system_chat(out: &mut Vec<u8>, text: &str) {
    VarInt(id::SYSTEM_CHAT).write(out);
    crate::world::nbt::write_string_tag(out, text);
    out.push(0);
}

pub fn write_set_health(out: &mut Vec<u8>, health: f32, food: i32, saturation: f32) {
    VarInt(id::SET_HEALTH).write(out);
    out.extend_from_slice(&health.to_be_bytes());
    VarInt(food).write(out);
    out.extend_from_slice(&saturation.to_be_bytes());
}

pub fn write_set_experience(out: &mut Vec<u8>, progress: f32, level: i32, total: i32) {
    VarInt(id::SET_EXPERIENCE).write(out);
    out.extend_from_slice(&progress.to_be_bytes());
    VarInt(level).write(out);
    VarInt(total).write(out);
}

/// A sound to play, and where it comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sound {
    /// The registry id of the sound.
    pub id: i32,
    /// Which slider it plays on: `sound::SOURCE_PLAYER` and the rest.
    pub source: i32,
    /// Where it comes from, in blocks.
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// How loud, and how high pitched.
    pub volume: f32,
    pub pitch: f32,
    /// A seed the client uses to vary the sound, so two of the same do not
    /// sound identical.
    pub seed: i64,
}

/// Writes a sound, so a client can hear it.
///
/// The sound is a registry id, the source is which slider it plays on, and
/// then the place it comes from, how loud, how high pitched, and a seed the
/// client uses to vary the sound so two of the same do not sound identical.
pub fn write_sound(out: &mut Vec<u8>, sound: &Sound) {
    VarInt(id::SOUND).write(out);
    VarInt(sound.id).write(out);
    VarInt(sound.source).write(out);
    out.extend_from_slice(&sound.x.to_be_bytes());
    out.extend_from_slice(&sound.y.to_be_bytes());
    out.extend_from_slice(&sound.z.to_be_bytes());
    out.extend_from_slice(&sound.volume.to_be_bytes());
    out.extend_from_slice(&sound.pitch.to_be_bytes());
    out.extend_from_slice(&sound.seed.to_be_bytes());
}

/// The sound of a player being hurt, and of one dying.
///
/// The ids are the registry ids the client knows them by, taken from the data
/// generator's report (`extracted/registries.json`, the `minecraft:sound_event`
/// registry), because a sound is a number on the wire and the number has to be
/// the one the client has.
pub mod sound {
    /// A player being hurt.
    pub const PLAYER_HURT: i32 = 1316;
    /// A player dying.
    pub const PLAYER_DEATH: i32 = 1315;
    /// A block of grass being broken.
    pub const GRASS_BREAK: i32 = 759;
    /// A block of stone being broken.
    pub const STONE_BREAK: i32 = 1600;
    /// A block of gravel being broken.
    pub const GRAVEL_BREAK: i32 = 764;
    /// A block of wood being broken.
    pub const WOOD_BREAK: i32 = 1857;

    /// Which slider a sound plays on. The order is the enum's own.
    pub const SOURCE_PLAYER: i32 = 6;
    pub const SOURCE_BLOCK: i32 = 3;
}

pub fn write_hurt_animation(out: &mut Vec<u8>, entity_id: i32, yaw: f32) {
    VarInt(id::HURT_ANIMATION).write(out);
    VarInt(entity_id).write(out);
    // Which way the hit came from, in degrees. Zero is straight ahead, which
    // is what a fall from above looks like.
    out.extend_from_slice(&yaw.to_be_bytes());
}

/// Writes the news that a player died.
///
/// The client does not work out on its own that a health of zero means
/// death: it is told, and this is the telling. The message is the line the
/// chat shows, and the entity id is the player it happened to.
pub fn write_player_combat_kill(out: &mut Vec<u8>, player_id: i32, message: &str) {
    VarInt(id::PLAYER_COMBAT_KILL).write(out);
    VarInt(player_id).write(out);
    // A component, which in a packet is one NBT tag: a plain text one is a
    // string tag.
    crate::world::nbt::write_string_tag(out, message);
}

/// Writes the packet that puts a dead player back in the world.
///
/// The tail is the same spawn information the login packet carries, and the
/// byte after it says what the client should keep: the game keeps the
/// attributes and the entity data, which is all of it.
pub fn write_respawn(out: &mut Vec<u8>, info: &SpawnInfo<'_>, keep_all_data: bool) {
    VarInt(id::RESPAWN).write(out);
    write_spawn_info(out, info);
    out.push(if keep_all_data { 3 } else { 0 });
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

/// `ClientboundSetTimePacket`, round-tripped by hand.
///
/// It was left unimplemented on purpose rather than sent wrong: the map of world
/// clock states is not obvious from the outside, and an approximation would have
/// moved the failure to the next packet along. Reading the reference is how it
/// stopped being a guess -- `ClientboundSetTimePacket`, `ClockNetworkState`,
/// and `ClientClockManager`, which is where the client was found to run the
/// clock itself between packets.
#[test]
fn set_time_carries_the_game_clock_and_where_the_sun_is() {
    // The two numbers are not the same number, and the failure mode of sending
    // one where two belong is a sky stuck at dawn forever.
    let mut out = Vec::new();
    write_set_time(&mut out, 12_345, 6_000);

    // An explicit cursor rather than re-slicing, because `VarInt::read` takes
    // `&mut &[u8]` and advances it itself -- re-slicing by the length it
    // returns skips a second time, which is a silent off-by-one that reads the
    // next field and blames it.
    fn var_int(out: &[u8], at: &mut usize) -> i32 {
        let (v, n) = VarInt::read(&mut &out[*at..]).expect("varint");
        *at += n;
        v.0
    }

    let mut at = 0usize;
    assert_eq!(var_int(&out, &mut at), id::SET_TIME, "the packet");

    let mut eight = [0u8; 8];
    eight.copy_from_slice(&out[at..at + 8]);
    at += 8;
    assert_eq!(i64::from_be_bytes(eight), 12_345, "how long the world has run");

    assert_eq!(var_int(&out, &mut at), 1, "one world clock, the overworld");
    assert_eq!(
        var_int(&out, &mut at),
        WORLD_CLOCK_OVERWORLD,
        "keyed by registry id, and the overworld is 0"
    );

    let (stored, n) = VarLong::read(&mut &out[at..]).expect("sun position");
    at += n;
    // `VarLong` is a plain varlong -- a magnitude, no sign -- so what comes back
    // is still folded. Folding doubles a small number, and folding is what keeps
    // -1 at one byte instead of ten.
    assert_eq!(stored.0, 12_000, "zigzagged on the way out");
    assert_eq!(stored.0 / 2, 6_000, "which folds back to the sun's position");

    let mut four = [0u8; 4];
    four.copy_from_slice(&out[at..at + 4]);
    at += 4;
    assert_eq!(
        f32::from_be_bytes(four),
        0.0,
        "no sub-tick, because the server has none to report"
    );

    four.copy_from_slice(&out[at..at + 4]);
    at += 4;
    assert_eq!(f32::from_be_bytes(four), 1.0, "and time is moving");

    assert_eq!(at, out.len(), "and nothing else trails it");
}

#[test]
fn the_periodic_beat_carries_an_age_and_no_clock() {
    // The game's own timeSync is a bare age: `ClientboundSetTimePacket(getGameTime(),
    // Map.of())`. Replacing the client's clock with the server's copy on every
    // beat is what a sky that steps once a second looked like on the wire, so
    // the map stays empty here and the count alone is corrected.
    let mut out = Vec::new();
    write_set_time_bare(&mut out, 12_345);

    let mut at = 0usize;
    let (v, n) = VarInt::read(&mut &out[at..]).expect("packet id");
    at += n;
    assert_eq!(v.0, id::SET_TIME, "the packet");

    let mut eight = [0u8; 8];
    eight.copy_from_slice(&out[at..at + 8]);
    at += 8;
    assert_eq!(i64::from_be_bytes(eight), 12_345, "how long the world has run");

    let (count, n) = VarInt::read(&mut &out[at..]).expect("update count");
    at += n;
    assert_eq!(count.0, 0, "no clock update: the client's own copy stands");

    assert_eq!(at, out.len(), "and nothing else trails it");
}

#[test]
fn zigzag_keeps_the_numbers_near_zero_small() {
    // A clock that has just been set sits near zero, and -1 costing ten bytes
    // is what a plain varlong does to it.
    assert_eq!(VarLong::zigzag(0).written_size(), 1);
    assert_eq!(VarLong::zigzag(-1).written_size(), 1, "not ten");
    assert_eq!(VarLong::zigzag(1).written_size(), 1);
    assert_eq!(VarLong::zigzag(23_999).written_size(), 3, "the length of a day");
}

/// Writes a `block_destruction`: how far through breaking a block is.
///
/// Stages are zero to nine, and minus one means there is no longer a crack on
/// that block. The minus one is written as a byte, so it is 255 on the wire and
/// comes back through an unsigned read as 255 too; it is signed on the way out.
///
/// The id is the entity that is doing the breaking. The client does not act on
/// it, but the game uses it to leave the breaker out of its own broadcast.
pub fn write_block_destruction(out: &mut Vec<u8>, breaker: i32, pos: (i32, i32, i32), stage: i8) {
    VarInt(id::BLOCK_DESTRUCTION).write(out);
    VarInt(breaker).write(out);
    out.extend_from_slice(&pack_block_pos(pos.0, pos.1, pos.2).to_be_bytes());
    out.push(stage as u8);
}

/// The entity type id for a player, in the registry the client was sent.
///
/// Read out of the data generator's report, where the entry carries an explicit
/// protocol id and the 161 of them run from 0 to 160 with nothing missing. That
/// matters: the client numbers this registry by the order it arrived in the
/// login frames, so an id that is off by one is a client trying to spawn the
/// wrong thing and, most likely, refusing to spawn anything at all.
pub const PLAYER_ENTITY_TYPE: i32 = 159;

/// The actions of `player_info_update`, by their position in the enum.
///
/// Named rather than written as bare bit numbers because the numbers are the
/// whole difficulty of this packet and they are not guessable: the enum has
/// exactly eight members, so the mask that selects them is **one byte**, and
/// getting an ordinal wrong does not fail here, it fails on the client, which
/// then reads a name out of the middle of a uuid.
mod action {
    /// A name and the properties of its profile.
    pub const ADD_PLAYER: u8 = 0;
    /// A game mode index.
    pub const UPDATE_GAME_MODE: u8 = 2;
    /// Whether the player appears in the tab list at all.
    pub const UPDATE_LISTED: u8 = 3;
    /// A ping in milliseconds.
    pub const UPDATE_LATENCY: u8 = 4;
}

/// Writes a `player_info_update`, the tab list.
///
/// The shape, and the part that is easy to get wrong:
///
/// ```text
/// 1 byte    a fixed-width bit set, one bit per action
/// VarInt    how many entries follow
///   for each entry:
///     16 bytes  the profile's uuid, always
///     then only the fields of the actions that were set, in enum order
/// ```
///
/// The mask is a **fixed-width bit set and not a VarInt**. Upstream writes it
/// with `writeFixedBitSet`, which emits `ceil(values.length / 8)` raw bytes,
/// least significant bit first. With eight actions that is one byte, and a
/// two-byte VarInt where one byte belongs reads the entry count out of the
/// action bits and the name out of the middle of the uuid.
///
/// Four actions are set, which is the smallest set that leaves a player a
/// client can name, list, play as and show a ping for:
///
/// - add player: the name, then the profile's properties. Properties are a
///   count and that many entries, and an offline profile owns none.
/// - update game mode: a VarInt, and survival is zero.
/// - update listed: one boolean. Without this a player is in the tab list's
///   data and not in the tab list.
/// - update latency: a VarInt, and zero is fine.
///
/// The rest of the enum — chat, display name, list order, hat — exists and is
/// not set. Fields are written in enum order, which is why listed comes after
/// the game mode and not where it would read most naturally.
pub fn write_player_info_add(out: &mut Vec<u8>, uuid: [u8; 16], name: &str) {
    use action::*;

    let mut mask = 0u8;
    for bit in [ADD_PLAYER, UPDATE_GAME_MODE, UPDATE_LISTED, UPDATE_LATENCY] {
        mask |= 1 << bit;
    }

    VarInt(id::PLAYER_INFO_UPDATE).write(out);

    // Eight actions, so one byte. Asserted rather than assumed: a ninth action
    // in a future version would make this two bytes and shift everything after
    // it, which is a crash on the client and not on this side.
    const ACTIONS: usize = 8;
    assert_eq!(
        ACTIONS.div_ceil(8),
        1,
        "the action enum changed size and the mask is no longer one byte"
    );
    out.push(mask);

    VarInt(1).write(out);

    // The profile's uuid, before any action field.
    out.extend_from_slice(&uuid);

    // ADD_PLAYER: the name, then the profile's properties, which are none.
    // A string on the wire is its byte length as a VarInt and then its UTF-8.
    // The length is in bytes and not in characters, which matters the moment
    // somebody has an accent in their name. The client caps it at 48, which is
    // sixteen characters of three bytes each.
    VarInt(name.len() as i32).write(out);
    out.extend_from_slice(name.as_bytes());
    VarInt(0).write(out);

    // UPDATE_GAME_MODE: survival is zero.
    VarInt(0).write(out);

    // UPDATE_LISTED: true.
    out.push(1);

    // UPDATE_LATENCY: zero.
    VarInt(0).write(out);
}

/// Writes a `player_info_remove`, taking players out of the tab list.
///
/// Sent alongside `remove_entities` and for the same reason: an entry left in
/// the tab list for a player who is gone is a client that keeps a name for
/// somebody who has left.
///
/// By uuid, and not by entity id, which is the same reason the update above is:
/// the list is keyed by profile, and the entity id is only ever something this
/// side remembers.
pub fn write_player_info_remove(out: &mut Vec<u8>, uuids: &[[u8; 16]]) {
    VarInt(id::PLAYER_INFO_REMOVE).write(out);
    VarInt(uuids.len() as i32).write(out);
    for uuid in uuids {
        out.extend_from_slice(uuid);
    }
}

/// Writes an `add_entity`: putting somebody into the world.
///
/// The three angles are a byte each, and they are **quantised to 1/256 of a
/// turn**, so 90.0 degrees is 64 and not 90. A full circle is 256.
///
/// The movement delta is not three doubles and not a long. It is a quantised
/// vector, and its length depends on the value:
///
/// - smaller than about 3e-5 in every component: **one byte**, zero;
/// - otherwise six bytes — a byte, a byte and an int, holding fifteen bits a
///   component with the scale folded into the low bits — plus a VarInt for the
///   scale when it does not divide evenly.
///
/// Writing a fixed eight bytes for it, which reads like a packed long and is
/// what the constant names suggest, makes every client's packet eight bytes
/// longer than expected and drops the connection on the second player.
///
/// A player being spawned has not moved yet, so the delta is zero and the one
/// byte is the whole of it. Movement after this arrives as its own packet with
/// relative deltas and never needs the other branch.
pub fn write_add_entity(
    out: &mut Vec<u8>,
    entity_id: i32,
    uuid: [u8; 16],
    entity_type: i32,
    pos: (f64, f64, f64),
    yaw: f32,
    pitch: f32,
) {
    VarInt(id::ADD_ENTITY).write(out);
    VarInt(entity_id).write(out);
    out.extend_from_slice(&uuid);
    VarInt(entity_type).write(out);

    out.extend_from_slice(&pos.0.to_be_bytes());
    out.extend_from_slice(&pos.1.to_be_bytes());
    out.extend_from_slice(&pos.2.to_be_bytes());

    // The delta, which is zero, which is one byte.
    out.push(0);

    // Three angles, and pitch *before* yaw. The order is the packet.
    //
    // ```java
    // output.writeByte(this.xRot);
    // output.writeByte(this.yRot);
    // output.writeByte(this.yHeadRot);
    // ```
    //
    // There is no head *pitch*: three angles and not four, and a fourth byte is
    // one too many on every entity ever spawned. But the first two are pitch
    // then yaw, and writing them the other way round leaves a body that faces
    // wherever it happened to be looking and looks wherever it happened to be
    // facing.
    //
    // It shows on another player's screen only, and only until they move: the
    // movement packet has the pair the right way round, so one step from the
    // other player fixes what the spawn got wrong. That is why it survived a
    // read-back test -- the reader in that test skipped all three angles
    // without reading them.
    out.push(angle_byte(pitch));
    out.push(angle_byte(yaw));
    out.push(angle_byte(yaw));

    // The data value: zero for a player, one for living things generally.
    VarInt(1).write(out);
}

/// Writes a `remove_entities`, taking them out of the world.
pub fn write_remove_entities(out: &mut Vec<u8>, entity_ids: &[i32]) {
    VarInt(id::REMOVE_ENTITIES).write(out);
    VarInt(entity_ids.len() as i32).write(out);
    for id in entity_ids {
        VarInt(*id).write(out);
    }
}

/// The hand a swing came from. Zero is the one most people use.
pub const MAIN_HAND: i32 = 0;
/// The off hand, which is what a client sends when shift is held while swinging.
pub const OFF_HAND: i32 = 1;

/// How a swing looks.
///
/// Read out of `SwingAnimationType`, which is a small enum with an explicit id
/// per value: none is zero, a sideways hit is one, and a straight stab is two.
/// A player clicking empty air does the first thing in that list.
pub const SWING_WHACK: i32 = 1;

/// How long a swing lasts, in ticks.
///
/// The game's own default, from `SwingAnimation.DEFAULT`. It is not a duration
/// the client ignores: the arm comes down on it, and a swing that says zero is
/// an arm that does not move.
pub const SWING_DURATION: i32 = 6;

/// The bit in `player_input` that means shift is down.
pub const INPUT_SHIFT: u8 = 32;

/// Standing, and the first of the game's pose values.
pub const POSE_STANDING: i32 = 0;
/// Crouching, which is the sixth. A count from zero, so this is a number and
/// not a flag.
pub const POSE_CROUCHING: i32 = 5;

/// Which piece of an entity's metadata the pose is.
///
/// Six, not five. It is counted by the order the fields are declared on the
/// entity, and the decompiled source in `mc-ref` lists seven of them where the
/// shipped jar has eight — a `DATA_CUSTOM_NAME` that is not in the source. Read
/// out of the jar's static initialiser instead, where the order is the order
/// they are actually assigned.
pub const ENTITY_DATA_POSE: u8 = 6;

/// Which serializer reads the pose, out of the table the game registers them in.
///
/// Twenty. Also from the jar's static initialiser, which is the only place the
/// order exists: the table is filled by a method that is called from nowhere in
/// the decompiled tree, and it is not a datapack registry, so it is in neither
/// the source nor the capture. Counting the field declarations in the source
/// gives fifteen, which is wrong — the table has two entries the declaration
/// order hides, an optional component and a plural particles.
pub const ENTITY_DATA_SERIALIZER_POSE: i32 = 20;

/// Marks the end of an entity's metadata.
///
/// A byte, not a VarInt, and 255 rather than -1: the ids above it are unsigned
/// bytes and this is the one value left over.
pub const ENTITY_DATA_END: u8 = 255;

/// Writes an entity's pose, which is how a crouch reaches other people.
///
/// ```text
/// VarInt   the entity id
/// byte     which piece of metadata this is
/// VarInt   which serializer reads what follows
/// VarInt   the pose
/// byte     255, and nothing after it
/// ```
///
/// One item per packet, and the end marker every time. Metadata is a list of
/// updates terminated by a value that cannot be an index, and sending the marker
/// is not optional: a packet that stops after the pose is a packet the client
/// is still waiting on.
pub fn write_entity_pose(out: &mut Vec<u8>, entity_id: i32, crouching: bool) {
    VarInt(id::SET_ENTITY_DATA).write(out);
    VarInt(entity_id).write(out);
    out.push(ENTITY_DATA_POSE);
    VarInt(ENTITY_DATA_SERIALIZER_POSE).write(out);
    VarInt(if crouching { POSE_CROUCHING } else { POSE_STANDING }).write(out);
    out.push(ENTITY_DATA_END);
}

/// Writes a `swing_animation`, so somebody else's swing is visible.
///
/// The player's own client already plays their swing when they click, from the
/// same packet that told us they clicked. This is the other half: what everyone
/// watching sees.
///
/// ```text
/// VarInt   the entity id
/// VarInt   which hand
/// VarInt   what the swing looks like
/// VarInt   how long it lasts, in ticks
/// ```
///
/// The last two are one field upstream, a record of a type and a duration, but
/// on the wire they are two VarInts.
pub fn write_swing_animation(
    out: &mut Vec<u8>,
    entity_id: i32,
    hand: i32,
    kind: i32,
    duration: i32,
) {
    VarInt(id::SWING_ANIMATION).write(out);
    VarInt(entity_id).write(out);
    VarInt(hand).write(out);
    VarInt(kind).write(out);
    VarInt(duration).write(out);
}

/// Writes a `block_changed_ack`, which is how a block change stops being a guess.
///
/// The client applies a block change straight away -- it draws the block gone
/// before the player has finished swinging, which is the whole point of the
/// prediction. What it does not do straight away is throw away the shape the
/// block used to have. That is settled when this arrives, and until it does the
/// block is invisible and still solid: a player stands on air and walks into a
/// wall that is not drawn.
///
/// It exists in only one direction. There is no serverbound packet of this name,
/// so the server sends it and the client has nothing to answer with.
///
/// One VarInt, a sequence number counting the changes this connection has been
/// told are final. The client reads it as a counter and not as a position or an
/// id, so what matters is that it goes up.
pub fn write_block_changed_ack(out: &mut Vec<u8>, sequence: i32) {
    VarInt(id::BLOCK_CHANGED_ACK).write(out);
    VarInt(sequence).write(out);
}

/// Writes a `rotate_head`, so a client knows which way somebody is looking.
pub fn write_rotate_head(out: &mut Vec<u8>, entity_id: i32, yaw: f32) {
    VarInt(id::ROTATE_HEAD).write(out);
    VarInt(entity_id).write(out);
    out.push(angle_byte(yaw));
}

/// How finely a movement delta is measured: one part in 4096 of a block.
///
/// From the game's own codec, which rounds on the way out and divides on the
/// way back in. That is finer than a block, so a slow walk is a delta of a few
/// hundred rather than of zero.
const DELTA_SCALE: f64 = 4096.0;

/// The largest movement a delta can express, in blocks.
///
/// Three shorts at 1/4096 of a block a unit: 32767 of them is a little under
/// eight blocks. Anything past that does not fit in the packet, and it is the
/// number that decides when a movement has to be sent as a position instead.
pub const MAX_DELTA_BLOCKS: f64 = 32767.0 / DELTA_SCALE;

/// Writes a `move_entity_pos_rot`: somebody moved.
///
/// **A delta, not a position.** Each client is told how far the body moved since
/// that client last heard about it, which is why the server has to remember what
/// each observer was last told separately — two players watching the same walk
/// have been told different amounts of it.
///
/// Returns false without writing anything when the movement is too large to be
/// a delta, which is the case for anything past about eight blocks. The caller
/// has to send the position outright instead. Clamping is not an option: it
/// moves the body a plausible-looking amount that is simply wrong, and a body
/// that is slightly wrong keeps being slightly wrong, because every later delta
/// is measured from where the client thinks it is. A body that fell on arrival
/// descends eight blocks per packet instead of forty and then stops, which is
/// exactly what it did.
///
/// The shape:
///
/// ```text
/// VarInt   the entity id
/// VarInt   properties: bit 0 is on the ground, the rest is a step count
/// short    dx, dy, dz   when the step count is zero
/// byte     yaw
/// byte     pitch
/// ```
///
/// The step count is packed into the same VarInt as the ground flag, which is
/// why it is shifted left by one when written. A count of zero means the simple
/// form: three shorts and nothing else. A stepped movement instead carries,
/// per step, a tick count and three shorts, and this server never sends one:
/// stepping is for smoothing a client's interpolation across a teleport or a
/// long jump, and a player walking between two positions does not need it.
pub fn write_move_entity_pos_rot(
    out: &mut Vec<u8>,
    entity_id: i32,
    delta: (f64, f64, f64),
    yaw: f32,
    pitch: f32,
    on_ground: bool,
) -> bool {
    if [delta.0, delta.1, delta.2]
        .iter()
        .any(|a| a.abs() > MAX_DELTA_BLOCKS)
    {
        return false;
    }

    VarInt(id::MOVE_ENTITY_POS_ROT).write(out);
    VarInt(entity_id).write(out);

    // No steps, and the ground flag in the low bit.
    VarInt(if on_ground { 1 } else { 0 }).write(out);

    for axis in [delta.0, delta.1, delta.2] {
        let scaled = (axis * DELTA_SCALE).round() as i16;
        out.extend_from_slice(&scaled.to_be_bytes());
    }

    out.push(angle_byte(yaw));
    out.push(angle_byte(pitch));
    true
}

/// One angle in the byte the protocol uses for it: 1/256 of a turn.
///
/// Two and a quarter degrees a step, so a face is not very precise. It is what
/// the protocol carries, and rounding to it is better than truncating, which
/// would put every facing a step below where it should be.
fn angle_byte(degrees: f32) -> u8 {
    let steps = degrees / 360.0 * 256.0;
    // Round half away from zero, and fold the result back into a byte so a
    // negative yaw wraps rather than saturating.
    let rounded = if steps < 0.0 {
        -(-steps + 0.5).floor()
    } else {
        (steps + 0.5).floor()
    };
    (rounded as i64).rem_euclid(256) as u8
}

/// A level event is a thing that happened at a place: a sound, or a burst of
/// particles.
///
/// The two event numbers this uses:
///
/// - `2001` is a block being destroyed. Its data is the **state id of the block
///   that was there**, because the client looks the block up from that number to
///   decide what the particles should look like. Sending air, or the id of the
///   block that replaced it, draws nothing.
/// - `2014` is the same without the sound.
pub const BLOCK_DESTROYED: i32 = 2001;
pub const BLOCK_DESTROYED_QUIET: i32 = 2014;

/// Writes a `level_event`.
///
/// The type and the data are plain four byte ints, not VarInts. Nearly every
/// other field in this file is a VarInt, which is exactly why these two are
/// worth saying out loud: a VarInt here reads the first three bytes as a length
/// and the client reports the rest as trailing garbage.
pub fn write_level_event(
    out: &mut Vec<u8>,
    event: i32,
    pos: (i32, i32, i32),
    data: i32,
    global: bool,
) {
    VarInt(id::LEVEL_EVENT).write(out);
    // The event and the data are plain four-byte ints here, not VarInts.
    //
    // Every other number in the play packets is a VarInt, which is what makes
    // this one worth a note. It is read with `readInt` and written with
    // `writeInt`, and a test that once "proved" the opposite was wrong.
    out.extend_from_slice(&event.to_be_bytes());
    out.extend_from_slice(&pack_block_pos(pos.0, pos.1, pos.2).to_be_bytes());
    out.extend_from_slice(&data.to_be_bytes());
    out.push(global as u8);
}

/// The stage that says "no crack here any more".
pub const NO_CRACK: i8 = -1;

/// Writes `ClientboundKeepAlivePacket`.
///
/// The id comes back verbatim, so it has to be distinguishable from one still
/// in flight when the answer arrives. The caller supplies it for that reason.
pub fn write_keep_alive(out: &mut Vec<u8>, id: i64) {
    VarInt(id::KEEP_ALIVE).write(out);
    out.extend_from_slice(&id.to_be_bytes());
}

/// `world_clock` registry id of `minecraft:overworld`.
///
/// Ordinals come from registration order, and `WorldClocks.bootstrap` registers
/// the overworld before the end.
const WORLD_CLOCK_OVERWORLD: i32 = 0;

/// Writes `ClientboundSetTimePacket`: the game clock, and where the sun is.
///
/// Two parts, and they are not the same number. `game_time` is how long the
/// world has been running and only ever goes up. The map is the world's own
/// clocks, keyed by registry id, and the overworld's total is the sun's
/// position -- the part that wraps at twenty-four thousand.
///
/// Sent once when a player arrives and then only when the time is changed,
/// because the client runs this clock itself from whatever it is told
/// (`ClientLevel.tickTime`). Sending it every tick would be correcting a clock
/// that is already right, twenty times a second, forever.
pub fn write_set_time(out: &mut Vec<u8>, game_time: i64, day_time: i64) {
    VarInt(id::SET_TIME).write(out);
    out.extend_from_slice(&game_time.to_be_bytes());

    // A map is a count and then that many entries.
    VarInt(1).write(out);
    VarInt(WORLD_CLOCK_OVERWORLD).write(out);
    VarLong::zigzag(day_time).write(out);
    // Partial tick is the fraction into the current tick. Zero here because the
    // server has no sub-tick to report, and inventing one would make the sun
    // jitter by up to a tick against everything else that happens on a tick.
    out.extend_from_slice(&0.0f32.to_be_bytes());
    // Rate one: real time is real time. Zero is what a paused world sends, and
    // this one is not paused.
    out.extend_from_slice(&1.0f32.to_be_bytes());
}

/// Writes `ClientboundSetTimePacket` with no clock updates, only the age.
///
/// The regular beat the server's game sends every twenty ticks, and the shape
/// matters more than it looks. An update carrying the clock overwrites what
/// the client has been running its own count against; the game's own periodic
/// sync carries an empty map, `Map.of()`, so the client keeps its smoothly
/// advanced clock and only the age it extrapolates by is corrected. The one
/// packet that names clocks is the full sync at join.
pub fn write_set_time_bare(out: &mut Vec<u8>, game_time: i64) {
    VarInt(id::SET_TIME).write(out);
    out.extend_from_slice(&game_time.to_be_bytes());

    // Zero entries: no clock update is carried, which is the point.
    VarInt(0).write(out);
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
mod tests;
