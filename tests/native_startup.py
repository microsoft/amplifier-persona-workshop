#!/usr/bin/env python3
"""Actual packaged current-user startup, using only a fresh UUID installation.

The driver uses the runner's Python. The app receives only OS tools on PATH;
no system Python, Rust, Node, provider or model is used by the installation.
"""
import argparse
import base64
import ctypes
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
import xml.etree.ElementTree as ET

from native_launch import extract, request


def checked(argv, **kwargs):
    return subprocess.run(argv, check=True, capture_output=True, text=True, timeout=40, **kwargs).stdout


def same_path(left, right):
    def normalized(value):
        return os.path.normcase(os.path.normpath(str(value).removeprefix('\\\\?\\')))
    return normalized(left) == normalized(right)


def windows_arguments(value):
    shell = ctypes.WinDLL('shell32', use_last_error=True)
    shell.CommandLineToArgvW.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_int)]
    shell.CommandLineToArgvW.restype = ctypes.POINTER(ctypes.c_wchar_p)
    count = ctypes.c_int()
    result = shell.CommandLineToArgvW('fixture.exe ' + value, ctypes.byref(count))
    if not result:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        return [result[i] for i in range(1, count.value)]
    finally:
        ctypes.WinDLL('kernel32').LocalFree(ctypes.cast(result, ctypes.c_void_p))


class Registration:
    def __init__(self, root, home, identifier):
        self.root, self.home = root, home
        self.label = 'persona-workshop-' + str(uuid.UUID(identifier))
        self.executable = home / 'bin' / ('persona-workshop.exe' if os.name == 'nt' else 'persona-workshop')
        self.validated = False
        self.started = False
        self.snapshot = None
        self.handle = None
        self.pid = None
        if sys.platform == 'darwin':
            self.file = Path.home() / 'Library/LaunchAgents' / (self.label + '.plist')
        elif os.name != 'nt':
            self.file = Path.home() / '.config/systemd/user' / (self.label + '.service')

    def read(self):
        if os.name == 'nt':
            # Console-independent Unicode readback. The product's own second API
            # request below separately exercises its COM query under a GUI parent.
            output = self.root / 'query-task.json'
            command = r"""$ErrorActionPreference = 'Stop'
function Resolve-UserSid([string] $name) {
    if ([string]::IsNullOrWhiteSpace($name)) { throw 'Task owner missing' }
    if ($name.StartsWith('S-1-')) { return $name }
    return ([Security.Principal.NTAccount]::new($name)).Translate([Security.Principal.SecurityIdentifier]).Value
}
$s = New-Object -ComObject Schedule.Service
$s.Connect()
$t = $s.GetFolder('\').GetTask($env:PERSONA_FIXTURE_TASK)
$d = $t.Definition
$owners = @()
foreach ($trigger in $d.Triggers) {
    if ($trigger.Type -ne 9) { throw 'Unexpected startup trigger' }
    $owners += Resolve-UserSid $trigger.UserId
}
$v = @{xml=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($t.Xml));sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;principal_sid=(Resolve-UserSid $d.Principal.UserId);trigger_sids=@($owners)}
[IO.File]::WriteAllText($env:PERSONA_FIXTURE_OUTPUT,($v|ConvertTo-Json -Compress),[Text.UTF8Encoding]::new($false))
"""
            env = dict(os.environ, PERSONA_FIXTURE_TASK=self.label, PERSONA_FIXTURE_OUTPUT=str(output))
            powershell = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'
            # A fixed native command avoids requiring script execution-policy
            # changes on an ordinary Windows desktop.
            checked([str(powershell), '-NoProfile', '-NonInteractive', '-Command', command], env=env)
            value = json.loads(output.read_text(encoding='utf-8'))
            # COM returns a Unicode BSTR whose XML declaration still says UTF-16.
            # Our transport explicitly encodes it as UTF-8; decode that transport
            # before passing Unicode (not mislabeled bytes) to the XML parser.
            xml = ET.fromstring(base64.b64decode(value['xml']).decode('utf-8'))
            ns = {'t': 'http://schemas.microsoft.com/windows/2004/02/mit/task'}
            actions = xml.findall('./t:Actions/t:Exec', ns)
            principals = xml.findall('./t:Principals/t:Principal', ns)
            assert len(actions) == len(principals) == 1
            assert same_path(actions[0].findtext('t:Command', namespaces=ns), self.executable)
            args = windows_arguments(actions[0].findtext('t:Arguments', namespaces=ns))
            assert len(args) == 3 and args[0] == '--home' and same_path(args[1], self.home) and args[2] == 'serve', args
            principal = principals[0]
            assert value['principal_sid'] == value['sid']
            assert value['trigger_sids'] == [value['sid']]
            assert principal.findtext('t:LogonType', namespaces=ns) == 'InteractiveToken'
            assert principal.findtext('t:RunLevel', namespaces=ns) in (None, 'LeastPrivilege')
            triggers = xml.findall('./t:Triggers/t:LogonTrigger', ns)
            assert len(triggers) == 1 and triggers[0].findtext('t:UserId', namespaces=ns)
            result = {'label': self.label, 'executable': str(self.executable), 'arguments': args, 'sid': value['sid']}
        elif sys.platform == 'darwin':
            value = plistlib.loads(self.file.read_bytes())
            assert value['Label'] == self.label
            assert value['ProgramArguments'] == [str(self.executable), '--home', str(self.home), 'serve']
            assert value['RunAtLoad'] is True
            result = value
        else:
            text = self.file.read_text(encoding='utf-8')
            expected = f'ExecStart="{self.executable}" --home "{self.home}" serve'
            assert expected in text.splitlines(), text
            assert 'WantedBy=default.target' in text.splitlines()
            assert checked(['systemctl', '--user', 'is-enabled', self.label + '.service']).strip() == 'enabled'
            result = {'label': self.label, 'unit': text}
        self.validated = True
        if self.snapshot is not None:
            assert result == self.snapshot, 'Idempotent startup altered the saved registration'
        self.snapshot = result
        return result

    def start(self):
        self.read()
        # Track an attempted start too: a lost command reply does not prove no child.
        self.started = True
        if os.name == 'nt':
            checked(['schtasks.exe', '/Run', '/TN', self.label])
        elif sys.platform == 'darwin':
            checked(['/bin/launchctl', 'bootstrap', f'gui/{os.getuid()}', str(self.file)])
        else:
            checked(['systemctl', '--user', 'start', self.label + '.service'])

    def bind_process(self, pid):
        self.pid = pid
        if os.name == 'nt':
            kernel = ctypes.WinDLL('kernel32', use_last_error=True)
            kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
            kernel.OpenProcess.restype = ctypes.c_void_p
            kernel.QueryFullProcessImageNameW.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_uint32)]
            kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
            kernel.CloseHandle.argtypes = [ctypes.c_void_p]
            self.handle = kernel.OpenProcess(0x1000 | 0x100000, False, pid)
            assert self.handle, ctypes.WinError(ctypes.get_last_error())
            buf, length = ctypes.create_unicode_buffer(32768), ctypes.c_uint32(32768)
            assert kernel.QueryFullProcessImageNameW(self.handle, 0, buf, ctypes.byref(length))
            assert same_path(buf.value, self.executable), buf.value
        elif sys.platform == 'darwin':
            info = checked(['/bin/launchctl', 'print', f'gui/{os.getuid()}/{self.label}'])
            assert re.search(r'\bpid = ' + str(pid) + r'\b', info), info
        else:
            actual = checked(['systemctl', '--user', 'show', self.label + '.service', '--property=MainPID', '--value']).strip()
            assert actual == str(pid), (actual, pid)
            assert same_path(Path(f'/proc/{pid}/exe').resolve(), self.executable)

    def stop(self):
        if not self.started:
            return
        assert self.validated
        if os.name == 'nt':
            checked(['schtasks.exe', '/End', '/TN', self.label])
            if self.handle:
                kernel = ctypes.WinDLL('kernel32', use_last_error=True)
                kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
                kernel.CloseHandle.argtypes = [ctypes.c_void_p]
                assert kernel.WaitForSingleObject(self.handle, 30000) == 0, 'Known task process has not exited'
                kernel.CloseHandle(self.handle)
                self.handle = None
        elif sys.platform == 'darwin':
            checked(['/bin/launchctl', 'bootout', f'gui/{os.getuid()}/{self.label}'])
            if self.pid is not None:
                for _ in range(300):
                    try:
                        os.kill(self.pid, 0)
                    except ProcessLookupError:
                        break
                    time.sleep(.1)
                else:
                    raise AssertionError('Known launch agent process has not exited')
        else:
            checked(['systemctl', '--user', 'stop', self.label + '.service'])
        self.started = False

    def absent(self):
        if os.name == 'nt':
            assert subprocess.run(['schtasks.exe', '/Query', '/TN', self.label], capture_output=True, timeout=20).returncode != 0
        else:
            assert not self.file.exists()
            if sys.platform != 'darwin':
                checked(['systemctl', '--user', 'daemon-reload'])

    def emergency_remove(self):
        # Only this fresh UUID with exact captured ownership may be removed.
        if not self.validated:
            self.read()
        self.stop()
        if os.name == 'nt':
            checked(['schtasks.exe', '/Delete', '/TN', self.label, '/F'])
        elif sys.platform == 'darwin':
            self.file.unlink()
        else:
            checked(['systemctl', '--user', 'disable', self.label + '.service'])
            self.file.unlink()
            checked(['systemctl', '--user', 'daemon-reload'])
        self.absent()


def run(package, result):
    checks = []
    with tempfile.TemporaryDirectory(prefix='Persona startup ') as temporary:
        root = Path(temporary).resolve()
        bundle = root / 'download'
        bundle.mkdir()
        extract(package, bundle)
        binary = next(p for p in bundle.rglob('persona-workshop.exe' if os.name == 'nt' else 'persona-workshop') if p.is_file())
        binary.chmod(0o755)
        home = root / 'workshop café 雪'
        env = {k: v for k, v in os.environ.items() if not k.upper().startswith(('SLACK_', 'OPENAI_', 'ANTHROPIC_', 'AZURE_', 'GOOGLE_', 'AMPLIFIER_'))}
        os_tools = root / 'os-tools'
        os_tools.mkdir()
        if os.name == 'nt':
            env['PATH'] = str(Path(os.environ['SystemRoot']) / 'System32')
            data = binary.read_bytes()
            pe = int.from_bytes(data[0x3c:0x40], 'little')
            assert int.from_bytes(data[pe + 24 + 68:pe + 24 + 70], 'little') == 2, 'Must test actual Windows GUI executable'
        else:
            if sys.platform != 'darwin':
                (os_tools / 'systemctl').symlink_to(shutil.which('systemctl'))
            env['PATH'] = str(os_tools)
        env['PYTHONDONTWRITEBYTECODE'] = '1'
        processes = []
        registration = None
        removed = False
        log = (root / 'server.log').open('ab')

        def ready(process=None, old_pid=None):
            for _ in range(400):
                if process is not None and process.poll() is not None:
                    raise AssertionError('Native startup exited: ' + (root / 'server.log').read_text(encoding='utf-8', errors='replace')[-1800:])
                try:
                    base = json.loads((home / 'runtime.json').read_text(encoding='utf-8'))['url']
                    token = (home / 'secrets/control-token').read_text(encoding='utf-8').strip()
                    status, _, value = request(base, '/api/v1/status', headers={'Authorization': 'Bearer ' + token})
                    if status == 200 and value['pid'] != old_pid:
                        return base, value, {'Authorization': 'Bearer ' + token}
                except (OSError, ValueError):
                    pass
                time.sleep(.1)
            raise AssertionError('Native startup did not become ready')

        def launch():
            process = subprocess.Popen([str(binary), '--home', str(home), '--no-browser'], env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
            processes.append(process)
            return process, ready(process)

        def stop(process):
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=30)

        try:
            process, (base, status, authorization) = launch()
            config = json.loads((home / 'config.json').read_text(encoding='utf-8'))
            registration = Registration(root, home, config['id'])
            cap = json.loads((home / 'secrets/first-account.json').read_text(encoding='utf-8'))['token']
            code, _, account = request(base, '/auth/first-account', {'setup_token': cap, 'password': 'Disposable startup password 29', 'start_at_signin': True})
            assert code == 200 and account['authenticated'], (code, account)
            startup = request(base, '/api/v1/startup', headers=authorization)[2]
            assert startup['configured'] is True, startup
            registration.read()
            checks.append('first-account opt-in registers exact current-user UUID, executable, home and serve arguments')
            code, _, value = request(base, '/api/v1/startup', {'enabled': True}, authorization)
            assert code == 200 and value == {'configured': True}, (code, value)
            registration.read()
            checks.append('idempotent startup query from packaged app (Windows GUI subsystem) preserves Unicode registration')
            preserved = {name: (home / name).read_bytes() for name in ['config.json', 'secrets/control-token', 'secrets/local-password.json', 'native-installation.json']}
            old_pid = status['pid']
            stop(process)
            registration.start()
            base, status, authorization = ready(old_pid=old_pid)
            assert status['id'] == config['id'] and status['pid'] != old_pid
            registration.bind_process(status['pid'])
            assert all((home / name).read_bytes() == data for name, data in preserved.items())
            checks.append('actual OS manager launches owned installed binary with same identity, account and configuration')
            registration.stop()
            process, (base, status, authorization) = launch()
            code, _, value = request(base, '/api/v1/startup', {'enabled': False}, authorization)
            assert code == 200 and value == {'configured': False}, (code, value)
            registration.absent()
            removed = True
            checks.append('native API removes only captured registration after OS-launched process exit')
            assert all((home / name).read_bytes() == data for name, data in preserved.items())
            stop(process)
        finally:
            try:
                for process in processes:
                    stop(process)
                if registration is not None and not removed:
                    registration.emergency_remove()
            finally:
                log.close()
        report = {'passed': True, 'platform': sys.platform, 'package_sha256': hashlib.sha256(package.read_bytes()).hexdigest(), 'checks': checks, 'registration': registration.snapshot, 'cleanup': 'exact UUID registration removed; all fixture processes stopped', 'limits': ['OS manager launch exercised without logging the runner out and in', 'Windows CI runners are recorded by workflow; no claim of every supported OS version', 'No model/provider, production installation or publisher signing operation']}
    result.parent.mkdir(parents=True, exist_ok=True)
    result.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', type=Path, required=True)
    parser.add_argument('--result', type=Path, required=True)
    arguments = parser.parse_args()
    run(arguments.package.resolve(), arguments.result.resolve())
