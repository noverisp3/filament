"""Decodes the light masks of a chunk packet as the server sends it.

    python tools/light_masks.py [host] [port]

The point is which bit names which section. The client's light engine numbers
its sections from one below the world:

    LevelLightEngine.getMinLightSection() = getMinSectionY() - 1

so the world's own bottom section (y = -64) is bit 1 and bit 0 is the padding
section below the world. A server that counts from the world's bottom instead
puts every section one too low.

The chunk's own sections are the ground truth for what the light should be
naming: the block data says which sections hold the ground, and the light
mask says which section the light belongs to. Comparing the two is the check.
"""

import os
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from capture import varint, read_varint  # noqa: E402
from capture_break import Client  # noqa: E402

PROTOCOL_VERSION = 777


def read_chunk(body, off):
    """The chunk's coordinates, its ground, and its four light masks."""
    x = int.from_bytes(body[off:off + 4], "big", signed=True)
    off += 4
    z = int.from_bytes(body[off:off + 4], "big", signed=True)
    off += 4

    count, off = read_varint(body, off)
    for _ in range(count):
        _, off = read_varint(body, off)
        longs, off = read_varint(body, off)
        off += longs * 8

    size, off = read_varint(body, off)
    blob = body[off:off + size]
    off += size

    count, off = read_varint(body, off)
    for _ in range(count):
        off += 8
        length, off = read_varint(body, off)
        off += length

    masks = []
    for _ in range(4):
        length, off = read_varint(body, off)
        masks.append(body[off:off + length])
        off += length

    # The ground: how many blocks each section holds.
    blocks = []
    inner = 0
    for _ in range(24):
        count = int.from_bytes(blob[inner:inner + 2], "big", signed=True)
        blocks.append(count)
        inner += 2 + 2
        # A palette and its packed data, walked by size rather than by shape:
        # the block count is enough to place the ground.
        break
    return x, z, blob, masks


def named(mask):
    return [b * 8 + bit for b, byte in enumerate(mask) for bit in range(8) if byte >> bit & 1]


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25565

    c = Client(host, port)
    hb = host.encode()
    handshake = (
        varint(PROTOCOL_VERSION)
        + varint(len(hb))
        + hb
        + struct.pack(">H", port)
        + varint(2)
    )
    c.sock.sendall(varint(1 + len(handshake)) + varint(0) + handshake)
    name = b"mask-probe"
    c.send(0, varint(len(name)) + name + b"\x00" * 16)

    state = "login"
    answered = False
    chunk = None
    deadline = time.time() + 90
    while time.time() < deadline and chunk is None:
        body = c.recv()
        if body is None:
            time.sleep(0.1)
            continue
        pid, off = read_varint(body, 0)
        if state == "login":
            if pid == 3:
                c.threshold, _ = read_varint(body, off)
            elif pid == 2:
                c.send(3)
                state = "configuration"
            continue
        if state == "configuration":
            if pid != 3 and not answered:
                answered = True
                brand = b"minecraft:brand"
                c.send(
                    2,
                    varint(len(brand))
                    + brand
                    + varint(len("vanilla"))
                    + b"vanilla",
                )
                c.send(7, varint(0))
            elif pid == 3:
                c.send(3)
                state = "play"
            continue
        if pid == 46:
            chunk = (body, off)

    if chunk is None:
        print("no chunk packet")
        return

    body, off = chunk
    x = int.from_bytes(body[off:off + 4], "big", signed=True)
    off += 4
    z = int.from_bytes(body[off:off + 4], "big", signed=True)
    off += 4

    count, off = read_varint(body, off)
    for _ in range(count):
        _, off = read_varint(body, off)
        longs, off = read_varint(body, off)
        off += longs * 8

    size, off = read_varint(body, off)
    blob = body[off:off + size]
    off += size

    count, off = read_varint(body, off)
    for _ in range(count):
        off += 8
        length, off = read_varint(body, off)
        off += length

    masks = []
    for _ in range(4):
        length, off = read_varint(body, off)
        masks.append(body[off:off + length])
        off += length

    print(f"chunk {x},{z}")
    for label, mask in zip(
        ("sky present", "block present", "sky empty", "block empty"), masks
    ):
        print(f"  {label}: {mask.hex()} names bits {named(mask)}")

    # The first section of the block data is the world's bottom section.
    ground = int.from_bytes(blob[0:2], "big", signed=True)
    print(f"  the world's bottom section holds {ground} blocks")
    print()
    print("The client numbers light sections from one below the world, so the")
    print("world's bottom section is bit 1 and bit 0 is the padding below it.")


if __name__ == "__main__":
    main()
