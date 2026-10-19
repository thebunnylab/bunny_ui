#!/usr/bin/env python3
"""Generate pinned Unicode grapheme properties. Tool dependency: blake3==1.0.8.

Download the three inputs named in scripts/unicode-grapheme-sources.json into
one directory; run this script with that directory and optionally --check.
No network or Unicode library is used by the framework or its tests.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

import blake3

ROOT = Path(__file__).resolve().parents[1]
NAMES = {
    "CR": "Cr", "LF": "Lf", "Control": "Control", "Extend": "Extend",
    "ZWJ": "Zwj", "Regional_Indicator": "RegionalIndicator", "Prepend": "Prepend",
    "SpacingMark": "SpacingMark", "L": "L", "V": "V", "T": "T",
}


def ranges(source, select):
    result = []
    for line in source.splitlines():
        fields = [x.strip() for x in line.split("#")[0].split(";")]
        if len(fields) < 2:
            continue
        value = select(fields[1:])
        if value is None:
            continue
        bounds = fields[0].split("..")
        start, end = int(bounds[0], 16), int(bounds[-1], 16)
        result.append((start, end, value))
    merged = []
    for start, end, value in sorted(result):
        if merged and start <= merged[-1][1]:
            raise ValueError("overlapping Unicode properties")
        if merged and start == merged[-1][1] + 1 and value == merged[-1][2]:
            merged[-1] = (merged[-1][0], end, value)
        else:
            merged.append((start, end, value))
    return merged


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ucd", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    manifest = json.loads((ROOT / "scripts/unicode-grapheme-sources.json").read_text())
    for entry in manifest["fixtures"]:
        if blake3.blake3((ROOT / entry["file"]).read_bytes()).hexdigest() != entry["blake3"]:
            raise ValueError(f"fixture digest mismatch: {entry['file']}")
    inputs = {}
    for entry in manifest["inputs"]:
        data = (args.ucd / entry["file"]).read_bytes()
        if blake3.blake3(data).hexdigest() != entry["blake3"]:
            raise ValueError(f"input digest mismatch: {entry['file']}")
        inputs[entry["file"]] = data.decode("utf-8")
    tables = [
        ("GCB", "Gcb", ranges(inputs["GraphemeBreakProperty.txt"], lambda p: NAMES.get(p[0]))),
        ("INCB", "Incb", ranges(inputs["DerivedCoreProperties.txt"],
             lambda p: p[1] if p[0] == "InCB" and p[1] != "None" else None)),
        ("EXTENDED_PICTOGRAPHIC", None, ranges(inputs["emoji-data.txt"],
             lambda p: True if p[0] == "Extended_Pictographic" else None)),
    ]
    lines = [
        "// Unicode 17.0.0 property data. Regenerate with scripts/generate-grapheme-tables.py.",
        "// Copyright Unicode, Inc.; Unicode License V3 in ../tests/data/unicode-17.0.0/LICENSE.txt.",
        "// Inputs and BLAKE3 digests: scripts/unicode-grapheme-sources.json.",
        "use super::{Gcb, Incb};", "",
    ]
    for name, kind, entries in tables:
        element = f"(u32, u32, {kind})" if kind else "(u32, u32)"
        lines.append(f"pub(super) const {name}: &[{element}] = &[")
        for start, end, value in entries:
            tail = f", {kind}::{value}" if kind else ""
            lines.append(f"    (0x{start:X}, 0x{end:X}{tail}),")
        lines.extend(["];", ""])
    with tempfile.TemporaryDirectory() as temp:
        rendered = Path(temp) / "grapheme_data.rs"
        rendered.write_text("\n".join(lines))
        subprocess.run(["rustfmt", "--edition", "2024", str(rendered)], check=True)
        content = rendered.read_bytes()
    target = ROOT / "crates/bunny_ui/src/grapheme_data.rs"
    if args.check:
        if target.read_bytes() != content:
            raise SystemExit("grapheme tables differ; regenerate with the pinned inputs")
    else:
        target.write_bytes(content)


if __name__ == "__main__":
    main()
