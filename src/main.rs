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

const USAGE: &str = "\
filament, a Minecraft server

usage:
  filament [--verbose] [--addr HOST:PORT]

  --verbose, -v   log every packet, in and out
                  off by default: a client waiting on chunks sends an empty
                  packet twenty times a second, which buries everything else
  --addr ADDR     what to listen on, default 127.0.0.1:25565

Loopback by default on purpose. A server that is not finished should not be
reachable from the network by accident.";

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

    log::set_verbose(verbose);

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
    log::note("status, login, configuration and the start of play are implemented");
    log::note("chunks are sent, and they are empty: the world has no terrain in it yet");
    if verbose {
        log::note("verbose: every packet will be logged");
    }

    server.serve_forever();
    Ok(())
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
mod tests {
    use super::*;

    /// Parses a list rather than the real command line, so the tests do not have
    /// to fight the process environment.
    fn parse(args: &[&str]) -> Result<(SocketAddr, bool), String> {
        let mut verbose = false;
        let mut addr: Option<SocketAddr> = None;

        let mut it = args.iter().map(|s| s.to_string());
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--verbose" | "-v" => verbose = true,
                "--addr" => {
                    let value = it.next().ok_or("--addr needs an address")?;
                    addr = Some(value.parse().map_err(|_| "bad address")?);
                }
                other if other.starts_with('-') => return Err(format!("unknown flag {other}")),
                other => addr = Some(other.parse().map_err(|_| "bad address")?),
            }
        }

        Ok((
            addr.unwrap_or_else(|| DEFAULT_ADDR.parse().expect("the default parses")),
            verbose,
        ))
    }

    #[test]
    fn no_arguments_gives_the_default_address_and_no_packets() {
        let (addr, verbose) = parse(&[]).expect("parses");
        assert_eq!(addr.to_string(), DEFAULT_ADDR);
        assert!(!verbose, "packet logging is opt in");
    }

    #[test]
    fn the_verbose_flag_turns_packet_logging_on() {
        let (_, verbose) = parse(&["--verbose"]).expect("parses");
        assert!(verbose);

        let (_, short) = parse(&["-v"]).expect("parses");
        assert!(short, "-v is the same flag");
    }

    #[test]
    fn the_address_can_be_given_either_way() {
        let (flag, _) = parse(&["--addr", "0.0.0.0:25566"]).expect("parses");
        assert_eq!(flag.to_string(), "0.0.0.0:25566");

        // The original invocation, which still works.
        let (bare, _) = parse(&["0.0.0.0:25566"]).expect("parses");
        assert_eq!(bare.to_string(), "0.0.0.0:25566");
    }

    #[test]
    fn flags_and_the_address_do_not_have_to_be_in_order() {
        let (addr, verbose) = parse(&["-v", "--addr", "0.0.0.0:25566"]).expect("parses");
        assert!(verbose);
        assert_eq!(addr.to_string(), "0.0.0.0:25566");

        let (addr, verbose) = parse(&["--addr", "0.0.0.0:25566", "-v"]).expect("parses");
        assert!(verbose);
        assert_eq!(addr.to_string(), "0.0.0.0:25566");
    }

    #[test]
    fn an_unknown_flag_is_an_error_rather_than_an_address() {
        let err = parse(&["--loud"]).expect_err("should refuse");
        assert!(err.contains("--loud"), "{err}");
    }

    #[test]
    fn a_bad_address_is_an_error() {
        assert!(parse(&["--addr", "not-an-address"]).is_err());
        assert!(parse(&["nonsense"]).is_err());
    }

    #[test]
    fn addr_without_a_value_is_an_error() {
        assert!(parse(&["--addr"]).is_err(), "the value is missing");
    }
}