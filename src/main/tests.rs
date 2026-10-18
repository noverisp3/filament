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