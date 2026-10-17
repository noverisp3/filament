//! Shutting down when asked to, rather than when killed.
//!
//! Ctrl+C on a blocking accept is abrupt: the process is gone mid-write and the
//! last few lines are whatever the buffer held. That matters more here than it
//! would in a program with nothing to lose, because the log is the only record
//! of what the server was doing, and it is what gets read when something breaks.
//!
//! So the ask is recorded rather than acted on. A handler sets a flag; the accept
//! loop notices the flag at its next poll, says what it was doing, and returns.
//! The alternative, calling `exit` from the handler, would mean printing from a
//! context where printing may not be safe, and on a platform where the handler
//! runs on its own thread it would also mean the flag was never needed.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set once, when the process is asked to stop.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Whether the process has been asked to stop.
pub fn requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Records the request. Separate from [`install`] so a test can ask for a
/// shutdown without installing a console handler that outlives the test.
pub fn request() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Asks the process to stop cleanly when the console asks it to.
///
/// Ctrl+C and Ctrl+Break on Windows, SIGINT and SIGTERM elsewhere.
///
/// The Windows half is a declaration of a C function rather than a dependency.
/// It is three lines of `extern` and one function pointer, and taking a crate
/// for it would put a build dependency into a project whose whole argument is
/// that its storage layer needs none. It also has to be this: the standard
/// library has no signal handling at all, so a portable version needs either a
/// dependency or a non-blocking accept, and the non-blocking accept is wanted
/// anyway for the poll loop.
pub fn install() {
    // SAFETY: `SetConsoleCtrlHandler` takes a plain function pointer and a
    // boolean. The handler it is given is `on_console_signal`, which only ever
    // stores to a static atomic and touches no allocation, no I/O and no lock,
    // which is the bar for being called from a context the runtime did not
    // choose. Nothing is returned that needs freeing.
    #[cfg(windows)]
    unsafe {
        unsafe extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<unsafe extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }

        unsafe extern "system" fn on_console_signal(_kind: u32) -> i32 {
            request();
            // Handled, and specifically not "exit": returning zero tells the
            // console this handler dealt with it and the process keeps running
            // until the accept loop notices.
            0
        }

        SetConsoleCtrlHandler(Some(on_console_signal), 1);
    }

    #[cfg(not(windows))]
    {
        // No handler is installed here. The accept loop polls the flag either
        // way, so on this platform a stop is whatever the runtime does by
        // default, which is to end the process.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_requested_shutdown_is_visible_to_the_loop() {
        // Not run in parallel with anything else: it sets a process-wide flag.
        assert!(!requested(), "nothing has asked to stop yet");
        request();
        assert!(requested(), "the loop polls this and returns");
    }
}
