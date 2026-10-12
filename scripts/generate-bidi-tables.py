#!/usr/bin/env python3
"""Generate Unicode 17 line-reset classes. Tool dependency: blake3==1.0.8.

Download scripts/unicode-bidi-sources.json inputs to one directory, then pass
that directory here (optionally with --check). Native DirectWrite still owns
paragraph resolution; this table supplies original classes for UAX #9 L1.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile

import blake3

ROOT = Path(__file__).resolve().parents[1]
NAMES = {
    "B": "Separator", "S": "Separator", "WS": "Reset",
    "FSI": "Reset", "LRI": "Reset", "RLI": "Reset", "PDI": "Reset",
    "BN": "Retained", "LRE": "Retained", "RLE": "Retained",
    "LRO": "Retained", "RLO": "Retained", "PDF": "Retained",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ucd", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    manifest = json.loads((ROOT / "scripts/unicode-bidi-sources.json").read_text())
    entry, = manifest["inputs"]
    data = (args.ucd / entry["file"]).read_bytes()
    if blake3.blake3(data).hexdigest() != entry["blake3"]:
        raise ValueError("bidi source digest mismatch")
    spec = importlib.util.spec_from_file_location("grapheme", ROOT / "scripts/generate-grapheme-tables.py")
    generator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(generator)
    entries = generator.ranges(data.decode("utf-8"), lambda fields: NAMES.get(fields[0]))
    lines = [
        "// Unicode 17.0.0 original bidi classes for UAX #9 L1 and section 5.2.",
        "// Regenerate: scripts/generate-bidi-tables.py; digests: scripts/unicode-bidi-sources.json.",
        "// Copyright Unicode, Inc.; license: crates/bunny_ui/tests/data/unicode-17.0.0/LICENSE.txt.",
        "use super::Class;", "", "pub(super) const CLASSES: &[(u32, u32, Class)] = &[",
    ]
    lines.extend(f"    (0x{start:X}, 0x{end:X}, Class::{value})," for start, end, value in entries)
    lines.extend(["];", ""])
    with tempfile.TemporaryDirectory() as temp:
        rendered = Path(temp) / "text_bidi_data.rs"
        rendered.write_text("\n".join(lines))
        subprocess.run(["rustfmt", "--edition", "2024", str(rendered)], check=True)
        content = rendered.read_bytes()
    target = ROOT / "crates/bunny_ui_windows/src/text_bidi_data.rs"
    if args.check:
        if target.read_bytes() != content:
            raise SystemExit("bidi tables differ; regenerate with the pinned input")
    else:
        target.write_bytes(content)


if __name__ == "__main__":
    main()
