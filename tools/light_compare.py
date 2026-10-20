"""Captures the light the server sends for each chunk, to compare two servers.

    python tools/light_compare.py <host> <port> <out.json>

The point is to have the game's own light as the answer: a chunk's light is
read out of the chunk packet, so two servers on the same world folder can be
asked the same question and their answers compared byte for byte.
"""

import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from capture import varint, read_varint  # noqa: E402
from capture_break import Client  # noqa: E402

PROTOCOL_VERSION = 777


def read_int(buf, off):
    return int.from_bytes(buf[off:off + 4], "big", signed=True), off + 4


def read_long_array(buf, off):
    count, off = read_varint(buf, off)
    return off + count * 8


def read_byte_array(buf, off):
    length, off = read_varint(buf, off)
    return length, off + length


def read_chunk_light(buf, off):
    """The light of a chunk packet: the sky's and the lamps', per section."""
    x, off = read_int(buf, off)
    z, off = read_int(buf, off)

    # The heightmaps: a map of a kind to a long array.
    count, off = read_varint(buf, off)
    for _ in range(count):
        _, off = read_varint(buf, off)
        off = read_long_array(buf, off)

    # The sections: one blob.
    size, off = read_varint(buf, off)
    off += size

    # The block entities: a list.
    count, off = read_varint(buf, off)
    for _ in range(count):
        off = read_block_entity(buf, off)

    # And the light: four masks and two lists.
    masks = []
    for _ in range(4):
        length, off = read_byte_array(buf, off)
        masks.append(buf[off - length:off])
    sky = []
    count, off = read_varint(buf, off)
    for _ in range(count):
        length, off = read_varint(buf, off)
        sky.append(buf[off:off + length].hex())
        off += length
    block = []
    count, off = read_varint(buf, off)
    for _ in range(count):
        length, off = read_varint(buf, off)
        block.append(buf[off:off + length].hex())
        off += length

    # The mask names the sections and the arrays follow it, one for each bit.
    def named(mask):
        out = []
        for index, byte in enumerate(mask):
            for bit in range(8):
                if byte & (1 << bit):
                    out.append(index * 8 + bit)
        return out

    if len(named(masks[0])) != len(sky) or len(named(masks[1])) != len(block):
        print(f"  MISMATCH in chunk {x},{z}: the sky mask names "
              f"{len(named(masks[0]))} and there are {len(sky)} arrays; "
              f"the block mask names {len(named(masks[1]))} and there are "
              f"{len(block)}")
    return (x, z), sky, block


def read_block_entity(buf, off):
    """A block entity: a block pos, a type and its data."""
    off += 8  # the packed block pos
    length, off = read_varint(buf, off)
    off += length
    return off


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25566
    out_path = sys.argv[3] if len(sys.argv) > 3 else "captured/light_compare.json"

    c = Client(host, port)
    hb = host.encode()
    handshake = (
        varint(PROTOCOL_VERSION)
        + varint(len(hb))
        + hb
        + struct_pack_port(port)
        + varint(2)
    )
    c.sock.sendall(varint(1 + len(handshake)) + varint(0) + handshake)
    name = b"light-probe"
    c.send(0, varint(len(name)) + name + b"\x00" * 16)

    state = "login"
    chunks = {}
    deadline = time.time() + 60
    while time.time() < deadline and len(chunks) < 40:
        try:
            body = c.recv()
        except Exception as e:  # noqa: BLE001
            print(f"connection ended: {e}")
            break
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
            if pid == 15:
                packs, o = read_varint(body, off)
                for _ in range(packs):
                    length, o = read_varint(body, o)
                    o += length
                # The brand. The game will not finish the configuration
                # without one: it disconnects a client that never says who
                # it is.
                brand = b"minecraft:brand"
                c.send(
                    1,
                    varint(len(brand))
                    + brand
                    + varint(len("vanilla"))
                    + b"vanilla",
                )
            elif pid == 3:
                # The serverbound "finish configuration". The game numbers
                # the packets each way separately, so this is not the number
                # the clientbound one has.
                c.send(2)
                state = "play"
            continue
        if pid == 46:
            try:
                key, sky, block = read_chunk_light(body, off)
            except Exception as e:  # noqa: BLE001
                print(f"could not read a chunk: {e}")
                break
            chunks[f"{key[0]},{key[1]}"] = {"sky": sky, "block": block}
            if len(chunks) >= 40:
                break

    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(chunks, f)
    print(f"{len(chunks)} chunks from {host}:{port} -> {out_path}")


def struct_pack_port(port):
    import struct
    return struct.pack(">H", port)


if __name__ == "__main__":
    main()
