#!/usr/bin/env python3
"""Generates src/generated/heightmaps.rs from the game's own data.

The client is told three heightmaps for every chunk it is sent, and it uses
them without checking: the client copies the numbers it is given into its own
heightmap and only works one out for itself when the server sent none. So the
numbers have to be right, and this is where they come from.

The game asks three questions about a column of blocks:

- `WORLD_SURFACE`: the height of the first block that is not air. The game's
  test is `!state.isAir()`, and the blocks that are air are the three written
  with the `air` property.
- `MOTION_BLOCKING`: the height of the first block that stops a player, which
  is the `minecraft:blocks_motion_in_heightmap` tag, or any block with water
  or lava in it.
- `MOTION_BLOCKING_NO_LEAVES`: the same, but the `minecraft:blocks_motion`
  tag without its leaves.

The tags are read out of the game's own data directory, which is where the game
reads them from, and the blocks that count as air and the blocks that hold a
fluid are read out of the game's own source, which is where the game writes
them. Reading either from anywhere else would be reading a different version
of the game.

Usage:
    python tools/extract_heightmaps.py
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MC_REF = ROOT.parent / "mc-ref" / "decompiled" / "server"
TAGS = MC_REF / "data" / "minecraft" / "tags" / "block"
BLOCKS = MC_REF / "net" / "minecraft" / "world" / "level" / "block" / "Blocks.java"
BLOCK_IDS = MC_REF / "net" / "minecraft" / "resources" / "ResourceKey.java"
OUT = ROOT / "src" / "generated"

NAMESPACE = "minecraft:"


def load_tags() -> dict[str, list[str]]:
    """Every tag in the game's data, by its name without the namespace."""
    tags: dict[str, list[str]] = {}
    for path in TAGS.glob("*.json"):
        with path.open(encoding="utf-8") as f:
            tags[path.stem] = json.load(f).get("values", [])
    return tags


def resolve(name: str, tags: dict[str, list[str]], seen: set[str] | None = None) -> set[str]:
    """The block names a tag holds, following the tags it names."""
    seen = seen or set()
    if name in seen:
        return set()
    seen.add(name)

    out: set[str] = set()
    for value in tags.get(name, []):
        if value.startswith("#"):
            out |= resolve(value.removeprefix("#").removeprefix(NAMESPACE), tags, seen)
        else:
            out.add(value.removeprefix(NAMESPACE))
    return out


def id_constants() -> dict[str, str]:
    """The game's own names for its blocks, by the constant that holds them.

    The source writes a block's name in a constant, and the constant is
    `WATER` where the block is `minecraft:water`, so the names have to be
    looked up rather than guessed.
    """
    out: dict[str, str] = {}
    for path in MC_REF.rglob("*.java"):
        if path.name not in ("BlockIds.java", "BlockItemIds.java"):
            continue
        source = path.read_text(encoding="utf-8")
        for name, value in re.findall(r'public static final \w+ (\w+) = \w+\.create\("(\w+)"\)', source):
            out[name] = value
    return out


def registered_blocks(source: str, marker: str) -> set[str]:
    """The blocks the source registers with a given property.

    The property is written inside the `register` call, so the block is the
    one named a few lines above it.
    """
    out: set[str] = set()
    lines = source.splitlines()
    for i, line in enumerate(lines):
        if marker not in line:
            continue
        for j in range(i, -1, -1):
            named = re.search(r"public static final Block (\w+) =", lines[j])
            if named:
                out.add(named.group(1))
                break
    return out


def state_ids(blocks_rs: str) -> dict[str, list[int]]:
    """Every state id of every block, by the block's name.

    The tag says which *blocks* stop a player, and every state of a block is
    that block, so all of a block's states are in or all of them are out. The
    default state alone is not enough: a log turned sideways is a different
    state and it is still a log.
    """
    out: dict[str, list[int]] = {}
    for match in re.finditer(
        r'name: "minecraft:(\w+)",\s*\n\s*default_state: (\d+),\s*\n\s*states: &\[(.*?)\],\s*\n\s*\},',
        blocks_rs,
        re.S,
    ):
        name, _default, body = match.groups()
        out[name] = [int(i) for i in re.findall(r"id: (\d+)", body)]
    return out


def write_table(
    out: list[str],
    name: str,
    doc: str,
    ids: list[int],
    note: str,
    state_count: int,
) -> None:
    """Writes a table as a bitset over the state ids.

    A list of thirty thousand numbers is four hundred kilobytes of source and
    four hundred kilobytes of binary. A bit per state is four kilobytes, and
    the question being asked is a single bit: is this state in the table.
    """
    words = (state_count + 63) // 64
    packed = [0] * words
    for i in ids:
        packed[i // 64] |= 1 << (i % 64)

    out.append(f"/// {doc}")
    out.append("///")
    out.append(f"/// {note}")
    out.append(f"///")
    out.append(f"/// One bit per block state, {len(ids)} of the {state_count} states set.")
    out.append(f"pub static {name}: &[u64] = &[")
    for i in range(0, len(packed), 6):
        out.append("    " + ", ".join(f"0x{w:x}" for w in packed[i : i + 6]) + ",")
    out.append("];")
    out.append("")


def main() -> None:
    if not TAGS.exists() or not BLOCKS.exists():
        sys.exit(
            f"missing {TAGS} or {BLOCKS}\n"
            "the game's own data is needed: the tags that say which blocks stop\n"
            "a player are in the decompiled server, and reading them from\n"
            "anywhere else would be reading a different version of the game"
        )

    tags = load_tags()
    ids_by_name = id_constants()
    source = BLOCKS.read_text(encoding="utf-8")

    # The tag the game asks for is `blocks_motion_in_heightmap`, which is
    # `blocks_motion`, which is the same tag without its leaves.
    without_leaves = resolve("blocks_motion_no_leaves", tags)
    leaves = resolve("leaves", tags)
    motion = without_leaves | leaves

    # A block with water or lava in it stops a player even when the tag leaves
    # it out, because the game asks about the fluid as well as the block.
    fluid_constants = registered_blocks(source, ".liquid()")
    fluids = {ids_by_name.get(c, c.lower()) for c in fluid_constants}

    # Air is written with the `air` property, and those are the blocks the
    # game asks `isAir` about.
    air_constants = registered_blocks(source, ".air()")
    airs = {ids_by_name.get(c, c.lower()) for c in air_constants}

    blocks_rs = (ROOT / "src" / "generated" / "blocks.rs").read_text(encoding="utf-8")
    states = state_ids(blocks_rs)
    if not states:
        sys.exit("no block states found in the block table")

    counted = re.search(r"pub const BLOCK_STATE_COUNT: u32 = (\d+);", blocks_rs)
    if not counted:
        sys.exit("the block table does not say how many states there are")
    state_count = int(counted.group(1))

    def collect(names: set[str], what: str) -> list[int]:
        missing = sorted(names - states.keys())
        if missing:
            sys.exit(
                f"{len(missing)} blocks are in {what} but not in the block table: "
                f"{missing[:10]}"
            )
        out: list[int] = []
        for name in names:
            out.extend(states[name])
        return sorted(out)

    motion_ids = collect(motion, "the motion tag")
    without_leaves_ids = collect(without_leaves, "the motion tag without leaves")
    fluid_ids = collect(fluids, "the fluid blocks")
    air_ids = collect(airs, "the air blocks")

    # A fluid block is in both of the game's motion heightmaps, whether or not
    # the tag says so.
    motion_ids = sorted(set(motion_ids) | set(fluid_ids))
    without_leaves_ids = sorted(set(without_leaves_ids) | set(fluid_ids))

    out = [
        "// @generated by tools/extract_heightmaps.py from the game's own data.",
        "// Do not edit: regenerate with `python tools/extract_heightmaps.py`.",
        "//",
        f"// {len(motion_ids)} states stop a player, {len(without_leaves_ids)} of them",
        "// without counting the leaves.",
        "//",
        "// These are block state ids, the same numbers the server is told when",
        "// it asks what is at a position.",
        "",
    ]

    write_table(
        out,
        "MOTION_BLOCKING",
        "The block states that stop a player, leaves included.",
        motion_ids,
        f"{len(fluids)} of the blocks hold a fluid, which stops a player on its own.",
        state_count,
    )
    write_table(
        out,
        "MOTION_BLOCKING_NO_LEAVES",
        "The block states that stop a player, leaves left out.",
        without_leaves_ids,
        "The leaves are what the two tables differ on.",
        state_count,
    )
    write_table(
        out,
        "AIR",
        "The block states that are air.",
        air_ids,
        "Three blocks are air, and the game asks about all of them.",
        state_count,
    )

    out.append("/// Whether a block state stops a player.")
    out.append("///")
    out.append("/// A bit in a table, because this is asked once for every block of")
    out.append("/// every column of every chunk that is sent.")
    out.append("pub fn motion_blocking(state: i32) -> bool {")
    out.append("    is_set(MOTION_BLOCKING, state)")
    out.append("}")
    out.append("")
    out.append("/// Whether a block state stops a player, leaves left out.")
    out.append("pub fn motion_blocking_no_leaves(state: i32) -> bool {")
    out.append("    is_set(MOTION_BLOCKING_NO_LEAVES, state)")
    out.append("}")
    out.append("")
    out.append("/// Whether a block state is air.")
    out.append("pub fn is_air(state: i32) -> bool {")
    out.append("    is_set(AIR, state)")
    out.append("}")
    out.append("")
    out.append("/// Whether a bit of a table is set for a state.")
    out.append("///")
    out.append("/// A state outside the table is not in it, and a state inside the")
    out.append("/// table but past its end is not in it either.")
    out.append("fn is_set(table: &[u64], state: i32) -> bool {")
    out.append("    if state < 0 {")
    out.append("        return false;")
    out.append("    }")
    out.append("    let index = state as usize;")
    out.append("    let word = index / 64;")
    out.append("    if word >= table.len() {")
    out.append("        return false;")
    out.append("    }")
    out.append("    table[word] & (1u64 << (index % 64)) != 0")
    out.append("}")
    out.append("")

    text = "\n".join(out) + "\n"
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "heightmaps.rs").write_text(text, encoding="utf-8")
    print(f"wrote {OUT / 'heightmaps.rs'} ({len(text) / 1024:.0f} KB)")
    print(f"  {len(motion_ids)} motion, {len(without_leaves_ids)} without leaves")
    print(f"  {len(fluid_ids)} fluid states, {len(air_ids)} air states")
    print(f"  leaves: {len(leaves)}")


if __name__ == "__main__":
    main()
