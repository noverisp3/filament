"""Checks every packet name in the server's log tables against packets.json.

The tables in src/server/log.rs were written by hand, and being wrong about them
is worse than having no name at all: "play-s2c:22" is honest, while a confident
wrong name sends whoever is reading the log looking for the wrong packet. Three
of the five play names this checks out were wrong.

The capture names live in tools/capture.py and are checked here too, so the two
cannot drift apart.

    python tools/check_names.py
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load_protocol():
    with open(ROOT / "extracted" / "packets.json", encoding="utf-8") as f:
        return json.load(f)


def protocol_names(protocol, state, direction):
    """Looks a direction up, tolerating states that have only one.

    `status` has a clientbound and a serverbound table, but `handshaking` only
    has one, so a state is not guaranteed to have both.
    """
    table = protocol[state].get(direction)
    if table is None:
        return {}
    return {int(entry["protocol_id"]): name.split(":")[-1] for name, entry in table.items()}


def names_from_rust(path):
    """Pulls the (state, id) -> name pairs out of a Rust match arm."""
    text = (ROOT / path).read_text(encoding="utf-8")
    arm = re.compile(r'\("([a-z0-9-]+)",\s*(-?\d+)\)\s*=>\s*"([a-z0-9_]+)"')
    out = {}
    for state, pid, name in arm.findall(text):
        out[(state, int(pid))] = name
    return out


def names_from_capture():
    """Pulls the tables out of tools/capture.py."""
    text = (ROOT / "tools" / "capture.py").read_text(encoding="utf-8")
    arm = re.compile(r'^\s*(\d+):\s*"([a-z0-9_]+)",', re.MULTILINE)
    out = {}
    for block in re.finditer(r'^(\w+)\s*=\s*\{(.*?)^\s*\}', text, re.MULTILINE | re.DOTALL):
        state = block.group(1)
        for pid, name in arm.findall(block.group(2)):
            out[(state.lower().replace("-s2c", ""), int(pid))] = name
    return out


def check(label, found, truth, direction):
    bad = []
    for (state, pid), name in sorted(found.items()):
        if state not in truth:
            continue
        expected = truth[state].get(pid)
        if expected is None:
            continue
        if expected != name:
            bad.append((state, pid, name, expected))

    print(f"{label}: {len(found)} entries, {len(bad)} wrong")
    for state, pid, name, expected in bad:
        print(f"  {state} {pid}: says {name!r}, protocol says {expected!r}")
    return bad


def main():
    protocol = load_protocol()

    failures = 0

    # The log tables use one name per id per state, and the direction is carried
    # in the state name with a -s2c suffix, so both directions land in one map.
    truth = {}
    for state in protocol:
        for pid, packet in protocol_names(protocol, state, "clientbound").items():
            truth.setdefault(state, {})[pid] = packet
        for pid, packet in protocol_names(protocol, state, "serverbound").items():
            truth.setdefault(state + "-s2c", {})[pid] = packet

    failures += len(check("src/server/log.rs", names_from_rust("src/server/log.rs"), truth, "both"))
    failures += len(check("tools/capture.py", names_from_capture(), truth, "both"))

    if failures:
        print(f"\n{failures} names are wrong")
        return 1

    print("\nevery name matches")
    return 0


if __name__ == "__main__":
    sys.exit(main())