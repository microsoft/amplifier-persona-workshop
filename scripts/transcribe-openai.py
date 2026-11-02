#!/usr/bin/env python3
"""Recording adapter for Persona Workshop; speaker labels never imply identity."""
import argparse
import json
import math
import mimetypes
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def normalized(value):
    if not isinstance(value, dict): raise ValueError('Provider returned an invalid transcript')
    segments = value.get('segments')
    if not isinstance(segments, list) or not 1 <= len(segments) <= 5000:
        raise ValueError('Provider did not return bounded speaker-labeled segments')
    result = []
    for s in segments:
        if not isinstance(s, dict): raise ValueError('Provider returned an invalid transcript segment')
        speaker, text = s.get('speaker'), s.get('text')
        start, end = s.get('start'), s.get('end')
        if not isinstance(speaker, str) or not speaker.strip() or len(speaker) > 160:
            raise ValueError('A speaker label is missing')
        if not isinstance(text, str) or not text.strip() or len(text) > 100000:
            raise ValueError('A transcript segment is missing or too long')
        if any(type(x) not in (int, float) or not math.isfinite(x) for x in (start, end)) or not 0 <= start <= end:
            raise ValueError('Invalid segment timing')
        result.append(dict(speaker=speaker, text=text, start=start, end=end))
    return {'segments': result}


def transcribe(request, args):
    if not isinstance(request, dict) or request.get('schema') != 'persona-transcription/1':
        raise ValueError('Unsupported transcription request')
    url = urllib.parse.urlsplit(args.endpoint)
    local = args.allow_local_http and url.scheme == 'http' and url.hostname in ('127.0.0.1', 'localhost', '::1')
    if (url.scheme != 'https' and not local) or not url.hostname or url.username or url.password or url.fragment or url.query:
        raise ValueError('Use a verified HTTPS transcription endpoint')
    path = Path(request['media_path'])
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise ValueError('Recording must be a regular local file')
    if not 0 < path.stat().st_size <= 20 * 1024 * 1024:
        raise ValueError('Recording must be between 1 byte and 20 MB')
    data = path.read_bytes()
    if len(data) > 20 * 1024 * 1024:
        raise ValueError('Recording changed beyond the size limit')
    key = Path(args.key_file).read_text().strip() if args.key_file else os.environ.get('OPENAI_API_KEY', '')
    if not key:
        raise ValueError('Configure a transcription account key')
    boundary = 'persona-' + uuid.uuid4().hex
    fields = {'model': args.model, 'response_format': 'diarized_json', 'chunking_strategy': 'auto'}
    body = bytearray()
    for name, value in fields.items():
        if '\r' in value or '\n' in value: raise ValueError('Invalid transcription setting')
        body.extend(f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode())
    # A neutral filename avoids sending a private path or caller-supplied headers.
    suffix = path.suffix.lower()
    if suffix not in ('.wav', '.mp3', '.m4a', '.ogg', '.webm', '.mp4', '.flac'):
        raise ValueError('Unsupported recording format')
    mime = mimetypes.guess_type('recording' + suffix)[0] or 'application/octet-stream'
    body.extend(f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="recording{suffix}"\r\nContent-Type: {mime}\r\n\r\n'.encode())
    body.extend(data)
    body.extend(f'\r\n--{boundary}--\r\n'.encode())
    req = urllib.request.Request(args.endpoint, data=bytes(body), headers={
        'Authorization': 'Bearer ' + key, 'Content-Type': 'multipart/form-data; boundary=' + boundary})
    try:
        with urllib.request.build_opener(NoRedirect()).open(req, timeout=240) as response:
            raw = response.read(3 * 1024 * 1024 + 1)
    except urllib.error.HTTPError as exc:
        raise ValueError(f'Transcription provider returned HTTP {exc.code}; check account and model support') from None
    if len(raw) > 3 * 1024 * 1024: raise ValueError('Transcript exceeds its size limit')
    return normalized(json.loads(raw))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--endpoint', default='https://api.openai.com/v1/audio/transcriptions')
    p.add_argument('--model', required=True, help='A model explicitly supporting diarized_json speaker segments')
    p.add_argument('--key-file')
    p.add_argument('--allow-local-http', action='store_true', help='Only for a loopback protocol test server')
    args = p.parse_args()
    try:
        raw = sys.stdin.read(16385)
        if len(raw) > 16384: raise ValueError('Request is too large')
        print(json.dumps(transcribe(json.loads(raw), args)))
    except (ValueError, OSError, KeyError) as exc:
        print(str(exc), file=sys.stderr)
        raise SystemExit(1)


if __name__ == '__main__': main()
