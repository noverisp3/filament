"""Generates src/generated/hardness.rs from the decompiled 26.3 source.

Hardness is how long a block takes to break, and the client needs it to decide
when a block is finished. That creates a problem for the server: the crack it
draws is paced against a break time it has to guess at, and a guess that is
slower than the client's is a crack whose last few stages get cut off.

So it is not guessed. It is read out of the block definitions, where the game
keeps it.

Where it is not, which is worth knowing before looking for it:

  - the data generator's blocks report carry state ids and properties, not
    hardness, and nothing else the generator emits has it either;
  - the registry frames the server sends at login do not have it either:
    `hardness` appears zero times in assets/registry_data.bin. It is static
    data, compiled into both the client and the server jar, and it is not
    something a login handshake ships.

Which leaves the source, and that is what this reads. It is the same arrangement
as src/generated/blocks.rs, which comes out of the data generator: the numbers
are produced by a tool that can be re-run, rather than typed into a server file
where nobody could check them.

Three shapes of declaration, from Blocks.java:

    DIRT = register(BlockItemIds.DIRT, ..., Properties.of().strength(0.5F)...);
    BARRIER = register(..., Properties.of().strength(-1.0F, 3600000.8F)...);
    AIR = register(BlockItemIds.AIR, ..., Properties.of()...);   // no strength

`strength(a)` and `strength(a, b)` both mean hardness a; the second number is
blast resistance and has nothing to do with mining. A block with no `strength`
call has the field's Java default, which is zero, so it breaks at once.

And the shapes that derive from another block:

    OAK_STAIRS = registerLegacyStair(BlockItemIds.OAK_STAIRS, OAK_PLANKS);

A stair is as hard as the block it is made of, so those look the base constant
up rather than parsing a Properties chain that is not there.

Run it against a fresh decompile:

    python tools/extract_hardness.py
"""

import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent
MC_REF = REPO.parent / "mc-ref" / "decompiled" / "server"
OUT = REPO / "src" / "generated" / "hardness.rs"

# `public static final Block DIRT = register...`
DECLARATION = re.compile(r"public static final Block (\w+) = (register\w*)\(")
# The id, which lives in a constants class rather than being spelled out.
IDENTIFIER = re.compile(r"Block(?:Item)?Ids\.(\w+)")
# strength(a) or strength(a, b); the first argument is the hardness.
STRENGTH = re.compile(r"\.strength\(\s*(-?[\d.]+)F?\s*(?:,\s*(-?[\d.]+)F?\s*)?\)")
INSTANT = re.compile(r"\.instanterbreak\(\)")

# The Properties factories: `private static BlockBehaviour.Properties logProperties(...) {`
HELPER_DECLARATION = re.compile(
    r"(?:private|public)\s+static\s+BlockBehaviour\.Properties\s+(\w+)\s*\([^)]*\)\s*\{"
)
# A call to one of them, as the Properties argument of a register.
HELPER_CALL = re.compile(r"(?<![\w.])(\w+)\s*\(")

# The register variants that take a base block as their second argument and copy
# its properties. Anything not listed parses a Properties chain of its own.
INHERITS = {"registerStair", "registerSlab", "registerWall", "registerLegacyStair"}


def blocks_file():
    found = sorted(MC_REF.rglob("Blocks.java"))
    if not found:
        sys.exit(f"no Blocks.java under {MC_REF}")
    return found[0]


def statement_at(text, open_paren):
    """The text of a call starting at an opening parenthesis, and where it ends.

    Balanced, because these declarations run to many lines and stop at the
    parenthesis that matches the one the declaration opened. Counting with a
    string-aware scan rather than a regex, since the arguments are full of
    parentheses of their own.
    """
    depth = 0
    in_string = None
    i = open_paren
    while i < len(text):
        ch = text[i]
        if in_string:
            if ch == "\\":
                i += 2
                continue
            if ch == in_string:
                in_string = None
        elif ch in "\"'":
            in_string = ch
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return text[open_paren + 1 : i], i + 1
        i += 1
    raise ValueError("unbalanced parentheses")


def properties_of(args, helpers, depth=0):
    """The hardness declared by a Properties argument, following helpers.

    Not every block spells out its Properties. Logs, leaves, buttons and a few
    others call a shared factory instead:

        ACACIA_LOG = register(BlockItemIds.ACACIA_LOG, RotatedPillarBlock::new,
                              logProperties(MapColor.COLOR_ORANGE, ...));

    and `logProperties` is where the strength is. Reading only the inline chains
    gives every such block the default of zero, which is wrong for most of the
    wood in the game: oak logs are two, not zero. A table that is quietly wrong
    about 878 blocks is worse than no table, so the helpers are followed.

    The depth is because a helper could call another helper. It is bounded at
    three, which is deeper than this file goes, and stops rather than looping if
    something ever does.
    """
    strength = STRENGTH.search(args)
    if strength:
        return float(strength.group(1))
    if INSTANT.search(args):
        return 0.0

    if depth < 3:
        for call in HELPER_CALL.finditer(args):
            helper = helpers.get(call.group(1))
            if helper is not None:
                return properties_of(helper, helpers, depth + 1)

    return 0.0


def helper_bodies(source):
    """The bodies of the Properties factories, by name."""
    found = {}
    for match in HELPER_DECLARATION.finditer(source):
        open_brace = match.end() - 1
        close = source.find("}", open_brace)
        if close != -1:
            found[match.group(1)] = source[open_brace:close]
    return found


def mine(source):
    """Every block name to (minecraft id, hardness source)."""
    hardness = {}
    names = {}
    helpers = helper_bodies(source)

    for match in DECLARATION.finditer(source):
        constant, kind = match.group(1), match.group(2)
        open_paren = match.end() - 1
        args, _ = statement_at(source, open_paren)

        identifier = IDENTIFIER.search(args)
        if not identifier:
            continue

        inherited = None
        if kind in INHERITS:
            # The base block is the second argument, a bare constant.
            parts = args.split(",", 1)
            if len(parts) == 2:
                base = parts[1].split(",", 1)[0].strip()
                inherited = base
            # Hardness comes from the base, which is resolved in a second pass.
            names[constant] = (identifier.group(1), inherited, None)
            continue

        names[constant] = (identifier.group(1), None, properties_of(args, helpers))

    # Resolve the derived blocks now that every base is known.
    for constant, (identifier, inherited, value) in names.items():
        if value is not None:
            hardness[identifier] = value
            continue
        base_value = None
        seen = set()
        while inherited and inherited not in seen:
            seen.add(inherited)
            entry = names.get(inherited)
            if entry is None:
                break
            _, next_base, next_value = entry
            if next_value is not None:
                base_value = next_value
                break
            inherited = next_base
        hardness[identifier] = 0.0 if base_value is None else base_value

    return hardness


def rust_name(identifier):
    return identifier.lower()


def emit(hardness, source_path):
    lines = [
        "//! How long each block takes to break, generated from the decompiled source.",
        "//!",
        "//! Generated by `tools/extract_hardness.py`. Do not edit by hand: re-run",
        f"//! the tool against a fresh decompile. Read from `{source_path}`.",
        "//!",
        "//! Hardness is the first number of a block's `strength(...)` call; the",
        "//! second is blast resistance, which has nothing to do with mining. A",
        "//! block with no strength call has the field's Java default of zero, so it",
        "//! breaks at once. Stairs, slabs and walls are as hard as the block they",
        "//! are made of, and are resolved from it rather than from a Properties",
        "//! chain they do not have.",
        "//!",
        "//! This is what the client uses to decide when a block is finished, so a",
        "//! server pacing its mining crack against different numbers gets its last",
        "//! few stages cut off. See `tools/extract_hardness.py` for where this is",
        "//! not available from: the data generator does not emit it and the",
        "//! registry frames sent at login do not contain it.",
        "",
        "/// Every block and its hardness, sorted so the output is stable.",
        "pub const HARDNESS: &[(&str, f32)] = &[",
    ]

    for identifier in sorted(hardness, key=rust_name):
        # Rust spells a float with a lowercase f, where Java's source this is
        # read from uses an uppercase one. Copying the suffix across verbatim
        # produces a file that does not compile.
        value = hardness[identifier]
        literal = repr(float(value)) + "f32"
        lines.append(f'    ("minecraft:{rust_name(identifier)}", {literal}),')

    lines += [
        "];",
        "",
        "/// The hardness of a block, or `None` if it is not in the table.",
        "///",
        "/// `None` rather than a default because a wrong number is worse than no",
        "/// number: it paces the crack against a time the client does not share.",
        "pub fn hardness_of(name: &str) -> Option<f32> {",
        "    HARDNESS",
        "        .binary_search_by(|(candidate, _)| (*candidate).cmp(name))",
        "        .ok()",
        "        .map(|at| HARDNESS[at].1)",
        "}",
        "",
    ]

    OUT.write_text("\n".join(lines), encoding="utf-8")


def main():
    path = blocks_file()
    source = path.read_text(encoding="utf-8")
    hardness = mine(source)

    unbreakable = [k for k, v in hardness.items() if v < 0]
    print(f"read {path}")
    print(f"blocks: {len(hardness)}")
    print(f"unbreakable (negative): {len(unbreakable)}")

    for check in ("dirt", "grass_block", "stone", "glowstone", "oak_stairs", "barrier", "air"):
        got = hardness.get(check.upper() if check.upper() in hardness else None)
        print(f"  {check:<14} {got}")

    emit(hardness, path)
    print(f"wrote {OUT.relative_to(REPO)}")


if __name__ == "__main__":
    main()
