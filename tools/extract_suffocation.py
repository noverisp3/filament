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
    # is generated from the data generator's own report.
    blocks_rs = (ROOT / "src" / "generated" / "blocks.rs").read_text(encoding="utf-8")
    states: dict[str, int] = {}
    for name, default in re.findall(
        r'name: "minecraft:(\w+)",\s*\n\s*default_state: (\d+),', blocks_rs
    ):
        states[name] = int(default)

    missing = sorted(suffocating - states.keys())
    if missing:
        sys.exit(
            f"{len(missing)} blocks are in the tag but not in the block table: "
            f"{missing[:10]}"
        )

    ids = sorted(states[name] for name in suffocating)

    out = [
        "// @generated by tools/extract_suffocation.py from the game's own data.",
        "// Do not edit: regenerate with `python tools/extract_suffocation.py`.",
        "//",
        f"// {len(ids)} block states stop a player from breathing.",
        "//",
        "// These are block state ids, the same numbers the server is told when",
        "// it asks what is at a position.",
        "",
        "/// The block states that make a player suffocate, in order.",
        "pub static CAUSES_SUFFOCATION: &[u32] = &[",
    ]
    for i in range(0, len(ids), 20):
        out.append("    " + ", ".join(str(i) for i in ids[i : i + 20]) + ",")
    out.append("];")
    out.append("")
    out.append("/// Whether a block state makes a player suffocate.")
    out.append("///")
    out.append("/// A binary search, because the list is a thousand long and this is")
    out.append("/// asked once a tick for each player.")
    out.append("pub fn causes_suffocation(state: i32) -> bool {")
    out.append("    CAUSES_SUFFOCATION.binary_search(&(state as u32)).is_ok()")
    out.append("}")
    text = "\n".join(out) + "\n"

    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "suffocation.rs").write_text(text, encoding="utf-8")
    print(f"wrote {OUT / 'suffocation.rs'} ({len(text) / 1024:.0f} KB)")
    print(f"  {len(ids)} states, {len(leaves)} leaves excluded, {len(always)} always")


if __name__ == "__main__":
    main()
