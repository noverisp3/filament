# filament

A Minecraft server, written from scratch in Rust.

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
| Protocol layer, targeting 26.3 | **not started** |

117 tests. Nothing is connected to anything yet: `world/` and `mesh.rs` are
complete and tested, but there is no network layer, so no client can connect.