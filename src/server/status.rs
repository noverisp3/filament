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
use crate::protocol::item::{self, slots, Inventory, ItemStack};
use crate::protocol::container as c;
use crate::world::pos::Direction;
use crate::protocol::VarInt;

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
                    // The status is built per connection, not once at startup,
                    // because the two numbers a browser shows -- how many and
                    // who -- are both things that change while the server runs.
                    let mut status = self.status.clone();
                    status.online = self.wire.player_count() as u32;
                    status.sample = self
                        .wire
                        .snapshot()
                        .into_iter()
                        .map(|p| (p.name, uuid_undashed(&p.uuid)))
                        .collect();
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
                            if client_went_away(&e) {
                                log::note(format!("[conn {id}] client went away: {e}"));
                            } else {
                                log::error(format!("[conn {id}] ended with an error: {e}"));
                            }
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

        // Waiting here, behind the connections being cut, is what makes the
        // save real rather than raced: shutdown is a command whose body is the
        // write, answered only once the files are closed. Whether anything
        // was saved is a matter of whether a world folder was opened at all:
        // an empty save reported as a save is a line the next reader trusts
        // for nothing.
        let ticks = self.world.shutdown();
        if std::env::var_os("FILAMENT_WORLD").is_some() {
            log::note(format!("the world saved, {} ticks run", ticks));
        } else {
            log::note(format!(
                "nothing saved -- no world folder was opened (FILAMENT_WORLD); {} ticks run",
                ticks
            ));
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

/// How far a player may claim to have moved in one poll without being put back.
///
/// The game's own check is one hundred blocks per tick against the velocity it
/// already knows; this runs per poll rather than per tick, and takes the same
/// hundred blocks as a floor. A real client cannot cross it -- sprinting is
/// five and a half -- while a teleport or a lag spike of any size can.
const MAX_MOVE_PER_POLL: f64 = 100.0;

/// The edge of the world, as the game itself clamps it.
const MAX_COORD: f64 = 3.0e7;

/// How many chunks a batch carries when the client has not said otherwise.
///
/// The game's own figure: `PlayerChunkSender` starts at nine a tick, and the
/// client replaces it with the rate it wants after the first batch.
const DEFAULT_CHUNKS_PER_TICK: f32 = 9.0;

/// Answers one connection's handshake and status exchange.
///
/// Deliberately does not implement login: the point here is to be visible in
/// the multiplayer browser, which the client reaches only after status.
/// Whether an error means the client went away, rather than the server failing.
///
/// A player closing the game is not a fault, and it is reported by whatever
/// the socket happened to be doing at the time. On Windows a window closed
/// mid-transfer leaves an overlapped read outstanding, and the next one on that
/// socket reports ERROR_IO_PENDING, which is os error 997. A connection reset
/// is 10054, an abort is 10053, and a timed out connection is 10060; the
/// portable kinds cover the same ground elsewhere.
///
/// It matters that this is answered before the connection is reported as
/// broken. The cleanup has already run by the time the error reaches here, so
/// an ERROR line for a player who closed their own game is a false alarm, and a
/// false alarm costs the time of whoever reads the log next.
fn client_went_away(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;

    if matches!(
        e.kind(),
        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
    ) {
        return true;
    }

    // Windows reports these by number. ERROR_IO_PENDING is the surprising one:
    // it is not a failure at all, it is the leftover of a read that was
    // cancelled by its timeout, and it only becomes an error because the
    // connection was already going down.
    matches!(e.raw_os_error(), Some(53 | 54 | 60 | 997))
}

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
    //
    // The entity id is taken here rather than inside play() because it is part
    // of what a registration is: the tab list, the body and every later movement
    // are all addressed by it, so a player who is registered without one cannot
    // be described to anybody else.
    let entity_id = crate::server::next_entity_id();
    let registration = session.wire.register(
        &name,
        profile_id.bytes(),
        entity_id,
        crate::world::terrain::spawn_point(),
    );
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

    // 0. The brand, which the game sends first thing in this phase
    //    (`startConfiguration` opens with it). The client keeps it and shows
    //    it in the debug screen as the server's name, so a server that says
    //    nothing is a null server there.
    let mut body = Vec::new();
    crate::protocol::configuration::write_brand(&mut body, "filament");
    enc.encode(&body, &mut out).map_err(io_err)?;
    log.out_packet("custom_payload", "configuration", body.len());

    // 1. The feature flags this world runs with.
    //
    //    ```java
    //    VANILLA = builder.createVanilla("vanilla");
    //    VANILLA_SET = FeatureFlagSet.of(VANILLA);
    //    DEFAULT_FLAGS = VANILLA_SET;
    //    ```
    //    (`FeatureFlags.java:31-42`) and `createVanilla` is
    //    `Identifier.withDefaultNamespace(name)`, so the one that ships is
    //    `minecraft:vanilla` (`FeatureFlagRegistry.java:87-88`).
    //
    //    An empty list is not "no requirement" -- it is "nothing is enabled",
    //    and every item whose prototype asks for a flag then reads as disabled
    //    and says so on its tooltip. Sending one the client has not heard of
    //    would be worse, so this is the game's own default rather than a
    //    guess.
    let mut body = Vec::new();
    write_update_enabled_features(&mut body, &["minecraft:vanilla"]);
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
                            // How far away other players stay in *this*
                            // player's world. Set here because this is where
                            // the client finally says how much world it wants;
                            // before that the registration's default stands.
                            session
                                .wire
                                .set_view_distance(registration.id, view_distance);
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
/// Sends the buffer and empties it.
///
/// `write_all` sends the bytes and leaves them in the buffer it was given, so a
/// buffer that is written twice sends its whole contents twice. That is how a
/// client received the login packet and every chunk in the opening batch a
/// second time, hours of play later than the first copy was drawn: the resend
/// is not noticed as a resend, it is noticed as the world undoing an edit.
///
/// Emptying is the whole of the discipline. Everything else about when to write
/// is the caller's, and this says nothing about the contents.
fn flush_buffer<S: Write>(stream: &mut S, out: &mut Vec<u8>) -> std::io::Result<()> {
    stream.write_all(out)?;
    stream.flush()?;
    out.clear();
    Ok(())
}

// Which chunks a player standing at `player_chunk` has coming, and which ones
// can be taken off them.
//
// Two squares, and the distance between them is the whole point. Chunks within
// the view radius are wanted: they are what gets sent. Chunks within one ring
// further out are merely kept, and nothing is forgotten until a chunk is
// outside that.
//
// Using one square for both meant a player crossing a boundary had the
// terrain under them forgotten before its replacement existed. want_chunk
// returns None until the terrain pool has built a chunk, so the forget went out
// on one pass and the replacement arrived on a later one, and in between the
// client held neither. It looked like a hitch to the player walking and a
// hitch to whoever was watching them, which is why it was reported as motion
// rather than as a world fault.
//
// Sorting both, so the same walk produces the same bytes in the same order,
// which is what makes a session reproducible.
/// A chunk coordinate pair, which is what a chunk is named by throughout.
type ChunkCoord = (i32, i32);

/// A square of chunk coordinates, used for the windows a player is sent and
/// for the wider one it is allowed to forget from.
type ChunkSquare = std::collections::HashSet<ChunkCoord>;

/// Arrivals are sorted by distance from the player, so the ground nearest them
/// is asked for first and arrives first.
///
/// This was sorted by coordinate, which is reproducible and wrong. A player
/// walking east is handed a wall of new terrain, and sorting that wall by
/// (x, z) sends its north end before its south, so the chunk at their feet is
/// near the back of the queue and the chunk eight to their left is near the
/// front. The login batch already goes nearest first, because it walks the
/// rings outwards, so one player was given two different orders in a session
/// and only one of them was the useful one.
///
/// The coordinate is the tiebreak rather than the other way round: two chunks
/// at the same distance are common at a corner, and the bytes a walk produces
/// should still come out in the same order every time.
fn chunk_edges(
    player_chunk: (i32, i32),
    radius: i32,
    sent: &ChunkSquare,
) -> (Vec<ChunkCoord>, Vec<ChunkCoord>) {
    let square = |r: i32| -> ChunkSquare {
        let side = (2 * r + 1) as usize;
        let mut set = std::collections::HashSet::with_capacity(side * side);
        for dx in -r..=r {
            for dz in -r..=r {
                set.insert((player_chunk.0 + dx, player_chunk.1 + dz));
            }
        }
        set
    };

    let wanted = square(radius);
    let keep = square(radius + 1);

    let distance = |at: ChunkCoord| {
        let dx = i64::from(at.0 - player_chunk.0);
        let dz = i64::from(at.1 - player_chunk.1);
        dx * dx + dz * dz
    };

    let mut arriving: Vec<ChunkCoord> = wanted.difference(sent).copied().collect();
    arriving.sort_by_key(|&at| (distance(at), at));
    let mut leaving: Vec<ChunkCoord> = sent.difference(&keep).copied().collect();
    leaving.sort_unstable();
    (arriving, leaving)
}

fn play(
    stream: &mut TcpStream,
    mut dec: FrameDecoder,
    enc: FrameEncoder,
    log: ConnLog,
    radius: i32,
    session: Session,
    registration: super::broadcast::Registration,
) -> std::io::Result<()> {
    use crate::protocol::play as p;

    let player_id = registration.id;
    let log = log.phase("play");

    // Armed here and dropped however this function returns, so a player who
    // disconnects mid-write is announced the same as one who leaves politely.
    let _departure = super::broadcast::Departure::arm(session.wire.clone(), &registration);

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
    //
    //    It was taken before the registration, so this is the same one that the
    //    other players are told about.
    let entity_id = registration.entity_id;
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

    // 5. No world border, and the full clock sync straight after it. The order
    //    is the game's own (`PlayerList.sendLevelInfo` sends the border, then
    //    `createFullSyncPacket`, then the spawn), because the client runs the
    //    sky from the count this packet carries and the later beats correct
    //    that count without naming the clock again.
    let mut body = Vec::new();
    p::write_initialize_border(&mut body);
    send(&mut out, &body, "initialize_border", &log)?;

    let mut body = Vec::new();
    if let Some((game_time, day_time)) = session.world.clock() {
        p::write_set_time(&mut body, game_time as i64, day_time as i64);
        // The number on the log, so a sky a player calls wrong can be checked
        // against the world's own count rather than argued about: a day is
        // twenty-four thousand ticks, and the clock runs on unbounded.
        log.note(format!(
            "clock sync: age {}, day count {} ({:.1}% through a day)",
            game_time,
            day_time,
            (day_time % 24000) as f64 / 240.0
        ));
    } else {
        p::write_set_time(&mut body, 0, 0);
    }
    send(&mut out, &body, "set_time", &log)?;

    // 6. The spawn, on the actual surface of the terrain rather than at a
    //    fixed height. See `terrain::spawn_point`: the old constant was forty
    //    to eighty blocks in the air over this terrain, so every player fell on
    //    arrival, and the fall is what broke the other players' view of them.
    // 6. The spawn, as the world file says it is.
    //
    //    A world the game saved has a spawn it chose, and a number typed in
    //    to try something out is not that: a player who arrives somewhere
    //    else is arriving in the wrong place, and a world folder whose spawn
    //    is ignored is a world folder this server does not really read.
    let spawn_at = session.world.spawn_point().unwrap_or((0, 64, 0));
    let spawn = p::Movement {
        x: spawn_at.0 as f64 + 0.5,
        y: spawn_at.1 as f64,
        z: spawn_at.2 as f64 + 0.5,
        ..p::Movement::default()
    };
    let mut body = Vec::new();
    p::write_player_position(&mut body, 1, &spawn);
    send(&mut out, &body, "player_position", &log)?;

    // 7. Where a respawn returns to. A block position, so whole blocks, unlike
    //    the spawn above which is inside one.
    let mut body = Vec::new();
    p::write_set_default_spawn(
        &mut body,
        "minecraft:overworld",
        spawn_at.0,
        spawn_at.1,
        spawn_at.2,
    );
    send(&mut out, &body, "set_default_spawn_position", &log)?;

    // 8. How far the world goes, then where it is centred. Until the radius
    //    arrives the client is guessing, and until the centre arrives it has
    //    nowhere to ask for chunks.
    let mut body = Vec::new();
    p::write_chunk_cache_radius(&mut body, radius);
    send(&mut out, &body, "set_chunk_cache_radius", &log)?;

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
            // Asked for without waiting: either the world has it and the bytes
            // come straight back, or the pool is told and the wait below picks
            // it up when it lands.
            session.world.want_chunk(ChunkPos { x: dx, z: dz });
        }
    }

    // How many chunks the batch holds, so the wait below knows when it is done.
    let expected_chunks = (2 * radius + 1) * (2 * radius + 1);

    // Wait for the batch, asking again for anything the pool has not finished.
    //
    // The player is on a loading screen here, so this does wait — but it waits
    // by asking, not by standing in the world thread's way. The chunk lands
    // there as soon as a worker hands it over, and the next ask gets it. What
    // used to happen is that this connection held the world's only thread for
    // the length of the batch, so every other player on the server stopped
    // being answered while this one loaded.
    while sent_count < expected_chunks {
        for (dx, dz) in wanted_positions(radius) {
            if sent.contains(&(dx, dz)) {
                continue;
            }
            let Some(body) = session.world.want_chunk(ChunkPos { x: dx, z: dz }) else {
                continue;
            };
            enc.encode(&body, &mut out).map_err(io_err)?;
            sent_count += 1;
            sent.insert((dx, dz));
            if super::log::verbose() {
                log.out_packet("level_chunk_with_light", "play", body.len());
            }
        }

        if sent_count < expected_chunks {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    let mut body = Vec::new();
    // The count is how many chunks the batch held, which is the number just sent.
    p::write_chunk_batch_finished(&mut body, sent_count);
    enc.encode(&body, &mut out).map_err(io_err)?;

    log.note(format!("sent {sent_count} chunks around the spawn"));

    flush_buffer(stream, &mut out)?;

    // Emptied by `flush_buffer` above, and that is the whole of a fault that
    // took a long time to find. Without it, the second write below -- the one
    // that puts the introductions on the wire, added after the chunk batch --
    // carried everything the first write had as well, all of it again: the
    // login packet, and every one of the chunks above.
    //
    // It looked like nothing for as long as it was there. A chunk arriving
    // twice is the same chunk arriving twice, and a world made of chunks does
    // not visibly object. What it does do is arrive late: those chunks reach a
    // client that has already started playing, and by then some of them have
    // been edited. The copy generated before the edit turns up afterwards and
    // undoes it, so the block is whole on the server and gone in the client's
    // hands.
    log.note(if std::env::var_os("FILAMENT_WORLD").is_some() {
        "chunks sent, and edits land in the world folder"
    } else {
        "chunks sent; terrain generated, and it is not persisted"
    });

    // 12. The other players, and telling them about this one.
    //
    //     Two directions, and both are needed. A player who arrives into an
    //     empty-looking world is not being told about anybody, and the players
    //     already here have no reason to know a new one turned up. Doing only one
    //     of the two is the usual way this goes wrong: everyone can see who is
    //     already there and the new arrival appears to be alone.
    //
    //     They go out after the chunks, because a body needs somewhere to stand.
    //
    //     Order matters within each player: the tab list entry first, then the
    //     body. The other way round is a client that has an entity it has never
    //     been told the name of, which it renders as a nameless figure or, on
    //     some versions, does not render at all.
    let others = session.wire.snapshot();
    for other in &others {
        if other.id == player_id {
            // This connection is in the snapshot, because it was registered
            // before this point so that nothing arriving in the gap is lost.
            // Being in your own world is not something you are told.
            continue;
        }
        let mut info = Vec::new();
        p::write_player_info_add(&mut info, other.uuid, &other.name);
        send(&mut out, &info, "player_info_add", &log)?;

        let mut entity_packet = Vec::new();
        p::write_add_entity(
            &mut entity_packet,
            other.entity_id,
            other.uuid,
            p::PLAYER_ENTITY_TYPE,
            other.pos,
            other.yaw,
            other.pitch,
        );
        send(&mut out, &entity_packet, "add_entity", &log)?;

        // This is where a delta's zero point gets set. Without it the first
        // movement of that player has nothing to be measured from and is
        // dropped, so they would stand still until they moved a second time.
        session.wire.note_told(
            player_id,
            other.id,
            other.pos,
            other.yaw,
            other.pitch,
        );
    }

    let player_name = registration.name.clone();
    let player_uuid = registration.uuid;
    let player_yaw = spawn.yaw;
    let player_pitch = spawn.pitch;
    let player_spawn = (spawn.x, spawn.y, spawn.z);

    if !others.iter().any(|o| o.id != player_id) {
        log.note("first player on the server; nobody to be introduced to");
    } else {
        let mut info = Vec::new();
        p::write_player_info_add(&mut info, player_uuid, &player_name);
        session.wire.send_others(player_id, info);

        let mut entity_packet = Vec::new();
        p::write_add_entity(
            &mut entity_packet,
            entity_id,
            player_uuid,
            p::PLAYER_ENTITY_TYPE,
            player_spawn,
            player_yaw,
            player_pitch,
        );
        session.wire.send_others(player_id, entity_packet);

        // Same as above, the other way round: every player already here now
        // has this one at a known place, which is where their first delta is
        // measured from.
        for other in &others {
            if other.id != player_id {
                session
                    .wire
                    .note_told(other.id, player_id, player_spawn, player_yaw, player_pitch);
            }
        }

        log.note(format!("announced to the {} already here", others.len() - 1));
    }

    // The buffer is flushed here and not after the chunk batch.
    //
    // Everything in step 12 that goes to the players already here is queued to
    // them directly and is unaffected. What is missing otherwise is the other
    // direction: the players *this* connection is told about are written into
    // `out` by the same `send` the rest of this function uses, and `out` was
    // last written to the socket a few lines ago, at the end of the chunk batch.
    //
    // So a player arriving second was written about everybody already here and
    // never received it, while they received the new arrival perfectly well,
    // because that went out through the queue. One direction worked, the other
    // silently did not, and it looked like a rendering fault rather than a
    // missing write.
    flush_buffer(stream, &mut out)?;

    // 13. What they are carrying, which is nothing.
    //
    // Sent as the whole window rather than a slot at a time, which is what the
    // game does on arrival. An empty inventory is forty-six zero bytes, so the
    // cost is one packet and it is sent once.
    {
        let mut body = Vec::new();
        item::write_container_set_content(&mut body, &Inventory::empty(), 0);
        send(&mut out, &body, "container_set_content", &log)?;
        stream.write_all(&out)?;
        stream.flush()?;
        out.clear();
    }

    // 14. Keep the connection alive, and follow the player.
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
    let mut keep_alive = KeepAlive::new();
    let mut last_keep_alive = std::time::Instant::now();
    let mut last_clock = std::time::Instant::now();
    let mut latency_logged = false;

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
    // The join teleport was id 1; corrections made against a too-fast move
    // count up from there so no two asks are ever the same number.
    let mut teleport_seq: i32 = 1;

    // How many chunks a batch may carry, and how fast the client would like
    // them.
    //
    // The client answers every finished batch with the rate it can take, and
    // the game keeps to it: nine a tick by default. A server that sends the
    // whole ring at once spends its bandwidth in one burst and leaves the
    // client to sort it out, which is what the client's own rate is there to
    // prevent.
    let mut desired_chunks_per_tick: f32 = DEFAULT_CHUNKS_PER_TICK;
    let mut batch_quota: f32 = 0.0;

    // What they are carrying, which starts empty and is filled by what they
    // break.
    //
    // The client keeps its own copy and draws it immediately, so the server has
    // to say what it thinks rather than leave the client to guess: a player
    // holding nothing the server has never heard of is a player holding
    // something they cannot put down.
    let mut inventory = Inventory::empty();

    // Slot changes to tell them about, each already a whole packet.
    //
    // Kept apart from the block edits because they are answered to nobody else:
    // what one player is carrying is not on anyone else's screen.
    let mut slots: Vec<(usize, Option<ItemStack>)> = Vec::new();

    // What is on the cursor: the stack a player picked up but has not put down.
    //
    // It lives nowhere in the window, so it is the one thing that can be held
    // without the server's slot numbering knowing, and it is what nearly every
    // click changes. A server that did not keep it would answer the second click
    // of a drag as though the first had never happened.
    let mut carried: Option<ItemStack> = None;

    // Which of the nine is selected. The client sends this every time the wheel
    // turns, and it is the only way the server finds out which block a right
    // click is about to place.
    let mut held_slot: u16 = slots::HOTBAR_START as u16;

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

    // The acknowledgements that settle a change the client has already guessed.
    //
    // Kept apart from the edits because they go to one person and not to
    // everybody: the guess being settled is the one belonging to the player who
    // made it, and the other players had no guess to settle.
    let mut acks: Vec<Vec<u8>> = Vec::new();

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
            {
                // The rotation is carried even on the packets that do not have
                // a position, because a client turning on the spot is still
                // turning and the people watching should see it.
                if let Some(yaw) = moved.yaw {
                    player.yaw = yaw;
                }
                if let Some(pitch) = moved.pitch {
                    player.pitch = pitch;
                }

                if let (Some(x), Some(y), Some(z)) = (moved.x, moved.y, moved.z) {
                    // The two checks the game's own listener runs, in the same
                    // order: numbers that are not numbers at all, and a move
                    // that is too far to have happened since the last one.
                    //
                    // A NaN or an infinity is a broken or malicious client,
                    // and writing it into the pose would poison every packet
                    // built from it -- the position field of an entity packet
                    // is not checked by anyone downstream.
                    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                        log.note("refused a move that was not a number");
                    } else if dist_squared(player.x, player.y, player.z, x, y, z)
                        > MAX_MOVE_PER_POLL * MAX_MOVE_PER_POLL
                    {
                        // Too fast to be real. The position is not taken, and
                        // the client is put back where it was: the game's own
                        // answer is a teleport, which the client obeys without
                        // asking, and the correction carries a fresh id so a
                        // client that ignored one is asked again.
                        teleport_seq += 1;
                        let mut fix = Vec::new();
                        p::write_player_position(&mut fix, teleport_seq, &player);
                        enc.encode(&fix, &mut out).map_err(io_err)?;
                        log.note(format!(
                            "refused a move to {x:.1} {y:.1} {z:.1}, too fast from {:.1} {:.1} {:.1}",
                            player.x, player.y, player.z
                        ));
                    } else {
                        // The game clamps to its own world border of three
                        // times ten to the seven; anything past that is not a
                        // place, and broadcasting it would overflow nothing
                        // here but everything on the client.
                        player.x = x.clamp(-MAX_COORD, MAX_COORD);
                        player.y = y.clamp(-MAX_COORD, MAX_COORD);
                        player.z = z.clamp(-MAX_COORD, MAX_COORD);
                        session.wire.update_pose(
                            player_id,
                            (player.x, player.y, player.z),
                            player.yaw,
                            player.pitch,
                        );
                        session.wire.send_movement(player_id, true);
                    }
                } else {
                    // Turned without moving: the pose still moved, so anybody
                    // told about it needs telling again.
                    session.wire.update_pose(player_id, (player.x, player.y, player.z), player.yaw, player.pitch);
                    session.wire.send_movement(player_id, true);
                }
            }

            // A swing, or a punch. The packet is empty — upstream it is a
            // record with no fields at all — so there is nothing to decode and
            // the id is the whole message.
            //
            // Their own client has already drawn it. This is so that everybody
            // *else* sees it too, which is the entire difference between two
            // people on a server and two people in separate worlds.
            if id == p::id::SERVERBOUND_PUNCH {
                session.wire.send_swing(player_id, p::MAIN_HAND);
            }

            // The answer to a keep alive: one long.
            //
            // Without this the server only ever sends challenges, and cannot tell
            // a player who is there from one whose socket is still half open
            // because their network went away and nothing followed -- no FIN, no
            // RST, nothing. That connection holds an entity id, a name and a
            // place in the player list until the operating system notices, which
            // on Windows is measured in hours.
            if id == p::id::SERVERBOUND_KEEP_ALIVE {
                // Step over the packet id first. It is not optional: the frame
                // still carries it, and it is a small number that parses as a
                // perfectly ordinary answer, so reading from byte zero means
                // every keep alive looks like a client echoing back the id
                // instead of the challenge -- and every connection is dropped
                // for being rude about it.
                //
                // The payload is a plain long, not a VarInt. The game writes it
                // with `output.writeLong(id)` and reads it with `readLong()`,
                // which is eight bytes big-endian; treating it as a VarInt
                // decodes the first byte and calls it an answer.
                let body = p::serverbound_body(&frame, p::id::SERVERBOUND_KEEP_ALIVE)
                    .map_err(|e| io_err(FrameError::Decode(e)))?;
                if body.len() < 8 {
                    return Err(io_err(FrameError::Decode(
                        crate::protocol::DecodeError::UnexpectedEof {
                            wanted: "a keep alive answer",
                            got: body.len(),
                        },
                    )));
                }
                let answer = i64::from_be_bytes(body[..8].try_into().expect("eight"));
                let since = last_keep_alive.elapsed();
                if !keep_alive.answered(answer, since) {
                    log.note(format!(
                        "answered keep alive {answer} which was never asked for; \
                         the connection is not one we are having a conversation with"
                    ));
                    return Ok(());
                }
                let latency = keep_alive.latency_ms();
                if super::log::verbose() && !latency_logged {
                    latency_logged = true;
                    log.note(format!("round trip about {latency} ms"));
                }
            }

            // The client's own ping, which it sends every tick to time the
            // round trip for the debug screen. The answer is the same eight
            // bytes back, unchanged; a server that stays quiet leaves the
            // latency reading blank.
            if id == p::id::SERVERBOUND_PING_REQUEST {
                let body = p::serverbound_body(&frame, p::id::SERVERBOUND_PING_REQUEST)
                    .map_err(|e| io_err(FrameError::Decode(e)))?;
                if body.len() >= 8 {
                    let time = i64::from_be_bytes(body[..8].try_into().expect("eight"));
                    let mut pong = Vec::new();
                    p::write_pong_response(&mut pong, time);
                    enc.encode(&pong, &mut out).map_err(io_err)?;
                }
            }

            // The client answering a teleport with the position it ended up
            // at. The server believes the player is where it put them until
            // this arrives, and the player is somewhere else the moment they
            // fall.
            if id == p::id::SERVERBOUND_ACCEPT_TELEPORTATION {
                let body = p::serverbound_body(&frame, p::id::SERVERBOUND_ACCEPT_TELEPORTATION)
                    .map_err(|e| io_err(FrameError::Decode(e)))?;
                // A VarInt id, then three doubles and two floats. The id is
                // read rather than skipped, because a VarInt is however many
                // bytes it needs and the position follows it.
                let mut rest = body;
                let (_teleport_id, _) = VarInt::read(&mut rest)
                    .map_err(|e| io_err(FrameError::Decode(e)))?;
                if rest.len() >= 24 + 8 {
                    let x = f64::from_be_bytes(rest[0..8].try_into().expect("eight"));
                    let y = f64::from_be_bytes(rest[8..16].try_into().expect("eight"));
                    let z = f64::from_be_bytes(rest[16..24].try_into().expect("eight"));
                    let yaw = f32::from_be_bytes(rest[24..28].try_into().expect("four"));
                    let pitch = f32::from_be_bytes(rest[28..32].try_into().expect("four"));
                    if x.is_finite() && y.is_finite() && z.is_finite() {
                        player.x = x.clamp(-MAX_COORD, MAX_COORD);
                        player.y = y.clamp(-MAX_COORD, MAX_COORD);
                        player.z = z.clamp(-MAX_COORD, MAX_COORD);
                        player.yaw = yaw;
                        player.pitch = pitch;
                        session.wire.update_pose(
                            player_id,
                            (player.x, player.y, player.z),
                            player.yaw,
                            player.pitch,
                        );
                        session.wire.send_movement(player_id, true);
                    }
                }
            }

            // Something was typed. The message is the first field of the
            // packet and the rest of it is the signature and the last-seen
            // list, which this server has no use for: what it cannot check,
            // it does not claim to have checked.
            //
            // The line is sent back as a system message rather than as a
            // player chat, because a player chat carries the signature and
            // the session the client never got. The client shows both the
            // same way, in the chat box.
            if id == p::id::SERVERBOUND_CHAT {
                let body = p::serverbound_body(&frame, p::id::SERVERBOUND_CHAT)
                    .map_err(|e| io_err(FrameError::Decode(e)))?;
                let mut r = body;
                let (VarInt(len), _) = VarInt::read(&mut r)
                    .map_err(|e| io_err(FrameError::Decode(e)))?;
                // The game allows two hundred and fifty-six characters.
                if len >= 0 && len as usize <= r.len() && len <= 256 {
                    match std::str::from_utf8(&r[..len as usize]) {
                        Ok(text) if !text.is_empty() => {
                            let line = format!("<{player_name}> {text}");
                            let mut chat = Vec::new();
                            p::write_system_chat(&mut chat, &line);
                            // The body, not a framed packet: every connection
                            // frames it for itself, because compression is
                            // per connection. A body framed here and framed
                            // again there is a packet with a length in front
                            // of a length, and the client reads the second one
                            // as the first byte of the next packet.
                            session.wire.send_all(chat);
                            log.note(format!("{player_name} said {text}"));
                        }
                        Ok(_) => {}
                        Err(_) => log.note("ignored a chat line that was not text"),
                    }
                }
            }

            // The client answering a finished chunk batch with the rate it
            // would like. One float, and the server keeps to it from then on.
            if id == p::id::SERVERBOUND_CHUNK_BATCH_RECEIVED {
                let body = p::serverbound_body(&frame, p::id::SERVERBOUND_CHUNK_BATCH_RECEIVED)
                    .map_err(|e| io_err(FrameError::Decode(e)))?;
                if body.len() >= 4 {
                    let rate = f32::from_be_bytes(body[..4].try_into().expect("four"));
                    if rate.is_finite() && rate >= 1.0 {
                        // Bounded: a client that asks for a million chunks a
                        // tick is not asking for chunks.
                        desired_chunks_per_tick = rate.min(64.0);
                    }
                }
            }

            // Putting a block down.
            //
            // The order here is the game's, and the order is most of the
            // behaviour:
            //
            // ```java
            // if (this.hasClientLoaded()) {
            //    this.ackBlockChangesUpTo(packet.sequence());      // <- first
            //    ...
            //    if (this.player.isWithinBlockInteractionRange(pos, 1.0)) {
            //       Vec3 distance = location.subtract(Vec3.atCenterOf(pos));
            //       if (|distance.x| < 1.0000001 && ...) { ... }
            // ```
            // (ServerGamePacketListenerImpl.java:1403-1417)
            //
            // The acknowledgement comes before any of it, including before the
            // reach check. A client that guessed a placement is waiting on that
            // number, and refusing to answer it is how a player ends up unable
            // to place anything at all.
            if id == p::id::SERVERBOUND_USE_ITEM_ON
                && let Ok(use_on) = item::UseItemOn::read(&frame)
            {
                let mut ack = Vec::new();
                p::write_block_changed_ack(&mut ack, use_on.sequence);
                acks.push(ack);

                let outcome = place_block(
                    &session.world,
                    &inventory,
                    held_slot,
                    &player,
                    use_on,
                );

                // What the client is told about the two positions involved,
                // whether or not anything was placed. This is not a confirmation
                // -- it is how a client prediction is corrected, and the game
                // sends it unconditionally (`:1458-1459`).
                let face = Direction::from_3d_data_value(use_on.face);
                for pos in [
                    use_on.pos,
                    face.map_or(use_on.pos, |f| use_on.pos.offset(f)),
                ] {
                    let now = session
                        .world
                        .block_at(pos)
                        .flatten()
                        .unwrap_or(AIR_STATE);
                    let mut body = Vec::new();
                    p::write_block_update(&mut body, pos.x, pos.y, pos.z, now);
                    edits.push(body);
                }

                match outcome {
                    Some(Placement::Placed { target, item }) => {
                        session.wire.send_swing(player_id, p::MAIN_HAND);
                        // One out of the stack. The last block emptying the slot
                        // is as much a change as the counts moving, so the slot
                        // is reported either way.
                        let left = inventory.take_one(held_slot as usize);
                        inventory.set(held_slot as usize, left);
                        slots.push((held_slot as usize, inventory.get(held_slot as usize)));
                        log.note(format!(
                            "placed a block at {} {} {} from {}",
                            target.x,
                            target.y,
                            target.z,
                            describe(Some(item))
                        ));
                    }
                    Some(Placement::Refused { why }) => log.note(format!("refused: {why}")),
                    None => {}
                }
            }

/// What a click on the world did, once every check has had its say.
enum Placement {
    Placed {
        target: crate::world::pos::BlockPos,
        item: ItemStack,
    },
    Refused {
        why: &'static str,
    },
}

/// Answers a right-click on a block face with the block that went down.
///
/// Every check the game makes before placing, in its order, and for the same
/// reasons. A refusal is a refusal rather than a silent no-op because the
/// client has already drawn the block and needs to be told otherwise.
fn place_block(
    world: &ticking::WorldHandle,
    inventory: &Inventory,
    held_slot: u16,
    player: &p::Movement,
    use_on: item::UseItemOn,
) -> Option<Placement> {
    // The reach is the same one that governs breaking: the player's eye to the
    // block, plus the one block of margin `isWithinBlockInteractionRange(pos,
    // 1.0)` adds (`:1414`). Placing without it is placing anywhere, which is
    // the same hole as breaking a block a kilometre away.
    if reach_to(player, use_on.pos) > MAX_REACH {
        return Some(Placement::Refused {
            why: "too far away to reach",
        });
    }

    // The off hand is a different window and there is nothing in it to place.
    if use_on.hand != p::MAIN_HAND {
        return Some(Placement::Refused {
            why: "the off hand holds nothing this server places",
        });
    }

    // What is in the selected slot. Nothing there means nothing placed, and no
    // block is invented from whatever else the player happens to be carrying.
    let Some(item) = inventory.get(held_slot as usize) else {
        return Some(Placement::Refused {
            why: "holding nothing",
        });
    };

    // An item id is not a block id, and the game's names are not the registry's
    // names -- `minecraft:grass_block` against `grass`. The join goes through
    // the same table that turns a block into the name the wire uses, so the two
    // cannot drift apart.
    let Some(name) = crate::generated::item_name(item.item as u32) else {
        return Some(Placement::Refused {
            why: "not an item this registry has",
        });
    };
    let Some(terrain) = world.terrain_blocks() else {
        return Some(Placement::Refused {
            why: "the world has no terrain palette",
        });
    };
    let block = ticking::block_for_name(&terrain, name);
    let Some(block) = block else {
        return Some(Placement::Refused {
            why: "not a block this world has",
        });
    };

    // A face the game does not have is not a face it placed against.
    let Some(face) = Direction::from_3d_data_value(use_on.face) else {
        return Some(Placement::Refused {
            why: "a face that is not one of the six",
        });
    };

    // Where the block goes. Clicking something solid puts the block on the
    // face; only something that can be replaced puts it inside, and nothing in
    // this world is clicked except a solid block, so this is the neighbour.
    let target = use_on.pos.offset(face);

    // The click is 0..1 within the block, so the location is the block plus it,
    // and the game's check is that each axis is within one block of the centre
    // (`:1415-1417`). A client claiming to have clicked somewhere else is
    // refused before it can place through a wall.
    for axis in 0..3 {
        let from_centre = use_on.click[axis] - 0.5;
        if !(-1.0000001..1.0000001).contains(&from_centre) {
            return Some(Placement::Refused {
                why: "the click is not on the block it names",
            });
        }
    }

    // Only into something empty. A block placed where there is already one is
    // not a replacement, it is a second block in the same place. `block_at`
    // answers in the game's state ids, and air is state zero.
    if world.block_at(target).flatten() != Some(AIR_STATE) {
        return Some(Placement::Refused {
            why: "there is already something there",
        });
    }

    match world.set_block(target, block) {
        Some(true) => Some(Placement::Placed { target, item }),
        _ => Some(Placement::Refused {
            why: "the world would not take it",
        }),
    }
}

            // Which keys are down: one byte of flags, after the id.
            //
            // This is the whole of sneaking as far as the network is concerned.
            // The client has already changed its own shape; everybody else has
            // to be told, and the pose is how.
            if id == p::id::SERVERBOUND_PLAYER_INPUT {
                let mut body = &frame[..];
                // Step over the id to reach the flags. Reading the id rather
                // than assuming its length is the whole discipline: a packet id
                // over 127 is two bytes and every field after it moves.
                if crate::protocol::VarInt::read(&mut body).is_ok()
                    && let Some((&flags, _)) = body.split_first()
                {
                    session
                        .wire
                        .set_crouching(player_id, flags & p::INPUT_SHIFT != 0);
                }
            }

            // Which of the nine is selected.
            //
            // One short, and the only way the server ever hears which hotbar slot
            // the player is on -- the client keeps it to itself otherwise, and
            // puts the right hand's block down without the server knowing which
            // block that was.
            if id == p::id::SERVERBOUND_SET_CARRIED_ITEM
                && let Ok(slot) = crate::protocol::container::read_set_carried_item(&frame)
            {
                // Nine windows slots, 36 to 44. Anything else is not a hotbar
                // slot whatever it claims to be, and is not adopted as one.
                if (slots::HOTBAR_START..slots::HOTBAR_END).contains(&(slot as usize))
                    && slot != held_slot
                {
                    held_slot = slot;
                    let held = inventory
                        .get(slot as usize)
                        .map_or("nothing".into(), |s| {
                            format!(
                                "{} x{}",
                                crate::generated::item_name(s.item as u32).unwrap_or("?"),
                                s.count
                            )
                        });
                    log.note(format!("holding slot {slot}: {held}"));
                }
            }

            // A click in the player's own window.
            //
            // The client does not describe the click, it reports the result of
            // it: every slot that changed goes back with its new contents, along
            // with whatever is left on the cursor. So this server never has to
            // understand a drag to know what a drag did.
            //
            // It does not mean the client gets the last word. Vanilla reads
            // those reported slots as a guess and compares:
            //
            // ```java
            // this.player.containerMenu.suppressRemoteUpdates();
            // this.player.containerMenu.clicked(slotIndex, buttonNum, containerInput, player);
            // for (Entry<HashedStack> e : fastIterable(packet.changedSlots())) {
            //     this.player.containerMenu.setRemoteSlotUnsafe(e.getIntKey(), e.getValue());
            // }
            // this.player.containerMenu.setRemoteCarried(packet.carriedItem());
            // this.player.containerMenu.resumeRemoteUpdates();
            // ```
            //
            // and then sends its own value wherever the two disagree. So the
            // server computes the click, the player is only corrected where the
            // two copies differ, and the server stays the authority -- which is
            // what stops a hotbar the server no longer agrees with.
            if id == p::id::SERVERBOUND_CONTAINER_CLICK {
                match crate::protocol::container::ContainerClickPacket::read(&frame) {
                    Ok(packet) if packet.container_id == slots::CONTAINER_ID => {
                        let got = c::click(
                            &mut inventory,
                            &mut carried,
                            packet.slot,
                            packet.button,
                            packet.input,
                        );

                        // The answer, plus wherever the client's own guess was
                        // wrong. Reporting a slot the client already has right
                        // is a packet per click for no change; leaving one the
                        // client has wrong is a window that stops matching.
                        let corrections =
                            corrections_needed(&got, &packet.changed);
                        slots.extend(corrections.iter().copied());

                        match got {
                            Some(got) => {
                                if got.carried != packet.carried {
                                    // The cursor is not a slot, so it cannot be
                                    // corrected by naming one. Saying so in the
                                    // log is the honest answer for now.
                                    log.note(format!(
                                        "cursor is {}, the client thought {}",
                                        describe(carried),
                                        describe(packet.carried)
                                    ));
                                }
                                if let Some(dropped) = got.dropped {
                                    log.note(format!(
                                        "dropped {} x{} -- there are no item entities yet",
                                        crate::generated::item_name(dropped.item as u32)
                                            .unwrap_or("?"),
                                        dropped.count
                                    ));
                                }
                            }
                            None => log.note(format!(
                                "refused {:?} on slot {}",
                                packet.input, packet.slot
                            )),
                        }
                    }
                    Ok(packet) if packet.container_id != slots::CONTAINER_ID => {
                        log.note(format!(
                            "a click on window {}, which is not this player's",
                            packet.container_id
                        ));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        // The bytes as well as the complaint. A parser that is
                        // wrong says where it stopped and nothing about what it
                        // was looking at, which is how a short read the wrong
                        // way round stayed a mystery for a whole session.
                        let shown = frame
                            .iter()
                            .take(24)
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        log.error(format!(
                            "a click would not read: {e}; {} bytes: {shown}",
                            frame.len()
                        ));
                    }
                }
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
                                    let registry = session.world.registry();
                                    let mined = session
                                        .world
                                        .world_block(pos)
                                        .flatten()
                                        .unwrap_or(terrain.air);
                                    break_time(mined, &terrain, registry.as_ref())
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

                                // And into their hand, which is how a player
                                // gets something to build with. The game drops
                                // an item entity for the player to walk over
                                // and pick up; there is no item entity here yet,
                                // so the block goes straight into the inventory,
                                // and a player who digs a hole has the dirt to
                                // fill it in again.
                                //
                                // What was there, what is there now, and the
                                // prediction number the client asked about, and
                                // what went into the hand that broke it.
                                //
                                // The names are the point. A state id on its
                                // own cannot be read by eye, and a block sent
                                // as the wrong one is a block that stays solid
                                // in the client while the server believes it
                                // is gone.
                                let name_of = |state: i32| {
                                    if session.world.terrain_blocks().is_some() {
                                        crate::server::ticking::state_id_name(state).to_string()
                                    } else {
                                        format!("state {state}")
                                    }
                                };

                                // The block that was there has a name, and
                                // that name is usually also an item -- grass
                                // block is `minecraft:grass_block` both ways.
                                // The name already carries its namespace, so it
                                // is looked up as it stands: adding
                                // `minecraft:` to it again asks for
                                // `minecraft:minecraft:grass_block`, which is
                                // not an item, and a server that quietly got
                                // that wrong would hand out nothing at all.
                                //
                                // The item id is a different number from the
                                // block state id, though: dirt is state 10 and
                                // item 484, grass block is 9 and 613.
                                if session.world.terrain_blocks().is_some() {
                                    let held = crate::server::ticking::state_id_name(was);
                                    match crate::server::ticking::held_item(was) {
                                        Some(item) => {
                                            // `add` says which slots it used. The
                                            // client is told about those and no
                                            // others, because a slot reported
                                            // changed that did not is a slot the
                                            // client redraws for nothing, and a
                                            // real change left unreported is a
                                            // hotbar that never fills.
                                            let used =
                                                inventory.add(ItemStack::new(item as i32, 1));
                                            let named = used.first().copied();
                                            for slot in &used {
                                                slots.push((*slot, inventory.get(*slot)));
                                            }
                                            if used.is_empty() {
                                                log.note(format!(
                                                    "{held} has nowhere to go and is lost"
                                                ));
                                            } else {
                                                log.note(format!(
                                                    "{held} went into slot {}",
                                                    named.unwrap_or_default()
                                                ));
                                            }
                                        }
                                        // A block with no item of that name is
                                        // simply not something that can be held.
                                        None => log.note(format!(
                                            "{held} has no item of that name"
                                        )),
                                    }
                                }
                                log.note(format!(
                                    "broke {bx} {by} {bz}: {} ({was}) -> {} ({now}), \
                                     client prediction sequence {}",
                                    name_of(was),
                                    name_of(now),
                                    action.sequence
                                ));

                                // And that the change is settled.
                                //
                                // The block is drawn gone the moment the update
                                // arrives, but the shape it used to have is
                                // kept until the guess that removed it is
                                // confirmed, and until then it is invisible
                                // and still solid: a player stands on air and
                                // walks into a wall that is not drawn. It only
                                // comes right when the chunk is sent again from
                                // the server, which is why it looked like the
                                // world was only right after reconnecting.
                                //
                                // The number to confirm it with is the one the
                                // player sent with their action. It is their
                                // prediction number and not one of ours: the
                                // client ends its predictions up to that
                                // number and no other, so a number of our own
                                // would settle nothing, and a number of ours
                                // that happened to be too large would settle
                                // guesses that had not been made yet.
                                let mut ack = Vec::new();
                                p::write_block_changed_ack(&mut ack, action.sequence);
                                acks.push(ack);

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
        //
        // Both lists are taken, not iterated. Leaving them in place makes every
        // pass send everything since the connection began, twenty times a
        // second: a repeated block update is invisible because it says the same
        // thing, and a repeated level event is a burst of particles that does
        // not stop until the player who caused it leaves.
        //
        // And they go to different places, which is not a detail. The block
        // update tells the client what is there now, and goes to everyone
        // including the player, who needs it because their own guess may have
        // been wrong. The level event is the dust and the sound of the break
        // happening, and the player's own client has already played both when
        // it predicted the block going: the game excludes them, and sending it
        // to them gives everyone watching a single break and the player who did
        // it two.
        // What the world has done to itself since this connection last looked.
        //
        // Drained, not read, so two players are not both sent the same change
        // and it is not sent twice because of that. It goes to everyone in
        // range rather than to whoever asked: a block that turned to grass
        // because something above it was mined is not one player's news.
        for (pos, _block) in session.world.take_tick_changes() {
            let now = session
                .world
                .block_at(pos)
                .flatten()
                .unwrap_or(AIR_STATE);
            // Into `edits` and not straight down the wire. `edits` is already
            // broadcast to everyone else when it is written out below, so
            // calling `send_others` here as well sent every tick-driven change
            // twice -- once here and once there -- while the player's own copy
            // went once, because only `edits` produces that. The log said two
            // broadcasts and one self-send for a single block, which is how it
            // was caught: it did not match any shape one change can have.
            let mut body = Vec::new();
            p::write_block_update(&mut body, pos.x, pos.y, pos.z, now);
            edits.push(body);
            if super::log::verbose() {
                log.note(format!(
                    "the world changed {} {} {} to {}",
                    pos.x,
                    pos.y,
                    pos.z,
                    ticking::state_id_name(now)
                ));
            }
        }

        if !edits.is_empty() || !effects.is_empty() || !acks.is_empty() || !slots.is_empty() {
            let outgoing = std::mem::take(&mut edits);
            let effects = std::mem::take(&mut effects);
            let acks = std::mem::take(&mut acks);

            let mut framed = Vec::new();
            for body in &outgoing {
                session.wire.send_others(player_id, body.clone());
                enc.encode(body, &mut framed).map_err(io_err)?;
            }
            // The acknowledgements go to nobody else. The guess they settle
            // belongs to the player who made it.
            for body in &acks {
                enc.encode(body, &mut framed).map_err(io_err)?;
            }
            // Nor do the slot changes. What one player is carrying is on
            // nobody else's screen, and broadcasting it would show everyone
            // what everyone else is holding.
            for (slot, stack) in std::mem::take(&mut slots) {
                let mut body = Vec::new();
                item::write_container_set_slot(&mut body, slot, stack, 0);
                enc.encode(&body, &mut framed).map_err(io_err)?;
                log.note(format!(
                    "slot {slot}: {}",
                    stack.map_or("empty".into(), |s| format!(
                        "{} x{}",
                        crate::generated::item_name(s.item as u32).unwrap_or("?"),
                        s.count
                    ))
                ));
            }
            // What this player is about to be handed, named.
            //
            // The broadcast log says what went out to the others. It says
            // nothing about the player who broke the block, whose copy is
            // written here instead, and that copy is the one that settles the
            // guess. A block drawn gone and still solid is what a missing or
            // out-of-order line here looks like, so it is written down.
            for body in outgoing.iter().chain(acks.iter()) {
                if let Some(what) = super::broadcast::describe_block_packet(body) {
                    log.note(format!("to the breaker: {what}"));
                }
            }
            if !framed.is_empty() {
                stream.write_all(&framed)?;
                stream.flush()?;
            }

            for body in &effects {
                session.wire.send_others(player_id, body.clone());
            }
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

        // Which chunks are coming and which may go: the window is a square of the view
        // radius, and what may be forgotten is a ring further out than that, so
        // the ground a player is leaving is still held when the ground they are
        // walking onto arrives. See chunk_edges, which is where the reason is.
        let (arriving, leaving) = chunk_edges(player_chunk, radius, &sent);

        if !arriving.is_empty() || !leaving.is_empty() {
            // Already sorted by chunk_edges, so the same walk produces the same
            // bytes in the same order.
            let sorted = arriving;

            // Take what is actually here *before* opening a batch.
            //
            // Opening one first and filling it afterwards means a pass where
            // the pool has finished nothing writes an empty batch — a start, a
            // finish saying zero, and a flush — twenty times a second for as
            // long as the ground takes. And it makes the count a lie: the
            // closing number is what the client uses to know the world it has
            // been sent is complete, so "17 chunks arrived" when none did is
            // worse than saying nothing.
            let mut ready: Vec<(crate::world::pos::ChunkPos, Vec<u8>)> = Vec::new();
            // What this batch has already taken. `sent` is no use for this: it
            // is only updated once the batch has been written, so during the tail
            // wait below it still says none of these have gone. Counting a chunk
            // twice means the batch claims more chunks than it was asked for --
            // which is how "17 chunks arrived" arrived as twenty-four, and the
            // same ground went on the wire twice.
            let mut taken: std::collections::HashSet<crate::world::pos::ChunkPos> =
                std::collections::HashSet::new();
            let mut waiting = false;
            for (dx, dz) in &sorted {
                let pos = crate::world::pos::ChunkPos { x: *dx, z: *dz };
                if let Some(body) = session.world.want_chunk(pos) {
                    taken.insert(pos);
                    ready.push((pos, body));
                } else {
                    waiting = true;
                }
            }

            // Pick up the tail of the wall before the batch is closed.
            //
            // The chunk the pool was still building when this pass started is
            // usually ready by now, microseconds later, against the fifty a
            // whole further pass would have cost. Taking it in the same batch
            // rather than the next is what stops the log reading "16 chunks
            // arrived" and then "1 chunks arrived", and the wire carrying a
            // second batch pair wrapped around a single chunk.
            //
            // Bounded, and only while something is genuinely outstanding: this
            // is the connection's own thread and it also has to answer keep
            // alives, so waiting here is waiting for the player too.
            if waiting {
                let deadline = std::time::Instant::now() + CHUNK_TAIL_BUDGET;
                while std::time::Instant::now() < deadline {
                    let mut outstanding = false;
                    for (dx, dz) in &sorted {
                        let pos = crate::world::pos::ChunkPos { x: *dx, z: *dz };
                        if !still_wanted(pos, &sent, &taken) {
                            continue;
                        }
                        match session.world.want_chunk(pos) {
                            Some(body) => {
                                taken.insert(pos);
                                ready.push((pos, body));
                            }
                            None => outstanding = true,
                        }
                    }
                    if !outstanding {
                        break;
                    }
                    thread::sleep(CHUNK_TAIL_STEP);
                }
            }

            if !ready.is_empty() {
                // The batch carries no more than the client asked for. What
                // is left over stays wanted, so the next pass sends it: the
                // chunks arrive over a few ticks rather than in one burst.
                batch_quota = (batch_quota + desired_chunks_per_tick)
                    .min(desired_chunks_per_tick.max(1.0));
                let allowed = batch_quota.floor().max(1.0) as usize;
                ready.truncate(allowed);

                let mut body = Vec::new();
                p::write_chunk_batch_start(&mut body);
                enc.encode(&body, &mut out).map_err(io_err)?;

                for (pos, body) in &ready {
                    enc.encode(body, &mut out).map_err(io_err)?;
                    sent.insert((pos.x, pos.z));
                    if super::log::verbose() {
                        log.out_packet("level_chunk_with_light", "play", body.len());
                    }
                }

                let mut body = Vec::new();
                p::write_chunk_batch_finished(&mut body, ready.len() as i32);
                enc.encode(&body, &mut out).map_err(io_err)?;
            }

            // The forgets go after the batch, and each is its own packet. They
            // are the client dropping chunks, so they are not part of a chunk
            // batch: the batch pair brackets chunks arriving, not leaving.
            //
            // Sorted already, like the arrivals: chunk_edges put them in order
            // so this walk is reproducible and so the forget of a chunk follows
            // the arrival that replaced it on the wire rather than preceding it.
            let dropped = leaving;
            for (dx, dz) in &dropped {
                let mut body = Vec::new();
                p::write_forget_chunk(&mut body, *dx, *dz);
                let mut wire = Vec::new();
                enc.encode(&body, &mut wire).map_err(io_err)?;
                out.extend_from_slice(&wire);
                sent.remove(&(*dx, *dz));
                session.world.forget(crate::world::pos::ChunkPos { x: *dx, z: *dz });
            }

            if !out.is_empty() {
                stream.write_all(&out)?;
                stream.flush()?;
            }

            // Only when something happened. The ring a player is walking into
            // stays in `arriving` for as long as the pool is working on it, so
            // logging on the fact of it being wanted rather than on the fact of
            // it being sent says the same thing every pass for as long as the
            // ground takes, which is a screenful of noise about nothing.
            if let Some(line) = chunk_report(sorted.len(), ready.len(), dropped.len()) {
                log.note(format!("{line} at {player_chunk:?}"));
            }
        } else if !out.is_empty() {
            // The center moved without any chunk following it, which is what
            // happens on a diagonal walk back through ground already sent.
            stream.write_all(&out)?;
            stream.flush()?;
        }

        // The age, every second, and nothing else. The clock the client runs is
        // left alone: it has been extrapolating from the join sync ever since,
        // and replacing it is what showed as a sun that stepped forward. This
        // beat corrects the count the client runs its clock by, and that count
        // is all the game's own periodic sync ever carries.
        if last_clock.elapsed() >= CLOCK_EVERY
            && let Some((game_time, _day_time)) = session.world.clock()
        {
            let mut body = Vec::new();
            p::write_set_time_bare(&mut body, game_time as i64);
            let mut wire = Vec::new();
            enc.encode(&body, &mut wire).map_err(io_err)?;
            stream.write_all(&wire)?;
            stream.flush()?;
            last_clock = std::time::Instant::now();

            if super::log::verbose() {
                log.out_packet("set_time", "play", body.len());
            }
        }

        match keep_alive.tick(last_keep_alive.elapsed(), KEEP_ALIVE_EVERY) {
            KeepAliveTick::Silent => {}
            KeepAliveTick::Send(id) => {
                let mut body = Vec::new();
                p::write_keep_alive(&mut body, id);
                let mut wire = Vec::new();
                enc.encode(&body, &mut wire).map_err(io_err)?;
                stream.write_all(&wire)?;
                stream.flush()?;
                last_keep_alive = std::time::Instant::now();

                if super::log::verbose() {
                    log.out_packet("keep_alive", "play", body.len());
                }
                continue;
            }
            KeepAliveTick::Timeout => {
                log.note(format!(
                    "keep alive {} was never answered; the connection is gone",
                    keep_alive.outstanding().unwrap_or_default()
                ));
                return Ok(());
            }
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
/// Every chunk within `radius` of the origin, in a fixed order.
///
/// Sorted so the same radius always produces the same sequence, which is what
/// makes a session reproducible: the same walk sends the same chunks in the
/// same order.
fn wanted_positions(radius: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::with_capacity(((2 * radius + 1) * (2 * radius + 1)) as usize);
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            out.push((dx, dz));
        }
    }
    out
}

/// Which slots the server has to send back, and why only those.
///
/// Vanilla compares the client's guess against its own and sends a slot only
/// where the two differ (`AbstractContainerMenu.synchronizeSlotToRemote`):
///
/// ```java
/// RemoteSlot remoteExpected = this.remoteSlots.get(i);
/// if (!remoteExpected.matches(current)) {
///     remoteExpected.force(current);
///     this.synchronizer.sendSlotChange(this, i, currentCopy.get());
/// }
/// ```
///
/// So a click the player got right is silent, and a click the player got wrong
/// is corrected rather than argued with -- the client keeps the last word on
/// what it draws, the server keeps it on what is true.
fn corrections_needed(
    got: &Option<c::Click>,
    reported: &[(i16, Option<ItemStack>)],
) -> Vec<(usize, Option<ItemStack>)> {
    let Some(got) = got else { return Vec::new() };
    got.changed
        .iter()
        .filter(|(slot, stack)| {
            // A slot the client did not mention at all has not been guessed at,
            // so it is told rather than trusted.
            reported
                .iter()
                .find(|(s, _)| *s as usize == *slot)
                .is_none_or(|(_, claimed)| claimed != stack)
        })
        .copied()
        .collect()
}

/// A stack in words, for the log.
fn describe(stack: Option<ItemStack>) -> String {
    stack
        .map(|s| {
            format!(
                "{} x{}",
                crate::generated::item_name(s.item as u32).unwrap_or("?"),
                s.count
            )
        })
        .unwrap_or_else(|| "nothing".into())
}

/// Whether a chunk still needs going into the batch being built.
///
/// Three things count as already handled: on the wire, taken by this batch
/// earlier in the pass, or taken by this batch's tail wait. Missing the last
/// one is what let the tail wait collect chunks the first loop had already
/// collected, so the same ground went out twice and the batch counted more
/// chunks than it was ever asked for.
fn still_wanted(
    pos: crate::world::pos::ChunkPos,
    sent: &std::collections::HashSet<(i32, i32)>,
    taken: &std::collections::HashSet<crate::world::pos::ChunkPos>,
) -> bool {
    !sent.contains(&(pos.x, pos.z)) && !taken.contains(&pos)
}

/// What the chunk-streaming branch of the loop reports, and when.
///
/// The decision to write a line at all is the thing worth isolating, not the
/// writing. A chunk the pool has not finished stays in `arriving` for as long
/// as the pool is working on it, so a loop that logs whenever `arriving` is
/// non-empty says the same thing on every pass — which is what the console
/// looked like: the same line, hundreds of times, within a tenth of a second,
/// each claiming seventeen chunks had arrived when the batch contained none.
///
/// Two rules follow, and they are the same rule. Say nothing unless something
/// happened, and say how many actually went rather than how many were wanted.
fn chunk_report(wanted: usize, sent_now: usize, dropped: usize) -> Option<String> {
    if sent_now == 0 && dropped == 0 {
        return None;
    }
    debug_assert!(
        sent_now <= wanted,
        "cannot send more than was wanted: {sent_now} of {wanted}"
    );
    Some(format!(
        "{sent_now} chunks arrived and {dropped} were let go"
    ))
}

/// How often to send a keep alive.
///
/// Fifteen seconds against a thirty second client timeout, so one can be lost
/// without the connection going.
const KEEP_ALIVE_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// How often the clock is sent again. Once on arrival, the client runs its own
/// count from there on, and the beat here is the game's own beat: every twenty
/// ticks the server's age goes out, `timeSync` in `MinecraftServer.tickChildren`.
/// The count is what the client extrapolates its clock by, so keeping it true
/// is what keeps the sky steady; the clock itself is only ever named in the
/// full sync at join, and resending it is what makes the sun step.
const CLOCK_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// What the keep alive exchange wants done.
///
/// Sent is not the same as answered, and the difference is the whole of it. A
/// server that only sends challenges has no way to tell a player who is there
/// from one whose socket is still half open because their network went away and
/// nothing followed: no FIN, no RST, nothing. That connection then holds an
/// entity id, a name and a place in the player list until the operating system's
/// own keepalive gives up, which on Windows is measured in hours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAliveTick {
    /// Nothing due yet.
    Silent,
    /// Ask, with this id.
    Send(i64),
    /// A challenge went unanswered for long enough. The player is gone.
    Timeout,
}

/// The keep alive exchange, as the game runs it.
///
/// ```java
/// if (now - this.keepAliveTime >= 15000L) {
///    if (this.keepAlivePending) { this.disconnect(TIMEOUT_DISCONNECTION_MESSAGE); }
///    else { this.keepAlivePending = true; this.send(new ClientboundKeepAlivePacket(now)); }
/// }
/// ```
///
/// so one unanswered challenge is enough, and it gets exactly one interval:
/// `keepAliveTime` is stamped when the challenge goes out, never when an answer
/// arrives. A client that stops answering is therefore written off between one
/// and two intervals after it stopped, depending on where in the cycle it fell.
///
/// The challenge here is a counter rather than the timestamp the game uses. Both
/// are opaque to the client, which only echoes it back, and a counter does not
/// depend on the clock agreeing with itself.
///
/// The clock is the caller's. `Instant` inside would make this untestable
/// without sleeping, and a fifteen second rule tested by waiting fifteen seconds
/// is a test nobody runs.
#[derive(Debug, Default)]
pub struct KeepAlive {
    /// The id outstanding, if one is.
    challenge: Option<i64>,
    next_id: i64,
    latency_ms: i32,
}

impl KeepAlive {
    pub fn new() -> Self {
        KeepAlive {
            challenge: None,
            next_id: 1,
            latency_ms: 0,
        }
    }

    /// What is due, if anything, given how long it has been since the last time
    /// anything was asked.
    pub fn tick(
        &mut self,
        since_last: std::time::Duration,
        every: std::time::Duration,
    ) -> KeepAliveTick {
        if since_last < every {
            return KeepAliveTick::Silent;
        }
        if self.challenge.is_some() {
            return KeepAliveTick::Timeout;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.challenge = Some(id);
        KeepAliveTick::Send(id)
    }

    /// The answer to a challenge, or a connection that is not telling the truth.
    ///
    /// ```java
    /// if (this.keepAlivePending && packet.getId() == this.keepAliveChallenge) { ...; keepAlivePending = false; }
    /// else { this.disconnect(TIMEOUT_DISCONNECTION_MESSAGE); }
    /// ```
    ///
    /// So an answer with nothing outstanding is refused, not ignored: a client
    /// replying to a challenge that was never sent is either confused or not the
    /// client it claims to be, and neither is worth carrying.
    pub fn answered(&mut self, id: i64, took: std::time::Duration) -> bool {
        if self.challenge == Some(id) {
            self.challenge = None;
            let took = took.as_millis().min(i32::MAX as u128) as i32;
            self.latency_ms = (self.latency_ms * 3 + took) / 4;
            true
        } else {
            false
        }
    }

    /// The challenge outstanding, for the caller to time the reply against.
    pub fn outstanding(&self) -> Option<i64> {
        self.challenge
    }

    /// The round trip, smoothed the way the game smooths it.
    ///
    /// Three parts in four of the old reading to one of the new, so a single
    /// slow packet does not move the number much.
    pub fn latency_ms(&self) -> i32 {
        self.latency_ms
    }
}

/// How long one loop iteration waits before looking again.
///
/// The game's tick, taken from the world's own definition of one rather than
/// written out again: twenty a second is not a choice about this loop, it is
/// what the tick is, and a second number that happened to agree would be a
/// second place for the two to drift apart.
///
/// The read returns as soon as there is anything to read, so this is only how
/// long an idle connection sleeps. It bounds how quickly a player walking into
/// new terrain is noticed, which is what it is for.
const LOOP_INTERVAL: std::time::Duration = ticking::TICK;

/// How long a connection will wait for the last chunk of an edge to finish being
/// built, rather than sending what it has and waiting for the next pass.
///
/// Short on purpose, and it is a quarter of the loop rather than half: the
/// point is to be done before the next pass would have come round, not to
/// replace it. It runs on the connection's own thread, which also has to answer
/// keep alives and drain movement, so a chunk that genuinely needs work to build
/// gets the next pass, which is what it got before.
const CHUNK_TAIL_BUDGET: std::time::Duration =
    std::time::Duration::from_millis(LOOP_INTERVAL.as_millis() as u64 / 4);

/// How long between looks at the pool while that budget is being spent.
///
/// A tenth of the budget, so a budget is spent in about ten looks rather than a
/// number of them that has to be kept in step with it by hand.
const CHUNK_TAIL_STEP: std::time::Duration =
    std::time::Duration::from_millis(CHUNK_TAIL_BUDGET.as_millis() as u64 / 8);

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
fn break_time(
    block: crate::world::block::BlockId,
    terrain: &crate::world::terrain::TerrainBlocks,
    registry: Option<&crate::world::block::BlockRegistry>,
) -> std::time::Duration {
    // Map the block to its minecraft name, the same way the chunk encoder
    // does, so loaded blocks get their real hardness rather than the
    // terrain-only fallback.
    let name = if block == terrain.air {
        "minecraft:air"
    } else if block == terrain.grass {
        "minecraft:grass_block"
    } else if block == terrain.dirt {
        "minecraft:dirt"
    } else if block == terrain.stone {
        "minecraft:stone"
    } else if block == terrain.glowstone {
        "minecraft:glowstone"
    } else if let Some(reg) = registry {
        // leak-free: build an owned string is not possible for hardness_of
        // (it takes &str). Look the hardness up directly instead.
        let n = reg.name(block);
        // hardness_of needs a &str living long enough; the generated table
        // holds static strings, so compare against the table here.
        let full = if n.contains(':') {
            n.to_string()
        } else {
            format!("minecraft:{}", n)
        };
        // Fall through with a sentinel; handled below.
        return duration_for(&full);
    } else {
        "minecraft:air"
    };

    duration_for(name)
}

fn duration_for(name: &str) -> std::time::Duration {
    let hardness = crate::generated::hardness_of(name)
        .filter(|h| *h > 0.0)
        .unwrap_or(FALLBACK_HARDNESS);
    let requires_tool = crate::generated::requires_tool(name);
    // Bare hand: speed 1.0. can-harvest blocks use the 30 denominator
    // (hardness * 1.5s); tool-gated blocks use the 100 denominator
    // (hardness * 5s).
    let seconds = if requires_tool {
        f64::from(hardness) * 5.0
    } else {
        f64::from(hardness) * SECONDS_PER_HARDNESS
    };
    std::time::Duration::from_secs_f64(seconds)
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
/// How far a player can reach, from the eye, to the nearest point of the block.
///
/// The game measures this as the distance from the eye to the *box* of the
/// block, not to its middle, and allows `block_interaction_range + 1.0`, which
/// is 5.5 with the attribute the game ships: Attributes.BLOCK_INTERACTION_RANGE
/// is 4.5 in the client jar, ranged to 64.
///
/// This used to measure from the middle of the player's feet to the middle of
/// the block, against a limit of 6.0 that appears to have been chosen. Both
/// halves of that are wrong in the same direction and the two mistakes very
/// nearly cancel, which is why nobody noticed -- a player could reach a little
/// too little with one and a little too much with the other.
///
/// Measured the game's way, so a block the client lets them click is a block
/// this accepts. That matters more than the exact number: a break refused that
/// the client thought was allowed is a block that does not break while the
/// crack is drawn on it.
/// Squared distance between two points, for the too-fast move check.
fn dist_squared(ax: f64, ay: f64, az: f64, bx: f64, by: f64, bz: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let dz = bz - az;
    dx * dx + dy * dy + dz * dz
}

/// A UUID as the status sample spells it: thirty-two hex digits, no dashes.
fn uuid_undashed(bytes: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn reach_to(player: &crate::protocol::play::Movement, block: crate::world::pos::BlockPos) -> f64 {
    // The eye sits above the feet. The client adds its own eye height to the
    // position it was sent, and this server sends the feet, so the height has
    // to be put back on here to measure what the client measured.
    const EYE_HEIGHT: f64 = 1.62;

    let eye = (player.x, player.y + EYE_HEIGHT, player.z);

    // The box of the block, which is a whole block rather than a point at its
    // middle. The distance from a point to a box is the gap on each axis: none
    // at all on an axis the point is inside, and how far past the edge it is on
    // one it is outside.
    let across = |p: f64, edge: f64| {
        let gap = p - edge;
        if gap < 0.0 {
            -gap
        } else if gap > 1.0 {
            gap - 1.0
        } else {
            0.0
        }
    };
    let (dx, dy, dz) = (
        across(eye.0, block.x as f64),
        across(eye.1, block.y as f64),
        across(eye.2, block.z as f64),
    );
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// The reach the game allows: `block_interaction_range` of 4.5 plus the one block
/// of slack the client adds, so 5.5.
const MAX_REACH: f64 = 5.5;

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
                //
                // The payload is a fixed eight-byte long in both directions
                // (`ServerboundPingRequestPacket` reads a long, the pong writes
                // one), not a VarLong: reading it as one leaves seven bytes
                // behind in the frame, the pong answers with the wrong number,
                // and the client sits on "Pinging..." forever because the
                // round trip never matches.
                1 => {
                    let mut time = [0u8; 8];
                    if r.len() < 8 {
                        continue;
                    }
                    time.copy_from_slice(&r[..8]);
                    let mut body = Vec::new();
                    VarInt(1).write(&mut body);
                    body.extend_from_slice(&time);

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
mod tests;
