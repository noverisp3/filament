use super::*;

fn pos(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

#[test]
fn nothing_runs_before_its_tick() {
    // The whole point of scheduling rather than doing it inline: this is work
    // the world owes itself later, and later means later.
    let mut ticks = BlockTicks::new();
    ticks.schedule(0, 20, pos(1, 2, 3));

    assert!(ticks.take_due(19, TICK_BUDGET).is_empty(), "not one tick early");
    assert_eq!(ticks.len(), 1, "and it is still waiting");
    assert_eq!(
        ticks.take_due(20, TICK_BUDGET),
        vec![pos(1, 2, 3)],
        "and it runs on the tick it was promised for"
    );
    assert!(ticks.is_empty(), "leaving nothing behind");
}

#[test]
fn work_runs_soonest_first() {
    let mut ticks = BlockTicks::new();
    ticks.schedule(0, 30, pos(9, 0, 0));
    ticks.schedule(0, 10, pos(3, 0, 0));
    ticks.schedule(0, 20, pos(6, 0, 0));

    assert_eq!(
        ticks.take_due(30, TICK_BUDGET),
        vec![pos(3, 0, 0), pos(6, 0, 0), pos(9, 0, 0)],
        "three due, soonest first"
    );
}

#[test]
fn two_things_due_at_once_run_in_the_order_they_were_asked_for() {
    // Water that flows one way and back oscillates if the order is not stable,
    // which is the kind of bug that looks like a rendering glitch and is not.
    let mut ticks = BlockTicks::new();
    ticks.schedule(0, 5, pos(7, 0, 0));
    ticks.schedule(0, 5, pos(2, 0, 0));
    ticks.schedule(0, 5, pos(5, 0, 0));

    assert_eq!(
        ticks.take_due(5, TICK_BUDGET),
        vec![pos(7, 0, 0), pos(2, 0, 0), pos(5, 0, 0)],
        "scheduled order, not position order and not reverse"
    );
}

#[test]
fn a_repeat_is_another_promise_and_not_a_replacement() {
    // A rule that reschedules itself -- a crop, a clock -- depends on this. If
    // scheduling the same position twice replaced the first, the rule would run
    // once instead of twice and nothing would say so.
    let mut ticks = BlockTicks::new();
    ticks.schedule(0, 1, pos(4, 5, 6));
    ticks.schedule(0, 1, pos(4, 5, 6));

    assert_eq!(ticks.len(), 2, "two promises");
    assert_eq!(ticks.take_due(1, TICK_BUDGET).len(), 2, "and both come due");
}

#[test]
fn a_budget_spreads_the_work_rather_than_dropping_it() {
    // More work due than one tick will do. Dropping it loses a block update and
    // the world quietly stops agreeing with itself; doing it all anyway makes
    // the next tick worse and the one after that worse still.
    let mut ticks = BlockTicks::new();
    for x in 0..10 {
        ticks.schedule(0, 1, pos(x, 0, 0));
    }

    assert_eq!(ticks.take_due(1, 4).len(), 4, "as many as the budget allows");
    assert_eq!(ticks.len(), 6, "and the rest are still owed");

    assert_eq!(ticks.take_due(1, 4).len(), 4, "coming back next tick");
    assert_eq!(ticks.len(), 2);
    assert_eq!(ticks.take_due(1, 4).len(), 2, "and the last of them");
    assert!(ticks.is_empty());
}

#[test]
fn a_budget_does_not_run_anything_not_due() {
    // The budget is on work that is due, not on work in general. Taking the
    // next four regardless of when they are would run the future early.
    let mut ticks = BlockTicks::new();
    ticks.schedule(0, 10, pos(1, 0, 0));
    ticks.schedule(0, 20, pos(2, 0, 0));

    assert!(
        ticks.take_due(5, TICK_BUDGET).is_empty(),
        "nothing is due at five"
    );
    assert_eq!(ticks.take_due(10, TICK_BUDGET).len(), 1, "one is at ten");
    assert_eq!(
        ticks.take_due(20, TICK_BUDGET).len(),
        1,
        "and the other at twenty"
    );
}

#[test]
fn the_earliest_is_known_without_taking_anything() {
    // A clock needs to know when it is next due to sleep until then, and taking
    // the whole backlog to find out would spend work to answer a question.
    let mut ticks = BlockTicks::new();
    assert_eq!(ticks.earliest(), None, "nothing promised");
    ticks.schedule(100, 30, pos(1, 0, 0));
    ticks.schedule(100, 10, pos(2, 0, 0));
    assert_eq!(ticks.earliest(), Some(110), "the soonest of them");
    assert_eq!(ticks.len(), 2, "and asking did not consume either");
}

#[test]
fn a_chunk_that_goes_takes_its_promises_with_it() {
    // A backlog for a chunk that no longer exists runs later against a world
    // that has been rebuilt underneath it. That is how a build ends up with
    // blocks that change on their own hours after it stopped loading.
    let mut ticks = BlockTicks::new();
    ticks.schedule(0, 5, pos(0, 0, 0));
    ticks.schedule(0, 5, pos(8, 0, 0)); // same chunk: x 0..15
    ticks.schedule(0, 5, pos(16, 0, 0)); // the next chunk

    let dropped = ticks.forget_chunk(|p| p.x < 16);
    assert_eq!(dropped, 2, "both promises in the first chunk");
    assert_eq!(
        ticks.take_due(5, TICK_BUDGET),
        vec![pos(16, 0, 0)],
        "and the chunk that stayed keeps its own"
    );
}

#[test]
fn a_tick_that_has_gone_runs_only_once() {
    // The bug a naive implementation has: `take_due` leaves things due but not
    // taken, and the next tick runs them again. Ticking twice is a rule that
    // fires twice, which for anything spreading looks like the world
    // oscillating.
    let mut ticks = BlockTicks::new();
    ticks.schedule(0, 1, pos(8, 0, 0));

    assert_eq!(ticks.take_due(1, TICK_BUDGET).len(), 1, "once");
    assert!(ticks.take_due(1, TICK_BUDGET).is_empty(), "and not again");
    assert!(ticks.take_due(99, TICK_BUDGET).is_empty(), "nor ever again");
}

#[test]
fn a_delay_larger_than_now_saturates_instead_of_wrapping() {
    // Unsigned subtraction that wrapped would schedule work in the past, which
    // would run on the very next tick and look like a rule firing far too early.
    let mut ticks = BlockTicks::new();
    ticks.schedule(u64::MAX, 20, pos(1, 0, 0));

    assert_eq!(
        ticks.earliest(),
        Some(u64::MAX),
        "it saturates to now rather than wrapping into the past"
    );
    assert!(
        ticks.take_due(u64::MAX - 1, TICK_BUDGET).is_empty(),
        "and so it has not come due a moment ago"
    );
    assert_eq!(
        ticks.take_due(u64::MAX, TICK_BUDGET).len(),
        1,
        "it comes due at the tick it saturated to, and then"
    );
}
