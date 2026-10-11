"""Verify native pixels, including after secondary-window teardown. macOS only.

Pass --output to retain raw captures, logs and verdicts at a chosen location.
A missing screen-capture grant is a failure to qualify, never a passing skip.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
output = (args.output or Path(tempfile.mkdtemp(prefix='window-presenters-'))).resolve()
if output.exists() and any(output.iterdir()):
    parser.error('The output directory must be empty')
output.mkdir(parents=True, exist_ok=True)
subprocess.run(['cargo','build','--locked','-p','bunny-ui-macos','--example','window_presenters'], cwd=root, check=True)
helper = output/'capture-window'
subprocess.run(['swiftc',str(Path(__file__).with_suffix('.swift')),'-o',str(helper)], check=True)
expected = {'initial': {'Presenter primary':[255,0,0]}, 'dual': {'Presenter primary':[0,255,0], 'Presenter secondary':[255,255,0]}, 'survivor': {'Presenter primary':[0,255,255]}, 'repeated': {'Presenter primary':[255,255,255]}, 'reverse-survivor': {'Presenter secondary':[0,0,255]}}
# Solid primary colors stay distinguishable across display color profiles.
# A low channel stays <=64 and a high channel >=191; gray and wrong hues fail.
checks = []
metadata = json.loads(subprocess.check_output(['cargo','metadata','--locked','--no-deps','--format-version','1'],cwd=root,text=True))
binary = Path(metadata['target_directory'])/'debug/examples/window_presenters'
with (output/'process.log').open('w') as log:
    process = subprocess.Popen([str(binary),str(output)], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
try:
    for stage, colors in expected.items():
        deadline = time.monotonic()+15
        while not (output/'stage').exists() or (output/'stage').read_text() != stage:
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f'Probe failed before {stage}; see {output}/process.log')
            time.sleep(.025)
        captured = subprocess.check_output([str(helper),str(process.pid),str(output),stage],text=True)
        (output/f'{stage}.json').write_text(captured)
        windows = json.loads(captured)
        actual = {item['title']:item['rgb'] for item in windows}
        okay = len(windows)==len(colors) and set(actual)==set(colors) and all(all(abs(a-b)<=64 for a,b in zip(actual[name],color)) for name,color in colors.items())
        checks.append(dict(stage=stage,expected=colors,actual=actual,passed=okay))
        print(stage, 'PASS' if okay else 'FAIL', actual, flush=True)
        (output/'continue').write_text(stage)
    code = process.wait(timeout=10)
    checks.append(dict(stage='close-last',exit_code=code,passed=code==0))
finally:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill();process.wait(timeout=10)
    (output/'verdict.json').write_text(json.dumps(checks,indent=2)+'\n')
    print('Receipts:', output)
if not checks or not all(check['passed'] for check in checks):
    raise SystemExit(1)
