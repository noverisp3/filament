//! Telling the other players what one of them did.
//!
//! Two people on this server share a world: one digs, and when the other arrives
//! the hole is there. But sharing the world is not the same as sharing what
//! happens in it. The digger is told about its own dig, because its connection
//! made it, and nobody else is told anything at all, so the second player keeps
//! looking at ground that is no longer there until they reconnect and the chunk
//! is sent again.
//!
//! # Why a channel rather than a shared socket
//!
//! Each connection owns its stream and writes to it from its own thread. The
//! alternative is handing the writer out to be locked from elsewhere, which
//! means a second thread can block a connection on a write, and that a socket
//! error can arrive on a thread that did not open it.
//!
//! So pushes go into a queue per connection, and that connection drains it in the
//! same loop that already drains its socket. Nothing is written to a socket by a
//! thread that does not own it.
//!
//! # What it costs
//!
//! A client that stops reading has its queue grow without bound, since these are
//! unbounded channels. That is a real limit and it is not defended against yet.
//! The fix is a bounded queue that drops the oldest push for a client that is
//! behind, on the grounds that a client which cannot keep up is better served by
//! a few missing updates than by the server running out of memory.
//!
//! # Why not just everyone, for now
//!
//! Every push goes to every player, including ones who have never loaded the
//! chunk it is about. That is wasted work but it is not wrong: a client ignores a
//! block update for a chunk it does not hold. Narrowing it to the players who
//! loaded the chunk wants each connection to publish its chunk set, which is
//! three hundred entries and moves whenever the player walks. Worth doing and
//! not worth doing before it is needed.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

/// One player's queue and the name they are known by.
struct Sink {
    id: u64,
    name: String,
    out: Sender<Vec<u8>>,
}

/// A registered player: who they are, and the queue that reaches them.
///
/// Carried as one thing rather than an id and a receiver side by side, because
/// the two are not meaningfully separable: the id means nothing without the
/// queue, and the queue does not know whose it is.
pub struct Registration {
    /// Which player this is, for telling everyone but them.
    pub id: u64,
    /// The connection's end of the queue.
    pub inbox: Receiver<Vec<u8>>,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration").field("id", &self.id).finish()
    }
}

/// The two things every phase after login needs: the world to ask, and the
/// players to tell.
///
/// Carried together because they are always passed together, and a function
/// taking eight arguments is one where a swap is a silent bug rather than a
/// compile error.
#[derive(Clone)]
pub struct Session {
    /// The thread that owns the world, asked through a handle.
    pub world: std::sync::Arc<super::ticking::WorldHandle>,
    /// Everyone who is connected.
    pub wire: std::sync::Arc<Broadcast>,
}

/// Every connected player, and the means to reach them.
#[derive(Default)]
pub struct Broadcast {
    sinks: Mutex<Vec<Sink>>,
    next_id: Mutex<u64>,
}

impl Broadcast {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a player and gives back the queue that reaches them.
    ///
    /// The receiver is the connection's to drain. It ends when the player is
    /// removed, or when the broadcast is dropped.
    pub fn register(&self, name: &str) -> Registration {
        let (out, inbox) = channel();

        let mut id = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        *id += 1;
        let id = *id;

        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        sinks.push(Sink {
            id,
            name: name.to_string(),
            out,
        });

        Registration { id, inbox }
    }

    /// Removes a player. Their queue is dropped, which ends their connection's
    /// drains with an empty one rather than a hang.
    pub fn remove(&self, id: u64) {
        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        sinks.retain(|s| s.id != id);
    }

    /// Sends one packet body to every player except `except`.
    ///
    /// `Vec<u8>` rather than a packet id and fields so that whatever builds it
    /// has already decided what it is; the receiver only has to frame it, which
    /// it does with its own encoder since compression is per connection.
    pub fn send_others(&self, except: u64, body: Vec<u8>) {
        let sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        for sink in sinks.iter().filter(|s| s.id != except) {
            // A send that fails means the player has gone; their sink is
            // removed when their connection unwinds, and one failed send is not
            // a reason to stop telling everybody else.
            let _ = sink.out.send(body.clone());
        }
    }

    /// How many players are listening.
    pub fn player_count(&self) -> usize {
        self.sinks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// The names of everyone listening, for the log.
    pub fn names(&self) -> Vec<String> {
        let sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        sinks.iter().map(|s| s.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_reaches_everyone_but_the_one_who_caused_it() {
        let wire = Broadcast::new();
        let mine = wire.register("digger");
        let theirs = wire.register("watcher");

        wire.send_others(mine.id, b"a hole".to_vec());

        assert_eq!(
            theirs.inbox.try_recv().expect("the watcher was told"),
            b"a hole"
        );
        assert!(
            mine.inbox.try_recv().is_err(),
            "the digger already knows, and is told by its own connection"
        );
    }

    #[test]
    fn a_removed_player_is_reached_by_nothing_further() {
        let wire = Broadcast::new();
        let gone = wire.register("leaving");
        let stayer = wire.register("staying");

        wire.remove(gone.id);
        wire.send_others(stayer.id, b"hello".to_vec());

        assert!(
            gone.inbox.try_recv().is_err(),
            "and their queue is closed, so a drain ends rather than waits"
        );
        assert_eq!(wire.player_count(), 1);
    }

    #[test]
    fn ids_do_not_repeat_when_someone_comes_back() {
        let wire = Broadcast::new();
        let first = wire.register("first");
        wire.remove(first.id);
        let second = wire.register("second");

        assert_ne!(first.id, second.id, "or a stale queue could be addressed as it");
    }

    #[test]
    fn a_push_carries_the_packet_id_and_the_body_together() {
        // A pushed body is handed to the receiver's own encoder, which adds a
        // length and nothing else. So the id has to be inside it. Sending the
        // body without the id makes the packet arrive with the first byte of
        // whatever field follows as its id, which for a block update is a byte
        // of the position: id 8, which is bundle_delimiter, a packet with no
        // body at all, and the client reports the position as eight bytes it
        // did not expect.
        let wire = Broadcast::new();
        let mine = wire.register("digger");
        let theirs = wire.register("watcher");

        let mut body = Vec::new();
        crate::protocol::play::write_block_update(&mut body, 1, 2, 3, 4);
        wire.send_others(mine.id, body.clone());

        let received = theirs.inbox.try_recv().expect("the watcher was told");
        assert_eq!(
            received,
            body,
            "what goes in is what comes out, byte for byte"
        );
        assert_eq!(
            received[0],
            crate::protocol::play::id::BLOCK_UPDATE as u8,
            "and the first byte is still the packet id"
        );
    }

    #[test]
    fn nobody_listening_is_not_a_problem() {
        let wire = Broadcast::new();
        wire.send_others(0, b"into the void".to_vec());
        assert_eq!(wire.player_count(), 0);
    }

    #[test]
    fn names_come_back_for_the_log() {
        let wire = Broadcast::new();
        wire.register("noverisp3");
        wire.register("notrealyet_");
        assert_eq!(wire.names(), vec!["noverisp3", "notrealyet_"]);
    }
}
