#!/usr/bin/env python3
"""Actual disposable workshop stop/swap/restart/rollback; no Persona or model activity."""
import argparse,contextlib,hashlib,importlib.util,json,os,shutil,subprocess,tempfile,time,traceback,uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
for name,path in [('launcher',ROOT/'tests/native_launch.py'),('worker',ROOT/'scripts/native_update.py')]:
 spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);globals()[name]=module

@contextlib.contextmanager
def database(path):
 import sqlite3
 db=sqlite3.connect(path)
 try:
  with db: yield db
 finally: db.close()


class ActualWorker:
 """Hold the OS process identity of this plan's actual packaged interpreter."""
 def __init__(self,folder,identifier):
  self.folder=folder.resolve();self.identifier=identifier;self.handle=None;self.pid=None;self.settled=False
  self.executable=self.folder/'worker-runtime/python'/('python.exe' if os.name=='nt' else 'bin/python3')
 def capture(self):
  if os.name!='nt':return
  import ctypes as c
  from ctypes import wintypes as w
  script=self.folder/'fixture-query.ps1';output=self.folder/'fixture-worker.json'
  script.write_text("""$ErrorActionPreference='Stop'
$p=@(Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -eq $env:PERSONA_FIXTURE_EXE -and $_.CommandLine.Contains($env:PERSONA_FIXTURE_PLAN)})
if($p.Count -ne 1){throw 'Expected one exact plan interpreter'}
[IO.File]::WriteAllText($env:PERSONA_FIXTURE_OUTPUT,(@{pid=$p[0].ProcessId}|ConvertTo-Json -Compress),[Text.UTF8Encoding]::new($false))
""",encoding='utf-8')
  env=dict(os.environ,PERSONA_FIXTURE_EXE=str(self.executable),PERSONA_FIXTURE_PLAN=self.identifier,PERSONA_FIXTURE_OUTPUT=str(output))
  powershell=Path(os.environ['SystemRoot'])/'System32/WindowsPowerShell/v1.0/powershell.exe'
  subprocess.run([str(powershell),'-NoProfile','-NonInteractive','-File',str(script)],env=env,check=True,capture_output=True,text=True,timeout=20)
  self.pid=json.loads(output.read_text(encoding='utf-8'))['pid']
  self.kernel=c.WinDLL('kernel32',use_last_error=True)
  self.kernel.OpenProcess.argtypes=[w.DWORD,w.BOOL,w.DWORD];self.kernel.OpenProcess.restype=w.HANDLE
  self.kernel.QueryFullProcessImageNameW.argtypes=[w.HANDLE,w.DWORD,w.LPWSTR,c.POINTER(w.DWORD)]
  self.kernel.WaitForSingleObject.argtypes=[w.HANDLE,w.DWORD];self.kernel.CloseHandle.argtypes=[w.HANDLE]
  self.handle=self.kernel.OpenProcess(0x100000|0x1000,False,self.pid)
  assert self.handle,'Exact worker process unavailable'
  buf=c.create_unicode_buffer(32768);size=w.DWORD(32768)
  assert self.kernel.QueryFullProcessImageNameW(self.handle,0,buf,c.byref(size))
  assert os.path.normcase(buf.value.removeprefix('\\\\?\\'))==os.path.normcase(str(self.executable))
 def wait(self,deadline):
  if self.settled:return
  if self.handle:
   assert self.kernel.WaitForSingleObject(self.handle,max(0,int((deadline-time.monotonic())*1000)))==0,'Known update worker did not exit'
   self.kernel.CloseHandle(self.handle);self.handle=None;self.settled=True
  else:
   # The ordinary API reports this same worker lock. Unix replacement allows
   # concurrent reads; acquire it only to establish that its operation returned.
   while time.monotonic()<deadline:
    try:
     with worker.locked(self.folder.parent/'worker.lock',False):self.settled=True;return
    except (BlockingIOError,PermissionError):time.sleep(.1)
   raise AssertionError('Update worker still owns its operation')

def run(package,result):
 checks=[];observations=[];actual_worker=None;worker_deadline=None
 with tempfile.TemporaryDirectory(prefix='Persona update café 子 ') as temp:
  root=Path(temp);bundle=root/'download';bundle.mkdir();launcher.extract(package,bundle)
  binary=next(p for p in bundle.rglob('persona-workshop.exe' if os.name=='nt' else 'persona-workshop') if p.is_file());home=root/'installation';log=open(root/'server.log','ab')
  proc=subprocess.Popen([str(binary),'--home',str(home),'--no-browser'],stdin=subprocess.DEVNULL,stdout=log,stderr=log)
  current=None
  try:
   for _ in range(400):
    try:
     base=json.loads((home/'runtime.json').read_text(encoding='utf-8'))['url']
     if launcher.request(base,'/auth/session')[0]==200:break
    except (OSError,ValueError):pass
    if proc.poll() is not None:raise AssertionError('Fixture workshop exited')
    time.sleep(.1)
   else:raise AssertionError('Fixture workshop did not start')
   config_bytes=(home/'config.json').read_bytes();config=json.loads(config_bytes);cap=worker.read(home/'secrets/first-account.json')['token']
   assert launcher.request(base,'/auth/first-account',{'setup_token':cap,'password':'Disposable update password','start_at_signin':False})[0]==200
   secret_before=worker.digest(home/'secrets/local-password.json');identity_before=config['id'];private_before=worker.digest(home/'secrets/control-token')
   # A new queued source may arrive during installation. It must never be rewound.
   with database(home/'state.sqlite3') as db:db.execute("INSERT INTO audit(at,kind,project_id,data) VALUES(1,'native-fixture',NULL,'{\"value\":1}')")
   expected_value=[1]
   def input_arrived_after_backup():
    expected_value[0]+=1
    with database(home/'state.sqlite3') as db:db.execute("UPDATE audit SET data=? WHERE kind='native-fixture'",(json.dumps({'value':expected_value[0]},separators=(',',':')),))
   original_binary=home/'bin'/binary.name
   for fail in [None,"health","start"]:
    identifier=str(uuid.uuid4());folder=home/'native-updates'/identifier;folder.mkdir(parents=True)
    shutil.copytree(bundle,folder/'candidate');entry=binary.relative_to(bundle).as_posix()
    job={'schema':1,'id':identifier,'state':'ready','action':'apply','entry':entry,'binary_sha256':worker.digest(binary),'installed_sha256':worker.digest(original_binary),'config_sha256':worker.digest(home/'config.json'),'version':subprocess.check_output([str(binary),'--version'],text=True).strip().split()[-1],'release_url':'https://example.invalid/disposable-fixture'}
    worker.write(folder/'plan.json',job);worker.write(home/'native-updates/active.json',{'id':identifier})
    current=worker.Worker(home,identifier)
    real_health=current.health;health_calls=[]
    if fail=="health":
     def fail_first(expected):
      health_calls.append(expected)
      if len(health_calls)==1:
       input_arrived_after_backup();raise RuntimeError('injected failed health after actual swap')
      return real_health(expected)
     current.health=fail_first
    if fail=="start":
     real_start=current.start;start_calls=[]
     def fail_start():
      start_calls.append(True)
      if len(start_calls)==1:
       input_arrived_after_backup();raise OSError('injected candidate start failure')
      return real_start()
     current.start=fail_start
    real_rollback=current.rollback;rollback_errors=[]
    def traced_rollback(reason):
     try:return real_rollback(reason)
     except Exception:
      rollback_errors.append(traceback.format_exc());raise
    current.rollback=traced_rollback
    if fail is None:
     # Use the packaged API dispatcher and its private interpreter for the real
     # no-toolchain worker path. Later cases inject faults at exact worker seams.
     request_id=str(uuid.uuid4())
     try:current.api('desktop/updates',{'action':'apply','request_id':request_id,'plan_id':identifier})
     except (TimeoutError,ConnectionError):pass  # Reconcile the same durable request; never redispatch.
     worker_deadline=time.monotonic()+240
     actual_worker=ActualWorker(folder,identifier)
     while time.monotonic()<worker_deadline:
      try:
       observed=current.api('desktop/updates');out=observed.get('current')
       if out is not None:
        assert out['id']==identifier
        sample={'state':out['state'],'worker_active':out.get('worker_active')}
        if not observations or observations[-1]!=sample:observations.append(sample)
        if os.name=='nt' and out.get('worker_active') is True and actual_worker.pid is None:actual_worker.capture()
        if out['state'] in {'succeeded','rolled_back','failed','deferred','recovery_required'} and out.get('worker_active') is False:break
      except (OSError,ConnectionError):pass  # Saved server is stopped/restarted by this exact update.
      time.sleep(.2)
     else:raise AssertionError('The installed native worker did not settle')
     if os.name=='nt':assert actual_worker.pid is not None,'Did not bind the actual private update worker'
     actual_worker.wait(worker_deadline)
     print('Observed real update API phases: '+json.dumps(observations))
     if result:result.with_suffix('.phases.json').write_text(json.dumps(observations,indent=2)+'\n')
     assert worker.read(home/'native-updates'/('request-'+request_id+'.json'))['id']==identifier
     assert (folder/'worker.py').read_bytes()==(ROOT/'scripts/native_update.py').read_bytes(),'Package worker differs from the fault-injection fixture'
    else:
     current.execute();out=worker.read(folder/'plan.json')
    assert out['state']==('rolled_back' if fail else 'succeeded'),{'plan':out,'rollback_errors':rollback_errors}
    checks.append('actual failed start restores prior binary/runtime' if fail=='start' else 'actual failed verification restores prior binary/runtime' if fail else 'actual API dispatch and packaged private worker perform idle stop, swap and verified restart')
    assert (home/'config.json').read_bytes()==config_bytes
    assert worker.digest(home/'secrets/local-password.json')==secret_before and worker.digest(home/'secrets/control-token')==private_before
    assert current.api('status')['id']==identity_before
    with database(home/'state.sqlite3') as db:assert db.execute("SELECT data FROM audit WHERE kind='native-fixture'").fetchone()==(json.dumps({'value':expected_value[0]},separators=(',',':')),)
    assert not (home/'updates/drain.json').exists()
   checks.append('identity/config/credentials/DB evidence unchanged; no database rewind')
   # A stop failure with the actual server still holding its lock cannot replace anything.
   identifier=str(uuid.uuid4());folder=home/'native-updates'/identifier;folder.mkdir();shutil.copytree(bundle,folder/'candidate');job.update(id=identifier,state='ready',installed_sha256=worker.digest(original_binary),config_sha256=worker.digest(home/'config.json'));worker.write(folder/'plan.json',job);worker.write(home/'native-updates/active.json',{'id':identifier});current=worker.Worker(home,identifier)
   before=worker.digest(original_binary);current.stop=lambda:(_ for _ in ()).throw(RuntimeError('injected unknown stop'))
   current.execute();assert worker.read(folder/'plan.json')['state']=='recovery_required';assert worker.digest(original_binary)==before;assert current.api('status')['id']==identity_before;checks.append('unknown stop never replaces a running service or rewinds state')
  finally:
   if actual_worker is not None:actual_worker.wait(worker_deadline)
   if current:
    current=worker.Worker(home,current.id);current.save('rolling_back')
    try:current.stop()
    except Exception:pass
   if proc.poll() is None:proc.terminate()
   proc.wait(timeout=20);log.close()
 receipt={'passed':True,'checks':checks,'status_api_observations':observations,'worker_exit':('exact Windows process handle' if os.name=='nt' else 'same worker operation lock released'),'package_sha256':hashlib.sha256(package.read_bytes()).hexdigest(),'limits':['Release download represented by local exact bundle; GitHub digest selection separately tested','No model/provider, project analysis or startup registration']}
 if result:result.write_text(json.dumps(receipt,indent=2)+'\n')
 print(json.dumps(receipt,indent=2))
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--package',type=Path,required=True);p.add_argument('--result',type=Path);a=p.parse_args();run(a.package.resolve(),a.result)
