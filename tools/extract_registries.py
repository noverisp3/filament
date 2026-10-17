"""Extracts the registry_data frames out of a capture into one asset.

    python tools/extract_registries.py

Reads captured/packets.bin together with its index and writes
assets/registry_data.bin, which is the same length-prefixed framing
capture.py writes. Rust embeds that file and hands each frame straight to the
encoder, so what goes on the wire is byte for byte what a vanilla server sent.

Both directions land in packets.bin, so the index is what separates them:
the frames wanted are the ones the server sent during the configuration phase
with the registry_data id. A frame the client sent can share that id in a later
phase, which is why the direction and phase are both checked rather than just
the id.

Run capture.py first. The output does not need a server to rebuild once
extracted/tags.json exists, but the frames themselves can only be captured.
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from capture import read_varint, varint  # noqa: E402

PACKETS = os.path.join("captured", "packets.bin")
INDEX = os.path.join("captured", "packets.jsonl")
TARGET = os.path.join("assets", "registry_data.bin")


def walk_frames(data):
    """Yields each frame body in wire order."""
    offset = 0
    while offset < len(data):
        length, offset = read_varint(data, offset)
        if offset + length > len(data):
            raise ValueError("the last frame is truncated")
        yield data[offset : offset + length]
        offset += length


def main():
    if not os.path.exists(PACKETS):
        print(f"{PACKETS} is missing; run tools/capture.py against a server first")
        return 1

    with open(INDEX, encoding="utf-8") as f:
        index = [json.loads(line) for line in f if line.strip()]

    with open(PACKETS, "rb") as f:
        data = f.read()

    frames = list(walk_frames(data))
    if len(frames) != len(index):
        print(
            f"{len(frames)} frames but {len(index)} index rows; "
            "the capture was cut short and the two cannot be lined up"
        )
        return 1

    out = bytearray()
    kept = []

    for frame, row in zip(frames, index):
        if row["dir"] != "s2c":
            continue
        if row["phase"] != "configuration":
            continue
        if row["name"] != "registry_data":
            continue

        packet_id, offset = read_varint(frame)
        if packet_id != 7:
            print(f"index says registry_data but the frame carries id {packet_id}")
            return 1

        out += varint(len(frame)) + frame
        kept.append(row["size"])

    if not out:
        print("no registry_data frames in the capture")
        return 1

    os.makedirs("assets", exist_ok=True)
    with open(TARGET, "wb") as f:
        f.write(out)

    print(f"{len(kept)} registry_data frames, {len(out)} bytes")
    print(f"largest {max(kept)} bytes, smallest {min(kept)}")
    print(f"wrote {TARGET}")


if __name__ == "__main__":
    sys.exit(main())