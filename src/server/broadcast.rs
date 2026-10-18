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

/// How finely a movement is measured, and how far away a player can be and
/// still be worth telling about.
///
/// The scale is the game's, from `VecDeltaCodec`; see
/// [`crate::protocol::play::write_move_entity_pos_rot`]. The range is the one
/// the game itself uses when it broadcasts a level event to the players near
/// something happening, which makes it a reasonable stand-in for "near enough
/// to see a person walk".
const DELTA_SCALE: f64 = 4096.0;

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
    /// What this connection has been told about every other player, and where
    /// it was told they were.
    ///
    /// Per observer, and that is the whole point. A movement is sent as a
    /// distance rather than a place, so the server cannot keep one copy of
    /// "where they are": it has to keep one copy per pair, because a client
    /// joining halfway through somebody's walk has been told a different
    /// starting point than one that was there from the start. Keeping the
    /// player's real position here and sending that instead would sidestep the
    /// problem, and the game does not do it because a client that is sent
    /// absolute positions instead of deltas interpolates differently and
    /// rubber-bands on a lossy link.
    seen: std::collections::HashMap<u64, Seen>,
}

/// What one connection was last told about one other player.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seen {
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
            seen: std::collections::HashMap::new(),
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

    /// Records that one connection has been told where another player is.
    ///
    /// Called whenever an `add_entity` goes out, because that packet is where a
    /// delta's zero point is set. A movement sent before this has no baseline
    /// to be measured from, and the options are to skip it â€” which leaves the
    /// player standing still until they next move â€” or to guess.
    pub fn note_told(&self, observer: u64, subject: u64, pos: (f64, f64, f64), yaw: f32, pitch: f32) {
        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sink) = sinks.iter_mut().find(|s| s.id == observer) {
            sink.seen.insert(subject, Seen { pos, yaw, pitch });
        }
    }

    /// Records a player's new position and facing, so a later arrival is told
    /// where they actually are rather than where they logged on.
    pub fn update_pose(&self, id: u64, pos: (f64, f64, f64), yaw: f32, pitch: f32) {
        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sink) = sinks.iter_mut().find(|s| s.id == id) {
            sink.pos = pos;
            sink.yaw = yaw;
            sink.pitch = pitch;
        }
    }

    /// Tells everyone who is watching that `id` moved.
    ///
    /// One packet per observer, each measuring the movement from what *that*
    /// observer was last told, which is what makes a relative delta work at
    /// all when players arrive at different times.
    ///
    /// Skips three kinds of observer, and each skip is a different reason:
    ///
    /// - one that has never been told this player exists, because it has no
    ///   baseline and a delta from nowhere is not a position;
    /// - one that is too far away to see them, at the same sixty-four blocks
    ///   the game uses for its own nearby broadcasts;
    /// - one that has not been told anything new, where the movement rounds to
    ///   no delta and the facing has not turned. A player standing still sends
    ///   twenty identical packets a second otherwise, each of which every other
    ///   client decodes and discards.
    pub fn send_movement(&self, id: u64, on_ground: bool) {
        const RANGE: f64 = 64.0;

        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        // Copied out rather than borrowed: the loop below mutates the very
        // sinks it is reading, and a borrow of one while another is written
        // needs the lock held twice over.
        let Some(subject) = sinks
            .iter()
            .find(|s| s.id == id)
            .map(|s| (s.entity_id, s.pos, s.yaw, s.pitch, s.uuid))
        else {
            return;
        };
        let (subject_entity_id, subject_pos, subject_yaw, subject_pitch, subject_uuid) = subject;

        let mut deliveries: Vec<(u64, Vec<u8>)> = Vec::new();
        for sink in sinks.iter_mut() {
            if sink.id == id {
                continue;
            }
            let Some(previous) = sink.seen.get(&id).copied() else {
                continue;
            };

            // How far this observer is from the player, which is not the same
            // thing as how far the player moved. A player crossing the room in
            // one step is a short movement and a long distance.
            let ex = subject_pos.0 - sink.pos.0;
            let ey = subject_pos.1 - sink.pos.1;
            let ez = subject_pos.2 - sink.pos.2;
            if ex * ex + ey * ey + ez * ez > RANGE * RANGE {
                // Too far to see. Their baseline is left alone, so walking back
                // into range sends one big delta rather than a series of small
                // ones, which is right: they were not watching.
                continue;
            }

            let dx = subject_pos.0 - previous.pos.0;
            let dy = subject_pos.1 - previous.pos.1;
            let dz = subject_pos.2 - previous.pos.2;

            // What the client will actually see, after it rounds to 1/4096. All
            // three axes: a movement of only one of them is still a movement.
            //
            // This used to be written as `[(dx, dy, dz)].iter().any(|a| a.0 …)`,
            // which is an array of one tuple rather than three values, so it
            // tested dx and nothing else. Walking and turning worked and
            // falling and jumping did not, and the axis that failed was the
            // one nobody was looking at while walking about on flat ground.
            let moved = [dx, dy, dz]
                .iter()
                .any(|axis| (axis * DELTA_SCALE).round() != 0.0);
            let turned = subject_yaw != previous.yaw || subject_pitch != previous.pitch;
            if !moved && !turned {
                continue;
            }

            let mut body = Vec::new();
            let wrote = crate::protocol::play::write_move_entity_pos_rot(
                &mut body,
                subject_entity_id,
                (dx, dy, dz),
                subject_yaw,
                subject_pitch,
                on_ground,
            );

            if !wrote {
                // Too far to be a delta, so say where they are instead. Putting
                // the body down again is the honest answer: it carries an
                // absolute position, so the client has nothing to add to
                // anything it was told before.
                //
                // This is what a player who fell on arrival used to need, and
                // did not get. Their fall was forty blocks of delta against a
                // range of eight, so it was clamped, so the body came down
                // eight blocks a packet, and once the fall was over there were
                // no more y deltas to send and the body hung there for good.
                body.clear();
                crate::protocol::play::write_add_entity(
                    &mut body,
                    subject_entity_id,
                    subject_uuid,
                    crate::protocol::play::PLAYER_ENTITY_TYPE,
                    subject_pos,
                    subject_yaw,
                    subject_pitch,
                );
            }

            deliveries.push((sink.id, body));

            sink.seen.insert(
                id,
                Seen {
                    pos: subject_pos,
                    yaw: subject_yaw,
                    pitch: subject_pitch,
                },
            );
        }

        // One packet, to the one observer it was measured for. Collecting the
        // bodies and then sending each of them to everybody would give every
        // observer one movement per observer, so the body would advance at the
        // square of the player count and walk off in the sky.
        for (observer, body) in deliveries {
            if let Some(sink) = sinks.iter().find(|s| s.id == observer) {
                let _ = sink.out.send(body);
            }
        }
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
/// connection has a great many ways out â€” a clean return, a read error, a write
/// that failed, an early return when the world would not answer â€” and a line of
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
        wire.register(name, [0u8; 16], 1, crate::world::terrain::spawn_point())
    }

    /// Reads the x delta out of a `move_entity_pos_rot` body.
    fn read_delta_x(body: &[u8]) -> f64 {
        let mut b = body;
        assert_eq!(
            crate::protocol::play::id::MOVE_ENTITY_POS_ROT as u8,
            b[0],
            "the packet is a movement"
        );
        b = &b[1..];
        // The entity id and the properties.
        b = skip_varints(b);
        b = skip_varints(b);
        let dx = i16::from_be_bytes([b[0], b[1]]);
        f64::from(dx) / DELTA_SCALE
    }

    fn skip_varints(mut b: &[u8]) -> &[u8] {
        while b[0] & 0x80 != 0 {
            b = &b[1..];
        }
        &b[1..]
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
    fn two_observers_who_joined_at_different_times_get_different_deltas() {
        // The reason "seen" is per observer and the reason the wire carries a
        // delta rather than a position.
        //
        // The watcher has been told the mover is at 0,0,0 and is sent a move to
        // 1,0,0, so its delta is one block. The latecomer is only now being
        // told where the mover is — at 1,0,0 — and then is sent the same move
        // to 2,0,0, so its delta is one block too.
        //
        // A server that kept one copy of "where the mover is" would send the
        // second observer a delta of two, and that observer would jump.
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
        let late = wire.register("late", [3u8; 16], 3, (0.0, 64.0, 0.0));

        wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

        // First move: only the watcher was told about the mover.
        wire.update_pose(mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);
        let first = watcher.inbox.try_recv().expect("the watcher was told");
        assert_eq!(read_delta_x(&first), 1.0, "one block from where it was told");

        // The latecomer arrives and is given the mover's current position.
        wire.note_told(late.id, mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);

        // Second move.
        wire.update_pose(mover.id, (2.0, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);

        let to_watcher = watcher.inbox.try_recv().expect("the watcher again");
        let to_late = late.inbox.try_recv().expect("the latecomer too");

        // Exactly one each. This assertion is the one that was missing, and its
        // absence is why a server with two observers sent everybody else's
        // movement as well: each observer received one packet per observer, so
        // the body advanced twice as fast as it was told it had and walked off
        // into the sky. The delta values were all correct, which is why the
        // earlier version of this test passed while the game was visibly wrong.
        assert!(
            watcher.inbox.try_recv().is_err(),
            "one packet per movement, however many are watching"
        );
        assert!(
            late.inbox.try_recv().is_err(),
            "and the same for the other one"
        );

        assert_eq!(
            read_delta_x(&to_watcher),
            1.0,
            "the watcher is told the move from where it last was"
        );
        assert_eq!(
            read_delta_x(&to_late),
            1.0,
            "and so is the latecomer, from where it was introduced"
        );
        // Both packets address the mover and both say one block, so they come
        // out identical here. That is the point: each was measured from what
        // that observer knew, and the observers happen to agree. Had the server
        // kept one copy of the mover's position, one of these would have been a
        // two block jump.
        assert_eq!(to_watcher, to_late);
    }

    #[test]
    fn every_observer_gets_one_packet_and_not_one_per_observer() {
        // Three watchers, so the mistake would have tripled the speed.
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let mut watchers = Vec::new();
        for i in 0..3 {
            let w = wire.register(&format!("watcher{i}"), [2u8; 16], 100 + i, (0.0, 64.0, 0.0));
            wire.note_told(w.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);
            watchers.push(w);
        }

        wire.update_pose(mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);

        for w in &watchers {
            assert_eq!(
                read_delta_x(&w.inbox.try_recv().unwrap_or_else(|_| {
                    panic!("watcher {} got nothing", w.name)
                })),
                1.0,
                "watcher {} was told the move",
                w.name
            );
            assert!(
                w.inbox.try_recv().is_err(),
                "watcher {} was told it more than once, and will walk at three \
                 times the speed it was told about",
                w.name
            );
        }
    }

    #[test]
    fn a_player_standing_still_sends_nothing() {
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
        wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

        for _ in 0..20 {
            wire.send_movement(mover.id, true);
        }
        assert!(
            watcher.inbox.try_recv().is_err(),
            "twenty identical movements a second is twenty packets a second \
             that every client decodes and throws away"
        );
    }

    #[test]
    fn a_movement_too_small_to_see_is_not_sent() {
        // The client rounds to 1/4096, so anything under half of that arrives
        // as zero. The test has to be on the rounded value, because that is
        // what the client ends up holding.
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
        wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

        // A quarter of a step, which rounds away to nothing.
        wire.update_pose(mover.id, (0.0 + 1.0 / 16384.0, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);
        assert!(watcher.inbox.try_recv().is_err(), "a quarter of a step");

        // Half a step, which rounds up to one and is worth sending.
        wire.update_pose(mover.id, (1.0 / 8192.0, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);
        assert_eq!(
            read_delta_x(&watcher.inbox.try_recv().unwrap()),
            1.0 / 4096.0,
            "which arrives as exactly one step"
        );

        // And a real movement.
        wire.update_pose(mover.id, (0.5, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);
        assert_eq!(read_delta_x(&watcher.inbox.try_recv().unwrap()), 0.5);
    }

    #[test]
    fn a_movement_is_not_sent_to_somebody_who_has_never_heard_of_the_player() {
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let stranger = wire.register("stranger", [2u8; 16], 2, (0.0, 64.0, 0.0));

        wire.update_pose(mover.id, (5.0, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);

        assert!(
            stranger.inbox.try_recv().is_err(),
            "a delta from nowhere is not a position"
        );
    }

    #[test]
    fn a_movement_is_not_sent_across_the_room() {
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let far = wire.register("far", [2u8; 16], 2, (1000.0, 64.0, 0.0));
        wire.note_told(far.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

        wire.update_pose(mover.id, (1.0, 64.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, true);
        assert!(far.inbox.try_recv().is_err(), "a kilometre is past seeing");
    }

    #[test]
    fn turning_in_place_is_worth_a_packet() {
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
        wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

        wire.update_pose(mover.id, (0.0, 64.0, 0.0), 90.0, 0.0);
        wire.send_movement(mover.id, true);

        let body = watcher.inbox.try_recv().expect("a turn is news");
        // Two VarInts, then six bytes of delta, then the two angles.
        assert_eq!(body[9], 64, "yaw east");
    }

    #[test]
    fn a_movement_on_one_axis_only_is_still_a_movement() {
        // Three axes, and the check that decides whether to send has to look at
        // all of them. It once looked at the first one only, which is why
        // walking worked, falling did not, and neither was noticed on flat
        // ground.
        for (name, to) in [
            ("across x", (3.0, 64.0, 0.0)),
            ("up and down y", (0.0, 70.0, 0.0)),
            ("along z", (0.0, 64.0, -3.0)),
            ("diagonally", (3.0, 67.0, -3.0)),
        ] {
            let wire = Broadcast::new();
            let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
            let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
            wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

            wire.update_pose(mover.id, to, 0.0, 0.0);
            wire.send_movement(mover.id, true);

            assert!(
                watcher.inbox.try_recv().is_ok(),
                "moving {} is worth a packet",
                name
            );
        }
    }

    #[test]
    fn a_move_that_cannot_be_a_delta_arrives_as_a_position() {
        // The self-healing path: a player who fell, or was teleported, or was
        // simply too far, is put down again rather than nudged. This is what a
        // player arriving used to need and did not get.
        let wire = Broadcast::new();
        let mover = wire.register("mover", [1u8; 16], 1, (0.0, 64.0, 0.0));
        let watcher = wire.register("watcher", [2u8; 16], 2, (0.0, 64.0, 0.0));
        wire.note_told(watcher.id, mover.id, (0.0, 64.0, 0.0), 0.0, 0.0);

        // A fall of forty blocks, which no short can carry.
        wire.update_pose(mover.id, (0.0, 24.0, 0.0), 0.0, 0.0);
        wire.send_movement(mover.id, false);

        let body = watcher
            .inbox
            .try_recv()
            .expect("something was sent, even though it could not be a delta");
        assert_eq!(
            body[0],
            crate::protocol::play::id::ADD_ENTITY as u8,
            "so it is an add_entity, carrying the position outright"
        );
        assert!(
            watcher.inbox.try_recv().is_err(),
            "once"
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
