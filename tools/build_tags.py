"""Builds the update_tags payload from the captured tag map.

The wire format is a map, so the order the registries and tags appear in does
not matter to the client. That is what makes this possible: the capture that
produced extracted/tags.json reached the tag packet, and the raw bytes were not
kept from that run, but the map it contained is enough to write the packet
again byte for byte in content if not in order.

    python tools/build_tags.py

Writes assets/update_tags.bin, the body of ClientboundUpdateTagsPacket with the
packet id stripped.
"""

import json
import os
import sys


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


def write_string(out, value):
    raw = value.encode("utf-8")
    out += varint(len(raw))
    out += raw


def main():
    source = "extracted/tags.json"
    target = "assets/update_tags.bin"

    with open(source, encoding="utf-8") as f:
        tags = json.load(f)

    registries = sorted(tags)

    out = bytearray()
    out += varint(len(registries))

    total_tags = 0
    for registry in registries:
        write_string(out, registry)

        names = sorted(tags[registry])
        out += varint(len(names))
        total_tags += len(names)

        for tag in names:
            write_string(out, tag)
            ids = tags[registry][tag]
            out += varint(len(ids))
            for value in ids:
                out += varint(value)

    os.makedirs("assets", exist_ok=True)
    with open(target, "wb") as f:
        f.write(out)

    print(f"{len(registries)} registries, {total_tags} tags")
    print(f"wrote {target} ({len(out)} bytes)")

    # The ids have to line up with the registry each tag belongs to, and the
    # only check available without the server is the width: a tag on a registry
    # with a different number of entries would shift every later id.
    widest = {}
    for registry in registries:
        for tag in tags[registry]:
            high = max(tags[registry][tag]) if tags[registry][tag] else -1
            widest[registry] = max(widest.get(registry, -1), high)

    problems = []
    for registry, high in sorted(widest.items()):
        if high >= 64:
            problems.append(f"{registry}: id {high}")

    if problems:
        print("\nids above 63, so these need more than a byte:")
        for line in problems:
            print(f"  {line}")
        print("\nThis file has to stay under the 32 KiB compression threshold,")
        print("or FrameEncoder will compress it and the byte comparison will not")
        print("match what the capture saw.")
    else:
        print("\nall ids fit in one byte")


if __name__ == "__main__":
    sys.exit(main())