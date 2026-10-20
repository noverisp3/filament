"""Generates src/generated/light.rs from the decompiled 26.3 source.

How bright a block is comes from the block's own definition:

    GLOWSTONE = register(BlockItemIds.GLOWSTONE, ...Properties.of()...lightLevel(state -> 15)...);
    TORCH     = register(BlockItemIds.TORCH, ...Properties.of()...lightLevel(state -> 14)...);
    REDSTONE_LAMP = register(..., ...Properties.of()...lightLevel(litBlockEmission(13))...);

`lightLevel(...)` takes a function of the block state, because some blocks are
bright only in one of their states: a redstone lamp is bright when it is lit
and dark when it is not. The table here is the brightest a block ever gets,
which is what a server that does not read a block's state can do.

The names come from `BlockItemIds`, which is where the game keeps the
identifier of every block: `GLOWSTONE = BlockItemId.create("glowstone")`.

This is the same arrangement as the hardness table, and for the same reason:
the numbers are produced by a tool that can be re-run rather than typed into a
server file where nobody could check them. A hand-written list of lamp names
is a list that is always missing something, and a world with a lamp in it that
the server does not know about is a world with a dark lamp in it.
"""

import os
import re
import sys

ROOT = r"C:\Users\Nguyen\Desktop\Projects\mc-ref\decompiled\server\net\minecraft"
BLOCKS = os.path.join(ROOT, "world", "level", "block", "Blocks.java")
ITEM_IDS = os.path.join(ROOT, "references", "BlockItemIds.java")
BLOCK_IDS = os.path.join(ROOT, "references", "BlockIds.java")
OUT = os.path.join(
    r"C:\Users\Nguyen\Desktop\Projects\filament", "src", "generated", "light.rs"
)


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def block_names():
    """The constant of every block, and the name it stands for.

    Two classes hold them: `BlockItemIds` for the blocks that have an item and
    `BlockIds` for the ones that do not, which is where the wall torches are.
    """
    names = {}
    for path in (ITEM_IDS, BLOCK_IDS):
        if not os.path.exists(path):
            continue
        for constant, name in re.findall(
            r'BlockItemId (\w+) = BlockItemId\.create\("([a-z0-9_]+)"\)', read(path)
        ):
            names[constant] = name
        # The blocks that have no item are a resource key instead.
        for constant, name in re.findall(
            r'ResourceKey<Block> (\w+) = create\("([a-z0-9_]+)"\)', read(path)
        ):
            names[constant] = name
    return names


def entries():
    """The registration of every block, and the text that goes with it.

    The text of one registration runs to the start of the next, which is how
    they are told apart: some of them are one line long and some are a dozen,
    and a rule that expects a shape will pair one block's name with another
    block's light.
    """
    text = read(BLOCKS)
    out = []
    parts = re.split(r"public static final Block ", text)[1:]
    for part in parts:
        constant = part.split(" ", 1)[0].split("=")[0].strip()
        out.append((constant, part))
    return out


# Some blocks keep their light in a helper instead of in the registration.
# The candles are made by a function that says
# `lightLevel(CandleBlock.LIGHT_EMISSION)`, which is three per candle and so
# fifteen at the most -- the brightest a candle ever is.
HELPERS = {
    "candleProperties": 15,
}


def light_of(body):
    """The light level in a registration, and the block it comes from."""
    match = re.search(r"\.lightLevel\(\s*(?:state ->\s*)?(\d+)\s*\)", body)
    if match:
        return int(match.group(1)), None
    match = re.search(r"\.lightLevel\(litBlockEmission\((\d+)\)\)", body)
    if match:
        return int(match.group(1)), None
    for helper, value in HELPERS.items():
        if helper + "(" in body:
            return value, None
    # The shapes that derive from another block: a slab is as bright as the
    # block it is made of, so the base is looked up rather than parsed.
    match = re.search(r"= register\w*\(\s*BlockItemIds\.\w+\s*,\s*(\w+)\s*\)", body)
    if match:
        return None, match.group(1)
    return None, None


def main():
    names = block_names()
    if not names:
        print("no block names found in BlockItemIds", file=sys.stderr)
        return 1

    # The blocks that say how bright they are, and the blocks that are made of
    # another block and are as bright as it.
    light = {}
    derived = []
    for constant, body in entries():
        name = names.get(constant)
        if name is None:
            continue
        value, base = light_of(body)
        if value is None:
            if base is not None:
                derived.append((name, base))
            continue
        light[name] = value

    # And then the ones made of another block, which need the first pass to
    # have run.
    for name, base in derived:
        value = light.get(base)
        if value:
            light[name] = value

    # The coloured candles are registered as a collection of sixteen blocks
    # rather than one at a time, so their names are not in the source at all.
    # They are the same candle in sixteen colours, and the game calls them
    # after the dye.
    for colour in (
        "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink",
        "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red",
        "black",
    ):
        light[f"{colour}_candle"] = 15

    if not light:
        print("no light levels found in Blocks", file=sys.stderr)
        return 1

    lines = [
        "//! How bright each block is, generated from the decompiled source.",
        "//!",
        "//! Generated by `tools/extract_light.py`. Do not edit by hand: re-run",
        "//! the tool against a fresh decompile.",
        "//!",
        "//! A block's light is the level of its own definition, and the game keeps",
        "//! it in `Blocks.java` as a `lightLevel(...)` call. The value here is the",
        "//! brightest the block ever gets: some blocks are bright only in one of",
        "//! their states, and this is the state they are brightest in.",
        "//!",
        "//! The names come from `BlockItemIds`, where the game keeps the identifier",
        "//! of every block.",
        "//!",
        "//! This is the same arrangement as the hardness table, and for the same",
        "//! reason: a hand-written list of lamp names is a list that is always",
        "//! missing something, and a lamp the server does not know about is a",
        "//! lamp the server does not light.",
        "",
        "/// Every block that gives off light, and how much of it.",
        "pub const LIGHT: &[(&str, u8)] = &[",
    ]
    for name in sorted(light):
        lines.append(f'    ("minecraft:{name}", {light[name]}),')
    lines.append("];")
    lines.append("")

    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines))
    print(f"{len(light)} blocks give off light -> {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
