"""Decodes a captured level_chunk_with_light packet: its blocks and its light.

    python tools/decode_chunk_light.py captured/play/chunk_reference.bin

The point is the mapping between the light masks and the sections they name.
The client's light engine numbers its sections from one below the bottom of the
world:

    LevelLightEngine.getMinLightSection() = getMinSectionY() - 1

So for an overworld the bottom section of the world (y = -64, section -4) is
bit 1, not bit 0, and bit 0 is a padding section below the world that always
holds nothing.

The blocks are decoded too, because the terrain says where the ground is, and
the ground says which section the light belongs to.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from capture import read_varint  # noqa: E402


def read_int(buf, off):
    return int.from_bytes(buf[off:off + 4], "big", signed=True), off + 4


def read_long_array(buf, off):
    count, off = read_varint(buf, off)
    return off + count * 8


def read_byte_array(buf, off):
    length, off = read_varint(buf, off)
    return buf[off:off + length], off + length


def read_block_entity(buf, off):
    off += 8
    length, off = read_varint(buf, off)
    return off + length


def named(mask):
    out = []
    for index, byte in enumerate(mask):
        for bit in range(8):
            if byte & (1 << bit):
                out.append(index * 8 + bit)
    return out


def read_container(buf, off, entries):
    """A paletted container: a width, a palette, and the packed entries.

    A width of zero means one value and no list; any other width means a
    palette of that many entries followed by the packed indices.
    """
    bits = buf[off]
    off += 1
    if bits == 0:
        value, off = read_varint(buf, off)
        return [value] * entries, off
    count, off = read_varint(buf, off)
    palette = []
    for _ in range(count):
        value, off = read_varint(buf, off)
        palette.append(value)
    per_long = 64 // bits
    longs = (entries + per_long - 1) // per_long
    out = []
    for _ in range(longs):
        value = int.from_bytes(buf[off:off + 8], "big")
        off += 8
        for _ in range(per_long):
            out.append((value >> (bits * (len(out) % per_long))) & ((1 << bits) - 1))
    return [palette[s] for s in out[:entries]], off


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "captured/play/chunk_reference.bin"
    with open(path, "rb") as f:
        body = f.read()

    pid, off = read_varint(body, 0)
    print(f"packet id {pid}, {len(body)} bytes")

    x, off = read_int(body, off)
    z, off = read_int(body, off)
    print(f"chunk {x},{z}")

    count, off = read_varint(body, off)
    for _ in range(count):
        _, off = read_varint(body, off)
        off = read_long_array(body, off)

    # The sections: one blob, bottom first.
    size, off = read_varint(body, off)
    blob = body[off:off + size]
    off += size

    count, off = read_varint(body, off)
    for _ in range(count):
        off = read_block_entity(body, off)

    masks = []
    for _ in range(4):
        data, off = read_byte_array(body, off)
        masks.append(data)

    layers = []
    for _ in range(2):
        count, off = read_varint(body, off)
        arrays = []
        for _ in range(count):
            data, off = read_byte_array(body, off)
            arrays.append(data)
        layers.append(arrays)

    # How many sections the chunk holds: the blob is a run of them, and the
    # game says nothing how many, so they are counted by walking them.
    print()
    print("sections of the chunk, bottom first:")
    inner = 0
    slot = 0
    while inner + 4 <= len(blob):
        blocks = int.from_bytes(blob[inner:inner + 2], "big", signed=True)
        fluids = int.from_bytes(blob[inner + 2:inner + 4], "big", signed=True)
        inner += 4
        states, inner = read_container(blob, inner, 4096)
        biomes, inner = read_container(blob, inner, 64)
        if blocks:
            seen = sorted(set(states))
            print(f"  slot {slot:2d}: {blocks} blocks, states {seen[:8]}"
                  f"{'...' if len(seen) > 8 else ''}")
        slot += 1
    print(f"  {slot} sections in the chunk")

    print()
    for label, mask, arrays in (
        ("sky present", masks[0], layers[0]),
        ("block present", masks[1], layers[1]),
        ("sky empty", masks[2], None),
        ("block empty", masks[3], None),
    ):
        bits = named(mask)
        print(f"{label}: mask {mask.hex()} names bits {bits}")
        if arrays is None:
            continue
        for bit, data in zip(bits, arrays):
            lit = sum(bin(b).count("1") for b in data)
            print(f"    bit {bit}: {len(data)} bytes, {lit} set bits")

    print()
    print("The client numbers light sections from one below the world:")
    print("  bit b names section y = -5 + b for an overworld (min y = -64).")
    print("The bottom section of the world is y = -4, so it is bit 1.")


if __name__ == "__main__":
    main()
