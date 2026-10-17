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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// Monotonic per-process connection counter.
///
/// An `AtomicU64` rather than a random id: what matters is telling two
/// connections apart in the log, and a counter makes that readable.
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_connection_id() -> u64 {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

/// Whether every packet is recorded.
///
/// Off by default. A client waiting on chunks sends an empty packet twenty times
/// a second, so packet logging turned on unconditionally buries the handful of
/// lines that say anything in a wall of one-byte records. What is left without
/// it is the lifecycle: connections, phases, and the errors, which are the lines
/// anyone actually reads.
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Turns packet logging on or off.
pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

/// Whether packet logging is on.
pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
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

/// Records a packet that arrived, when packet logging is on.
///
/// `packet` is the name from `packets.json` and `state` the phase it
/// belongs to, because the same id means different things in different
/// phases: 3 is `finish_configuration` in configuration and
/// `login_compression` in login.
pub fn in_packet(&self, packet: &str, state: &str, len: usize) {
    if !verbose() {
        return;
    }
    log_line(&format_args!(
        "[conn {}] <- {} ({}/{}) {} bytes",
        self.id, packet, state, self.phase, len
    ));
}

/// Records a packet that was sent, when packet logging is on.
pub fn out_packet(&self, packet: &str, state: &str, len: usize) {
    if !verbose() {
        return;
    }
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

/// Prints one line with a timestamp.
///
/// Goes to stdout rather than a logging framework: there is one stream, it is
/// ordered, and a dependency here would be for formatting that is two lines.
///
/// The timestamp is how long the server has been up, not the wall clock. Epoch
/// milliseconds is thirteen digits of which a reader wants none: to answer "how
/// long between the chunk batch and the disconnect" the useful number is the
/// difference between two lines, and making that a subtraction of two large
/// numbers is work the log should have done. It also lines up by eye with the
/// gaps between lines, which is what a log is scanned with.
///
/// Width is fixed at eight characters so the messages stay in a column, which
/// matters more than it sounds once there are two thousand of them.
fn log_line(args: &std::fmt::Arguments<'_>) {
    // stdout is locked for the whole call so that two connections logging at the
    // same moment cannot interleave halfway through a line.
    let out = std::io::stdout();
    let mut lock = out.lock();
    use std::io::Write;
    let _ = writeln!(lock, "{}  {}", elapsed(), args);
}

/// When the process started, captured once so every line counts from the same
/// zero.
fn started() -> std::time::Instant {
    static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *STARTED.get_or_init(std::time::Instant::now)
}

/// Time since startup, as `  12.4s`, ` 1:02.3` or `1:02:03`.
///
/// A tenth of a second is the resolution that matters here: everything the server
/// does that anyone watches happens inside a second or two, and the last unit
/// changes to minutes once a run gets long enough that the earlier part of the
/// log is what gets read.
fn elapsed() -> String {
    format_elapsed(started().elapsed())
}

/// Formats a duration the way the log wants it.
///
/// Split out from [`elapsed`] so it can be tested: this is pure formatting, and
/// a clock in a test is a test that only passes sometimes.
///
/// Every branch is padded to the same width at the end rather than aligning the
/// leading number. The branches are genuinely different lengths, and working out
/// per branch which field has to be padded by how much is arithmetic that only
/// pays off in the layout.
fn format_elapsed(d: std::time::Duration) -> String {
    let seconds = d.as_secs();
    let tenths = d.subsec_millis() / 100;

    let text = if seconds < 60 {
        // Under a minute: seconds and a tenth, because that is the range where
        // the gap between two lines is the interesting number.
        format!("{seconds}.{tenths}s")
    } else if seconds < 3600 {
        // Past a minute the tenths go, since whole seconds are what get compared.
        let (m, s) = (seconds / 60, seconds % 60);
        format!("{m}:{s:02}.{tenths}")
    } else {
        // Past an hour there is no room left for a tenth.
        let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
        format!("{h}:{m:02}:{s:02}")
    };

    format!("{text:>WIDTH$}")
}

/// Width of a timestamp column, chosen to fit the longest of them.
const WIDTH: usize = 8;

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
        ("status", 0) => "status_response".to_string(),
        ("status", 1) => "pong_response".to_string(),

        // Serverbound in status. Same ids as above and a different meaning, which
        // is why the state carries the direction.
        ("status-s2c", 0) => "status_request".to_string(),
        ("status-s2c", 1) => "ping_request".to_string(),

        // Serverbound in login. The suffix keeps these apart from the
        // clientbound ids in the same state, which overlap.
        ("login-s2c", 0) => "hello".to_string(),
        ("login-s2c", 1) => "key".to_string(),
        ("login-s2c", 2) => "custom_query_answer".to_string(),
        ("login-s2c", 3) => "login_acknowledged".to_string(),
        ("login-s2c", 4) => "cookie_response".to_string(),

        // Clientbound in login.
        ("login", 0) => "login_disconnect".to_string(),
        ("login", 1) => "hello".to_string(),
        ("login", 2) => "login_finished".to_string(),
        ("login", 3) => "login_compression".to_string(),
        ("login", 4) => "custom_query".to_string(),
        ("login", 5) => "cookie_request".to_string(),

        // Serverbound in configuration.
        ("configuration-s2c", 0) => "client_information".to_string(),
        ("configuration-s2c", 1) => "cookie_response".to_string(),
        ("configuration-s2c", 2) => "custom_payload".to_string(),
        ("configuration-s2c", 3) => "finish_configuration".to_string(),
        ("configuration-s2c", 4) => "keep_alive".to_string(),
        ("configuration-s2c", 5) => "pong".to_string(),
        ("configuration-s2c", 6) => "resource_pack".to_string(),
        ("configuration-s2c", 7) => "select_known_packs".to_string(),

        // Clientbound in configuration.
        ("configuration", 1) => "custom_payload".to_string(),
        ("configuration", 2) => "disconnect".to_string(),
        ("configuration", 3) => "finish_configuration".to_string(),
        ("configuration", 4) => "keep_alive".to_string(),
        ("configuration", 5) => "ping".to_string(),
        ("configuration", 7) => "registry_data".to_string(),
        ("configuration", 13) => "update_enabled_features".to_string(),
        ("configuration", 14) => "update_tags".to_string(),
        ("configuration", 15) => "select_known_packs".to_string(),

        // Play, clientbound. The ids come from packets.json and the ones here
        // are the ones a connection entering the world actually sees, which is
        // what made the capture readable: before this every play packet came
        // out as "play:75".
        ("play", 10) => "change_difficulty".to_string(),
        ("play", 11) => "chunk_batch_finished".to_string(),
        ("play", 12) => "chunk_batch_start".to_string(),
        ("play", 13) => "chunks_biomes".to_string(),
        ("play", 16) => "commands".to_string(),
        ("play", 32) => "disconnect".to_string(),
        ("play", 34) => "entity_event".to_string(),
        ("play", 39) => "game_event".to_string(),
        ("play", 44) => "initialize_border".to_string(),
        ("play", 45) => "keep_alive".to_string(),
        ("play", 46) => "level_chunk_with_light".to_string(),
        ("play", 49) => "light_update".to_string(),
        ("play", 50) => "login".to_string(),
        ("play", 63) => "pong_response".to_string(),
        ("play", 65) => "player_abilities".to_string(),
        ("play", 71) => "player_info_update".to_string(),
        ("play", 72) => "player_look_at".to_string(),
        ("play", 73) => "player_position".to_string(),
        ("play", 75) => "recipe_book_add".to_string(),
        ("play", 77) => "recipe_book_settings".to_string(),
        ("play", 83) => "post_effects".to_string(),
        ("play", 88) => "server_data".to_string(),
        ("play", 96) => "set_chunk_cache_center".to_string(),
        ("play", 97) => "set_chunk_cache_radius".to_string(),
        ("play", 99) => "set_default_spawn_position".to_string(),
        ("play", 107) => "set_held_slot".to_string(),
        ("play", 115) => "set_time".to_string(),
        ("play", 120) => "start_configuration".to_string(),
        ("play", 130) => "ticking_state".to_string(),
        ("play", 131) => "ticking_step".to_string(),
        ("play", 134) => "update_attributes".to_string(),
        ("play", 136) => "update_recipes".to_string(),
        ("play", 137) => "update_tags".to_string(),
        ("play", 142) => "clear_dialog".to_string(),

        // Serverbound in play. Only the ones a connection actually sends
        // before any chunk arrives. The names come from packets.json, which the
        // data generator produced from the same jar, and tools/check_names.py
        // keeps them honest: being wrong about a name sends whoever reads the
        // log looking for the wrong packet, which is worse than "play:22".
        ("play-s2c", 0) => "accept_teleportation".to_string(),
        ("play-s2c", 6) => "chat_ack".to_string(),
        ("play-s2c", 13) => "client_tick_end".to_string(),
        ("play-s2c", 22) => "custom_payload".to_string(),
        ("play-s2c", 23) => "debug_subscription_request".to_string(),
        ("play-s2c", 32) => "move_player_rot".to_string(),
        ("play-s2c", 38) => "ping_request".to_string(),

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
    fn a_duration_reads_as_the_time_the_server_has_been_up() {
        let s = |n: u64| format_elapsed(std::time::Duration::from_secs(n));

        // Under a minute it is seconds with a tenth, because that is the range
        // where the gap between two lines is the interesting number.
        assert_eq!(s(0), "    0.0s");
        assert_eq!(s(12), "   12.0s");
        assert_eq!(
            format_elapsed(std::time::Duration::from_millis(12_450)),
            "   12.4s"
        );

        // Past a minute the tenths go, since whole seconds are what get compared.
        assert_eq!(s(60), "  1:00.0");
        assert_eq!(s(1800), " 30:00.0");

        // Past an hour there is no room left for a tenth.
        assert_eq!(s(3600), " 1:00:00");
        assert_eq!(s(3725), " 1:02:05");
    }

    #[test]
    fn every_timestamp_is_the_same_width_so_the_messages_stay_in_a_column() {
        let widths: Vec<usize> = [0u64, 5, 45, 59, 60, 600, 3599, 3600, 86_400]
            .iter()
            .map(|n| format_elapsed(std::time::Duration::from_secs(*n)).len())
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "widths were {widths:?}"
        );
    }

    #[test]
    fn packet_logging_is_off_until_it_is_asked_for() {
        // The gate itself is what is being checked; what it prints goes to stdout
        // and is not worth capturing here.
        set_verbose(false);
        assert!(!verbose());

        set_verbose(true);
        assert!(verbose());

        // Left off, so a later test in this binary does not depend on order.
        set_verbose(false);
    }

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
        // Ids overlap completely between phases. 3 is login_compression in
        // login, login_acknowledged serverbound in login, and
        // finish_configuration in configuration. Logging an id without the phase
        // would be ambiguous exactly when it matters.
        assert_eq!(packet_name("login", 3), "login_compression");
        assert_eq!(packet_name("login-s2c", 3), "login_acknowledged");
        assert_eq!(packet_name("configuration", 3), "finish_configuration");
        assert_eq!(
            packet_name("configuration-s2c", 3),
            "finish_configuration"
        );
    }

    #[test]
    fn an_unknown_id_still_reports_the_state_it_came_from() {
        assert_eq!(packet_name("play", 9999), "play:9999");
        assert_eq!(packet_name("nonsense", 1), "nonsense:1");
    }

    #[test]
    fn every_play_name_comes_from_packets_json() {
        // The seven that were wrong before, all of them serverbound. Being
        // wrong about a name is not cosmetic: it sends whoever reads the log
        // looking for the wrong packet. tools/check_names.py checks the whole
        // table against packets.json; this keeps the interesting ones under test
        // so a regression fails in cargo rather than needing the script.
        for (id, expected) in [
            (0, "accept_teleportation"),
            (6, "chat_ack"),
            (13, "client_tick_end"),
            (22, "custom_payload"),
            (23, "debug_subscription_request"),
            (32, "move_player_rot"),
            (38, "ping_request"),
        ] {
            assert_eq!(packet_name("play-s2c", id), expected, "play serverbound {id}");
        }
    }

    #[test]
    fn the_client_tick_packet_is_named_because_it_is_the_pulse() {
        // A client waiting on chunks sends this once a tick, one byte each, so a
        // run where it appears sixty times a second is the client reporting that
        // it is alive and still waiting. That is the signal to send chunks.
        assert_eq!(packet_name("play-s2c", 13), "client_tick_end");
    }

    #[test]
    fn status_has_a_name_in_both_directions() {
        // Ids 0 and 1 exist on both sides with the other meaning, which is why
        // the state carries the direction.
        assert_eq!(packet_name("status-s2c", 0), "status_request");
        assert_eq!(packet_name("status", 0), "status_response");
        assert_eq!(packet_name("status-s2c", 1), "ping_request");
        assert_eq!(packet_name("status", 1), "pong_response");
    }

    #[test]
    fn the_configuration_packets_this_server_sends_are_named() {
        // Without these, the log for the configuration phase reads
        // "configuration:14" and "configuration:15", which is what made the ids
        // worth confirming against a capture in the first place.
        assert_eq!(packet_name("configuration", 14), "update_tags");
        assert_eq!(packet_name("configuration", 15), "select_known_packs");
        assert_eq!(packet_name("configuration", 7), "registry_data");
        assert_eq!(
            packet_name("configuration", 13),
            "update_enabled_features"
        );
    }

    #[test]
    fn select_known_packs_has_a_name_in_both_directions() {
        // 15 clientbound is the server asking about packs, and 7 serverbound is
        // the client answering. Same packet, different id, opposite direction.
        assert_eq!(packet_name("configuration", 15), "select_known_packs");
        assert_eq!(packet_name("configuration-s2c", 7), "select_known_packs");
    }
}