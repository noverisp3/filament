"""Asks the server whether a broken block survived the walk away and back.

The ghost the players see: a block is broken, both clients watch it go, and
walking away and back finds it solid again, invisible and standing-on-able,
until a rejoin.

The server half of that is testable without a client at all. Break a block,
walk far enough that the chunk is dropped, walk back so it is asked for again,
and then try to break the same block a second time:

    second block_update arrives   the server rebuilt the chunk with the block
                                  in it, so the edit was lost on the way
                                  through the drop -- the ghost is ours
    nothing arrives               the server still has air there, the chunk it
                                  re-sent is right, and the ghost is the
                                  client's

    python tools/capture_ghost.py [host] [port]
"""

import json
import os
import socket
import struct
import sys
import time
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from capture import varint, read_varint, read_string  # noqa: E402

PROTOCOL_VERSION = 777
OUT = os.path.join("captured", "ghost")

SB_ACCEPT_TELEPORT = 0
SB_MOVE_PLAYER_POS = 30
SB_CLIENT_TICK_END = 13
SB_KEEP_ALIVE = 28
SB_PING_REQUEST = 38
SB_PLAYER_ACTION = 41

START_DESTROY_BLOCK = 0
STOP_DESTROY_BLOCK = 3

CB_KEEP_ALIVE = 45
CB_PING = 62
CB_PLAYER_POSITION = 73
CB_LEVEL_CHUNK = 46
CB_DISCONNECT = 32
CB_BLOCK_UPDATE = 8

# How far to walk, in blocks. The server sends a square of the view radius
# around the player, eight either way, and drops a chunk when the last player
# leaves it, so twice that plus a margin is well outside.
WALK = 160


def pack_block_pos(x, y, z):
    return ((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)


def unpack_block_pos(packed):
    x = packed >> 38
    z = (packed << 26) >> 38
    y = ((packed << 52) >> 52) & 0xFFF
    if y >= 2048:
        y -= 4096
    return x, y, z


class Client:
    def __init__(self, host, port):
        self.sock = socket.create_connection((host, port), timeout=10)
        self.threshold = None
        self.buf = b""

    def send(self, packet_id, payload=b""):
        body = varint(packet_id) + payload
        if self.threshold is None:
            frame = varint(len(body)) + body
        elif len(body) >= self.threshold:
            inner = varint(len(body)) + zlib.compress(body)
            frame = varint(len(inner)) + inner
        else:
            inner = varint(0) + body
            frame = varint(len(inner)) + inner
        self.sock.sendall(frame)

    def _fill(self):
        self.sock.settimeout(1)
        try:
            chunk = self.sock.recv(1 << 20)
        except socket.timeout:
            return False
        if not chunk:
            raise EOFError("server closed the connection")
        self.buf += chunk
        return True

    def recv(self):
        while True:
            try:
                length, offset = read_varint(self.buf)
                if len(self.buf) >= offset + length:
                    break
            except EOFError:
                if not self._fill():
                    return None
        raw = self.buf[offset : offset + length]
        self.buf = self.buf[offset + length :]
        if self.threshold is None:
            return raw
        data_len, consumed = read_varint(raw)
        rest = raw[consumed:]
        if data_len == 0:
            return rest
        return zlib.decompress(rest)


class Watcher:
    """Drains the socket for a while, counting the packets that matter."""

    def __init__(self, client):
        self.c = client
        self.block_updates = []
        self.chunks = 0
        self.disconnect = None
        self.log = []

    def run(self, seconds, label):
        deadline = time.time() + seconds
        while time.time() < deadline:
            try:
                body = self.c.recv()
            except (EOFError, zlib.error) as e:
                self.disconnect = str(e)
                return
            if body is None:
                time.sleep(0.05)
                continue
            pid, offset = read_varint(body, 0)
            if pid == CB_KEEP_ALIVE:
                self.c.send(SB_KEEP_ALIVE, body[offset:])
            elif pid == CB_PING:
                self.c.send(SB_PING_REQUEST, body[offset:])
            if pid == CB_LEVEL_CHUNK:
                self.chunks += 1
                self.log.append(f"{label}: chunk {self.chunks}")
            if pid == CB_BLOCK_UPDATE:
                packed = struct.unpack_from(">q", body, offset)[0]
                state, _ = read_varint(body, offset + 8)
                x, y, z = unpack_block_pos(packed)
                self.block_updates.append((x, y, z, state))
                self.log.append(f"{label}: block_update {x} {y} {z} state {state}")
            if pid == CB_DISCONNECT:
                self.disconnect = body[offset:][:200].decode(errors="replace")
                return
        self.c.send(SB_CLIENT_TICK_END)


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25565

    os.makedirs(OUT, exist_ok=True)
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

    name = b"filament-probe"
    c.send(0, varint(len(name)) + name + b"\x00" * 16)

    state = "login"
    position = None
    print(f"connecting to {host}:{port}")

    while True:
        try:
            body = c.recv()
        except (EOFError, zlib.error) as e:
            print(f"connection ended: {e}")
            return
        if body is None:
            time.sleep(0.05)
            continue
        pid, offset = read_varint(body, 0)

        if state == "login":
            if pid == 3:
                c.threshold, _ = read_varint(body, offset)
            elif pid == 2:
                c.send(3)
                state = "configuration"
            elif pid == 0:
                print(f"REFUSED: {body[offset:][:200]}")
                return
            continue

        if state == "configuration":
            if pid == 15:
                packs, o = read_varint(body, offset)
                for _ in range(packs):
                    _, o = read_string(body, o)
                c.send(7, varint(0))
            elif pid == 3:
                c.send(3)
                state = "play"
                print("in the play state")
            continue

        if pid == CB_DISCONNECT:
            print(f"REFUSED: {body[offset:][:200]}")
            return
        if pid == CB_KEEP_ALIVE:
            c.send(SB_KEEP_ALIVE, body[offset:])
        elif pid == CB_PING:
            c.send(SB_PING_REQUEST, body[offset:])
        elif pid == CB_PLAYER_POSITION:
            teleport, at = read_varint(body, offset)
            position = struct.unpack_from(">ddd", body, at)
            c.send(
                SB_ACCEPT_TELEPORT,
                varint(teleport) + body[at : at + 24] + body[at + 24 : at + 32],
            )
            c.send(SB_CLIENT_TICK_END)
            print(f"spawned at {tuple(round(v, 1) for v in position)}")
            break

    w = Watcher(c)
    w.run(6, "arriving")
    print(f"chunks so far: {w.chunks}")
    if w.chunks == 0:
        print("no chunks arrived; cannot test")
        return

    bx, by, bz = 1, -6, 2  # a grass block a real player broke in the session log
    packed = pack_block_pos(bx, by, bz)
    print(f"breaking {bx} {by} {bz} the first time")

    c.send(SB_PLAYER_ACTION, varint(START_DESTROY_BLOCK) + struct.pack(">q", packed) + b"\x01" + varint(1))
    time.sleep(0.5)
    c.send(SB_PLAYER_ACTION, varint(STOP_DESTROY_BLOCK) + struct.pack(">q", packed) + b"\x01" + varint(2))

    w.run(3, "first break")
    first = len(w.block_updates)
    print(f"block updates after the first break: {first}")
    if first == 0:
        print("the server did not accept the first break; cannot test")
        return

    def walk(target, seconds):
        start = position[0]
        steps = 40
        for i in range(1, steps + 1):
            x = start + (target - start) * i / steps
            c.send(SB_MOVE_PLAYER_POS, struct.pack(">ddd", x, position[1], position[2]) + b"\x01")
            time.sleep(seconds / steps)
        c.send(SB_CLIENT_TICK_END)

    print(f"walking away {WALK} blocks")
    walk(position[0] + WALK, 4)
    w.run(4, "away")
    away_chunks = w.chunks
    print(f"chunks seen while away: {away_chunks}")

    print("walking back")
    walk(position[0], 4)
    w.run(6, "back")
    print(f"chunks seen in total: {w.chunks}")

    print(f"breaking {bx} {by} {bz} the second time")
    updates_before = len(w.block_updates)
    c.send(SB_PLAYER_ACTION, varint(START_DESTROY_BLOCK) + struct.pack(">q", packed) + b"\x01" + varint(3))
    time.sleep(0.5)
    c.send(SB_PLAYER_ACTION, varint(STOP_DESTROY_BLOCK) + struct.pack(">q", packed) + b"\x01" + varint(4))
    w.run(3, "second break")
    second = len(w.block_updates) - updates_before

    print()
    print(f"first break: {first} block_update")
    print(f"second break: {second} block_update")
    if second == 0:
        print("VERDICT: the server still has air there. The re-sent chunk is")
        print("correct and the ghost is on the client side.")
    else:
        print("VERDICT: the server rebuilt the chunk with the block back in it.")
        print("The edit was lost when the chunk was dropped and re-sent: the")
        print("ghost is ours, in the journal replay.")

    with open(os.path.join(OUT, f"{port}_ghost.json"), "w", encoding="utf-8") as f:
        json.dump({"log": w.log, "updates": w.block_updates}, f, indent=2)


if __name__ == "__main__":
    main()