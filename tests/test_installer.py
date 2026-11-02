"""Run the installer against isolated service-manager fixtures, without sudo."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

COMMAND = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
with open(os.environ['INSTALL_COMMANDS'], 'a') as f:
    f.write(json.dumps([name, *sys.argv[1:]]) + '\\n')
home = Path(os.environ['INSTALL_EXPECTED_HOME'])
if name == 'systemctl':
    if 'cat' in sys.argv:
        sys.exit(0 if os.environ['INSTALL_LEGACY_SERVICE'] == '1' else 1)
    if 'restart' in sys.argv:
        (home / 'runtime.json').write_text(json.dumps({'port':18448, 'url':'https://127.0.0.1:18448'}))
if name == 'persona-workshop':
    home = Path(sys.argv[sys.argv.index('--home') + 1])
    assert 'init' in sys.argv
    (home / 'config.json').write_text(json.dumps({'id':'fixture', 'pam_service':'persona-workshop'}))
'''


class InstallerTests(unittest.TestCase):
    def run_install(self, legacy=False, explicit=False):
        with tempfile.TemporaryDirectory(prefix='persona-install-') as folder:
            base = Path(folder)
            repo, commands = base / 'repo', base / 'commands'
            (repo / 'target/release').mkdir(parents=True)
            (repo / 'ops').mkdir()
            commands.mkdir()
            shutil.copy(ROOT / 'install.sh', repo / 'install.sh')
            shutil.copy(ROOT / 'ops/pam-persona-workshop', repo / 'ops/pam-persona-workshop')
            for name in ['cargo', 'cc', 'pkg-config', 'pdftotext', 'sudo', 'systemctl', 'curl']:
                p = commands / name
                p.write_text(COMMAND)
                p.chmod(0o700)
            binary = repo / 'target/release/persona-workshop'
            binary.write_text(COMMAND)
            binary.chmod(0o700)
            old = base / 'data/twin-workbench/default'
            fresh = base / 'data/persona-workshop/default'
            previous_config = json.dumps({'id':'fixture', 'pam_service':'existing-owner-pam'})
            if legacy:
                (old / 'bin').mkdir(parents=True)
                (old / 'bin/twin-workbench').write_text('old executable')
                (old / 'config.json').write_text(previous_config)
                (old / 'projects.db').write_bytes(b'preserve existing project data')
            target = base / 'custom' if explicit else old if legacy else fresh
            log = base / 'commands.jsonl'
            env = dict(os.environ, PATH=f'{commands}:{os.environ["PATH"]}',
                       XDG_DATA_HOME=str(base / 'data'), XDG_CONFIG_HOME=str(base / 'config'),
                       INSTALL_COMMANDS=str(log), INSTALL_EXPECTED_HOME=str(target),
                       INSTALL_LEGACY_SERVICE='1' if legacy and not explicit else '0')
            args = ['bash', str(repo / 'install.sh'), '--no-browser']
            if explicit:
                args.extend(['--home', str(target)])
            result = subprocess.run(args, env=env, capture_output=True, text=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            unit = base / 'config/systemd/user/persona-workshop-fixture.service'
            self.assertIn(str(target / 'bin/persona-workshop'), unit.read_text())
            self.assertTrue((target / 'bin/persona-workshop').is_file())
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            disabled = ['systemctl', '--user', 'disable', '--now', 'twin-workbench-fixture.service']
            self.assertEqual(disabled in calls, legacy and not explicit)
            if legacy:
                self.assertEqual((old / 'config.json').read_text(), previous_config)
                self.assertEqual((old / 'projects.db').read_bytes(), b'preserve existing project data')
                if not explicit:
                    self.assertTrue((old / 'bin/twin-workbench').is_symlink())
                    self.assertFalse(fresh.exists())

    def test_fresh_install_uses_persona_names(self):
        self.run_install()

    def test_upgrade_preserves_data_and_retires_previous_service(self):
        self.run_install(legacy=True)

    def test_explicit_new_home_leaves_previous_installation_alone(self):
        self.run_install(legacy=True, explicit=True)


if __name__ == '__main__':
    unittest.main()
