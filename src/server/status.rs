//! The TCP listener and the handshake and status phases.
//!
//! One thread per connection. That is the simple choice and it is the right one
//! here: these phases do almost no work, so a thread spends nearly all its life
//! blocked on a read. The parallelism that matters for this server is chunk
//! generation and lighting, which is CPU-bound and gets its own pool.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use crate::protocol::frame::{FrameDecoder, FrameEncoder, FrameError};
use crate::protocol::handshake::{ClientIntent, ClientIntention, ServerStatus, status_response_body};
use crate::protocol::{VarInt, VarLong};

use super::log::{next_connection_id, packet_name, ConnLog};
use super::log;
use super::broadcast::{Broadcast, Session};
use super::ticking::WorldHandle;
use super::ticking;

/// The protocol version this server speaks, from the data generator.
pub const PROTOCOL_VERSION: i32 = 777;

/// A running server.
pub struct StatusServer {
    listener: TcpListener,
    status: ServerStatus,
    /// A way to ask the world for things, not the world itself. It lives on a
    /// thread of its own; see `server::ticking`.
    world: WorldHandle,
    /// Who is connected, and the means to tell them something.
    wire: Arc<Broadcast>,
}

impl StatusServer {
    /// Binds `addr` and prepares to answer handshakes.
    pub fn bind(addr: &str, status: ServerStatus) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        Ok(StatusServer {
            listener,
            status,
            world: ticking::spawn(),
            wire: Arc::new(Broadcast::new()),
        })
    }

    /// The address actually bound, which is useful when port 0 was asked for.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts connections until the process is asked to stop, each on its own
    /// thread.
    ///
    /// Non-blocking rather than a plain `incoming()` loop, so a Ctrl+C is seen
    /// within a poll instead of at the end of the process. The cost is a wakeup
    /// every `POLL` to find out nothing happened, which is a twentieth of a
    /// second of work doing nothing.
    pub fn serve_forever(self) {
        self.listener
            .set_nonblocking(true)
            .expect("a listener can be set non-blocking");

        let mut live: Vec<thread::JoinHandle<()>> = Vec::new();

        loop {
            match self.listener.accept() {
                Ok((stream, peer)) => {
                    // The listener is non-blocking so that a Ctrl+C is seen at
                    // this loop's next poll, and on Windows an accepted socket
                    // inherits that flag from the listener. Left alone, every
                    // read on the connection returns "would block" immediately
                    // and a client that says anything at all is dropped with
                    // error 10035.
                    //
                    // Unix does not inherit it, which is why this is easy to miss
                    // until a real client connects.
                    if let Err(e) = stream.set_nonblocking(false) {
                        log::error(format!("accept failed: {e}"));
                        continue;
                    }

                    let id = next_connection_id();
                    let status = self.status.clone();
                    let session = Session {
                        world: Arc::new(self.world.clone()),
                        wire: self.wire.clone(),
                    };

                    log::note(format!("[conn {id}] accepted from {peer}"));

                    // Detached on purpose: a server's whole job is to handle
                    // many people at once, so joining these would serialise the
                    // entire server on whichever connection finished first.
                    //
                    // The handle is kept rather than dropped anyway, so that
                    // shutdown can wait for the connections to finish writing
                    // instead of cutting them off mid-packet.
                    let handle = thread::spawn(move || {
                        if let Err(e) = handle_connection(stream, status, session) {
                            log::error(format!("[conn {id}] ended with an error: {e}"));
                        }
                    });
                    live.push(handle);
                    live.retain(|h| !h.is_finished());
                }
                // Nothing waiting. The normal case, and the reason this loop is
                // a poll rather than a block.
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if super::shutdown::requested() {
                        break;
                    }
                    thread::sleep(POLL);
                }
                Err(e) => {
                    log::error(format!("accept failed: {e}"));
                    thread::sleep(POLL);
                }
            }
        }

        // The connections are not waited for. A connection thread runs until its
        // client goes away, and there is no way yet to tell one to finish, so
        // joining here would hang for as long as anybody stayed connected, which
        // is a worse Ctrl+C than the one being fixed. Saying so is better than
        // appearing to shut down cleanly and then not.
        if live.is_empty() {
            log::note("stopped");
        } else {
            log::note(format!(
                "stopped, cutting off {} connection(s) still open",
                live.len()
            ));
        }
    }
}

/// How long the accept loop waits between looks for a connection.
///
/// Short enough that a Ctrl+C feels immediate and long enough that an idle
/// server is not spinning.
const POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Answers one connection's handshake and status exchange.
///
/// Deliberately does not implement login: the point here is to be visible in
/// the multiplayer browser, which the client reaches only after status.
fn handle_connection(
    mut stream: TcpStream,
    status: ServerStatus,
    session: Session,
) -> std::io::Result<()> {
    let log = ConnLog::new(
        next_connection_id(),
        stream.peer_addr().map(|a| a.to_string()).unwrap_or_default(),
        "handshake",
    );
    let mut dec = FrameDecoder::new();
    let enc = FrameEncoder::new();
    let mut scratch = [0u8; 8192];

    // The handshake is the first thing on the connection, and nothing is
    // compressed before login, so the decoder needs no threshold here.
    //
    // The decoder is handed to whichever phase comes next rather than replaced.
    // A client sends its handshake and its login hello back to back, so both are
    // usually in one segment, and the read above tends to bring both in at once.
    // A fresh decoder in the next phase drops the second one on the floor, and
    // the server then waits for a hello that has already arrived and been thrown
    // away: the client times out and hangs up, and the log says it closed before
    // the hello arrived, which is exactly what it says.
    let intention = loop {
        let n = stream.read(&mut scratch)?;
        if n == 0 {
            // The client hung up before saying anything.
            return Ok(());
        }
        dec.queue(&scratch[..n]);

        match dec.next_frame() {
Ok(Some(frame)) => {
            log.in_packet(
                &packet_name("handshake", packet_id_of(&frame)),
                "handshake",
                frame.len(),
            );
            break match ClientIntention::read(&frame) {
                Ok(p) => p,
                Err(e) => {
                    log.error(format!("bad handshake: {e}"));
                    return Ok(());
                }
            }
        }
        Ok(None) => continue,
        Err(e) => {
            log.error(format!("framing error during handshake: {e}"));
            return Ok(());
        }
    }
    };

    log.note(format!(
        "protocol {}, host {:?}, port {}, intent {:?}",
        intention.protocol_version.0,
        intention.host,
        intention.port,
        intention.intent
    ));

    match intention.intent {
        ClientIntent::Status => serve_status(&mut stream, dec, enc, status, log),
        ClientIntent::Login => login(&mut stream, log, dec, session),
        ClientIntent::Transfer => {
            log.note("transfer is not implemented");
            Ok(())
        }
    }
}

/// The login phase, in the order `finishLoginAndWaitForClient` uses upstream.
///
/// 1. `login_compression`, which announces the threshold
/// 2. compression turns on, from this point on
/// 3. `login_finished`, carrying the profile
/// 4. wait for `login_acknowledged`
///
/// The order of 1 and 2 is the part that is easy to get wrong: the client
/// applies the threshold to whatever arrives *after* the compression packet, so
/// sending that packet already compressed means the client reads its own
/// threshold as garbage and disconnects.
fn login(
    stream: &mut TcpStream,
    log: ConnLog,
    mut dec: FrameDecoder,
    session: Session,
) -> std::io::Result<()> {
    use crate::protocol::frame::DEFAULT_COMPRESSION_THRESHOLD;
    use crate::protocol::login::{write_login_compression, write_login_finished, Uuid};

    let log = log.phase("login");
    let mut enc = FrameEncoder::new();
    let mut scratch = [0u8; 8192];

    // The client names itself. The loop is the value: `break name` leaves it with
    // the name, and that is what the block evaluates to.
    let name: String = {
        loop {
            // Drain what is already buffered before reading. The hello is often
            // in the same segment as the handshake, so it is usually already
            // here on the first pass, and reading the socket first would wait
            // on a client that is waiting for us.
            if let Some(frame) = dec.next_frame().map_err(io_err)? {
                let id = packet_id_of(&frame);
                log.in_packet(&packet_name("login-s2c", id), "login", frame.len());

                match crate::protocol::login::read_hello(&frame) {
                    Ok((name, _)) => break name,
                    Err(e) => {
                        log.error(format!("bad login hello: {e}"));
                        return Ok(());
                    }
                }
            }

            let n = stream.read(&mut scratch)?;
            if n == 0 {
                log.note("closed before the login hello arrived");
                return Ok(());
            }
            dec.queue(&scratch[..n]);
        }
    };

    log.note(format!(
        "player {name:?} from {}, sending the profile",
        stream.peer_addr().map(|a| a.to_string()).unwrap_or_default()
    ));

    let profile_id = Uuid::offline_for(&name);
    let session_id = Uuid::offline_for("filament-session");

    // From here the player is somebody to tell things to. Registered before the
    // profile goes out so that a push arriving in the same moment as the
    // profile is queued rather than lost, and taken back down when the
    // connection ends however it ends.
    let registration = session.wire.register(&name);
    let player_id = registration.id;
    log.note(format!(
        "player {name:?} is player {player_id}, {} in total",
        session.wire.player_count()
    ));

    // 1. Announce compression, still uncompressed.
    let mut out = Vec::new();
    let mut compression_packet = Vec::new();
    write_login_compression(&mut compression_packet, DEFAULT_COMPRESSION_THRESHOLD);
    enc.encode(&compression_packet, &mut out).map_err(io_err)?;
    stream.write_all(&out)?;
    log.out_packet("login_compression", "login", compression_packet.len());

    // 2. From here on, both directions are compressed.
    enc.set_compression(DEFAULT_COMPRESSION_THRESHOLD);
    dec.set_compression(DEFAULT_COMPRESSION_THRESHOLD);
    log.note(format!("compression on at {DEFAULT_COMPRESSION_THRESHOLD}"));

    // 3. The profile.
    let mut body = Vec::new();
    write_login_finished(&mut body, profile_id, &name, session_id);
    let mut out = Vec::new();
    enc.encode(&body, &mut out).map_err(io_err)?;
    stream.write_all(&out)?;
    stream.flush()?;
    log.out_packet("login_finished", "login", body.len());
    log.note(format!("profile {name} as {profile_id}"));

    // 4. The acknowledgement is the last packet of the *login* phase. It has to be
    // read and logged as such: the client sends it and only then switches to
    // the configuration protocol, so labelling it configuration makes the log
    // claim the client sent something it never sent, and every real
    // configuration packet gets logged as a login one.
    //
    // Drain first: the client's acknowledgement usually arrives in the same
    // segment as its hello, so it is often already buffered.
    loop {
        if let Some(frame) = dec.next_frame().map_err(io_err)? {
            let id = packet_id_of(&frame);
            log.in_packet(
                &packet_name("login-s2c", id),
                "login",
                frame.len(),
            );

            if id == crate::protocol::login::id::SERVERBOUND_LOGIN_ACKNOWLEDGED {
                log.note("acknowledged; moving to the configuration phase");
                // The decoder is handed over rather than replaced. It already
                // has compression enabled, which is what the next phase needs,
                // and more importantly it still holds any bytes that arrived
                // behind the acknowledgement. Starting a fresh decoder dropped
                // them, and the first configuration packets went missing.
                return configure(stream, log, dec, session, registration);
            }
        }

        let n = stream.read(&mut scratch)?;
        if n == 0 {
            log.note("closed before acknowledging");
            return Ok(());
        }
        dec.queue(&scratch[..n]);
    }
}

/// The configuration phase, in the order `startConfiguration` uses upstream.
///
/// 1. enabled feature flags
/// 2. registry data, one packet per registry
/// 3. tags
/// 4. finish configuration
/// 5. wait for the client's own finish
///
/// Steps 2 and 3 are both required even though neither looks like it. The
/// client falls back to the registries built into its jar when no registry_data
/// arrives, but that set is not the one the tags name, and tags carry registry
/// indices, so one packet without the other fails either way.
///
/// `dec` comes from the login phase and arrives with compression already on and
/// whatever bytes followed the acknowledgement still buffered.
fn configure(
    stream: &mut TcpStream,
    log: ConnLog,
    mut dec: FrameDecoder,
    session: Session,
    registration: super::broadcast::Registration,
) -> std::io::Result<()> {
    use crate::protocol::configuration::{
        registry_data_count, registry_data_frames, update_tags_summary,
        write_finish_configuration, write_update_enabled_features, write_update_tags,
    };
    use crate::protocol::frame::DEFAULT_COMPRESSION_THRESHOLD;

    let log = log.phase("configuration");

    // Compression is already on in the decoder that arrived from login and
    // stays on for the rest of the connection, so only the encoder is set up
    // here.
    let enc = FrameEncoder::compressed(DEFAULT_COMPRESSION_THRESHOLD);
    let mut scratch = [0u8; 8192];

    let mut out = Vec::new();

    // 1. No feature flags: a client does not require any particular one, and
    //    sending one it does not know about is worse than sending none.
    let mut body = Vec::new();
    write_update_enabled_features(&mut body, &[]);
    enc.encode(&body, &mut out).map_err(io_err)?;
    log.out_packet("update_enabled_features", "configuration", body.len());

    // 2. Registry data. These look optional and are not: with none of them the
    //    client falls back to the registries built into its jar, and that set
    //    is not the one the tags refer to. It resolves a tag against the
    //    fallback registries and throws on the first datapack registry it meets
    //    with "Missing registry: ResourceKey[minecraft:root / minecraft:dialog]".
    //    Sending them also fixes the other direction: tag contents are registry
    //    indices, so without the registries there is no order to read them
    //    against.
    let mut registries = 0;
    let mut registry_bytes = 0;
    let mut frames = registry_data_frames();
    for frame in &mut frames {
        // The walk reports a malformed capture as a decode error and
        // `io_err` is the same adapter the encoder uses, so wrap rather than
        // keeping a second conversion here.
        let frame = frame.map_err(|e| io_err(FrameError::Decode(e)))?;
        enc.encode(frame, &mut out).map_err(io_err)?;
        registries += 1;
        registry_bytes += frame.len();
    }
    if let Some(e) = frames.failure() {
        log.error(format!("the embedded capture stopped early: {e}"));
    }
    if registries != registry_data_count() {
        log.error(format!(
            "the capture holds {registries} frames but {} were expected",
            registry_data_count()
        ));
    }
    log.note(format!("registries: {registries} frames, {registry_bytes} bytes"));
    log.out_packet("registry_data", "configuration", registry_bytes);

    // 3. Tags, which are not optional either. The client builds its tag list
    //    from this and from nothing else, and the first tag it looks for is
    //    damage_type/is_fire. Without it the client throws
    //    "Missing tag TagKey[minecraft:damage_type / minecraft:is_fire]" while
    //    assembling its components and hangs up before the world.
    let mut body = Vec::new();
    write_update_tags(&mut body);
    log.note(format!("tags: {}", update_tags_summary()));
    enc.encode(&body, &mut out).map_err(io_err)?;
    log.out_packet("update_tags", "configuration", body.len());

    // 4. Finish.
    let mut body = Vec::new();
    write_finish_configuration(&mut body);
    enc.encode(&body, &mut out).map_err(io_err)?;
    log.out_packet("finish_configuration", "configuration", body.len());

    stream.write_all(&out)?;
    stream.flush()?;
    log.note("configuration sent, waiting for the client to finish");

    // 4. Wait. The client sends its own finish, and after that the connection
    //    is in the play state.
    //
    //    Draining before reading matters here too: the client sends its
    //    information and its brand together with its finish, and reading the
    //    socket first would wait on a client that is waiting for us to finish.
    //
    //    How much world to build is the client's answer, so the radius is not
    //    decided until its settings arrive here. The default is for a client
    //    that sends them after the finish, which would otherwise have no
    //    radius at all.
    let mut view_distance = crate::protocol::chunk::RADIUS;
    loop {
        while let Some(frame) = dec.next_frame().map_err(io_err)? {
            let id = packet_id_of(&frame);
            // The names are looked up in the serverbound table: these are
            // packets the client sent, and the ids overlap with the
            // clientbound ones in the same phase.
            log.in_packet(
                &packet_name("configuration-s2c", id),
                "configuration",
                frame.len(),
            );

            match id {
                crate::protocol::configuration::id::SERVERBOUND_FINISH_CONFIGURATION => {
                    log.note("client finished configuration");
                    return play(stream, dec, enc, log, view_distance, session, registration);
                }
                crate::protocol::configuration::id::SERVERBOUND_CLIENT_INFORMATION => {
                    match crate::protocol::configuration::ClientInformation::read(&frame) {
                        Ok(info) => {
                            // The client asked for this much world, so that is
                            // how much to generate. Sending less leaves the far
                            // ring of the view as the void it started as.
                            view_distance = i32::from(info.view_distance);
                            log.note(format!(
                                "client settings: {} view distance {}, main hand {}",
                                info.language, info.view_distance, info.main_hand
                            ));
                        }
                        Err(e) => log.error(format!("bad client information: {e}")),
                    }
                }
                crate::protocol::configuration::id::SERVERBOUND_CUSTOM_PAYLOAD => {
                    // The brand packet the client sends with its settings.
                    log.note("client payload (brand), ignored");
                }
                _ => {}
            }
        }

        let n = stream.read(&mut scratch)?;
        if n == 0 {
            log.note("client closed during configuration");
            return Ok(());
        }
        dec.queue(&scratch[..n]);
    }
}

/// The play phase, and the first one that shows anything.
///
/// The order is the order a real server sends, and it matters more here than in
/// the earlier phases: the client processes what arrives and drops what it
/// cannot use, so a packet in the wrong place is not an error, it is silence.
/// The sequence the capture recorded is login, difficulty, abilities, held slot,
/// recipes, position, spawn, border, time, and finally the game event that says
/// to start waiting for chunks.
///
/// There are no chunks yet, so the client will sit on the loading screen. That
/// is the honest state of the server: it can be joined, and the world is empty.
///
/// Note the fresh output buffer. The configuration phase's buffer already went
/// out on the wire; handing it here and writing it again made the client read
/// 158 KB of configuration a second time while it was in the play phase, where
/// registry id 7 means something else entirely.
fn play(
    stream: &mut TcpStream,
    mut dec: FrameDecoder,
    mut enc: FrameEncoder,
    log: ConnLog,
    radius: i32,
    session: Session,
    registration: super::broadcast::Registration,
) -> std::io::Result<()> {
    use crate::protocol::play as p;

    let player_id = registration.id;
    let log = log.phase("play");

    // Compression carries over from the configuration phase; both sides keep
    // it for the rest of the connection.
    let mut scratch = [0u8; 8192];
    let mut out = Vec::new();

    let send = |out: &mut Vec<u8>, body: &[u8], name: &str, log: &ConnLog| -> std::io::Result<()> {
        enc.encode(body, out).map_err(io_err)?;
        log.out_packet(name, "play", body.len());
        Ok(())
    };

    // 1. Login. The entity id comes from the shared counter rather than being
    //    written as 0, which is not a valid id: the client reads it as "not yet
    //    assigned" and throws when it adds the player to the level.
    let entity_id = crate::server::next_entity_id();
    let mut body = Vec::new();
    p::write_login(
        &mut body,
        &p::Login {
            entity_id,
            hardcore: false,
            levels: &[
                "minecraft:overworld",
                "minecraft:the_nether",
                "minecraft:the_end",
            ],
            max_players: 20,
            view_distance: 10,
            simulation_distance: 10,
            dimension: "minecraft:overworld",
            seed: 0,
            game_type: 0,
            is_flat: true,
            portal_cooldown: 0,
            sea_level: -64,
        },
    );
    send(&mut out, &body, "login", &log)?;

    // 2. Difficulty: easy, which is index 1. Peaceful would be 0 and would stop
    //    mobs spawning, which is not a decision this server should make.
    let mut body = Vec::new();
    p::write_difficulty(&mut body, 1);
    send(&mut out, &body, "change_difficulty", &log)?;

    // 3. Abilities: nothing allowed.
    let mut body = Vec::new();
    p::write_player_abilities(&mut body);
    send(&mut out, &body, "player_abilities", &log)?;

    // 4. The slot in hand.
    let mut body = Vec::new();
    p::write_set_held_slot(&mut body, 0);
    send(&mut out, &body, "set_held_slot", &log)?;

    // 5. The spawn. Halfway between block corners, so the player is inside a
    //    block rather than on a seam, and above the sea level the login says.
    let spawn = p::Movement::default();
    let mut body = Vec::new();
    p::write_player_position(&mut body, 1, &spawn);
    send(&mut out, &body, "player_position", &log)?;

    // 6. Where a respawn returns to. A block position, so whole blocks, unlike
    //    the spawn above which is inside one.
    let mut body = Vec::new();
    p::write_set_default_spawn(&mut body, "minecraft:overworld", 0, 64, 0);
    send(&mut out, &body, "set_default_spawn_position", &log)?;

    // 7. No world border.
    let mut body = Vec::new();
    p::write_initialize_border(&mut body);
    send(&mut out, &body, "initialize_border", &log)?;

    // 8 is the world clock, which is not written yet. See play::set_time.

    // 9. Where chunks are centred. Until this arrives the client has nowhere to
    //    ask for them.
    let mut body = Vec::new();
    p::write_chunk_cache_center(&mut body, 0, 0);
    send(&mut out, &body, "set_chunk_cache_center", &log)?;

    // 10. Start waiting for level chunks. Reason 13 is the one the client treats
    //     as "hold the loading screen", and it is what makes it wait instead of
    //     reporting that the world failed to load.
    let mut body = Vec::new();
    p::write_game_event(&mut body, 13, 0.0);
    send(&mut out, &body, "game_event", &log)?;

    // 11. The chunks. This is the first thing the client renders, so it goes after
    //     the world setup above and before anything else about the world.
    //
    //     Generated on demand into a world that lives as long as the
    //     connection. It used to be generated per chunk and thrown away, which
    //     made the storage layer unreachable: a block the player broke had
    //     nowhere to live, and walking back regenerated the ground over it.
    //
    //     The terrain generator's own block ids are mapped to the game's state
    //     ids on the way out: the two registries are unrelated, and the world
    //     module has no business knowing a protocol id.
    use crate::world::pos::ChunkPos;

    // The chunks the client is holding. Filled by the spawn batch below, and
    // compared against the set it should be holding on every pass after, so
    // both directions are known: what arrived and what left.
    let mut sent: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();

    // A batch, so the client knows more chunks are coming. The float in the
    // closing packet is the tick rate the server intends to run at.
    let mut body = Vec::new();
    p::write_chunk_batch_start(&mut body);
    enc.encode(&body, &mut out).map_err(io_err)?;

    let mut sent_count = 0;
    for ring in 0..=radius {
        let mut chunks: Vec<(i32, i32)> = Vec::new();
        for dx in -ring..=ring {
            for dz in -ring..=ring {
                // Only the edge of each ring; the inner square was already sent.
                if ring > 0 && dx.abs() != ring && dz.abs() != ring {
                    continue;
                }
                chunks.push((dx, dz));
            }
        }

        for (dx, dz) in chunks {
            // Asked of the thread that owns the world, and waited for. The
            // generation happens there rather than here, which is the whole
            // reason the two hundred and eighty-nine of an initial join no
            // longer block anyone else.
            let Some(body) = session.world.chunk(ChunkPos { x: dx, z: dz }) else {
                return Ok(());
            };

            enc.encode(&body, &mut out).map_err(io_err)?;
            sent_count += 1;
            sent.insert((dx, dz));
            if super::log::verbose() {
                log.out_packet("level_chunk_with_light", "play", body.len());
            }
        }
    }
    let mut body = Vec::new();
    // The count is how many chunks the batch held, which is the number just sent.
    p::write_chunk_batch_finished(&mut body, sent_count);
    enc.encode(&body, &mut out).map_err(io_err)?;

    log.note(format!("sent {sent_count} chunks around the spawn"));

    stream.write_all(&out)?;
    stream.flush()?;

    log.note("chunks sent; terrain generated, and it is not persisted");

    // 12. Keep the connection alive, and follow the player.
    //
    //     The client hangs up after thirty seconds of silence and reads silence
    //     as "the server has stopped answering". There is no tick loop yet, so
    //     the only traffic there is to send is this.
    //
    //     A read timeout rather than a second thread, so the deadline is checked
    //     in the same loop that drains packets and there is no writer to lock
    //     against.
    //
    //     The movement packets update where the player is, and when that moves
    //     the chunk it stands in outside the square already sent, the new edge
    //     follows. That is the difference between a world and a diorama: without
    //     it the player walks off the terrain into white nothing.
    //     Fifty milliseconds, which is one game tick. It used to be a second,
    //     because the only thing this loop had to do was answer a keep alive on
    //     a fifteen second deadline and there was no reason to wake sooner.
    //     Mining progress is that reason: a crack has ten stages and moves
    //     through them in about a second, which a loop that wakes once a second
    //     cannot show.
    stream.set_read_timeout(Some(LOOP_INTERVAL))?;
    let mut last_keep_alive = std::time::Instant::now();
    let mut keep_alive_id: i64 = 1;

    // The block this player is currently breaking, if any.
    //
    // Cracks are drawn by the client but paced by the server, so the server has to
    // keep track of how far along it is. It is per connection because mining is per
    // player: two people can be breaking different blocks at once and neither should
    // be told about the other's.
    let mut mining: Option<Mining> = None;
    // The stage the other players were last told about, and `None` for "no crack
    // has been sent at all".
    //
    // An `Option` rather than a number outside the range, because zero is a real
    // stage: a crack that has just started *is* stage zero, and a sentinel that
    // reads as zero makes the first stage compare equal to it and never go out.
    // The block then stays uncracked until stage one, which is most of a second
    // of it looking like nothing happened.
    let mut last_stage: Option<i8> = None;

    // Where the player is, updated by the movement packets. The spawn is where
    // the teleport above put it.
    let mut player = spawn;

    // The cache center the client was last told about. The client keeps chunks
    // only within the view distance of this point, so it has to move with the
    // player: left at the spawn forever, the world renders as a fixed window
    // that the player walks out of, with chunks arriving that never draw.
    let mut center_sent = (0i32, 0i32);

    // Blocks changed by the player, each already a whole packet.
    //
    // One packet each, not one packet per pass. A frame carries a length and
    // then exactly one packet, so two edits written into one buffer and framed
    // once arrive as a single frame holding two packets: the client reads the
    // first, finds ten bytes where there should be none, and hangs up. Two edits
    // in one pass is not exotic, it is what a player holding the mouse down
    // does.
    let mut edits: Vec<Vec<u8>> = Vec::new();

    // The dust and noise a break makes, as whole packets, kept apart from the
    // edits because they are separate packets rather than part of the same one.
    let mut effects: Vec<Vec<u8>> = Vec::new();

    loop {
        while let Some(frame) = dec.next_frame().map_err(io_err)? {
            let id = packet_id_of(&frame);
            if super::log::verbose() {
                log.in_packet(&packet_name("play-s2c", id), "play", frame.len());
            }

            // The four movement packets. Position when the packet has one,
            // rotation ignored for now: nothing on the server looks at it.
            if matches!(
                id,
                p::id::MOVE_PLAYER_POS
                    | p::id::MOVE_PLAYER_POS_ROT
                    | p::id::MOVE_PLAYER_ROT
                    | p::id::MOVE_PLAYER_STATUS_ONLY
            ) && let Ok(moved) = p::ClientMovement::read(&frame)
                && let (Some(x), Some(y), Some(z)) = (moved.x, moved.y, moved.z)
            {
                player.x = x;
                player.y = y;
                player.z = z;
            }

            // Breaking a block.
            //
            // Three actions, and they do three different things. START is the
            // button going down: the crack starts, and the others are told it
            // has. STOP is the block going: the crack comes off, then the block
            // does. ABORT is the button coming up early, which is STOP without
            // the block, so the crack comes off and nothing else happens.
            //
            // All three carry the reach check, not just the one that edits the
            // world. A crack drawn on a block a kilometre away is a way to find
            // out what is there, which is the same hole as editing it and wants
            // the same answer.
            if id == p::id::SERVERBOUND_PLAYER_ACTION
                && let Ok(action) = p::PlayerActionPacket::read(&frame)
            {
                let (bx, by, bz) = p::unpack_block_pos(action.pos);
                let pos = crate::world::pos::BlockPos::new(bx, by, bz);
                let reach = reach_to(&player, pos);
                let too_far = reach > MAX_REACH;

                if too_far {
                    if super::log::verbose() {
                        log.note(format!(
                            "refused a break at {bx} {by} {bz}, {reach:.1} blocks away"
                        ));
                    }
                } else {
                    match action.action {
                        p::PlayerAction::StartDestroy => {
                            // Read the block now so the crack is paced against
                            // this block rather than a guess. A world edited
                            // underneath the crack cannot change its pace
                            // halfway through, because the duration is carried
                            // from here.
                            let duration = {
                                let Some(terrain) = session.world.terrain_blocks() else {
                                    return Ok(());
                                };
                                let mined = session
                                    .world
                                    .world_block(pos)
                                    .flatten()
                                    .unwrap_or(terrain.air);
                                break_time(mined, &terrain)
                            };

                            mining = Some(Mining {
                                pos,
                                started: std::time::Instant::now(),
                                duration,
                            });
                            last_stage = None;
                        }

                        p::PlayerAction::AbortDestroy => {
                            clear_crack(&mut mining, last_stage, &session, player_id, entity_id);
                        }

                        p::PlayerAction::StopDestroy => {
                            clear_crack(&mut mining, last_stage, &session, player_id, entity_id);

                            // What is there now, before it is gone.
                            //
                            // Read first because after the break the answer is
                            // air, and air is what the particles would then be
                            // told to draw. The level event carries the state id
                            // of the block that was destroyed, so it has to be
                            // remembered before the world forgets it.
                            let was = session
                                .world
                                .block_at(pos)
                                .flatten()
                                .unwrap_or(AIR_STATE);

                            if session.world.break_block(pos).flatten().is_some() {
                                let now = session
                                    .world
                                    .block_at(pos)
                                    .flatten()
                                    .unwrap_or(AIR_STATE);

                                let mut body = Vec::new();
                                p::write_block_update(&mut body, bx, by, bz, now);
                                edits.push(body);

                                // The dust and the sound. Without this a block
                                // is simply deleted rather than broken, which
                                // looks like the server lost it.
                                let mut effect = Vec::new();
                                p::write_level_event(
                                    &mut effect,
                                    p::BLOCK_DESTROYED,
                                    (bx, by, bz),
                                    was,
                                    false,
                                );
                                effects.push(effect);

                                if super::log::verbose() {
                                    log.note(format!("broke the block at {bx} {by} {bz}"));
                                }
                            }
                        }

                        p::PlayerAction::Other => {}
                    }
                }
            }
        }

        // Advance the crack, if there is one.
        //
        // Sent only when the stage changes, so a block being mined for ten
        // seconds costs ten packets rather than two hundred.
        if let Some(current) = &mining {
            let stage = current.stage();
            if Some(stage) != last_stage {
                last_stage = Some(stage);

                let mut body = Vec::new();
                p::write_block_destruction(
                    &mut body,
                    entity_id,
                    (current.pos.x, current.pos.y, current.pos.z),
                    stage,
                );
                session.wire.send_others(player_id, body);
            }
        }

        // Tell the client about each edit, and everybody else too.
        //
        // Without the first half the change exists on the server and nowhere
        // else: the client keeps drawing the block it was sent and never learns
        // it is gone. Without the second half the other player keeps drawing it
        // as well, until they walk away and back and the chunk is sent again.
        //
        // Each edit is framed on its own, and each body goes out whole: framing
        // adds a length and nothing else, so a body sent without the packet id
        // arrives with the first byte of the position standing in for it, and
        // two bodies framed together arrive as one packet with ten bytes over.
        //
        // The framed bytes cannot be shared between connections, because
        // compression is per connection, so each one is framed again on the far
        // side with that player's encoder.
        if !edits.is_empty() {
            let mut framed = Vec::new();
            for body in edits.iter().chain(effects.iter()) {
                session.wire.send_others(player_id, body.clone());
                enc.encode(body, &mut framed).map_err(io_err)?;
            }
            stream.write_all(&framed)?;
            stream.flush()?;
        }

        // Anything the other players have told us, drained in the same loop that
        // drains the socket. No thread other than this one ever writes to this
        // stream, which is why it is a queue rather than a shared writer.
        let mut pushed = Vec::new();
        while let Ok(body) = registration.inbox.try_recv() {
            let mut framed = Vec::new();
            enc.encode(&body, &mut framed).map_err(io_err)?;
            pushed.extend_from_slice(&framed);
        }
        if !pushed.is_empty() {
            stream.write_all(&pushed)?;
            stream.flush()?;
        }

        // Which chunk the player stands in now.
        let player_chunk = (
            (player.x as i32).div_euclid(16),
            (player.z as i32).div_euclid(16),
        );

        // The cache center moves with the player, and it has to move even when
        // no chunk follows: the client drops chunks outside the view distance
        // of it, so leaving it behind means chunks arriving that the client
        // then discards because they fall outside the window it is keeping.
        let mut out = Vec::new();
        if center_sent != player_chunk {
            let mut body = Vec::new();
            p::write_chunk_cache_center(&mut body, player_chunk.0, player_chunk.1);
            enc.encode(&body, &mut out).map_err(io_err)?;
            center_sent = player_chunk;
        }

        // The set of chunks the client should be holding: a square of the view
        // radius around the player. The game sends a square too, so this is not
        // a smaller world than the client expects.
        //
        // What was missing was the other half. The set this is compared against
        // only ever grew, so every chunk the player ever walked past stayed
        // loaded on both sides and was paid for again on every subsequent
        // move.
        let mut wanted = std::collections::HashSet::with_capacity(((2 * radius + 1) * (2 * radius + 1)) as usize);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                wanted.insert((player_chunk.0 + dx, player_chunk.1 + dz));
            }
        }

        // New: wanted but not yet sent. Gone: sent but no longer wanted.
        //
        // Sorted, so the same walk produces the same bytes in the same order,
        // which is what makes a session reproducible.
        let arriving: Vec<(i32, i32)> = wanted.difference(&sent).copied().collect();
        let leaving: Vec<(i32, i32)> = sent.difference(&wanted).copied().collect();

        if !arriving.is_empty() || !leaving.is_empty() {
            let mut body = Vec::new();
            p::write_chunk_batch_start(&mut body);
            enc.encode(&body, &mut out).map_err(io_err)?;

            let mut sorted = arriving.clone();
            sorted.sort_unstable();

            for (dx, dz) in &sorted {
                if !send_one_chunk(&mut out, &mut enc, &session.world, *dx, *dz)? {
                    return Ok(());
                }
                sent.insert((*dx, *dz));
            }

            let mut body = Vec::new();
            p::write_chunk_batch_finished(&mut body, sorted.len() as i32);
            enc.encode(&body, &mut out).map_err(io_err)?;

            // The forgets go after the batch, and each is its own packet. They
            // are the client dropping chunks, so they are not part of a chunk
            // batch: the batch pair brackets chunks arriving, not leaving.
            let mut dropped = leaving.clone();
            dropped.sort_unstable();
            for (dx, dz) in &dropped {
                let mut body = Vec::new();
                p::write_forget_chunk(&mut body, *dx, *dz);
                let mut wire = Vec::new();
                enc.encode(&body, &mut wire).map_err(io_err)?;
                out.extend_from_slice(&wire);
                sent.remove(&(*dx, *dz));
                session.world.forget(crate::world::pos::ChunkPos { x: *dx, z: *dz });
            }

            stream.write_all(&out)?;
            stream.flush()?;

            if !sorted.is_empty() {
                log.note(format!(
                    "{} chunks arrived and {} were let go at {player_chunk:?}",
                    sorted.len(),
                    dropped.len()
                ));
            }
        } else if !out.is_empty() {
            // The center moved without any chunk following it, which is what
            // happens on a diagonal walk back through ground already sent.
            stream.write_all(&out)?;
            stream.flush()?;
        }

        if last_keep_alive.elapsed() >= KEEP_ALIVE_EVERY {
            let mut body = Vec::new();
            p::write_keep_alive(&mut body, keep_alive_id);
            let mut wire = Vec::new();
            enc.encode(&body, &mut wire).map_err(io_err)?;
            stream.write_all(&wire)?;
            stream.flush()?;

            keep_alive_id = keep_alive_id.wrapping_add(1).max(1);
            last_keep_alive = std::time::Instant::now();

            if super::log::verbose() {
                log.out_packet("keep_alive", "play", body.len());
            }
            continue;
        }

        match stream.read(&mut scratch) {
            Ok(0) => {
                log.note("client closed during play");
                return Ok(());
            }
            Ok(n) => dec.queue(&scratch[..n]),
            // The read timeout firing is the normal case. It is what lets the
            // keep alive deadline be looked at.
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e),
        }
    }
}

/// Writes one chunk into `out`, as a framed packet.
///
/// The chunk comes out of the shared world, so a block one player broke is still
/// missing when the chunk is sent to the next, and to the same one walking back.
fn send_one_chunk(
    out: &mut Vec<u8>,
    enc: &mut crate::protocol::frame::FrameEncoder,
    world: &WorldHandle,
    dx: i32,
    dz: i32,
) -> std::io::Result<bool> {
    use crate::world::pos::ChunkPos;

    let Some(body) = world.chunk(ChunkPos { x: dx, z: dz }) else {
        return Ok(false);
    };
    enc.encode(&body, out).map_err(io_err)?;

    Ok(true)
}

/// How often to send a keep alive.
///
/// Fifteen seconds against a thirty second client timeout, so one can be lost
/// without the connection going.
const KEEP_ALIVE_EVERY: std::time::Duration = std::time::Duration::from_secs(15);
/// How long one loop iteration waits before looking again.
///
/// One game tick. Long enough that an idle server is not spinning, short enough
/// that a crack moves visibly.
const LOOP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Seconds of bare-handed breaking per point of hardness.
///
/// The game's own numbers, read out of the decompiled block definitions rather
/// than remembered, and they line up exactly:
///
/// ```text
/// dirt        strength(0.5)  ->  0.5 * 1.5 = 0.75s
/// grass_block strength(0.6)  ->  0.6 * 1.5 = 0.90s
/// stone       strength(1.5)  ->  1.5 * 1.5 = 2.25s
/// ```
///
/// The first number of `strength` is the hardness and the second is the blast
/// resistance, which has nothing to do with how long a block takes to mine.
const SECONDS_PER_HARDNESS: f64 = 1.5;

/// How long a block takes to break in bare hands.
///
/// The hardness comes from the generated table, which reads it out of the
/// game's own block definitions. The factor is the game's: a bare hand takes
/// one and a half seconds per point of hardness, which the three blocks this
/// world generates check out against exactly.
///
/// What is still missing is the tool. The client's number is much shorter with
/// the right one in hand, and nothing here knows what is in the player's hand,
/// because the inventory is not implemented and so there is nothing to have a
/// tool in. With a tool the client finishes before the crack does, and the crack
/// is still cut short. That is the remaining half of this, and it waits on the
/// inventory.
fn break_time(block: crate::world::block::BlockId, terrain: &crate::world::terrain::TerrainBlocks) -> std::time::Duration {
    let hardness = crate::server::ticking::hardness_of(block, terrain)
        // Nothing in the table is ever a negative number the crack can use, and
        // a negative hardness means the block cannot be broken at all.
        .filter(|h| *h > 0.0)
        .unwrap_or(FALLBACK_HARDNESS);

    std::time::Duration::from_secs_f64(f64::from(hardness) * SECONDS_PER_HARDNESS)
}

/// The hardness used when the table has nothing.
///
/// Dirt's, which is the quickest thing a bare hand can break. Too short only
/// makes a crack finish early; too long makes the client cut it off, and the cut
/// is the one that looks broken.
const FALLBACK_HARDNESS: f32 = 0.5;

/// A block being broken, when it started, and how long it should take.
///
/// The duration is carried per break rather than looked up each pass: the block
/// is read once, at the start, so a world being edited underneath the crack
/// cannot change its pace halfway through.
struct Mining {
    pos: crate::world::pos::BlockPos,
    started: std::time::Instant,
    duration: std::time::Duration,
}

impl Mining {
    /// How far through the break this is, as one of the ten crack stages.
    fn stage(&self) -> i8 {
        stage_at(self.started.elapsed(), self.duration)
    }
}

/// The crack stage for an elapsed time, out of ten.
///
/// Integer arithmetic, because the floating point version was wrong at exactly
/// the points that matter. A tenth of a break is a ratio like 0.1, which is not
/// representable in binary, so `elapsed / duration * 10` came out as
/// 0.9999999999999999 and truncating it to an integer gave stage zero instead of
/// one. The stages that vanished were the ones landing on a boundary, so the
/// crack sat a stage behind at every tenth and then caught up.
///
/// Clamped at nine: the client has ten cracks and no picture for a higher one.
/// The block itself is removed by the client's stop packet, which is what
/// actually ends the break.
fn stage_at(elapsed: std::time::Duration, duration: std::time::Duration) -> i8 {
    let total = duration.as_nanos();
    if total == 0 {
        return 0;
    }
    let done = elapsed.as_nanos().saturating_mul(10) / total;
    done.min(9) as i8
}

/// Takes the crack off a block, if there was one on that block.
///
/// Sent only when there was a stage to clear, which is what makes NO_STAGE worth
/// having: a stop with nothing drawn costs nothing, and a stop that clears
/// somebody else's crack does not happen.
fn clear_crack(
    mining: &mut Option<Mining>,
    last_stage: Option<i8>,
    session: &Session,
    player_id: u64,
    entity_id: i32,
) {
    let Some(current) = mining.take() else {
        return;
    };
    // Nothing was drawn, so there is nothing to take off. Sending the clear
    // anyway would make every other player drop a crack that was not theirs,
    // which on a shared world is how one player's mining cancels another's.
    if last_stage.is_none() {
        return;
    }

    let mut body = Vec::new();
    crate::protocol::play::write_block_destruction(
        &mut body,
        entity_id,
        (current.pos.x, current.pos.y, current.pos.z),
        crate::protocol::play::NO_CRACK,
    );
    session.wire.send_others(player_id, body);
}

/// How far a player may reach to break a block, in blocks.
///
/// The game's own limit is 4.5 in survival and 5 in creative, and neither is
/// negotiated: this is a server saying what it will accept. It is generous past
/// both, because the number that matters is that it is a number.
const MAX_REACH: f64 = 6.0;

/// How far the player is from a block, in blocks.
///
/// The player's position is the middle of their feet, so the y comparison is
/// against the middle of the block rather than its base. Without that the
/// reach shrinks by half a block at every step and the check is tighter near the
/// ground than above it, which is backwards.
fn reach_to(player: &crate::protocol::play::Movement, block: crate::world::pos::BlockPos) -> f64 {
    let dx = player.x - (block.x as f64 + 0.5);
    let dy = player.y - (block.y as f64 + 0.5);
    let dz = player.z - (block.z as f64 + 0.5);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// The game state id for air, used when a block cannot be read.
///
/// A hole in the world, or a chunk the server has not built: either way the
/// honest answer for "what is there" is nothing.
const AIR_STATE: i32 = 0;

/// The leading VarInt of a frame body, which is its packet id.
fn packet_id_of(frame: &[u8]) -> i32 {
    let mut r = frame;
    VarInt::read(&mut r).map(|(v, _)| v.0).unwrap_or(-1)
}

/// Bridges a framing error into the `io::Error` the socket code returns.
///
/// `FrameError` is not an `io::Error` and these functions deal in sockets, so
/// the conversion happens here rather than by making every framing error an
/// `io::Error` and losing what kind of failure it was.
fn io_err(e: crate::protocol::FrameError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}

/// The status exchange: a request, a response, then an optional ping.
fn serve_status(
    stream: &mut TcpStream,
    mut dec: FrameDecoder,
    enc: FrameEncoder,
    status: ServerStatus,
    log: ConnLog,
) -> std::io::Result<()> {
    let log = log.phase("status");
    let mut scratch = [0u8; 8192];

    // Drain before reading. The client sends its handshake and its status
    // request together, so the request is usually already sitting in the
    // decoder when this starts. Reading the socket first waits for bytes that
    // are not coming, because the client is waiting for the response: the
    // symptom is the server entry stuck on "Pinging..." until the client gives
    // up and opens a second connection to log in directly.
    loop {
        while let Some(body) = dec.next_frame().map_err(io_err)? {
            let mut r = &body[..];
            let (VarInt(id), _) = match VarInt::read(&mut r) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // These are packets the client sent, so the serverbound table, where the
            // same ids mean the other half of the exchange.
            log.in_packet(&packet_name("status-s2c", id), "status", body.len());

            match id {
                // Status request: answer with the JSON.
                0 => {
                    let out = status_response_body(&status);
                    let mut wire = Vec::new();
                    enc.encode(&out, &mut wire).map_err(io_err)?;
                    stream.write_all(&wire)?;
                    stream.flush()?;
                    log.out_packet("status_response", "status", out.len());
                }
                // Ping: echo it back so the client can measure latency.
                1 => {
                    let payload = r.to_vec();
                    let value = VarLong::read(&mut &payload[..])
                        .map(|(v, _)| v)
                        .unwrap_or(VarLong(0));
                    let mut body = Vec::new();
                    VarInt(1).write(&mut body);
                    value.write(&mut body);

                    let mut wire = Vec::new();
                    enc.encode(&body, &mut wire).map_err(io_err)?;
                    stream.write_all(&wire)?;
                    stream.flush()?;
                    log.out_packet("pong_response", "status", body.len());
                }
                _ => {}
            }
        }

        let n = stream.read(&mut scratch)?;
        if n == 0 {
            return Ok(());
        }
        dec.queue(&scratch[..n]);
    }
}
#[cfg(test)]
mod tests {
    use crate::server::ticking::{state_id, to_state_id};
    use super::*;

    /// The palette the generator makes, so the hardness table can be keyed on it.
    fn terrain() -> crate::world::terrain::TerrainBlocks {
        crate::world::terrain::register_terrain(&mut crate::world::block::BlockRegistry::new())
    }

    /// How long dirt takes in bare hands: hardness 0.5 at the rate above.
    ///
    /// The quickest block the generator makes, standing in for an arbitrary
    /// break time in the crack tests.
    const DIRT_BREAK: std::time::Duration = std::time::Duration::from_millis(750);

    #[test]
    fn break_times_are_the_games_own_numbers() {
        // Read out of the decompiled block definitions by tools/extract_hardness.py,
        // at one and a half seconds per point of hardness for a bare hand. These
        // are the numbers a client uses to decide when it is finished, so a
        // server using different ones gets its crack cut short.
        let t = terrain();

        assert_eq!(break_time(t.dirt, &t), DIRT_BREAK);
        assert_eq!(
            break_time(t.stone, &t),
            DIRT_BREAK * 3,
            "stone is three times dirt"
        );

        // Halfway cases are where an f32 shows up, so the tenths are compared
        // as tenths rather than as exact seconds.
        let tenths = |block| (break_time(block, &t).as_secs_f64() * 10.0).round() as i64;
        assert_eq!(tenths(t.dirt), 8, "0.75s");
        assert_eq!(tenths(t.grass), 9, "0.9s");
        assert_eq!(tenths(t.stone), 23, "2.25s, three times dirt");
        assert_eq!(tenths(t.glowstone), 5, "0.45s");
    }

    #[test]
    fn a_block_that_cannot_be_broken_does_not_pace_a_crack() {
        // Hardness is negative for the fifteen blocks with no break time at all,
        // barrier among them. Multiplying a negative number by the rate gives a
        // negative duration, and dividing by it gives a stage that is nonsense.
        assert_eq!(
            crate::generated::hardness_of("minecraft:barrier"),
            Some(-1.0),
            "which is how the table says it"
        );

        let t = terrain();
        assert!(
            break_time(t.air, &t) > std::time::Duration::ZERO,
            "and air falls back rather than going negative"
        );
    }

    #[test]
    fn a_crack_starts_at_stage_zero_and_finishes_at_nine() {
        // The two ends, because the interesting part is that the first one is a
        // real stage rather than the absence of one.
        assert_eq!(stage_at(std::time::Duration::ZERO, DIRT_BREAK), 0);
        assert_eq!(stage_at(DIRT_BREAK * 2, DIRT_BREAK), 9, "clamped, not past the end");
    }

    #[test]
    fn a_crack_moves_through_its_stages_over_time() {
        // A tenth of dirt's break in whole milliseconds, rather than dividing a
        // duration and multiplying the result. `DIRT_BREAK / 10 * n` reads as
        // dividing by ten times n, because division and multiplication bind
        // equally and the left one wins, and that is a test which checks a
        // different thing from the one it claims to.
        let tenth = 75u64;

        let stages: Vec<i8> = (0..10u64)
            .map(|n| stage_at(std::time::Duration::from_millis(tenth * n), DIRT_BREAK))
            .collect();

        assert_eq!(
            stages,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            "ten stages over the length of one break"
        );
    }

    #[test]
    fn a_slower_block_takes_longer_to_crack() {
        // The same elapsed time is further through a dirt break than a stone
        // one, because stone takes three times as long. Half of dirt's break is
        // halfway through dirt and a sixth of the way through stone.
        let half_of_dirt = DIRT_BREAK / 2;

        assert_eq!(stage_at(half_of_dirt, DIRT_BREAK), 5);
        assert_eq!(stage_at(half_of_dirt, DIRT_BREAK * 3), 1);
    }

    #[test]
    fn a_crack_that_has_been_held_stays_at_the_last_stage() {
        assert_eq!(stage_at(DIRT_BREAK, DIRT_BREAK), 9, "the last stage, held");
        assert_eq!(stage_at(DIRT_BREAK * 100, DIRT_BREAK), 9, "and held longer");
    }

    #[test]
    fn two_edits_are_two_frames_and_not_one_frame_holding_two_packets() {
        // The bug this is for: both edits written into one buffer and framed
        // once, so the frame claimed a length that covered two packets. The
        // client reads the first, finds ten bytes where there should be none,
        // and hangs up. Both clients died, so it looked like the crack and was
        // not.
        let enc = crate::protocol::frame::FrameEncoder::new();

        let mut first = Vec::new();
        crate::protocol::play::write_block_update(&mut first, 1, 2, 3, 0);
        let mut second = Vec::new();
        crate::protocol::play::write_block_update(&mut second, 4, 5, 6, 0);

        assert_eq!(first.len(), 10, "one block update is ten bytes");

        // What the code used to do: both bodies into one frame.
        let mut both = first.clone();
        both.extend_from_slice(&second);
        let mut one_frame = Vec::new();
        enc.encode(&both, &mut one_frame).expect("framed");

        // What it does now: a frame each.
        let mut two_frames = Vec::new();
        enc.encode(&first, &mut two_frames).expect("framed");
        enc.encode(&second, &mut two_frames).expect("framed");

        assert_ne!(
            one_frame, two_frames,
            "these are different streams, which is the whole point"
        );

        // The length the single frame claims is the whole of both packets, so
        // after the client has read one it has ten bytes left in a frame that
        // should have ended.
        let claimed = {
            let mut r = &one_frame[..];
            // read gives back the VarInt and how many bytes it took; the value
            // is inside the VarInt.
            crate::protocol::VarInt::read(&mut r).expect("a length").0 .0
        };
        assert_eq!(claimed, both.len() as i32);
        assert_eq!(
            claimed, 2 * first.len() as i32,
            "twice what one packet needs, which is how ten bytes end up over"
        );
    }

    /// Generates a chunk and reads the blocks back out of the wire bytes.
    ///
    /// This is the check that was missing when terrain was first connected: the
    /// packet decoded at the client and showed nothing, which is the one failure
    /// a length assertion cannot see. Decoding the bytes here says whether the
    /// ground is actually in them.
    #[test]
    fn generated_terrain_survives_the_wire_format() {
        use crate::world::block::BlockRegistry;
        use crate::world::pos::ChunkPos;
        use crate::world::terrain;

        let mut registry = BlockRegistry::new();
        let palette = terrain::register_terrain(&mut registry);
        let chunk = terrain::generate_chunk(ChunkPos { x: 0, z: 0 }, &palette);

        let mut sections: Vec<Vec<i32>> = Vec::new();
        for section in chunk.sections() {
            sections.push(
                crate::world::section::iter_blocks(section)
                    .map(|block| to_state_id(block, &palette))
                    .collect(),
            );
        }

        // The generator fills from the bottom of the world up, so the very first
        // block of the first section is stone, and there has to be a surface
        // somewhere above it with grass on it.
        let first = sections[0][0];
        assert_eq!(
            first,
            state_id("minecraft:stone"),
            "the bottom of the world is stone"
        );

        let grass = state_id("minecraft:grass_block");
        let air = state_id("minecraft:air");
        let all: Vec<i32> = sections.concat();
        assert!(all.contains(&grass), "there is grass somewhere");
        assert!(all.contains(&air), "and air above the surface");

        // Now the actual wire bytes, decoded by a reader written from the
        // decompiled layout rather than from the writer.
        let mut wire = Vec::new();
        crate::protocol::chunk::write_chunk_with_states(&mut wire, 0, 0, &sections);
        let decoded = decode_first_section(&wire);

assert_eq!(
            decoded.first().copied(),
            Some(first),
            "the first block must come back out of the packed data unchanged: \
             wrote {first}, read {:?}, palette {:?}",
            decoded.first(),
            {
                let mut j = 1 + 8;
                let (maps, used) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("a count");
                j += used;
                for _ in 0..maps.0 {
                    let (_, u1) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("a type");
                    j += u1;
                    let (longs, u2) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("longs");
                    j += u2 + longs.0 as usize * 8;
                }
                let (_, u3) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("len");
                j += u3;
                let (_, u4) = crate::protocol::VarInt::read(&mut &wire[j + 4..]).expect("bits");
                j += 4 + u4;
                let (size, u5) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("size");
                j += u5;
                let mut palette = Vec::new();
                for _ in 0..size.0 {
                    let (v, u6) = crate::protocol::VarInt::read(&mut &wire[j..]).expect("id");
                    j += u6;
                    palette.push(v.0);
                }
                palette
            }
        );
assert_eq!(
            decoded.iter().filter(|b| **b == grass).count(),
            sections[0].iter().filter(|b| **b == grass).count(),
            "and every grass block in the section that was decoded"
        );
    }

    /// Reads the block states of the first section back out of a chunk packet.
    ///
    /// Mirrors what the client does: skip the id and the coordinates, the
    /// heightmap map, the buffer length, then walk the first section's container.
    fn decode_first_section(packet: &[u8]) -> Vec<i32> {
        use crate::protocol::VarInt;

        let mut i = 0;
        let varint = |i: &mut usize| -> i32 {
            let (value, used) = VarInt::read(&mut &packet[*i..]).expect("a VarInt");
            *i += used;
            value.0
        };

        assert_eq!(varint(&mut i), 46, "the chunk packet id");
        i += 8; // x and z

        let heightmaps = varint(&mut i);
        for _ in 0..heightmaps {
            varint(&mut i); // the heightmap type
            let longs = varint(&mut i);
            i += longs as usize * 8;
        }

        let buffer_len = varint(&mut i) as usize;
        let start = i;

        i += 4; // the two shorts
        let bits = packet[i];
        i += 1;

        let mut out = Vec::new();
if bits == 0 {
            // A single value fills every entry, which is what the client does
            // with a zero bit width. Returning nothing here was the bug: the
            // section looked empty when it was uniformly one block.
            let (value, _) = VarInt::read(&mut &packet[i..]).expect("a VarInt");
            return vec![value.0; crate::protocol::chunk::BLOCK_ENTRIES_DIV];
        }

        let size = varint(&mut i);
        let mut palette = Vec::new();
        for _ in 0..size {
            palette.push(varint(&mut i));
        }

        let per_long = 64 / bits as usize;
        let count = crate::protocol::chunk::BLOCK_ENTRIES_DIV;
        for entry in 0..count {
            let long = entry / per_long;
            let shift = (entry % per_long) * bits as usize;
            let bytes = &packet[i + long * 8..i + long * 8 + 8];
            let word = u64::from_be_bytes(bytes.try_into().expect("eight bytes"));
            out.push(palette[((word >> shift) as u32 & ((1 << bits) - 1)) as usize]);
        }

        assert!(start + buffer_len <= packet.len(), "the buffer length is sane");
        out
    }
}
