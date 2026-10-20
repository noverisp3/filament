"""Looks at the light a server sent and finds the lamps in it.

    python tools/light_finder.py <captured.json>

A glowstone is a block of light 15, and the light falls off by one a block.
So a section that holds a lamp has a ring of 14 around it, then 13, and so
on: a staircase in the data. Finding it says whether the server sent the
lamp's light or not.
"""

import io
import json
import sys

SECTION_DIM = 16


def cells(hex_data):
    """The light of every cell of a section, in the order the game reads them."""
    raw = bytes.fromhex(hex_data)
    out = []
    for index in range(SECTION_DIM * SECTION_DIM * SECTION_DIM):
        byte = raw[index // 2]
        nibble = (byte >> 4) if index % 2 else (byte & 0x0F)
        out.append(nibble)
    return out


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else r'captured/light_compare.json'
    chunks = json.load(io.open(path, encoding='utf-8'))
    for key, chunk in sorted(chunks.items()):
        for layer, name in ((chunk['sky'], 'sky'), (chunk['block'], 'block')):
            for index, hex_data in enumerate(layer):
                light = cells(hex_data)
                brightest = max(light)
                if brightest < 10:
                    continue
                # Where is it, and what does the light look like around it?
                at = light.index(brightest)
                x = at & 0xF
                z = (at >> 4) & 0xF
                y = (at >> 8) & 0xF
                ring = []
                for distance in range(0, 6):
                    value = light[((y) << 8) | (z << 4) | x] if False else None
                # A row through the lamp.
                row = []
                for dx in range(-4, 5):
                    cell = (y << 8) | (z << 4) | ((x + dx) & 0xF)
                    row.append(light[cell])
                print(f"chunk {key} {name} section {index}: brightest {brightest} "
                      f"at x {x} y {y} z {z}")
                print(f"    a row through it: {row}")
                # How many cells are lit at all?
                lit = sum(1 for v in light if v > 0)
                print(f"    {lit} cells have light in them of {len(light)}")


if __name__ == '__main__':
    main()
