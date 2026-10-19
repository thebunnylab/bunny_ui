"""Require native attachment and nonzero tenant size before document submission.

Only process-local mount numbers and fixed geometry enter this protocol. Every
load needs its own immediately preceding placement observation; a prior good
placement cannot certify a later detached load. This is structural evidence,
never a claim about on-screen visibility or readable pixels.
"""
import json
from pathlib import Path
import re
import sys

PLACEMENT = re.compile(r"U (\d+\.\d+) mount=([1-9]\d*) attached=([01]) width=(\d+\.\d+) height=(\d+\.\d+)")
LOAD = re.compile(r"V (\d+\.\d+) mount=([1-9]\d*) stage=load-request")


def check(source):
    pending = None
    loads = []
    previous = 0.0
    for line in source.splitlines():
        if line.startswith("U "):
            found = PLACEMENT.fullmatch(line)
            if not found or pending is not None:
                raise ValueError("missing load or noncanonical placement observation")
            when, mount, attached, width, height = found.groups()
            when, width, height = float(when), float(width), float(height)
            if when < previous:
                raise ValueError("placement clock moved backwards")
            previous = when
            if attached != "1" or width <= 0 or height <= 0:
                raise ValueError("document submitted before native attachment and sizing")
            pending = {"mount": mount, "when": when, "width": width, "height": height}
        elif line.startswith("V ") and "stage=load-request" in line:
            found = LOAD.fullmatch(line)
            if not found or pending is None:
                raise ValueError("load has no native placement observation")
            when, mount = found.groups()
            when = float(when)
            if pending["mount"] != mount or when < pending["when"]:
                raise ValueError("placement belongs to another mount or later time")
            loads.append(pending)
            previous = when
            pending = None
        elif line.startswith("V ") and pending is not None:
            raise ValueError("placement is not followed by its document submission")
    if pending is not None or not loads:
        raise ValueError("no complete document placement/load pair")
    return {"loads": loads, "count": len(loads)}


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: check-html-placement.py <native-trace>")
    try:
        print(json.dumps(check(Path(sys.argv[1]).read_text()), indent=2))
    except (OSError, ValueError) as error:
        raise SystemExit(str(error))
