"""Record the runner's native font inputs without copying font binaries.

Uses the existing Unicode tooling's pinned build-time blake3 package. Only
machine font directories are inspected; user font libraries are not included.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess

import blake3


def font_roots():
    system = platform.system()
    if system == "Windows":
        return [Path(os.environ["WINDIR"]) / "Fonts"]
    if system == "Darwin":
        return [Path("/System/Library/Fonts"), Path("/Library/Fonts")]
    if system == "Linux":
        return [Path("/usr/share/fonts")]
    raise RuntimeError(f"unsupported native font environment: {system}")


def command_version(command):
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    fonts = []
    for root in font_roots():
        for path in sorted(root.rglob("*")):
            if not path.is_file() or path.suffix.lower() not in {".ttf", ".otf", ".ttc", ".otc"}:
                continue
            digest = blake3.blake3()
            with path.open("rb") as stream:
                while block := stream.read(1024 * 1024):
                    digest.update(block)
            fonts.append({"path": path.as_posix(), "bytes": path.stat().st_size,
                          "blake3": digest.hexdigest()})
    if not fonts:
        raise RuntimeError("the native runner supplied no font inputs")
    result = {
        "platform": platform.platform(), "architecture": platform.machine(),
        "python": platform.python_version(), "blake3_tool": blake3.__version__,
        "rustc": command_version(["rustc", "--version", "--verbose"]),
        "cargo": command_version(["cargo", "--version"]),
        "source": command_version(["git", "rev-parse", "HEAD"]),
        "runner_image": {key: os.environ.get(key) for key in
                         ["ImageOS", "ImageVersion", "RUNNER_OS", "RUNNER_ARCH"]},
        "scope": "machine font inventory, not proof that every font was selected by the shaper",
        "fonts": fonts,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"Recorded {len(fonts)} font inputs for {result['source']}")


if __name__ == "__main__":
    main()
