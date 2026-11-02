import argparse
import importlib.util
import json
from pathlib import Path
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

spec=importlib.util.spec_from_file_location('transcriber',Path(__file__).parents[1]/'scripts/transcribe-openai.py')
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

class TranscriptionTest(unittest.TestCase):
    def test_wire_format_and_labels_with_real_http_transport(self):
        captured=[]
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args): pass
            def do_POST(self):
                captured.append((dict(self.headers),self.rfile.read(int(self.headers['Content-Length']))))
                self.send_response(200);self.end_headers()
                self.wfile.write(json.dumps({'segments':[{'speaker':'speaker_A','text':'Example','start':0,'end':2}]}).encode())
        server=HTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            with tempfile.TemporaryDirectory() as d:
                p=Path(d);(p/'private-name.wav').write_bytes(b'RIFF synthetic fixture');(p/'key').write_text('fixture-secret')
                args=argparse.Namespace(endpoint=f'http://127.0.0.1:{server.server_port}/v1/audio/transcriptions',allow_local_http=True,model='fixture-diarize',key_file=str(p/'key'))
                result=module.transcribe({'schema':'persona-transcription/1','media_path':str(p/'private-name.wav')},args)
                self.assertEqual(result['segments'][0]['speaker'],'speaker_A')
                headers,body=captured[0];self.assertEqual(headers['Authorization'],'Bearer fixture-secret')
                self.assertIn(b'diarized_json',body);self.assertIn(b'RIFF synthetic fixture',body)
                self.assertNotIn(b'private-name',body);self.assertNotIn(str(p).encode(),body)
        finally:server.shutdown();server.server_close()

    def test_missing_speakers_or_invalid_timestamps_are_not_invented(self):
        for value in [None,[],{'segments':['invalid']},{'segments':[]},{'segments':[{'text':'x','start':0,'end':1}]},{'segments':[{'speaker':'A','text':'x','start':2,'end':1}]}]:
            with self.assertRaises(ValueError): module.normalized(value)

    def test_external_http_is_rejected_before_reading_recording_or_key(self):
        args=argparse.Namespace(endpoint='http://example.com/transcribe',allow_local_http=True,model='x',key_file='/nonexistent')
        with self.assertRaisesRegex(ValueError,'HTTPS'):module.transcribe({'schema':'persona-transcription/1','media_path':'/missing.wav'},args)

if __name__=='__main__':unittest.main()
