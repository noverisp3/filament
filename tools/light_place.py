"""Places a lamp and reads the light update the server sends back.

    python tools/light_place.py [host] [port]

The point is to see the light the client is given after a lamp is placed, and
where it stops. The server's own numbers are the yardstick; this is what the
client is handed.

The lamp goes near the edge of a chunk, so the light has to cross into the
next one, and a one-block gap at the border shows up here rather than in the
world.
"""

import os
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from capture import varint, read_varint  # noqa: E402
from capture_break import Client  # noqa: E402

PROTOCOL_VERSION = 777

SB_USE_ITEM_ON = 66
SB_PLAYER_POSITION = 31
SB_CLIENT_TICK_END = 13
SB_ACCEPT_TELEPORT = 0
SB_CHAT = 2

CB_LIGHT_UPDATE = 49
CB_BLOCK_UPDATE = 8
CB_PLAYER_POSITION = 73
CB_KEEP_ALIVE = 45
CB_PING = 62

# The client's light engine numbers its sections from one below the world.
MIN_LIGHT_SECTION = -5

# Where the lamp goes: two blocks from the edge of the chunk at x = 0, so the
# light has to cross into the chunk at x = 1.
LAMP_X = 14
LAMP_Y = 70
LAMP_Z = 8


def named(mask):
    return [b * 8 + bit for b, byte in enumerate(mask) for bit in range(8) if byte >> bit & 1]


def cell(data, x, y, z):
    """The light at a cell, in the game's own packing."""
    index = x + z * 16 + y * 256
    byte = data[index >> 1]
    return (byte >> 4) if index & 1 else (byte & 0x0F)


def pack_block_pos(x, y, z):
    return ((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)


def read_light_update(body, off):
    x, off = read_varint(body, off)
    z, off = read_varint(body, off)
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
    return (x, z), masks, layers


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
    name = b"place-probe"
    c.send(0, varint(len(name)) + name + b"\x00" * 16)

    state = "login"
    answered = False
    deadline = time.time() + 120
    placed = False
    updates = []

    while time.time() < deadline:
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

        if pid == CB_PLAYER_POSITION:
            teleport, _ = read_varint(body, off)
            position = body[off + 1:off + 25]
            c.send(SB_ACCEPT_TELEPORT, varint(teleport) + position + body[off + 25:off + 33])
            c.send(SB_CLIENT_TICK_END)
            continue

        if pid == CB_KEEP_ALIVE:
            c.send(28, body[off:])
            continue
        if pid == CB_PING:
            c.send(38, body[off:])
            continue

        if pid == CB_LIGHT_UPDATE:
            key, masks, layers = read_light_update(body, off)
            updates.append((key, masks, layers))
            continue

        if pid == CB_BLOCK_UPDATE and not placed:
            placed = True
            # Place the lamp: the hand, the position, the face, the click
            # point, two flags and the client's prediction number.
            packed = pack_block_pos(LAMP_X, LAMP_Y, LAMP_Z)
            body_use = (
                varint(0)
                + struct.pack(">q", packed)
                + varint(1)  # the face: up
                + struct.pack(">fff", 0.5, 1.0, 0.5)
                + b"\x00"  # not inside the block
                + b"\x00"  # not the world border
                + varint(1)  # the client's prediction number
            )
            c.send(SB_USE_ITEM_ON, body_use)
            print(f"placed a lamp at {LAMP_X} {LAMP_Y} {LAMP_Z}")
            continue

    print(f"{len(updates)} light updates")
    for key, masks, layers in updates:
        print(f"chunk {key}")
        for label, mask, arrays in zip(
            ("sky present", "block present", "sky empty", "block empty"),
            masks,
            (layers[0], layers[1], None, None),
        ):
            bits = named(mask)
            print(f"  {label}: {mask.hex()} bits {bits}")
            if arrays is None:
                continue
            for bit, data in zip(bits, arrays):
                lit = sum(
                    1
                    for k in range(len(data) * 2)
                    if ((data[k // 2] >> (4 if k % 2 else 0)) & 0xF) > 0
                )
                print(f"    bit {bit} (section y={bit + MIN_LIGHT_SECTION}): {lit} lit cells")


if __name__ == "__main__":
    main()
