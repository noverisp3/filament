"""Compares the light two neighbouring chunks were sent, across their border.

    python tools/border_light.py [host] [port]

The point is to see whether the light the client is given is continuous
where one chunk ends and the next begins. A one-block gap at the border is
a client drawing a seam, and the server's own numbers are the yardstick it
is measured against.

The chunks are compared the way the client holds them: the light of a chunk
is a set of sections, and the section the client's light engine numbers
first is the one below the world, so the world's own bottom section is the
second.
"""

import os
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from capture import varint, read_varint  # noqa: E402
from capture_break import Client  # noqa: E402

PROTOCOL_VERSION = 777

# The client's light engine numbers its sections from one below the world.
MIN_LIGHT_SECTION = -5


def named(mask):
    return [b * 8 + bit for b, byte in enumerate(mask) for bit in range(8) if byte >> bit & 1]


def read_chunk_light(body, off):
    """The chunk's light: the masks and the arrays, section by section."""
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

    layers = []
    for _ in range(2):
        count, off = read_varint(body, off)
        arrays = []
        for _ in range(count):
            length, off = read_varint(body, off)
            arrays.append(body[off:off + length])
            off += length
        layers.append(arrays)

    # The arrays follow the mask, in the order the mask names them.
    sections = []
    for mask, arrays in zip((masks[0], masks[1]), layers):
        by_section = {}
        for bit, data in zip(named(mask), arrays):
            by_section[bit] = data
        sections.append(by_section)

    return (x, z), sections


def cell(data, x, y, z):
    """The light at a cell, in the game's own packing."""
    index = x + z * 16 + y * 256
    byte = data[index >> 1]
    return (byte >> 4) if index & 1 else (byte & 0x0F)


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
    name = b"border-probe"
    c.send(0, varint(len(name)) + name + b"\x00" * 16)

    state = "login"
    answered = False
    chunks = {}
    deadline = time.time() + 120
    while time.time() < deadline and len(chunks) < 40:
        body = c.recv()
        if body is None:
            time.sleep(0.05)
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
            key, sections = read_chunk_light(body, off)
            chunks[key] = sections
        if pid == 45:
            c.send(28, body[off:])

    print(f"{len(chunks)} chunks")

    # Find a pair that share an edge, and compare the columns along it.
    for (ax, az), _ in sorted(chunks.items()):
        for bx, bz in ((ax + 1, az), (ax, az + 1)):
            if (bx, bz) not in chunks:
                continue
            a = chunks[(ax, az)]
            b = chunks[(bx, bz)]
            print()
            print(f"chunks ({ax},{az}) and ({bx},{bz})")
            for layer, name in ((0, "sky"), (1, "block")):
                # The world's own sections, in the client's numbering.
                for section_y in range(-4, 20):
                    bit = section_y - MIN_LIGHT_SECTION
                    da = a[layer].get(bit)
                    db = b[layer].get(bit)
                    if da is None or db is None:
                        continue
                    # The column along the shared edge, at the bottom of the
                    # section: the last column of A and the first of B.
                    edge = 0
                    if bx == ax + 1:
                        left = [cell(da, 15, y, 0) for y in range(16)]
                        right = [cell(db, 0, y, 0) for y in range(16)]
                    else:
                        left = [cell(da, 0, y, 15) for y in range(16)]
                        right = [cell(db, 0, y, 0) for y in range(16)]
                    jump = sum(
                        1 for p, q in zip(left, right) if abs(p - q) > 1
                    )
                    if jump:
                        print(
                            f"  {name} section y={section_y}: {jump} cells jump "
                            f"across the border"
                        )
                        print(f"    A: {left}")
                        print(f"    B: {right}")
                    edge += jump
                if edge == 0:
                    print(f"  {name}: continuous across the border")


if __name__ == "__main__":
    main()
