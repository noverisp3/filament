"""Sends a real status handshake, so a regression in accept() shows up.

The smoke test that came before this one opened a socket and closed it,
which a broken accept path passes: nothing is ever read, so nothing goes
wrong. This sends the three packets a client sends when it looks at a
server in the browser, and reads the answer back.
"""

import socket
import struct
import sys

HOST, PORT = "127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 25565


def varint(value):
    out = b""
    while True:
        byte = value & 0x7F
        value >>= 7
        out += bytes([byte | (0x80 if value else 0)])
        if not value:
            return out


def packet(packet_id, body):
    inner = varint(packet_id) + body
    return varint(len(inner)) + inner


with socket.create_connection((HOST, PORT), timeout=10) as s:
    # Handshake: protocol 777, "localhost", port 25565, next state = status.
    host = b"localhost"
    s.sendall(
        packet(
            0,
            varint(777) + varint(len(host)) + host + struct.pack(">H", PORT) + varint(1),
        )
    )
    # Status request.
    s.sendall(packet(0, b""))

    # Read the length-prefixed answer back.
    head = s.recv(1)
    if not head:
        print("FAIL: the server closed without answering the status request")
        sys.exit(1)

    length = head[0]
    while length & 0x80:
        length = (length & 0x7F) | (s.recv(1)[0] << 7)

    data = b""
    while len(data) < length:
        chunk = s.recv(length - len(data))
        if not chunk:
            break
        data += chunk

    if b'"version"' in data:
        print(f"PASS: status answered with {len(data)} bytes")
        print("  ", data[:160].decode("utf-8", "replace"))
    else:
        print(f"FAIL: unexpected answer: {data[:160]!r}")
        sys.exit(1)
