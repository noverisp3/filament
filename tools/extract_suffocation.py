#!/usr/bin/env python3
"""Generates src/generated/suffocation.rs from the game's own data.

The game asks `BlockState.isSuffocating`, which is one of three things:

- the default, which is whether the block is in the `minecraft:causes_suffocation`
  tag: the blocks that stop a player's head, which is stone and wood and wool
- `Blocks::never`, which is the blocks that do not: glass, water, and the leaves
- `Blocks::always`, which is the blocks that do even though the tag leaves them
  out: farmland and soul sand and mud and the dirt path

The tag is read out of the game's own data directory, which is where the game
reads it from, and the two exceptions are read out of the game's own source,
which is where the game writes them. Reading either from somewhere else would
be reading a different version of the game: the tag file in this repository's
`extracted/` is from an older capture and stops at state 1278, while wool is
state 2409 now.

Usage:
    python tools/extract_suffocation.py
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


def exceptions(source: str) -> tuple[set[str], set[str]]:
    """The blocks that never and always stop a player from breathing.

    They are written as `isSuffocating(Blocks::never)` on the block they
    belong to, so the block is the one named just above.
    """
    never: set[str] = set()
    always: set[str] = set()
    lines = source.splitlines()
    for i, line in enumerate(lines):
        match = re.search(r"isSuffocating\(Blocks::(never|always)\)", line)
        if not match:
            continue
        for j in range(i, -1, -1):
            named = re.search(r"public static final Block (\w+) =", lines[j])
            if named:
                target = never if match.group(1) == "never" else always
                # The source writes the block in the game's own constant, which
                # is `FARMLAND` where the block is `minecraft:farmland`.
                target.add(named.group(1).lower())
                break
    return never, always


def main() -> None:
    if not TAGS.exists():
        sys.exit(
            f"missing {TAGS}\n"
            "the game's own data is needed: the tag that says which blocks stop a\n"
            "player from breathing is in the decompiled server, and reading it from\n"
            "anywhere else would be reading a different version of the game"
        )

    tags = load_tags()
    suffocating = resolve("causes_suffocation", tags)

    # The leaves are in the tag but do not stop a player: the game writes them
    # with `Blocks::never`, and every leaf is written that way.
    leaves = resolve("leaves", tags)
    suffocating -= leaves

    source = BLOCKS.read_text(encoding="utf-8") if BLOCKS.exists() else ""
    never, always = exceptions(source) if source else (set(), set())
    suffocating -= never
    suffocating |= always

    # The game asks two questions and both have to be yes: the block is not air,
    # and it is in the tag. Air is in the tag, because the tag says which blocks
    # a player cannot walk through, and air is a block a player can walk
    # through.
    #
    # Air is dropped by name rather than by id, because the state that is not
    # air is the one that matters and the rest of air's states are the same
    # block.
    suffocating = {name for name in suffocating if name != "air"}

    if not suffocating:
        sys.exit("no blocks stop a player from breathing, which cannot be right")

    # The state ids come from the block table this repository already has, which
    # is generated from the data generator's own report. Every state of a block
    # is counted, not just its default one: a log turned sideways is a
    # different state and it is still a log, and a player walled in by one
    # suffocates just the same.
    blocks_rs = (ROOT / "src" / "generated" / "blocks.rs").read_text(encoding="utf-8")
    states: dict[str, list[int]] = {}
    for match in re.finditer(
        r'name: "minecraft:(\w+)",\s*\n\s*default_state: (\d+),\s*\n\s*states: &\[(.*?)\],\s*\n\s*\},',
        blocks_rs,
        re.S,
    ):
        states[match.group(1)] = [int(i) for i in re.findall(r"id: (\d+)", match.group(3))]
    if not states:
        sys.exit("no block states found in the block table")

    counted = re.search(r"pub const BLOCK_STATE_COUNT: u32 = (\d+);", blocks_rs)
    if not counted:
        sys.exit("the block table does not say how many states there are")
    state_count = int(counted.group(1))

    missing = sorted(suffocating - states.keys())
    if missing:
        sys.exit(
            f"{len(missing)} blocks are in the tag but not in the block table: "
            f"{missing[:10]}"
        )

    ids: list[int] = []
    for name in suffocating:
        ids.extend(states[name])
    ids.sort()

    # One bit per block state, rather than a list of numbers: the list of
    # thirty thousand numbers is four hundred kilobytes of binary and the
    # question being asked is a single bit.
    words = (state_count + 63) // 64
    packed = [0] * words
    for i in ids:
        packed[i // 64] |= 1 << (i % 64)

    out = [
        "// @generated by tools/extract_suffocation.py from the game's own data.",
        "// Do not edit: regenerate with `python tools/extract_suffocation.py`.",
        "//",
        f"// {len(ids)} block states stop a player from breathing.",
        "//",
        "// These are block state ids, the same numbers the server is told when",
        "// it asks what is at a position.",
        "",
        "/// The block states that make a player suffocate.",
        "///",
        "/// One bit per block state, so the question is a single bit.",
        "pub static CAUSES_SUFFOCATION: &[u64] = &[",
    ]
    for i in range(0, len(packed), 6):
        out.append("    " + ", ".join(f"0x{w:x}" for w in packed[i : i + 6]) + ",")
    out.append("];")
    out.append("")
    out.append("/// Whether a block state makes a player suffocate.")
    out.append("///")
    out.append("/// A bit in a table, because this is asked once a tick for each")
    out.append("/// player and each of the blocks around their head.")
    out.append("pub fn causes_suffocation(state: i32) -> bool {")
    out.append("    if state < 0 {")
    out.append("        return false;")
    out.append("    }")
    out.append("    let index = state as usize;")
    out.append("    let word = index / 64;")
    out.append("    if word >= CAUSES_SUFFOCATION.len() {")
    out.append("        return false;")
    out.append("    }")
    out.append("    CAUSES_SUFFOCATION[word] & (1u64 << (index % 64)) != 0")
    out.append("}")
    text = "\n".join(out) + "\n"

    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "suffocation.rs").write_text(text, encoding="utf-8")
    print(f"wrote {OUT / 'suffocation.rs'} ({len(text) / 1024:.0f} KB)")
    print(f"  {len(ids)} states, {len(leaves)} leaves excluded, {len(always)} always")


if __name__ == "__main__":
    main()
