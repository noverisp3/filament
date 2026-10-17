//! The server: accepting connections and answering them.
//!
//! At the moment this answers the handshake, the status exchange, an offline
//! login, the configuration phase and the start of play. What it does not do is
//! tick anything, because there is nothing to tick yet.

pub mod log;
pub mod status;

pub use status::{StatusServer, PROTOCOL_VERSION};

use std::sync::atomic::{AtomicI32, Ordering};

/// Hands out entity ids.
///
/// Ids start at 1 and never repeat within a connection, because the client
/// treats 0 as "not yet assigned" rather than as an id:
///
/// ```java
/// // Entity.java
/// public int getId() {
///    if (this.id == 0) throw new IllegalStateException("Tried to access entity ID before ID assignment");
///    else return this.id;
/// }
/// ```
///
/// So a login packet carrying id 0 is accepted by the decoder and then blows up
/// when the client adds the player to the level, which is a confusing way to
/// find out. It costs one static counter to not have that problem, and the ids
/// are needed per connection anyway once there are entities to hand them to.
static NEXT_ENTITY_ID: AtomicI32 = AtomicI32::new(1);

/// The next entity id.
///
/// Wrapping back to zero would reintroduce the problem above, so it wraps to one
/// instead. Realistically that is billions of entities and it does not matter
/// what happens then; skipping zero matters now.
pub fn next_entity_id() -> i32 {
    let id = NEXT_ENTITY_ID.fetch_add(1, Ordering::Relaxed);
    if id == i32::MAX {
        NEXT_ENTITY_ID.store(1, Ordering::Relaxed);
        return 1;
    }
    id
}

#[cfg(test)]
mod tests {
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
}