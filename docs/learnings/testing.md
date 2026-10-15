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
