//! The TCP listener and the handshake and status phases.
//!
//! One thread per connection. That is the simple choice and it is the right one
//! here: these phases do almost no work, so a thread spends nearly all its life
//! blocked on a read. The parallelism that matters for this server is chunk
//! generation and lighting, which is CPU-bound and gets its own pool.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;

use crate::protocol::frame::{FrameDecoder, FrameEncoder};
use crate::protocol::handshake::{ClientIntent, ClientIntention, ServerStatus, status_response_body};
use crate::protocol::{VarInt, VarLong};

use super::log::{next_connection_id, packet_name, ConnLog};
use super::log;

/// The protocol version this server speaks, from the data generator.
pub const PROTOCOL_VERSION: i32 = 777;

/// A running server.
pub struct StatusServer {
    listener: TcpListener,
    status: ServerStatus,
}

impl StatusServer {
    /// Binds `addr` and prepares to answer handshakes.
    pub fn bind(addr: &str, status: ServerStatus) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        Ok(StatusServer { listener, status })
    }

    /// The address actually bound, which is useful when port 0 was asked for.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts connections until the process ends, each on its own thread.
    pub fn serve_forever(self) {
        for stream in self.listener.incoming() {
            match stream {
                Ok(stream) => {
                    let peer = stream
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|_| "unknown".to_string());
                    let id = next_connection_id();
                    let status = self.status.clone();

                    log::note(format!("[conn {id}] accepted from {peer}"));

                    // Detached on purpose: a server's whole job is to handle
                    // many people at once, so joining these would serialise the
                    // entire server on whichever connection finished first.
                    thread::spawn(move || {
                        if let Err(e) = handle_connection(stream, status) {
                            log::error(format!("[conn {id}] ended with an error: {e}"));
                        }
                    });
                }
                Err(e) => {
                    log::error(format!("accept failed: {e}"));
                }
            }
        }
    }
}

/// Answers one connection's handshake and status exchange.
///
/// Deliberately does not implement login: the point here is to be visible in
/// the multiplayer browser, which the client reaches only after status.
fn handle_connection(mut stream: TcpStream, status: ServerStatus) -> std::io::Result<()> {
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
        ClientIntent::Login => login(&mut stream, log),
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
fn login(stream: &mut TcpStream, log: ConnLog) -> std::io::Result<()> {
    use crate::protocol::frame::DEFAULT_COMPRESSION_THRESHOLD;
    use crate::protocol::login::{
        read_hello, write_login_compression, write_login_finished, Uuid,
    };

    let log = log.phase("login");
    let mut dec = FrameDecoder::new();
    let mut enc = FrameEncoder::new();
    let mut scratch = [0u8; 8192];

    // The client names itself.
    let name = 'hello: {
        loop {
            let n = stream.read(&mut scratch)?;
            if n == 0 {
                log.note("closed before the login hello arrived");
                return Ok(());
            }
            dec.queue(&scratch[..n]);

            // One frame is expected here, but `while let` is wrong for that:
            // breaking out of it is a break with no condition, which reads like
            // a loop that cannot terminate and is flagged as one. Only the first
            // frame is wanted, so the rest is drained below.
            if let Some(frame) = dec.next_frame().map_err(io_err)? {
                let id = packet_id_of(&frame);
                log.in_packet(&packet_name("login-s2c", id), "login", frame.len());

                match read_hello(&frame) {
                    Ok((name, _)) => break 'hello name,
                    Err(e) => {
                        log.error(format!("could not read the login hello: {e}"));
                        return Ok(());
                    }
                }
            }
        }
    };

    log.note(format!(
        "player {name:?} from {}, sending the profile",
        stream.peer_addr().map(|a| a.to_string()).unwrap_or_default()
    ));

    let profile_id = Uuid::offline_for(&name);
    let session_id = Uuid::offline_for("filament-session");

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
    loop {
        let n = stream.read(&mut scratch)?;
        if n == 0 {
            log.note("closed before acknowledging");
            return Ok(());
        }
        dec.queue(&scratch[..n]);

        if let Some(frame) = dec.next_frame().map_err(io_err)? {
            let id = packet_id_of(&frame);
            log.in_packet(
                &packet_name("login-s2c", id),
                "login",
                frame.len(),
            );

            if id == crate::protocol::login::id::SERVERBOUND_LOGIN_ACKNOWLEDGED {
                log.note("acknowledged; the client is now waiting for configuration");
                log.note("configuration is not implemented, closing the connection");
                return Ok(());
            }
        }
    }
}

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

    loop {
        let n = stream.read(&mut scratch)?;
        if n == 0 {
            return Ok(());
        }
        dec.queue(&scratch[..n]);

        while let Some(body) = dec.next_frame().map_err(io_err)? {
            let mut r = &body[..];
            let (VarInt(id), _) = match VarInt::read(&mut r) {
                Ok(v) => v,
                Err(_) => continue,
            };

            log.in_packet(&packet_name("status", id), "status", body.len());

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
    }
}