"""Compares the light two servers sent for the same chunks.

    python tools/light_diff.py <one.json> <other.json>

The light of a chunk is the same in both, or one of them is wrong: the same
terrain, the same lamps, the same sun. So the first chunk where the two
differ is the chunk where one of them is.
"""

import io
import json
import sys


def cells(hex_data):
    raw = bytes.fromhex(hex_data)
    out = []
    for index in range(len(raw) * 2):
        byte = raw[index // 2]
        out.append((byte >> 4) if index % 2 else (byte & 0x0F))
    return out


def main():
    one = json.load(io.open(sys.argv[1], encoding='utf-8'))
    two = json.load(io.open(sys.argv[2], encoding='utf-8'))
    shared = sorted(set(one) & set(two))
    print(f"{len(shared)} chunks in both")
    for key in shared:
        for layer in ('sky', 'block'):
            a = one[key][layer]
            b = two[key][layer]
            if len(a) != len(b):
                print(f"chunk {key} {layer}: {len(a)} sections and {len(b)}")
                continue
            for index, (x, y) in enumerate(zip(a, b)):
                if x == y:
                    continue
                cells_x = cells(x)
                cells_y = cells(y)
                differing = sum(1 for p, q in zip(cells_x, cells_y) if p != q)
                lit_x = sum(1 for v in cells_x if v > 0)
                lit_y = sum(1 for v in cells_y if v > 0)
                print(f"chunk {key} {layer} section {index}: DIFFER, "
                      f"{differing} cells differ; {lit_x} lit here, "
                      f"{lit_y} lit there")
                break


if __name__ == '__main__':
    main()
