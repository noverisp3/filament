//! The TCP listener, and the phases a connection goes through.
//!
//! One thread per connection. That is the simple choice and it is the right one
//! here: these phases do almost no work, so a thread spends nearly all its life
//! blocked on a read. The parallelism that matters for this server is chunk
//! generation and lighting, which is CPU-bound and gets its own pool.

use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use crate::protocol::frame::{FrameDecoder, FrameEncoder};
use crate::protocol::handshake::{ClientIntent, ClientIntention};
use crate::protocol::status::ServerStatus;
use crate::protocol::VarInt;

use super::broadcast::{Broadcast, Session};
use super::log;
use super::log::{next_connection_id, packet_name, ConnLog};
use super::ticking::{self, WorldHandle};

mod configuration;
mod login;
mod play;
mod server_list;

#[cfg(test)]
mod tests;

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
                    // On Windows an accepted socket inherits the listener's
                    // non-blocking flag. Left alone, every read on the
                    // connection returns "would block" immediately and a client
                    // that says anything at all is dropped with error 10035.
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

        // Waiting here, behind the connections being cut, is what makes the save
        // real rather than raced: shutdown is a command whose body is the write,
        // answered only once the files are closed.
        let ticks = self.world.shutdown();
        if std::env::var_os("FILAMENT_WORLD").is_some() {
            log::note(format!("the world saved, {} ticks run", ticks));
        } else {
            log::note(format!(
                "nothing saved -- no world folder was opened (FILAMENT_WORLD); {} ticks run",
                ticks
            ));
        }

        // The connections are not waited for. There is no way yet to tell one to
        // finish, so joining here would hang for as long as anybody stayed
        // connected, which is a worse Ctrl+C than the one being fixed.
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

/// Answers one connection's handshake and hands it to the phase it asked for.
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
        ClientIntent::Status => server_list::serve_status(&mut stream, dec, enc, status, log),
        ClientIntent::Login => login::login(&mut stream, log, dec, session),
        ClientIntent::Transfer => {
            log.note("transfer is not implemented");
            Ok(())
        }
    }
}

/// A UUID as the status sample spells it: thirty-two hex digits, no dashes.
fn uuid_undashed(bytes: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
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
