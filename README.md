# filament

A Minecraft server, written from scratch in Rust.

MIT licensed. See `LICENSE`.

Minecraft's own server ticks chunks, entities and light on a single thread. That
is the weakness this exists to fix, so everything here is built around one rule:

> **No work that can be parallel may run on the main thread.**

Mojang's game is a *reference only*. See `../mc-ref/README.md` for why none of its
code may be copied here. It is read to learn how a problem was solved, and the
answer is then written independently in Rust.

## Build

```powershell
cargo build
cargo test
cargo clippy --all-targets
```

The toolchain is pinned by `rust-toolchain.toml` to the GNU toolchain, because
this machine has no MSVC Build Tools. Delete that file if Visual Studio is ever
installed.

## Running it

```text
filament                # 0.0.0.0:25565, every interface
filament --verbose      # and log every packet
filament --addr 127.0.0.1:25565   # only this machine

FILAMENT_WORLD=/path/to/world      # load and save a real world folder
```

Every interface by default, with the address printed:

```text
0.0s  listening on 0.0.0.0:25565
0.0s  reachable on this network at 192.168.55.106:25565
0.0s  anyone who can reach the port can join as anyone: there is no sign-in yet
```

That middle line is the machine's own answer rather than a question for the user.
The last line is not ceremony: login is offline, so there is no authentication and
the UUID is derived from whatever name the client sends. Anyone who reaches the
port can join as any name, including one that is already taken.

**On Windows the firewall will usually block this first**, and the symptom when it
has not been done is a connection that times out rather than one that is refused:

```powershell
New-NetFirewallRule -DisplayName "filament 25565" -Direction Inbound `
  -Protocol TCP -LocalPort 25565 -Action Allow -Profile Private
```

Once, from an elevated prompt.

### A world folder

Point `FILAMENT_WORLD` at a folder the game wrote and the server opens it: the
ground is read before it is generated, and what players change is written back
when the server stops. The level file is edited, not replaced, so the seed,
the game rules, the spawn and the day of the year stay the game's own.

```text
set FILAMENT_WORLD=C:\Users\Me\world
filament
```

Without it the world is generated on the spot and lives only as long as the
process does.

## Milestones

Grouped by what they are about, not by when they were done.

### The world

- [x] Chunk storage in paletted sections
- [x] Sky light across sections and chunks
- [x] Block light: a lamp's pool, worked out over the whole world and
      relit around every edit, and crossing a chunk border without a seam
- [x] Greedy meshing
- [x] Raycasting, for the reach check
- [x] Breaking a block
- [x] Placing a block, from the item in hand
- [x] Block state table, generated from 26.3
- [x] Terrain from two octaves of value noise
- [x] Edits journaled per chunk, and replayed when it comes back

### The clock and the tick

- [x] A tick loop at twenty a second, on its own thread, with bounded catch-up
- [x] The game clock, and the world's own clock where the sun is
- [x] Scheduled ticks in a budgeted heap, ordered by when they are due
- [x] Neighbour updates scheduled rather than answered inline
- [x] What the world changes by itself, sent to everyone in range
- [x] Grass spreading to dirt that has grass under it and air over it

### Playing

- [x] Inventory contents, and the slot in hand
- [x] Click handling, so a picked-up block lands in the bag
- [x] Placing a block from the bag, and the stack coming down
- [x] Keep-alive, answered

### The wire

- [x] Framing, compression, primitives
- [x] Status, so the server appears in the browser
- [x] Offline login
- [x] Configuration: feature flags, registries, tags
- [x] Play phase: spawn and world setup

### Getting the world to a client

- [x] Chunks sent, and generated on the fly
- [x] Chunks follow the player as they walk
- [x] A chunk that is dropped and rebuilt keeps what was changed in it
- [x] A real world folder, read and written in the game's own format
- [x] Chunks sent at the rate the client asks for

### Two players on one server

- [x] Breaking a block, seen by everyone in real time
- [x] Crack stages paced by the block's own hardness
- [x] Dust and sound on the break
- [x] Players, and their names
- [x] Movement, body rotation and head rotation
- [x] Swinging and punching
- [x] Crouching, which in 26.3 is a pose rather than a flag
- [x] Bodies come and go with the observer's view distance
- [x] Chat, to everyone at once

### Running one

- [x] Every interface by default, with the address printed
- [x] Readable logs, and a clean Ctrl+C
- [x] Terrain built in parallel, so walking does not freeze a player
- [x] Every packet id checked against the data generator's report
- [x] The server's name in the debug screen, and a ping it answers
- [x] Movement checked rather than believed
- [x] The server list says who is on and how many

### Not yet

- [x] Persistence to disk. A world folder (`FILAMENT_WORLD`) is opened on
      startup; edits and generated chunks are written back on shutdown, and a
      restarted server continues from where it stopped. Only changed chunks are
      saved.
- [ ] Lighting. The sun and the lamps are done: a chunk works its light out
      when it arrives, an edit relights everything it reaches, and the client
      is told the sections that moved. Every block that gives off light lights,
      because a block read out of a world file is registered with the light the
      game gives it. Not done: the shape of a block (every one of them is
      drawn as a full cube, so a torch is a lantern-sized brick).
- [ ] Mobs, water, weather
- [ ] The rest of the inventory. The authority is there: the client says which
      slots changed and the server checks the totals against what the player
      was issued, so a stale count cannot leave the server holding the wrong
      block. Not done: the player's own chest, and the off hand.
- [ ] Authentication

The ways the world hurts a player are all here: a fall from too high, lava, a
cactus, the water, and being walled in. The numbers are the game's own, read
out of the game: three blocks of fall are free, lava takes four half-hearts a
second, a cactus takes one and then waits, the water gives you three hundred
ticks of air and then takes two half-hearts a tick, and a wall takes a heart
every half second.

937 tests. A client connects, negotiates, and gets a world: either one the
game saved, read out of its own folder, or one generated on the spot — grass
on top, dirt under it, stone below, hills from two octaves of value noise. A
second client joins the same world and sees the first one walk about, break and
place blocks, and have the change appear on both screens at once, and the two
of them can talk.

The world ticks at twenty a second. It counts the game clock, runs scheduled work
from a budgeted queue, and has one rule so far: dirt with grass beneath it and air
above it turns back to grass, which is what makes a dug hillside green again
instead of leaving a bare stripe across it. What the world changes by itself is
drained by the connections and sent to everyone in range, because a block that
turned to grass because something above it was mined is not one player's news.

Three things are worth knowing about the world. A chunk is thrown away when the
last player walks away from it, and what survives that is a journal of the
blocks players changed, replayed onto a chunk as it comes back. The generator's
five-block registry and the game's thirty-five thousand block states are
unrelated, so blocks are mapped between them on the way out. And a world folder
is **read, not replaced**: a server that wrote its own level file over the
game's would reset a player's world without saying so.

The light is the world's own, and it is the part that took longest to get
right. The client's light engine numbers its sections from *one below* the
world, so the world's bottom section is mask bit one and the padding section
underneath it is bit zero; a server that counts from the world's bottom puts
every section of light sixteen blocks below where it belongs. And a lamp's pool
has to cross the border between two chunks without a seam, which means working
the light out over a region that is not aligned to the chunks in it. Both were
wrong at some point, and both showed up as a lamp that lit the wrong place, or
as a strip of darkness across the middle of a chunk.
