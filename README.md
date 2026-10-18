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

## Milestones

Grouped by what they are about, not by when they were done.

### The world

- [x] Chunk storage in paletted sections
- [x] Sky light across sections and chunks
- [x] Greedy meshing
- [x] Raycasting, for the reach check
- [x] Breaking a block
- [x] Block state table, generated from 26.3
- [x] Terrain from two octaves of value noise

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

### Two players on one server

- [x] Breaking a block, seen by everyone in real time
- [x] Crack stages paced by the block's own hardness
- [x] Dust and sound on the break
- [x] Players, and their names
- [x] Movement, body rotation and head rotation
- [x] Swinging and punching
- [x] Crouching, which in 26.3 is a pose rather than a flag
- [x] Bodies come and go with the observer's view distance

### Running one

- [x] Every interface by default, with the address printed
- [x] Readable logs, and a clean Ctrl+C
- [x] Terrain built in parallel, so walking does not freeze a player
- [x] Every packet id checked against the data generator's report

### Not yet

- [ ] Persistence to disk. Edits survive a chunk being dropped; they do not
      survive a restart, and that is the one a player notices.
- [ ] The tick loop, and the world clock
- [ ] Inventory, and placing blocks
- [ ] Mobs
- [ ] Authentication

612 tests. A client connects, negotiates, and gets a world of generated terrain:
grass on top, dirt under it, stone below, hills from two octaves of value noise.
A second client joins the same world and sees the first one walk about.

Two things are worth knowing about the terrain. It is generated on request, and a
chunk is thrown away when the last player walks away from it, so nothing about it
survives being dropped — what does survive is a journal of the blocks players have
changed, replayed onto a chunk as it comes back. And the generator's five-block
registry and the game's thirty-five thousand block states are unrelated, so blocks
are mapped between them on the way out.
