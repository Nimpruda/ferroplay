#!/usr/bin/env python3
"""End-to-end test: real HLS (MPEG-TS) and links served by our server, in headless Firefox and
Chrome. Python stdlib only.

    ./build.sh && python3 e2e/run.py
"""
import glob, os, subprocess, sys, time, urllib.request
from urllib.parse import quote
from browser import Chrome, Firefox, free_port

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HIDE_NATIVE_HLS = """const canPlayType = HTMLMediaElement.prototype.canPlayType;
HTMLMediaElement.prototype.canPlayType = function (t) { return /mpegurl/i.test(t) ? '' : canPlayType.call(this, t); };"""
MP4 = 'https://download.blender.org/durian/trailer/sintel_trailer-480p.mp4'
V = "document.getElementById('main')"
failures = []


def check(name, ok, detail=''):
    print(f"{'PASS' if ok else 'FAIL'}  {name}  {detail}")
    if not ok:
        failures.append(name)


def until(b, condition, seconds):
    for _ in range(int(seconds * 4)):
        if b.js(condition):
            return True
        time.sleep(0.25)
    return False


def state(b):
    return b.js(f"(v => ({{ t: +v.currentTime.toFixed(1), paused: v.paused, width: v.videoWidth, "
                f"src: v.src.slice(0, 5), error: v.error && v.error.message }}))({V})")


def downloads(b, part):
    return b.js(f"performance.getEntriesByType('resource').map(e => e.name).filter(n => n.includes('{part}'))")


def qualities(b):
    return [n.split('/')[-2] for n in downloads(b, '.ts')]


def open_page(b, url, play=True):
    b.open(url)
    until(b, "document.querySelector('.vp')", 15)
    b.js(f"{V}.muted = true")
    if play:
        b.js("document.querySelector('.vp [data-do=toggle]').click()")


def converter():
    """Every segment of every quality, through the Rust converter, must decode cleanly in ffmpeg."""
    subprocess.run(['cargo', 'build', '-q', '-p', 'ts2mp4'], cwd=ROOT, check=True)
    tool = os.path.join(ROOT, 'target/debug/ts2mp4')
    segments = sorted(glob.glob(os.path.join(ROOT, 'web/hls/*/*.ts')))
    bad = []
    for path in segments:
        with open(path, 'rb') as ts:
            mp4 = subprocess.run([tool], stdin=ts, capture_output=True).stdout
        decode = subprocess.run(['ffmpeg', '-v', 'error', '-f', 'mp4', '-i', 'pipe:', '-f', 'null', '-'],
                                input=mp4, capture_output=True)
        if not mp4 or decode.returncode or decode.stderr:
            bad.append(os.path.relpath(path, ROOT))
    check('converter: every segment decodes', segments and not bad, f'{len(segments)} segments' + (f', bad: {bad}' if bad else ''))


def server_checks(url, other):
    types = {path: urllib.request.urlopen(url + path).headers['content-type'] for path in ('hls/master.m3u8', 'hls/240p/000.ts')}
    check('server: HLS content types', list(types.values()) == ['application/vnd.apple.mpegurl', 'video/mp2t'], types)
    master = urllib.request.urlopen(f'{url}proxy/{other}hls/master.m3u8').read().decode()
    uris = [line for line in master.splitlines() if line and not line.startswith('#')]
    check('server: proxy rewrites playlists through itself', uris and all(u.startswith(f'/proxy/{other}hls/') for u in uris), uris)


def playback(name, b, url, engine):
    open_page(b, url + '?plugins=keyboard,hls')
    playing = until(b, f"{V}.currentTime > 2", 20)
    s = state(b)
    via = 'blob' if engine else 'http'
    check(f'{name}: plays', playing and not s['paused'] and s['width'] > 0 and s['src'].startswith(via) and not s['error'], s)
    if engine:
        best = until(b, f"{V}.videoWidth === 854", 20)
        check(f'{name}: climbs to 480p on a fast network', best, f'qualities fetched: {qualities(b)}')
    b.js("document.querySelector('.vp').focus()")
    b.key('5')  # jump to 50% (26 s)
    landed = until(b, f"{V}.currentTime > 26.5 && {V}.currentTime < 32", 15)
    check(f'{name}: seeks to the middle and plays on', landed, state(b))
    b.js(f"{V}.currentTime = {V}.duration - 2")
    check(f'{name}: plays to the end', until(b, f"{V}.ended", 15), state(b))
    check(f'{name}: no errors', not b.logs and not state(b)['error'], b.logs)
    b.close()


def links(url, other):
    # A playlist on another origin without CORS headers: only playable through the proxy.
    b = Firefox()
    open_page(b, f"{url}?play={quote(other + 'hls/master.m3u8')}&plugins=hls", play=False)
    ok = until(b, f"{V}.currentTime > 2", 20)
    proxied = downloads(b, f'/proxy/{other}hls/')
    check('firefox: plays a cross-origin playlist link via the proxy', ok and len(proxied) > 2 and state(b)['src'] == 'blob:',
          f'{state(b)}, {len(proxied)} downloads via the proxy')

    # Plugin checkboxes rebuild the player in place, keeping the position (HLS reloads, then seeks back).
    b.js(f"{V}.currentTime = 20")
    until(b, f"{V}.currentTime > 21", 15)
    before = state(b)['t']
    b.js("document.querySelector('#plugins [name=speed]').click()")
    rebuilt = until(b, "document.querySelector('.vp [aria-label=\"Playback speed\"]')", 5)
    resumed = until(b, f"{V}.currentTime >= {before} && !{V}.paused", 15)
    check('firefox: plugin checkbox rebuilds the player and keeps the position', rebuilt and resumed,
          f"before {before}, after {state(b)}, url {b.js('location.search')}")
    b.close()

    # The paste box plays a direct MP4 link, and puts it in the page URL.
    b = Chrome()
    open_page(b, url, play=False)
    b.js(f"(f => {{ f.play.value = '{MP4}'; f.requestSubmit(); }})(document.getElementById('open'))")
    ok = until(b, f"{V}.currentTime > 1 && {V}.src === '{MP4}'", 20)
    check('chrome: paste box plays an MP4 link', ok and 'play=' in b.js('location.search'), state(b))
    check('chrome: no errors', not b.logs, b.logs)
    b.close()


def slow_network(url):
    b = Chrome(init_script=HIDE_NATIVE_HLS)
    b.throttle(0.6)
    open_page(b, url + '?plugins=keyboard,hls')
    time.sleep(20)
    fetched, s = qualities(b), state(b)
    check('chrome at 0.6 Mbit/s: stays on 240p and keeps playing', set(fetched) == {'240p'} and s['t'] > 8 and not s['paused'],
          f'qualities fetched: {fetched}, {s}')
    b.close()


def main():
    for needed in ('web/player.wasm', 'web/hls/master.m3u8'):
        if not os.path.exists(os.path.join(ROOT, needed)):
            sys.exit(f'missing {needed}: run ./build.sh (with ffmpeg installed) first')
    converter()

    port = free_port()
    server = subprocess.Popen(['cargo', 'run', '-q', '-p', 'server'], cwd=ROOT, env={**os.environ, 'PORT': str(port)})
    url, other = f'http://127.0.0.1:{port}/', f'http://localhost:{port}/'  # two origins, one server
    try:
        for _ in range(240):
            try:
                urllib.request.urlopen(url)
                break
            except OSError:
                time.sleep(0.5)
        server_checks(url, other)
        playback('firefox (Rust engine)', Firefox(), url, engine=True)
        playback('chrome (Rust engine)', Chrome(init_script=HIDE_NATIVE_HLS), url, engine=True)
        playback('chrome (built-in HLS)', Chrome(), url, engine=False)
        links(url, other)
        slow_network(url)
    finally:
        server.kill()

    print(f'\n{len(failures)} failed' if failures else '\nall passed')
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()
