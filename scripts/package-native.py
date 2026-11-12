#!/usr/bin/env python3
"""Build self-contained native Persona Workshop downloads from native CI binaries."""
import argparse
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import zipfile
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('python_support',ROOT/'scripts/vendor/python/python_support.py')
python_support=importlib.util.module_from_spec(spec);spec.loader.exec_module(python_support)
TARGETS={
'x86_64-unknown-linux-gnu':('linux','x86_64'),'aarch64-unknown-linux-gnu':('linux','aarch64'),
'x86_64-apple-darwin':('macos','x86_64'),'aarch64-apple-darwin':('macos','aarch64'),
'x86_64-pc-windows-msvc':('windows','x86_64'),'aarch64-pc-windows-msvc':('windows','aarch64')}
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def verify_vendor():
    vendor=json.loads((ROOT/'scripts/vendor/python/vendor.json').read_text())
    for name,expected in vendor['files'].items():
        path=ROOT/'scripts/vendor/python'/name
        if not path.is_file() or path.is_symlink() or digest(path)!=expected:raise ValueError('Pinned private Python support changed')
def install_pdf(private,cache,system):
    asset=json.loads((ROOT/'scripts/pypdf-lock.json').read_text())
    wheel=python_support.download(asset,cache)
    site=private/('Lib/site-packages' if system=='windows' else 'lib/python3.13/site-packages')
    site.mkdir(parents=True,exist_ok=True)
    with zipfile.ZipFile(wheel) as archive:
        names=set()
        for entry in archive.infolist():
            name=python_support.safe_name(entry.filename)
            if name in names or not name.startswith(('pypdf/', 'pypdf-7.0.1.dist-info/')):
                raise ValueError('Unexpected PDF wheel member')
            names.add(name)
            if entry.is_dir():continue
            if entry.file_size>8*1024*1024 or stat.S_ISLNK(entry.external_attr>>16):raise ValueError('Unsafe PDF wheel member')
            destination=site/name;destination.parent.mkdir(parents=True,exist_ok=True)
            with destination.open('xb') as output:output.write(archive.read(entry))
    license_path=site/'pypdf-7.0.1.dist-info/licenses/LICENSE'
    if not license_path.is_file():raise ValueError('Missing PDF license notice')
    return {'backend':'pypdf','version':asset['version'],'wheel_sha256':asset['sha256'],'license_sha256':digest(license_path),'ocr':False,'encrypted_pdf':False}

def package(binary,target,version,output,cache):
    verify_vendor()
    if target not in TARGETS or not re.fullmatch(r'\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?',version):raise ValueError('Invalid target/version')
    if not binary.is_file() or binary.is_symlink():raise ValueError('Use a native CI executable')
    system,architecture=TARGETS[target]; output.mkdir(parents=True,exist_ok=True)
    guard=ROOT/'scripts/vendor/native-binary';meta=json.loads((guard/'vendor.json').read_text())
    if digest(guard/'native_binary.py')!=meta['sha256']:raise ValueError('Native binary validator changed')
    spec=importlib.util.spec_from_file_location('native_binary',guard/'native_binary.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    binary_report=module.inspect(binary.read_bytes(),system+'-'+architecture)
    reported=subprocess.run([str(binary.resolve()),'--version'],capture_output=True,text=True,timeout=15,check=True).stdout.strip()
    if reported!='persona-workshop '+version:raise ValueError('Package version does not match the actual executable')
    if system=='macos':
        libraries=subprocess.check_output(['/usr/bin/otool','-L',str(binary)],text=True).splitlines()[1:]
        if any(not row.strip().split(' (',1)[0].startswith(('/usr/lib/','/System/Library/')) for row in libraries):raise ValueError('Unexpected external Mac dynamic library')
    with tempfile.TemporaryDirectory(prefix='persona-native-') as temp:
        stage=Path(temp)/'bundle';stage.mkdir()
        if system=='macos':
            content=stage/'Persona Workshop.app/Contents';binroot=content/'MacOS';resources=content/'Resources'
            content.mkdir(parents=True)
            (content/'Info.plist').write_bytes(plistlib.dumps({'CFBundleName':'Persona Workshop','CFBundleDisplayName':'Persona Workshop','CFBundleIdentifier':'com.microsoft.constellation.persona','CFBundleExecutable':'persona-workshop','CFBundleVersion':version,'CFBundleShortVersionString':version,'CFBundlePackageType':'APPL','LSMinimumSystemVersion':'11.0','LSUIElement':True}))
        else:binroot=stage;resources=stage
        binroot.mkdir(parents=True,exist_ok=True);resources.mkdir(parents=True,exist_ok=True)
        executable='persona-workshop.exe' if system=='windows' else 'persona-workshop'
        shutil.copyfile(binary,binroot/executable);(binroot/executable).chmod(0o755)
        private=resources/'runtime/python'
        python=python_support.prepare(target,cache,private)
        pdf=install_pdf(private,cache,system)
        inventory={p.relative_to(private).as_posix():digest(p) for p in sorted(private.rglob('*')) if p.is_file()}
        (resources/'native-bundle.json').write_text(json.dumps({'schema':1,'component':'persona-workshop','version':version,'target':target,'python':python,'pdf':pdf,'python_files':inventory},indent=2)+'\n')
        (stage/'START-HERE.txt').write_text('Open Persona Workshop.app on Mac, persona-workshop.exe on Windows, or persona-workshop on Linux.\nChoose a password for your workshop. No Rust, Python, WSL, Poppler or developer tools are needed. Keep the download together.\nText, email, HTML, DOCX, ODT and readable-text PDF conversion are bundled. Review extracted layout and authorship. Encrypted/scanned PDF and OCR are not supported.\nAnalysis and recording transcription use only providers you explicitly configure; this download does not configure or call a model.\nThis preview is not publisher-signed or notarized. Verify its published checksum; do not disable system security protections.\n')
        shutil.copyfile(ROOT/'LICENSE',stage/'LICENSE')
        filename=f'persona-workshop-{version}-{target}'+('.tar.gz' if system=='linux' else '.zip');destination=output/filename
        if system=='linux':
            with tarfile.open(destination,'w:gz') as out:
                for p in sorted(stage.rglob('*')):
                    if p.is_file():out.add(p,arcname=p.relative_to(stage).as_posix(),recursive=False)
        else:
            with zipfile.ZipFile(destination,'w',compression=zipfile.ZIP_DEFLATED) as out:
                for p in sorted(stage.rglob('*')):
                    if not p.is_file():continue
                    item=zipfile.ZipInfo(p.relative_to(stage).as_posix());item.create_system=3;item.external_attr=(stat.S_IFREG|(0o755 if p.stat().st_mode&0o111 else 0o644))<<16;item.compress_type=zipfile.ZIP_DEFLATED
                    out.writestr(item,p.read_bytes())
        record={'schema':1,'component':'persona-workshop','version':version,'target':target,'os':system,'architecture':architecture,'filename':filename,'entry':(binroot/executable).relative_to(stage).as_posix(),'sha256':digest(destination),'binary_sha256':digest(binary),'binary_metadata':binary_report,'python':python,'pdf':pdf,'signed_or_notarized':False,'requires_developer_toolchain':False}
        (output/(filename+'.json')).write_text(json.dumps(record,indent=2)+'\n');(output/(filename+'.sha256')).write_text(record['sha256']+'  '+filename+'\n')
        return record
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',required=True,type=Path);p.add_argument('--target',required=True,choices=TARGETS);p.add_argument('--version',required=True);p.add_argument('--output',type=Path,default=Path('dist'));p.add_argument('--python-cache',type=Path,default=Path('.python-cache'));a=p.parse_args();print(json.dumps(package(a.binary,a.target,a.version,a.output,a.python_cache),indent=2))
