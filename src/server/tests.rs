use super::*;
use std::collections::HashSet;

#[test]
fn entity_ids_are_never_zero() {
    // The one property that matters, checked directly rather than inferred.
    for _ in 0..1000 {
        assert_ne!(
            next_entity_id(),
            0,
            "the client reads 0 as unassigned and throws when it is asked for"
        );
    }
}

#[test]
fn entity_ids_are_unique() {
    let mut seen = HashSet::new();
    for _ in 0..1000 {
        assert!(seen.insert(next_entity_id()), "an id was handed out twice");
    }
}

#[test]
fn the_counter_is_shared_across_threads() {
    // Connections are handled on their own threads, so two of them asking at
    // once must not get the same id. A plain `static mut` counter here would
    // be a data race; this is the reason the fetch_add is atomic.
    let handles: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(|| (0..250).map(|_| next_entity_id()).collect::<Vec<_>>()))
        .collect();

    let mut all = HashSet::new();
    for h in handles {
        for id in h.join().expect("thread did not panic") {
            assert!(all.insert(id), "two threads were given entity id {id}");
        }
    }
    assert_eq!(all.len(), 1000);
}