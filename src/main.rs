//! filament
//!
//! A Minecraft server. Run it and it will show up in the multiplayer browser.

pub mod generated;
pub mod protocol;
pub mod render;
pub mod server;
pub mod world;

use std::net::SocketAddr;

use server::StatusServer;
use server::log;

/// Where to listen unless told otherwise.
const DEFAULT_ADDR: &str = "127.0.0.1:25565";

/// Shown in the multiplayer browser.
const DEFAULT_DESCRIPTION: &str = "filament";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Loopback rather than every interface: a server that is not finished
    // should not be reachable from the network by accident. Both the address and
    // the port can be given on the command line.
    let addr: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_ADDR.to_string())
        .parse()?;

    let status = protocol::handshake::status(
        DEFAULT_DESCRIPTION,
        format!("filament {}", server::PROTOCOL_VERSION),
        server::PROTOCOL_VERSION,
        0,
        20,
    );

    let server = StatusServer::bind(&addr.to_string(), status)?;

    log::note(format!("filament listening on {addr}"));
    log::note(format!("protocol {}", server::PROTOCOL_VERSION));
    log::note("max players 20");
    log::note("status, login and configuration are implemented; the world is not yet");

    server.serve_forever();
    Ok(())
}