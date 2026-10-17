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
                    let status = self.status.clone();
                    // Detached on purpose: a server's whole job is to handle
                    // many people at once, so joining these would serialise the
                    // entire server on whichever connection finished first.
                    thread::spawn(move || {
                        let _ = handle_connection(stream, status);
                    });
                }
                Err(e) => {
                    eprintln!("[filament] accept failed: {e}");
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
            Ok(Some(body)) => break match ClientIntention::read(&body) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[filament] bad handshake: {e}");
                    return Ok(());
                }
            },
            Ok(None) => continue,
            Err(e) => {
                eprintln!("[filament] framing error during handshake: {e}");
                return Ok(());
            }
        }
    };

    println!(
        "[filament] {} connected: protocol {}, host {:?}, port {}, intent {:?}",
        stream.peer_addr().map(|a| a.to_string()).unwrap_or_default(),
        intention.protocol_version.0,
        intention.host,
        intention.port,
        intention.intent
    );

    match intention.intent {
        ClientIntent::Status => serve_status(&mut stream, dec, enc, status),
        ClientIntent::Login => {
            // Login needs the compression switch and a whole configuration
            // phase before it can be answered. Saying so plainly beats
            // hanging up, which would look like a network problem.
            eprintln!("[filament] login is not implemented yet");
            Ok(())
        }
        ClientIntent::Transfer => Ok(()),
    }
}

/// The status exchange: a request, a response, then an optional ping.
fn serve_status(
    stream: &mut TcpStream,
    mut dec: FrameDecoder,
    enc: FrameEncoder,
    status: ServerStatus,
) -> std::io::Result<()> {
    let mut scratch = [0u8; 8192];

    loop {
        let n = stream.read(&mut scratch)?;
        if n == 0 {
            return Ok(());
        }
        dec.queue(&scratch[..n]);

        while let Ok(Some(body)) = dec.next_frame() {
            let mut r = &body[..];
            let (VarInt(id), _) = match VarInt::read(&mut r) {
                Ok(v) => v,
                Err(_) => continue,
            };

            match id {
                // Status request: answer with the JSON.
                0 => {
                    let out = status_response_body(&status);
                    let mut wire = Vec::new();
                    enc.encode(&out, &mut wire);
                    stream.write_all(&wire)?;
                    stream.flush()?;
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
                    enc.encode(&body, &mut wire);
                    stream.write_all(&wire)?;
                    stream.flush()?;
                }
                _ => {}
            }
        }
    }
}