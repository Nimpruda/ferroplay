"""Tiny stdlib drivers for headless Chrome (DevTools protocol) and Firefox (Marionette).
Both offer: open(url), js(expr) -> value (promises awaited), key(k), close()."""
import base64
import contextlib
import json
import os
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import urllib.request


def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


KEY_CODES = {'ArrowRight': 39, 'ArrowLeft': 37, 'ArrowUp': 38, 'ArrowDown': 40, ' ': 32}  # Chrome's windowsVirtualKeyCode


class Chrome:
    def __init__(self, init_script=None):
        port, self.dir, self.id, self.logs, self.buf = free_port(), tempfile.mkdtemp(), 0, [], b''
        # Chrome won't run as root (containers) with its sandbox on.
        sandbox = ['--no-sandbox'] if os.geteuid() == 0 else []
        self.proc = subprocess.Popen([os.environ.get('CHROME', 'google-chrome'), *sandbox, '--headless=new',
            f'--remote-debugging-port={port}', '--autoplay-policy=no-user-gesture-required',
            f'--user-data-dir={self.dir}', 'about:blank'],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                pages = json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json'))
                break
            except OSError:
                time.sleep(0.2)
        path = next(p for p in pages if p['type'] == 'page')['webSocketDebuggerUrl'].split(str(port), 1)[1]
        self.sock = socket.create_connection(('127.0.0.1', port))
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(f'GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                          f'Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode())
        while b'\r\n\r\n' not in self.buf:
            self.buf += self.sock.recv(4096)
        self.buf = self.buf.split(b'\r\n\r\n', 1)[1]
        self.cdp('Runtime.enable')
        self.cdp('Network.enable')
        self.cdp('Page.enable')  # needed for addScriptToEvaluateOnNewDocument
        if init_script:
            self.cdp('Page.addScriptToEvaluateOnNewDocument', source=init_script)

    def _read(self, n):
        while len(self.buf) < n:
            self.buf += self.sock.recv(1 << 16)
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def _message(self):
        data = b''
        while True:
            b0, b1 = self._read(2)
            n = b1 & 0x7f
            n = struct.unpack('>H', self._read(2))[0] if n == 126 else struct.unpack('>Q', self._read(8))[0] if n == 127 else n
            data += self._read(n)
            if b0 & 0x80:
                return json.loads(data)

    def cdp(self, method, **params):
        self.id += 1
        data, mask = json.dumps({'id': self.id, 'method': method, 'params': params}).encode(), os.urandom(4)
        n = len(data)
        head = bytes([0x81]) + (bytes([0x80 | n]) if n < 126 else bytes([0xfe]) + struct.pack('>H', n) if n < 65536 else bytes([0xff]) + struct.pack('>Q', n))
        self.sock.sendall(head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))
        while True:
            m = self._message()
            if m.get('method') == 'Runtime.exceptionThrown':
                self.logs.append(m['params']['exceptionDetails'].get('text', '')[:300])
            if m.get('method') == 'Runtime.consoleAPICalled' and m['params']['type'] in ('error', 'warning'):
                text = ' '.join(str(a.get('value', a.get('description'))) for a in m['params']['args'])
                # The demo page uses the Tailwind browser build on purpose, which warns about it on every load.
                if not text.startswith('The browser build of Tailwind CSS'):
                    self.logs.append(text[:300])
            if m.get('id') == self.id:
                return m.get('result', m)

    def open(self, url):
        self.cdp('Page.navigate', url=url)

    def js(self, expr):
        r = self.cdp('Runtime.evaluate', expression=expr, awaitPromise=True, returnByValue=True)
        return r['result'].get('value')

    def key(self, k):
        for kind in ('keyDown', 'keyUp'):
            self.cdp('Input.dispatchKeyEvent', type=kind, key=k, text=k if kind == 'keyDown' and len(k) == 1 else '',
                     windowsVirtualKeyCode=KEY_CODES.get(k, ord(k.upper()[0])))

    def throttle(self, mbit):
        self.cdp('Network.emulateNetworkConditions', offline=False, latency=40,
                 downloadThroughput=mbit * 1e6 / 8, uploadThroughput=1e6)

    def close(self):
        self.proc.kill()
        self.proc.wait()
        shutil.rmtree(self.dir, ignore_errors=True)


class Firefox:
    def __init__(self):
        port, self.dir, self.id, self.logs, self.buf = free_port(), tempfile.mkdtemp(), 0, [], b''
        with open(f'{self.dir}/user.js', 'w') as prefs:
            prefs.write(f'user_pref("marionette.port", {port});\nuser_pref("media.autoplay.default", 0);\n')
        firefox = os.environ.get('FIREFOX') or ('/usr/lib/firefox/firefox' if os.path.exists('/usr/lib/firefox/firefox') else 'firefox')
        self.proc = subprocess.Popen([firefox, '--headless', '--marionette', '--no-remote', '--profile', self.dir],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                self.sock = socket.create_connection(('127.0.0.1', port))
                break
            except OSError:
                time.sleep(0.2)
        self._receive()  # greeting
        self.call('WebDriver:NewSession', capabilities={})

    def _receive(self):
        while b':' not in self.buf:
            self.buf += self.sock.recv(1 << 16)
        n, rest = self.buf.split(b':', 1)
        while len(rest) < int(n):
            rest += self.sock.recv(1 << 16)
        self.buf = rest[int(n):]
        return json.loads(rest[:int(n)])

    def call(self, name, **params):
        self.id += 1
        data = json.dumps([0, self.id, name, params]).encode()
        self.sock.sendall(str(len(data)).encode() + b':' + data)
        while True:
            m = self._receive()
            if m[1] == self.id:
                if m[2]:
                    raise RuntimeError(m[2])
                return m[3]

    def open(self, url):
        self.call('WebDriver:Navigate', url=url)

    def js(self, expr):
        script = f'const done = arguments[arguments.length - 1]; (async () => ({expr}))().then(done, e => done("error: " + e));'
        return self.call('WebDriver:ExecuteAsyncScript', script=script, args=[], scriptTimeout=60000)['value']

    def key(self, k):
        k = {'ArrowRight': '', 'ArrowLeft': '', 'ArrowUp': '', 'ArrowDown': ''}.get(k, k)
        self.call('WebDriver:PerformActions', actions=[{'type': 'key', 'id': 'kb', 'actions': [
            {'type': 'keyDown', 'value': k}, {'type': 'keyUp', 'value': k}]}])

    def close(self):
        # Best effort: the browser may already be gone (a connection error, or an error reply); it is killed below anyway.
        with contextlib.suppress(OSError, RuntimeError, ValueError):
            self.call('Marionette:Quit')
        self.proc.kill()
        self.proc.wait()
        shutil.rmtree(self.dir, ignore_errors=True)
