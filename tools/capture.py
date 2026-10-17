"""Captures everything a vanilla Minecraft 26.3 server sends, once.

Connects to a real server, walks handshake -> login -> configuration -> play,
and writes every byte it saw to disk. The point is to never need to do this
again: the tag payload, the registry entry order, and the play packets all come
from a live connection, and none of them can be derived from the data
generator's reports.

    python tools/capture.py [host] [port]

Writes into captured/:

    packets.bin         every frame, sent and received, length prefixed
    packets.jsonl       index of those frames: direction, phase, id, name

`extracted/tags.json` is the one that mattered. The tag payload references
registry entries by index, and nothing in the data generator's reports records
those indices: registries.json covers 95 registries and none of the 31 that get
synchronised, so damage_type is absent from it entirely. The order the server
sends entries in *is* the id order, and guessing it from the tag files would
have been wrong. is_fire is [21, 3, 31, 24, 20, 42, 47, 14], and alphabetically
in_fire would have come out at 18 rather than 21.

Once this has run, tools/build_tags.py rebuilds the packet from the json and no
server is needed again.
"""

import json
import os
import socket
import struct
import sys
import zlib

PROTOCOL_VERSION = 777
OUT_DIR = "captured"

CLIENTBOUND = {
    "handshaking": {0: "intention"},
    "status": {0: "status_response", 1: "pong_response"},
    "login": {
        0: "login_disconnect",
        1: "hello",
        2: "login_finished",
        3: "login_compression",
        4: "custom_query",
        5: "cookie_request",
    },
    "configuration": {
        1: "custom_payload",
        2: "disconnect",
        3: "finish_configuration",
        4: "keep_alive",
        5: "ping",
        6: "reset_chat",
        7: "registry_data",
        8: "remove_resource_pack",
        9: "add_resource_pack",
        10: "store_cookie",
        11: "transfer",
        12: "clear_dialog",
        13: "update_enabled_features",
        14: "update_tags",
        15: "select_known_packs",
        16: "show_dialog",
    },
    "play": {
        0: "bundle_delimiter",
        8: "player_chat",
        9: "player_combat_end",
        11: "client_information",
        12: "cookie_request",
        14: "custom_payload",
        18: "disconnect",
        26: "add_entity",
        40: "update_attributes",
        44: "login",
        45: "game_state_change",
        46: "difficulty",
        52: "tick",
        53: "keep_alive",
        55: "set_default_spawn",
        64: "start_configuration",
    },
}

SERVERBOUND = {
    "login": {0: "hello", 1: "encryption_response", 3: "login_acknowledged"},
    "configuration": {
        0: "client_information",
        1: "cookie_response",
        2: "custom_payload",
        3: "finish_configuration",
        4: "keep_alive",
        5: "pong",
        6: "resource_pack_response",
        7: "select_known_packs",
    },
    "play": {
        0: "confirm_teleportation",
        6: "client_command",
        23: "keep_alive",
        32: "position",
    },
}


def varint(value):
    value &= 0xFFFFFFFF
    out = bytearray()
    while True:
        b = value & 0x7F
        value >>= 7
        if value:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def read_varint(data, offset=0):
    result = 0
    shift = 0
    pos = offset
    while True:
        if pos >= len(data):
            raise EOFError("ran out of data reading a VarInt")
        b = data[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not b & 0x80:
            return result, pos
        shift += 7
        if shift > 35:
            raise EOFError("VarInt too long")


def read_string(data, offset=0):
    length, offset = read_varint(data, offset)
    return data[offset : offset + length].decode("utf-8"), offset + length


class Recorder:
    """Collects frames in wire order and writes them out at the end.

    Registry entry names are deliberately not written as a separate index. The
    entry payloads are NBT whose length is unknown without a reader for it, so
    recovering the names means scanning, and a scan finds some strings inside
    the NBT that look like identifiers. The result was missing entries, which
    shifts every later id, and a file like that is worse than no file: it looks
    authoritative. The frames in packets.bin are exact, and the tag packet in
    extracted/tags.json is parsed properly.
    """

    def __init__(self):
        self.frames = bytearray()
        self.index = []
        self.tags = {}
        self.update_tags = None

    def add(self, direction, phase, packet_id, name, body):
        self.frames += varint(len(body)) + body
        self.index.append(
            {
                "i": len(self.index),
                "dir": direction,
                "phase": phase,
                "id": packet_id,
                "name": name,
                "size": len(body),
            }
        )

    def flush(self):
        os.makedirs(OUT_DIR, exist_ok=True)

        with open(os.path.join(OUT_DIR, "packets.bin"), "wb") as f:
            f.write(self.frames)

        with open(os.path.join(OUT_DIR, "packets.jsonl"), "w", encoding="utf-8") as f:
            for row in self.index:
                f.write(json.dumps(row) + "\n")

        os.makedirs("extracted", exist_ok=True)
        with open("extracted/tags.json", "w", encoding="utf-8") as f:
            json.dump(self.tags, f, indent=1, sort_keys=True)

        registries = len(self.tags)
        tags = sum(len(v) for v in self.tags.values())

        print(f"\nwrote {OUT_DIR}/packets.bin ({len(self.frames)} bytes, {len(self.index)} frames)")
        print(f"wrote {OUT_DIR}/packets.jsonl")
        print(f"wrote extracted/tags.json ({registries} registries, {tags} tags)")
        if self.update_tags is None:
            print("no update_tags packet: the run stopped before it")


class Client:
    """A connection that tracks the compression threshold.

    The threshold applies from the packet *after* the one that announces it,
    which is the detail that makes or breaks this: read the announcement fully,
    then switch, or the next frame is parsed with the wrong framing.
    """

    def __init__(self, host, port, recorder):
        self.sock = socket.create_connection((host, port), timeout=10)
        self.threshold = None
        self.buf = b""
        self.rec = recorder
        self.phase = "login"
        self.dir = "s2c"

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
        name = SERVERBOUND.get(self.phase, {}).get(packet_id, f"{self.phase}:{packet_id}")
        self.rec.add("c2s", self.phase, packet_id, name, body)
        print(f"-> {name} ({packet_id}) {len(body)} bytes")

    def send_handshake(self):
        host = b"127.0.0.1"
        payload = (
            varint(PROTOCOL_VERSION)
            + varint(len(host))
            + host
            + struct.pack(">H", self.port)
            + varint(2)
        )
        body = varint(0) + payload
        self.sock.sendall(varint(len(body)) + body)
        self.rec.add("c2s", "handshaking", 0, "intention", body)

    def _fill(self):
        self.sock.settimeout(20)
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

        # Every frame carries an inner length once compression is on. A zero
        # means the payload follows as it is, which is the common case for the
        # small packets in the configuration phase.
        data_len, consumed = read_varint(raw)
        rest = raw[consumed:]
        if data_len == 0:
            return rest
        return zlib.decompress(rest)

    def receive(self, body):
        packet_id, offset = read_varint(body)
        name = CLIENTBOUND.get(self.phase, {}).get(packet_id, f"{self.phase}:{packet_id}")
        self.rec.add("s2c", self.phase, packet_id, name, body)
        return packet_id, offset




def parse_tags(body, offset):
    """Update tags: registry -> tag -> ids."""
    registry_count, offset = read_varint(body, offset)

    out = {}
    for _ in range(registry_count):
        registry, offset = read_string(body, offset)
        tag_count, offset = read_varint(body, offset)

        entries = {}
        for _ in range(tag_count):
            tag_name, offset = read_string(body, offset)
            id_count, offset = read_varint(body, offset)
            ids = []
            for _ in range(id_count):
                value, offset = read_varint(body, offset)
                ids.append(value)
            entries[tag_name] = ids

        out[registry] = entries

    return out, body[offset:]


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25566

    rec = Recorder()
    c = Client(host, port, rec)
    c.port = port

    c.send_handshake()

    name = b"filament-probe"
    c.send(0, varint(len(name)) + name + b"\x00" * 16)

    play_packets = 0

    while True:
        try:
            body = c.recv()
        except (EOFError, zlib.error) as e:
            print(f"connection ended: {type(e).__name__}: {e}")
            break
        if body is None:
            print("no more packets")
            break

        packet_id, offset = c.receive(body)
        pretty = c.rec.index[-1]["name"]
        print(f"<- {pretty} ({packet_id}) {len(body)} bytes")

        if c.phase == "login":
            if packet_id == 3:
                threshold, _ = read_varint(body, offset)
                print(f"   threshold {threshold}")
                c.threshold = threshold
            elif packet_id == 2:
                print("   login accepted")
                c.send(3)
                c.phase = "configuration"

                # The settings go out as soon as the state changes rather than
                # at the end. The server waits on them and stops sending
                # registry data until it has them.
                c.send(
                    0,
                    varint(5)
                    + b"en_us"
                    + bytes([8])
                    + varint(0)
                    + bytes([1, 0])
                    + varint(1)
                    + bytes([0, 1])
                    + varint(0),
                )
            elif packet_id == 0:
                print(f"   REFUSED: {body[offset:][:200]}")

        elif c.phase == "configuration":
            if packet_id == 2:
                print(f"   REFUSED: {body[offset:][:300]}")
                break
            if packet_id == 7:
                key, o = read_string(body, offset)
                print(f"   {key}")
            elif packet_id == 15:
                packs, o = read_varint(body, offset)
                offered = []
                for _ in range(packs):
                    pack, o = read_string(body, o)
                    offered.append(pack)
                print(f"   offers {packs} packs: {offered}")
                c.send(7, varint(0))
            elif packet_id == 14:
                tags, tail = parse_tags(body, offset)
                rec.tags = tags
                rec.update_tags = tail
                print(f"   {len(tags)} registries carry tags")
                for key, entries in sorted(tags.items()):
                    print(f"     {key}: {len(entries)} tags")
            elif packet_id == 3:
                print("   finish_configuration")
                c.send(3)
                c.phase = "play"
                print("   entered the play state")

        elif c.phase == "play":
            play_packets += 1
            if packet_id == 18:
                print(f"   REFUSED: {body[offset:][:200]}")
                break
            if packet_id == 44:
                print(f"   login play packet, {len(body)} bytes")
            if play_packets >= 40:
                print("   stopping after 40 play packets")
                break


    rec.flush()


if __name__ == "__main__":
    main()
