#!/usr/bin/env python3
"""End-to-end test: real HLS (MPEG-TS) and links served by our server, in headless Firefox and
Chrome. Python stdlib only.

    ./build.sh && python3 e2e/run.py
"""
import glob, os, shutil, signal, subprocess, sys, time, urllib.request
from urllib.parse import quote
from browser import Chrome, Firefox, free_port

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RECORD_LIVE_RANGES = """(() => { const set = MediaSource.prototype.setLiveSeekableRange; window.liveRanges = [];
MediaSource.prototype.setLiveSeekableRange = function (start, end) { window.liveRanges.push([start, end]); return set.call(this, start, end); }; })();"""
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


def stop_live_streams():
    """A stream still writing into web/hls/live would race the converter check and the live test."""
    subprocess.run(['pkill', '-INT', '-x', 'ffmpeg'])
    time.sleep(2)


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


def live(url):
    """A live stream made by make-live.sh (no ENDLIST, segments created and deleted as it runs): playback starts
    near the live edge, keeps going across playlist reloads, and ends normally once the stream is stopped."""
    stop_live_streams()
    live_dir = os.path.join(ROOT, 'web/hls/live')
    shutil.rmtree(live_dir, ignore_errors=True)
    stream = subprocess.Popen([os.path.join(ROOT, 'make-live.sh')], cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        playlist = os.path.join(live_dir, 'live.m3u8')
        ready = wait_for(lambda: os.path.exists(playlist) and open(playlist).read().count('#EXTINF') >= 2, 30)
        check('live: the stream writes a playlist', ready)
        if not ready:
            return

        b = Firefox()
        open_page(b, url + '?play=hls/live/live.m3u8&plugins=hls', play=False)
        # Errors after the page loads: uncaught ones, and rejected promises (a failed core call lands here).
        b.js("(() => { window.liveErrors = []; window.addEventListener('error', e => liveErrors.push(String(e.error ?? e.message)));"
             " window.addEventListener('unhandledrejection', e => liveErrors.push('rejected: ' + String(e.reason))); return 0; })()")
        started = until(b, f"{V}.currentTime > 2", 20)
        start = state(b)
        seekable_end = b.js(f"(v => v.seekable.length ? v.seekable.end(v.seekable.length - 1) : 0)({V})")
        check('live: starts near the live edge', started and seekable_end - start['t'] < 16, f'{start}, seekable ends at {seekable_end:.1f}')

        time.sleep(35)
        s = state(b)
        reloads = len(downloads(b, 'hls/live/live.m3u8'))
        dur = b.js(f"String({V}.duration)")
        recent = b.js("performance.getEntriesByType('resource').map(e => e.name.split('/').pop() + ' ' + Math.round(e.startTime) + ' ' + e.responseStatus).slice(-10)")
        s['ended'] = b.js(f"{V}.ended")
        check('live: keeps playing across playlist reloads', s['t'] > 30 and not s['paused'] and not s['ended'] and reloads >= 3,
              f'{s}, playlist requests: {reloads}, recent requests: {recent}')
        check('live: no fixed duration and no end while live', dur == 'Infinity', f'duration {dur}')
        seekable_later = b.js(f"(v => v.seekable.length ? v.seekable.end(v.seekable.length - 1) : 0)({V})")
        check('live: the seekable range follows the playlist window', seekable_later > seekable_end + 8,
              f'seekable end {seekable_end:.1f} s at the start, {seekable_later:.1f} s later')
        check('live: no errors', not s['error'] and not b.js('window.liveErrors'), b.js('window.liveErrors'))

        # The range reaches the MediaSource: the calls are recorded in Chrome (Rust engine), while the stream is live.
        c = Chrome(init_script=HIDE_NATIVE_HLS + '\n' + RECORD_LIVE_RANGES)
        open_page(c, url + '?play=hls/live/live.m3u8&plugins=hls', play=False)
        until(c, f"{V}.currentTime > 2", 20)
        time.sleep(12)
        ranges = c.js('window.liveRanges')
        ok = bool(ranges) and all(0 <= start <= end and end < 1e9 for start, end in ranges) and ranges[-1][1] > ranges[0][1]
        check('live: the window reaches the MediaSource, and it moves', ok, f'{len(ranges)} calls, first {ranges[:1]}, last {ranges[-1:]}')
        c.close()

        stream.send_signal(signal.SIGINT)  # the playlist gets ENDLIST: a finished stream
        stream.wait(timeout=30)
        check('live: stopping the stream ends the playlist', 'EXT-X-ENDLIST' in open(playlist).read())
        ended = until(b, f"{V}.ended", 60)
        check('live: plays to the end once the stream has ended', ended and not state(b)['error'], state(b))
        b.close()
    finally:
        if stream.poll() is None:
            stream.send_signal(signal.SIGINT)
            stream.wait(timeout=30)


def wait_for(condition, seconds):
    for _ in range(int(seconds * 2)):
        if condition():
            return True
        time.sleep(0.5)
    return False


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
    stop_live_streams()
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
        live(url)
        links(url, other)
        slow_network(url)
    finally:
        server.kill()

    print(f'\n{len(failures)} failed' if failures else '\nall passed')
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()
