//! The login phase: the client names itself, and the server answers with a profile.
//!
//! The phase ends when the client acknowledges the profile, and the decoder it
//! leaves behind is handed to the configuration phase rather than replaced.

use std::io::{Read, Write};
use std::net::TcpStream;

use crate::protocol::frame::{FrameDecoder, FrameEncoder};

use crate::server::broadcast::Session;
use super::configuration::configure;
use super::log::packet_name;
use super::log::ConnLog;
use super::{io_err, packet_id_of};

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
pub(super) fn login(
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

    // The seat this player holds is the registration's, and the registration is
    // dropped however the connection ends: a client that cancels while the
    // world is loading never reaches the play phase, and the count of players
    // has to come back down all the same.

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
