"""Places a block and watches the light the server sends back.

    python tools/light_place.py [host] [port]

The point is the light update that follows a placement: the mask it carries
names the sections the light belongs to, and the client's light engine
numbers its sections from one below the world, so the world's own bottom
section is bit 1.

The block placed is a glowstone, because a glowstone is the block whose
light is worth watching: the server works out a pool of light around it and
sends the sections that pool is in.
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
SB_SET_HELD_ITEM = 47
SB_PLAYER_POSITION = 31
SB_CLIENT_TICK_END = 13
SB_ACCEPT_TELEPORT = 0

CB_LIGHT_UPDATE = 49
CB_BLOCK_UPDATE = 8

# The glowstone's state id in the game's own table.
GLOWSTONE_STATE = 1


def pack_block_pos(x, y, z):
    """The packed position a block_update carries."""
    return ((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)


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
    name = b"place-probe"
    c.send(0, varint(len(name)) + name + b"\x00" * 16)

    state = "login"
    answered = False
    placed = False
    deadline = time.time() + 120
    teleport = None

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

        # In play.
        if pid == 73:  # the player position: the server puts the player somewhere
            teleport, _ = read_varint(body, off)
            position = body[off + 1:off + 25]
            c.send(SB_ACCEPT_TELEPORT, varint(teleport) + position + body[off + 25:off + 33])
            c.send(SB_CLIENT_TICK_END)
            # Tell the server where the player is, so it will answer.
            c.send(
                SB_PLAYER_POSITION,
                struct.pack(">ddd", 8.5, 70.0, 8.5)
                + struct.pack(">ff", 0.0, 0.0)
                + b"\x00"
                + b"\x00",
            )
            c.send(SB_CLIENT_TICK_END)
            continue

        if pid == 45:  # keep alive
            c.send(28, body[off:])
            continue

        if pid == 62:  # ping
            c.send(38, body[off:])
            continue

        if pid == CB_BLOCK_UPDATE and not placed:
            # The server is telling the client a block changed. That is our cue
            # to place one: it means the world is there.
            placed = True
            continue

        if pid == CB_LIGHT_UPDATE:
            x, off = read_varint(body, off)
            z, off = read_varint(body, off)
            masks = []
            for _ in range(4):
                length, off = read_varint(body, off)
                masks.append(body[off:off + length])
                off += length
            arrays = []
            for _ in range(2):
                count, off = read_varint(body, off)
                layer = []
                for _ in range(count):
                    length, off = read_varint(body, off)
                    layer.append(body[off:off + length])
                    off += length
                arrays.append(layer)
            print(f"light update for chunk {x},{z}")
            print(f"  sky present: {masks[0].hex()} bits {named(masks[0])}")
            print(f"  block present: {masks[1].hex()} bits {named(masks[1])}")
            print(f"  sky empty: {masks[2].hex()} bits {named(masks[2])}")
            print(f"  block empty: {masks[3].hex()} bits {named(masks[3])}")
            for index, data in enumerate(arrays[1]):
                lit = sum(
                    1
                    for k in range(len(data) * 2)
                    if ((data[k // 2] >> (4 if k % 2 else 0)) & 0xF) > 0
                )
                print(f"  block array {index}: {len(data)} bytes, {lit} lit cells")
            return

    print("no light update")


if __name__ == "__main__":
    main()
