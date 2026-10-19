from pathlib import Path
import difflib,json,subprocess
root=Path(__file__).resolve().parent
base='46080c1145390e227eb4d6601c9f311ebfec24ff'
files=subprocess.check_output(['git','diff',base+'..HEAD','--name-only','--','*.rs'],text=True).splitlines()
report=[]
def changes(text):
 p=subprocess.run(['rustfmt','--edition','2024','--emit','stdout','--config','skip_children=true'],input=text,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True)
 a=text.splitlines(keepends=True);b=p.stdout.splitlines(keepends=True)
 return [(tuple(a[x:y]),tuple(b[u:v])) for op,x,y,u,v in difflib.SequenceMatcher(None,a,b,autojunk=False).get_opcodes() if op!='equal']
for name in files:
 old=subprocess.run(['git','show',base+':'+name],text=True,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
 baseline=changes(old.stdout) if old.returncode==0 else []
 current=changes(Path(name).read_text())
 unexpected=[x for x in current if x not in baseline]
 report.append(dict(file=name,baseline_formatting_blocks=len(baseline),current_formatting_blocks=len(current),new_unformatted_blocks=len(unexpected)))
(root/'format.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
if any(x['new_unformatted_blocks'] for x in report): raise SystemExit(1)
