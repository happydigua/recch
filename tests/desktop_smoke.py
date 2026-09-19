"""Linux native WebView/IPC smoke test using the standard WebDriver HTTP API."""
import base64
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

assert os.environ.get('RECCH_INTEGRATION') == '1', 'Use disposable CI services only'
URL = 'http://127.0.0.1:4444'

def call(method, path, data=None):
    request = urllib.request.Request(URL + path, data=None if data is None else json.dumps(data).encode(),
                                     headers={'Content-Type': 'application/json'}, method=method)
    with urllib.request.urlopen(request, timeout=60) as response:
        result = json.load(response).get('value')
    if isinstance(result, dict) and result.get('error'):
        raise RuntimeError(result)
    return result

def wait_for(fn, seconds=30):
    end = time.monotonic() + seconds
    last = None
    while time.monotonic() < end:
        try:
            value = fn()
            if value:
                return value
        except (OSError, RuntimeError) as error:
            last = error
        time.sleep(.2)
    raise AssertionError(f'Condition not met: {last}')

with tempfile.TemporaryDirectory(prefix='recch-desktop-test-') as directory:
    env = {**os.environ, 'XDG_CONFIG_HOME': directory, 'WEBKIT_DISABLE_COMPOSITING_MODE': '1', 'LIBGL_ALWAYS_SOFTWARE': '1'}
    process = subprocess.Popen(['tauri-driver'], env=env, start_new_session=True)
    session = None
    try:
        wait_for(lambda: call('GET', '/status'))
        result = call('POST', '/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'wry', 'tauri:options': {'application': str(Path('src-tauri/target/debug/recch').resolve())}
        }}})
        session = result['sessionId']
        def execute(script, args=None, asynchronous=False):
            return call('POST', f'/session/{session}/execute/{"async" if asynchronous else "sync"}', {'script': script, 'args': args or []})
        wait_for(lambda: execute("return document.querySelectorAll('button').length >= 2"))
        assert execute("return document.body.innerText.length > 20")
        assert execute("return getComputedStyle(document.querySelector('button')).display") == 'inline-flex', 'Naive UI styles failed under CSP'
        assert not execute("window.__recchInlineExecuted = false; const s=document.createElement('script'); s.textContent='window.__recchInlineExecuted=true'; document.head.appendChild(s); return window.__recchInlineExecuted"), 'Inline scripts were not blocked'
        assert execute("const done=arguments[arguments.length-1]; window.__TAURI__.core.invoke('get_connections').then(v=>done(Array.isArray(v)), e=>done(false))", asynchronous=True)
        cfg = {'id': 'desktop-smoke', 'name': 'desktop-audit-fixture', 'db_type': 'postgresql', 'host': '127.0.0.1', 'port': 5432, 'username': 'postgres', 'password': 'integration-only', 'database': 'postgres'}
        assert execute("const done=arguments[arguments.length-1]; window.__TAURI__.core.invoke('save_connection',{config:arguments[0]}).then(()=>done(true),()=>done(false))", [cfg], True)
        execute('window.location.reload(); return true')
        wait_for(lambda: execute("return document.body.innerText.includes('desktop-audit-fixture')"))
        execute("const b=[...document.querySelectorAll('button')].find(b=>/^(Connect|连接)$/.test(b.innerText.trim())); if(!b) throw Error('Connect button not found'); b.click(); return true")
        wait_for(lambda: execute("return location.pathname.includes('/manage/desktop-smoke')"))
        wait_for(lambda: execute("return document.body.innerText.includes('desktop-audit-fixture')"))
        execute("const t=[...document.querySelectorAll('[role=tab]')].find(t=>/Info|信息/i.test(t.innerText)); if(!t) throw Error('Info tab not found'); t.click(); return true")
        wait_for(lambda: execute("return document.body.innerText.includes('127.0.0.1')"))
        assert not execute("return document.body.innerText.includes('integration-only')"), 'Password leaked in information view'
        Path('desktop-smoke.png').write_bytes(base64.b64decode(call('GET', f'/session/{session}/screenshot')))
        print('PASS: native page rendering, styles, CSP inline-script rejection, real IPC, config save/reload, route navigation, password masking')
    finally:
        if session:
            try:
                call('DELETE', f'/session/{session}')
            except OSError:
                pass
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        process.wait(timeout=10)
