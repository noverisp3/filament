#!/usr/bin/env python3
"""Generates src/generated/shapes.rs from the game's own source.

The game works out three things about a block that change how light behaves
and how a player is stopped:

- Whether the block's shape is a full cube. A block that is a full cube
  takes all the light away; a plant takes none, and a slab takes none either
  because light goes down through it.
- Whether the block occludes. Glass and leaves do not, so they let the light
  through, and they take one level away doing it.
- Whether the block holds a fluid. Water takes one level, and it is the
  reason the light of a torch reaches the bottom of a pool.

The rules the game writes down:

    BlockBehaviour.getLightDampening:
        if (state.isSolidRender()) return 15;
        return state.propagatesSkylightDown() ? 0 : 1;

    BlockBehaviour.propagatesSkylightDown:
        return !Block.isShapeFullBlock(state.getShape(...)) && state.getFluidState().isEmpty();

    BlockBehaviour.BlockStateBase.initCache:
        this.occlusionShape = this.canOcclude ? this.owner.getOcclusionShape(state) : Shapes.empty();
        this.solidRender = Block.isShapeFullBlock(this.occlusionShape);

So a block that is a full cube and occludes takes fifteen, a full cube that
does not occlude takes one, a block that is not a full cube but holds a fluid
takes one, and a block that is not a full cube and holds no fluid takes none.

The shape of a block is its own `getShape`, which is the whole cell unless the
block's class says otherwise, and whether it occludes is a property of the
block, `.noOcclusion()`.

The classes and the properties are read out of the decompiled server, which
is where the game writes them, and the state ids come from the block table
this repository already has.

Usage:
    python tools/extract_shapes.py
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MC_REF = ROOT.parent / "mc-ref" / "decompiled" / "server"
BLOCKS = MC_REF / "net" / "minecraft" / "world" / "level" / "block" / "Blocks.java"
OUT = ROOT / "src" / "generated"

# The class the shape comes from when a block does not say otherwise. The
# game's `BlockBehaviour.getShape` is the whole cell, and `Block` does not
# change it.
DEFAULT_CLASS = "Block"

# The classes the helper methods in `Blocks.java` register.
HELPER_CLASSES = {
    "registerSlab": "SlabBlock",
    "registerStair": "StairBlock",
    "registerLegacyStair": "StairBlock",
    "registerWall": "WallBlock",
}

# The sixteen dye colours, in the order the game keeps them.
COLOURS = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
]

# The eight ways copper can be weathered.
COPPER = [
    "",
    "exposed_",
    "weathered_",
    "oxidized_",
    "waxed_",
    "waxed_exposed_",
    "waxed_weathered_",
    "waxed_oxidized_",
]


def split_arguments(text: str) -> list[str]:
    """The arguments of a call, with the commas that are inside them kept."""
    out: list[str] = []
    depth = 0
    current: list[str] = []
    in_string = False
    for char in text:
        if in_string:
            current.append(char)
            if char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
            current.append(char)
            continue
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        if char == "," and depth == 0:
            out.append("".join(current).strip())
            current = []
            continue
        current.append(char)
    if current:
        out.append("".join(current).strip())
    return out


def call_arguments(source: str, start: int) -> str:
    """The text of a call's arguments, given where the opening bracket is."""
    depth = 1
    index = start
    while depth > 0 and index < len(source):
        if source[index] == "(":
            depth += 1
        elif source[index] == ")":
            depth -= 1
        index += 1
    return source[start : index - 1]


def property_helpers(source: str) -> dict[str, str]:
    """The bodies of the methods that make a block's properties.

    A block's properties are often written by a method instead of written out,
    and the leaves are one of them: the method says `.noOcclusion()` and the
    leaves are the blocks that let the light through.
    """
    out: dict[str, str] = {}
    for match in re.finditer(
        r"private static BlockBehaviour\.Properties (\w+)\([^)]*\) \{", source
    ):
        depth = 1
        index = match.end()
        while depth > 0 and index < len(source):
            if source[index] == "{":
                depth += 1
            elif source[index] == "}":
                depth -= 1
            index += 1
        out[match.group(1)] = source[match.end() : index - 1]
    return out


def id_names() -> tuple[dict[str, str], dict[str, str], dict[str, str], dict[str, str], dict[str, list[str]]]:
    """The game's own names for its blocks, by the constant that holds them.

    Two files hold them, and they are kept apart: a block's name is in one
    and the item's name may be in the other, and they are not the same name
    -- the item of a redstone wire is redstone dust.

    The third and the fourth are the names a group of blocks is built from:
    the wool is sixteen blocks and the name they share is `wool`, and the
    copper is eight and the name they share is `copper_bars` and so on.
    """
    block_ids: dict[str, str] = {}
    item_ids: dict[str, str] = {}
    coloured: dict[str, str] = {}
    copper_groups: dict[str, str] = {}
    copper_special: dict[str, list[str]] = {}
    for path in MC_REF.rglob("*.java"):
        if path.name not in ("BlockIds.java", "BlockItemIds.java"):
            continue
        source = path.read_text(encoding="utf-8")
        found = re.findall(
            r'public static final [\w<>]+ (\w+) = (?:\w+\.)?create\("(\w+)"(?:,\s*"\w+")?\)',
            source,
        )
        groups = re.findall(
            r'public static final ColorCollection<[\w<>]+> (\w+) = createSimpleColored\("(\w+)"\)',
            source,
        )
        copper = re.findall(
            r'public static final WeatheringCopperCollection<[\w<>]+> (\w+) = createSimpleCopper\("(\w+)"\)',
            source,
        )
        special = re.findall(
            r"WeatheringCopperCollection\.ByState<String> (\w+) = new WeatheringCopperCollection\.ByState<>\(\s*((?:\s*\"(\w+)\",?)+)\s*\)",
            source,
            re.S,
        )
        if path.name == "BlockIds.java":
            block_ids.update(found)
        else:
            item_ids.update(found)
        coloured.update(groups)
        copper_groups.update(copper)
        for name, body, _unused in special:
            copper_special[name] = re.findall(r'"(\w+)"', body)
        # The constant that uses the hand-written names.
        for name, source_name in re.findall(
            r"public static final WeatheringCopperCollection<[\w<>]+> (\w+) = WeatheringCopperCollection\.prefixWithState\(\s*WeatheringCopperCollection\.same\((\w+)\)",
            source,
        ):
            if source_name in copper_special:
                copper_special[name] = copper_special[source_name]
    return block_ids, item_ids, coloured, copper_groups, copper_special


def classes() -> dict[str, tuple[str | None, bool]]:
    """Every block class: what it extends, and whether it changes its shape.

    A block's shape is the whole cell unless the class says otherwise, and a
    class says otherwise by having a `getShape` of its own. The chain is
    followed upwards, because a class that changes its shape may have a
    parent that changes it too.
    """
    out: dict[str, tuple[str | None, bool]] = {}

    for path in MC_REF.rglob("*.java"):
        source = path.read_text(encoding="utf-8")
        for match in re.finditer(
            r"public (?:abstract |final )?class (\w+) extends (\w+)", source
        ):
            name, parent = match.group(1), match.group(2)
            changes_shape = "VoxelShape getShape(" in source
            out[name] = (parent, changes_shape)

    return out


def state_ids() -> dict[str, list[int]]:
    """Every state id of every block, by the block's name."""
    source = (ROOT / "src" / "generated" / "blocks.rs").read_text(encoding="utf-8")
    out: dict[str, list[int]] = {}
    for match in re.finditer(
        r'name: "minecraft:(\w+)",\s*\n\s*default_state: (\d+),\s*\n\s*states: &\[(.*?)\],\s*\n\s*\},',
        source,
        re.S,
    ):
        out[match.group(1)] = [int(i) for i in re.findall(r"id: (\d+)", match.group(3))]
    return out


def write_table(
    out: list[str],
    name: str,
    doc: str,
    ids: list[int],
    state_count: int,
) -> None:
    """Writes a table as a bit per block state."""
    words = (state_count + 63) // 64
    packed = [0] * words
    for i in ids:
        packed[i // 64] |= 1 << (i % 64)

    out.append(f"/// {doc}")
    out.append("///")
    out.append(f"/// One bit per block state, {len(ids)} of the {state_count} states set.")
    out.append(f"pub static {name}: &[u64] = &[")
    for i in range(0, len(packed), 6):
        out.append("    " + ", ".join(f"0x{w:x}" for w in packed[i : i + 6]) + ",")
    out.append("];")
    out.append("")


def main() -> None:
    if not BLOCKS.exists():
        sys.exit(f"missing {BLOCKS}\nthe game's own source is needed")

    source = BLOCKS.read_text(encoding="utf-8")
    block_names, item_names, coloured, copper_groups, copper_special = id_names()
    helpers = property_helpers(source)
    hierarchy = classes()
    states = state_ids()

    counted = re.search(
        r"pub const BLOCK_STATE_COUNT: u32 = (\d+);",
        (ROOT / "src" / "generated" / "blocks.rs").read_text(encoding="utf-8"),
    )
    if not counted:
        sys.exit("the block table does not say how many states there are")
    state_count = int(counted.group(1))

    # Every block: its name, its class, and its properties.
    blocks: dict[str, tuple[str, str]] = {}

    def properties_of(text: str) -> str:
        """The properties, following a method that writes them out."""
        called = re.match(r"^(\w+)\(", text)
        if called and called.group(1) in helpers:
            return helpers[called.group(1)]
        return text

    def class_of(text: str) -> str:
        found = re.search(r"new (\w+)\(", text) or re.search(r"(\w+)::new", text)
        return found.group(1) if found else DEFAULT_CLASS

    for match in re.finditer(
        r"public static final Block (\w+) = (register\w*)\(", source
    ):
        name, helper = match.group(1), match.group(2)
        arguments = split_arguments(call_arguments(source, match.end()))

        if helper in HELPER_CLASSES:
            # The helper knows the class, and the id is its first argument.
            id_constant = arguments[0] if arguments else ""
            block_class = HELPER_CLASSES[helper]
            properties = ""
        elif len(arguments) == 2:
            # No class given, so the plain one.
            id_constant = arguments[0]
            block_class = DEFAULT_CLASS
            properties = ""
        else:
            id_constant = arguments[0]
            block_class = class_of(arguments[1])
            properties = properties_of(arguments[2])

        if id_constant.startswith("BlockItemIds."):
            block_name = item_names.get(id_constant.split(".")[-1])
        else:
            block_name = block_names.get(id_constant.split(".")[-1])
        if block_name is None:
            continue
        blocks[block_name] = (block_class, properties)

    # The groups of coloured blocks: the wool, the stained glass, the
    # carpets. They are registered all at once, so their names are the colour
    # and the name the group shares.
    for match in re.finditer(
        r"public static final ColorCollection<Block> (\w+) = ColorCollection\.(\w+)\(",
        source,
    ):
        group, how = match.group(1), match.group(2)
        arguments = split_arguments(call_arguments(source, match.end()))

        if how == "registerBlocks":
            ids_argument, class_argument, properties_argument = (
                arguments[0],
                arguments[2],
                arguments[3],
            )
            block_class = class_of(class_argument)
            properties = properties_of(properties_argument)
        else:
            # `zipMap` gives the ids and a function that registers each one
            # from another group, so the class and the properties are that
            # other group's.
            ids_argument = arguments[1]
            function = arguments[2]
            helper = re.search(r"(register\w+)\(", function)
            base = re.search(r"(\w+)\.pick\(", function)
            block_class = HELPER_CLASSES.get(helper.group(1), DEFAULT_CLASS) if helper else DEFAULT_CLASS
            properties = blocks.get(base.group(1).lower(), ("", ""))[1] if base else ""

        id_constant = ids_argument.split(".")[-1]
        base_name = coloured.get(id_constant)
        if base_name is None:
            continue
        for colour in COLOURS:
            blocks[f"{colour}_{base_name}"] = (block_class, properties)

    # The copper, which comes in eight kinds: new, exposed, weathered,
    # oxidized, and the four waxed ones.
    for match in re.finditer(
        r"public static final WeatheringCopperCollection<Block> (\w+) = WeatheringCopperCollection\.(\w+)\(",
        source,
    ):
        arguments = split_arguments(call_arguments(source, match.end()))
        if len(arguments) < 5:
            continue
        id_constant = arguments[0].split(".")[-1]
        base_name = copper_groups.get(id_constant)
        special = copper_special.get(id_constant)
        if base_name is None and special is None:
            continue
        block_class = class_of(arguments[2])
        properties = properties_of(arguments[4])
        if base_name is not None:
            for prefix in COPPER:
                blocks[f"{prefix}{base_name}"] = (block_class, properties)
        else:
            # The copper blocks are the one family the game names by hand: the
            # new one is `copper_block` and the other three are `copper` with
            # the weather on the front. The waxed ones are the same four.
            for index, prefix in enumerate(COPPER):
                if index < len(special):
                    blocks[f"{prefix}{special[index]}"] = (block_class, properties)
                elif index - len(special) < len(special):
                    blocks[f"{prefix}{special[index - len(special)]}"] = (
                        block_class,
                        properties,
                    )

    full_cube: list[int] = []
    occludes: list[int] = []
    fluid: list[int] = []

    unknown_classes: set[str] = set()

    for block_name, (block_class, properties) in blocks.items():
        if block_name not in states:
            # A block this build's table does not have. Nothing to say about
            # it, and nothing to break.
            continue

        # The shape: the whole cell unless the class changes it, following
        # the chain upwards.
        shape_is_full = True
        current: str | None = block_class
        seen: set[str] = set()
        while current and current not in seen:
            seen.add(current)
            entry = hierarchy.get(current)
            if entry is None:
                # The chain ends at the class every block comes from, and
                # that one is not a block class.
                if current not in (DEFAULT_CLASS, "BlockBehaviour"):
                    unknown_classes.add(current)
                break
            parent, changes_shape = entry
            if changes_shape:
                shape_is_full = False
                break
            current = parent

        block_occludes = "noOcclusion()" not in properties
        block_has_fluid = ".liquid()" in properties

        for state in states[block_name]:
            if shape_is_full:
                full_cube.append(state)
            if block_occludes:
                occludes.append(state)
            if block_has_fluid:
                fluid.append(state)

    if unknown_classes:
        print(f"  {len(unknown_classes)} classes were not found: {sorted(unknown_classes)[:5]}")

    out = [
        "// @generated by tools/extract_shapes.py from the game's own source.",
        "// Do not edit: regenerate with `python tools/extract_shapes.py`.",
        "//",
        f"// {len(full_cube)} states are a full cube, {len(fluid)} hold a fluid.",
        "//",
        "// These are block state ids, the same numbers the server is told when",
        "// it asks what is at a position.",
        "",
    ]

    write_table(
        out,
        "FULL_CUBE",
        "The block states whose shape is a whole cell.",
        sorted(full_cube),
        state_count,
    )
    write_table(
        out,
        "OCCLUDES",
        "The block states that hide what is behind them.",
        sorted(occludes),
        state_count,
    )
    write_table(
        out,
        "FLUID",
        "The block states that hold a fluid.",
        sorted(fluid),
        state_count,
    )

    out.append("/// How much light a block takes away, the way the game takes it away.")
    out.append("///")
    out.append("/// A full cube that hides what is behind it takes all fifteen, a")
    out.append("/// full cube that does not hide takes one, a block that is not a full")
    out.append("/// cube but holds a fluid takes one, and a block that is not a full cube")
    out.append("/// at all takes none, because light goes down through it.")
    out.append("pub fn light_dampening(state: i32) -> u8 {")
    out.append("    if !is_set(FULL_CUBE, state) {")
    out.append("        return if is_set(FLUID, state) { 1 } else { 0 };")
    out.append("    }")
    out.append("    if is_set(OCCLUDES, state) {")
    out.append("        return 15;")
    out.append("    }")
    out.append("    1")
    out.append("}")
    out.append("")
    out.append("/// Whether a block's shape is a whole cell.")
    out.append("pub fn is_full_cube(state: i32) -> bool {")
    out.append("    is_set(FULL_CUBE, state)")
    out.append("}")
    out.append("")
    out.append("/// Whether a block hides what is behind it.")
    out.append("pub fn occludes(state: i32) -> bool {")
    out.append("    is_set(OCCLUDES, state)")
    out.append("}")
    out.append("")
    out.append("/// Whether a block holds a fluid.")
    out.append("pub fn has_fluid(state: i32) -> bool {")
    out.append("    is_set(FLUID, state)")
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
    (OUT / "shapes.rs").write_text(text, encoding="utf-8")
    print(f"wrote {OUT / 'shapes.rs'} ({len(text) / 1024:.0f} KB)")
    print(f"  {len(blocks)} blocks, {len(full_cube)} full cube, {len(fluid)} fluid")


if __name__ == "__main__":
    main()
