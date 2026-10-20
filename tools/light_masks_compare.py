"""Compares the light masks two servers send for the same chunk.

    python tools/light_masks_compare.py <one.json> <other.json>

The two files are what `light_compare.py` wrote: the light of each chunk, the
arrays as they came. The masks are not in them, so this reads them off the
wire again.

The point is the numbering: the client's light engine counts its sections from
one below the world, so the world's own bottom section is bit 1. A server that
counts from the world's bottom instead names every section one bit too low.
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


def capture(host, port, chunks_wanted=30):
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
    name = b"mask-compare"
    c.send(0, varint(len(name)) + name + b"\x00" * 16)

    state = "login"
    answered = False
    out = {}
    deadline = time.time() + 120
    while time.time() < deadline and len(out) < chunks_wanted:
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
        if pid == 45:
            c.send(28, body[off:])
            continue
        if pid == 62:
            c.send(38, body[off:])
            continue
        if pid != 46:
            continue

        # A chunk packet: the four masks and the two lists.
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
        counts = []
        for _ in range(2):
            n, off = read_varint(body, off)
            counts.append(n)
            for _ in range(n):
                length, off = read_varint(body, off)
                off += length
        out[(x, z)] = (masks, counts)
    return out


def main():
    one = capture(sys.argv[1], int(sys.argv[2]))
    two = capture(sys.argv[3], int(sys.argv[4]))
    shared = sorted(set(one) & set(two))
    print(f"{len(shared)} chunks in both")
    for key in shared[:6]:
        (m1, c1) = one[key]
        (m2, c2) = two[key]
        print(f"chunk {key}")
        for label, mask, count in (
            ("sky present", 0, 0),
            ("block present", 1, 1),
            ("sky empty", 2, None),
            ("block empty", 3, None),
        ):
            print(
                f"  {label}: one names {named(m1[mask])} "
                f"({c1[count] if count is not None else '-'} arrays), "
                f"other names {named(m2[mask])} "
                f"({c2[count] if count is not None else '-'} arrays)"
            )


if __name__ == "__main__":
    main()
