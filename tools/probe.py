"""Acts as a Minecraft client for the handshake and status exchange.

Speaks the wire format directly rather than using a library, so a mismatch
between this and the server is a mismatch against the format itself and not
against somebody else's idea of it.

    python tools/probe.py [host] [port]
"""

import socket
import struct
import sys


def varint(value):
    """Seven bits per byte, little endian, high bit continues."""
    out = bytearray()
    v = value & 0xFFFFFFFF
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def read_varint(sock):
    result = 0
    shift = 0
    for _ in range(5):
        b = sock.recv(1)
        if not b:
            raise EOFError("connection closed while reading a VarInt")
        result |= (b[0] & 0x7F) << shift
        if not b[0] & 0x80:
            return result
        shift += 7
    raise ValueError("VarInt too long")


def frame(packet_id, payload=b""):
    body = varint(packet_id) + payload
    return varint(len(body)) + body


def handshake(host, port, server_port, intent):
    host_bytes = host.encode()
    payload = varint(777) + varint(len(host_bytes)) + host_bytes
    # Big-endian, unlike a VarInt. Getting this wrong does not corrupt
    # anything obvious, it just puts the server on the wrong port.
    payload += struct.pack(">H", server_port)
    payload += varint(intent)
    return frame(0x00, payload)


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25565

    sock = socket.create_connection((host, port), timeout=5)
    print(f"connected to {host}:{port}")

    # Intent 1 is status.
    sock.sendall(handshake(host, port, port, 1))
    print("sent handshake with intent=status (1)")

    # Status request, packet id 0.
    sock.sendall(frame(0x00))
    print("sent status request")

    length = read_varint(sock)
    print(f"response frame length: {length}")

    data = b""
    while len(data) < length:
        chunk = sock.recv(length - len(data))
        if not chunk:
            raise EOFError("closed mid-frame")
        data += chunk

    # Body is: packet id, string length, JSON.
    i = 0
    while data[i] & 0x80:
        i += 1
    i += 1
    str_len = 0
    shift = 0
    while True:
        b = data[i]
        i += 1
        str_len |= (b & 0x7F) << shift
        if not b & 0x80:
            break
        shift += 7

    json_bytes = data[i:i + str_len]
    print(f"packet id byte: {data[0]}")
    print(f"status JSON ({str_len} bytes):")
    print(json_bytes.decode("utf-8"))

    import json
    parsed = json.loads(json_bytes)
    assert parsed["version"]["protocol"] == 777, parsed
    print("parsed OK, protocol", parsed["version"]["protocol"])

    # Ping, to measure the round trip the way the client does.
    sock.sendall(frame(0x01, varint(123456789)))
    length = read_varint(sock)
    data = b""
    while len(data) < length:
        data += sock.recv(length - len(data))
    print(f"pong frame: {data.hex()}")

    sock.close()
    print("OK")


if __name__ == "__main__":
    main()