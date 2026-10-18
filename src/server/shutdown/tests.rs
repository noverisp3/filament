use super::*;

#[test]
fn a_requested_shutdown_is_visible_to_the_loop() {
    // Not run in parallel with anything else: it sets a process-wide flag.
    assert!(!requested(), "nothing has asked to stop yet");
    request();
    assert!(requested(), "the loop polls this and returns");
}