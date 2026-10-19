"""Check an own-window HTML protocol's optional native lifecycle tape.

Only fixed milestones and process-local mount numbers may enter V records.
Interrupted documents may lack a finish; at least one completed navigation and
one accepted application post are required to witness the instrumented path.
This never interprets a callback as presented body pixels.
"""
import json
from pathlib import Path
import re
import sys

LINE = re.compile(r"V (\d+\.\d+) mount=([1-9]\d*) stage=(load-request|load-return|commit|finish|post)")


def check(source):
    mounts = {}
    counts = {}
    previous = 0.0
    for line in source.splitlines():
        if not line.startswith("V "):
            continue
        found = LINE.fullmatch(line)
        if not found:
            raise ValueError("noncanonical or potentially private document trace record")
        when, mount, stage = found.groups()
        when = float(when)
        if when < previous:
            raise ValueError("document trace clock moved backwards")
        previous = when
        counts[stage] = counts.get(stage, 0) + 1
        current = mounts.get(mount)
        if stage == "load-request":
            mounts[mount] = {"load-request": when}
            continue
        if current is None:
            raise ValueError("document milestone has no load request")
        if stage == "load-return" and "load-return" in current:
            raise ValueError("duplicate load return")
        if stage in ("commit", "post", "finish") and "load-return" not in current:
            raise ValueError("document callback precedes load return")
        if stage == "finish" and "commit" not in current:
            raise ValueError("navigation finish precedes commit")
        current.setdefault(stage, when)
    if not counts.get("finish") or not counts.get("post"):
        raise ValueError("trace lacks completed document navigation or accepted application post")
    return {"counts": counts, "mounts": mounts}


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: check-html-lifecycle.py <native-trace>")
    try:
        print(json.dumps(check(Path(sys.argv[1]).read_text()), indent=2))
    except (OSError, ValueError) as error:
        raise SystemExit(str(error))
