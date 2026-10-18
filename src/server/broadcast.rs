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

/// One player's queue, and enough about them for somebody else to be told they
/// are here.
struct Sink {
    id: u64,
    name: String,
    uuid: [u8; 16],
    out: Sender<Vec<u8>>,
    /// The id the client knows this player by, which is not the same as the
    /// profile uuid and not the same as the registration id either. Three ids
    /// for one person, none of them derivable from the others.
    entity_id: i32,
    /// Where they are and which way they face.
    ///
    /// Held here rather than kept by the connection because the connection has
    /// nowhere to be asked: the thread that learns a player moved is not the
    /// thread that needs to tell anyone, and the one that needs to tell anyone
    /// does not know it happened until the queue says so.
    pos: (f64, f64, f64),
    yaw: f32,
    pitch: f32,
}

/// What a connection needs to know about another player to put them in its world.
///
/// The name says the tab list, which is only part of it. The entity id is what
/// the tab list, the body and every later movement are all addressed by, so the
/// connection's own entity id cannot be worked out locally from the name and has
/// to be carried across.
#[derive(Clone, Debug)]
pub struct PlayerInfo {
    pub id: u64,
    pub name: String,
    pub uuid: [u8; 16],
    /// The connection's entity id, the one the client knows them by.
    pub entity_id: i32,
    pub pos: (f64, f64, f64),
    pub yaw: f32,
    pub pitch: f32,
}

/// A registered player: who they are, and the queue that reaches them.
///
/// Carried as one thing rather than an id and a receiver side by side, because
/// the two are not meaningfully separable: the id means nothing without the
/// queue, and the queue does not know whose it is.
pub struct Registration {
    /// Which player this is, for telling everyone but them.
    pub id: u64,
    /// Who they are, carried here because the connection outlives the function
    /// that read the name off the login.
    pub name: String,
    pub uuid: [u8; 16],
    /// The entity id the client knows this player by, for packets that address
    /// the body rather than the profile.
    pub entity_id: i32,
    /// The connection's end of the queue.
    pub inbox: Receiver<Vec<u8>>,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish()
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
    pub fn register(
        &self,
        name: &str,
        uuid: [u8; 16],
        entity_id: i32,
        pos: (f64, f64, f64),
    ) -> Registration {
        let (out, inbox) = channel();

        let mut id = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        *id += 1;
        let id = *id;

        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        sinks.push(Sink {
            id,
            name: name.to_string(),
            uuid,
            out,
            entity_id,
            pos,
            yaw: 0.0,
            pitch: 0.0,
        });

        Registration {
            id,
            name: name.to_string(),
            uuid,
            entity_id,
            inbox,
        }
    }

    /// Everyone who is here, for a connection that has just arrived.
    ///
    /// Taken as a copy under one lock rather than handed out as references,
    /// because the caller writes these out over its own socket, which can block,
    /// and a lock held across a blocked write is a lock nobody else can take.
    pub fn snapshot(&self) -> Vec<PlayerInfo> {
        let sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        sinks
            .iter()
            .map(|s| PlayerInfo {
                id: s.id,
                name: s.name.clone(),
                uuid: s.uuid,
                entity_id: s.entity_id,
                pos: s.pos,
                yaw: s.yaw,
                pitch: s.pitch,
            })
            .collect()
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
    pub fn player_count(&self) -> usize {        self.sinks
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

/// Announces that a player has gone, when their connection ends however it ends.
///
/// A guard rather than a call at the end of the connection, because the
/// connection has a great many ways out — a clean return, a read error, a write
/// that failed, an early return when the world would not answer — and a line of
/// cleanup written at the bottom of a function is reached by none of the others.
/// A value that is dropped is reached by all of them, including the ones nobody
/// writes down.
pub struct Departure {
    wire: std::sync::Arc<Broadcast>,
    id: u64,
    entity_id: i32,
    uuid: [u8; 16],
    name: String,
}

impl Departure {
    /// Arms the announcement. Nothing is sent until this value is dropped.
    pub fn arm(wire: std::sync::Arc<Broadcast>, reg: &Registration) -> Self {
        Self {
            wire,
            id: reg.id,
            entity_id: reg.entity_id,
            uuid: reg.uuid,
            name: reg.name.clone(),
        }
    }
}

impl Drop for Departure {
    fn drop(&mut self) {
        // Only worth announcing to somebody. A server that just lost its first
        // player has nobody to tell, and building the packets for nobody is
        // work that grows with every join on an empty server.
        if self.wire.player_count() > 1 {
            // The body first, then the tab list entry, matching the order they
            // arrived in: the client is holding a name for a body it is about to
            // stop drawing, which is briefly a ghost in the tab list.
            let mut body = Vec::new();
            crate::protocol::play::write_remove_entities(&mut body, &[self.entity_id]);
            self.wire.send_others(self.id, body);

            let mut info = Vec::new();
            crate::protocol::play::write_player_info_remove(&mut info, &[self.uuid]);
            self.wire.send_others(self.id, info);
        }

        // After the announcement, so the packets are queued to the sinks that
        // are still there rather than to one that has already been dropped.
        self.wire.remove(self.id);
        crate::server::log::note(format!(
            "player {:?} (entity {}) left, {} remaining",
            self.name,
            self.entity_id,
            self.wire.player_count()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registration with the parts these tests do not care about filled in.
    ///
    /// A uuid that is all zeroes is not a uuid a real player would have, and
    /// nothing here looks at one. Naming the helper rather than spelling out
    /// four arguments at each call keeps the tests about the queue, which is
    /// what they are actually about.
    fn join(wire: &Broadcast, name: &str) -> Registration {
        wire.register(name, [0u8; 16], 1, crate::protocol::play::SPAWN)
    }

    #[test]
    fn a_player_arriving_is_told_about_the_ones_already_here() {
        let wire = std::sync::Arc::new(Broadcast::new());
        let first = wire.register("first", [1u8; 16], 7, (0.0, 64.0, 0.0));
        let second = wire.register("second", [2u8; 16], 9, (0.0, 64.0, 0.0));

        let seen = wire.snapshot();
        assert_eq!(
            seen.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["first", "second"],
            "the second arrival is handed both, including the one before it"
        );

        // Each entry carries the things the receiving side cannot work out for
        // itself: the name, the profile uuid, and the entity id the client will
        // address the body by.
        let me = seen.iter().find(|p| p.name == "second").expect("the second arrival");
        assert_eq!(me.uuid, [2u8; 16]);
        assert_eq!(me.entity_id, 9);
        assert_eq!(me.id, second.id);
        assert_ne!(
            me.id, me.entity_id as u64,
            "the registration id and the entity id are unrelated numbers, and \
             sending one where the other belongs addresses the wrong player"
        );
        let _ = first;
    }


    #[test]
    fn a_push_reaches_everyone_but_the_one_who_caused_it() {
        let wire = Broadcast::new();
        let mine = join(&wire, "digger");
        let theirs = join(&wire, "watcher");

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
        let gone = join(&wire, "leaving");
        let stayer = join(&wire, "staying");

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
        let first = join(&wire, "first");
        wire.remove(first.id);
        let second = join(&wire, "second");

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
        let mine = join(&wire, "digger");
        let theirs = join(&wire, "watcher");

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
        join(&wire, "noverisp3");
        join(&wire, "notrealyet_");
        assert_eq!(wire.names(), vec!["noverisp3", "notrealyet_"]);
    }
}
