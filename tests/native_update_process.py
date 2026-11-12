#!/usr/bin/env python3
"""Actual disposable workshop stop/swap/restart/rollback; no Persona or model activity."""
import argparse,contextlib,hashlib,importlib.util,json,os,shutil,stat,subprocess,tempfile,time,traceback,types,uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
for name,path in [('launcher',ROOT/'tests/native_launch.py'),('worker',ROOT/'scripts/native_update.py')]:
 spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);globals()[name]=module

def public_failure_evidence(details):
 """Hash launch binding nonces consistently; keep original private files intact."""
 nonces=set()
 def collect(value):
  if isinstance(value,dict):
   for key,item in value.items():
    if key=='nonce' and isinstance(item,str):nonces.add(item)
    collect(item)
  elif isinstance(value,list):
   for item in value:collect(item)
 collect(details)
 for name in [*details.get('native_receipts',{}),*details.get('claim_files',[])]:
  for prefix in ['manager-stop-','manager-claim-']:
   if name.startswith(prefix) and name.endswith('.json'):nonces.add(name[len(prefix):-5])
 replacements={value:'sha256:'+hashlib.sha256(value.encode()).hexdigest() for value in nonces if value}
 def redact(value):
  if isinstance(value,str):
   for old,new in replacements.items():value=value.replace(old,new)
   return value
  if isinstance(value,list):return [redact(item) for item in value]
  if isinstance(value,dict):return {redact(key):redact(item) for key,item in value.items()}
  return value
 return redact(details)

@contextlib.contextmanager
def observe_apply_failure(evidence):
 """Observe the original response bytes once; never repeat an HTTP operation."""
 original=worker.http
 class Connection(original.client.HTTPConnection):
  def request(self,method,url,*args,**kwargs):
   self.observe_failure=method=='POST' and url=='/api/v1/desktop/updates'
   return super().request(method,url,*args,**kwargs)
  def getresponse(self):
   response=super().getresponse()
   if self.observe_failure and not 200<=response.status<300:
    record={'status':response.status};evidence.append(record)
    read=response.read
    def observed_read(*args,**kwargs):
     data=read(*args,**kwargs)
     # This is only the already requested body. No headers, proof response,
     # credentials, request body or additional reads enter the receipt.
     if len(data)<=4096:
      try:
       value=json.loads(data)
       if isinstance(value,dict) and isinstance(value.get('error'),str):record['error']=value['error'][:1024]
       else:record['body_omitted']='not_the_expected_error_object'
      except (ValueError,UnicodeError):record['body_omitted']='not_json'
     else:record['body_omitted']='exceeds_evidence_bound'
     return data
    response.read=observed_read
   return response
 client=types.SimpleNamespace(**vars(original.client));client.HTTPConnection=Connection
 worker.http=types.SimpleNamespace(client=client)
 try:yield
 finally:worker.http=original

def plan_never_dispatched(home,identifier,request_id):
 """Only a ready plan with no dispatch evidence can skip native stop cleanup."""
 folder=home/'native-updates'/identifier;plan=worker.read(folder/'plan.json')
 return (plan.get('id')==identifier and plan.get('state')=='ready'
  and not plan.get('request_id') and not plan.get('worker_request_id')
  and not (folder/'dispatch').exists() and not (folder/'manager-launch.json').exists()
  and not any(folder.glob('manager-claim-*.json'))
  and (request_id is None or not (home/'native-updates'/('request-'+request_id+'.json')).exists()))

def startup_definition_evidence(home,folder,registration):
 """Observe only this fixture's current dispatch and known startup definition."""
 def bounded(path,limit):
  metadata=path.lstat()
  if not stat.S_ISREG(metadata.st_mode) or path.is_symlink() or metadata.st_size>limit:raise ValueError('definition_evidence_input_not_bounded_regular_file')
  with path.open('rb') as stream:data=stream.read(limit+1)
  if len(data)>limit:raise ValueError('definition_evidence_input_grew')
  return data
 result={'fixture_home':str(home),'resolved_home':str(home.resolve())}
 try:
  plan=json.loads(bounded(folder/'plan.json',65536));request=str(uuid.UUID(plan.get('worker_request_id',plan['request_id'])))
  binding=json.loads(bounded(folder/'dispatch'/request/'manager.json',2*1024*1024))
  if binding['plan_id']!=folder.name or binding['request_id']!=request:raise ValueError('definition_evidence_dispatch_mismatch')
  app=binding.get('application')
  if app is None:return {**result,'application':'not_captured'}
  service=app['service']
  baseline=app['baseline']
  result['baseline_flags']={key:baseline.get(key) for key in ('definition_matches','enabled','masked','start_allowed','loaded') if baseline.get(key) in (None,True,False)}
  if registration is None or service['manager']!='launch_agent':return {**result,'definition_bytes':'Mac startup evidence only'}
  expected_path=registration.file
  if service['name']!=registration.label or Path(service['definition_path'])!=expected_path:raise ValueError('definition_evidence_foreign_startup')
  result['service']={key:service[key] for key in ('manager','name','definition_path')}
  def xml(text):return str(text).replace('&','&amp;').replace('<','&lt;').replace('>','&gt;').replace('"','&quot;').replace("'",'&apos;')
  def known(candidate):
   return ('<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>Label</key><string>'+xml(registration.label)+'</string><key>ProgramArguments</key><array><string>'+xml(candidate/'bin/persona-workshop')+'</string><string>--home</string><string>'+xml(candidate)+'</string><string>serve</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>5</integer></dict></plist>\n').encode()
  expected={'fixture_home':known(home),'resolved_home':known(home.resolve())}
  result['expected_definition_sha256']={key:hashlib.sha256(data).hexdigest() for key,data in expected.items()}
  def describe(data):
   matches=[key for key,value in expected.items() if data==value]
   value={'sha256':hashlib.sha256(data).hexdigest(),'size':len(data),'matches_known_home_shapes':matches}
   # Unknown plist contents may include environment values, arguments or XML
   # comments. Hash them, but emit bytes only for the exact fixed fixture shape.
   if matches:value['bytes']=list(data)
   else:value['bytes_omitted']='not_an_exact_known_fixture_startup_shape'
   return value
  saved=baseline['definition']['bytes']
  if not isinstance(saved,list) or len(saved)>65536 or any(type(v) is not int or not 0<=v<=255 for v in saved):raise ValueError('definition_evidence_baseline_bytes_invalid')
  result['baseline_definition']=describe(bytes(saved))
  result['current_definition']=describe(bounded(expected_path,65536))
  result['definition_unchanged']=result['baseline_definition']['sha256']==result['current_definition']['sha256']
 except (OSError,ValueError,KeyError,TypeError) as error:result['inspection_error_type']=type(error).__name__
 return result

@contextlib.contextmanager
def database(path):
 import sqlite3
 db=sqlite3.connect(path)
 try:
  with db: yield db
 finally: db.close()

@contextlib.contextmanager
def retained_fixture():
 root=Path(tempfile.mkdtemp(prefix='Persona update café 子 '))
 try:yield str(root)
 except BaseException:
  print('Retained failed fixture for exact lifetime/cleanup inspection: '+json.dumps(str(root)),flush=True)
  raise
 else:shutil.rmtree(root)


class ActualWorker:
 """Hold the OS process identity of this plan's actual packaged interpreter."""
 def __init__(self,folder,identifier):
  self.folder=folder.resolve();self.identifier=identifier;self.handle=None;self.pid=None;self.settled=False
  self.executable=self.folder/'worker-runtime/python'/('python.exe' if os.name=='nt' else 'bin/python3')
 def capture(self):
  if os.name!='nt':return
  import ctypes as c
  from ctypes import wintypes as w
  self.kernel=c.WinDLL('kernel32',use_last_error=True)
  self.kernel.OpenProcess.argtypes=[w.DWORD,w.BOOL,w.DWORD];self.kernel.OpenProcess.restype=w.HANDLE
  self.kernel.QueryFullProcessImageNameW.argtypes=[w.HANDLE,w.DWORD,w.LPWSTR,c.POINTER(w.DWORD)]
  self.kernel.WaitForSingleObject.argtypes=[w.HANDLE,w.DWORD];self.kernel.CloseHandle.argtypes=[w.HANDLE]
  # Inspect executable identities directly; console/CIM text may normalize the
  # runner's short Unicode temp path differently from the process image path.
  psapi=c.WinDLL('psapi',use_last_error=True)
  psapi.EnumProcesses.argtypes=[c.POINTER(w.DWORD),w.DWORD,c.POINTER(w.DWORD)]
  ids=(w.DWORD*8192)();used=w.DWORD()
  assert psapi.EnumProcesses(ids,c.sizeof(ids),c.byref(used))
  assert used.value<c.sizeof(ids),'Process enumeration exceeded fixture bound'
  expected=os.path.normcase(str(self.executable.resolve()))
  for pid in ids[:used.value//c.sizeof(w.DWORD)]:
   handle=self.kernel.OpenProcess(0x100000|0x1000,False,pid)
   if not handle:continue
   buf=c.create_unicode_buffer(32768);size=w.DWORD(32768)
   matches=False
   try:
    if self.kernel.QueryFullProcessImageNameW(handle,0,buf,c.byref(size)):
     matches=os.path.normcase(str(Path(buf.value).resolve()))==expected
    if matches:
     assert self.handle is None,'Multiple processes use this exact plan interpreter'
     self.handle=handle;self.pid=pid
   finally:
    if not matches:self.kernel.CloseHandle(handle)
  assert self.handle,'Exact per-plan interpreter was not observed'
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

def run(package,result,managed=False,storage_helper=None):
 checks=[];observations=[];actual_worker=None;worker_deadline=None;registration=None;apply_failures=[];fixture_error=None;request_id=None
 if storage_helper is not None:
  storage_helper=storage_helper.resolve(strict=True)
  if not storage_helper.is_file() or storage_helper.is_symlink():raise ValueError('Native fixture helper must be an ordinary file')
 elif os.name=='nt':raise ValueError('Windows synthetic plan directories require --storage-helper; Python mkdir is not equivalent to native private creation')
 storage_sha256=worker.digest(storage_helper) if storage_helper else None
 def private_fixture_directory(path):
  if storage_helper:
   assert worker.digest(storage_helper)==storage_sha256,'Native fixture storage helper changed'
   subprocess.run([str(storage_helper),str(path.absolute())],check=True,capture_output=True,timeout=15)
  else:path.mkdir(mode=0o700,exist_ok=True)
 with retained_fixture() as temp:
  root=Path(temp);bundle=root/'download';bundle.mkdir();launcher.extract(package,bundle)
  binary=next(p for p in bundle.rglob('persona-workshop.exe' if os.name=='nt' else 'persona-workshop') if p.is_file());home=root/'installation';log=open(root/'server.log','ab')
  proc=subprocess.Popen([str(binary),'--home',str(home),'--no-browser'],stdin=subprocess.DEVNULL,stdout=log,stderr=log)
  current=None
  try:
   # First installation verifies and durably copies the complete private
   # runtime. Match the bounded full-package setup budget; update/stop waits
   # below keep their separate existing bounds.
   for _ in range(1800):
    try:
     base=json.loads((home/'runtime.json').read_text(encoding='utf-8'))['url']
     if launcher.request(base,'/auth/session')[0]==200:break
    except (OSError,ValueError):pass
    if proc.poll() is not None:raise AssertionError('Fixture workshop exited')
    time.sleep(.1)
   else:raise AssertionError('Fixture workshop did not start')
   config_bytes=(home/'config.json').read_bytes();config=json.loads(config_bytes);cap=worker.read(home/'secrets/first-account.json')['token']
   assert launcher.request(base,'/auth/first-account',{'setup_token':cap,'password':'Disposable update password','start_at_signin':managed})[0]==200
   if managed:
    from native_startup import Registration
    registration=Registration(root,home,config['id']);registration.read()
    first_pid=current_pid=launcher.request(base,'/api/v1/status',headers={'Authorization':'Bearer '+(home/'secrets/control-token').read_text().strip()})[2]['pid']
    proc.terminate();proc.wait(timeout=20);registration.start()
    deadline=time.monotonic()+40
    while time.monotonic()<deadline:
     try:
      status=launcher.request(base,'/api/v1/status',headers={'Authorization':'Bearer '+(home/'secrets/control-token').read_text().strip()})[2]
      if status.get('pid')!=first_pid and status.get('id')==config['id']:break
     except (OSError,ValueError):pass
     time.sleep(.1)
    else:raise AssertionError('OS-managed original app did not start')
    registration.bind_process(status['pid']);checks.append('original application launched by its exact current-user OS registration')
   secret_before=worker.digest(home/'secrets/local-password.json');identity_before=config['id'];private_before=worker.digest(home/'secrets/control-token')
   # A new queued source may arrive during installation. It must never be rewound.
   with database(home/'state.sqlite3') as db:db.execute("INSERT INTO audit(at,kind,project_id,data) VALUES(1,'native-fixture',NULL,'{\"value\":1}')")
   expected_value=[1]
   def input_arrived_after_backup():
    expected_value[0]+=1
    with database(home/'state.sqlite3') as db:db.execute("UPDATE audit SET data=? WHERE kind='native-fixture'",(json.dumps({'value':expected_value[0]},separators=(',',':')),))
   original_binary=home/'bin'/binary.name
   updates=home/'native-updates';private_fixture_directory(updates)
   def manager(identifier,operation):
    return json.loads(subprocess.check_output([str(original_binary),'--home',str(home),'native-update-manager',identifier,operation],stderr=subprocess.PIPE,timeout=90))
   def prepare_direct(identifier,job):
    folder=updates/identifier;job.update(request_id=str(uuid.uuid4()),state='queued');worker.write(folder/'plan.json',job)
    shutil.copytree(home/'runtime',folder/'worker-runtime');shutil.copyfile(ROOT/'scripts/native_update.py',folder/'worker.py')
    manager(identifier,'prepare')
   def retire_worker(identifier):
    deadline=time.monotonic()+20
    while time.monotonic()<deadline:
     value=manager(identifier,'retire-worker')
     if value.get('unresolved'):raise AssertionError('Native worker retirement remained unknown: '+json.dumps(value))
     if not value.get('waiting_for_exit'):return value
     time.sleep(.2)
    raise AssertionError('Native worker did not finish exiting')
   for fail in [None,"health","start"]:
    identifier=str(uuid.uuid4());folder=home/'native-updates'/identifier;private_fixture_directory(folder)
    shutil.copytree(bundle,folder/'candidate');entry=binary.relative_to(bundle).as_posix()
    job={'schema':1,'id':identifier,'state':'ready','action':'apply','entry':entry,'binary_sha256':worker.digest(binary),'installed_sha256':worker.digest(original_binary),'config_sha256':worker.digest(home/'config.json'),'version':subprocess.check_output([str(binary),'--version'],text=True).strip().split()[-1],'release_url':'https://example.invalid/disposable-fixture'}
    worker.write(folder/'plan.json',job);worker.write(home/'native-updates/active.json',{'id':identifier})
    if fail is not None:prepare_direct(identifier,job)
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
     try:
      with observe_apply_failure(apply_failures):current.api('desktop/updates',{'action':'apply','request_id':request_id,'plan_id':identifier})
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
    retire_worker(identifier)
    if registration is not None:registration.read()
    # The application must remain available after the separate worker retires.
    assert current.api('status')['id']==identity_before
   checks.append('identity/config/credentials/DB evidence unchanged; no database rewind')
   # A stop failure with the actual server still holding its lock cannot replace anything.
   identifier=str(uuid.uuid4());folder=home/'native-updates'/identifier;private_fixture_directory(folder);shutil.copytree(bundle,folder/'candidate');job.update(id=identifier,state='ready',installed_sha256=worker.digest(original_binary),config_sha256=worker.digest(home/'config.json'));worker.write(folder/'plan.json',job);worker.write(home/'native-updates/active.json',{'id':identifier});prepare_direct(identifier,job);current=worker.Worker(home,identifier)
   before=worker.digest(original_binary);current.stop=lambda:(_ for _ in ()).throw(RuntimeError('injected unknown stop'))
   current.execute();assert worker.read(folder/'plan.json')['state']=='recovery_required';assert worker.digest(original_binary)==before;assert current.api('status')['id']==identity_before;checks.append('unknown stop never replaces a running service or rewinds state')
  except BaseException as error:
   fixture_error=error
   # Preserve the native outcome before cleanup can itself fail. These fixed
   # manager receipts contain invocation/profile evidence, never credentials.
   details={'error_type':type(error).__name__,'error':str(error),'native_receipts':{},'apply_http_failures':apply_failures,'native_fixture_storage_sha256':storage_sha256}
   if current is not None:
    folder=home/'native-updates'/current.id
    paths=[folder/'plan.json',folder/'manager-launch.json',*sorted(folder.glob('manager-stop-*.json'))[-4:]]
    for path in paths:
     try:
      if path.is_file() and not path.is_symlink() and path.stat().st_size<=65536:details['native_receipts'][path.name]=worker.read(path)
     except (OSError,ValueError) as inspect_error:details['native_receipts'][path.name]={'inspection_error':str(inspect_error)}
    details['claim_files']=[p.name for p in folder.glob('manager-claim-*.json')]
    details['startup_definition']=startup_definition_evidence(home,folder,registration)
   details=public_failure_evidence(details)
   details['error']=details['error'][:4096]
   if result:result.with_suffix('.failure.json').write_text(json.dumps(details,indent=2)+'\n')
   print('Native update failure evidence: '+json.dumps(details),flush=True)
   raise RuntimeError('Native update fixture failed; see sanitized failure evidence') from None
  finally:
   try:
    if actual_worker is not None:actual_worker.wait(worker_deadline)
    if current and not plan_never_dispatched(home,current.id,request_id):
     current=worker.Worker(home,current.id)
     # Missing request evidence is unknown, not permission to manufacture a
     # rolling_back plan or invoke a manager operation without its binding.
     assert current.job.get('request_id'),'Native dispatch identity is unknown; retain fixture'
     current.save('rolling_back');current.stop()
     current.save('failed',reason='fixture_cleanup_after_retained_unknown_stop')
     retired=manager(current.id,'retire-runtime')
     assert not retired.get('waiting_for_exit') and not retired.get('unresolved'),retired
    if registration is not None:registration.emergency_remove()
    if proc.poll() is None:proc.terminate()
    proc.wait(timeout=20)
   except BaseException as cleanup_error:
    if fixture_error is None:raise
    print('Fixture cleanup unconfirmed; original failure retained: '+json.dumps({'error_type':type(cleanup_error).__name__}),flush=True)
   finally:log.close()
 receipt={'passed':True,'managed_original':managed,'checks':checks,'status_api_observations':observations,'worker_exit':('exact Windows process handle' if os.name=='nt' else 'same worker operation lock released'),'package_sha256':hashlib.sha256(package.read_bytes()).hexdigest(),'native_fixture_storage_sha256':storage_sha256,'limits':['Release download represented by local exact bundle; GitHub digest selection separately tested','No model/provider or project analysis; only captured fresh UUID registrations']}
 if result:result.write_text(json.dumps(receipt,indent=2)+'\n')
 print(json.dumps(receipt,indent=2))
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--package',type=Path,required=True);p.add_argument('--result',type=Path);p.add_argument('--managed',action='store_true');p.add_argument('--storage-helper',type=Path);a=p.parse_args();run(a.package.resolve(),a.result,a.managed,a.storage_helper)
