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

`python3 e2e/run.py` on this 12-core machine: **436 s** sequential, **78–106 s** now (8 workers, six runs,
all 48 checks passed, the same check names), **29 s** for `--quick` (13 checks).

- **Parallel scenarios did most of it.** One thread per scenario against the one server; each records its
  PASS/FAIL lines, printed per scenario at the end. The live stream (about 90 s: a 24 s window to fill, 30 s
  of play, the end) is the floor, so it starts first.
- **8 workers.** 14 (everything at once) was 95 s but flaked the 0.6 Mbit/s check; 8 was as fast in
  practice and held the timing checks (480p climb, 240p at 0.6 Mbit/s) in three runs.
- **Condition waits instead of sleeps** (35 s, 12 s, 20 s): wait on the very value the check reads
  (`state()`, which rounds `currentTime` to 0.1): a wait on the raw `currentTime > 30` passed at 30.04,
  then the check read `30` and failed.
- **A browser that dies must not hang the run.** The drivers spun forever on a closed socket (`recv`
  returning `b''`); they now raise, sockets time out at 120 s, and every browser a scenario starts is
  closed when it ends. `free_port()` never hands out the same port twice, as browsers start at once.
