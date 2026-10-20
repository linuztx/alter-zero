#!/usr/bin/env python3
"""Offline, real-CLI code-generation RAM stress probe. Run in an activated shell.

python3 scripts/codegen_mem.py target/release/alter-zero --lines 800 --turns 4
A loopback Chat Completions stand-in streams TSX/CSS/JSON write calls; no API
keys are used and no generated code is executed. The CLI writes a throwaway
project, retains its transcript, opens both overlays, resizes, then clears.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def files(lines, turn):
    tsx = ['import React, { useMemo, useState } from "react";',
           'import "./App.css";',
           'type Task = { id: number; title: string; done: boolean; priority: number };',
           'export default function TaskManager() {',
           '  const [tasks, setTasks] = useState<Task[]>([]);',
           '  const [query, setQuery] = useState("");',
           '  const active = useMemo(() => tasks.filter(t => !t.done), [tasks]);',
           '  return <main className="task-manager"><aside>Inbox · Today · Upcoming</aside>',
           '    <section><h1>Todoist style task manager</h1>']
    for i in range(lines):
        tsx.append(f'      <button className="task row-{i}" onClick={{() => setQuery("task-{i}")}}>{{active.length}} Task {i} · version {turn}</button>')
    tsx += ['    </section></main>;', '}']
    css = ['.task-manager { display: grid; grid-template-columns: 240px 1fr; font: 14px system-ui; }',
           'aside { background: #faf8f6; padding: 24px; }',
           'section { max-width: 920px; padding: 32px; }']
    css += [f'.row-{i} {{ display: flex; padding: 12px; border-bottom: 1px solid #eeeeee; color: #334455; }}' for i in range(max(20, lines // 3))]
    manifest = {'name': 'todoist-memory-fixture', 'private': True, 'version': f'0.0.{turn}',
                'scripts': {'dev': 'vite', 'build': 'tsc -b && vite build'},
                'dependencies': {'react': '^19.0.0', 'react-dom': '^19.0.0'},
                'devDependencies': {'vite': '^6.0.0', 'typescript': '^5.0.0'}}
    return [('src/App.tsx', '\n'.join(tsx) + '\n'), ('src/App.css', '\n'.join(css) + '\n'), ('package.json', json.dumps(manifest, indent=2) + '\n')]


def memory(pid):
    status = Path(f'/proc/{pid}/status').read_text()
    result = {}
    for field, key in [('VmRSS', 'rss_kib'), ('VmHWM', 'peak_kib'), ('RssAnon', 'anon_kib'), ('RssFile', 'file_kib')]:
        match = re.search(rf'^{field}:\s+(\d+)', status, re.M)
        if match:
            result[key] = int(match.group(1))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--lines', type=int, default=800)
    parser.add_argument('--turns', type=int, default=4)
    parser.add_argument('--pace-ms', type=float, default=1)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--keep', action='store_true')
    args = parser.parse_args()
    if args.lines < 1 or args.turns < 1 or not math.isfinite(args.pace_ms) or args.pace_ms < 0:
        parser.error('--lines/--turns must be positive and --pace-ms nonnegative')
    if not Path('/proc/self/status').exists():
        parser.error('this resident-memory probe requires Linux /proc')
    if shutil.which('tmux') is None:
        parser.error('tmux must be available on PATH')
    binary = args.binary.resolve(strict=True)
    if not os.access(binary, os.X_OK):
        parser.error('binary is not executable')
    scratch = Path(__file__).resolve().parent.parent / 'work'
    scratch.mkdir(exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='codegen-memory-', dir=scratch))
    for folder in ['cfg', 'sessions', 'skills', 'agents', 'project/src']:
        (root / folder).mkdir(parents=True, exist_ok=True)
    # Keep the full stress history in context; a fixture must not launch a
    # summarization request once its generated source crosses the threshold.
    (root / 'cfg' / 'settings.json').write_text('{"auto_compact": false}\n')
    sock = str(root / 'tmux.sock')
    turn_counter = [0]
    requests = []

    class Stub(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def do_GET(self):
            body = b'{"object":"list","data":[{"id":"memory-fixture-model","context_length":1000000}]}'
            self.send_response(200)
            self.send_header('content-type', 'application/json')
            self.send_header('content-length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get('content-length') or 0))
            messages = json.loads(body)['messages']
            requests.append({'last_role': messages[-1]['role'],
                             'last_user_text': next((m.get('content') for m in reversed(messages) if m['role'] == 'user'), None)})
            self.send_response(200)
            self.send_header('content-type', 'text/event-stream')
            self.send_header('connection', 'close')
            self.end_headers()
            def frame(delta, finish=None):
                payload = {'choices': [{'delta': delta, 'finish_reason': finish}]}
                self.wfile.write(('data: ' + json.dumps(payload) + '\n\n').encode())
                self.wfile.flush()
            if messages[-1]['role'] == 'user':
                turn_counter[0] += 1
                turn = turn_counter[0]
                frame({'content': 'Building a task manager with an inbox, tasks, priorities, and filters.\n\n'})
                for index, (path, content) in enumerate(files(args.lines, turn)):
                    frame({'tool_calls': [{'index': index, 'id': f'fixture_{turn}_{index}', 'type': 'function',
                                           'function': {'name': 'write', 'arguments': ''}}]})
                    arguments = json.dumps({'path': str(root / 'project' / path), 'content': content})
                    for at in range(0, len(arguments), 256):
                        frame({'tool_calls': [{'index': index, 'function': {'arguments': arguments[at:at + 256]}}]})
                        if args.pace_ms:
                            time.sleep(args.pace_ms / 1000)
                frame({}, 'tool_calls')
            else:
                frame({'content': f'Task manager generation {turn_counter[0]} is complete.\n'})
                frame({}, 'stop')
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Stub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    providers = root / 'providers.toml'
    providers.write_text('[providers.fixture]\nname = "Memory fixture"\n[providers.fixture.kwargs]\napi_base = "http://127.0.0.1:%d/v1"\n' % server.server_port)
    overrides = {
        'NO_PROXY': '127.0.0.1', 'no_proxy': '127.0.0.1',
        'ALTER_ZERO_CONFIG_DIR': str(root / 'cfg'), 'ALTER_ZERO_SESSIONS_DIR': str(root / 'sessions'),
        'ALTER_ZERO_SKILLS_DIR': str(root / 'skills'), 'ALTER_ZERO_AGENTS_DIR': str(root / 'agents'),
        'ALTER_ZERO_PROVIDERS_FILE': str(providers), 'ALTER_ZERO_PROVIDER': 'fixture',
        'ALTER_ZERO_MODEL': 'memory-fixture-model', 'FIXTURE_API_KEY': 'stand-in',
        'ALTER_ZERO_HISTORY_FILE': '/dev/null', 'ALTER_ZERO_CHECKPOINTS': '0',
        'ALTER_ZERO_PROJECT_CONFIG': '0', 'ALTER_ZERO_PROJECT_DOC_MAX_BYTES': '0',
        'ALTER_ZERO_TELEMETRY': '0', 'ALTER_ZERO_UPDATE_CHECK': '0', 'ALTER_ZERO_TIPS': '0',
        'ALTER_ZERO_PERMISSIONS': '0', 'ALTER_ZERO_CONTEXT_WINDOW': '1000000',
    }
    # Isolate all app-specific overrides and model keys. Keep the runtime's
    # proxy, CA trust, and other managed environment variables untouched.
    inherited = {key: value for key, value in os.environ.items()
                 if not key.startswith(('ALTER_ZERO_', 'HERDR_'))
                 and not key.endswith('API_KEY')
                 and key not in {'OLLAMA_HOST', 'NO_COLOR', 'DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY'}}
    def tmux(*parts):
        proc = subprocess.run(['tmux', '-S', sock, '-f', '/dev/null', *map(str, parts)],
                              capture_output=True, text=True, env=inherited)
        if proc.returncode:
            raise RuntimeError(proc.stderr)
        return proc.stdout
    def pane():
        return tmux('capture-pane', '-t', 'memory', '-p', '-S', '-10000')
    def wait_for(check, timeout=120, interval=.05):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if check():
                return
            time.sleep(interval)
        raise RuntimeError('timed out\n' + pane())
    def summaries():
        total = 0
        for rollout in (root / 'sessions').rglob('*.jsonl'):
            for line in rollout.read_text().splitlines():
                try:
                    total += json.loads(line).get('type') == 'summary'
                except ValueError:
                    pass
        return total
    readings = []
    timings_ms = {}
    def sample(stage):
        reading = {'stage': stage, **memory(pid)}
        readings.append(reading)
        print(json.dumps(reading), flush=True)
    try:
        command = 'exec ' + shlex.join(['env', *[f'{key}={val}' for key, val in overrides.items()], str(binary)])
        tmux('new-session', '-d', '-s', 'memory', '-x', 120, '-y', 40, '-c', root / 'project', command)
        pid = int(tmux('display-message', '-p', '-t', 'memory', '#{pane_pid}').strip())
        wait_for(lambda: '❯' in pane(), 20)
        time.sleep(.3)
        sample('startup')
        for turn in range(1, args.turns + 1):
            prompt = f'Make a taskmanager like todoist. Revision {turn}.'
            tmux('send-keys', '-t', 'memory', '-l', prompt)
            wait_for(lambda: prompt in pane())
            time.sleep(.2)
            tmux('send-keys', '-t', 'memory', 'Enter')
            wait_for(lambda: summaries() >= turn)
            time.sleep(.5)
            for path, expected in files(args.lines, turn):
                actual = (root / 'project' / path).read_text()
                if actual != expected:
                    raise RuntimeError(f'generated file differs from fixture: {path}')
            sample(f'turn_{turn}')
        opened_at = time.monotonic()
        tmux('send-keys', '-t', 'memory', 'C-o')
        wait_for(lambda: 'T R A N S C R I P T' in pane(), interval=.005)
        timings_ms['transcript_open'] = round((time.monotonic() - opened_at) * 1000, 1)
        time.sleep(.3)
        sample('transcript_open')
        tmux('send-keys', '-t', 'memory', 'C-o')
        tmux('send-keys', '-t', 'memory', 'C-d')
        time.sleep(.5)
        sample('context_open')
        tmux('send-keys', '-t', 'memory', 'C-d')
        tmux('resize-window', '-t', 'memory', '-x', 90, '-y', 30)
        time.sleep(.7)
        sample('resize_90')
        tmux('resize-window', '-t', 'memory', '-x', 120, '-y', 40)
        time.sleep(.7)
        sample('resize_120')
        tmux('send-keys', '-t', 'memory', '-l', '/clear')
        wait_for(lambda: '/clear' in pane())
        time.sleep(.2)
        tmux('send-keys', '-t', 'memory', 'Enter')
        wait_for(lambda: 'Task manager generation' not in pane())
        time.sleep(.7)
        sample('clear')
        if turn_counter[0] != args.turns:
            raise RuntimeError(f'expected {args.turns} generation requests, got {turn_counter[0]}')
        digest = hashlib.sha256()
        with binary.open('rb') as executable:
            for chunk in iter(lambda: executable.read(65536), b''):
                digest.update(chunk)
        result = {'binary': str(binary), 'binary_sha256': digest.hexdigest(),
                  'lines': args.lines, 'turns': args.turns,
                  'workload': 'offline streamed TSX/CSS/JSON writes; generated code is not executed',
                  'files': [{'path': p, 'bytes': len(c.encode())} for p,c in files(args.lines, 1)],
                  'readings': readings, 'timings_ms': timings_ms, 'requests': requests}
        print(json.dumps({'timings_ms': timings_ms}), flush=True)
        if args.output:
            args.output.write_text(json.dumps(result, indent=2) + '\n')
    finally:
        subprocess.run(['tmux', '-S', sock, '-f', '/dev/null', 'kill-server'], capture_output=True, env=inherited)
        server.shutdown()
        server.server_close()
        if args.keep:
            (root / 'requests.json').write_text(json.dumps(requests, indent=2) + '\n')
            print(f'kept {root}', flush=True)
        else:
            shutil.rmtree(root)

if __name__ == '__main__':
    main()
