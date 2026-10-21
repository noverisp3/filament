//! filament
//!
//! A Minecraft server. Run it and it will show up in the multiplayer browser.

pub mod generated;
pub mod protocol;
pub mod render;
pub mod server;
pub mod world;

use std::net::{IpAddr, SocketAddr};

use server::Server;
use server::log;

/// Where to listen unless told otherwise.
///
/// Every interface, not loopback: binding to loopback is the kind of decision
/// that is invisible until somebody asks why the friend on the same wifi cannot
/// connect. The startup log prints the address either way, so the cost of
/// being visible by default is that the address gets read by whoever starts the
/// server.
///
/// Pass `--addr 127.0.0.1:25565` for the old behaviour, and that is the only
/// thing the flag is for now.
const DEFAULT_ADDR: &str = "0.0.0.0:25565";

/// Shown in the multiplayer browser.
const DEFAULT_DESCRIPTION: &str = "filament";

const USAGE: &str = "\
filament, a Minecraft server

usage:
  filament [--verbose] [--addr HOST:PORT]

  --verbose, -v   log every packet, in and out
                  off by default: a client waiting on chunks sends an empty
                  packet twenty times a second, which buries everything else
  --addr ADDR     what to listen on, default 0.0.0.0:25565

Every interface by default, so anyone on the network can join. There is no
sign-in: login is offline and the UUID comes from the name the client sends,
so anyone who reaches the port can take any name, including one that is
already in use. Use --addr 127.0.0.1:25565 to keep it to this machine.";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parsing first and printing here rather than returning the error, because a
    // `Box<dyn Error>` carrying a String comes back out through `Debug`, which
    // escapes the newlines and turns a usage message into one unreadable line.
    let (addr, verbose) = match parse_args() {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    log::set_verbose(verbose);
    server::shutdown::install();

    let status = protocol::status::status(
        DEFAULT_DESCRIPTION,
        format!("filament {}", server::PROTOCOL_VERSION),
        server::PROTOCOL_VERSION,
        0,
        20,
    );

    let server = Server::bind(&addr.to_string(), status)?;

    print_build();
    log::note(format!("listening on {addr}"));
    announce_reachability(&addr);
    log::note(format!("protocol {}", server::PROTOCOL_VERSION));
    log::note("max players 20");
    log::note("status, login, configuration and the start of play are implemented");
    log::note(
        "chunks come from the world folder `FILAMENT_WORLD` names, or the generator; edits survive a restart",
    );
    if verbose {
        log::note("verbose: every packet will be logged");
    }

    server.serve_forever();
    Ok(())
}

/// Says who can reach this, when anyone other than this machine can.
///
/// Binding to `0.0.0.0` is the switch, and the switch has a cost worth
/// stating out loud rather than leaving to be found out about: there is no
/// authentication, so anyone who reaches the port can join as any name they
/// like, and the offline UUIDs are derived from the name rather than from an
/// account.
///
/// The address printed is the one to hand out. Finding it is not something to
/// ask the user to do, because the answer is in the machine and not in any
/// config file.
fn announce_reachability(addr: &SocketAddr) {
    let wildcard = match addr.ip() {
        IpAddr::V4(ip) => ip.is_unspecified(),
        IpAddr::V6(ip) => ip.is_unspecified(),
    };

    if !wildcard {
        log::note("only this machine can reach that; pass --addr 0.0.0.0:25565 to share it");
        return;
    }

    match local_network_address() {
        Some(ip) => log::note(format!(
            "reachable on this network at {ip}:{}",
            addr.port(),
        )),
        None => log::note("reachable on every address this machine has"),
    }

    log::note("anyone who can reach the port can join as anyone: there is no sign-in yet");
}

/// The local address a packet to the outside world would leave from.
///
/// Binding a UDP socket and connecting it picks a route without sending
/// anything: a connected UDP socket is given a destination and a source address
/// by the kernel, and stays silent until something is written to it. Nothing
/// here touches the network.
///
/// It is a heuristic and says so by having a way to fail: a machine with no
/// route out, or no interface, answers with nothing and the caller prints a
/// sentence that does not need an address to be true.
fn local_network_address() -> Option<IpAddr> {
    // Port zero, so this cannot collide with anything, and a destination that
    // is only ever used to ask the routing question.
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("192.0.2.1:9").ok()?; // TEST-NET-1, never routed to anything

    let local = probe.local_addr().ok()?;
    if local.ip().is_unspecified() {
        None
    } else {
        Some(local.ip())
    }
}

/// Prints which build this is.
///
/// A server log that cannot say what produced it is a log you have to guess
/// about, and "it misbehaved" only becomes actionable once you know the binary
/// matches the repository.
///
/// Everything is labelled, because this originally put two hashes on two lines
/// with nothing to say which was which: the project's and the compiler's. They
/// look the same and they are not the same thing, so the compiler's is named
/// rather than left to be worked out.
fn print_build() {
    let sha = env!("FILAMENT_GIT_SHA");
    let dirty = env!("FILAMENT_GIT_DIRTY") == "true";
    // A dirty tree is said plainly rather than hidden: the binary and the
    // repository are then not the same thing, which is worth knowing before
    // chasing a bug in either.
    let commit = if dirty { format!("{sha}+dirty") } else { sha.to_string() };

    log::note(format!(
        "filament {} | {} | {}",
        env!("CARGO_PKG_VERSION"),
        env!("FILAMENT_PROFILE"),
        env!("FILAMENT_TARGET"),
    ));
    log::note(format!("commit    {commit}"));
    log::note(format!(
        "built     {} by {}",
        env!("FILAMENT_BUILT_AT"),
        env!("FILAMENT_RUSTC"),
    ));
}

/// Reads the command line, returning the address and whether to log packets.
///
/// Flags and the address are order independent, and a bare address still works,
/// because that was the original invocation and there is no reason to break it.
fn parse_args() -> Result<(SocketAddr, bool), String> {
    let mut verbose = false;
    let mut addr: Option<SocketAddr> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--verbose" | "-v" => verbose = true,
            "--addr" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("--addr needs an address, as in --addr 0.0.0.0:25565\n\n{USAGE}"))?;
                addr = Some(parse_addr(&value)?);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag {other}\n\n{USAGE}"));
            }
            other => addr = Some(parse_addr(other)?),
        }
    }

    Ok((addr.unwrap_or_else(default_addr), verbose))
}

/// The address used when none is given.
fn default_addr() -> SocketAddr {
    DEFAULT_ADDR.parse().expect("the default address is valid")
}

/// Parses an address, turning the failure into something a person can act on.
///
/// `SocketAddr`'s own error names the type it wanted, which is not helpful when
/// the mistake was a typo.
fn parse_addr(value: &str) -> Result<SocketAddr, String> {
    value.parse().map_err(|_| {
        format!("{value:?} is not an address\n\nit should look like 127.0.0.1:25565\n\n{USAGE}")
    })
}

#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;