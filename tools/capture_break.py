"""Captures what a server sends back for one block break, verbatim.

capture_play.py watches a server from the outside and never acts, so it cannot
see the answer to the only question that matters: when a block is broken, what
does the server actually put on the wire. Every answer to that had so far come
from reading the client's bytecode, and the bytecode says the block is gone
while the client walks on air.

So this behaves like the client for long enough to break one block, and writes
down every byte of every packet that comes back in answer.

    python tools/capture_break.py [host] [port]

Writes captured/break/, and prints a decode of anything block related:

    block_update        id 8   pos + state
    block_changed_ack   id 4   sequence
    block_destruction   id 5   entity, pos, stage
    level_event         id 47  type, pos, data
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
OUT = os.path.join("captured", "break")

# Serverbound play.
SB_ACCEPT_TELEPORT = 0
SB_CLIENT_TICK_END = 13
SB_KEEP_ALIVE = 28
SB_PING_REQUEST = 38
SB_PLAYER_ACTION = 41
SB_PLAYER_POSITION = 31
CB_SOUND = 119
SB_CHANGE_GAME_MODE = 5

# ServerboundPlayerActionPacket$Action, in declaration order.
START_DESTROY_BLOCK = 0
STOP_DESTROY_BLOCK = 3

# Clientbound play.
CB_KEEP_ALIVE = 45
CB_PING = 62
CB_PLAYER_POSITION = 73
CB_LEVEL_CHUNK = 46
CB_DISCONNECT = 32

# The block related ones this is here to see.
CB_BLOCK_UPDATE = 8
CB_BLOCK_CHANGED_ACK = 4
CB_BLOCK_DESTRUCTION = 5
CB_LEVEL_EVENT = 47
CB_SECTION_BLOCKS_UPDATE = 86

# The item that a broken block leaves behind, and the picking up of it.
CB_ADD_ENTITY = 1
CB_SET_ENTITY_DATA = 101
CB_TAKE_ITEM_ENTITY = 127
CB_REMOVE_ENTITIES = 44

# The damage that was done, and the health that is left.
CB_DAMAGE_EVENT = 25
CB_SET_HEALTH = 106

INTERESTING = {
    CB_BLOCK_UPDATE: "block_update",
    CB_BLOCK_CHANGED_ACK: "block_changed_ack",
    CB_BLOCK_DESTRUCTION: "block_destruction",
    CB_LEVEL_EVENT: "level_event",
    CB_SECTION_BLOCKS_UPDATE: "section_blocks_update",
    CB_ADD_ENTITY: "add_entity",
    CB_SET_ENTITY_DATA: "set_entity_data",
    CB_TAKE_ITEM_ENTITY: "take_item_entity",
    CB_REMOVE_ENTITIES: "remove_entities",
    CB_DAMAGE_EVENT: "damage_event",
    CB_SET_HEALTH: "set_health",
}


def pack_block_pos(x, y, z):
    return ((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)


def unpack_block_pos(packed):
    x = packed >> 38
    z = (packed << 26) >> 38
    y = ((packed << 52) >> 52) & 0xFFF
    if y >= 2048:
        y -= 4096
    return x, y, z


def read_block_pos(buf, offset):
    return unpack_block_pos(struct.unpack_from(">q", buf, offset)[0]), offset + 8


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


def decode(pid, body, offset):
    """Turns a block related packet into something readable by eye."""
    try:
        if pid == CB_BLOCK_UPDATE:
            (x, y, z), at = read_block_pos(body, offset)
            state, _ = read_varint(body, at)
            return f"pos {x} {y} {z} state {state}"
        if pid == CB_BLOCK_CHANGED_ACK:
            seq, _ = read_varint(body, offset)
            return f"sequence {seq}"
        if pid == CB_BLOCK_DESTRUCTION:
            entity, at = read_varint(body, offset)
            (x, y, z), at = read_block_pos(body, at)
            stage = struct.unpack_from(">b", body, at)[0]
            return f"entity {entity} pos {x} {y} {z} stage {stage}"
        if pid == CB_LEVEL_EVENT:
            kind = struct.unpack_from(">i", body, offset)[0]
            (x, y, z), at = read_block_pos(body, offset + 4)
            data = struct.unpack_from(">i", body, at)[0]
            glob = body[at + 4]
            return f"type {kind} pos {x} {y} {z} data {data} global {glob}"
        if pid == CB_SECTION_BLOCKS_UPDATE:
            # one short section y, then a list of (short pos, short state) pairs
            section_y = struct.unpack_from(">h", body, offset)[0]
            count = struct.unpack_from(">H", body, offset + 2)[0]
            at = offset + 4
            pairs = []
            for _ in range(count):
                short, state = struct.unpack_from(">HH", body, at)
                at += 4
                pairs.append(
                    f"({short & 0xF},{short >> 8},{short >> 4})->{state}"
                )
            return f"section y {section_y}, {count} blocks: " + " ".join(pairs)
        if pid == CB_ADD_ENTITY:
            entity, at = read_varint(body, offset)
            uuid = body[at : at + 16]
            at += 16
            kind, at = read_varint(body, at)
            x, y, z = struct.unpack_from(">ddd", body, at)
            at += 24
            return (
                f"entity {entity} type {kind} at {x:.2f} {y:.2f} {z:.2f} "
                f"uuid {uuid.hex()[:8]}"
            )
        if pid == CB_SET_ENTITY_DATA:
            entity, at = read_varint(body, offset)
            accessor = body[at]
            at += 1
            serializer, at = read_varint(body, at)
            count, at = read_varint(body, at)
            item, at = read_varint(body, at)
            return (
                f"entity {entity} piece {accessor} serializer {serializer} "
                f"count {count} item {item}"
            )
        if pid == CB_TAKE_ITEM_ENTITY:
            item, at = read_varint(body, offset)
            player, at = read_varint(body, at)
            amount, at = read_varint(body, at)
            return f"item {item} to player {player}, {amount} of them"
        if pid == CB_REMOVE_ENTITIES:
            count, at = read_varint(body, offset)
            ids = []
            for _ in range(count):
                entity, at = read_varint(body, at)
                ids.append(entity)
            return "entities " + " ".join(str(i) for i in ids)
        if pid == CB_DAMAGE_EVENT:
            entity, at = read_varint(body, offset)
            kind, at = read_varint(body, at)
            cause, at = read_varint(body, at)
            direct, at = read_varint(body, at)
            has_place = body[at]
            at += 1
            where = ""
            if has_place:
                x, y, z = struct.unpack_from(">ddd", body, at)
                where = f" at {x:.1f} {y:.1f} {z:.1f}"
            return (
                f"entity {entity} damage type {kind} cause {cause - 1} "
                f"direct {direct - 1}{where}"
            )
        if pid == CB_SET_HEALTH:
            health = struct.unpack_from(">f", body, offset)[0]
            food, _ = read_varint(body, offset + 4)
            return f"health {health} food {food}"
    except Exception as e:  # noqa: BLE001
        return f"undecodable: {type(e).__name__}: {e}"
    return None


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
    position = None
    chunks = 0
    broke = False
    fell = False
    started = None
    index = []
    caught = []
    counts = {}

    print(f"connecting to {host}:{port}")
    deadline = time.time() + 240

    while time.time() < deadline:
        try:
            body = c.recv()
        except (EOFError, zlib.error) as e:
            print(f"connection ended: {type(e).__name__}: {e}")
            break
        if body is None:
            time.sleep(0.1)
            continue

        pid, offset = read_varint(body, 0)

        if state == "login":
            if pid == 3:
                c.threshold, _ = read_varint(body, offset)
                print(f"compression on at {c.threshold}")
            elif pid == 2:
                c.send(3)
                state = "configuration"
                print("configuration")
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
            break
        if pid == CB_KEEP_ALIVE:
            c.send(SB_KEEP_ALIVE, body[offset:])
        elif pid == CB_PING:
            c.send(SB_PING_REQUEST, body[offset:])
        elif pid == CB_PLAYER_POSITION:
            teleport, at = read_varint(body, offset)
            position = struct.unpack_from(">ddd", body, at)
            rotation = struct.unpack_from(">ff", body, at + 24)
            c.send(
                SB_ACCEPT_TELEPORT,
                varint(teleport)
                + body[at : at + 24]
                + body[at + 24 : at + 32],
            )
            c.send(SB_CLIENT_TICK_END)
            print(f"spawned at {position}")
            # Survival will not take a break action from a probe that has no
            # tool and no business being there. Creative will.
            c.send(SB_CHANGE_GAME_MODE, varint(1))
            print("asked for creative")
        elif pid == CB_LEVEL_CHUNK:
            chunks += 1
            if chunks == 1:
                c.send(SB_CLIENT_TICK_END)

        if pid in INTERESTING:
            described = decode(pid, body, offset)
            caught.append({"id": pid, "name": INTERESTING[pid], "desc": described,
                           "size": len(body), "hex": body.hex()})
            print(f"<- {INTERESTING[pid]}: {described}  ({len(body)} bytes)")

        if pid == CB_SOUND:
            sound, o = read_varint(body, offset)
            print(f"<- SOUND id {sound}")

        index.append({"dir": "s2c", "phase": state, "id": pid, "size": len(body)})
        if pid not in INTERESTING:
            counts[pid] = counts.get(pid, 0) + 1
            if broke:
                print(f"<- after break id {pid} ({len(body)} bytes)")
            elif state == "play" and counts[pid] <= 2:
                print(f"<- play id {pid} ({len(body)} bytes)")

        # Break the block the player is standing on, once there is ground
        # under them to break and the chunks around it have arrived.
        if (
            state == "play"
            and not broke
            and position is not None
            and chunks > 20
        ):
            broke = True
            started = time.time()
            bx = int(position[0] // 1)
            bz = int(position[2] // 1)
            for by in (int(position[1] // 1) - 1, int(position[1] // 1) - 2):
                packed = pack_block_pos(bx, by, bz)
                print(f"breaking {bx} {by} {bz}")
                c.send(
                    SB_PLAYER_ACTION,
                    varint(START_DESTROY_BLOCK) + struct.pack(">q", packed) + b"\x01" + varint(1),
                )
                for _ in range(4):
                    time.sleep(0.2)
                    c.send(SB_CLIENT_TICK_END)
                time.sleep(0.8)
                c.send(
                    SB_PLAYER_ACTION,
                    varint(STOP_DESTROY_BLOCK) + struct.pack(">q", packed) + b"\x01" + varint(2),
                )
                time.sleep(1.2)
            continue

        if broke and not fell and time.time() - started > 6:
            print("six seconds after the break")
            # Now fall, so that the server tells us the sound of being hurt
            # and what it says was the damage.
            #
            # The server refuses a move of more than a hundred blocks, so the
            # fall is a staircase rather than one jump, and the last step says
            # the player is on the ground: that is what makes the landing hurt.
            def at(y, on_ground):
                return (
                    struct.pack(">ddd", 0.5, y, 0.5)
                    + struct.pack(">ff", 0.0, 0.0)
                    + bytes([1 if on_ground else 0])
                )

            c.send(SB_PLAYER_POSITION, at(60.0, False))
            print("went up")
            for step, y in enumerate([40.0, 20.0, 0.0, -20.0]):
                for _ in range(10):
                    time.sleep(0.05)
                    c.send(SB_CLIENT_TICK_END)
                c.send(SB_PLAYER_POSITION, at(y, False))
                print(f"fell to {y}")
            for _ in range(10):
                time.sleep(0.05)
                c.send(SB_CLIENT_TICK_END)
            c.send(SB_PLAYER_POSITION, at(-7.0, True))
            print("landed")
            fell_at = time.time()
            fell = True
            continue

        if broke and time.time() - started > 26:
            print("twenty seconds after the fall")
            break

    with open(os.path.join(OUT, f"{port}_index.jsonl"), "w", encoding="utf-8") as f:
        for row in index:
            f.write(json.dumps(row) + "\n")

    with open(os.path.join(OUT, f"{port}_blocks.json"), "w", encoding="utf-8") as f:
        json.dump(caught, f, indent=2)

    print(f"\n{len(caught)} block related packets on {port}:")
    for row in caught:
        print(f"   {row['name']:20} {row['desc']}")
    print(f"\npackets by id: {sorted(counts.items())}")


if __name__ == "__main__":
    main()