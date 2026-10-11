"""Compare native feed receipts with independent frame-ID pixels. macOS only.

Exit 77 means pixels may exist but native visible-presentation qualification was
not established. A locked/unknown session is never promoted to an on-screen pass.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--cpu', action='store_true')
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
output = args.output.resolve()
if output.exists() and any(output.iterdir()):
    parser.error('Output must be empty')
output.mkdir(parents=True, exist_ok=True)
subprocess.run(['cargo', 'build', '--locked', '-p', 'bunny-ui-macos', '--example', 'feed_receipts'], cwd=root, check=True)
helper = output / 'capture'
subprocess.run(['swiftc', str(Path(__file__).with_suffix('.swift')), '-o', str(helper)], check=True)
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], cwd=root))
binary = Path(metadata['target_directory']) / 'debug/examples/feed_receipts'
environment = dict(os.environ)
if args.cpu:
    environment['BUNNY_PRESENT'] = 'cpu'
checks = []
with (output / 'process.log').open('w') as log:
    process = subprocess.Popen([str(binary), str(output)], stdout=log, stderr=subprocess.STDOUT,
                               env=environment, start_new_session=True)
try:
    for stage, marker in [('frame-2', 85), ('repeat-2', 85), ('frame-4', 170), ('frame-5', 240)]:
        deadline = time.monotonic() + 20
        while not (output / 'stage').exists() or (output / 'stage').read_text() != stage:
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f'Fixture failed before {stage}')
            time.sleep(.025)
        captured = subprocess.run([str(helper), str(process.pid), str(output), stage], capture_output=True, text=True)
        (output / f'{stage}.json').write_text(json.dumps(dict(command=captured.args, exit_code=captured.returncode,
                                                            stdout=captured.stdout, stderr=captured.stderr), indent=2))
        windows = json.loads(captured.stdout) if captured.returncode == 0 else []
        checks.append(dict(stage=stage, marker=marker, windows=windows,
                           pixels_match=len(windows) == 1 and windows[0]['marker'] == marker,
                           visible=len(windows) == 1 and windows[0]['on_screen'] and windows[0]['session_unlocked']))
        (output / 'continue').write_text(stage)
    code = process.wait(timeout=10)
finally:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)
    cleanup = subprocess.run(['ps', '-p', str(process.pid), '-o', 'pid=,command='], capture_output=True, text=True)
    (output / 'cleanup.json').write_text(json.dumps(dict(pid=process.pid, returncode=process.returncode,
                                                       ps_exit=cleanup.returncode, remaining=cleanup.stdout), indent=2))
log = (output / 'process.log').read_text()
receipts = [int(value) for value in re.findall(r'Presented \{ generation: (\d+),', log)]
closed = 'CLOSED PresentationSnapshot' in log and 'closed: true' in log
pixels = all(check['pixels_match'] for check in checks)
visible = all(check['visible'] for check in checks)
no_false_credit = 1 not in receipts and 3 not in receipts and len(receipts) == len(set(receipts))
qualified = not args.cpu and receipts == [2, 4, 5] and visible and pixels and closed and code == 0
cpu_unavailable = args.cpu and not receipts and 'Unavailable(Cpu)' in log and pixels and closed and code == 0
result = dict(checks=checks, receipts=receipts, closed=closed, no_false_credit=no_false_credit,
              cpu_unavailable=cpu_unavailable, qualified=qualified, exit_code=code)
(output / 'verdict.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result, indent=2))
if not pixels or not no_false_credit or not closed or code != 0:
    raise SystemExit(1)
raise SystemExit(0 if qualified or cpu_unavailable else 77)
