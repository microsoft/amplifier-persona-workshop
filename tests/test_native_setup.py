"""Failure evidence must preserve copy progress without account/credential data."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import time
import unittest
import uuid
from unittest.mock import Mock

from native_launch import startup_failure, native_fixture, safe_log_tail


class NativeSetupEvidence(unittest.TestCase):
    def test_partial_unpublished_runtime_is_counted_without_reading_secrets(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / ('.persona-setup-' + str(uuid.uuid4()))
            (stage / 'runtime/python').mkdir(parents=True)
            (stage / 'runtime/python/module.py').write_bytes(b'copied')
            (stage / 'secrets').mkdir()
            (stage / 'secrets/control-token').write_text('never-print-this')
            (stage / 'config.json').write_text('{"owner":"private-owner"}')
            process = Mock()
            process.poll.return_value = None
            output = io.StringIO()
            result = root / 'result.json'
            with contextlib.redirect_stderr(output):
                value = startup_failure(root, root / 'home', result, 'initial_setup', time.monotonic(), process, 180)
            staged = next(item for item in value['copies'] if item['kind'] == 'unpublished_setup')
            self.assertEqual(staged['runtime_files'], 1)
            self.assertEqual(staged['runtime_bytes'], 6)
            self.assertTrue(staged['markers']['config.json'])
            saved = result.with_suffix('.failure.json').read_text()
            self.assertNotIn('never-print-this', saved + output.getvalue())
            self.assertNotIn('private-owner', saved + output.getvalue())
            self.assertNotIn('control-token', saved + output.getvalue())
            self.assertEqual(json.loads(saved)['budget_seconds'], 180)

    def test_unrelated_directory_is_not_enumerated(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            other = root / '.persona-setup-not-a-uuid/runtime'
            other.mkdir(parents=True)
            (other / 'ignore').write_bytes(b'untouched')
            with contextlib.redirect_stderr(io.StringIO()):
                value = startup_failure(root, root / 'home', None, 'restart', time.monotonic(), None, 40)
            self.assertEqual(len(value['copies']), 1)
            self.assertEqual(value['copies'][0]['runtime_files'], 0)

    def test_prior_failure_evidence_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = root / 'result.json'
            path = result.with_suffix('.failure.json')
            path.write_bytes(b'first observation')
            with self.assertRaises(FileExistsError):
                startup_failure(root, root / 'home', result, 'restart', time.monotonic(), None, 40)
            self.assertEqual(path.read_bytes(), b'first observation')

    def test_unconfirmed_failure_retains_fixture_until_explicit_test_cleanup(self):
        import shutil
        root = None
        try:
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaisesRegex(RuntimeError, 'unconfirmed'):
                with native_fixture('Persona diagnostic unit ') as root:
                    (root / 'evidence').write_text('retained')
                    raise RuntimeError('unconfirmed process exit')
            self.assertEqual((root / 'evidence').read_text(), 'retained')
        finally:
            if root:
                shutil.rmtree(root)

    def test_setup_capability_is_redacted_before_log_truncation(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'log'
            path.write_text('{"url":"http://127.0.0.1:1/#setup=private-capability"}\n')
            value = safe_log_tail(path, 1000)
            self.assertNotIn('private-capability', value)
            self.assertIn('#setup=[redacted]', value)


if __name__ == '__main__':
    unittest.main()
