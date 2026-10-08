"""Run strict clippy on both revisions and refuse any new diagnostic.

Existing failures remain failures in the logs and summary. This differential
gate does not turn either strict invocation into a passing lint gate.
"""
import argparse
import json
from pathlib import Path
import subprocess


def diagnostic_keys(log, source):
    diagnostics = set()
    for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("reason") != "compiler-message":
            continue
        message = event["message"]
        if message["level"] not in ("error", "warning"):
            continue
        spans = [span for span in message.get("spans", []) if span["is_primary"]]
        diagnostic = {
            "code": (message.get("code") or {}).get("code"),
            "level": message["level"],
            "message": message["message"],
        }
        if spans:
            span = spans[0]
            filename = Path(span["file_name"])
            if filename.is_absolute():
                try:
                    filename = filename.relative_to(source)
                except ValueError:
                    # Macro definitions may anchor in the compiler's own
                    # library. Both invocations use that same toolchain.
                    pass
            file = source / filename
            contents = file.read_text(encoding="utf-8").splitlines() if file.is_file() else []
            diagnostic.update(file=filename.as_posix(),
                              source=contents[span["line_start"] - 1].strip() if contents else "")
        diagnostics.add(json.dumps(diagnostic, sort_keys=True))
    return diagnostics


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", default=Path.cwd(), type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--package", action="append", required=True)
    arguments = parser.parse_args()
    output = arguments.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    record = {"packages": arguments.package, "strict_runs": {}}
    keys = {}
    for name, source in (("baseline", arguments.baseline), ("candidate", arguments.candidate)):
        source = source.resolve()
        record["strict_runs"][name] = {}
        keys[name] = set()
        # A failing core invocation can stop Cargo before it reaches a native
        # dependent. Each touched package must therefore run independently.
        for package in arguments.package:
            command = ["cargo", "clippy", "--locked", "--all-targets", "--all-features",
                       "--no-deps", "--message-format=json", "--target-dir", str(output / (name + "-target")),
                       "-p", package, "--", "-D", "warnings"]
            log = output / (name + "-" + package + ".jsonl")
            with log.open("w", encoding="utf-8") as stream:
                result = subprocess.run(command, cwd=source, stdout=stream, stderr=subprocess.STDOUT)
            package_keys = diagnostic_keys(log, source)
            record["strict_runs"][name][package] = {
                "exit_code": result.returncode,
                "diagnostics": [json.loads(key) for key in sorted(package_keys)],
            }
            if result.returncode not in (0, 101) or (result.returncode and not package_keys):
                raise RuntimeError(f"{name}/{package} strict invocation failed without diagnostic coverage; see {log}")
            keys[name].update(json.dumps({"package": package, **json.loads(key)}, sort_keys=True)
                              for key in package_keys)
    record["shared"] = len(keys["baseline"] & keys["candidate"])
    record["new"] = [json.loads(key) for key in sorted(keys["candidate"] - keys["baseline"])]
    record["removed"] = [json.loads(key) for key in sorted(keys["baseline"] - keys["candidate"])]
    (output / "comparison.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"strict_exit_codes": {name: {package: run["exit_code"]
                                                   for package, run in runs.items()}
                                          for name, runs in record["strict_runs"].items()},
                      "shared": record["shared"], "new": len(record["new"]),
                      "removed": len(record["removed"])}, indent=2))
    for diagnostic in record["new"]:
        print(json.dumps(diagnostic))
    return int(bool(record["new"]))


if __name__ == "__main__":
    raise SystemExit(main())
