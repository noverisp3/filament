//! The status phase: the answer the multiplayer browser reads.
//!
//! The client sends a request and gets the server's description back, and may
//! then send a ping to measure the round trip. No compression, no player.

use std::io::{Read, Write};
use std::net::TcpStream;

use crate::protocol::frame::{FrameDecoder, FrameEncoder};
use crate::protocol::status::{status_response_body, ServerStatus};
use crate::protocol::VarInt;

use super::log::packet_name;
use super::log::ConnLog;
use super::io_err;

/// The status exchange: a request, a response, then an optional ping.
pub(super) fn serve_status(
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
