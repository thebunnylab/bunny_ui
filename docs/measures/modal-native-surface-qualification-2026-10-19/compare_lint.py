import importlib.util,json,os,subprocess
from pathlib import Path
root=Path.cwd();raw=root/'target/verification/T2-STUDIO-177';test=Path('crates/bunny_ui_macos/tests/accessibility_native.rs');saved=test.read_bytes();baseline=subprocess.check_output(['git','show','ad68287a:'+str(test)])
spec=importlib.util.spec_from_file_location('lint',root/'scripts/check-clippy-regressions.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
env=dict(os.environ,CARGO_TARGET_DIR=str(root/'target/studio-execution'),CARGO_BUILD_BUILD_DIR=str(root/'target/studio-execution'))
report={'base_revision':'ad68287a32f0cc9e50036bcdf290fbacc387e996','method':'Same isolated checkout, sequential exact baseline/candidate Rust snapshots; own artifact directory. Production Rust unchanged. CI diagnostic comparator with all-target/all-feature keep-going strict invocations.','strict_runs':{}}
keys={}
try:
 for name,content in [('baseline',baseline),('candidate',saved)]:
  test.write_bytes(content)
  paths=subprocess.check_output(['git','diff','--name-only','--','*.rs'],text=True).splitlines();assert paths==([] if name=='baseline' else [str(test)]),paths
  log=raw/f'{name}-bunny-ui-macos.jsonl'
  with log.open('w') as f:r=subprocess.run(['cargo','clippy','-p','bunny-ui-macos','--locked','--all-targets','--all-features','--keep-going','--no-deps','--message-format=json','--','-D','warnings'],env=env,stdout=f,stderr=subprocess.STDOUT)
  keys[name]=m.diagnostic_keys(log,root,'crates/bunny_ui_macos');report['strict_runs'][name]={'exit_code':r.returncode,'diagnostics':[json.loads(k) for k in sorted(keys[name])]}
  assert r.returncode==0 or (r.returncode==101 and keys[name])
finally:test.write_bytes(saved)
report['shared']=len(keys['baseline']&keys['candidate']);report['new']=[json.loads(k) for k in sorted(keys['candidate']-keys['baseline'])];report['removed']=[json.loads(k) for k in sorted(keys['baseline']-keys['candidate'])]
(raw/'comparison.json').write_text(json.dumps(report,indent=2)+'\n');print({k:report[k] for k in ['shared','new','removed']});raise SystemExit(bool(report['new']))
