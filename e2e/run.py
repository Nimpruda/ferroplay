#!/usr/bin/env python3
"""End-to-end test: real HLS (MPEG-TS) and links served by our server, in headless Firefox and
Chrome. Python stdlib only.

    ./build.sh && python3 e2e/run.py
"""
import contextlib
import glob
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from urllib.parse import quote

from browser import Chrome, Firefox, free_port

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RECORD_LIVE_RANGES = """(() => { const set = MediaSource.prototype.setLiveSeekableRange; window.liveRanges = [];
MediaSource.prototype.setLiveSeekableRange = function (start, end) { window.liveRanges.push([start, end]); return set.call(this, start, end); }; })();"""
HIDE_NATIVE_HLS = """const canPlayType = HTMLMediaElement.prototype.canPlayType;
HTMLMediaElement.prototype.canPlayType = function (t) { return /mpegurl/i.test(t) ? '' : canPlayType.call(this, t); };"""
MP4 = 'https://download.blender.org/durian/trailer/sintel_trailer-480p.mp4'
V = "document.getElementById('main')"
# Both browsers on the Rust engine: Chrome's own HLS hidden.
BROWSERS = (('firefox', Firefox), ('chrome', lambda: Chrome(init_script=HIDE_NATIVE_HLS)))
failures = []


def check(name, ok, detail=''):
    print(f"{'PASS' if ok else 'FAIL'}  {name}  {detail}")
    if not ok:
        failures.append(name)


def wait_for(condition, seconds, step=0.25):
    for _ in range(int(seconds / step)):
        if condition():
            return True
        time.sleep(step)
    return False


def until(b, condition, seconds):
    return wait_for(lambda: b.js(condition), seconds)


def seekable_end(b):
    return b.js(f"(v => v.seekable.length ? v.seekable.end(v.seekable.length - 1) : 0)({V})")


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


def paste(b, link):
    """Plays `link` from the page's paste box. A full URL: the box is a URL field, and refuses a relative one."""
    b.js(f"(f => {{ f.play.value = '{link}'; f.requestSubmit(); }})(document.getElementById('open'))")


def paste_mp4(b):
    """Plays the MP4 link through the paste box; true once it is playing."""
    paste(b, MP4)
    return until(b, f"{V}.currentTime > 1 && {V}.src === '{MP4}'", 20)


def seek_middle(b):
    """Presses 5 on the player (keyboard plugin: jump to 50%); true once it plays on from there (26 s in both videos)."""
    b.js("document.querySelector('.vp').focus()")
    b.key('5')
    return until(b, f"{V}.currentTime > 26.5 && {V}.currentTime < 32", 15)


def converter():
    """Every segment of every quality, through the Rust converter, must decode cleanly in ffmpeg."""
    subprocess.run(['cargo', 'build', '-q', '-p', 'ts2mp4'], cwd=ROOT, check=True)
    tool = os.path.join(ROOT, 'target/debug/ts2mp4')
    segments = sorted(glob.glob(os.path.join(ROOT, 'web/hls/[0-9]*p/*.ts')))  # the VOD qualities, not a live folder
    bad = []
    for path in segments:  # check=False: a failing segment lands in bad instead of stopping the run
        with open(path, 'rb') as ts:
            mp4 = subprocess.run([tool], stdin=ts, capture_output=True, check=False).stdout
        decode = subprocess.run(['ffmpeg', '-v', 'error', '-f', 'mp4', '-i', 'pipe:', '-f', 'null', '-'],
                                input=mp4, capture_output=True, check=False)
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
    open_page(b, url + '?plugins=keyboard,gaps,hls')
    playing = until(b, f"{V}.currentTime > 2", 20)
    s = state(b)
    via = 'blob' if engine else 'http'
    check(f'{name}: plays', playing and not s['paused'] and s['width'] > 0 and s['src'].startswith(via) and not s['error'], s)
    bar = b.js("(s => [+s.getPropertyValue('--pos'), +s.getPropertyValue('--buf')])(document.querySelector('.vp-seek').style)")
    check(f'{name}: the seek bar shows what is buffered ahead', 0 < bar[0] < bar[1] <= 1, f'position, buffered: {bar}')
    if engine:
        best = until(b, f"{V}.videoWidth === 854", 20)
        check(f'{name}: climbs to 480p on a fast network', best, f'qualities fetched: {qualities(b)}')
    check(f'{name}: seeks to the middle and plays on', seek_middle(b), state(b))
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
    check('chrome: paste box plays an MP4 link', paste_mp4(b) and 'play=' in b.js('location.search'), state(b))
    check('chrome: no errors', not b.logs, b.logs)
    b.close()


def core_build(url):
    """The page on player-core.wasm (no HLS): the paste box plays an MP4 link with every Rust plugin it has, and asking
    it for hls throws instead of quietly playing without it."""
    b = Chrome()
    try:
        open_page(b, f'{url}?core=1&plugins=keyboard,gaps,abLoop,loop,startAt', play=False)
        started, jumped = paste_mp4(b), seek_middle(b)
        wasm = [n.split('/')[-1] for n in downloads(b, '.wasm')]
        error = b.js("import('./player.js').then(m => { try { m.player(document.createElement('video'), { hls: 0 }); } "
                     "catch (e) { return e.message; } })")
        check('chrome, core build: the paste box plays an MP4 link, and the keyboard plugin works',
              started and jumped and wasm == ['player-core.wasm'] and not b.js("document.querySelector('#plugins [name=hls]')"),
              f'{state(b)}, wasm {wasm}')
        check('chrome, core build: use("hls") throws', error == 'plugin "hls" is not in this build', error)
        check('chrome, core build: no errors', not b.logs, b.logs)
    finally:
        b.close()


def live(url):
    """A live stream made by make-live.sh (no ENDLIST, segments created and deleted as it runs): playback starts
    at the hold-back, keeps going across playlist reloads, and ends normally once the stream is stopped. The
    stream gets its own folder, so one run by hand in web/hls/live is left alone."""
    live_dir = os.path.join(ROOT, 'web/hls/live-e2e')
    page = url + '?play=hls/live-e2e/live.m3u8&plugins=hls'
    shutil.rmtree(live_dir, ignore_errors=True)
    stream = subprocess.Popen([os.path.join(ROOT, 'make-live.sh')], cwd=ROOT, env={**os.environ, 'OUT': 'web/hls/live-e2e'},
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    browsers = []
    try:
        playlist = Path(live_dir, 'live.m3u8')
        # A full window (6 segments of 4 s), so the start is a real hold-back (3 × 4 s from the end), not 0.
        ready = wait_for(lambda: playlist.exists() and playlist.read_text().count('#EXTINF') >= 6, 45, 0.5)
        check('live: the stream writes a full window', ready)
        if not ready:
            return

        b = Firefox()
        browsers.append(b)
        open_page(b, page, play=False)
        # Errors after the page loads: uncaught ones, and rejected promises (a failed core call lands here).
        b.js("(() => { window.liveErrors = []; window.addEventListener('error', e => liveErrors.push(String(e.error ?? e.message)));"
             " window.addEventListener('unhandledrejection', e => liveErrors.push('rejected: ' + String(e.reason))); return 0; })()")
        started = until(b, f"{V}.currentTime > 2", 20)
        start, edge = state(b), seekable_end(b)
        check('live: starts at the hold-back, about 12 s from the live edge', started and 8 <= edge - start['t'] <= 16,
              f'{start}, seekable ends at {edge:.1f}')

        time.sleep(35)
        s = state(b)
        s['ended'] = b.js(f"{V}.ended")
        reloads = len(downloads(b, 'hls/live-e2e/live.m3u8'))
        recent = b.js("performance.getEntriesByType('resource').map(e => e.name.split('/').pop() + ' ' + Math.round(e.startTime) + ' ' + e.responseStatus).slice(-10)")
        check('live: keeps playing across playlist reloads', s['t'] > 30 and not s['paused'] and not s['ended'] and reloads >= 3,
              f'{s}, playlist requests: {reloads}, recent requests: {recent}')
        dur = b.js(f"String({V}.duration)")
        check('live: no fixed duration and no end while live', dur == 'Infinity', f'duration {dur}')
        later = seekable_end(b)
        check('live: the seekable range follows the playlist window', later > edge + 8, f'seekable end {edge:.1f} s at the start, {later:.1f} s later')
        check('live: no errors', not s['error'] and not b.js('window.liveErrors'), b.js('window.liveErrors'))

        # The range reaches the MediaSource: the calls are recorded in Chrome (Rust engine), while the stream is live.
        c = Chrome(init_script=HIDE_NATIVE_HLS + '\n' + RECORD_LIVE_RANGES)
        browsers.append(c)
        open_page(c, page, play=False)
        until(c, f"{V}.currentTime > 2", 20)
        time.sleep(12)
        ranges = c.js('window.liveRanges')
        ok = bool(ranges) and all(0 <= start <= end < 1e9 for start, end in ranges) and ranges[-1][1] > ranges[0][1]
        check('live: the window reaches the MediaSource, and it moves', ok, f'{len(ranges)} calls, first {ranges[:1]}, last {ranges[-1:]}')

        stream.send_signal(signal.SIGINT)  # the playlist gets ENDLIST: a finished stream
        stream.wait(timeout=30)
        check('live: stopping the stream ends the playlist', 'EXT-X-ENDLIST' in playlist.read_text())
        ended = until(b, f"{V}.ended", 60)
        check('live: plays to the end once the stream has ended', ended and not state(b)['error'], state(b))
    finally:
        for browser in browsers:
            browser.close()
        if stream.poll() is None:
            stream.send_signal(signal.SIGINT)
            try:
                stream.wait(timeout=30)
            except subprocess.TimeoutExpired:
                stream.kill()
                stream.wait()
        shutil.rmtree(live_dir, ignore_errors=True)


@contextlib.contextmanager
def served_playlist(name, playlist, fixtures=()):
    """Serves `playlist` (and copies of these fixtures) from web/hls/<name>/ while in use; yields its path."""
    folder = os.path.join(ROOT, 'web/hls', name)
    os.makedirs(folder, exist_ok=True)
    for fixture in fixtures:
        shutil.copy(os.path.join(ROOT, 'fixtures', fixture), folder)
    Path(folder, 'list.m3u8').write_text(playlist)
    try:
        yield f'hls/{name}/list.m3u8'
    finally:
        shutil.rmtree(folder, ignore_errors=True)


def unusable(url):
    """Segments that are not MPEG-TS (the playlist itself, here): each one is warned about, and after three the
    player stops and shows why, instead of a frozen frame."""
    playlist = '#EXTM3U\n#EXT-X-TARGETDURATION:2\n' + '#EXTINF:2,\n../master.m3u8\n' * 5 + '#EXT-X-ENDLIST\n'
    with served_playlist('unusable-e2e', playlist) as path:
        b = Chrome(init_script=HIDE_NATIVE_HLS)
        try:
            open_page(b, f'{url}?play={path}&plugins=hls', play=False)
            shown = until(b, "document.querySelector('.vp').dataset.error", 15)
            error = b.js("document.querySelector('.vp').dataset.error")
            check('chrome: unusable segments stop playback and say why on the player',
                  shown and 'no MPEG-TS media' in error, error)
            warnings = [log for log in b.logs if 'no MPEG-TS media' in log]
            check('chrome: each unusable segment is warned about in the console', len(warnings) >= 3, b.logs)
        finally:
            b.close()


def skipped_segment(url):
    """One segment that is not MPEG-TS (the master playlist, here) among the demo's 240p ones: it is warned about
    and skipped, and the playhead, which would wait at its hole forever, is moved past it and plays to the end."""
    segments = ('../240p/000.ts', '../master.m3u8', '../240p/002.ts')
    playlist = '#EXTM3U\n#EXT-X-TARGETDURATION:4\n' + ''.join(f'#EXTINF:4,\n{s}\n' for s in segments) + '#EXT-X-ENDLIST\n'
    with served_playlist('skipped-e2e', playlist) as path:
        for name, browser in BROWSERS:
            b = browser()
            try:
                open_page(b, f'{url}?plugins=gaps,hls', play=False)
                # Warnings are recorded in the page, as Firefox's console is out of reach; then the paste box plays the playlist.
                b.js("(() => { window.warnings = []; const warn = console.warn;"
                     " console.warn = (...args) => { warnings.push(args.join(' ')); warn(...args); }; return 0; })()")
                paste(b, url + path)
                ended, s, warnings = until(b, f"{V}.ended", 40), state(b), b.js('window.warnings')
                check(f'{name}: a VOD plays over the hole an unusable segment leaves, to the end', ended and not s['error'], s)
                check(f'{name}: the unusable segment is warned about as skipped', any('master.m3u8 skipped' in w for w in warnings), warnings)
            finally:
                b.close()


def fmp4_start_gap(url):
    """ffmpeg's fMP4 fixtures as a VOD: its first frame is at 66 ms (B-frames), and Media Source ignores the edit
    list that hides it, so the buffer starts after 0. With the gaps plugin it plays to the end everywhere."""
    playlist = ('#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:1\n#EXT-X-MAP:URI="fmp4-init.mp4"\n'
                '#EXTINF:1,\nfmp4-0.m4s\n#EXTINF:1,\nfmp4-1.m4s\n#EXT-X-ENDLIST\n')
    with served_playlist('fmp4-e2e', playlist, ('fmp4-init.mp4', 'fmp4-0.m4s', 'fmp4-1.m4s')) as path:
        for name, browser in BROWSERS:
            b = browser()
            try:
                open_page(b, f'{url}?play={path}&plugins=gaps,hls', play=False)
                ended, s = until(b, f"{V}.ended", 20), state(b)
                check(f'{name}: an fMP4 stream whose buffer starts after 0 plays to the end', ended and not s['error'] and not b.logs,
                      f'{s}, {b.logs}')
            finally:
                b.close()


def slow_network(url):
    b = Chrome(init_script=HIDE_NATIVE_HLS)
    b.throttle(0.6)
    open_page(b, url + '?plugins=keyboard,gaps,hls')
    time.sleep(20)
    fetched, s = qualities(b), state(b)
    check('chrome at 0.6 Mbit/s: stays on 240p and keeps playing', set(fetched) == {'240p'} and s['t'] > 8 and not s['paused'],
          f'qualities fetched: {fetched}, {s}')
    b.close()


def main():
    for needed in ('web/player.wasm', 'web/player-core.wasm', 'web/hls/master.m3u8'):
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
        live(url)
        unusable(url)
        skipped_segment(url)
        fmp4_start_gap(url)
        links(url, other)
        core_build(url)
        slow_network(url)
    finally:
        server.kill()

    print(f'\n{len(failures)} failed' if failures else '\nall passed')
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()
