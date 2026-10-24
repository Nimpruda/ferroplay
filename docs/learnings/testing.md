# Testing in real browsers without Node

`e2e/browser.py` drives headless Chrome (DevTools protocol over a raw WebSocket) and Firefox (Marionette)
with Python's standard library.

- **Firefox:** `--marionette --no-remote --profile <fresh dir>`. Without `--no-remote` it attaches to the
  Firefox you already have open.
- **Chrome:** `Page.enable` is needed before `Page.addScriptToEvaluateOnNewDocument` takes effect (we use it
  to hide built-in HLS and force the Rust engine).
- **Cross-origin without a second server:** serve the page from `127.0.0.1` and a link from `localhost`.
  That's two origins and one server, a real cross-origin test.
- **Quality switching** is tested with network throttling (`Network.emulateNetworkConditions`).

## A faster suite (2026-10-23)

`python3 e2e/run.py` on this 12-core machine: **436 s** sequential, **78–106 s** parallel (8 workers, six runs,
all 48 checks passed, the same check names), **64–68 s** after the cleanup below; `--quick` (13 checks) **29 s**,
then **23 s**.

- **Parallel scenarios did most of it.** One thread per scenario against the one server; each records its
  PASS/FAIL lines, printed per scenario as it ends. The live stream (about 60 s: a 24 s window to fill, 20 s
  of play, the end) is the floor, so it starts first; the rest go longest first, so the queue holds the short ones.
- **8 workers.** 14 (everything at once) was 95 s but flaked the 0.6 Mbit/s check; 8 was as fast in
  practice and held the timing checks (480p climb, 240p at 0.6 Mbit/s) in three runs.
- **Condition waits instead of sleeps** (35 s, 12 s, 20 s): wait on the very values the checks read, in one JS
  expression (`STATE`, which rounds `currentTime` to 0.1): a wait on the raw `currentTime > 30` passed at
  30.04, then the check read `30` and failed; a wait on time and reloads alone let the seekable-range check
  (read later) fail once. Every value a check compares is in its wait.
- **The live scenario's two browsers run side by side.** Chrome records the seekable range reaching the
  MediaSource while Firefox carries the playback checks; it used to start after them.
- **A browser that dies must not hang the run.** The drivers spun forever on a closed socket (`recv`
  returning `b''`); they now raise, and sockets time out at 120 s. Each driver is a context manager, so
  `with Chrome() as b:` closes it however the scenario ends.
- **Each browser picks its own port.** `--remote-debugging-port=0` / `marionette.port` 0, then the port is read
  from `DevToolsActivePort` / `MarionetteActivePort` in the profile. A port probed by the harness is free only
  until the browser binds it, a race once browsers start at once.
