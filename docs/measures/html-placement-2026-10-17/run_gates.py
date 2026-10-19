from pathlib import Path
import subprocess,json,os,datetime,hashlib
root=Path.cwd(); out=root/'target/verification/html-placement-20261017'; base=root.parent/'mail-placement-baseline'
source=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
env=dict(os.environ,CARGO_INCREMENTAL='0',CARGO_BUILD_JOBS='1')
steps=[]
def run(name,cmd,cwd=root,extra=None):
 e=dict(env,**(extra or {}));start=datetime.datetime.now(datetime.timezone.utc).isoformat()
 print(name+' started',flush=True)
 with (out/(name+'.log')).open('w') as f:r=subprocess.run(cmd,cwd=cwd,env=e,stdout=f,stderr=subprocess.STDOUT)
 steps.append(dict(name=name,source=source,revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=cwd,text=True).strip(),cwd=str(cwd),command=cmd,started=start,ended=datetime.datetime.now(datetime.timezone.utc).isoformat(),exit_code=r.returncode))
 (out/'gates.json').write_text(json.dumps(steps,indent=2)+'\n');print(name+' exit='+str(r.returncode),flush=True)
 return r.returncode
for package in ['bunny-ui-apple','bunny-ui-macos']:
 run('baseline-clippy-'+package,['cargo','clippy','-p',package,'--all-targets','--locked','--no-deps','--keep-going','--jobs','1','--message-format=json','--','-D','warnings'],base)
run('build-final',['cargo','build','-p','bunny-ui-macos','--example','html_reuse','--locked','--jobs','1'])
binary=root/'target/debug/examples/html_reuse'
(out/'binary.json').write_text(json.dumps(dict(source=source,sha256=hashlib.sha256(binary.read_bytes()).hexdigest()),indent=2)+'\n')
run('reuse-final',[str(binary)],extra={'BUNNY_PRESENT_TRACE':str(out/'final.trace')})
run('placement-final',['python3','scripts/check-html-placement.py',str(out/'final.trace')])
run('lifecycle-final',['python3','scripts/check-html-lifecycle.py',str(out/'final.trace')])
run('placement-checker-tests',['python3','scripts/test_html_placement.py'])
run('lifecycle-checker-tests',['python3','scripts/test_html_lifecycle.py'])
for package in ['bunny-ui-apple','bunny-ui-macos']:
 run('tests-'+package,['cargo','test','-p',package,'--lib','--locked','--jobs','1','--','--test-threads=2'])
run('tests-trace',['cargo','test','-p','bunny-ui-macos','--example','trace_report','--locked','--jobs','1','--','--test-threads=2'])
run('native-host-scroll',['cargo','test','-p','bunny-ui-macos','--test','host_scroll_native','--locked','--jobs','1'])
run('native-window-lifecycle',['cargo','test','-p','bunny-ui-macos','--test','window_lifecycle','--locked','--jobs','1'])
run('check-ios',['cargo','check','-p','bunny-ui-ios','--target','aarch64-apple-ios-sim','--locked','--jobs','1'])
for package in ['bunny-ui-apple','bunny-ui-macos']:
 run('candidate-clippy-'+package,['cargo','clippy','-p',package,'--all-targets','--locked','--no-deps','--keep-going','--jobs','1','--message-format=json','--','-D','warnings'])
run('whitespace',['git','diff','b7ca959..HEAD','--check'])
