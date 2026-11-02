#!/usr/bin/env python3
"""Deterministic, tool-free ACP endpoint for synthetic Twin Workbench tests."""
import json
import os
from pathlib import Path
import sys
import time

folder = Path(os.environ['TWIN_FIXTURE_DIR'])
folder.mkdir(parents=True, exist_ok=True)
mode = os.environ.get('TWIN_FIXTURE_MODE', 'normal')
session = 'fixture-session'
log = folder / f'{os.getpid()}.jsonl'


def emit(value):
    print(json.dumps({'jsonrpc': '2.0', **value}), flush=True)


def record(value):
    with log.open('a') as stream:
        stream.write(json.dumps(value) + '\n')


def read():
    line = sys.stdin.readline()
    if not line:
        raise SystemExit(0)
    value = json.loads(line)
    record(value)
    return value


def update(kind, text, sid=session):
    emit({'method': 'session/update', 'params': {'sessionId': sid, 'update': {
        'sessionUpdate': kind, 'content': {'type': 'text', 'text': text}}}})


PROFILE_RULES = {
    'communication': [
        ('voice', 'In a peer chat, use a brief conversational acknowledgment, then develop one practical point in a connected paragraph.'),
        ('phrasing', 'Use "Yeah, that tracks" when an idea lands; follow it with the specific part that clicked, rather than repeating agreement.'),
        ('structure', 'Put the proposed next move first. Add the reason in the next sentence and end with one concrete question when a choice is still open.'),
    ],
    'working': [
        ('framing', 'Before choosing an implementation, sketch the desired user experience and identify which assumptions a trial must resolve.'),
        ('decisions', 'Prefer a reversible trial when uncertainty is about how people will use the product; expand only after observing where they hesitate.'),
        ('collaboration', 'Give an owner a concrete outcome and room to choose the method. Revisit the boundary when ownership creates repeated handoffs.'),
    ],
}


while True:
    message = read()
    method = message.get('method')
    if method == 'initialize':
        result = {'protocolVersion': 1, 'agentInfo': {'name': 'synthetic-fixture'},
                  'agentCapabilities': {}}
    elif method == 'session/new':
        result = {'sessionId': session}
    elif method == 'session/prompt':
        prompt = '\n'.join(p.get('text', '') for p in message['params']['prompt'])
        (folder / f'ready-{os.getpid()}').write_text(prompt)
        if mode == 'hold':
            while not (folder / 'release').exists():
                time.sleep(.02)
        if mode == 'tool':
            emit({'method': 'session/update', 'params': {'sessionId': session,
                'update': {'sessionUpdate': 'tool_call', 'toolCallId': 't1',
                           'title': 'Synthetic prohibited tool attempt', 'status': 'in_progress'}}})
            time.sleep(30)
        if mode == 'callbacks':
            emit({'id': 'permission-fixture', 'method': 'session/request_permission', 'params': {
                'sessionId': session, 'toolCall': {'toolCallId': 'fictional'},
                'options': [{'optionId': 'yes', 'kind': 'allow_once', 'name': 'Allow'}]}})
            read()
            emit({'id': 'file-fixture', 'method': 'fs/read_text_file', 'params': {
                'sessionId': session, 'path': '/synthetic/not-a-real-file'}})
            read()
        if 'Evidence JSON:\n' in prompt:
            evidence = json.loads(prompt.split('Evidence JSON:\n', 1)[1])
            observations = []
            for collection in ('communication', 'working'):
                ids = [span['id'] for span in evidence if collection in span['collections']]
                if not ids:
                    continue
                target = ('working' if collection == 'communication' else 'communication') if mode == 'wrong_collection' else collection
                observation = {'collection': target,
                    'statement': 'Use concise, concrete language.' if collection == 'communication' else 'Compare evidence before making a decision.',
                    'scope': 'Synthetic workshop discussions',
                    'confidence': {'attribution': 'confirmed', 'support': 'limited',
                                   'generalization': 'narrow', 'reason': 'Only synthetic selected evidence was supplied.'},
                    'evidence_ids': ['nonexistent-evidence'] if mode == 'missing_evidence' else ids,
                    'conflicts': []}
                if mode == 'profiles':
                    for section, statement in PROFILE_RULES[collection]:
                        observations.append({**observation, 'section': section, 'statement': statement})
                elif mode == 'compile_overflow':
                    for index in range(12):
                        statement = f'Synthetic oversized instruction {index}. ' + ('Keep this synthetic paragraph intact. ' * 80)
                        observations.append({**observation, 'section': 'voice' if collection == 'communication' else 'framing', 'statement': statement[:2900]})
                elif mode == 'wrong_section':
                    observations.append({**observation, 'section': 'decisions' if collection == 'communication' else 'voice'})
                elif mode == 'invalid_section_type':
                    observations.append({**observation, 'section': {'unexpected': 'object'}})
                else:
                    # Omitting section exercises compatibility with older ACP extractors.
                    observations.append(observation)
            answer = json.dumps({'observations': observations})
        else:
            answer = 'SIMULATION: I would compare the available evidence and suggest a small reversible test.'
        update('agent_thought_chunk', 'HIDDEN-FIXTURE-REASONING')
        update('agent_message_chunk', 'WRONG-SESSION-FIXTURE', 'another-session')
        update('agent_message_chunk', answer)
        result = {'stopReason': 'end_turn'}
    else:
        result = {}
    if 'id' in message:
        emit({'id': message['id'], 'result': result})
