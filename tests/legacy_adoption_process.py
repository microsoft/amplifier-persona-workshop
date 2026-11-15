"""Actual 0.6 Linux service -> complete native bundle, preserving its owner/data.

Uses only a unique disposable user service. --fail-start injects one failure at
existing worker's candidate start boundary; rollback still uses the real manager.
"""
import argparse, fcntl, hashlib, importlib.util, json, os, shutil, sqlite3, ssl, subprocess, sys, tarfile, tempfile, time, traceback, urllib.request, uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('worker',ROOT/'scripts/native_update.py');worker=importlib.util.module_from_spec(spec);spec.loader.exec_module(worker)
def run(*args):
    result=subprocess.run([str(x) for x in args],capture_output=True,text=True,timeout=180)
    if result.returncode:raise RuntimeError(f'{args[1:]}: {result.stderr[-3000:]}')
    return result.stdout
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def wait(test,limit=90):
    deadline=time.monotonic()+limit
    while time.monotonic()<deadline:
        try:
            value=test()
            if value:return value
        except (OSError,ValueError,subprocess.CalledProcessError):pass
        time.sleep(.15)
    raise AssertionError('Disposable fixture did not reach expected state')
def rows(home):
    with sqlite3.connect(home/'state.sqlite3') as db:
        return {table:db.execute('SELECT * FROM '+table+' ORDER BY 1').fetchall() for table in ('projects','jobs','browser_sessions','views')}
def main():
    p=argparse.ArgumentParser();p.add_argument('--package',type=Path,required=True);p.add_argument('--legacy-binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--fail-start',action='store_true');a=p.parse_args()
    assert sys.platform=='linux'
    root=Path(tempfile.mkdtemp(prefix='workshop-legacy-adoption-'));root.chmod(0o700)
    home=root/'legacy-home';home.mkdir(mode=0o700);(home/'bin').mkdir(mode=0o700)
    old=home/'bin/twin-workbench';shutil.copy2(a.legacy_binary,old)
    download=root/'candidate';download.mkdir();binary=download/'persona-workshop'
    name='twin-workbench-adoption-fixture-'+str(uuid.uuid4())+'.service'
    definitions=Path(os.environ.get('XDG_CONFIG_HOME',str(Path.home()/'.config')))/'systemd/user'
    definition=definitions/name;assert not definition.exists();plans=[];checks=[]
    record={'fixture':str(root),'service':name,'package_sha256':digest(a.package),'legacy_sha256':digest(a.legacy_binary),'worker_source_sha256':digest(ROOT/'scripts/native_update.py'),'injected_candidate_start_failure':a.fail_start,'passed':False}
    try:
        with tarfile.open(a.package) as archive:archive.extractall(download,filter='data')
        binary.chmod(0o700)
        run(old,'--home',home,'init','--owner',os.environ['USER'],'--bind','127.0.0.1','--port','0')
        config=json.loads((home/'config.json').read_text())
        definitions.mkdir(parents=True,exist_ok=True)
        unit=f'[Unit]\nDescription=Disposable legacy Workshop adoption fixture\n[Service]\nExecStart={old} --home {home} serve\nRestart=on-failure\nRestartSec=3\nUMask=0077\n[Install]\nWantedBy=default.target\n'
        definition.write_text(unit);definition.chmod(0o600)
        run('systemctl','--user','daemon-reload');run('systemctl','--user','enable','--now',name)
        def api(path='/api/v1/status',body=None):
            r=json.loads((home/'runtime.json').read_text());c=json.loads((home/'config.json').read_text());ctx=ssl.create_default_context(cafile=c.get('tls_ca') or c['tls_cert'])
            headers={'Authorization':'Bearer '+(home/'secrets/control-token').read_text().strip(),'Content-Type':'application/json'}
            req=urllib.request.Request(f'https://127.0.0.1:{r["port"]}'+path,headers=headers,data=None if body is None else json.dumps(body).encode())
            with urllib.request.urlopen(req,context=ctx,timeout=180 if body is not None else 5) as response:return json.load(response)
        assert wait(lambda:api())['version']=='0.6.0'
        with sqlite3.connect(home/'state.sqlite3') as db:
            db.execute("INSERT INTO audit(at,kind,project_id,data) VALUES(1,'adoption-fixture',NULL,'{\"keep\":true}')")
            db.execute('INSERT INTO projects VALUES(?,?)',('kept',json.dumps({'id':'kept','alias':'Synthetic retained project','purpose':'Continuity fixture','aliases':[],'subject_type':'fictional','revision':4,'created_at':1,'updated_at':2,'artifacts':[],'observations':[],'identity':{}})))
            for status in ('complete','interrupted','unknown'):
                job={'id':status,'project_id':'kept','status':status,'stage':status,'note':'retained original outcome; receipt unknown stays unknown','kind':'evaluation','created_at':1,'updated_at':2,'source_revision':4,'result_id':None}
                db.execute('INSERT INTO jobs VALUES(?,?,?)',(status,'kept',json.dumps(job)))
            db.execute('INSERT INTO browser_sessions VALUES(?,?,?,?)',('fixture-session-hash',os.environ['USER'],'fixture-csrf',9999999999999))
            db.execute('INSERT INTO views VALUES(?,?)',('kept-view','{"selected":"kept"}'))
        before=rows(home)
        protected={str(p.relative_to(home)):digest(p) for p in [home/'config.json',*list((home/'secrets').rglob('*')),*list((home/'tls').rglob('*'))] if p.is_file()}
        plan=str(uuid.uuid4());request=str(uuid.uuid4());plans.append(plan)
        prepare=[str(binary),'--home',str(home),'adopt-existing','prepare','--service',name,'--binary',str(old),'--request',plan]
        updates=home/'native-updates';updates.mkdir(mode=0o700,exist_ok=True)
        with (updates/'queue.lock').open('a+b') as competing:
            fcntl.flock(competing,fcntl.LOCK_EX|fcntl.LOCK_NB)
            refused=subprocess.run(prepare,capture_output=True,text=True,timeout=180)
            assert refused.returncode!=0 and not (home/'native-adoption.json').exists()
        checks.append('existing queue lock refuses concurrent preparation without publishing an origin')
        # Terminate only our preparing child after its durable intent, before
        # publishing an active plan. The running original service is untouched.
        child=subprocess.Popen(prepare,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        wait(lambda:(home/'native-adoption.json').exists());child.kill();child.communicate(timeout=30)
        prepared=json.loads(run(*prepare))
        assert prepared['origin']=='observed_legacy_service' and not (home/'native-installation.json').exists()
        assert json.loads(run(*prepare))==prepared
        checks.append('interrupted preparation and lost reply recover the exact request without a new baseline')
        alternate=download/'alternative';shutil.copy2(binary,alternate)
        with alternate.open('ab') as f:f.write(b'fixture distinct complete candidate\n')
        different=prepare.copy();different[0]=str(alternate)
        refused=subprocess.run(different,capture_output=True,text=True)
        assert refused.returncode!=0 and json.loads((home/'native-adoption.json').read_text())==prepared
        checks.append('same request refuses a different candidate without changing retained intent')
        refusal=subprocess.run([str(binary),'--home',str(home),'adopt-existing','apply','--plan',plan,'--request',request],capture_output=True,text=True)
        assert refusal.returncode!=0 and digest(old)==digest(a.legacy_binary)
        checks.append('active original service refused without stopping it')
        run('systemctl','--user','stop',name)
        with sqlite3.connect(home/'state.sqlite3') as db:db.execute('INSERT INTO jobs VALUES(?,?,?)',('pending','kept',json.dumps({'id':'pending','status':'queued','kind':'evaluation'})))
        refusal=subprocess.run([str(binary),'--home',str(home),'adopt-existing','apply','--plan',plan,'--request',request],capture_output=True,text=True)
        assert refusal.returncode!=0 and digest(old)==digest(a.legacy_binary)
        with sqlite3.connect(home/'state.sqlite3') as db:db.execute("DELETE FROM jobs WHERE id='pending'")
        checks.append('new queued work after stop refuses cutover; only fixture queue removed deliberately')
        def terminal(identifier):
            j=json.loads((home/'native-updates'/identifier/'plan.json').read_text())
            return j if j['state'] in ('succeeded','failed','rolled_back','recovery_required') else None
        if a.fail_start:
            folder=home/'native-updates'/plan;j=worker.read(folder/'plan.json');j.update(state='queued',request_id=request,worker_request_id=request)
            worker.write(folder/'plan.json',j);shutil.copytree(folder/'candidate/runtime',folder/'worker-runtime');shutil.copyfile(ROOT/'scripts/native_update.py',folder/'worker.py')
            run(binary,'--home',home,'native-update-manager',plan,'prepare')
            current=worker.Worker(home,plan);actual_start=current.start;calls=[];rollback_errors=[]
            def fail_once():
                calls.append(True)
                if len(calls)==1:
                    with sqlite3.connect(home/'state.sqlite3') as db:db.execute("INSERT INTO audit(at,kind,project_id,data) VALUES(2,'after-backup',NULL,'{\"newer\":true}')")
                    raise OSError('Injected candidate start failure after replacement')
                return actual_start()
            current.start=fail_once;real_rollback=current.rollback
            def traced(reason):
                try:return real_rollback(reason)
                except Exception:rollback_errors.append(traceback.format_exc());raise
            current.rollback=traced;current.execute();record['rollback_errors']=rollback_errors
            final=terminal(plan);record['final']=final;assert final['state']=='rolled_back',record
            assert api()['version']=='0.6.0' and digest(old)==digest(a.legacy_binary) and not (home/'native-installation.json').exists()
            assert not (home/'runtime').exists()
            with sqlite3.connect(home/'state.sqlite3') as db:assert db.execute("SELECT data FROM audit WHERE kind='after-backup'").fetchone()==('{"newer":true}',)
            checks.append('failed candidate start restores original service/binary without baseline or runtime fabrication; later durable row survives')
            successor=str(uuid.uuid4());plans.append(successor);prepare[-1]=successor
            again=json.loads(run(*prepare));assert again['plan_id']==successor and again['previous_plan_id']==plan
            assert worker.read(folder/'legacy-origin.json')['plan_id']==plan
            checks.append('a new adoption after verified rollback retains prior origin and operation history')
        else:
            queued=json.loads(run(binary,'--home',home,'adopt-existing','apply','--plan',plan,'--request',request));assert queued['id']==plan
            final=wait(lambda:terminal(plan),180);record['final']=final;assert final['state']=='succeeded',final
            status=api();assert status['id']==config['id'] and digest(old)==digest(binary)
            marker=worker.read(home/'native-installation.json');assert marker['legacy']['service']['name']==name and marker['adoption']['plan_id']==plan
            repeat=json.loads(run(binary,'--home',home,'adopt-existing','apply','--plan',plan,'--request',request));assert repeat['state']=='succeeded'
            checks.append('actual packaged dispatcher adopts complete bundle and lost-reply retry does not relaunch')
            # Same existing authenticated update API/worker, on the preserved
            # HTTPS/PAM installation. Local exact bundle stands in for download.
            successor=str(uuid.uuid4());plans.append(successor);folder=home/'native-updates'/successor;folder.mkdir(mode=0o700)
            shutil.copytree(download,folder/'candidate');next_binary=folder/'candidate/persona-workshop'
            with next_binary.open('ab') as f:f.write(b'fixture successor packaging\n')
            j={'schema':1,'id':successor,'state':'ready','action':'apply','source':'local_download','entry':'persona-workshop','binary_sha256':digest(next_binary),'runtime_manifest_sha256':digest(folder/'candidate/native-bundle.json'),'installed_sha256':digest(old),'config_sha256':digest(home/'config.json'),'version':status['version'],'release_url':'https://example.invalid/disposable-fixture'}
            worker.write(folder/'plan.json',j);worker.write(home/'native-updates/active.json',{'id':successor})
            api('/api/v1/desktop/updates',{'action':'apply','request_id':str(uuid.uuid4()),'plan_id':successor})
            following=wait(lambda:terminal(successor),180);record['subsequent_update']=following;assert following['state']=='succeeded',following
            assert digest(old)==digest(next_binary) and api()['id']==config['id']
            checks.append('subsequent normal authenticated HTTPS update stops/swaps/restarts through the same original manager')
        assert rows(home)==before
        assert {name:digest(home/name) for name in protected}==protected
        assert definition.read_text()==unit and run('systemctl','--user','is-enabled',name).strip()=='enabled'
        checks.append('exact retained jobs/unknowns/browser sessions/views, configuration, credential/TLS bytes and unit/enablement unchanged')
        record.update(passed=True,checks=checks,identity=config['id'],candidate_sha256=digest(binary),protected_hashes=protected,retained_rows_sha256=hashlib.sha256(json.dumps(before,sort_keys=True).encode()).hexdigest())
    except BaseException as error:
        record.update(error=repr(error),checks=checks);raise
    finally:
        cleanup=[]
        for identifier in plans:
            if not (home/'native-updates'/identifier/'plan.json').exists():continue
            try:
                job=worker.read(home/'native-updates'/identifier/'plan.json')
                if job.get('request_id'):
                    def retired():
                        value=json.loads(run(binary,'--home',home,'native-update-manager',identifier,'retire-worker'))
                        if value.get('unresolved'):raise AssertionError('Fixture worker retirement remains unknown')
                        return value if not value.get('waiting_for_exit') else None
                    value=wait(retired,30)
                    cleanup.append({'plan':identifier,'result':value})
            except Exception as error:cleanup.append({'plan':identifier,'error':repr(error)})
        subprocess.run(['systemctl','--user','stop',name],capture_output=True)
        subprocess.run(['systemctl','--user','disable',name],capture_output=True)
        if definition.exists() and definition.read_text().startswith('[Unit]\nDescription=Disposable legacy Workshop adoption fixture\n'):definition.unlink()
        subprocess.run(['systemctl','--user','daemon-reload'],capture_output=True)
        record['cleanup']=cleanup;a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(json.dumps(record,indent=2)+'\n')
if __name__=='__main__':main()
