//! Telling the other players what one of them did.
//!
//! # Why a channel rather than a shared socket
//!
//! Each connection owns its stream and writes to it from its own thread. The
//! alternative is handing the writer out to be locked from elsewhere, which
//! means a second thread can block a connection on a write.
//!
//! So pushes go into a queue per connection, and that connection drains it in
//! the same loop that already drains its socket. Nothing is written to a socket
//! by a thread that does not own it.
//!
//! # What it costs
//!
//! A client that stops reading has its queue grow without bound, since these
//! are unbounded channels. The fix is a bounded queue that drops the oldest
//! push for a client that is behind.
//!
//! # Why not just everyone, for now
//!
//! Every push goes to every player, including ones who have never loaded the
//! chunk it is about. That is wasted work but it is not wrong: a client
//! ignores a block update for a chunk it does not hold.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

/// How finely a movement is measured: one part in 4096 of a block.
///
/// The game's, from `VecDeltaCodec`; see
/// [`crate::protocol::play::write_move_entity_pos_rot`].
const DELTA_SCALE: f64 = 4096.0;

    /// How far away a player can be and still be in this observer's world.
    ///
    /// The game's rule, from `ChunkMap.TrackedEntity.updatePlayer`: the
    /// distance is measured **horizontally**, x and z only, and the range is
    /// the lesser of the entity's own tracking range and the observer's view
    /// distance in chunks times sixteen.
    ///
    /// Both halves matter. Measuring vertically as well hides two players
    /// standing three blocks apart on a hillside, which is the whole point of
    /// being able to see who is on the ridge above you. And a fixed constant
    /// makes one player's render distance decide everybody's.
    fn visible_range(observer_view_chunks: i32) -> f64 {
        (observer_view_chunks.clamp(2, 32) * 16) as f64
    }

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
    /// How much world this client asked for, in chunks.
    ///
    /// Their own, and it is what bounds how far away another player stays in
    /// their world, which sounds backwards until it is phrased the game's way:
    /// the range is bounded by *their* view distance because it is their
    /// client that has to hold the body.
    view_chunks: i32,
    /// Whether they are crouching, so a key held down is not a pose sent over
    /// and over.
    crouching: bool,
    /// What this connection has been told about every other player, and where
    /// it was told they were.
    ///
    /// Per observer, and that is the whole point. A movement is sent as a
    /// distance rather than a place, so the server cannot keep one copy of
    /// "where they are": it has to keep one copy per pair, because a client
    /// joining halfway through somebody's walk has been told a different
    /// starting point than one that was there from the start.
    seen: std::collections::HashMap<u64, Seen>,
}

/// What one connection was last told about one other player.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seen {
    pos: (f64, f64, f64),
    yaw: f32,
    pitch: f32,
    /// Whether the body is currently in this observer's world at all.
    ///
    /// A missing entry is "this observer was never introduced to that player",
    /// and a present entry with this false is "we took it away again because
    /// they walked off", which needs putting back.
    visible: bool,
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
    /// The seat this player holds, given back when this value is dropped.
    ///
    /// The seat is taken at the login, because a push that arrives in the
    /// moment between the login and the play phase must not be lost. That means
    /// the seat has to be given back when the connection ends however it ends,
    /// and a connection that dies in the configuration phase never reaches the
    /// play phase at all: a client that cancels while the world is loading
    /// would take a seat with it, and the count of players would only ever go
    /// up.
    ///
    /// Underscored because it is never read: it is here to be dropped.
    _seat: Option<Seat>,
}

/// The place a player holds while they are connected.
///
/// Dropped with the registration, so the seat is released however the
/// connection ends and without anything having to remember to release it.
struct Seat {
    wire: std::sync::Arc<Broadcast>,
    id: u64,
    entity_id: i32,
    name: String,
}

impl Drop for Seat {
    fn drop(&mut self) {
        self.wire.remove(self.id);
        crate::server::log::note(format!(
            "player {:?} (entity {}) left, {} remaining",
            self.name,
            self.entity_id,
            self.wire.player_count()
        ));
    }
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
    ///
    /// The seat the player holds is released when the registration is dropped,
    /// so a connection that ends before the player is in the world still gives
    /// its place back.
    pub fn register(
        self: &std::sync::Arc<Self>,
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
            view_chunks: 8,
            crouching: false,
            seen: std::collections::HashMap::new(),
        });

        Registration {
            id,
            name: name.to_string(),
            uuid,
            entity_id,
            inbox,
            _seat: Some(Seat {
                wire: self.clone(),
                id,
                entity_id,
                name: name.to_string(),
            }),
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

    /// Tells everyone who can see them whether they are crouching.
    ///
    /// Only when it changes. The client's keys arrive twenty times a second and
    /// shift is held for as long as it is held, so a client that sends a pose on
    /// every input packet sends the same pose two hundred times a second to
    /// everybody, and a client that receives them redraws the player every time.
    pub fn set_crouching(&self, id: u64, crouching: bool) {
        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        let Some(sink) = sinks.iter_mut().find(|s| s.id == id) else {
            return;
        };
        if sink.crouching == crouching {
            return;
        }
        sink.crouching = crouching;
        let entity_id = sink.entity_id;

        let mut body = Vec::new();
        crate::protocol::play::write_entity_pose(&mut body, entity_id, crouching);

        for other in sinks.iter() {
            if other.id != id {
                let _ = other.out.send(body.clone());
            }
        }
    }

    /// Tells everyone who can see them that they swung.
    ///
    /// Their own client has already drawn it — it is the one that decided to —
    /// so this is the same shape as every other push here: to the others, never
    /// back to the sender.
    ///
    /// Sending it to the sender as well is not a duplicate animation, because
    /// the client's own click is what drew the first one, and this would be a
    /// second swing a fraction of a second later. It reads as the arm twitching.
    pub fn send_swing(&self, id: u64, hand: i32) {
        let sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entity_id) = sinks.iter().find(|s| s.id == id).map(|s| s.entity_id) else {
            return;
        };

        let mut body = Vec::new();
        crate::protocol::play::write_swing_animation(
            &mut body,
            entity_id,
            hand,
            crate::protocol::play::SWING_WHACK,
            crate::protocol::play::SWING_DURATION,
        );

        for sink in sinks.iter() {
            if sink.id != id {
                let _ = sink.out.send(body.clone());
            }
        }
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
            sink.seen.insert(subject, Seen { pos, yaw, pitch, visible: true });
        }
    }

    /// Records how much world a client asked for, which bounds how far away
    /// other players stay in its world.
    pub fn set_view_distance(&self, id: u64, chunks: i32) {
        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sink) = sinks.iter_mut().find(|s| s.id == id) {
            sink.view_chunks = chunks;
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

            // How far away this observer is from the player, **horizontally**.
            // The game measures x and z and ignores y, and that is the whole
            // difference between this and the obvious three-dimensional
            // version: measuring vertically as well hides two players standing
            // three blocks apart on a hillside.
            let range = visible_range(sink.view_chunks);
            let ex = subject_pos.0 - sink.pos.0;
            let ez = subject_pos.2 - sink.pos.2;
            let too_far = ex * ex + ez * ez > range * range;

            if too_far {
                // Take the body out of their world rather than stopping the
                // updates. Leaving it in place and going quiet looks the same
                // from here — no more packets — and completely different from
                // there: the client still holds the entity and keeps drawing
                // it, so the player stands frozen in mid-air for as long as
                // they care to walk away.
                if previous.visible {
                    let mut gone = Vec::new();
                    crate::protocol::play::write_remove_entities(&mut gone, &[subject_entity_id]);
                    deliveries.push((sink.id, gone));

                    if let Some(entry) = sink.seen.get_mut(&id) {
                        entry.visible = false;
                    }
                }
                continue;
            }

            if !previous.visible {
                // They walked back into range. Put them down again at where
                // they actually are, rather than sending a delta from the far
                // side of the world: the entry above holds where they were when
                // they left, and that is hundreds of blocks stale.
                let mut back = Vec::new();
                crate::protocol::play::write_add_entity(
                    &mut back,
                    subject_entity_id,
                    subject_uuid,
                    crate::protocol::play::PLAYER_ENTITY_TYPE,
                    subject_pos,
                    subject_yaw,
                    subject_pitch,
                    1,
                );
                deliveries.push((sink.id, back));

                // And the baseline moves with them, or the next delta is
                // measured from where they were when they left.
                sink.seen.insert(
                    id,
                    Seen {
                        pos: subject_pos,
                        yaw: subject_yaw,
                        pitch: subject_pitch,
                        visible: true,
                    },
                );
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
                    1,
                );
            }
            deliveries.push((sink.id, body));

            // The head, which the movement packet does not carry.
            //
            // move_entity_pos_rot has exactly two angles on it, yRot and xRot,
            // and the first of those is the body's. The head turns on its own
            // and is a separate packet for that reason: a player can look
            // sideways while walking in a straight line.
            if subject_yaw != previous.yaw {
                let mut head = Vec::new();
                crate::protocol::play::write_rotate_head(&mut head, subject_entity_id, subject_yaw);
                deliveries.push((sink.id, head));
            }

            sink.seen.insert(
                id,
                Seen {
                    pos: subject_pos,
                    yaw: subject_yaw,
                    pitch: subject_pitch,
                    visible: true,
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

    /// Sends one packet body to one player.
    ///
    /// For the answers that are nobody else's business: what a command
    /// replied, what the player themselves was told.
    pub fn send_one(&self, id: u64, body: Vec<u8>) {
        let sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        let Some(sink) = sinks.iter().find(|s| s.id == id) else {
            return;
        };
        // A send that fails means the player has gone, which is their
        // business and not the caller's.
        let _ = sink.out.send(body);
    }

    /// Sends one packet body to every player except `except`.
    ///
    /// `Vec<u8>` rather than a packet id and fields so that whatever builds it
    /// has already decided what it is; the receiver only has to frame it, which
    /// it does with its own encoder since compression is per connection.
    pub fn send_others(&self, except: u64, body: Vec<u8>) {
        let sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());

        // Who actually gets this, named.
        //
        // For the block packets this is the whole question: a block the breaker
        // sees go and a second player still standing in it means either this
        // loop never reached them or what they were sent did not decode. The
        // two are indistinguishable from the breaker's side of the world, so the
        // names go in the log where the difference is visible.
        if let Some(what) = describe_block_packet(&body) {
            let who: Vec<&str> = sinks
                .iter()
                .filter(|s| s.id != except)
                .map(|s| s.name.as_str())
                .collect();
            crate::server::log::note(format!(
                "broadcast packet {} ({} bytes) to [{}]",
                what,
                body.len(),
                if who.is_empty() { "nobody".into() } else { who.join(", ") }
            ));
        }

        for sink in sinks.iter().filter(|s| s.id != except) {
            // A send that fails means the player has gone; their sink is
            // removed when their connection unwinds, and one failed send is not
            // a reason to stop telling everybody else.
            let _ = sink.out.send(body.clone());
        }
    }

    /// Sends one packet body to every player, the sender included.
    ///
    /// Chat is the case that needs this: the person who typed a line sees it
    /// too, and everyone else sees it at the same time.
    pub fn send_all(&self, body: Vec<u8>) {
        let sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        for sink in sinks.iter() {
            // A send that fails means the player has gone; one failure is not
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

/// A readable line for a block packet, decoded back off the wire.
///
/// `None` for anything else, so that only the packets whose delivery is in
/// question end up in the log and the log stays readable during a break.
///
/// Reading the bytes back rather than being handed a description is the point:
/// a writer that builds the wrong packet still produces a right-looking line
/// here, and the log would then agree with the bug instead of showing it. What
/// is printed is what the other end will actually be handed.
pub(crate) fn describe_block_packet(body: &[u8]) -> Option<String> {
    use crate::protocol::{VarInt, play::id};

    let mut r = body;
    let (packet, _) = VarInt::read(&mut r).ok()?;

    // A position is eight bytes and is never a VarInt, so it has to be stepped
    // over rather than read: leaving it in place would have the next field read
    // out of the low end of the position instead.
    let position = |r: &[u8]| -> Option<(i32, i32, i32)> {
        let packed = i64::from_be_bytes(r.get(0..8)?.try_into().ok()?);
        Some(crate::protocol::play::unpack_block_pos(packed))
    };

    match packet.0 {
        id::BLOCK_UPDATE => {
            let (x, y, z) = position(r)?;
            r = r.get(8..)?;
            let (state, _) = VarInt::read(&mut r).ok()?;

            // The leftover count is printed because it is the check that matters:
            // a byte stranded after the state is a byte the client reads as the
            // next field of a packet it has already been given the end of.
            Some(format!(
                "block_update at {x} {y} {z} state {state} ({} bytes, {} left over)",
                body.len(),
                r.len()
            ))
        }
        id::BLOCK_CHANGED_ACK => {
            let (sequence, _) = VarInt::read(&mut r).ok()?;
            Some(format!("block_changed_ack sequence {sequence}"))
        }
id::LEVEL_EVENT => {
            // A plain four-byte int, not a VarInt. Reading it as a VarInt stops
            // at the first byte, which reports event 0 and a position seven
            // bytes out of step: a level_event at 8004 0 0 for a block broken
            // at 0 -7 1. A log that reads a packet wrongly sends whoever is
            // looking for the fault looking at the log instead.
            let event = i32::from_be_bytes(r.get(0..4)?.try_into().ok()?);
            let (x, y, z) = position(r.get(4..)?)?;
            Some(format!("level_event {event} at {x} {y} {z}"))
        }
        id::BLOCK_DESTRUCTION => {
            let entity = VarInt::read(&mut r).ok()?.0;
            let (x, y, z) = position(r)?;
            let stage = *r.get(9)?;
            Some(format!(
                "block_destruction entity {entity} at {x} {y} {z} stage {stage}"
            ))
        }
        _ => None,
    }
}

/// Announces that a player has gone, when their connection ends however it ends.
///
/// A guard rather than a call at the end of the connection, because the
/// connection has a great many ways out — a clean return, a read error, a write
/// that failed, an early return when the world would not answer — and a line of
/// cleanup written at the bottom of a function is reached by none of the others.
///
/// The seat the player held is not this guard's business: that is the
/// registration's, and the registration is dropped however the connection ends
/// and in whatever phase it was in.
pub struct Departure {
    wire: std::sync::Arc<Broadcast>,
    id: u64,
    entity_id: i32,
    uuid: [u8; 16],
    /// Whether the other players have been told about this one.
    ///
    /// A player who never reached the play phase was never introduced to
    /// anybody, so there is nobody to take them away from. Saying otherwise
    /// sends a removal for a body the others have never heard of.
    introduced: bool,
}

impl Departure {
    /// Arms the announcement. Nothing is sent until this value is dropped.
    pub fn arm(wire: std::sync::Arc<Broadcast>, reg: &Registration) -> Self {
        Self {
            wire,
            id: reg.id,
            entity_id: reg.entity_id,
            uuid: reg.uuid,
            introduced: false,
        }
    }

    /// The other players have been told about this one.
    ///
    /// Called when they are, so that a player who never got that far is not
    /// taken away from people who never knew of them.
    pub fn introduced(&mut self) {
        self.introduced = true;
    }
}

impl Drop for Departure {
    fn drop(&mut self) {
        // Only worth announcing to somebody. A server that just lost its first
        // player has nobody to tell, and building the packets for nobody is
        // work that grows with every join on an empty server.
        if self.introduced && self.wire.player_count() > 1 {
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
    }
}

#[cfg(test)]
mod tests;
