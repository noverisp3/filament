//! Packet-level logging.
//!
//! Every frame in and out is recorded with its connection id, direction and
//! packet id. This exists because the failure mode of a protocol
//! implementation is invisible: a misread field still parses, it just parses the
//! wrong thing, and the only evidence is where the client gives up.
//!
//! A Minecraft client that fails mid-handshake says one of a handful of fixed
//! messages. Knowing which packet was the last one to arrive narrows that down
//! to exactly one place in this server.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Monotonic per-process connection counter.
///
/// An `AtomicU64` rather than a random id: what matters is telling two
/// connections apart in the log, and a counter makes that readable.
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_connection_id() -> u64 {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

/// A handle used to log one connection's traffic.
#[derive(Debug, Clone)]
pub struct ConnLog {
    id: u64,
    peer: String,
    phase: &'static str,
}

impl ConnLog {
    pub fn new(id: u64, peer: String, phase: &'static str) -> Self {
        ConnLog { id, peer, phase }
    }

    pub fn phase(&self, phase: &'static str) -> Self {
        ConnLog {
            id: self.id,
            peer: self.peer.clone(),
            phase,
        }
    }

    /// The peer's address, kept for the accept log.
    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// Records a packet that arrived.
    ///
    /// `packet` is the name from `packets.json` and `state` the phase it
    /// belongs to, because the same id means different things in different
    /// phases: 3 is `finish_configuration` in configuration and
    /// `login_compression` in login.
    pub fn in_packet(&self, packet: &str, state: &str, len: usize) {
        log_line(&format_args!(
            "[conn {}] <- {} ({}/{}) {} bytes",
            self.id, packet, state, self.phase, len
        ));
    }

    /// Records a packet that was sent.
    pub fn out_packet(&self, packet: &str, state: &str, len: usize) {
        log_line(&format_args!(
            "[conn {}] -> {} ({}/{}) {} bytes",
            self.id, packet, state, self.phase, len
        ));
    }

    pub fn note(&self, msg: impl AsRef<str>) {
        log_line(&format_args!("[conn {}] {}", self.id, msg.as_ref()));
    }

    pub fn error(&self, msg: impl AsRef<str>) {
        log_line(&format_args!("[conn {}] ERROR {}", self.id, msg.as_ref()));
    }
}

/// Prints one line with a millisecond timestamp.
///
/// Goes to stdout rather than a logging framework: there is one stream, it is
/// ordered, and a dependency here would be for formatting that is two lines.
/// The timestamp is epoch milliseconds because nothing else in the codebase
/// wants a clock, and a human reading a log mostly needs ordering, which the
/// order already gives.
fn log_line(args: &std::fmt::Arguments<'_>) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    // stdout is locked for the whole call so that two connections logging at the
    // same moment cannot interleave halfway through a line.
    let out = std::io::stdout();
    let mut lock = out.lock();
    use std::io::Write;
    let _ = writeln!(lock, "{:>13}  {}", millis, args);
}

/// A free-form note that is not tied to a connection.
pub fn note(msg: impl AsRef<str>) {
    log_line(&format_args!("{}", msg.as_ref()));
}

pub fn error(msg: impl AsRef<str>) {
    log_line(&format_args!("ERROR {}", msg.as_ref()));
}

/// The packet name for an id, or a placeholder.
///
/// Kept as a plain match rather than generated: there are under forty ids in
/// the phases that are implemented, and a table here is easier to extend than a
/// build script that has to be re-run when a packet is added.
pub fn packet_name(state: &str, id: i32) -> String {
    match (state, id) {
        ("handshake", 0) => "intention".to_string(),
        ("status", 0) => "status_request".to_string(),
        ("status", 1) => "ping_request".to_string(),
        ("login", 0) => "login_disconnect".to_string(),
        ("login", 1) => "hello".to_string(),
        ("login", 2) => "login_finished".to_string(),
        ("login", 3) => "login_compression".to_string(),
        ("login", 4) => "custom_query".to_string(),
        ("login", 5) => "cookie_request".to_string(),
        ("login-s2c", 0) => "hello".to_string(),
        ("login-s2c", 1) => "key".to_string(),
        ("login-s2c", 2) => "custom_query_answer".to_string(),
        ("login-s2c", 3) => "login_acknowledged".to_string(),
        ("login-s2c", 4) => "cookie_response".to_string(),
        (state, id) => format!("{state}:{id}"),
    }
}

/// Guards against a stream of identical warnings from one bad connection.
///
/// The server reads in a loop, and a client sending the same malformed frame
/// repeatedly would otherwise fill the log faster than it can be read, hiding
/// everything else.
#[derive(Debug)]
pub struct RateLimiter {
    seen: Mutex<Option<(String, u64)>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        RateLimiter { seen: Mutex::new(None) }
    }

    /// Returns true when this message should be printed.
    pub fn should_log(&self, msg: &str) -> bool {
        let mut seen = match self.seen.lock() {
            Ok(g) => g,
            // A poisoned lock means another thread panicked while holding it.
            // Printing is better than propagating that.
            Err(poisoned) => poisoned.into_inner(),
        };

        match &*seen {
            Some((last, count)) if last == msg => {
                let next = count + 1;
                *seen = Some((msg.to_string(), next));
                // Report the first few, then every hundredth.
                next <= 3 || next % 100 == 0
            }
            _ => {
                *seen = Some((msg.to_string(), 1));
                true
            }
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_limiter_repeats_only_the_first_few() {
        let rl = RateLimiter::new();

        assert!(rl.should_log("x"), "first is always printed");
        assert!(rl.should_log("x"), "second is printed");
        assert!(rl.should_log("x"), "third is printed");
        assert!(!rl.should_log("x"), "fourth is suppressed");

        // A different message resets the count.
        assert!(rl.should_log("y"));
        assert!(rl.should_log("x"), "a different message makes this one new again");
    }

    #[test]
    fn the_hundredth_repeat_is_printed() {
        let rl = RateLimiter::new();
        let mut printed = 0;

        for i in 1..=250 {
            if rl.should_log("same") {
                printed += 1;
                if i == 1 {
                    assert_eq!(printed, 1);
                }
            }
        }

        // First three, then 100, 200.
        assert_eq!(printed, 5, "the count should reappear at each hundred");
    }

    #[test]
    fn packet_names_distinguish_the_same_id_across_phases() {
        // 3 is login_compression in login and finish_configuration in
        // configuration. Logging it without the phase would be ambiguous.
        assert_eq!(packet_name("login", 3), "login_compression");
        assert_eq!(packet_name("configuration", 3), "configuration:3");
    }
}