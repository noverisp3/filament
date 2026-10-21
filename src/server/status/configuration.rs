//! The configuration phase: the registries, the tags, and the client's settings.
//!
//! The phase ends when the client says it is done, and the view distance it
//! asks for here is what decides how much world the play phase builds.

use std::io::{Read, Write};
use std::net::TcpStream;

use crate::protocol::frame::{FrameDecoder, FrameEncoder, FrameError};

use crate::server::broadcast::{Registration, Session};
use super::log::packet_name;
use super::log::ConnLog;
use super::play::play;
use super::{io_err, packet_id_of};

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
pub(super) fn configure(
    stream: &mut TcpStream,
    log: ConnLog,
    mut dec: FrameDecoder,
    session: Session,
    registration: Registration,
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
    //    `FeatureFlags.java:31-42` builds the game's own set, and
    //    `createVanilla` is `Identifier.withDefaultNamespace(name)`, so the one
    //    that ships is `minecraft:vanilla`.
    //
    //    An empty list is not "no requirement", it is "nothing is enabled",
    //    and every item whose prototype asks for a flag then reads as disabled.
    let mut body = Vec::new();
    write_update_enabled_features(&mut body, &["minecraft:vanilla"]);
    enc.encode(&body, &mut out).map_err(io_err)?;
    log.out_packet("update_enabled_features", "configuration", body.len());

    // 2. Registry data. These look optional and are not: with none of them the
    //    client falls back to the registries built into its jar, and that set is
    //    not the one the tags refer to.
    //
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
                    return play(
                        stream,
                        dec,
                        enc,
                        log,
                        view_distance,
                        session,
                        registration,
                    );
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
