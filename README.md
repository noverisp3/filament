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

Protocol and world:

- [x] Chunk storage in paletted sections
- [x] Skylight propagation across sections and chunks
- [x] Greedy meshing
- [x] Raycast, place and break
- [x] Block state table, generated from 26.3
- [x] Framing, compression, primitives
- [x] Status, so the server appears in the browser
- [x] Offline login
- [x] Configuration: feature flags, registries, tags
- [x] Play phase: spawn and world setup
- [x] Chunks sent to a client, generated on the fly
- [x] Chunks follow the player as they walk

Two players on the same server:

- [x] Breaking a block is seen by everyone else, in real time
- [x] Crack stages paced by the block's own hardness
- [x] Dust and sound on the break, and neither is doubled for the breaker
- [x] Players see each other, and each other's names
- [x] Movement, body rotation and head rotation
- [x] Bodies come and go with the observer's view distance
- [x] Playable over LAN with two clients

Still to do:

- [ ] Persistence — nothing survives a restart
- [ ] The tick loop, and the world clock (`set_time` is not written)
- [ ] Inventory, and placing blocks
- [ ] Mobs
- [ ] Authentication

576 tests. A client connects, negotiates, and gets a world of generated terrain:
grass on top, dirt under it, stone below, hills from two octaves of value noise.
A second client joins the same world and sees the first one walk about.

Two things are worth knowing about the terrain. It is generated on request and
nothing is stored, so it costs nothing to start and is forgotten on exit. And the
generator's five-block registry and the game's thirty-five thousand block states
are unrelated, so blocks are mapped between them on the way out.
