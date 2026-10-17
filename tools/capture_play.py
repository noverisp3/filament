"""Captures play-phase packets from a vanilla 26.3 server.

This is the counterpart to capture.py. That one stops as soon as the client
disconnects, which happens before any chunk arrives, so the whole of the play
phase was missing from the repository and every packet in it was written from
the decompiled source. Eight of those turned out to be wrong.

So this one stays connected. Staying connected is the whole difficulty, and it
means behaving like the client the server expects to talk to:

* confirm the teleport, or the server has nowhere to think the player is and
  sends no chunks at all
* echo every keep alive, or the server drops the connection after thirty seconds
* answer pings

None of that is interesting in itself. It is the minimum needed to be watched
for long enough to be worth watching.

    python tools/capture_play.py [host] [port]

Writes into captured/play/, one file per packet, each a whole packet body with
the id in front:

    chunk_reference.bin    a level_chunk_with_light, verbatim
    player_position.bin    what the server places the player with
    chunk_batch_start.bin
    chunk_batch_finished.bin
    keep_alive.bin
    index.jsonl            everything seen, with ids and sizes
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
OUT = os.path.join("captured", "play")

# Clientbound play ids, from packets.json.
CB_PLAYER_POSITION = 73
CB_CHUNK_BATCH_START = 12
CB_CHUNK_BATCH_FINISHED = 11
CB_KEEP_ALIVE = 45
CB_PING = 62
CB_LEVEL_CHUNK = 46
CB_DISCONNECT = 32

# Serverbound play ids.
SB_ACCEPT_TELEPORT = 0
SB_CLIENT_TICK_END = 13
SB_KEEP_ALIVE = 28
SB_PING_REQUEST = 38

# Saved from the first occurrence, since the second chunk is a different chunk
# and the first is as good a reference as any.
SAVE = {
    CB_LEVEL_CHUNK: "chunk_reference",
    CB_PLAYER_POSITION: "player_position",
    CB_CHUNK_BATCH_START: "chunk_batch_start",
    CB_CHUNK_BATCH_FINISHED: "chunk_batch_finished",
    CB_KEEP_ALIVE: "keep_alive",
}

# Long enough for chunks and some movement, short enough to not sit waiting.
WATCH_SECONDS = 12


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


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25566

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
    index = []
    saved = set()
    quiet_since = None
    entered_play = None

    def save(label, body):
        if label in saved:
            return
        saved.add(label)
        with open(os.path.join(OUT, label + ".bin"), "wb") as f:
            f.write(body)
        print(f"   saved {label}.bin, {len(body)} bytes")

    while True:
        try:
            body = c.recv()
        except (EOFError, zlib.error) as e:
            print(f"connection ended: {type(e).__name__}: {e}")
            break
        if body is None:
            # Nothing for a second while in play means the stream has gone quiet.
            if state == "play":
                if quiet_since is None:
                    quiet_since = time.time()
                elif time.time() - quiet_since > 2:
                    print("the play stream went quiet")
                    break
            time.sleep(0.1)
            continue

        quiet_since = None
        if entered_play is not None and time.time() - entered_play > WATCH_SECONDS:
            print("watched long enough")
            break
        pid, offset = read_varint(body, 0)
        label = SAVE.get(pid, f"{state}:{pid}")
        print(f"<- {label} ({pid}) {len(body)} bytes")

        index.append({"dir": "s2c", "phase": state, "id": pid, "name": label, "size": len(body)})

        if state == "login":
            if pid == 3:
                threshold, _ = read_varint(body, offset)
                c.threshold = threshold
                print(f"   threshold {threshold}")
            elif pid == 2:
                c.send(3)
                state = "configuration"
            elif pid == 0:
                print(f"   REFUSED: {body[offset:][:200]}")
                break
            continue

        if state == "configuration":
            if pid == 2:
                print(f"   REFUSED: {body[offset:][:300]}")
                break
            if pid == 15:
                packs, o = read_varint(body, offset)
                for _ in range(packs):
                    _, o = read_string(body, o)
                c.send(7, varint(0))
            elif pid == 3:
                # Serverbound finish_configuration, which is what the server is
                # waiting for before it will send anything at all. Without it the
                # stream goes quiet: no login packet, no chunk, nothing.
                c.send(3)
                state = "play"
                entered_play = time.time()
                print("   in the play state")
            continue

        # Play.
        if pid == CB_DISCONNECT:
            print(f"   REFUSED: {body[offset:][:200]}")
            break
        if pid in SAVE:
            save(label, body)

        if pid == CB_KEEP_ALIVE:
            # Echo it back, or the server stops sending anything.
            c.send(SB_KEEP_ALIVE, body[offset:])
            print("   echoed the keep alive")
        elif pid == CB_PING:
            c.send(SB_PING_REQUEST, body[offset:])
            print("   answered the ping")
        elif pid == CB_PLAYER_POSITION:
            # The whole PositionMoveRotation has to go back, not just the
            # teleport id:
            #
            #   ServerboundAcceptTeleportationPacket(int id, double x, double y,
            #                                            double z, float yRot, float xRot)
            #
            # Sending only the id gets the connection closed with "Failed to
            # decode packet serverbound/minecraft:accept_teleportation".
            #
            # The player's position is the first of the two vectors, so the
            # second is skipped rather than echoed.
            teleport, at = read_varint(body, offset)
            position = body[at : at + 24]
            at += 24  # the movement delta, which is not echoed
            rotation = body[at : at + 8]

            c.send(SB_ACCEPT_TELEPORT, varint(teleport) + position + rotation)
            print(f"   confirmed teleport {teleport}")
            c.send(SB_CLIENT_TICK_END)

    with open(os.path.join(OUT, "index.jsonl"), "w", encoding="utf-8") as f:
        for row in index:
            f.write(json.dumps(row) + "\n")

    print(f"\n{len(index)} packets, saved {len(saved)}:")
    for label in sorted(saved):
        size = os.path.getsize(os.path.join(OUT, label + ".bin"))
        print(f"   {label}.bin  {size}")


if __name__ == "__main__":
    main()