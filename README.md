# filament

A Minecraft server, written from scratch in Rust.

MIT licensed. See `LICENSE`.

Minecraft's own server ticks chunks, entities and light on a single thread. That
is the weakness this exists to fix, so everything here is built around one rule:

> **No work that can be parallel may run on the main thread.**

The name is `filament`: a thin strand, a thread running through something. It was
picked after checking crates.io, where the short evocative names were already
gone — `lattice`, `flurry`, `stratum`, `strata`, `tessera`, `cairn`, `flint`,
`shard`, `spindle`, `weft`, `woven`, `fila`, `ferrum`, `ferrite`, `ferrox` and
`dream` all turned out to be taken. Which is a decent summary of the namespace.

Mojang's game is a *reference only*. See `../mc-ref/README.md` for why none of
its code may be copied here. It is read to learn how a problem was solved, and
the answer is then written independently in Rust.

## Where things are

```
src/
  lib.rs           crate root and the one rule above
  world/           chunk storage, lighting, coordinates. Pure std.
  render/
    mesh.rs        greedy meshing. Pure std, imports nothing from wgpu.
```

`world/` has no dependencies outside the standard library, and that is
deliberate. Storage and light propagation are the parts worth being able to
unit test with no window, no GPU and no network, and an engine that claims to be
multithreaded should not be holding a render device from its storage layer.

`mesh.rs` lives under `render/` but has no business there: it never imports
wgpu. It is kept where it is only because the history predates the decision to
make this a server. A server does not mesh chunks; since 1.8 the client does.

## Storage layout

Fixed early and expensive to change later.

- A chunk is 16x384x16, split into 24 vertical **sections** of 16³ blocks.
  y runs from -64 to 320, which is Minecraft's own range.
- Each section's blocks live in one `Paletted`: a bit-packed array of palette
  indices plus the palette itself. A uniform section spends **zero** dense
  bytes.
- Entry width grows only when the palette outgrows it, so widening is amortised
  O(1) per write.
- Light is deliberately **not** in the palette. It changes far more often than
  block ids, so folding it in would blow the palette up and destroy the
  compression. It lives in `world/light.rs` as two independent layers.

SoA/AoS has two different answers, and mixing them up is the easy mistake:

- **Chunk storage is SoA at layer granularity.** Blocks, light and biomes are
  separate arrays because the hot operations genuinely differ: a skylight sweep
  walks one attribute across thousands of blocks and would be ruined by
  interleaving it with the others.
- **The vertex buffer is interleaved**, because the GPU reads all of a vertex's
  attributes together.

## Build

```powershell
cargo build
cargo test
cargo clippy --all-targets
```

The toolchain is pinned by `rust-toolchain.toml` to the GNU toolchain, because
this machine has no MSVC Build Tools and the GNU toolchain links against the
MinGW-w64 runtime in w64devkit. Delete that file if Visual Studio is ever
installed.

## Progress

| | |
|---|---|
| Chunk storage, paletted sections | done |
| Skylight propagation across sections and chunks | done |
| Greedy meshing | done |
| Raycast, place and break | done |
| Block state table, generated from 26.3 | done |
| Framing, compression, primitives | done |
| Status, so the server appears in the browser | done |
| Offline login | done |
| Configuration: feature flags, registries, tags | done |
| Play phase: spawn and world setup | done |
| Chunks sent to a client | done |
| Terrain in those chunks | done, generated on the fly |
| Persistence | **not started** |
| Entities, ticking, the tick loop | **not started** |

241 tests. A client connects, negotiates, and gets nine chunks of generated
terrain: grass on top, dirt under it, stone below, hills from two octaves of
value noise. The world is generated on request and nothing is stored, so it costs
nothing to start and is forgotten on exit.

Two things about it are worth knowing. The generator's five-block registry and
the game's thirty-five thousand block states are unrelated, so blocks are mapped
between them on the way out; and a palette with more than one entry is stored at
four bits an entry even though four blocks would fit in two, because that is what
the game does and the reader takes the layout from that byte.

### Running it

```text
filament                # 127.0.0.1:25565
filament --verbose      # and log every packet
filament --addr 0.0.0.0:25565
```

Loopback by default on purpose. A server that is not finished should not be
reachable from the network by accident.

`--addr 0.0.0.0:25565` is the switch, and the startup log then says what to hand
out:

```text
0.0s  listening on 0.0.0.0:25565
0.0s  reachable on this network at 192.168.55.106:25565
0.0s  anyone who can reach the port can join as anyone: there is no sign-in yet
```

That middle line is the machine's own answer rather than a question for the
user: it binds a UDP socket, connects it to a documentation address so the
kernel picks a route, and reads back the local end. Nothing is sent.

The warning is not ceremony. Login is offline, so there is no authentication and
the UUID is derived from whatever name the client sends. Anyone who reaches the
port can join as any name, including yours. On a network you control that is a
testing convenience; do not point it at the open internet.

**On Windows the firewall will usually block this first.** It needs a rule for
the port, and the rule has to allow private networks rather than public ones:

```powershell
New-NetFirewallRule -DisplayName "filament 25565" -Direction Inbound `
  -Protocol TCP -LocalPort 25565 -Action Allow -Profile Private
```

Once, from an elevated prompt. Nothing in the server can do this for itself, and
the symptom when it has not been done is a connection that times out rather than
one that is refused.

Packet logging is off by default. A client waiting on chunks sends an empty

Timestamps are how long the server has been up, not the wall clock:

```text
    0.0s  filament 0.1.0 | debug | x86_64-pc-windows-gnu
    0.0s  commit    a30d052+dirty
    0.0s  built     2026-10-17T18:45:55Z (UTC) by rustc 1.99.1 (58cf8f9c3 2026-10-13)
    0.0s  listening on 127.0.0.1:25565
    1.4s  [conn 4] profile noverisp3 as 7bb85004-..., sending the profile
   12.4s  [conn 4] sent 289 chunks around the spawn
   42.6s  [conn 4] client closed during play
```

Epoch milliseconds is thirteen digits of which a reader wants none: the useful
number is the difference between two lines, and it should not be a subtraction in
the reader's head.

Three things in that banner are worth knowing, each of which cost a confusing
minute to somebody other than the person who wrote it:

- **`+dirty`** means the binary was built from a tree with uncommitted changes,
  so it and the repository are not the same thing.
- **The build time is UTC**, labelled as such, while the clock on the wall is
  local. It is UTC because a build stamp is an instant: printed local, the same
  commit built in Bangkok and in Berlin would carry different strings.
- **There are two hashes and only one of them is this project's.** `a30d052` is
  filament's commit. `58cf8f9c3` is the Rust compiler's own commit, with its
  release date beside it, and it matters when a bug turns out to be the compiler's.

`build.rs` lists `src`, `Cargo.toml` and itself as its rerun inputs. Any
`rerun-if-changed` switches Cargo to tracking only the paths listed, so leaving
the list to the default is not the same as leaving it alone: a banner that
reports a stale commit is worse than no banner.

Ctrl+C stops the server between connections rather than killing it mid-write. The
console handler sets a flag and the accept loop notices it at its next poll, which
is why the listener is non-blocking. Connections that are still open are cut rather
than waited for: there is no way yet to ask one to finish, and joining would hang
for as long as anybody stayed connected.

### Where the protocol data came from

#### Read every packet's own codec

Nearly every packet bug in this project was the same mistake: taking the shape of
a neighbouring packet and assuming it carried over. Three so far, each of which
decoded cleanly here and was rejected by the game:

- `accept_teleportation` written with one field. The game reads six, and says so
  by hanging up.
- `block_update` with its position as a VarLong. A block position is a fixed
  eight byte long, so a VarLong is one byte and the client runs out of packet.
- `forget_level_chunk` with two VarInts for the chunk coordinate. It is a
  `ChunkPos`, which is one long with x in the low half.

The neighbours are all different, which is exactly why they are dangerous:

| Packet | Coordinate encoding |
|---|---|
| `level_chunk_with_light` | two `int32` |
| `set_chunk_cache_center` | two `VarInt` |
| `block_update` | one packed long, 26/12/26 |
| `forget_level_chunk` | one long, x low and z high |

The rule that follows: open the packet's own class in the decompiled source and
read its `STREAM_CODEC` before writing it. Do not infer it from a packet in the
same family, and do not infer it from the id.

#### Three tables

Three tables are captured or generated rather than written by hand, and it is
worth saying why, because in two cases the data generator's output does not
contain what is needed and guessing would have been wrong:

| | |
|---|---|
| `src/generated/blocks.rs` | from `--reports`, the generator does emit it |
| `assets/update_tags.bin` | captured: tag contents are registry *indices* and nothing reports those |
| `assets/registry_data.bin` | captured: same reason, and the client needs the contents to build the registries the tags refer to |

Regenerate the last two with `tools/capture.py` and `tools/extract_registries.py`
against a stock 26.3 server. The captures are committed, so regenerating is an
option rather than a requirement.