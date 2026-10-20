//! The server: accepting connections and answering them.
//!
//! At the moment this answers the handshake, the status exchange, an offline
//! login, the configuration phase and the start of play. What it does not do is
//! tick anything, because there is nothing to tick yet.

pub mod broadcast;
pub mod log;
pub mod shutdown;
pub mod status;
pub mod ticking;

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
/// A login packet carrying id 0 is accepted by the decoder and then blows up
/// when the client adds the player to the level, which is a confusing way to
/// find out.
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
mod tests;
