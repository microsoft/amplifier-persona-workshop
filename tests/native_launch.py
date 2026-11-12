#!/usr/bin/env python3
"""Exercise an extracted native download using only its bundled runtime (no model calls)."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import time
import urllib.request
import urllib.error
import zipfile
import uuid

@contextlib.contextmanager
def native_fixture(prefix):
    root=Path(tempfile.mkdtemp(prefix=prefix))
    try:yield root
    except BaseException:
        # An unconfirmed process wait must never be followed by deleting its
        # installation. Failed fixtures retain only their own temporary tree.
        print('Retained failed native fixture: '+str(root),file=__import__('sys').stderr)
        raise
    else:shutil.rmtree(root)

def safe_log_tail(path,limit):
    with path.open('rb') as stream:
        stream.seek(0,os.SEEK_END);offset=max(0,stream.tell()-65536);stream.seek(offset)
        data=stream.read(65536)
    # Drop a partial first line so a truncated setup fragment cannot lose its
    # recognizable prefix while retaining capability bytes in the error tail.
    if offset:data=data.partition(b'\n')[2]
    text=data.decode('utf-8',errors='replace')
    return re.sub(r'(#setup=)[^\s"<>]+',r'\1[redacted]',text)[-limit:]

def startup_failure(root,home,result,phase,started,process,budget):
    """Bounded metadata only, captured before the fixture removes partial setup."""
    report={'phase':phase,'elapsed_seconds':time.monotonic()-started,
            'budget_seconds':budget,'process_returncode':process.poll() if process else None,
            'copies':[]}
    paths=[('installation',home)]
    for candidate in root.glob('.persona-setup-*'):
        try:uuid.UUID(candidate.name.removeprefix('.persona-setup-'))
        except ValueError:continue
        paths.append(('unpublished_setup',candidate))
        if len(paths)==5:break
    deadline=time.monotonic()+2
    markers=('config.json','native-installation.json','runtime.json','state.sqlite3',
             'bin/persona-workshop','bin/persona-workshop.exe','runtime/native-bundle.json')
    for kind,path in paths:
        item={'kind':kind,'exists':path.exists(),'symlink':path.is_symlink(),'runtime_files':0,
              'runtime_bytes':0,'entries_examined':0,'bounded':False,'observation_errors':0}
        item['markers']={name:(path/name).is_file() for name in markers} if not item['symlink'] else {}
        pending=[path/'runtime'] if not item['symlink'] else []
        while pending and item['entries_examined']<10000 and time.monotonic()<deadline:
            directory=pending.pop()
            if not directory.is_dir() or directory.is_symlink():continue
            try:
                with os.scandir(directory) as entries:
                    for entry in entries:
                        if item['entries_examined']>=10000 or time.monotonic()>=deadline:
                            item['bounded']=True;break
                        item['entries_examined']+=1
                        if entry.is_symlink():continue
                        if entry.is_dir(follow_symlinks=False):pending.append(Path(entry.path))
                        elif entry.is_file(follow_symlinks=False):
                            item['runtime_files']+=1;item['runtime_bytes']+=entry.stat(follow_symlinks=False).st_size
            except OSError:item['observation_errors']+=1
        item['bounded']|=bool(pending)
        report['copies'].append(item)
    if result:
        target=result.with_suffix('.failure.json');target.parent.mkdir(parents=True,exist_ok=True)
        with target.open('x',encoding='utf-8') as stream:json.dump(report,stream,indent=2);stream.write('\n')
    print('Native setup failure metadata: '+json.dumps(report),file=__import__('sys').stderr)
    return report

def superseded_launch(binary, home, env):
    """A delayed replacement must fail before opening saved state or a listener."""
    root=home/'native-updates';assert not root.exists()
    plan=str(uuid.uuid4());old_nonce=str(uuid.uuid4());folder=root/plan;folder.mkdir(parents=True)
    executable=home/'bin'/binary.name;sha=hashlib.sha256(executable.read_bytes()).hexdigest()
    record={'schema':1,'id':plan,'state':'rolling_back','action':'rollback','previous_sha256':sha,'launch':{'nonce':str(uuid.uuid4()),'mode':'rollback','binary_sha256':sha}}
    marker=root/'active.json';marker.write_text(json.dumps({'id':plan}),encoding='utf-8')
    journal=folder/'plan.json';journal.write_text(json.dumps(record),encoding='utf-8')
    before={p:p.read_bytes() for p in [journal,marker,home/'config.json',*home.glob('state.sqlite3*')]}
    try:
        result=subprocess.run([str(executable),'--home',str(home),'serve'],env={**env,'PERSONA_UPDATE_PLAN':plan,'PERSONA_UPDATE_LAUNCH':old_nonce},capture_output=True,text=True,encoding='utf-8',timeout=10)
        assert result.returncode!=0 and 'Update launch was superseded' in result.stderr,(result.returncode,result.stderr)
        assert all(p.read_bytes()==data for p,data in before.items()),'Rejected old launch changed saved state'
        recovery=subprocess.Popen([str(executable),'--home',str(home),'serve'],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        try:
            base=json.loads((home/'runtime.json').read_text(encoding='utf-8'))['url']
            for _ in range(100):
                try:
                    if request(base,'/auth/session')[0]==200:break
                except (OSError,ValueError):pass
                assert recovery.poll() is None,'Management recovery process exited';time.sleep(.1)
            else:raise AssertionError('Management recovery did not become available')
            headers={'Authorization':'Bearer '+(home/'secrets/control-token').read_text(encoding='utf-8').strip()}
            code,_,update=request(base,'/api/v1/desktop/updates',headers=headers)
            assert code==200 and update['current']['id']==plan
            code,_,_=request(base,'/api/v1/projects',{'alias':'Must remain uncreated','subject_type':'fictional'},headers)
            assert code==503,'Recovery UI must not admit new work'
            journal.write_bytes(b'{damaged')
            code,_,update=request(base,'/api/v1/desktop/updates',headers=headers)
            assert code==200 and update['journal_error'] and update['current'] is None
            assert request(base,'/api/v1/projects',{'alias':'Still not created','subject_type':'fictional'},headers)[0]==503
            assert request(base,'/api/v1/desktop/updates',{'action':'check','request_id':str(uuid.uuid4())},headers)[0]!=200
            assert journal.read_bytes()==b'{damaged' and marker.read_bytes()==before[marker]
        finally:recovery.terminate();recovery.wait(timeout=20)
    finally:shutil.rmtree(root)

def extract(package,folder):
    if package.name.endswith('.zip'):
        with zipfile.ZipFile(package) as source:
            for item in source.infolist():
                name=Path(item.filename)
                if name.is_absolute() or '..' in name.parts:raise ValueError('Unsafe member')
                dest=folder/name
                if item.is_dir():dest.mkdir(parents=True,exist_ok=True);continue
                dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(source.read(item));dest.chmod((item.external_attr>>16)&0o777 or 0o644)
    else:
        with tarfile.open(package) as source:source.extractall(folder,filter='data')
def request(base,path,body=None,headers=None):
    data=None if body is None else json.dumps(body).encode()
    r=urllib.request.Request(base+path,data=data,headers={'Content-Type':'application/json',**(headers or {})})
    try:response=urllib.request.build_opener(urllib.request.ProxyHandler({})).open(r,timeout=20)
    except urllib.error.HTTPError as e:response=e
    with response:return response.status,{k.lower():v for k,v in response.headers.items()},json.load(response)
def run(package,result,browser=False):
    checks=[]
    with native_fixture('Persona native space ') as temporary:
        root=Path(temporary);bundle=root/'download';bundle.mkdir();extract(package,bundle)
        binary=next(p for p in bundle.rglob('persona-workshop.exe' if os.name=='nt' else 'persona-workshop') if p.is_file())
        binary.chmod(0o755);home=root/'private connection café 雪';env={k:v for k,v in os.environ.items() if not k.upper().startswith(('SLACK_','OPENAI_','ANTHROPIC_','AZURE_','GOOGLE_','AMPLIFIER_'))};env['PATH']=str(root/'no-tools');env['PYTHONDONTWRITEBYTECODE']='1'
        def start(initial=False):
            # Fresh setup copies/verifies the complete private runtime. Restart
            # retains its separate 40s allowance; HTTP/mutation limits are unchanged.
            budget=180 if initial else 40;started=time.monotonic()
            log=open(root/'server.log','ab');p=None
            try:
                p=subprocess.Popen([str(binary),'--home',str(home),'--no-browser'],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log)
                while time.monotonic()-started<budget:
                    if p.poll() is not None:raise AssertionError('Native app exited during startup: '+safe_log_tail(root/'server.log',1000))
                    try:
                        endpoint=json.loads((home/'runtime.json').read_text(encoding='utf-8'));base=endpoint['url'];status,_,_=request(base,'/auth/session')
                        if status==200:return p,base,log
                    except (OSError,ValueError):pass
                    time.sleep(.1)
                raise AssertionError('Native app did not become ready')
            except BaseException as error:
                try:startup_failure(root,home,result,'initial_setup' if initial else 'restart',started,p,budget)
                except Exception as diagnostic:error.add_note('Setup diagnostic failed: '+type(diagnostic).__name__)
                try:
                    if p is not None:
                        if p.poll() is None:p.terminate()
                        p.wait(timeout=20)
                except Exception as cleanup:error.add_note('Exact startup process cleanup unconfirmed: '+type(cleanup).__name__)
                finally:log.close()
                raise
        p,base,log=start(initial=True)
        try:
            config=json.loads((home/'config.json').read_text(encoding='utf-8'));assert config['auth_mode']=='local-password' and config['web_transport']=='local-http'
            assert config['extractor'] is None and config['transcription'] is None and config['pdf_backend']=='pypdf';checks.append('fresh native no-agent installation')
            status,_,session=request(base,'/auth/session');assert status==200 and session['first_account_needed']
            status,_,_=request(base,'/api/v1/projects');assert status==401
            status,_,_=request(base,'/auth/session',headers={'Host':'evil.test','Origin':'http://evil.test'});assert status==403;checks.append('unauthenticated and cross-origin refused')
            cap=json.loads((home/'secrets/first-account.json').read_text(encoding='utf-8'))['token'];password='Disposable native password 29'
            if browser:
                fixture=root/'browser-input.json';fixture.write_text(json.dumps({'base':base,'cap':cap,'password':password}))
                subprocess.run([shutil.which('node'),str(Path(__file__).with_name('native-browser.mjs')),str(fixture)],check=True,timeout=90)
                status,headers,session=request(base,'/auth/login',{'username':config['owner'],'password':password})
                checks.append('actual browser first-account and sign-in flow')
            else:status,headers,session=request(base,'/auth/first-account',{'setup_token':cap,'password':password,'start_at_signin':False})
            assert status==200 and session['authenticated']
            encoded=(home/'secrets/local-password.json').read_bytes();assert json.loads(encoded)['password_hash'].startswith('$argon2id$') and password.encode() not in encoded;checks.append('create-only protected local account')
            config_bytes=(home/'config.json').read_bytes();password_hash=hashlib.sha256(encoded).hexdigest()
            reopened=subprocess.run([str(binary),'--home',str(home),'--no-browser'],env=env,capture_output=True,text=True,timeout=20);assert reopened.returncode==0,reopened.stderr;assert (home/'config.json').read_bytes()==config_bytes;checks.append('second open verifies existing identity without replacement')
            status_run=subprocess.run([str(binary),'--home',str(home),'api','GET','/api/v1/status'],env=env,capture_output=True,text=True,timeout=20);assert status_run.returncode==0,status_run.stderr;assert json.loads(status_run.stdout)['id']==config['id'];checks.append('same-connection listener proof before CLI authorization')
            private_python=home/'runtime/python'/('python.exe' if os.name=='nt' else 'bin/python3')
            py=subprocess.run([str(private_python),'-I','-B','-X','utf8','-c','import ssl,sqlite3,json; print(json.dumps({"isolated":__import__("sys").flags.isolated,"ssl":bool(ssl.OPENSSL_VERSION)}))'],env=env,capture_output=True,text=True,timeout=20);assert py.returncode==0,py.stderr;assert json.loads(py.stdout)=={'isolated':1,'ssl':True};checks.append('private Python starts without system tools')
            cookie=headers['set-cookie'].split(';',1)[0]
            token=(home/'secrets/control-token').read_text(encoding='utf-8').strip();authorization={'Authorization':'Bearer '+token}
            status,_,created=request(base,'/api/v1/projects',{'alias':'Synthetic subject','aliases':['Avery'],'subject_type':'fictional'},authorization);assert status==200,(status,created)
            project=created['project'] if 'project' in created else created
            # Real document conversion is invoked by the actual saved installation, with no PATH tools.
            import base64,io
            word=io.BytesIO()
            with zipfile.ZipFile(word,'w') as out:out.writestr('word/document.xml','<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Keep this author statement.</w:t></w:r></w:p></w:body></w:document>')
            pdf_bytes=subprocess.check_output([str(private_python),'-I','-B','-X','utf8','-c','import runpy,base64,sys; m=runpy.run_path(sys.argv[1]); print(base64.b64encode(m["text_pdf"]()).decode())',str(Path(__file__).with_name('test_native_pdf.py'))],env=env,text=True,timeout=20)
            files=[('readable.pdf',base64.b64decode(pdf_bytes)),('note.html',b'<p>Keep this separate HTML statement.</p><script>hidden</script>'),('note.docx',word.getvalue())]
            for name,data in files:
                status,_,current=request(base,'/api/v1/projects/'+project['id'],headers=authorization);assert status==200
                current=current.get('project',current)
                status,_,value=request(base,'/api/v1/projects/'+project['id']+'/file-import',{'expected_revision':current['revision'],'filename':name,'content_base64':base64.b64encode(data).decode(),'authors':['Avery']},authorization)
                assert status==200,(name,status,value)
            checks.append('actual PDF/HTML/DOCX imports use private Python with no system interpreter')
            pdfcheck=subprocess.run([str(private_python),'-I','-B','-X','utf8',str(Path(__file__).with_name('test_native_pdf.py'))],env=env,capture_output=True,text=True,timeout=35);assert pdfcheck.returncode==0,pdfcheck.stdout+pdfcheck.stderr;checks.append('bundled PDF reader: text, attribution, encrypted/scanned refusal and expansion bound')
            status,_,capabilities=request(base,'/api/v1/import-capabilities',headers=authorization);assert status==200 and capabilities['pdf_backend']=='pypdf' and capabilities['transcription_ready'] is False
            project_id=project['id'];status,_,saved=request(base,'/api/v1/projects/'+project_id,headers=authorization);assert status==200

        finally:p.terminate();p.wait(timeout=20);log.close()
        superseded_launch(binary,home,env);checks.extend(['superseded replacement refuses before state/listener mutation','recovery management stays available; unreadable journal is visible, preserved and refuses new work'])
        p,base,log=start()
        try:
            assert (home/'config.json').read_bytes()==config_bytes
            status,_,session=request(base,'/auth/login',{'username':config['owner'],'password':password});assert status==200 and session['authenticated']
            assert hashlib.sha256((home/'secrets/local-password.json').read_bytes()).hexdigest()==password_hash;checks.append('restart retains account, identity, configuration and saved evidence')
            status,_,reloaded=request(base,'/api/v1/projects/'+project_id,headers=authorization);assert status==200 and reloaded==saved
        finally:p.terminate();p.wait(timeout=20);log.close()
        assert not (home/'startup.json').exists();checks.append('startup false does not register an OS service')
    receipt={'passed':True,'checks':checks,'package':str(package),'package_sha256':hashlib.sha256(package.read_bytes()).hexdigest(),'limitations':['No model/provider or transcription call','No publisher signing/notarization claim','Startup opt-in tested separately by the packaged native_startup.py fixture']}
    if result:result.write_text(json.dumps(receipt,indent=2)+'\n')
    print(json.dumps(receipt,indent=2))
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--package',required=True,type=Path);p.add_argument('--result',type=Path);p.add_argument('--browser',action='store_true');a=p.parse_args();run(a.package.resolve(),a.result,a.browser)
