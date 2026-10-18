"""Checks that a client's handshake and login hello can arrive together.

The failure this exists for: a client sends its handshake and its login hello back
to back, both usually in one segment. A server that reads the handshake and then
starts the next phase with a fresh decoder drops the hello on the floor, then
waits for a packet that has already arrived. The client times out and hangs up,
and the log says "closed before the login hello arrived", which is true and
points at the wrong thing.

Whether the two packets are segmented together is up to the network, so the
symptom comes and goes. An intermittent failure that only a real client can
produce is a failure that gets fixed by guessing.

This sends both in a single write, which forces the case, and looks for the
player's name coming back in the server's answer. It does not parse the answer:
reading it properly means tracking which frames are compressed and how long each
uncompressed one is, which is most of a client.

    python tools/check_login.py [port]

It covers the handshake to login handoff and nothing else. Configuration, play,
chunks and the world are exercised by a real client.
"""

import socket
import struct
import sys
import uuid

HOST, PORT = "127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 25565
PROTOCOL = 777
NAME = "ProbePlayer"


def varint(value):
    out = b""
    while True:
        byte = value & 0x7F
        value >>= 7
        out += bytes([byte | (0x80 if value else 0)])
        if not value:
            return out


def string(text):
    raw = text.encode()
    return varint(len(raw)) + raw


def frame(packet_id, body=b""):
    inner = varint(packet_id) + body
    return varint(len(inner)) + inner


with socket.create_connection((HOST, PORT), timeout=15) as sock:
    sock.settimeout(10)

    # One write, both packets. This is the case that breaks.
    sock.sendall(
        frame(0, varint(PROTOCOL) + string(HOST) + struct.pack(">H", PORT) + varint(2))
        + frame(0, string(NAME) + uuid.UUID(int=0).bytes)
    )

    print("sent handshake and login hello in a single write")

    received = b""
    try:
        while True:
            data = sock.recv(4096)
            if not data:
                break
            received += data
    except socket.timeout:
        pass

    print(f"  ..   {len(received)} bytes back")

    problems = []

    # The compression frame is the server saying it read the hello, since it
    # sends that before anything else and never otherwise.
    if b"\x03\x03" not in received[:8]:
        problems.append("no login_compression, so the hello was never read")

    if NAME.encode() not in received:
        problems.append(f"the answer does not name {NAME!r}")

    for problem in problems:
        print(f"FAIL: {problem}")

    if problems:
        print(f"      got {received.hex()[:200]}")
        sys.exit(1)

    print("  ok   compression was announced")
    print(f"  ok   the profile came back naming {NAME}")
    print()
    print("PASS: a handshake and a hello in one segment both got through")
    print()
    print("Covers the handshake to login handoff only. Configuration, play, chunks")
    print("and the world are exercised by a real client.")
